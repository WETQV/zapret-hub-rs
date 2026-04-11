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
    RuntimeStatus {
        winws_running: is_process_running(bundle_path, "winws.exe"),
        telegram_proxy_running: is_process_running(bundle_path, "TgWsProxy_windows.exe"),
        service_state: detect_service_state(bundle_path, "zapret"),
    }
}

fn is_process_running(current_dir: &Path, image_name: &str) -> bool {
    let output = run_hidden_command_output(
        "tasklist",
        &["/FI", &format!("IMAGENAME eq {image_name}")],
        current_dir,
    );

    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains(&image_name.to_ascii_lowercase()),
        Err(_) => false,
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
