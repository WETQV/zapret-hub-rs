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
        }
    }
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
