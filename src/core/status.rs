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

    parse_service_state(&output.stdout, &output.stderr)
}

fn parse_service_state(stdout: &[u8], stderr: &[u8]) -> ServiceState {
    let output = String::from_utf8_lossy(stdout).to_ascii_uppercase();
    let error = String::from_utf8_lossy(stderr).to_ascii_uppercase();

    if output.contains("FAILED 1060")
        || output.contains("DOES NOT EXIST")
        || error.contains("FAILED 1060")
        || error.contains("DOES NOT EXIST")
    {
        return ServiceState::NotInstalled;
    }

    if output.contains("RUNNING") {
        return ServiceState::Running;
    }

    if output.contains("STOP_PENDING") {
        return ServiceState::StopPending;
    }

    if output.contains("STOPPED") {
        return ServiceState::Stopped;
    }

    ServiceState::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_service_is_detected_from_stderr() {
        let state = parse_service_state(
            b"",
            b"[SC] EnumQueryServicesStatus:OpenService FAILED 1060:\r\nThe specified service does not exist as an installed service.",
        );

        assert_eq!(state, ServiceState::NotInstalled);
    }

    #[test]
    fn installed_service_states_are_detected_from_stdout() {
        assert_eq!(
            parse_service_state(b"STATE : 1 STOPPED", b""),
            ServiceState::Stopped
        );
        assert_eq!(
            parse_service_state(b"STATE : 3 STOP_PENDING", b""),
            ServiceState::StopPending
        );
        assert_eq!(
            parse_service_state(b"STATE : 4 RUNNING", b""),
            ServiceState::Running
        );
    }
}
