use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use crate::core::bundle_update::patch_profile_script;
use crate::core::config::TelegramProxyMode;
use crate::core::process::{
    run_hidden_batch_wait, run_hidden_command_wait, run_visible_batch_detached,
    try_run_hidden_command,
};
use crate::core::status::{
    TG_PROXY_IMAGE, WINDIVERT_SERVICES, WINWS_IMAGE, ZAPRET_SERVICE, has_process,
    query_service_state, running_process_names,
};

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
const STOP_RUNTIME_TIMEOUT: Duration = Duration::from_secs(8);
const WINWS_START_TIMEOUT: Duration = Duration::from_secs(4);
const TELEGRAM_PROXY_STANDARD_ARGS: &str = "--dc-ip 2:149.154.167.220 --dc-ip 4:149.154.167.220";
const TELEGRAM_PROXY_CF_MEDIA_ARGS_PREFIX: &str = "--dc-ip 4:149.154.167.220 --cfproxy-domain ";
const TELEGRAM_PROXY_CONFIG_DIR_NAME: &str = "TgWsProxy";
const TELEGRAM_PROXY_CONFIG_FILE_NAME: &str = "config.json";
const TELEGRAM_PROXY_PORT: u16 = 1443;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BundleAction {
    StartProfile {
        profile: BundleProfile,
        use_builtin_whitelist: bool,
    },
    StopAll,
    RefreshIpset,
    RemoveService,
    OpenServiceManager,
}

impl BundleAction {
    fn done_message(&self) -> String {
        match self {
            Self::StartProfile { profile, .. } => format!("Профиль {} запущен.", profile.label()),
            Self::StopAll => "Обход остановлен.".to_owned(),
            Self::RefreshIpset => "ipset восстановлен из ipset-all.txt.backup.".to_owned(),
            Self::RemoveService => "Служба zapret удалена, WinDivert выгружен.".to_owned(),
            Self::OpenServiceManager => {
                "service.bat открыт в отдельном окне — завершите действие там.".to_owned()
            }
        }
    }
}

pub(crate) fn run_action(
    bundle_path: &Path,
    action: BundleAction,
    telegram_proxy: &TelegramProxyLaunchConfig,
) -> Result<String> {
    let message = action.done_message();

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
        BundleAction::RemoveService => remove_service(bundle_path)?,
        BundleAction::OpenServiceManager => launch_visible_root_script(bundle_path, "service.bat")?,
    }

    Ok(message)
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

    patch_profile_script(&profile_path)?;
    sync_vrchat_hostlist(bundle_path)?;
    sync_cf_media_hostlist(bundle_path, telegram_proxy)?;

    if telegram_proxy.enabled {
        reset_telegram_proxy_log(bundle_path)?;
        ensure_telegram_media_compat(bundle_path)?;
        sync_telegram_proxy_appdata_config(telegram_proxy)?;
        ensure_telegram_proxy_scripts(bundle_path, telegram_proxy)?;
    }

    run_hidden_batch_wait(&profile_path, bundle_path)?;
    ensure_winws_started(bundle_path)?;

    if telegram_proxy.enabled {
        thread::sleep(Duration::from_secs(2));
        run_hidden_script(bundle_path, TELEGRAM_PROXY_SCRIPT_NAME)?;
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

    let existing = fs::read_to_string(&config_path).ok().and_then(|content| {
        serde_json::from_str::<serde_json::Value>(content.trim_start_matches('\u{feff}')).ok()
    });

    let config = telegram_proxy_appdata_config(telegram_proxy, existing);
    let serialized = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, format!("{serialized}\n"))?;

    Ok(())
}

/// The proxy is an MTProto server on 127.0.0.1:1443 whose settings live in `config.json`.
/// Only the keys Zapret Hub owns are overwritten; everything else (secret, buffers, language,
/// worker domains, …) is kept, and an absent secret is generated by the proxy on first start.
fn telegram_proxy_appdata_config(
    telegram_proxy: &TelegramProxyLaunchConfig,
    existing: Option<serde_json::Value>,
) -> serde_json::Value {
    use serde_json::{Value, json};

    let mut config = match existing {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let (dc_ip, user_domain) = match telegram_proxy.mode {
        TelegramProxyMode::Standard => (vec!["2:149.154.167.220", "4:149.154.167.220"], Vec::new()),
        // Media-only DC4 through the user's own Cloudflare domain.
        TelegramProxyMode::CfMedia => (
            vec!["4:149.154.167.220"],
            vec![telegram_proxy.cf_domain.trim().to_owned()],
        ),
    };
    let cf_media = telegram_proxy.mode == TelegramProxyMode::CfMedia;

    config.insert("host".into(), json!("127.0.0.1"));
    config.insert("port".into(), json!(TELEGRAM_PROXY_PORT));
    config.insert("dc_ip".into(), json!(dc_ip));
    config.insert("check_updates".into(), json!(false));
    config.insert("autostart".into(), json!(false));
    config.insert("cfproxy".into(), json!(true));
    config.insert("cfproxy_user_domain_enabled".into(), json!(cf_media));
    config.insert("cfproxy_user_domain".into(), json!(user_domain));
    // Stale keys of the pre-MTProto config format.
    for stale in ["cfproxy_domain", "cfproxy_priority"] {
        config.remove(stale);
    }
    if config
        .get("secret")
        .and_then(Value::as_str)
        .is_none_or(|secret| secret.trim().len() != 32)
    {
        config.remove("secret");
    }
    Value::Object(config)
}

/// `tg://proxy` link for Telegram Desktop, once the proxy has generated its secret.
pub(crate) fn telegram_proxy_link() -> Option<String> {
    let content = fs::read_to_string(telegram_proxy_appdata_config_path().ok()?).ok()?;
    let config: serde_json::Value =
        serde_json::from_str(content.trim_start_matches('\u{feff}')).ok()?;
    let secret = config.get("secret")?.as_str()?.trim();
    if secret.len() != 32 || !secret.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let port = config
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(u64::from(TELEGRAM_PROXY_PORT));
    Some(format!(
        "tg://proxy?server=127.0.0.1&port={port}&secret=dd{secret}"
    ))
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
    ping -n 3 127.0.0.1 > nul\r\n\
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

/// Stops only the Telegram proxy (e.g. to replace its executable). Returns whether it was running.
pub(crate) fn stop_telegram_proxy(bundle_path: &Path) -> bool {
    if !has_process(&running_process_names(), TG_PROXY_IMAGE) {
        return false;
    }
    try_run_hidden_command(
        "taskkill",
        &["/IM", TG_PROXY_IMAGE, "/T", "/F"],
        bundle_path,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while has_process(&running_process_names(), TG_PROXY_IMAGE) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    true
}

/// Starts the proxy again with the arguments of the last profile launch.
pub(crate) fn restart_telegram_proxy(bundle_path: &Path) -> Result<()> {
    run_hidden_script(bundle_path, TELEGRAM_PROXY_SCRIPT_NAME)
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

/// The profile .bat only spawns winws.exe and exits, so give it a moment to actually appear.
fn ensure_winws_started(bundle_path: &Path) -> Result<()> {
    let deadline = Instant::now() + WINWS_START_TIMEOUT;
    while Instant::now() < deadline {
        if has_process(&running_process_names(), WINWS_IMAGE) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }

    let service_running = query_service_state(ZAPRET_SERVICE).is_active();
    Err(anyhow!(if service_running {
        "winws.exe не запустился: уже работает служба zapret. Удалите её в «Настройки → Служба»."
            .to_owned()
    } else {
        format!(
            "winws.exe не запустился. Проверьте, что антивирус не удалил {}",
            bundle_path.join("bin").join(WINWS_IMAGE).display()
        )
    }))
}

pub(crate) fn stop_runtime(bundle_path: &Path) -> Result<()> {
    request_runtime_stop(bundle_path);
    let deadline = Instant::now() + STOP_RUNTIME_TIMEOUT;
    let mut retried = false;
    let mut remaining = remaining_runtime_items();
    while !remaining.is_empty() && Instant::now() < deadline {
        if !retried && Instant::now() + Duration::from_secs(4) >= deadline {
            request_runtime_stop(bundle_path);
            retried = true;
        }
        thread::sleep(Duration::from_millis(250));
        remaining = remaining_runtime_items();
    }
    if !remaining.is_empty() {
        return Err(anyhow!(
            "не удалось остановить за 8 секунд: {}",
            remaining.join("; ")
        ));
    }

    Ok(())
}

fn request_runtime_stop(bundle_path: &Path) {
    try_run_hidden_command("sc", &["stop", ZAPRET_SERVICE], bundle_path);
    let processes = running_process_names();
    for image_name in [WINWS_IMAGE, TG_PROXY_IMAGE] {
        if has_process(&processes, image_name) {
            try_run_hidden_command("taskkill", &["/IM", image_name, "/T", "/F"], bundle_path);
        }
    }
    // Unloads the driver so bin\WinDivert64.sys is no longer locked (needed for bundle updates).
    for service_name in WINDIVERT_SERVICES {
        if query_service_state(service_name).is_active() {
            try_run_hidden_command("sc", &["stop", service_name], bundle_path);
        }
    }
}

fn remove_service(bundle_path: &Path) -> Result<()> {
    stop_runtime(bundle_path)?;
    for service_name in [ZAPRET_SERVICE, WINDIVERT_SERVICES[0], WINDIVERT_SERVICES[1]] {
        let _ = run_hidden_command_wait("sc", &["delete", service_name], bundle_path);
    }

    Ok(())
}

fn remaining_runtime_items() -> Vec<String> {
    let mut remaining = Vec::new();

    for service_name in [ZAPRET_SERVICE, WINDIVERT_SERVICES[0], WINDIVERT_SERVICES[1]] {
        if query_service_state(service_name).is_active() {
            remaining.push(format!("служба {service_name}"));
        }
    }

    let processes = running_process_names();
    for image_name in [WINWS_IMAGE, TG_PROXY_IMAGE] {
        if has_process(&processes, image_name) {
            remaining.push(format!("процесс {image_name}"));
        }
    }

    remaining
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
    fn runtime_stop_uses_one_eight_second_deadline() {
        assert_eq!(STOP_RUNTIME_TIMEOUT, Duration::from_secs(8));
    }

    #[test]
    fn telegram_proxy_config_uses_mtproto_schema_and_keeps_user_keys() {
        let existing = serde_json::json!({
            "port": 1080, "secret": "6c3c7e85fc245242b3d113cfe307b520",
            "check_updates": true, "language": "ru", "cfproxy_domain": "old.example"
        });
        let standard = telegram_proxy_appdata_config(
            &TelegramProxyLaunchConfig {
                enabled: true,
                mode: TelegramProxyMode::Standard,
                cf_domain: String::new(),
            },
            Some(existing.clone()),
        );
        let cf = telegram_proxy_appdata_config(
            &TelegramProxyLaunchConfig {
                enabled: true,
                mode: TelegramProxyMode::CfMedia,
                cf_domain: " media.example ".to_owned(),
            },
            Some(existing),
        );
        assert_eq!(standard["port"], 1443);
        assert_eq!(standard["check_updates"], false);
        assert_eq!(standard["language"], "ru");
        assert_eq!(standard["secret"], "6c3c7e85fc245242b3d113cfe307b520");
        assert_eq!(standard["cfproxy_user_domain_enabled"], false);
        assert!(standard.get("cfproxy_domain").is_none());
        assert_eq!(cf["cfproxy_user_domain_enabled"], true);
        assert_eq!(cf["cfproxy_user_domain"][0], "media.example");
        assert_eq!(cf["dc_ip"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn invalid_secret_is_dropped_so_the_proxy_generates_one() {
        let config = telegram_proxy_appdata_config(
            &TelegramProxyLaunchConfig {
                enabled: true,
                mode: TelegramProxyMode::Standard,
                cf_domain: String::new(),
            },
            Some(serde_json::json!({"secret": "short"})),
        );
        assert!(config.get("secret").is_none());
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
