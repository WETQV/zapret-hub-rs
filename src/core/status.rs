//! Runtime status straight from Win32 (process snapshot + service manager).
//! Polling used to spawn `tasklist` and `sc query` every two seconds.

pub(crate) const WINWS_IMAGE: &str = "winws.exe";
pub(crate) const TG_PROXY_IMAGE: &str = "TgWsProxy_windows.exe";
pub(crate) const ZAPRET_SERVICE: &str = "zapret";
pub(crate) const WINDIVERT_SERVICES: [&str; 2] = ["WinDivert", "WinDivert14"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum ServiceState {
    Unknown,
    NotInstalled,
    Stopped,
    StopPending,
    Running,
}

impl ServiceState {
    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::StopPending)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeStatus {
    pub(crate) winws_running: bool,
    pub(crate) telegram_proxy_running: bool,
    pub(crate) service_state: ServiceState,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            winws_running: false,
            telegram_proxy_running: false,
            service_state: ServiceState::Unknown,
        }
    }
}

impl RuntimeStatus {
    pub(crate) fn is_active(&self) -> bool {
        self.winws_running || self.telegram_proxy_running || self.service_state.is_active()
    }
}

pub(crate) fn refresh_runtime_status() -> RuntimeStatus {
    let processes = running_process_names();
    RuntimeStatus {
        winws_running: has_process(&processes, WINWS_IMAGE),
        telegram_proxy_running: has_process(&processes, TG_PROXY_IMAGE),
        service_state: query_service_state(ZAPRET_SERVICE),
    }
}

pub(crate) fn has_process(names: &[String], image_name: &str) -> bool {
    names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(image_name))
}

#[cfg(windows)]
pub(crate) fn running_process_names() -> Vec<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut names = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return names;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(entry.szExeFile.len());
            names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    names
}

#[cfg(not(windows))]
pub(crate) fn running_process_names() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
pub(crate) fn query_service_state(service_name: &str) -> ServiceState {
    use windows_sys::Win32::Foundation::{ERROR_SERVICE_DOES_NOT_EXIST, GetLastError};
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
        SERVICE_STOP_PENDING, SERVICE_STOPPED,
    };

    let name: Vec<u16> = service_name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return ServiceState::Unknown;
        }

        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        if service.is_null() {
            let error = GetLastError();
            CloseServiceHandle(manager);
            return if error == ERROR_SERVICE_DOES_NOT_EXIST {
                ServiceState::NotInstalled
            } else {
                ServiceState::Unknown
            };
        }

        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let ok = QueryServiceStatus(service, &mut status) != 0;
        CloseServiceHandle(service);
        CloseServiceHandle(manager);

        if !ok {
            return ServiceState::Unknown;
        }
        match status.dwCurrentState {
            SERVICE_RUNNING | SERVICE_START_PENDING => ServiceState::Running,
            SERVICE_STOP_PENDING => ServiceState::StopPending,
            SERVICE_STOPPED => ServiceState::Stopped,
            _ => ServiceState::Unknown,
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn query_service_state(_service_name: &str) -> ServiceState {
    ServiceState::NotInstalled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_names_match_case_insensitively() {
        let names = vec!["System".to_owned(), "WINWS.EXE".to_owned()];
        assert!(has_process(&names, WINWS_IMAGE));
        assert!(!has_process(&names, TG_PROXY_IMAGE));
    }

    #[test]
    fn stop_pending_service_counts_as_active_runtime() {
        let status = RuntimeStatus {
            winws_running: false,
            telegram_proxy_running: false,
            service_state: ServiceState::StopPending,
        };
        assert!(status.is_active());
        assert!(!RuntimeStatus::default().is_active());
    }
}
