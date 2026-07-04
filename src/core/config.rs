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
    pub(crate) fn script_name(self) -> &'static str {
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
    #[serde(default)]
    pub(crate) main_profile_script: Option<String>,
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
            main_profile_script: Some(ZapretProfile::default().script_name().to_owned()),
            startup_notifications_enabled: default_startup_notifications_enabled(),
            last_seen_app_version: None,
        }
    }
}

impl AppConfig {
    pub(crate) fn main_profile_script_or_legacy(&self) -> &str {
        self.main_profile_script
            .as_deref()
            .unwrap_or_else(|| self.main_profile.script_name())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_profile_script_falls_back_to_legacy_enum() {
        let config: AppConfig = serde_json::from_str(
            r#"{
  "main_profile": "alt11"
}"#,
        )
        .expect("legacy config parses");

        assert_eq!(
            config.main_profile_script_or_legacy(),
            "general (ALT11).bat"
        );
    }
}
