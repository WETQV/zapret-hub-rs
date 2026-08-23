#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod zapret;

use eframe::egui;
use eframe::egui::IconData;
use image::ImageFormat;

const APP_WINDOW_TITLE: &str = "Zapret Hub";

fn main() -> eframe::Result {
    let launch_mode = LaunchMode::detect();

    #[cfg(windows)]
    let _instance_guard = match SingleInstanceGuard::acquire(!launch_mode.is_autostart()) {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => {
            show_startup_error(&format!(
                "Не удалось проверить второй экземпляр приложения: {error}"
            ));
            return Ok(());
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([860.0, 560.0])
            .with_active(!launch_mode.is_autostart())
            .with_icon(load_app_icon().unwrap_or_default()),
        ..Default::default()
    };

    eframe::run_native(
        APP_WINDOW_TITLE,
        native_options,
        Box::new(move |cc| Ok(Box::new(app::ZapretHubApp::new(cc, launch_mode)))),
    )
}

fn load_app_icon() -> anyhow::Result<IconData> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/icons/app.ico"),
        ImageFormat::Ico,
    )?
    .into_rgba8();
    let (width, height) = image.dimensions();

    Ok(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

#[cfg(windows)]
struct SingleInstanceGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl SingleInstanceGuard {
    fn acquire(show_existing_instance_message: bool) -> anyhow::Result<Option<Self>> {
        use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = to_wide("Local\\ZapretHubRs.SingleInstance");
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };

        if handle.is_null() {
            anyhow::bail!("CreateMutexW failed");
        }

        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            if show_existing_instance_message && !activate_existing_window() {
                show_info_message(
                    "Zapret Hub уже запущен",
                    "Приложение уже работает, но его окно пока не удалось восстановить.",
                );
            }
            return Ok(None);
        }

        Ok(Some(Self(handle)))
    }
}

#[cfg(windows)]
fn activate_existing_window() -> bool {
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };

    let title = to_wide(APP_WINDOW_TITLE);
    for _ in 0..20 {
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            unsafe {
                let show_command = if IsIconic(window) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                };
                ShowWindow(window, show_command);
                SetForegroundWindow(window);
            }
            return true;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    false
}

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn show_startup_error(message: &str) {
    show_info_message("Ошибка запуска", message);
}

#[cfg(windows)]
fn show_info_message(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};

    let title = to_wide(title);
    let message = to_wide(message);

    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchMode {
    Normal,
    Autostart,
}

impl LaunchMode {
    fn detect() -> Self {
        let is_autostart = std::env::args()
            .skip(1)
            .any(|argument| argument.eq_ignore_ascii_case("--autostart"));

        if is_autostart {
            Self::Autostart
        } else {
            Self::Normal
        }
    }

    pub(crate) fn is_autostart(self) -> bool {
        matches!(self, Self::Autostart)
    }
}
