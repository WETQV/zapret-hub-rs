//! Single-instance handling and small Win32 helpers.
//!
//! A second launch does not try to find and poke the first window (it may be hidden in the
//! tray); it sets a named event instead, and the running instance shows itself.

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum InstanceCheck {
    Primary(SingleInstance),
    AlreadyRunning,
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
        WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    use super::InstanceCheck;

    const MUTEX_NAME: &str = "Local\\ZapretHubRs.SingleInstance";
    const SHOW_EVENT_NAME: &str = "Local\\ZapretHubRs.ShowWindow";

    pub(crate) struct SingleInstance {
        mutex: HANDLE,
        show_event: HANDLE,
    }

    // The handles are process-wide kernel objects; using them from another thread is fine.
    unsafe impl Send for SingleInstance {}

    impl SingleInstance {
        /// Calls `on_show` (on a background thread) every time another launch asks to show us.
        pub(crate) fn listen_for_show(&self, on_show: impl Fn() + Send + 'static) {
            let event = self.show_event as usize;
            std::thread::spawn(move || {
                loop {
                    let result = unsafe { WaitForSingleObject(event as HANDLE, INFINITE) };
                    if result != WAIT_OBJECT_0 {
                        return;
                    }
                    on_show();
                }
            });
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.mutex);
                // show_event stays open: the listener thread may still be waiting on it, and the
                // process is exiting anyway.
            }
        }
    }

    pub(crate) fn acquire() -> anyhow::Result<InstanceCheck> {
        let mutex_name = to_wide(MUTEX_NAME);
        let event_name = to_wide(SHOW_EVENT_NAME);

        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        if mutex.is_null() {
            anyhow::bail!("CreateMutexW failed");
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(mutex);
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !event.is_null() {
                    SetEvent(event);
                    CloseHandle(event);
                }
            }
            return Ok(InstanceCheck::AlreadyRunning);
        }

        // Auto-reset, initially non-signalled.
        let show_event = unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) };
        if show_event.is_null() {
            unsafe { CloseHandle(mutex) };
            anyhow::bail!("CreateEventW failed");
        }

        Ok(InstanceCheck::Primary(SingleInstance { mutex, show_event }))
    }

    pub(crate) fn show_message(title: &str, message: &str) {
        let title = to_wide(title);
        let message = to_wide(message);
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::InstanceCheck;

    pub(crate) struct SingleInstance;

    impl SingleInstance {
        pub(crate) fn listen_for_show(&self, _on_show: impl Fn() + Send + 'static) {}
    }

    pub(crate) fn acquire() -> anyhow::Result<InstanceCheck> {
        Ok(InstanceCheck::Primary(SingleInstance))
    }

    pub(crate) fn show_message(title: &str, message: &str) {
        eprintln!("{title}: {message}");
    }
}

pub(crate) use imp::{SingleInstance, acquire as acquire_single_instance, show_message};

/// Opens a link in the default browser. Goes through the OS directly: the `webbrowser` crate
/// used by egui mangles arguments of snap-packaged browsers on Linux.
pub(crate) fn open_url(url: &str) {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command.creation_flags(0x0800_0000);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command.spawn();
}

/// Opens a folder in Explorer (a GUI process, so no console window).
pub(crate) fn open_in_file_manager(path: &std::path::Path) {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(not(windows))]
    let program = "xdg-open";

    let _ = std::process::Command::new(program).arg(path).spawn();
}
