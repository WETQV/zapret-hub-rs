//! Debug-only: `ZAPRET_HUB_SCREENSHOT_DIR=dir` renders every page, saves PNGs and exits.
//! Used to eyeball the UI from the Linux dev box.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

use super::{Page, ZapretHubApp};

const SETTLE_FRAMES: u32 = 45;

pub(crate) struct Tour {
    dir: PathBuf,
    step: usize,
    frames: u32,
    saved: Option<Arc<AtomicBool>>,
}

impl Tour {
    pub(crate) fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("ZAPRET_HUB_SCREENSHOT_DIR")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self {
            dir,
            step: 0,
            frames: 0,
            saved: None,
        })
    }
}

impl ZapretHubApp {
    pub(super) fn advance_screenshot_tour(&mut self, ctx: &egui::Context) {
        let Some(tour) = &mut self.screenshot_tour else {
            return;
        };
        ctx.request_repaint();
        // Linux-only noise (no schtasks etc.) would just clutter the pictures.
        self.toasts.clear();
        self.log.retain(|entry| entry.kind != super::LogKind::Error);
        if std::env::var_os("ZAPRET_HUB_FAKE_ACTIVE").is_some() {
            self.status.winws_running = true;
            self.status.telegram_proxy_running = self.config.launch_telegram_proxy_for_profiles;
        }
        let Some(&page) = Page::ALL.get(tour.step) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        self.page = page;
        self.page_changed = std::time::Instant::now() - std::time::Duration::from_secs(5);

        match &tour.saved {
            Some(saved) if saved.load(Ordering::Acquire) => {
                tour.step += 1;
                tour.frames = 0;
                tour.saved = None;
            }
            Some(_) => {}
            None => {
                tour.frames += 1;
                if tour.frames < SETTLE_FRAMES {
                    return;
                }
                let saved = Arc::new(AtomicBool::new(false));
                let flag = saved.clone();
                let path = tour.dir.join(format!("{}-{}.png", tour.step, page.id()));
                let repaint = ctx.clone();
                ctx.request_screenshot(move |image| {
                    let [width, height] = image.size;
                    let rgba: Vec<u8> = image
                        .pixels
                        .iter()
                        .flat_map(|pixel| pixel.to_array())
                        .collect();
                    if let Some(buffer) =
                        image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                    {
                        let _ = buffer.save(&path);
                    }
                    flag.store(true, Ordering::Release);
                    repaint.request_repaint();
                });
                tour.saved = Some(saved);
            }
        }
    }
}
