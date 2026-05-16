use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "Zapret Hub";
const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TelegramProxyMode {
    #[default]
    Standard,
    CfMedia,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ZapretProfile {
    General,
    Alt,
    Alt2,
    Alt3,
    Alt4,
    Alt5,
    Alt6,
    Alt7,
    Alt8,
    Alt9,
    Alt10,
    Alt11,
    FakeTlsAuto,
    FakeTlsAutoAlt,
    FakeTlsAutoAlt2,
    FakeTlsAutoAlt3,
    SimpleFake,
    SimpleFakeAlt,
    #[default]
    SimpleFakeAlt2,
}

impl ZapretProfile {
    pub(crate) const ALL: [Self; 19] = [
        Self::Alt,
        Self::Alt2,
        Self::Alt3,
        Self::Alt4,
        Self::Alt5,
        Self::Alt6,
        Self::Alt7,
        Self::Alt8,
        Self::Alt9,
        Self::Alt10,
        Self::Alt11,
        Self::FakeTlsAutoAlt,
        Self::FakeTlsAutoAlt2,
        Self::FakeTlsAutoAlt3,
        Self::FakeTlsAuto,
        Self::SimpleFakeAlt,
        Self::SimpleFakeAlt2,
        Self::SimpleFake,
        Self::General,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Alt => "ALT",
            Self::Alt2 => "ALT2",
            Self::Alt3 => "ALT3",
            Self::Alt4 => "ALT4",
            Self::Alt5 => "ALT5",
            Self::Alt6 => "ALT6",
            Self::Alt7 => "ALT7",
            Self::Alt8 => "ALT8",
            Self::Alt9 => "ALT9",
            Self::Alt10 => "ALT10",
            Self::Alt11 => "ALT11",
            Self::FakeTlsAuto => "FAKE TLS AUTO",
            Self::FakeTlsAutoAlt => "FAKE TLS AUTO ALT",
            Self::FakeTlsAutoAlt2 => "FAKE TLS AUTO ALT2",
            Self::FakeTlsAutoAlt3 => "FAKE TLS AUTO ALT3",
            Self::SimpleFake => "SIMPLE FAKE",
            Self::SimpleFakeAlt => "SIMPLE FAKE ALT",
            Self::SimpleFakeAlt2 => "SIMPLE FAKE ALT2",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AppConfig {
    #[serde(default)]
    pub(crate) autostart_enabled: bool,
    #[serde(default)]
    pub(crate) use_builtin_whitelist: bool,
    #[serde(default)]
    pub(crate) launch_telegram_proxy_for_profiles: bool,
    #[serde(default)]
    pub(crate) telegram_proxy_mode: TelegramProxyMode,
    #[serde(default)]
    pub(crate) telegram_cf_domain: String,
    #[serde(default)]
    pub(crate) dismissed_tg_proxy_release_tag: Option<String>,
    #[serde(default = "default_selected_tab")]
    pub(crate) selected_tab: String,
    #[serde(default)]
    pub(crate) dismissed_bundle_release_tag: Option<String>,
    #[serde(default)]
    pub(crate) main_profile: ZapretProfile,
    #[serde(default = "default_startup_notifications_enabled")]
    pub(crate) startup_notifications_enabled: bool,
    #[serde(default)]
    pub(crate) last_seen_app_version: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            autostart_enabled: false,
            use_builtin_whitelist: false,
            launch_telegram_proxy_for_profiles: false,
            telegram_proxy_mode: TelegramProxyMode::Standard,
            telegram_cf_domain: String::new(),
            dismissed_tg_proxy_release_tag: None,
            selected_tab: default_selected_tab(),
            dismissed_bundle_release_tag: None,
            main_profile: ZapretProfile::default(),
            startup_notifications_enabled: default_startup_notifications_enabled(),
            last_seen_app_version: None,
        }
    }
}

fn default_selected_tab() -> String {
    "main".to_owned()
}

fn default_startup_notifications_enabled() -> bool {
    true
}

pub(crate) fn load_app_config() -> Result<AppConfig> {
    let config_path = config_path()?;
    if !config_path.is_file() {
        return Ok(AppConfig::default());
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;

    let config: AppConfig = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    Ok(config)
}

pub(crate) fn save_app_config(config: &AppConfig) -> Result<()> {
    let config_path = config_path()?;
    let config_dir = config_path
        .parent()
        .context("config path has no parent directory")?;

    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;

    let content = serde_json::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(&config_path, content)
        .with_context(|| format!("failed to write {}", config_path.display()))
}

fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(SETTINGS_FILE_NAME))
}

fn config_dir() -> Result<PathBuf> {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        return Ok(PathBuf::from(local_app_data).join(APP_DIR_NAME));
    }

    if let Ok(app_data) = env::var("APPDATA") {
        return Ok(PathBuf::from(app_data).join(APP_DIR_NAME));
    }

    let current_exe = env::current_exe().context("failed to locate current executable")?;
    let exe_dir = current_exe
        .parent()
        .context("current executable has no parent directory")?;

    Ok(exe_dir.join("config"))
}
