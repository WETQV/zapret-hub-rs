use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::core::config::{TelegramProxyMode, ZapretProfile};
use crate::core::process::{
    run_hidden_batch_wait, run_hidden_command_output, run_hidden_command_wait,
    run_visible_batch_detached, try_run_hidden_command,
};
use crate::core::status::ServiceState;

const MANAGED_WHITELIST_START: &str = "# Zapret Hub managed whitelist start";
const MANAGED_WHITELIST_END: &str = "# Zapret Hub managed whitelist end";
const MANAGED_CF_MEDIA_START: &str = "# Zapret Hub managed cf media start";
const MANAGED_CF_MEDIA_END: &str = "# Zapret Hub managed cf media end";
const BUILTIN_WHITELIST_FILE_NAME: &str = "builtin-whitelist.txt";
const EMBEDDED_BUILTIN_WHITELIST: &str = include_str!("../../assets/builtin-whitelist.txt");
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BundleAction {
    StartProfile {
        profile: ZapretProfile,
        use_builtin_whitelist: bool,
    },
    StopAll,
    RefreshIpset,
    InstallService,
    RemoveService,
    OpenServiceManager,
}

impl BundleAction {
    fn label(self) -> String {
        match self {
            Self::StartProfile {
                profile,
                use_builtin_whitelist,
            } => {
                if use_builtin_whitelist {
                    format!("{} profile started with whitelist", profile.label())
                } else {
                    format!("{} profile started", profile.label())
                }
            }
            Self::StopAll => "all known bypass processes stopped".to_owned(),
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

    Ok(action.label())
}

impl ZapretProfile {
    fn script_name(self) -> &'static str {
        match self {
            Self::General => "general.bat",
            Self::Alt => "general (ALT).bat",
            Self::Alt2 => "general (ALT2).bat",
            Self::Alt3 => "general (ALT3).bat",
            Self::Alt4 => "general (ALT4).bat",
            Self::Alt5 => "general (ALT5).bat",
            Self::Alt6 => "general (ALT6).bat",
            Self::Alt7 => "general (ALT7).bat",
            Self::Alt8 => "general (ALT8).bat",
            Self::Alt9 => "general (ALT9).bat",
            Self::Alt10 => "general (ALT10).bat",
            Self::Alt11 => "general (ALT11).bat",
            Self::FakeTlsAuto => "general (FAKE TLS AUTO).bat",
            Self::FakeTlsAutoAlt => "general (FAKE TLS AUTO ALT).bat",
            Self::FakeTlsAutoAlt2 => "general (FAKE TLS AUTO ALT2).bat",
            Self::FakeTlsAutoAlt3 => "general (FAKE TLS AUTO ALT3).bat",
            Self::SimpleFake => "general (SIMPLE FAKE).bat",
            Self::SimpleFakeAlt => "general (SIMPLE FAKE ALT).bat",
            Self::SimpleFakeAlt2 => "general (SIMPLE FAKE ALT2).bat",
        }
    }
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

    let secret = existing
        .as_ref()
        .map(|config| config.secret.trim())
        .filter(|secret| !secret.is_empty())
        .unwrap_or(TELEGRAM_PROXY_DEFAULT_SECRET)
        .to_owned();

    let config = match telegram_proxy.mode {
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
            check_updates: existing.as_ref().is_none_or(|config| config.check_updates),
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
            check_updates: existing.as_ref().is_none_or(|config| config.check_updates),
            cfproxy: true,
            cfproxy_priority: true,
            cfproxy_domain: telegram_proxy.cf_domain.trim().to_owned(),
            secret,
        },
    };

    let serialized = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, format!("{serialized}\n"))?;

    Ok(())
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
    stop_service_if_present(bundle_path, "zapret")?;
    kill_process_if_running(bundle_path, "winws.exe");
    kill_process_if_running(bundle_path, "TgWsProxy_windows.exe");
    let _ = stop_service_if_present(bundle_path, "WinDivert");
    let _ = stop_service_if_present(bundle_path, "WinDivert14");

    Ok(())
}

fn remove_service(bundle_path: &Path) -> Result<()> {
    stop_runtime(bundle_path)?;
    delete_service_if_present(bundle_path, "zapret");
    delete_service_if_present(bundle_path, "WinDivert");
    delete_service_if_present(bundle_path, "WinDivert14");

    Ok(())
}

fn stop_service_if_present(bundle_path: &Path, service_name: &str) -> Result<()> {
    match query_service_state(bundle_path, service_name)? {
        Some(ServiceState::Running | ServiceState::StopPending) => {
            try_run_hidden_command("net", &["stop", service_name], bundle_path);
            wait_for_service_stop(bundle_path, service_name)?;
        }
        Some(ServiceState::Stopped | ServiceState::NotInstalled | ServiceState::Unknown) | None => {
        }
    }

    Ok(())
}

fn wait_for_service_stop(bundle_path: &Path, service_name: &str) -> Result<()> {
    for _ in 0..8 {
        match query_service_state(bundle_path, service_name)? {
            Some(ServiceState::Stopped | ServiceState::NotInstalled) | None => return Ok(()),
            _ => thread::sleep(Duration::from_millis(350)),
        }
    }

    try_run_hidden_command("sc", &["stop", service_name], bundle_path);
    thread::sleep(Duration::from_millis(350));

    Ok(())
}

fn delete_service_if_present(bundle_path: &Path, service_name: &str) {
    let _ = run_hidden_command_wait("sc", &["delete", service_name], bundle_path);
}

fn kill_process_if_running(bundle_path: &Path, image_name: &str) {
    let _ = run_hidden_command_wait("taskkill", &["/IM", image_name, "/T", "/F"], bundle_path);
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

fn cf_media_domains(domain: &str) -> Vec<String> {
    let mut domains = Vec::with_capacity(7);
    domains.push(domain.to_owned());
    for suffix in ["kws1", "kws2", "kws3", "kws4", "kws5", "kws203"] {
        domains.push(format!("{suffix}.{domain}"));
    }
    domains
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
    fn all_profiles_map_to_general_batch_files() {
        let script_names: Vec<&str> = ZapretProfile::ALL
            .iter()
            .map(|profile| profile.script_name())
            .collect();

        assert_eq!(script_names.len(), 19);
        assert!(script_names.contains(&"general.bat"));
        assert!(script_names.contains(&"general (ALT11).bat"));
        assert!(script_names.contains(&"general (FAKE TLS AUTO ALT3).bat"));
        assert!(script_names.contains(&"general (SIMPLE FAKE ALT2).bat"));
        assert!(
            script_names
                .iter()
                .all(|script_name| script_name.starts_with("general")
                    && script_name.ends_with(".bat"))
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_millis();
        std::env::temp_dir().join(format!("{prefix}-{millis}"))
    }
}
