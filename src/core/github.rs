//! GitHub release lookup and downloads shared by the bundle and Telegram proxy updaters.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const USER_AGENT: &str = concat!("zapret-hub-rs/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub(crate) struct ReleaseAsset {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) digest: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Release {
    pub(crate) repo: &'static str,
    pub(crate) tag: String,
    pub(crate) html_url: String,
    /// `None` when the tag came from the web fallback and the asset list is unknown.
    assets: Option<Vec<ReleaseAsset>>,
}

impl Release {
    /// Finds an asset by exact name. Without the API asset list the conventional
    /// download URL is assumed (no digest available then).
    pub(crate) fn asset(&self, name: &str) -> Option<ReleaseAsset> {
        match &self.assets {
            Some(assets) => assets
                .iter()
                .find(|asset| asset.name.eq_ignore_ascii_case(name))
                .cloned(),
            None => Some(ReleaseAsset {
                name: name.to_owned(),
                url: format!(
                    "https://github.com/{}/releases/download/{}/{name}",
                    self.repo, self.tag
                ),
                digest: None,
            }),
        }
    }
}

/// Bytes received / total bytes (0 while unknown). Shared with the UI thread.
#[derive(Clone, Debug, Default)]
pub(crate) struct DownloadProgress {
    received: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl DownloadProgress {
    pub(crate) fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.received.load(Ordering::Relaxed) as f32 / total as f32)
    }

    fn reset(&self, total: Option<u64>) {
        self.received.store(0, Ordering::Relaxed);
        self.total.store(total.unwrap_or(0), Ordering::Relaxed);
    }
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

pub(crate) fn latest_release(repo: &'static str) -> Result<Release> {
    match release_from_api(repo, "latest") {
        Ok(release) => Ok(release),
        Err(api_error) => latest_release_from_web(repo).map_err(|web_error| {
            anyhow!(
                "{api_error:#}; запасная проверка через github.com тоже не удалась: {web_error:#}"
            )
        }),
    }
}

/// A pinned release (used by the packaging scripts). Falls back to conventional asset URLs.
pub(crate) fn release_by_tag(repo: &'static str, tag: &str) -> Result<Release> {
    release_from_api(repo, &format!("tags/{tag}")).or_else(|_| {
        Ok(Release {
            repo,
            tag: tag.to_owned(),
            html_url: format!("https://github.com/{repo}/releases/tag/{tag}"),
            assets: None,
        })
    })
}

fn release_from_api(repo: &'static str, which: &str) -> Result<Release> {
    let response = http_client()?
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/{which}"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("GitHub API недоступен")?;

    let status = response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("GitHub API временно ограничил запросы (лимит 60 в час)");
    }
    let release: ApiRelease = response
        .error_for_status()
        .context("GitHub API вернул ошибку")?
        .json()
        .context("не удалось разобрать ответ GitHub API")?;

    Ok(Release {
        repo,
        tag: release.tag_name,
        html_url: release.html_url,
        assets: Some(
            release
                .assets
                .into_iter()
                .map(|asset| ReleaseAsset {
                    name: asset.name,
                    url: asset.browser_download_url,
                    digest: asset.digest,
                })
                .collect(),
        ),
    })
}

/// `github.com/<repo>/releases/latest` redirects to `.../releases/tag/<tag>` and is not rate limited.
fn latest_release_from_web(repo: &'static str) -> Result<Release> {
    ensure_tls_provider();
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build HTTP client")?;
    let response = client
        .get(format!("https://github.com/{repo}/releases/latest"))
        .send()
        .context("github.com недоступен")?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow!("github.com не вернул ссылку на релиз"))?;
    let tag = tag_from_release_url(location)
        .ok_or_else(|| anyhow!("не удалось определить тег релиза из {location}"))?;

    Ok(Release {
        repo,
        html_url: location.to_owned(),
        tag,
        assets: None,
    })
}

fn tag_from_release_url(url: &str) -> Option<String> {
    let (_, tag) = url.split_once("/releases/tag/")?;
    let tag = tag.split(['?', '#', '/']).next()?.trim();
    (!tag.is_empty()).then(|| tag.to_owned())
}

pub(crate) fn download(asset: &ReleaseAsset, progress: &DownloadProgress) -> Result<Vec<u8>> {
    let mut response = http_client()?
        .get(&asset.url)
        .send()
        .with_context(|| format!("не удалось скачать {}", asset.name))?
        .error_for_status()
        .with_context(|| format!("сервер не отдал {}", asset.name))?;

    progress.reset(response.content_length());
    let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0) as usize);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .with_context(|| format!("загрузка {} прервалась", asset.name))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        progress.received.fetch_add(read as u64, Ordering::Relaxed);
    }

    if let Some(expected) = &asset.digest {
        verify_digest(&bytes, expected)
            .with_context(|| format!("{} повреждён при загрузке", asset.name))?;
    }
    Ok(bytes)
}

pub(crate) fn verify_digest(bytes: &[u8], expected_digest: &str) -> Result<()> {
    let expected_hash = expected_digest
        .strip_prefix("sha256:")
        .unwrap_or(expected_digest)
        .to_ascii_lowercase();
    let actual_hash = hex(&Sha256::digest(bytes));

    if actual_hash != expected_hash {
        anyhow::bail!("контрольная сумма SHA-256 не совпала");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Windows uses schannel; the Linux dev build needs a rustls crypto provider installed once.
fn ensure_tls_provider() {
    #[cfg(not(windows))]
    {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}

fn http_client() -> Result<Client> {
    ensure_tls_provider();
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .context("failed to build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hits github.com"]
    fn live_web_fallback_finds_latest_tag() {
        let release = latest_release_from_web("Flowseal/zapret-discord-youtube").unwrap();
        assert!(!release.tag.is_empty());
        assert!(release.asset("x.zip").unwrap().url.contains(&release.tag));
    }

    #[test]
    fn tag_is_parsed_from_release_redirect() {
        assert_eq!(
            tag_from_release_url(
                "https://github.com/Flowseal/zapret-discord-youtube/releases/tag/1.10.4"
            )
            .as_deref(),
            Some("1.10.4")
        );
        assert_eq!(
            tag_from_release_url("https://github.com/x/y/releases"),
            None
        );
    }

    #[test]
    fn web_fallback_builds_conventional_asset_url() {
        let release = Release {
            repo: "Flowseal/tg-ws-proxy",
            tag: "v1.11.2".to_owned(),
            html_url: String::new(),
            assets: None,
        };
        let asset = release.asset("TgWsProxy_windows.exe").unwrap();
        assert_eq!(
            asset.url,
            "https://github.com/Flowseal/tg-ws-proxy/releases/download/v1.11.2/TgWsProxy_windows.exe"
        );
        assert!(asset.digest.is_none());
    }

    #[test]
    fn digest_check_accepts_prefixed_lowercase_hash() {
        let digest = format!("sha256:{}", hex(&Sha256::digest(b"abc")));
        assert!(verify_digest(b"abc", &digest).is_ok());
        assert!(verify_digest(b"abd", &digest).is_err());
    }
}
