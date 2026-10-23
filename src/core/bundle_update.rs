use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::core::bundle_metadata::detect_bundle_version;
use crate::core::github::{self, DownloadProgress, ReleaseAsset};
use crate::core::paths::{has_profile_script, is_valid_bundle_dir};
use crate::core::profile_test::PROFILE_TEST_PING_TARGETS;
use crate::core::tg_proxy_update::{
    TG_PROXY_ASSET_NAME, TG_PROXY_VERSION_FILE_NAME, check_for_update as check_tg_proxy_update,
    install_update as install_tg_proxy_update,
};

const BUNDLE_REPO: &str = "Flowseal/zapret-discord-youtube";
const BUNDLE_VERSION_FILE_NAME: &str = "ZapretBundle.version.json";
const SHA256SUMS_FILE_NAME: &str = "SHA256SUMS.txt";
const UPSTREAM_UPDATE_CHECK_FLAG_RELATIVE_PATH: &str = "utils/check_updates.enabled";
const PROFILE_TEST_TARGETS_HEADING: &str =
    "### Public network endpoints (PING-only, not used as DNS resolvers)";
const UPSTREAM_WINWS_LAUNCH: &str = r#"start "zapret: %~n0" /min "%BIN%winws.exe""#;
const HIDDEN_WINWS_LAUNCH: &str = r#"start "" /B "%BIN%winws.exe""#;
const RENAME_ATTEMPTS: u32 = 6;

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
    "lists/list-general-user.txt",
    "lists/list-exclude-user.txt",
    "lists/ipset-exclude-user.txt",
    "bin/ACTIVE_DISCORD_UDP.bin",
    "bin/ACTIVE_GAME_UDP.bin",
    "utils/game_filter.enabled",
    "tgproxy-runtime.log",
    "tgproxy-launch.log",
];

#[derive(Clone, Debug)]
pub(crate) struct BundleRelease {
    pub(crate) tag: String,
    pub(crate) release_url: String,
    pub(crate) asset: ReleaseAsset,
}

#[derive(Clone, Debug)]
pub(crate) struct BundleUpdateStatus {
    pub(crate) latest: BundleRelease,
    pub(crate) update_available: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedBundleUpdate {
    pub(crate) release: BundleRelease,
    pub(crate) work_dir: PathBuf,
    pub(crate) staged_bundle: PathBuf,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct BundleUpdateOutcome {
    pub(crate) message: String,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BundleVersionFile<'a> {
    version: &'a str,
    source_folder: String,
}

pub(crate) fn check_for_update(bundle_path: &Path) -> Result<BundleUpdateStatus> {
    let latest = fetch_latest_release()?;
    let installed_version = is_valid_bundle_dir(bundle_path)
        .then(|| detect_bundle_version(bundle_path))
        .flatten();
    let update_available = installed_version.as_deref() != Some(latest.tag.as_str());

    Ok(BundleUpdateStatus {
        latest,
        update_available,
    })
}

pub(crate) fn prepare_update(
    bundle_path: &Path,
    release: &BundleRelease,
    progress: &DownloadProgress,
) -> Result<PreparedBundleUpdate> {
    let work_dir = unique_work_dir(bundle_path)?;
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    match prepare_update_in_work_dir(bundle_path, release, &work_dir, progress) {
        Ok(prepared) => Ok(prepared),
        Err(error) => {
            let _ = fs::remove_dir_all(&work_dir);
            Err(error)
        }
    }
}

pub(crate) fn apply_prepared_update(
    bundle_path: &Path,
    prepared: &PreparedBundleUpdate,
) -> Result<BundleUpdateOutcome> {
    if !prepared.staged_bundle.is_dir() {
        anyhow::bail!(
            "подготовленный bundle пропал: {}",
            prepared.staged_bundle.display()
        );
    }

    preserve_user_files(bundle_path, &prepared.staged_bundle)?;

    if !is_valid_bundle_dir(&prepared.staged_bundle) {
        anyhow::bail!("подготовленный bundle не прошёл проверку");
    }

    swap_bundle(bundle_path, &prepared.staged_bundle)?;

    let mut warnings = prepared.warnings.clone();
    if let Err(error) = discard_prepared_update(prepared) {
        warnings.push(format!("временная папка обновления не удалена: {error}"));
    }

    Ok(BundleUpdateOutcome {
        message: format!("Bundle обновлён до {}.", prepared.release.tag),
        warnings,
    })
}

pub(crate) fn discard_prepared_update(prepared: &PreparedBundleUpdate) -> Result<()> {
    if prepared.work_dir.exists() {
        fs::remove_dir_all(&prepared.work_dir)
            .with_context(|| format!("failed to remove {}", prepared.work_dir.display()))?;
    }

    Ok(())
}

fn prepare_update_in_work_dir(
    bundle_path: &Path,
    release: &BundleRelease,
    work_dir: &Path,
    progress: &DownloadProgress,
) -> Result<PreparedBundleUpdate> {
    let extract_dir = work_dir.join("extract");
    let staged_bundle = work_dir.join("bundle");

    let archive = github::download(&release.asset, progress)?;
    extract_zip(&archive, &extract_dir)?;
    let source_bundle = find_source_bundle_root(&extract_dir)?;
    // Same volume as work_dir, so this is a cheap rename.
    fs::rename(&source_bundle, &staged_bundle)
        .or_else(|_| copy_dir_recursive(&source_bundle, &staged_bundle))?;
    let _ = fs::remove_dir_all(&extract_dir);
    let warnings = stage_bundle(&staged_bundle, release, bundle_path)?;

    Ok(PreparedBundleUpdate {
        release: release.clone(),
        work_dir: work_dir.to_owned(),
        staged_bundle,
        warnings,
    })
}

/// Packaging helper: downloads a release and prepares it into `target` exactly like the in-app
/// update does (`zapret-hub-rs --stage-bundle <dir> [--bundle-tag <tag>]`).
pub(crate) fn stage_release_bundle(target: &Path, tag: Option<&str>) -> Result<String> {
    let release = match tag {
        Some(tag) => bundle_release(github::release_by_tag(BUNDLE_REPO, tag)?)?,
        None => fetch_latest_release()?,
    };
    let prepared = prepare_update(target, &release, &DownloadProgress::default())?;
    let outcome = apply_prepared_update(target, &prepared)?;
    let mut message = outcome.message;
    for warning in outcome.warnings {
        message.push_str(&format!("\nwarning: {warning}"));
    }
    Ok(message)
}

fn fetch_latest_release() -> Result<BundleRelease> {
    bundle_release(github::latest_release(BUNDLE_REPO)?)
}

fn bundle_release(release: github::Release) -> Result<BundleRelease> {
    let asset = release
        .asset(&bundle_asset_name(&release.tag))
        .ok_or_else(|| anyhow!("в релизе {} нет zip-архива bundle", release.tag))?;

    Ok(BundleRelease {
        tag: release.tag,
        release_url: release.html_url,
        asset,
    })
}

fn bundle_asset_name(tag: &str) -> String {
    format!("zapret-discord-youtube-{tag}.zip")
}

fn extract_zip(bytes: &[u8], extract_dir: &Path) -> Result<()> {
    fs::create_dir_all(extract_dir)
        .with_context(|| format!("failed to create {}", extract_dir.display()))?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("скачанный архив bundle повреждён")?;
    // `extract` rejects entries that would escape the target directory.
    archive
        .extract(extract_dir)
        .context("не удалось распаковать архив bundle")
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
        "в архиве не найден bundle zapret (bin, lists, general*.bat)"
    ))
}

fn is_source_bundle_dir(path: &Path) -> bool {
    path.join("bin").join("winws.exe").is_file()
        && path.join("lists").is_dir()
        && path.join("service.bat").is_file()
        && has_profile_script(path)
}

fn stage_bundle(
    staged_bundle: &Path,
    release: &BundleRelease,
    current_bundle: &Path,
) -> Result<Vec<String>> {
    stage_bundle_with_tg_proxy_installer(
        staged_bundle,
        release,
        current_bundle,
        install_latest_tg_proxy,
    )
}

fn stage_bundle_with_tg_proxy_installer<F>(
    staged_bundle: &Path,
    release: &BundleRelease,
    current_bundle: &Path,
    install_tg_proxy: F,
) -> Result<Vec<String>>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let mut warnings = Vec::new();

    write_bundle_version_metadata(staged_bundle, &release.tag)?;
    preserve_existing_tg_proxy(current_bundle, staged_bundle)?;
    if let Err(error) = install_tg_proxy(staged_bundle) {
        warnings.push(format!("Telegram WS proxy не обновлён: {error}"));
    }
    ensure_hub_scripts(staged_bundle)?;
    patch_profile_scripts(staged_bundle)?;
    patch_profile_test_targets(staged_bundle)?;
    disable_upstream_update_checks(staged_bundle)?;
    let general_list = staged_bundle.join("lists").join("list-general.txt");
    if general_list.is_file() {
        add_hostlist_entries(&general_list)?;
    }

    if !is_valid_bundle_dir(staged_bundle) {
        anyhow::bail!("подготовленный bundle не прошёл проверку");
    }

    Ok(warnings)
}

fn install_latest_tg_proxy(staged_bundle: &Path) -> Result<()> {
    let status = check_tg_proxy_update(staged_bundle)?;
    if status.update_available {
        install_tg_proxy_update(staged_bundle, &status.latest, &DownloadProgress::default())?;
    }
    Ok(())
}

fn preserve_existing_tg_proxy(current_bundle: &Path, staged_bundle: &Path) -> Result<()> {
    for file_name in [TG_PROXY_ASSET_NAME, TG_PROXY_VERSION_FILE_NAME] {
        let source = current_bundle.join(file_name);
        if !source.is_file() {
            continue;
        }

        fs::copy(&source, staged_bundle.join(file_name)).with_context(|| {
            format!("failed to preserve {} into staged bundle", source.display())
        })?;
    }

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

fn patch_profile_test_targets(bundle_root: &Path) -> Result<()> {
    let targets_path = bundle_root.join("utils").join("targets.txt");
    let content = fs::read_to_string(&targets_path).unwrap_or_default();
    let updated = patch_profile_test_targets_content(&content);
    fs::create_dir_all(
        targets_path
            .parent()
            .context("targets path has no parent")?,
    )?;
    fs::write(&targets_path, updated)
        .with_context(|| format!("failed to write {}", targets_path.display()))?;
    remove_sha256sum_entry(bundle_root, "utils/targets.txt")
}

fn patch_profile_test_targets_content(content: &str) -> String {
    let mut lines = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed == "### Public DNS (PING-only)" || trimmed == PROFILE_TEST_TARGETS_HEADING {
                return false;
            }
            if trimmed.starts_with('#') {
                return true;
            }
            line.split_once('=')
                .map(|(_, value)| {
                    !value
                        .trim()
                        .trim_matches('"')
                        .to_ascii_uppercase()
                        .starts_with("PING:")
                })
                .unwrap_or(true)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();

    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.push(String::new());
    lines.push(PROFILE_TEST_TARGETS_HEADING.to_owned());
    lines.extend(
        PROFILE_TEST_PING_TARGETS
            .iter()
            .map(|(key, _, address)| format!("{key} = \"PING:{address}\"")),
    );
    lines.push(String::new());
    lines.join("\r\n")
}

pub(crate) fn ensure_hub_scripts(bundle_root: &Path) -> Result<()> {
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
    for entry in fs::read_dir(bundle_root)
        .with_context(|| format!("failed to read {}", bundle_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let script_name = entry.file_name().to_string_lossy().to_string();
        if is_profile_script_name(&script_name) {
            patch_profile_script(&entry.path())?;
        }
    }

    Ok(())
}

/// Upstream starts winws.exe with `start ... /min`, which leaves a minimized console window
/// in the taskbar. `/B` keeps it inside the invisible console of the hidden `cmd /C`.
/// Idempotent; returns whether the file changed.
pub(crate) fn patch_profile_script(script_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(script_path)
        .with_context(|| format!("failed to read {}", script_path.display()))?;
    if !content.contains(UPSTREAM_WINWS_LAUNCH) {
        return Ok(false);
    }

    let updated = content.replace(UPSTREAM_WINWS_LAUNCH, HIDDEN_WINWS_LAUNCH);
    fs::write(script_path, updated)
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    Ok(true)
}

fn is_profile_script_name(script_name: &str) -> bool {
    let lower = script_name.to_ascii_lowercase();
    lower.starts_with("general") && lower.ends_with(".bat")
}

fn disable_upstream_update_checks(bundle_root: &Path) -> Result<()> {
    let flag_path = bundle_root.join("utils").join("check_updates.enabled");
    match fs::remove_file(&flag_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to remove {}", flag_path.display()));
        }
    }

    remove_sha256sum_entry(bundle_root, UPSTREAM_UPDATE_CHECK_FLAG_RELATIVE_PATH)
}

fn remove_sha256sum_entry(bundle_root: &Path, relative_path: &str) -> Result<()> {
    let checksums_path = bundle_root.join(SHA256SUMS_FILE_NAME);
    if !checksums_path.is_file() {
        return Ok(());
    }

    let content = fs::read_to_string(&checksums_path)
        .with_context(|| format!("failed to read {}", checksums_path.display()))?;
    let mut changed = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if sha256sum_line_targets(line, relative_path) {
            changed = true;
            continue;
        }
        lines.push(line);
    }

    if changed {
        let mut updated = lines.join("\n");
        if content.ends_with('\n') {
            updated.push('\n');
        }
        fs::write(&checksums_path, updated)
            .with_context(|| format!("failed to write {}", checksums_path.display()))?;
    }

    Ok(())
}

fn sha256sum_line_targets(line: &str, relative_path: &str) -> bool {
    let Some(path) = line.split_whitespace().last() else {
        return false;
    };
    let normalized = path.replace('\\', "/");
    let relative_path = relative_path.trim_start_matches("./");
    normalized == relative_path || normalized == format!("./{relative_path}")
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

/// Replaces the bundle with the staged one. The fast path renames the whole folder; when
/// Windows refuses (folder open in Explorer, antivirus scan, …) the files are replaced in place.
fn swap_bundle(bundle_path: &Path, staged_bundle: &Path) -> Result<()> {
    let parent = bundle_path
        .parent()
        .ok_or_else(|| anyhow!("bundle path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    if !bundle_path.exists() {
        return move_dir(staged_bundle, bundle_path);
    }

    let backup_path = parent.join(format!("bundle.backup-{}", timestamp()?));
    if rename_with_retry(bundle_path, &backup_path).is_err() {
        return replace_in_place(bundle_path, staged_bundle);
    }

    if let Err(error) = move_dir(staged_bundle, bundle_path) {
        let _ = fs::remove_dir_all(bundle_path);
        let _ = rename_with_retry(&backup_path, bundle_path);
        return Err(error);
    }

    let _ = fs::remove_dir_all(&backup_path);
    Ok(())
}

fn replace_in_place(bundle_path: &Path, staged_bundle: &Path) -> Result<()> {
    let parent = bundle_path
        .parent()
        .ok_or_else(|| anyhow!("bundle path has no parent"))?;
    let backup_path = parent.join(format!("bundle.backup-{}", timestamp()?));
    copy_dir_recursive(bundle_path, &backup_path)
        .context("не удалось сделать резервную копию bundle перед обновлением")?;

    let result = remove_stale_upstream_files(bundle_path, staged_bundle)
        .and_then(|()| copy_dir_recursive(staged_bundle, bundle_path));

    match result {
        Ok(()) => {
            let _ = fs::remove_dir_all(&backup_path);
            let _ = fs::remove_dir_all(staged_bundle);
            Ok(())
        }
        Err(error) => {
            let restored = copy_dir_recursive(&backup_path, bundle_path).is_ok();
            if restored {
                let _ = fs::remove_dir_all(&backup_path);
                Err(error
                    .context("файлы bundle заняты другой программой; старая версия восстановлена"))
            } else {
                Err(error.context(format!(
                    "обновление не удалось, резервная копия лежит в {}",
                    backup_path.display()
                )))
            }
        }
    }
}

/// Upstream-owned files that disappeared in the new release: old profiles and fake payloads.
fn remove_stale_upstream_files(bundle_path: &Path, staged_bundle: &Path) -> Result<()> {
    for entry in fs::read_dir(bundle_path)? {
        let entry = entry?;
        let name = entry.file_name();
        if entry.file_type()?.is_file()
            && is_profile_script_name(&name.to_string_lossy())
            && !staged_bundle.join(&name).exists()
        {
            fs::remove_file(entry.path())?;
        }
    }

    let bin_dir = bundle_path.join("bin");
    if bin_dir.is_dir() {
        for entry in fs::read_dir(&bin_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let is_fake = name
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".bin");
            if entry.file_type()?.is_file()
                && is_fake
                && !staged_bundle.join("bin").join(&name).exists()
            {
                fs::remove_file(entry.path())?;
            }
        }
    }

    Ok(())
}

fn move_dir(source: &Path, target: &Path) -> Result<()> {
    if rename_with_retry(source, target).is_ok() {
        return Ok(());
    }
    copy_dir_recursive(source, target)?;
    let _ = fs::remove_dir_all(source);
    Ok(())
}

/// Antivirus scanners briefly lock freshly written files, so a failed rename is retried.
fn rename_with_retry(source: &Path, target: &Path) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..RENAME_ATTEMPTS {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < RENAME_ATTEMPTS {
            thread::sleep(Duration::from_millis(300));
        }
    }

    Err(anyhow!(
        "failed to move {} to {}: {}",
        source.display(),
        target.display(),
        last_error.map_or_else(String::new, |error| error.to_string())
    ))
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;

    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if entry.file_type()?.is_dir() {
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

/// Next to the bundle, so the final swap is a same-volume rename (a `%TEMP%` on C: cannot be
/// renamed into a portable install on D:).
fn unique_work_dir(bundle_path: &Path) -> Result<PathBuf> {
    let name = format!(".zapret-hub-update-{}", timestamp()?);
    let parent = bundle_path
        .parent()
        .filter(|parent| fs::create_dir_all(parent).is_ok() && is_writable_dir(parent));
    Ok(match parent {
        Some(parent) => parent.join(name),
        None => std::env::temp_dir().join(name),
    })
}

fn is_writable_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".zapret-hub-write-probe-{}", std::process::id()));
    let writable = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    writable
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
set "TG_PROXY_ARGS=--dc-ip 2:149.154.167.220 --dc-ip 4:149.154.167.220"

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
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 > nul
cd /d "%~dp0"

set "FAILED="

call :stop_service zapret
call :kill_process winws.exe
call :kill_process TgWsProxy_windows.exe
call :stop_service WinDivert
call :stop_service WinDivert14
call :verify_service_stopped zapret
call :verify_process_stopped winws.exe
call :verify_process_stopped TgWsProxy_windows.exe
call :verify_service_stopped WinDivert
call :verify_service_stopped WinDivert14

if defined FAILED (
    echo Failed to stop: !FAILED!
    exit /b 1
)

sc delete zapret > nul 2>&1
sc delete WinDivert > nul 2>&1
sc delete WinDivert14 > nul 2>&1

echo Service removal command completed.
exit /b 0

:stop_service
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
net stop "%~1" > nul 2>&1
for /l %%I in (1,1,10) do (
    sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
    if errorlevel 1 exit /b 0
    timeout /t 1 /nobreak > nul
)
sc stop "%~1" > nul 2>&1
exit /b 0

:kill_process
for /l %%I in (1,1,3) do (
    tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
    if errorlevel 1 exit /b 0
    taskkill /IM "%~1" /T /F > nul 2>&1
    timeout /t 1 /nobreak > nul
)
exit /b 0

:verify_service_stopped
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
if not errorlevel 1 call :append_failure "service %~1"
exit /b 0

:verify_process_stopped
tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
if not errorlevel 1 call :append_failure "process %~1"
exit /b 0

:append_failure
if defined FAILED (
    set "FAILED=!FAILED!; %~1"
) else (
    set "FAILED=%~1"
)
exit /b 0
"#
}

fn stop_all_script() -> &'static str {
    r#"@echo off
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 > nul
cd /d "%~dp0"

set "FAILED="

call :stop_service zapret
call :kill_process winws.exe
call :kill_process TgWsProxy_windows.exe
call :stop_service WinDivert
call :stop_service WinDivert14
call :verify_service_stopped zapret
call :verify_process_stopped winws.exe
call :verify_process_stopped TgWsProxy_windows.exe
call :verify_service_stopped WinDivert
call :verify_service_stopped WinDivert14

if defined FAILED (
    echo Failed to stop: !FAILED!
    exit /b 1
)

echo Bypass processes were stopped.
exit /b 0

:stop_service
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
net stop "%~1" > nul 2>&1
for /l %%I in (1,1,10) do (
    sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
    if errorlevel 1 exit /b 0
    timeout /t 1 /nobreak > nul
)
sc stop "%~1" > nul 2>&1
exit /b 0

:kill_process
for /l %%I in (1,1,3) do (
    tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
    if errorlevel 1 exit /b 0
    taskkill /IM "%~1" /T /F > nul 2>&1
    timeout /t 1 /nobreak > nul
)
exit /b 0

:verify_service_stopped
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
if not errorlevel 1 call :append_failure "service %~1"
exit /b 0

:verify_process_stopped
tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
if not errorlevel 1 call :append_failure "process %~1"
exit /b 0

:append_failure
if defined FAILED (
    set "FAILED=!FAILED!; %~1"
) else (
    set "FAILED=%~1"
)
exit /b 0
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
    fn bundle_asset_name_matches_upstream_release_zip() {
        assert_eq!(
            bundle_asset_name("1.10.4"),
            "zapret-discord-youtube-1.10.4.zip"
        );
    }

    #[test]
    fn profile_script_patch_hides_winws_console_and_is_idempotent() -> Result<()> {
        let root = temp_root("zapret-hub-profile-script-patch-test")?;
        let script = root.join("general (ALT).bat");
        fs::write(
            &script,
            format!("@echo off\r\n{UPSTREAM_WINWS_LAUNCH} --wf-tcp=80\r\n"),
        )?;

        assert!(patch_profile_script(&script)?);
        assert!(!patch_profile_script(&script)?);
        let content = fs::read_to_string(&script)?;
        assert!(content.contains(HIDDEN_WINWS_LAUNCH));
        assert!(!content.contains("/min"));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn zip_archive_with_top_level_folder_is_extracted_and_found() -> Result<()> {
        use std::io::Write;

        let root = temp_root("zapret-hub-zip-extract-test")?;
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for name in [
                "zapret-discord-youtube-1.10.4/bin/winws.exe",
                "zapret-discord-youtube-1.10.4/lists/list-general.txt",
                "zapret-discord-youtube-1.10.4/service.bat",
                "zapret-discord-youtube-1.10.4/general (ALT13).bat",
            ] {
                writer.start_file(name, options)?;
                writer.write_all(b"x")?;
            }
            writer.finish()?;
        }

        extract_zip(buffer.get_ref(), &root.join("extract"))?;
        let found = find_source_bundle_root(&root.join("extract"))?;
        assert!(found.ends_with("zapret-discord-youtube-1.10.4"));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn replace_in_place_updates_files_and_drops_stale_profiles() -> Result<()> {
        let root = temp_root("zapret-hub-replace-in-place-test")?;
        let current = root.join("bundle");
        let staged = root.join("staged");
        write_minimal_source_bundle(&current)?;
        fs::write(current.join("general (OLD).bat"), "old")?;
        fs::write(current.join("bin").join("old_fake.bin"), "old")?;
        write_minimal_source_bundle(&staged)?;
        fs::write(staged.join("service.bat"), "new")?;

        replace_in_place(&current, &staged)?;

        assert_eq!(fs::read_to_string(current.join("service.bat"))?, "new");
        assert!(!current.join("general (OLD).bat").exists());
        assert!(!current.join("bin").join("old_fake.bin").exists());
        assert!(current.join("general (ALT11).bat").exists());
        assert!(!staged.exists());
        assert!(fs::read_dir(&root)?.flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with("bundle.backup-")
        }));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn swap_bundle_installs_into_missing_bundle_dir() -> Result<()> {
        let root = temp_root("zapret-hub-fresh-install-test")?;
        let current = root.join("bundle");
        let staged = root.join("staged");
        write_minimal_source_bundle(&staged)?;

        swap_bundle(&current, &staged)?;

        assert!(is_valid_bundle_dir(&current));
        assert!(!staged.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn targets_comments_are_kept_verbatim() {
        let content = "#   KeyName = \"PING:1.2.3.4\"       -> Ping only\r\nDiscord = \"https://discord.com\"\r\n";
        let updated = patch_profile_test_targets_content(content);
        assert!(updated.contains("#   KeyName = \"PING:1.2.3.4\""));
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
    fn profile_test_targets_replace_duplicate_provider_endpoints() {
        let content = "Discord = \"https://discord.com\"\r\nCloudflare1 = \"PING:1.1.1.1\"\r\nCloudflare2 = \"PING:1.0.0.1\"\r\n";

        let updated = patch_profile_test_targets_content(content);

        assert!(updated.contains("Discord = \"https://discord.com\""));
        assert!(!updated.contains("1.0.0.1"));
        for (key, _, address) in PROFILE_TEST_PING_TARGETS {
            assert!(updated.contains(&format!("{key} = \"PING:{address}\"")));
        }
        assert_eq!(
            patch_profile_test_targets_content(&updated)
                .matches(PROFILE_TEST_TARGETS_HEADING)
                .count(),
            1
        );
    }

    #[test]
    fn stage_bundle_does_not_copy_user_lists_before_apply() -> Result<()> {
        let root = temp_root("zapret-hub-stage-no-preserve-test")?;
        let current = root.join("bundle");
        let staged = root.join("work").join("bundle");
        write_current_bundle_user_file(&current, "before prepare")?;
        write_minimal_source_bundle(&staged)?;

        let warnings =
            stage_bundle_with_tg_proxy_installer(&staged, &test_release(), &current, |_| Ok(()))?;

        assert!(warnings.is_empty());
        assert_eq!(
            fs::read_to_string(current.join("lists").join("list-general-user.txt"))?,
            "before prepare"
        );
        assert!(!staged.join("lists").join("list-general-user.txt").exists());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn stage_bundle_disables_upstream_auto_update_checks() -> Result<()> {
        let root = temp_root("zapret-hub-disable-upstream-updates-test")?;
        let current = root.join("bundle");
        let staged = root.join("work").join("bundle");
        write_minimal_source_bundle(&staged)?;

        stage_bundle_with_tg_proxy_installer(&staged, &test_release(), &current, |_| Ok(()))?;

        assert!(!staged.join("utils").join("check_updates.enabled").exists());
        assert!(
            !fs::read_to_string(staged.join(SHA256SUMS_FILE_NAME))?
                .contains("check_updates.enabled")
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn stage_bundle_patches_all_profile_launchers() -> Result<()> {
        let root = temp_root("zapret-hub-profile-patch-test")?;
        let current = root.join("bundle");
        let staged = root.join("work").join("bundle");
        write_minimal_source_bundle(&staged)?;

        stage_bundle_with_tg_proxy_installer(&staged, &test_release(), &current, |_| Ok(()))?;

        for script_name in [
            "general (SIMPLE FAKE ALT2).bat",
            "general (ALT11).bat",
            "general (ALT12).bat",
            "general.bat",
        ] {
            let content = fs::read_to_string(staged.join(script_name))?;
            assert!(content.contains(r#"start "" /B "%BIN%winws.exe""#));
            assert!(!content.contains(r#"start "zapret: %~n0" /min "%BIN%winws.exe""#));
        }

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn apply_prepared_update_preserves_user_files_changed_after_prepare() -> Result<()> {
        let root = temp_root("zapret-hub-apply-preserve-test")?;
        let current = root.join("bundle");
        let work_dir = root.join("work");
        let staged = work_dir.join("bundle");
        write_current_bundle_user_file(&current, "before prepare")?;
        write_minimal_source_bundle(&staged)?;
        stage_bundle_with_tg_proxy_installer(&staged, &test_release(), &current, |_| Ok(()))?;

        write_current_bundle_user_file(&current, "changed after prepare")?;
        let prepared = PreparedBundleUpdate {
            release: test_release(),
            work_dir: work_dir.clone(),
            staged_bundle: staged,
            warnings: Vec::new(),
        };

        let outcome = apply_prepared_update(&current, &prepared)?;

        assert_eq!(outcome.message, "Bundle обновлён до 1.9.9a.");
        assert_eq!(
            fs::read_to_string(current.join("lists").join("list-general-user.txt"))?,
            "changed after prepare"
        );
        assert!(!work_dir.exists());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn preserve_user_files_keeps_fake_files_and_game_filter_state() -> Result<()> {
        let root = temp_root("zapret-hub-preserve-runtime-files-test")?;
        let current = root.join("current");
        let staged = root.join("staged");
        for relative in PRESERVED_RELATIVE_FILES {
            let source = current.join(relative);
            fs::create_dir_all(source.parent().expect("parent"))?;
            fs::write(&source, relative.as_bytes())?;
        }

        preserve_user_files(&current, &staged)?;
        for relative in PRESERVED_RELATIVE_FILES {
            assert_eq!(fs::read(staged.join(relative))?, relative.as_bytes());
        }

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn stage_bundle_keeps_existing_tg_proxy_when_update_fails() -> Result<()> {
        let root = temp_root("zapret-hub-tg-warning-test")?;
        let current = root.join("bundle");
        let staged = root.join("work").join("bundle");
        write_current_bundle_user_file(&current, "user file")?;
        fs::write(current.join(TG_PROXY_ASSET_NAME), "old proxy")?;
        fs::write(current.join(TG_PROXY_VERSION_FILE_NAME), "old version")?;
        write_minimal_source_bundle(&staged)?;

        let warnings =
            stage_bundle_with_tg_proxy_installer(&staged, &test_release(), &current, |_| {
                anyhow::bail!("network unavailable")
            })?;

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("network unavailable"));
        assert_eq!(
            fs::read_to_string(staged.join(TG_PROXY_ASSET_NAME))?,
            "old proxy"
        );
        assert_eq!(
            fs::read_to_string(staged.join(TG_PROXY_VERSION_FILE_NAME))?,
            "old version"
        );

        fs::remove_dir_all(root)?;
        Ok(())
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

    fn test_release() -> BundleRelease {
        BundleRelease {
            tag: "1.9.9a".to_owned(),
            release_url: "https://example.test/release".to_owned(),
            asset: ReleaseAsset {
                name: "zapret-discord-youtube-1.9.9a.zip".to_owned(),
                url: "https://example.test/bundle.zip".to_owned(),
                digest: None,
            },
        }
    }

    fn temp_root(prefix: &str) -> Result<PathBuf> {
        let root = env::temp_dir().join(format!("{prefix}-{}", timestamp()?));
        fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn write_current_bundle_user_file(bundle: &Path, content: &str) -> Result<()> {
        let lists_dir = bundle.join("lists");
        fs::create_dir_all(&lists_dir)?;
        fs::write(lists_dir.join("list-general-user.txt"), content)?;
        Ok(())
    }

    fn write_minimal_source_bundle(bundle: &Path) -> Result<()> {
        fs::create_dir_all(bundle.join("bin"))?;
        fs::create_dir_all(bundle.join("lists"))?;
        fs::create_dir_all(bundle.join("utils"))?;
        fs::write(bundle.join("bin").join("winws.exe"), "exe")?;
        fs::write(bundle.join("service.bat"), "@echo off\r\n")?;
        fs::write(
            bundle.join("utils").join("check_updates.enabled"),
            "ENABLED\r\n",
        )?;
        fs::write(
            bundle.join(SHA256SUMS_FILE_NAME),
            "abc  ./utils/check_updates.enabled\nabc  ./service.bat\n",
        )?;
        fs::write(
            bundle.join("lists").join("list-general.txt"),
            "discord.com\r\n",
        )?;
        for script_name in [
            "general (SIMPLE FAKE ALT2).bat",
            "general (ALT11).bat",
            "general (ALT12).bat",
            "general (FAKE TLS AUTO ALT3).bat",
            "general (ALT7).bat",
            "general.bat",
        ] {
            fs::write(
                bundle.join(script_name),
                r#"@echo off
start "zapret: %~n0" /min "%BIN%winws.exe"
"#,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Full "Обновить" flow against the real GitHub release: check → download → stage → swap.
    /// `cargo test -- --ignored live_bundle_update`
    #[test]
    #[ignore = "downloads the live bundle from GitHub"]
    fn live_bundle_update_installs_latest_release() -> Result<()> {
        let root = std::env::temp_dir().join(format!("zapret-hub-live-update-{}", timestamp()?));
        let bundle = root.join("bundle");
        // An "old" install with user data that must survive.
        fs::create_dir_all(bundle.join("bin"))?;
        fs::create_dir_all(bundle.join("lists"))?;
        fs::write(bundle.join("bin").join("winws.exe"), "old")?;
        fs::write(bundle.join("general (OLD).bat"), "old")?;
        fs::write(
            bundle.join("lists").join("list-general-user.txt"),
            "my.example\r\n",
        )?;
        write_bundle_version_metadata(&bundle, "1.0.0")?;

        let status = check_for_update(&bundle)?;
        assert!(status.update_available);
        let progress = DownloadProgress::default();
        let prepared = prepare_update(&bundle, &status.latest, &progress)?;
        assert_eq!(progress.fraction(), Some(1.0));
        assert!(
            prepared.work_dir.starts_with(&root),
            "staged next to the bundle"
        );
        apply_prepared_update(&bundle, &prepared)?;

        assert!(is_valid_bundle_dir(&bundle));
        assert_eq!(
            detect_bundle_version(&bundle).as_deref(),
            Some(status.latest.tag.as_str())
        );
        assert_eq!(
            fs::read_to_string(bundle.join("lists").join("list-general-user.txt"))?,
            "my.example\r\n"
        );
        assert!(!bundle.join("general (OLD).bat").exists());
        assert!(bundle.join("hub").join("telegram_proxy.cmd").is_file());
        let profile = fs::read_to_string(bundle.join("general (ALT).bat"))?;
        assert!(profile.contains(HIDDEN_WINWS_LAUNCH) && !profile.contains(UPSTREAM_WINWS_LAUNCH));
        assert!(!prepared.work_dir.exists());
        assert_eq!(
            fs::read_dir(&root)?.count(),
            1,
            "no leftovers next to the bundle"
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
