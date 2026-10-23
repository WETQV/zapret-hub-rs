//! System tray icon. Events arrive on the winit thread through the handlers below, are queued,
//! and `App::logic` picks them up — eframe calls `logic` even while the window is hidden as long
//! as somebody requests a repaint, which the handlers do.

use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum TrayCommand {
    Show,
    Toggle,
    Quit,
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::Sender;

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::TrayCommand;

    const ID_SHOW: &str = "show";
    const ID_TOGGLE: &str = "toggle";
    const ID_QUIT: &str = "quit";

    pub(crate) struct Tray {
        icon: TrayIcon,
        toggle_item: MenuItem,
        status_item: MenuItem,
        active_icon: Icon,
        idle_icon: Icon,
        shown_state: Option<(bool, bool, String)>,
    }

    impl Tray {
        pub(crate) fn new(
            ctx: egui::Context,
            sender: Sender<TrayCommand>,
            icon_rgba: &[u8],
            icon_size: u32,
        ) -> anyhow::Result<Self> {
            let active_icon = Icon::from_rgba(icon_rgba.to_vec(), icon_size, icon_size)?;
            let idle_icon = Icon::from_rgba(desaturate(icon_rgba), icon_size, icon_size)?;

            let status_item = MenuItem::new("Обход выключен", false, None);
            let show_item = MenuItem::with_id(ID_SHOW, "Открыть Zapret Hub", true, None);
            let toggle_item = MenuItem::with_id(ID_TOGGLE, "Включить обход", true, None);
            let quit_item = MenuItem::with_id(ID_QUIT, "Выход", true, None);
            let menu = Menu::with_items(&[
                &status_item,
                &PredefinedMenuItem::separator(),
                &show_item,
                &toggle_item,
                &PredefinedMenuItem::separator(),
                &quit_item,
            ])?;

            let icon = TrayIconBuilder::new()
                .with_icon(idle_icon.clone())
                .with_tooltip("Zapret Hub")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()?;

            let click_sender = sender.clone();
            let click_ctx = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                let open = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                );
                if open {
                    let _ = click_sender.send(TrayCommand::Show);
                    click_ctx.request_repaint();
                }
            }));

            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let command = match event.id().as_ref() {
                    ID_SHOW => TrayCommand::Show,
                    ID_TOGGLE => TrayCommand::Toggle,
                    ID_QUIT => TrayCommand::Quit,
                    _ => return,
                };
                let _ = sender.send(command);
                ctx.request_repaint();
            }));

            Ok(Self {
                icon,
                toggle_item,
                status_item,
                active_icon,
                idle_icon,
                shown_state: None,
            })
        }

        /// Cheap to call every frame: only touches the OS when something changed.
        pub(crate) fn sync(&mut self, active: bool, busy: bool, status_text: &str) {
            let state = (active, busy, status_text.to_owned());
            if self.shown_state.as_ref() == Some(&state) {
                return;
            }

            self.status_item.set_text(status_text);
            self.toggle_item.set_text(if active {
                "Выключить обход"
            } else {
                "Включить обход"
            });
            self.toggle_item.set_enabled(!busy);
            let icon = if active {
                &self.active_icon
            } else {
                &self.idle_icon
            };
            let _ = self.icon.set_icon(Some(icon.clone()));
            let _ = self
                .icon
                .set_tooltip(Some(format!("Zapret Hub — {status_text}")));
            self.shown_state = Some(state);
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        }
    }

    /// Grey, slightly faded copy of the icon for the "bypass off" state.
    fn desaturate(rgba: &[u8]) -> Vec<u8> {
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| {
                let luma = (0.299 * f32::from(pixel[0])
                    + 0.587 * f32::from(pixel[1])
                    + 0.114 * f32::from(pixel[2])) as u8;
                [luma, luma, luma, (f32::from(pixel[3]) * 0.8) as u8]
            })
            .collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::mpsc::Sender;

    use eframe::egui;

    use super::TrayCommand;

    pub(crate) struct Tray;

    impl Tray {
        pub(crate) fn new(
            _ctx: egui::Context,
            _sender: Sender<TrayCommand>,
            _icon_rgba: &[u8],
            _icon_size: u32,
        ) -> anyhow::Result<Self> {
            anyhow::bail!("системный трей поддерживается только в Windows")
        }

        pub(crate) fn sync(&mut self, _active: bool, _busy: bool, _status_text: &str) {}
    }
}

pub(crate) use imp::Tray;

pub(crate) fn create(
    ctx: eframe::egui::Context,
    sender: Sender<TrayCommand>,
    icon_rgba: &[u8],
    icon_size: u32,
) -> anyhow::Result<Tray> {
    Tray::new(ctx, sender, icon_rgba, icon_size)
}
