use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TG_PROXY_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Flowseal/tg-ws-proxy/releases/latest";
const TG_PROXY_ASSET_NAME: &str = "TgWsProxy_windows.exe";
const TG_PROXY_VERSION_FILE_NAME: &str = "TgWsProxy_windows.version.json";
const USER_AGENT: &str = "zapret-hub-rs/0.1";

#[derive(Clone, Debug)]
pub(crate) struct TelegramProxyRelease {
    pub(crate) tag: String,
    pub(crate) release_url: String,
    pub(crate) asset_url: String,
    pub(crate) digest: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TelegramProxyUpdateStatus {
    pub(crate) installed_tag: Option<String>,
    pub(crate) latest: TelegramProxyRelease,
    pub(crate) update_available: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct InstalledTelegramProxyVersion {
    tag: String,
    release_url: String,
    asset_url: String,
    digest: Option<String>,
}

pub(crate) fn check_for_update(bundle_path: &Path) -> Result<TelegramProxyUpdateStatus> {
    let latest = fetch_latest_release()?;
    let installed_tag = read_installed_version(bundle_path)?.map(|version| version.tag);
    let update_available = installed_tag.as_deref() != Some(latest.tag.as_str());

    Ok(TelegramProxyUpdateStatus {
        installed_tag,
        latest,
        update_available,
    })
}

pub(crate) fn install_update(bundle_path: &Path, release: &TelegramProxyRelease) -> Result<String> {
    fs::create_dir_all(bundle_path)
        .with_context(|| format!("failed to create {}", bundle_path.display()))?;

    let client = http_client()?;
    let bytes = client
        .get(&release.asset_url)
        .send()
        .context("failed to request tg-ws-proxy asset")?
        .error_for_status()
        .context("tg-ws-proxy download returned an error status")?
        .bytes()
        .context("failed to read tg-ws-proxy response body")?;

    if let Some(expected_digest) = &release.digest {
        verify_digest(bytes.as_ref(), expected_digest)?;
    }

    let target_path = bundle_path.join(TG_PROXY_ASSET_NAME);
    let temp_path = target_path.with_extension("download");
    let old_path = target_path.with_extension("old");

    fs::write(&temp_path, bytes.as_ref())
        .with_context(|| format!("failed to write {}", temp_path.display()))?;

    if old_path.exists() {
        let _ = fs::remove_file(&old_path);
    }

    if target_path.exists() {
        fs::rename(&target_path, &old_path)
            .with_context(|| format!("failed to move {}", target_path.display()))?;
    }

    fs::rename(&temp_path, &target_path)
        .with_context(|| format!("failed to replace {}", target_path.display()))?;

    let _ = fs::remove_file(&old_path);
    write_installed_version(bundle_path, release)?;

    Ok(format!("Telegram WS proxy обновлён до {}.", release.tag))
}

fn fetch_latest_release() -> Result<TelegramProxyRelease> {
    let release: GitHubRelease = http_client()?
        .get(TG_PROXY_RELEASE_API_URL)
        .send()
        .context("failed to request tg-ws-proxy release metadata")?
        .error_for_status()
        .context("tg-ws-proxy release metadata returned an error status")?
        .json()
        .context("failed to parse tg-ws-proxy release metadata")?;

    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == TG_PROXY_ASSET_NAME)
        .ok_or_else(|| anyhow!("tg-ws-proxy windows asset not found in latest release"))?;

    Ok(TelegramProxyRelease {
        tag: release.tag_name,
        release_url: release.html_url,
        asset_url: asset.browser_download_url,
        digest: asset.digest,
    })
}

fn read_installed_version(bundle_path: &Path) -> Result<Option<InstalledTelegramProxyVersion>> {
    let version_path = version_file_path(bundle_path);
    if !version_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&version_path)
        .with_context(|| format!("failed to read {}", version_path.display()))?;
    let version = match serde_json::from_str(content.trim_start_matches('\u{feff}')) {
        Ok(version) => version,
        Err(_) => return Ok(None),
    };

    Ok(Some(version))
}

fn write_installed_version(bundle_path: &Path, release: &TelegramProxyRelease) -> Result<()> {
    let version = InstalledTelegramProxyVersion {
        tag: release.tag.clone(),
        release_url: release.release_url.clone(),
        asset_url: release.asset_url.clone(),
        digest: release.digest.clone(),
    };
    let content =
        serde_json::to_string_pretty(&version).context("failed to serialize tg proxy version")?;

    fs::write(version_file_path(bundle_path), content)
        .context("failed to write tg proxy version sidecar")
}

fn verify_digest(bytes: &[u8], expected_digest: &str) -> Result<()> {
    let expected_hash = expected_digest
        .strip_prefix("sha256:")
        .unwrap_or(expected_digest)
        .to_ascii_lowercase();
    let actual_hash = format!("{:x}", Sha256::digest(bytes));

    if actual_hash != expected_hash {
        anyhow::bail!("tg-ws-proxy digest mismatch");
    }

    Ok(())
}

fn version_file_path(bundle_path: &Path) -> PathBuf {
    bundle_path.join(TG_PROXY_VERSION_FILE_NAME)
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[ignore = "hits GitHub and downloads the live TgWsProxy_windows.exe asset"]
    fn live_check_and_install_update_replaces_proxy() -> Result<()> {
        let bundle_path = temp_bundle_path();
        let _cleanup = TempBundleCleanup(bundle_path.clone());

        fs::create_dir_all(&bundle_path)?;
        fs::write(bundle_path.join(TG_PROXY_ASSET_NAME), b"old proxy binary")?;
        fs::write(
            version_file_path(&bundle_path),
            r#"{
  "tag": "v0.0.0",
  "release_url": "https://example.invalid/old",
  "asset_url": "https://example.invalid/old.exe",
  "digest": null
}"#,
        )?;

        let status = check_for_update(&bundle_path)?;
        assert_eq!(status.installed_tag.as_deref(), Some("v0.0.0"));
        assert!(status.update_available);

        let message = install_update(&bundle_path, &status.latest)?;
        assert!(message.contains(&status.latest.tag));

        let installed = read_installed_version(&bundle_path)?.expect("version sidecar exists");
        assert_eq!(installed.tag, status.latest.tag);
        assert!(bundle_path.join(TG_PROXY_ASSET_NAME).metadata()?.len() > 1_000_000);
        assert!(!bundle_path.join("TgWsProxy_windows.old").exists());

        Ok(())
    }

    #[test]
    fn read_installed_version_accepts_utf8_bom() -> Result<()> {
        let bundle_path = temp_bundle_path();
        let _cleanup = TempBundleCleanup(bundle_path.clone());

        fs::create_dir_all(&bundle_path)?;
        fs::write(
            version_file_path(&bundle_path),
            "\u{feff}{\"tag\":\"v1.6.0\",\"release_url\":\"https://example.test/release\",\"asset_url\":\"https://example.test/proxy.exe\",\"digest\":null}",
        )?;

        let version = read_installed_version(&bundle_path)?.expect("version parses");
        assert_eq!(version.tag, "v1.6.0");

        Ok(())
    }

    #[test]
    fn read_installed_version_ignores_invalid_sidecar() -> Result<()> {
        let bundle_path = temp_bundle_path();
        let _cleanup = TempBundleCleanup(bundle_path.clone());

        fs::create_dir_all(&bundle_path)?;
        fs::write(version_file_path(&bundle_path), b"not json")?;

        assert!(read_installed_version(&bundle_path)?.is_none());

        Ok(())
    }

    fn temp_bundle_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after unix epoch")
            .as_nanos();
        let process_id = std::process::id();
        env::temp_dir().join(format!(
            "zapret-hub-tg-proxy-update-test-{process_id}-{stamp}"
        ))
    }

    struct TempBundleCleanup(PathBuf);

    impl Drop for TempBundleCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
