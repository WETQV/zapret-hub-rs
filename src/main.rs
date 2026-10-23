#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod platform;
mod tray;
mod zapret;

use eframe::egui;
use image::ImageFormat;

use crate::platform::{InstanceCheck, acquire_single_instance, show_message};

const APP_WINDOW_TITLE: &str = "Zapret Hub";
const TRAY_ICON_SIZE: u32 = 32;

pub(crate) struct AppIcon {
    pub(crate) window: egui::IconData,
    pub(crate) tray_rgba: Vec<u8>,
    pub(crate) tray_size: u32,
}

fn main() -> eframe::Result {
    if let Some(exit_code) = run_cli() {
        std::process::exit(exit_code);
    }

    let launch_mode = LaunchMode::detect();

    let instance = match acquire_single_instance() {
        Ok(InstanceCheck::Primary(instance)) => instance,
        // The running instance got the "show" signal and brings itself up.
        Ok(InstanceCheck::AlreadyRunning) => return Ok(()),
        Err(error) => {
            show_message(
                "Ошибка запуска",
                &format!("Не удалось проверить второй экземпляр приложения: {error}"),
            );
            return Ok(());
        }
    };

    let icon = load_app_icon().ok();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_WINDOW_TITLE)
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([780.0, 540.0])
            // Zapret Hub draws its own title bar (see app/chrome.rs).
            .with_decorations(false)
            .with_visible(!launch_mode.is_autostart())
            .with_icon(
                icon.as_ref()
                    .map(|icon| icon.window.clone())
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };

    eframe::run_native(
        APP_WINDOW_TITLE,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::ZapretHubApp::new(
                cc,
                launch_mode,
                instance,
                icon,
            )))
        }),
    )
}

/// Non-GUI entry points for the packaging scripts. `None` means "start the app".
fn run_cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
    };

    let target = value_of("--stage-bundle")?;
    let tag = value_of("--bundle-tag").map(String::as_str);
    match core::bundle_update::stage_release_bundle(std::path::Path::new(target), tag) {
        Ok(message) => {
            println!("{message}");
            Some(0)
        }
        Err(error) => {
            eprintln!("stage failed: {error:#}");
            Some(1)
        }
    }
}

fn load_app_icon() -> anyhow::Result<AppIcon> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/icons/app.ico"),
        ImageFormat::Ico,
    )?;
    let tray = image
        .resize_exact(
            TRAY_ICON_SIZE,
            TRAY_ICON_SIZE,
            image::imageops::FilterType::Lanczos3,
        )
        .into_rgba8();
    let window = image.into_rgba8();
    let (width, height) = window.dimensions();

    Ok(AppIcon {
        window: egui::IconData {
            rgba: window.into_raw(),
            width,
            height,
        },
        tray_rgba: tray.into_raw(),
        tray_size: TRAY_ICON_SIZE,
    })
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
