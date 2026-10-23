use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::core::github::{self, DownloadProgress, ReleaseAsset};

const TG_PROXY_REPO: &str = "Flowseal/tg-ws-proxy";
pub(crate) const TG_PROXY_ASSET_NAME: &str = "TgWsProxy_windows.exe";
pub(crate) const TG_PROXY_VERSION_FILE_NAME: &str = "TgWsProxy_windows.version.json";

#[derive(Clone, Debug)]
pub(crate) struct TelegramProxyRelease {
    pub(crate) tag: String,
    pub(crate) release_url: String,
    pub(crate) asset: ReleaseAsset,
}

#[derive(Clone, Debug)]
pub(crate) struct TelegramProxyUpdateStatus {
    pub(crate) installed_tag: Option<String>,
    pub(crate) latest: TelegramProxyRelease,
    pub(crate) update_available: bool,
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

pub(crate) fn install_update(
    bundle_path: &Path,
    release: &TelegramProxyRelease,
    progress: &DownloadProgress,
) -> Result<String> {
    fs::create_dir_all(bundle_path)
        .with_context(|| format!("failed to create {}", bundle_path.display()))?;

    let bytes = github::download(&release.asset, progress)?;

    let target_path = bundle_path.join(TG_PROXY_ASSET_NAME);
    let temp_path = target_path.with_extension("download");
    let old_path = target_path.with_extension("old");

    fs::write(&temp_path, &bytes)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;

    if old_path.exists() {
        let _ = fs::remove_file(&old_path);
    }

    if target_path.exists() {
        fs::rename(&target_path, &old_path).with_context(|| {
            format!(
                "не удалось заменить {}: файл занят, остановите Telegram proxy",
                target_path.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(&temp_path, &target_path) {
        let _ = fs::rename(&old_path, &target_path);
        return Err(error).with_context(|| format!("failed to replace {}", target_path.display()));
    }

    let _ = fs::remove_file(&old_path);
    write_installed_version(bundle_path, release)?;

    Ok(format!("Telegram WS proxy обновлён до {}.", release.tag))
}

fn fetch_latest_release() -> Result<TelegramProxyRelease> {
    let release = github::latest_release(TG_PROXY_REPO)?;
    let asset = release
        .asset(TG_PROXY_ASSET_NAME)
        .ok_or_else(|| anyhow!("в релизе {} нет {TG_PROXY_ASSET_NAME}", release.tag))?;

    Ok(TelegramProxyRelease {
        tag: release.tag,
        release_url: release.html_url,
        asset,
    })
}

fn read_installed_version(bundle_path: &Path) -> Result<Option<InstalledTelegramProxyVersion>> {
    let version_path = version_file_path(bundle_path);
    if !version_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&version_path)
        .with_context(|| format!("failed to read {}", version_path.display()))?;
    Ok(serde_json::from_str(content.trim_start_matches('\u{feff}')).ok())
}

fn write_installed_version(bundle_path: &Path, release: &TelegramProxyRelease) -> Result<()> {
    let version = InstalledTelegramProxyVersion {
        tag: release.tag.clone(),
        release_url: release.release_url.clone(),
        asset_url: release.asset.url.clone(),
        digest: release.asset.digest.clone(),
    };
    let content =
        serde_json::to_string_pretty(&version).context("failed to serialize tg proxy version")?;

    fs::write(version_file_path(bundle_path), content)
        .context("failed to write tg proxy version sidecar")
}

fn version_file_path(bundle_path: &Path) -> PathBuf {
    bundle_path.join(TG_PROXY_VERSION_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

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
