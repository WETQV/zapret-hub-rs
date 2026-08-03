use std::path::Path;

use crate::core::process::run_hidden_command_output;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceState {
    Unknown,
    NotInstalled,
    Stopped,
    StopPending,
    Running,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeStatus {
    pub(crate) winws_running: bool,
    pub(crate) telegram_proxy_running: bool,
    pub(crate) service_state: ServiceState,
}

pub(crate) fn refresh_runtime_status(bundle_path: &Path) -> RuntimeStatus {
    let process_output = run_hidden_command_output("tasklist", &[], bundle_path)
        .map(|output| String::from_utf8_lossy(&output.stdout).to_ascii_lowercase())
        .unwrap_or_default();
    RuntimeStatus {
        winws_running: process_output.contains("winws.exe"),
        telegram_proxy_running: process_output.contains("tgwsproxy_windows.exe"),
        service_state: detect_service_state(bundle_path, "zapret"),
    }
}

fn detect_service_state(current_dir: &Path, service_name: &str) -> ServiceState {
    let output = run_hidden_command_output("sc", &["query", service_name], current_dir);

    let Ok(output) = output else {
        return ServiceState::Unknown;
    };

    let text = String::from_utf8_lossy(&output.stdout).to_ascii_uppercase();

    if text.contains("FAILED 1060") || text.contains("DOES NOT EXIST") {
        return ServiceState::NotInstalled;
    }

    if text.contains("RUNNING") {
        return ServiceState::Running;
    }

    if text.contains("STOP_PENDING") {
        return ServiceState::StopPending;
    }

    if text.contains("STOPPED") {
        return ServiceState::Stopped;
    }

    ServiceState::Unknown
}
