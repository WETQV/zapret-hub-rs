use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::core::config::TelegramProxyMode;
use crate::core::process::{
    run_hidden_batch_wait, run_hidden_command_output, run_hidden_command_wait,
    run_visible_batch_detached, try_run_hidden_command,
};
use crate::core::status::ServiceState;

const MANAGED_WHITELIST_START: &str = "# Zapret Hub managed whitelist start";
const MANAGED_WHITELIST_END: &str = "# Zapret Hub managed whitelist end";
const MANAGED_CF_MEDIA_START: &str = "# Zapret Hub managed cf media start";
const MANAGED_CF_MEDIA_END: &str = "# Zapret Hub managed cf media end";
const MANAGED_VRCHAT_START: &str = "# Zapret Hub managed VRChat start";
const MANAGED_VRCHAT_END: &str = "# Zapret Hub managed VRChat end";
const BUILTIN_WHITELIST_FILE_NAME: &str = "builtin-whitelist.txt";
const EMBEDDED_BUILTIN_WHITELIST: &str = include_str!("../../assets/builtin-whitelist.txt");
const VRCHAT_HOSTLIST_ENTRIES: &[&str] = &[
    "vrchat.com",
    "api.vrchat.cloud",
    "files.vrchat.cloud",
    "assets.vrchat.com",
    "vrcpm.vrchat.cloud",
    "*.vrcdn.cloud",
    "*.vrcdn.live",
    "*.vrcdn.video",
    "dbinj8iahsbec.cloudfront.net",
];
const TELEGRAM_PROXY_SCRIPT_NAME: &str = "telegram_proxy.cmd";
const TELEGRAM_PROXY_SILENT_SCRIPT_NAME: &str = "start_telegram_proxy_silent.cmd";
pub(crate) const TELEGRAM_PROXY_LOG_FILE_NAME: &str = "tgproxy-runtime.log";
pub(crate) const TELEGRAM_PROXY_LAUNCH_LOG_FILE_NAME: &str = "tgproxy-launch.log";
const TELEGRAM_PROXY_STANDARD_ARGS: &str =
    "--dc-ip 2:149.154.167.220 --dc-ip 4:149.154.167.220 --dc-ip 203:149.154.167.220";
const TELEGRAM_PROXY_CF_MEDIA_ARGS_PREFIX: &str = "--dc-ip 4:149.154.167.220 --cfproxy-domain ";
const TELEGRAM_PROXY_CONFIG_DIR_NAME: &str = "TgWsProxy";
const TELEGRAM_PROXY_CONFIG_FILE_NAME: &str = "config.json";
const TELEGRAM_PROXY_DEFAULT_SECRET: &str = "6c3c7e85fc245242b3d113cfe307b520";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BundleProfile {
    script_name: String,
    label: String,
}

impl BundleProfile {
    pub(crate) fn new(script_name: String) -> Option<Self> {
        if !is_profile_script_name(&script_name) {
            return None;
        }

        let label = profile_label_from_script_name(&script_name);
        Some(Self { script_name, label })
    }

    pub(crate) fn script_name(&self) -> &str {
        &self.script_name
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TelegramProxyLaunchConfig {
    pub(crate) enabled: bool,
    pub(crate) mode: TelegramProxyMode,
    pub(crate) cf_domain: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct TelegramProxyAppDataConfig {
    port: u16,
    host: String,
    dc_ip: Vec<String>,
    verbose: bool,
    autostart: bool,
    log_max_mb: u32,
    buf_kb: u32,
    pool_size: u32,
    check_updates: bool,
    cfproxy: bool,
    cfproxy_priority: bool,
    cfproxy_domain: String,
    secret: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BundleAction {
    StartProfile {
        profile: BundleProfile,
        use_builtin_whitelist: bool,
    },
    StopAll,
    RefreshIpset,
    InstallService,
    RemoveService,
    OpenServiceManager,
}

impl BundleAction {
    fn label(&self) -> String {
        match self {
            Self::StartProfile {
                profile,
                use_builtin_whitelist,
            } => {
                if *use_builtin_whitelist {
                    format!(
                        "{} profile started with whitelist and VRChat preset",
                        profile.label()
                    )
                } else {
                    format!("{} profile started with VRChat preset", profile.label())
                }
            }
            Self::StopAll => "bypass, proxy and services stopped".to_owned(),
            Self::RefreshIpset => "ipset list refreshed from bundled backup".to_owned(),
            Self::InstallService => "service installer launched".to_owned(),
            Self::RemoveService => "service removal completed".to_owned(),
            Self::OpenServiceManager => "service manager opened".to_owned(),
        }
    }
}

pub(crate) fn run_action(
    bundle_path: &Path,
    action: BundleAction,
    telegram_proxy: &TelegramProxyLaunchConfig,
) -> Result<String> {
    let label = action.label();

    match action {
        BundleAction::StartProfile {
            profile,
            use_builtin_whitelist,
        } => {
            sync_builtin_whitelist(bundle_path, use_builtin_whitelist)?;
            start_profile(bundle_path, profile.script_name(), telegram_proxy)?
        }
        BundleAction::StopAll => stop_runtime(bundle_path)?,
        BundleAction::RefreshIpset => refresh_ipset_from_backup(bundle_path)?,
        BundleAction::InstallService => {
            launch_visible_script(bundle_path, "install_service_simple_fake_alt2.cmd")?
        }
        BundleAction::RemoveService => remove_service(bundle_path)?,
        BundleAction::OpenServiceManager => launch_visible_root_script(bundle_path, "service.bat")?,
    }

    Ok(label)
}

pub(crate) fn discover_profiles(bundle_path: &Path) -> Result<Vec<BundleProfile>> {
    if !bundle_path.is_dir() {
        return Ok(Vec::new());
    }

    let mut profiles = Vec::new();
    for entry in fs::read_dir(bundle_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }

        let script_name = entry.file_name().to_string_lossy().to_string();
        if let Some(profile) = BundleProfile::new(script_name) {
            profiles.push(profile);
        }
    }

    profiles.sort_by(|left, right| {
        natural_sort_key(left.script_name()).cmp(&natural_sort_key(right.script_name()))
    });
    Ok(profiles)
}

pub(crate) fn find_profile_by_script(
    bundle_path: &Path,
    script_name: &str,
) -> Result<Option<BundleProfile>> {
    Ok(discover_profiles(bundle_path)?
        .into_iter()
        .find(|profile| profile.script_name().eq_ignore_ascii_case(script_name)))
}

fn is_profile_script_name(script_name: &str) -> bool {
    let lower = script_name.to_ascii_lowercase();
    lower.starts_with("general") && lower.ends_with(".bat")
}

fn profile_label_from_script_name(script_name: &str) -> String {
    let stem = script_name
        .strip_suffix(".bat")
        .unwrap_or(script_name)
        .trim();
    if stem.eq_ignore_ascii_case("general") {
        return "general".to_owned();
    }

    let label = stem
        .strip_prefix("general")
        .unwrap_or(stem)
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();

    if label.is_empty() {
        stem.to_owned()
    } else {
        label.to_owned()
    }
}

fn natural_sort_key(value: &str) -> String {
    let mut key = String::with_capacity(value.len() + 8);
    let mut digits = String::new();

    for char in value.chars() {
        if char.is_ascii_digit() {
            digits.push(char);
            continue;
        }

        if !digits.is_empty() {
            push_padded_digits(&mut key, &digits);
            digits.clear();
        }
        key.push(char.to_ascii_lowercase());
    }

    if !digits.is_empty() {
        push_padded_digits(&mut key, &digits);
    }

    key
}

fn push_padded_digits(target: &mut String, digits: &str) {
    target.push_str(&format!("{:0>8}", digits));
}

fn start_profile(
    bundle_path: &Path,
    profile_script: &str,
    telegram_proxy: &TelegramProxyLaunchConfig,
) -> Result<()> {
    let profile_path = bundle_path.join(profile_script);
    if !profile_path.is_file() {
        return Err(anyhow!(
            "profile script not found: {}",
            profile_path.display()
        ));
    }

    sync_vrchat_hostlist(bundle_path)?;
    sync_cf_media_hostlist(bundle_path, telegram_proxy)?;

    if telegram_proxy.enabled {
        reset_telegram_proxy_log(bundle_path)?;
        ensure_telegram_media_compat(bundle_path)?;
        sync_telegram_proxy_appdata_config(telegram_proxy)?;
        ensure_telegram_proxy_scripts(bundle_path, telegram_proxy)?;
    }

    run_hidden_batch_wait(&profile_path, bundle_path)?;

    if telegram_proxy.enabled {
        thread::sleep(Duration::from_secs(2));
        run_hidden_script(bundle_path, "telegram_proxy.cmd")?;
    }

    Ok(())
}

fn ensure_telegram_media_compat(bundle_path: &Path) -> Result<()> {
    let utils_dir = bundle_path.join("utils");
    fs::create_dir_all(&utils_dir)?;
    fs::write(utils_dir.join("game_filter.enabled"), "all\r\n")?;

    let lists_dir = bundle_path.join("lists");
    fs::create_dir_all(&lists_dir)?;
    fs::write(lists_dir.join("ipset-all.txt"), "")?;

    Ok(())
}

fn refresh_ipset_from_backup(bundle_path: &Path) -> Result<()> {
    let lists_dir = bundle_path.join("lists");
    let backup_path = lists_dir.join("ipset-all.txt.backup");
    let target_path = lists_dir.join("ipset-all.txt");

    if !backup_path.is_file() {
        return Err(anyhow!("ipset backup not found: {}", backup_path.display()));
    }

    fs::create_dir_all(&lists_dir)?;
    fs::copy(&backup_path, &target_path)?;
    Ok(())
}

fn reset_telegram_proxy_log(bundle_path: &Path) -> Result<()> {
    let log_path = bundle_path.join(TELEGRAM_PROXY_LOG_FILE_NAME);
    if log_path.exists() {
        fs::remove_file(&log_path)?;
    }

    let launch_log_path = bundle_path.join(TELEGRAM_PROXY_LAUNCH_LOG_FILE_NAME);
    if launch_log_path.exists() {
        fs::remove_file(&launch_log_path)?;
    }

    Ok(())
}

fn ensure_telegram_proxy_scripts(
    bundle_path: &Path,
    telegram_proxy: &TelegramProxyLaunchConfig,
) -> Result<()> {
    let hub_dir = bundle_path.join("hub");
    fs::create_dir_all(&hub_dir)?;
    fs::write(
        hub_dir.join(TELEGRAM_PROXY_SCRIPT_NAME),
        telegram_proxy_script_content(build_telegram_proxy_args(telegram_proxy)?),
    )?;
    fs::write(
        hub_dir.join(TELEGRAM_PROXY_SILENT_SCRIPT_NAME),
        start_telegram_proxy_silent_script_content(),
    )?;

    Ok(())
}

fn sync_telegram_proxy_appdata_config(telegram_proxy: &TelegramProxyLaunchConfig) -> Result<()> {
    let config_path = telegram_proxy_appdata_config_path()?;
    let config_dir = config_path
        .parent()
        .ok_or_else(|| anyhow!("telegram proxy config directory not found"))?;
    fs::create_dir_all(config_dir)?;

    let existing = fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<TelegramProxyAppDataConfig>(&content).ok());

    let config = telegram_proxy_appdata_config(telegram_proxy, existing.as_ref());
    let serialized = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, format!("{serialized}\n"))?;

    Ok(())
}

fn telegram_proxy_appdata_config(
    telegram_proxy: &TelegramProxyLaunchConfig,
    existing: Option<&TelegramProxyAppDataConfig>,
) -> TelegramProxyAppDataConfig {
    let secret = existing
        .map(|config| config.secret.trim())
        .filter(|secret| !secret.is_empty())
        .unwrap_or(TELEGRAM_PROXY_DEFAULT_SECRET)
        .to_owned();

    match telegram_proxy.mode {
        TelegramProxyMode::Standard => TelegramProxyAppDataConfig {
            port: 1080,
            host: "127.0.0.1".to_owned(),
            dc_ip: vec![
                "2:149.154.167.220".to_owned(),
                "4:149.154.167.220".to_owned(),
                "203:149.154.167.220".to_owned(),
            ],
            verbose: false,
            autostart: false,
            log_max_mb: existing.as_ref().map_or(5, |config| config.log_max_mb),
            buf_kb: existing.as_ref().map_or(256, |config| config.buf_kb),
            pool_size: existing.as_ref().map_or(4, |config| config.pool_size),
            check_updates: false,
            cfproxy: false,
            cfproxy_priority: false,
            cfproxy_domain: String::new(),
            secret,
        },
        TelegramProxyMode::CfMedia => TelegramProxyAppDataConfig {
            port: 1080,
            host: "127.0.0.1".to_owned(),
            dc_ip: vec!["4:149.154.167.220".to_owned()],
            verbose: false,
            autostart: false,
            log_max_mb: existing.as_ref().map_or(5, |config| config.log_max_mb),
            buf_kb: existing.as_ref().map_or(256, |config| config.buf_kb),
            pool_size: existing.as_ref().map_or(4, |config| config.pool_size),
            check_updates: false,
            cfproxy: true,
            cfproxy_priority: true,
            cfproxy_domain: telegram_proxy.cf_domain.trim().to_owned(),
            secret,
        },
    }
}

fn telegram_proxy_appdata_config_path() -> Result<PathBuf> {
    let appdata = env::var("APPDATA")
        .map_err(|_| anyhow!("APPDATA is not available, cannot configure Telegram proxy"))?;

    Ok(PathBuf::from(appdata)
        .join(TELEGRAM_PROXY_CONFIG_DIR_NAME)
        .join(TELEGRAM_PROXY_CONFIG_FILE_NAME))
}

fn build_telegram_proxy_args(telegram_proxy: &TelegramProxyLaunchConfig) -> Result<String> {
    match telegram_proxy.mode {
        TelegramProxyMode::Standard => Ok(TELEGRAM_PROXY_STANDARD_ARGS.to_owned()),
        TelegramProxyMode::CfMedia => {
            let domain = telegram_proxy.cf_domain.trim();
            if domain.is_empty() {
                return Err(anyhow!(
                    "cf media режим требует указать домен Cloudflare для Telegram proxy"
                ));
            }
            if domain
                .chars()
                .any(|char| char.is_whitespace() || char == '"')
            {
                return Err(anyhow!(
                    "домен Cloudflare для Telegram proxy содержит недопустимые символы"
                ));
            }

            Ok(format!("{TELEGRAM_PROXY_CF_MEDIA_ARGS_PREFIX}{domain}"))
        }
    }
}

fn telegram_proxy_script_content(telegram_proxy_args: String) -> String {
    format!(
        "@echo off\r\n\
chcp 65001 > nul\r\n\
cd /d \"%~dp0\"\r\n\
set \"ROOT=%~dp0..\"\r\n\
set \"TG_PROXY_ARGS={telegram_proxy_args}\"\r\n\
set \"TG_PROXY_LOG=%ROOT%\\{TELEGRAM_PROXY_LOG_FILE_NAME}\"\r\n\
set \"TG_PROXY_LAUNCH_LOG=%ROOT%\\{TELEGRAM_PROXY_LAUNCH_LOG_FILE_NAME}\"\r\n\
\r\n\
if not exist \"%ROOT%\\TgWsProxy_windows.exe\" (\r\n\
    call :log binary missing\r\n\
    exit /b 1\r\n\
)\r\n\
\r\n\
tasklist /FI \"IMAGENAME eq TgWsProxy_windows.exe\" | find /I \"TgWsProxy_windows.exe\" > nul\r\n\
if errorlevel 1 (\r\n\
    call :log launch requested\r\n\
    start \"\" /B \"%ROOT%\\TgWsProxy_windows.exe\" %TG_PROXY_ARGS% -v --log-file \"%TG_PROXY_LOG%\"\r\n\
    timeout /t 2 > nul\r\n\
    tasklist /FI \"IMAGENAME eq TgWsProxy_windows.exe\" | find /I \"TgWsProxy_windows.exe\" > nul\r\n\
    if errorlevel 1 (\r\n\
        call :log process missing after launch\r\n\
    ) else (\r\n\
        call :log process detected after launch\r\n\
    )\r\n\
) else (\r\n\
    call :log process already running\r\n\
)\r\n\
\r\n\
exit /b 0\r\n\
\r\n\
:log\r\n\
>>\"%TG_PROXY_LAUNCH_LOG%\" echo [%date% %time%] %*\r\n\
goto :eof\r\n"
    )
}

fn start_telegram_proxy_silent_script_content() -> &'static str {
    "@echo off\r\n\
chcp 65001 > nul\r\n\
cd /d \"%~dp0\"\r\n\
\r\n\
start \"\" /min \"%~dp0telegram_proxy.cmd\"\r\n"
}

fn run_hidden_script(bundle_path: &Path, script_name: &str) -> Result<()> {
    let script_path = hub_script_path(bundle_path, script_name);
    if !script_path.is_file() {
        return Err(anyhow!(
            "helper script not found: {}",
            script_path.display()
        ));
    }

    run_hidden_batch_wait(&script_path, bundle_path)
}

fn stop_runtime(bundle_path: &Path) -> Result<()> {
    request_service_stop(bundle_path, "zapret");
    request_process_stop(bundle_path, "winws.exe");
    request_process_stop(bundle_path, "TgWsProxy_windows.exe");
    request_service_stop(bundle_path, "WinDivert");
    request_service_stop(bundle_path, "WinDivert14");

    let remaining = remaining_runtime_items(bundle_path);
    if !remaining.is_empty() {
        return Err(anyhow!(
            "не удалось полностью остановить runtime: {}",
            remaining.join("; ")
        ));
    }

    Ok(())
}

fn remove_service(bundle_path: &Path) -> Result<()> {
    stop_runtime(bundle_path)?;
    delete_service_if_present(bundle_path, "zapret");
    delete_service_if_present(bundle_path, "WinDivert");
    delete_service_if_present(bundle_path, "WinDivert14");

    Ok(())
}

fn request_service_stop(bundle_path: &Path, service_name: &str) {
    let Ok(Some(ServiceState::Running | ServiceState::StopPending | ServiceState::Unknown)) =
        query_service_state(bundle_path, service_name)
    else {
        return;
    };

    try_run_hidden_command("net", &["stop", service_name], bundle_path);
    if wait_for_service_stop(bundle_path, service_name, 20) {
        return;
    }

    try_run_hidden_command("sc", &["stop", service_name], bundle_path);
    let _ = wait_for_service_stop(bundle_path, service_name, 10);
}

fn wait_for_service_stop(bundle_path: &Path, service_name: &str, attempts: usize) -> bool {
    for _ in 0..attempts {
        match query_service_state(bundle_path, service_name) {
            Ok(Some(ServiceState::Stopped | ServiceState::NotInstalled)) | Ok(None) => {
                return true;
            }
            _ => thread::sleep(Duration::from_millis(500)),
        }
    }

    false
}

fn delete_service_if_present(bundle_path: &Path, service_name: &str) {
    let _ = run_hidden_command_wait("sc", &["delete", service_name], bundle_path);
}

fn request_process_stop(bundle_path: &Path, image_name: &str) {
    for _ in 0..3 {
        if !is_process_running(bundle_path, image_name) {
            return;
        }

        try_run_hidden_command("taskkill", &["/IM", image_name, "/T", "/F"], bundle_path);
        if wait_for_process_stop(bundle_path, image_name, 10) {
            return;
        }
    }
}

fn wait_for_process_stop(bundle_path: &Path, image_name: &str, attempts: usize) -> bool {
    for _ in 0..attempts {
        if !is_process_running(bundle_path, image_name) {
            return true;
        }

        thread::sleep(Duration::from_millis(300));
    }

    false
}

fn remaining_runtime_items(bundle_path: &Path) -> Vec<String> {
    let mut remaining = Vec::new();

    for service_name in ["zapret", "WinDivert", "WinDivert14"] {
        match query_service_state(bundle_path, service_name) {
            Ok(Some(ServiceState::Running | ServiceState::StopPending | ServiceState::Unknown)) => {
                remaining.push(format!("service {service_name} is still active"));
            }
            Ok(Some(ServiceState::Stopped | ServiceState::NotInstalled)) | Ok(None) => {}
            Err(error) => remaining.push(format!("service {service_name} check failed: {error}")),
        }
    }

    for image_name in ["winws.exe", "TgWsProxy_windows.exe"] {
        if is_process_running(bundle_path, image_name) {
            remaining.push(format!("process {image_name} is still running"));
        }
    }

    remaining
}

fn is_process_running(bundle_path: &Path, image_name: &str) -> bool {
    let output = run_hidden_command_output(
        "tasklist",
        &["/FI", &format!("IMAGENAME eq {image_name}")],
        bundle_path,
    );

    match output {
        Ok(output) => tasklist_output_has_process(&output.stdout, image_name),
        Err(_) => false,
    }
}

fn tasklist_output_has_process(output: &[u8], image_name: &str) -> bool {
    String::from_utf8_lossy(output)
        .to_ascii_lowercase()
        .contains(&image_name.to_ascii_lowercase())
}

fn query_service_state(bundle_path: &Path, service_name: &str) -> Result<Option<ServiceState>> {
    let output = run_hidden_command_output("sc", &["query", service_name], bundle_path)?;
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_uppercase();
    let error = String::from_utf8_lossy(&output.stderr).to_ascii_uppercase();

    if text.contains("FAILED 1060")
        || text.contains("DOES NOT EXIST")
        || error.contains("FAILED 1060")
        || error.contains("DOES NOT EXIST")
    {
        return Ok(Some(ServiceState::NotInstalled));
    }

    if !output.status.success() && text.is_empty() && error.is_empty() {
        return Ok(None);
    }

    if text.contains("RUNNING") {
        return Ok(Some(ServiceState::Running));
    }

    if text.contains("STOP_PENDING") {
        return Ok(Some(ServiceState::StopPending));
    }

    if text.contains("STOPPED") {
        return Ok(Some(ServiceState::Stopped));
    }

    Ok(Some(ServiceState::Unknown))
}

fn launch_visible_script(bundle_path: &Path, script_name: &str) -> Result<()> {
    let script_path = hub_script_path(bundle_path, script_name);
    if !script_path.is_file() {
        return Err(anyhow!(
            "helper script not found: {}",
            script_path.display()
        ));
    }

    run_visible_batch_detached(&script_path, bundle_path)
}

fn launch_visible_root_script(bundle_path: &Path, script_name: &str) -> Result<()> {
    let script_path = bundle_path.join(script_name);
    if !script_path.is_file() {
        return Err(anyhow!("script not found: {}", script_path.display()));
    }

    run_visible_batch_detached(&script_path, bundle_path)
}

fn sync_builtin_whitelist(bundle_path: &Path, enabled: bool) -> Result<()> {
    let list_path = bundle_path.join("lists").join("list-exclude-user.txt");
    let existing_content = if list_path.is_file() {
        fs::read_to_string(&list_path)?
    } else {
        String::new()
    };
    let builtin_domains = load_builtin_whitelist_domains();

    let updated_content = apply_managed_block(
        &existing_content,
        &builtin_domains,
        enabled,
        MANAGED_WHITELIST_START,
        MANAGED_WHITELIST_END,
    );

    if let Some(parent) = list_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(list_path, updated_content)?;
    Ok(())
}

fn sync_cf_media_hostlist(
    bundle_path: &Path,
    telegram_proxy: &TelegramProxyLaunchConfig,
) -> Result<()> {
    let list_path = bundle_path.join("lists").join("list-general-user.txt");
    let existing_content = if list_path.is_file() {
        fs::read_to_string(&list_path)?
    } else {
        String::new()
    };

    let enabled = telegram_proxy.enabled && telegram_proxy.mode == TelegramProxyMode::CfMedia;
    let cf_domains = if enabled {
        cf_media_domains(telegram_proxy.cf_domain.trim())
    } else {
        Vec::new()
    };

    let updated_content = apply_managed_block(
        &existing_content,
        &cf_domains,
        enabled,
        MANAGED_CF_MEDIA_START,
        MANAGED_CF_MEDIA_END,
    );

    if let Some(parent) = list_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(list_path, updated_content)?;
    Ok(())
}

fn sync_vrchat_hostlist(bundle_path: &Path) -> Result<()> {
    let list_path = bundle_path.join("lists").join("list-general-user.txt");
    let existing_content = if list_path.is_file() {
        fs::read_to_string(&list_path)?
    } else {
        String::new()
    };
    let vrchat_domains = vrchat_domains();

    let updated_content = apply_managed_block(
        &existing_content,
        &vrchat_domains,
        true,
        MANAGED_VRCHAT_START,
        MANAGED_VRCHAT_END,
    );

    if let Some(parent) = list_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(list_path, updated_content)?;
    Ok(())
}

fn cf_media_domains(domain: &str) -> Vec<String> {
    let mut domains = Vec::with_capacity(7);
    domains.push(domain.to_owned());
    for suffix in ["kws1", "kws2", "kws3", "kws4", "kws5", "kws203"] {
        domains.push(format!("{suffix}.{domain}"));
    }
    domains
}

fn vrchat_domains() -> Vec<String> {
    VRCHAT_HOSTLIST_ENTRIES
        .iter()
        .map(|entry| (*entry).to_owned())
        .collect()
}

fn apply_managed_block(
    existing_content: &str,
    managed_domains: &[String],
    enabled: bool,
    block_start: &str,
    block_end: &str,
) -> String {
    let mut lines = Vec::new();
    let mut skipping_managed_block = false;

    for line in existing_content.lines() {
        if line == block_start {
            skipping_managed_block = true;
            continue;
        }
        if line == block_end {
            skipping_managed_block = false;
            continue;
        }
        if !skipping_managed_block {
            lines.push(line);
        }
    }

    while matches!(lines.last(), Some(line) if line.trim().is_empty()) {
        lines.pop();
    }

    let mut content = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\r\n", lines.join("\r\n"))
    };

    if enabled {
        if !content.is_empty() {
            content.push_str("\r\n");
        }
        content.push_str(block_start);
        content.push_str("\r\n");
        for domain in managed_domains {
            content.push_str(domain);
            content.push_str("\r\n");
        }
        content.push_str(block_end);
        content.push_str("\r\n");
    }

    content
}

fn load_builtin_whitelist_domains() -> Vec<String> {
    let file_content = whitelist_file_path()
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_else(|| EMBEDDED_BUILTIN_WHITELIST.to_owned());

    file_content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

fn whitelist_file_path() -> Result<PathBuf> {
    let current_exe = env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent directory"))?;

    Ok(exe_dir.join(BUILTIN_WHITELIST_FILE_NAME))
}

fn hub_script_path(bundle_path: &Path, script_name: &str) -> PathBuf {
    bundle_path.join("hub").join(script_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn refresh_ipset_copies_backup_to_active_list() {
        let bundle_dir = unique_temp_dir("zapret-hub-ipset-test");
        let lists_dir = bundle_dir.join("lists");
        fs::create_dir_all(&lists_dir).expect("lists dir created");
        fs::write(lists_dir.join("ipset-all.txt"), "203.0.113.113/32\r\n")
            .expect("old ipset written");
        fs::write(
            lists_dir.join("ipset-all.txt.backup"),
            "1.1.1.0/24\r\n8.8.8.0/24\r\n",
        )
        .expect("backup ipset written");

        refresh_ipset_from_backup(&bundle_dir).expect("ipset refreshed");

        let refreshed =
            fs::read_to_string(lists_dir.join("ipset-all.txt")).expect("refreshed ipset read");
        assert_eq!(refreshed, "1.1.1.0/24\r\n8.8.8.0/24\r\n");

        fs::remove_dir_all(bundle_dir).expect("temp bundle removed");
    }

    #[test]
    fn discover_profiles_includes_new_upstream_profiles_and_excludes_service() {
        let bundle_dir = unique_temp_dir("zapret-hub-profile-discovery-test");
        fs::create_dir_all(&bundle_dir).expect("temp bundle created");

        for script_name in [
            "service.bat",
            "general (ALT2).bat",
            "general (ALT12).bat",
            "general.bat",
            "not-general.bat",
        ] {
            fs::write(bundle_dir.join(script_name), "@echo off\r\n").expect("script written");
        }

        let profiles = discover_profiles(&bundle_dir).expect("profiles discovered");
        let script_names: Vec<&str> = profiles.iter().map(BundleProfile::script_name).collect();

        assert_eq!(
            script_names,
            vec!["general (ALT2).bat", "general (ALT12).bat", "general.bat"]
        );
        assert_eq!(profiles[1].label(), "ALT12");

        fs::remove_dir_all(bundle_dir).expect("temp bundle removed");
    }

    #[test]
    fn tasklist_output_detects_running_process_by_image_name() {
        let output =
            b"Image Name                     PID Session Name        Session#    Mem Usage\r\n\
winws.exe                    1234 Console                    1      1,024 K\r\n";

        assert!(tasklist_output_has_process(output, "winws.exe"));
        assert!(!tasklist_output_has_process(
            output,
            "TgWsProxy_windows.exe"
        ));
    }

    #[test]
    fn telegram_proxy_appdata_config_disables_proxy_update_checks() {
        let existing = TelegramProxyAppDataConfig {
            port: 1080,
            host: "127.0.0.1".to_owned(),
            dc_ip: vec!["2:149.154.167.220".to_owned()],
            verbose: true,
            autostart: true,
            log_max_mb: 12,
            buf_kb: 512,
            pool_size: 8,
            check_updates: true,
            cfproxy: false,
            cfproxy_priority: false,
            cfproxy_domain: String::new(),
            secret: "custom-secret".to_owned(),
        };

        let standard = telegram_proxy_appdata_config(
            &TelegramProxyLaunchConfig {
                enabled: true,
                mode: TelegramProxyMode::Standard,
                cf_domain: String::new(),
            },
            Some(&existing),
        );
        let cf_media = telegram_proxy_appdata_config(
            &TelegramProxyLaunchConfig {
                enabled: true,
                mode: TelegramProxyMode::CfMedia,
                cf_domain: "example.test".to_owned(),
            },
            Some(&existing),
        );

        assert!(!standard.check_updates);
        assert!(!cf_media.check_updates);
        assert_eq!(standard.secret, "custom-secret");
        assert_eq!(cf_media.secret, "custom-secret");
    }

    #[test]
    fn vrchat_hostlist_is_added_once() {
        let bundle_dir = unique_temp_dir("zapret-hub-vrchat-once-test");
        let profile_path = bundle_dir.join("lists").join("list-general-user.txt");

        sync_vrchat_hostlist(&bundle_dir).expect("vrchat hostlist synced");
        sync_vrchat_hostlist(&bundle_dir).expect("vrchat hostlist synced again");

        let content = fs::read_to_string(profile_path).expect("hostlist read");
        assert_eq!(content.matches(MANAGED_VRCHAT_START).count(), 1);
        assert_eq!(content.matches("api.vrchat.cloud").count(), 1);
        assert!(content.contains("dbinj8iahsbec.cloudfront.net"));

        fs::remove_dir_all(bundle_dir).expect("temp bundle removed");
    }

    #[test]
    fn vrchat_hostlist_preserves_user_entries() {
        let bundle_dir = unique_temp_dir("zapret-hub-vrchat-user-test");
        let lists_dir = bundle_dir.join("lists");
        fs::create_dir_all(&lists_dir).expect("lists dir created");
        fs::write(
            lists_dir.join("list-general-user.txt"),
            "custom.example\r\n\r\n",
        )
        .expect("user hostlist written");

        sync_vrchat_hostlist(&bundle_dir).expect("vrchat hostlist synced");

        let content =
            fs::read_to_string(lists_dir.join("list-general-user.txt")).expect("hostlist read");
        assert!(content.starts_with("custom.example\r\n\r\n"));
        assert!(content.contains(MANAGED_VRCHAT_START));
        assert!(content.contains("*.vrcdn.cloud"));

        fs::remove_dir_all(bundle_dir).expect("temp bundle removed");
    }

    #[test]
    fn vrchat_and_cf_media_blocks_coexist() {
        let bundle_dir = unique_temp_dir("zapret-hub-vrchat-cf-test");
        let telegram_proxy = TelegramProxyLaunchConfig {
            enabled: true,
            mode: TelegramProxyMode::CfMedia,
            cf_domain: "media.example".to_owned(),
        };

        sync_vrchat_hostlist(&bundle_dir).expect("vrchat hostlist synced");
        sync_cf_media_hostlist(&bundle_dir, &telegram_proxy).expect("cf media synced");
        sync_vrchat_hostlist(&bundle_dir).expect("vrchat hostlist synced again");

        let content = fs::read_to_string(bundle_dir.join("lists").join("list-general-user.txt"))
            .expect("hostlist read");
        assert_eq!(content.matches(MANAGED_VRCHAT_START).count(), 1);
        assert_eq!(content.matches(MANAGED_CF_MEDIA_START).count(), 1);
        assert!(content.contains("api.vrchat.cloud"));
        assert!(content.contains("kws203.media.example"));

        fs::remove_dir_all(bundle_dir).expect("temp bundle removed");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_millis();
        std::env::temp_dir().join(format!("{prefix}-{millis}"))
    }
}
