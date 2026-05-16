use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::bundle_metadata::detect_bundle_version;
use crate::core::paths::is_valid_bundle_dir;
use crate::core::tg_proxy_update::{
    check_for_update as check_tg_proxy_update, install_update as install_tg_proxy_update,
};

const BUNDLE_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Flowseal/zapret-discord-youtube/releases/latest";
const USER_AGENT: &str = "zapret-hub-rs/0.1";
const BUNDLE_VERSION_FILE_NAME: &str = "ZapretBundle.version.json";

const GITHUB_HOSTLIST_ENTRIES: &[&str] = &[
    "github.com",
    "www.github.com",
    "gist.github.com",
    "api.github.com",
    "github.githubassets.com",
    "githubassets.com",
    "githubusercontent.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "avatars.githubusercontent.com",
    "camo.githubusercontent.com",
    "user-images.githubusercontent.com",
];

const PRESERVED_RELATIVE_FILES: &[&str] = &[
    r"lists\list-general-user.txt",
    r"lists\list-exclude-user.txt",
    r"lists\ipset-exclude-user.txt",
    "tgproxy-runtime.log",
    "tgproxy-launch.log",
];

#[derive(Clone, Debug)]
pub(crate) struct BundleRelease {
    pub(crate) tag: String,
    pub(crate) release_url: String,
    pub(crate) asset_url: String,
    pub(crate) asset_name: String,
    pub(crate) digest: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct BundleUpdateStatus {
    pub(crate) installed_version: Option<String>,
    pub(crate) latest: BundleRelease,
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

#[derive(Debug, Serialize)]
struct BundleVersionFile<'a> {
    version: &'a str,
    source_folder: String,
}

pub(crate) fn check_for_update(bundle_path: &Path) -> Result<BundleUpdateStatus> {
    let latest = fetch_latest_release()?;
    let installed_version = detect_bundle_version(bundle_path);
    let update_available = installed_version.as_deref() != Some(latest.tag.as_str());

    Ok(BundleUpdateStatus {
        installed_version,
        latest,
        update_available,
    })
}

pub(crate) fn install_update(bundle_path: &Path, release: &BundleRelease) -> Result<String> {
    let work_dir = unique_work_dir()?;
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let result = install_update_in_work_dir(bundle_path, release, &work_dir);
    let cleanup_result = fs::remove_dir_all(&work_dir);

    match (result, cleanup_result) {
        (Ok(message), _) => Ok(message),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            Err(error).with_context(|| format!("also failed to remove {}", cleanup_error))
        }
    }
}

fn install_update_in_work_dir(
    bundle_path: &Path,
    release: &BundleRelease,
    work_dir: &Path,
) -> Result<String> {
    let zip_path = work_dir.join(&release.asset_name);
    let extract_dir = work_dir.join("extract");
    let staged_bundle = work_dir.join("bundle");

    download_release_asset(release, &zip_path)?;
    expand_zip(&zip_path, &extract_dir)?;
    let source_bundle = find_source_bundle_root(&extract_dir)?;
    copy_dir_recursive(&source_bundle, &staged_bundle)?;
    stage_bundle(&staged_bundle, release, bundle_path)?;
    swap_bundle(bundle_path, &staged_bundle)?;

    Ok(format!("Bundle updated to {}.", release.tag))
}

fn fetch_latest_release() -> Result<BundleRelease> {
    let release: GitHubRelease = http_client()?
        .get(BUNDLE_RELEASE_API_URL)
        .send()
        .context("failed to request bundle release metadata")?
        .error_for_status()
        .context("bundle release metadata returned an error status")?
        .json()
        .context("failed to parse bundle release metadata")?;

    let asset = select_bundle_asset(&release.tag_name, release.assets)
        .ok_or_else(|| anyhow!("bundle zip asset not found in latest release"))?;

    Ok(BundleRelease {
        tag: release.tag_name,
        release_url: release.html_url,
        asset_url: asset.browser_download_url,
        asset_name: asset.name,
        digest: asset.digest,
    })
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(40))
        .build()
        .context("failed to build HTTP client")
}

fn select_bundle_asset(tag: &str, assets: Vec<GitHubAsset>) -> Option<GitHubAsset> {
    let exact_name = format!("zapret-discord-youtube-{tag}.zip");

    assets
        .into_iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(&exact_name))
        .or_else(|| None)
}

fn download_release_asset(release: &BundleRelease, zip_path: &Path) -> Result<()> {
    let bytes = http_client()?
        .get(&release.asset_url)
        .send()
        .context("failed to request bundle zip asset")?
        .error_for_status()
        .context("bundle zip download returned an error status")?
        .bytes()
        .context("failed to read bundle zip response body")?;

    if let Some(expected_digest) = &release.digest {
        verify_digest(bytes.as_ref(), expected_digest)?;
    }

    fs::write(zip_path, bytes.as_ref())
        .with_context(|| format!("failed to write {}", zip_path.display()))
}

fn verify_digest(bytes: &[u8], expected_digest: &str) -> Result<()> {
    let expected_hash = expected_digest
        .strip_prefix("sha256:")
        .unwrap_or(expected_digest)
        .to_ascii_lowercase();
    let actual_hash = format!("{:x}", Sha256::digest(bytes));

    if actual_hash != expected_hash {
        anyhow::bail!("bundle zip digest mismatch");
    }

    Ok(())
}

fn expand_zip(zip_path: &Path, extract_dir: &Path) -> Result<()> {
    fs::create_dir_all(extract_dir)
        .with_context(|| format!("failed to create {}", extract_dir.display()))?;

    let status = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg("Expand-Archive -LiteralPath $args[0] -DestinationPath $args[1] -Force")
        .arg(zip_path)
        .arg(extract_dir)
        .status()
        .context("failed to launch PowerShell Expand-Archive")?;

    if !status.success() {
        anyhow::bail!("PowerShell Expand-Archive failed with {status}");
    }

    Ok(())
}

fn find_source_bundle_root(extract_dir: &Path) -> Result<PathBuf> {
    if is_source_bundle_dir(extract_dir) {
        return Ok(extract_dir.to_owned());
    }

    for entry in fs::read_dir(extract_dir)
        .with_context(|| format!("failed to read {}", extract_dir.display()))?
    {
        let path = entry?.path();
        if path.is_dir() && is_source_bundle_dir(&path) {
            return Ok(path);
        }
    }

    Err(anyhow!(
        "extracted archive does not contain a valid zapret bundle"
    ))
}

fn is_source_bundle_dir(path: &Path) -> bool {
    path.join("bin").is_dir()
        && path.join("lists").is_dir()
        && path.join("general (SIMPLE FAKE ALT2).bat").is_file()
        && path.join("service.bat").is_file()
}

fn stage_bundle(
    staged_bundle: &Path,
    release: &BundleRelease,
    current_bundle: &Path,
) -> Result<()> {
    write_bundle_version_metadata(staged_bundle, &release.tag)?;
    install_latest_tg_proxy(staged_bundle)?;
    ensure_hub_scripts(staged_bundle)?;
    patch_profile_scripts(staged_bundle)?;
    add_hostlist_entries(
        staged_bundle
            .join("lists")
            .join("list-general.txt")
            .as_path(),
    )?;
    preserve_user_files(current_bundle, staged_bundle)?;

    if !is_valid_bundle_dir(staged_bundle) {
        anyhow::bail!("staged bundle failed validation");
    }

    Ok(())
}

fn install_latest_tg_proxy(staged_bundle: &Path) -> Result<()> {
    let status = check_tg_proxy_update(staged_bundle)?;
    install_tg_proxy_update(staged_bundle, &status.latest)?;
    Ok(())
}

fn write_bundle_version_metadata(staged_bundle: &Path, version: &str) -> Result<()> {
    let content = serde_json::to_string_pretty(&BundleVersionFile {
        version,
        source_folder: format!("zapret-discord-youtube-{version}"),
    })
    .context("failed to serialize bundle version metadata")?;

    fs::write(staged_bundle.join(BUNDLE_VERSION_FILE_NAME), content)
        .context("failed to write bundle version metadata")
}

fn ensure_hub_scripts(bundle_root: &Path) -> Result<()> {
    let hub_dir = bundle_root.join("hub");
    fs::create_dir_all(&hub_dir).context("failed to create hub scripts directory")?;

    write_profile_launcher(
        &hub_dir.join("run_full_simple_fake_alt2.cmd"),
        "general (SIMPLE FAKE ALT2).bat",
    )?;
    write_profile_launcher(&hub_dir.join("run_full_alt11.cmd"), "general (ALT11).bat")?;
    write_profile_launcher(
        &hub_dir.join("run_full_fake_tls_auto_alt3.cmd"),
        "general (FAKE TLS AUTO ALT3).bat",
    )?;
    write_profile_launcher(&hub_dir.join("run_full_alt7.cmd"), "general (ALT7).bat")?;

    fs::write(hub_dir.join("telegram_proxy.cmd"), telegram_proxy_script())?;
    fs::write(
        hub_dir.join("start_telegram_proxy_silent.cmd"),
        telegram_proxy_silent_script(),
    )?;
    fs::write(hub_dir.join("hub.cmd"), hub_menu_script())?;
    fs::write(
        hub_dir.join("configure_for_friends.cmd"),
        configure_for_friends_script(),
    )?;
    fs::write(
        hub_dir.join("install_service_simple_fake_alt2.cmd"),
        open_service_manager_script(),
    )?;
    fs::write(hub_dir.join("remove_service.cmd"), remove_service_script())?;
    fs::write(hub_dir.join("stop_all.cmd"), stop_all_script())?;

    Ok(())
}

fn write_profile_launcher(path: &Path, profile_script: &str) -> Result<()> {
    fs::write(
        path,
        format!(
            r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

start "" "%ROOT%\{profile_script}"
"#
        ),
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

fn patch_profile_scripts(bundle_root: &Path) -> Result<()> {
    for script_name in [
        "general (SIMPLE FAKE ALT2).bat",
        "general (ALT11).bat",
        "general (FAKE TLS AUTO ALT3).bat",
        "general (ALT7).bat",
    ] {
        let script_path = bundle_root.join(script_name);
        let content = fs::read_to_string(&script_path)
            .with_context(|| format!("failed to read {}", script_path.display()))?;
        let updated = content.replace(
            r#"start "zapret: %~n0" /min "%BIN%winws.exe""#,
            r#"start "" /B "%BIN%winws.exe""#,
        );
        fs::write(&script_path, updated)
            .with_context(|| format!("failed to write {}", script_path.display()))?;
    }

    Ok(())
}

pub(crate) fn add_hostlist_entries(list_path: &Path) -> Result<usize> {
    let content = fs::read_to_string(list_path)
        .with_context(|| format!("failed to read {}", list_path.display()))?;
    let (updated, added) = add_hostlist_entries_to_content(&content);

    if added > 0 {
        fs::write(list_path, updated)
            .with_context(|| format!("failed to write {}", list_path.display()))?;
    }

    Ok(added)
}

fn add_hostlist_entries_to_content(content: &str) -> (String, usize) {
    let mut lines: Vec<String> = content.lines().map(ToOwned::to_owned).collect();
    let mut existing = std::collections::HashSet::new();

    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            existing.insert(trimmed.to_ascii_lowercase());
        }
    }

    let mut added = 0;
    for entry in GITHUB_HOSTLIST_ENTRIES {
        if existing.insert(entry.to_ascii_lowercase()) {
            if added == 0 && lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push((*entry).to_owned());
            added += 1;
        }
    }

    let mut updated = lines.join("\n");
    updated.push('\n');
    (updated, added)
}

fn preserve_user_files(current_bundle: &Path, staged_bundle: &Path) -> Result<()> {
    if !current_bundle.is_dir() {
        return Ok(());
    }

    for relative_path in PRESERVED_RELATIVE_FILES {
        let source = current_bundle.join(relative_path);
        if !source.is_file() {
            continue;
        }

        let target = staged_bundle.join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&source, &target).with_context(|| {
            format!(
                "failed to preserve {} into {}",
                source.display(),
                target.display()
            )
        })?;
    }

    Ok(())
}

fn swap_bundle(bundle_path: &Path, staged_bundle: &Path) -> Result<()> {
    let parent = bundle_path
        .parent()
        .ok_or_else(|| anyhow!("bundle path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let backup_path = parent.join(format!("bundle.backup-{}", timestamp()?));
    let had_existing_bundle = bundle_path.exists();

    if had_existing_bundle {
        fs::rename(bundle_path, &backup_path).with_context(|| {
            format!(
                "failed to move {} to {}",
                bundle_path.display(),
                backup_path.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(staged_bundle, bundle_path) {
        if had_existing_bundle {
            let _ = fs::rename(&backup_path, bundle_path);
        }
        return Err(error).with_context(|| {
            format!(
                "failed to move {} to {}",
                staged_bundle.display(),
                bundle_path.display()
            )
        });
    }

    if had_existing_bundle {
        let _ = fs::remove_dir_all(&backup_path);
    }

    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;

    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn unique_work_dir() -> Result<PathBuf> {
    Ok(std::env::temp_dir().join(format!("zapret-hub-bundle-update-{}", timestamp()?)))
}

fn timestamp() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before unix epoch")?
        .as_millis())
}

fn telegram_proxy_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."
set "TG_PROXY_ARGS=--dc-ip 2:149.154.167.220 --dc-ip 4:149.154.167.220 --dc-ip 203:149.154.167.220"

if not exist "%ROOT%\TgWsProxy_windows.exe" (
    exit /b 1
)

tasklist /FI "IMAGENAME eq TgWsProxy_windows.exe" | find /I "TgWsProxy_windows.exe" > nul
if errorlevel 1 (
    start "" /B "%ROOT%\TgWsProxy_windows.exe" %TG_PROXY_ARGS%
)
"#
}

fn telegram_proxy_silent_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"

start "" /min "%~dp0telegram_proxy.cmd"
"#
}

fn open_service_manager_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

start "" "%ROOT%\service.bat"
"#
}

fn configure_for_friends_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

echo all>"%ROOT%\utils\game_filter.enabled"
break>"%ROOT%\lists\ipset-all.txt"

echo Preconfigured:
echo   Game Filter = enabled (TCP and UDP)
echo   IPSet Filter = any
echo.
echo Open the service manager and install the SIMPLE FAKE ALT2 profile if needed.
pause
"#
}

fn remove_service_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"

sc query zapret > nul 2>&1
if not errorlevel 1 (
    net stop zapret > nul 2>&1
    sc delete zapret > nul 2>&1
)

tasklist /FI "IMAGENAME eq winws.exe" | find /I "winws.exe" > nul
if not errorlevel 1 (
    taskkill /IM winws.exe /F > nul 2>&1
)

sc stop WinDivert > nul 2>&1
sc stop WinDivert14 > nul 2>&1

echo Service removal command completed.
"#
}

fn stop_all_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"

sc query zapret > nul 2>&1
if not errorlevel 1 (
    net stop zapret > nul 2>&1
)

tasklist /FI "IMAGENAME eq winws.exe" | find /I "winws.exe" > nul
if not errorlevel 1 (
    taskkill /IM winws.exe /F > nul 2>&1
)

tasklist /FI "IMAGENAME eq TgWsProxy_windows.exe" | find /I "TgWsProxy_windows.exe" > nul
if not errorlevel 1 (
    taskkill /IM TgWsProxy_windows.exe /F > nul 2>&1
)

sc stop WinDivert > nul 2>&1
sc stop WinDivert14 > nul 2>&1

echo Bypass processes were stopped.
"#
}

fn hub_menu_script() -> &'static str {
    r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

:menu
cls
echo.
echo   ZAPRET HUB
echo   ----------
echo.
echo   1. Start main profile (SIMPLE FAKE ALT2)
echo   2. Start ALT11
echo   3. Start FAKE TLS AUTO ALT3
echo   4. Start ALT7
echo   5. Start Telegram Desktop proxy
echo   6. Stop all
echo   7. Install service / open service manager
echo   8. Remove service
echo   9. Configure for friends
echo   10. Open upstream service manager
echo.
echo   0. Exit
echo.
set "choice="
set /p "choice=Select option: "

if "%choice%"=="1" start "" "%~dp0run_full_simple_fake_alt2.cmd"
if "%choice%"=="2" start "" "%~dp0run_full_alt11.cmd"
if "%choice%"=="3" start "" "%~dp0run_full_fake_tls_auto_alt3.cmd"
if "%choice%"=="4" start "" "%~dp0run_full_alt7.cmd"
if "%choice%"=="5" start "" "%~dp0start_telegram_proxy_silent.cmd"
if "%choice%"=="6" call "%~dp0stop_all.cmd"
if "%choice%"=="7" start "" "%ROOT%\service.bat"
if "%choice%"=="8" call "%~dp0remove_service.cmd"
if "%choice%"=="9" call "%~dp0configure_for_friends.cmd"
if "%choice%"=="10" start "" "%ROOT%\service.bat"
if "%choice%"=="0" exit /b

goto menu
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn select_bundle_asset_prefers_exact_zip_name() {
        let assets = vec![
            GitHubAsset {
                name: "zapret-discord-youtube-1.9.8c.rar".to_owned(),
                browser_download_url: "https://example.test/bundle.rar".to_owned(),
                digest: None,
            },
            GitHubAsset {
                name: "zapret-discord-youtube-1.9.8c.zip".to_owned(),
                browser_download_url: "https://example.test/bundle.zip".to_owned(),
                digest: Some("sha256:abc".to_owned()),
            },
        ];

        let asset = select_bundle_asset("1.9.8c", assets).expect("zip asset selected");

        assert_eq!(asset.name, "zapret-discord-youtube-1.9.8c.zip");
        assert_eq!(
            asset.browser_download_url,
            "https://example.test/bundle.zip"
        );
        assert_eq!(asset.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn select_bundle_asset_rejects_non_exact_zip() {
        let assets = vec![GitHubAsset {
            name: "zapret-discord-youtube-1.9.7b.zip".to_owned(),
            browser_download_url: "https://example.test/old.zip".to_owned(),
            digest: None,
        }];

        assert!(select_bundle_asset("1.9.8c", assets).is_none());
    }

    #[test]
    fn hostlist_entries_are_added_once() {
        let (updated, added) =
            add_hostlist_entries_to_content("discord.com\nGitHub.com\nraw.githubusercontent.com\n");

        assert_eq!(added, GITHUB_HOSTLIST_ENTRIES.len() - 2);
        assert_eq!(
            updated
                .lines()
                .filter(|line| line.eq_ignore_ascii_case("github.com"))
                .count(),
            1
        );
        assert!(updated.contains("objects.githubusercontent.com"));
        assert!(updated.ends_with('\n'));
    }

    #[test]
    fn swap_bundle_replaces_current_bundle_and_removes_backup() -> Result<()> {
        let root = env::temp_dir().join(format!("zapret-hub-swap-test-{}", timestamp()?));
        let current = root.join("bundle");
        let staged = root.join("staged");

        fs::create_dir_all(&current)?;
        fs::create_dir_all(&staged)?;
        fs::write(current.join("version.txt"), "old")?;
        fs::write(staged.join("version.txt"), "new")?;

        swap_bundle(&current, &staged)?;

        assert_eq!(fs::read_to_string(current.join("version.txt"))?, "new");
        assert!(!staged.exists());
        assert!(fs::read_dir(&root)?.all(|entry| {
            entry
                .map(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("bundle.backup-")
                })
                .unwrap_or(false)
        }));

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
