//! Page layouts. State changes go through methods on `ZapretHubApp`; nothing here blocks.

use std::time::Instant;

use eframe::egui::{
    self, Align, Align2, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui,
    vec2,
};

use super::theme::{
    self, Tone, callout, callout_text, card, card_header, danger_button, hint, kv_row, nav_item,
    page_title, palette, pill, power_button, primary_button, secondary_button, segmented,
    status_dot, toggle_row,
};
use super::{LogKind, Page, TgTask, ZapretHubApp};
use crate::core::build_info::{APP_AUTHOR, APP_NAME, APP_VERSION, BUILD_DATE, BUILT_BY};
use crate::core::config::TelegramProxyMode;
use crate::core::profile_test::{ProfileTestMode, ProfileTestReport};
use crate::core::status::ServiceState;
use crate::platform::open_in_file_manager;
use crate::zapret::bundle::BundleAction;
use crate::zapret::fakes::FakeTarget;

const CONTENT_MAX_WIDTH: f32 = 920.0;

impl ZapretHubApp {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let palette = palette(ui);

        self.title_bar(ui);

        egui::Panel::left("sidebar")
            .exact_size(214.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(palette.sidebar)
                    .inner_margin(Margin::symmetric(12, 18)),
            )
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(palette.bg))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin {
                                left: 28,
                                right: 28,
                                top: 24,
                                bottom: 28,
                            })
                            .show(ui, |ui| {
                                // Fade + slide in on every page change.
                                let t = self.page_transition(ui.ctx());
                                ui.set_opacity(t);
                                ui.add_space((1.0 - t) * 16.0);
                                ui.set_max_width(ui.available_width().min(CONTENT_MAX_WIDTH));
                                ui.spacing_mut().item_spacing.y = 12.0;
                                match self.page {
                                    Page::Home => self.page_home(ui),
                                    Page::Profiles => self.page_profiles(ui),
                                    Page::Telegram => self.page_telegram(ui),
                                    Page::Updates => self.page_updates(ui),
                                    Page::Settings => self.page_settings(ui),
                                }
                            });
                    });
            });

        self.draw_toasts(ui.ctx());
        self.window_frame_and_resize(ui.ctx());
    }

    /// 0 → 1 over a fraction of a second after the page changed (ease-out).
    fn page_transition(&self, ctx: &egui::Context) -> f32 {
        const DURATION: f32 = 0.22;
        let progress = (self.page_changed.elapsed().as_secs_f32() / DURATION).min(1.0);
        if progress < 1.0 {
            ctx.request_repaint();
        }
        1.0 - (1.0 - progress).powi(3)
    }

    // ----- sidebar ----------------------------------------------------------------------------

    fn sidebar(&mut self, ui: &mut Ui) {
        let palette = palette(ui);

        let updates_badge = self.bundle_update.update_available() && self.bundle_valid
            || !self.bundle_valid
            || (self.tg_update.update_available()
                && self.config.launch_telegram_proxy_for_profiles);
        let mut clicked = None;
        // Highlight that glides to the selected entry; entries paint on top of it.
        let top = ui.cursor().top();
        let index = Page::ALL
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        let target =
            top + index as f32 * (theme::NAV_ITEM_HEIGHT + ui.spacing().item_spacing.y + 2.0);
        let pill_y = ui
            .ctx()
            .animate_value_with_time(egui::Id::unique("nav_pill"), target, 0.2);
        let pill_rect = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().left(), pill_y),
            vec2(ui.available_width(), theme::NAV_ITEM_HEIGHT),
        );
        ui.painter()
            .rect_filled(pill_rect, CornerRadius::same(8), palette.accent_soft);
        for page in Page::ALL {
            let badge = page == Page::Updates && updates_badge;
            if nav_item(ui, page.icon(), page.label(), self.page == page, badge).clicked() {
                clicked = Some(page);
            }
            ui.add_space(2.0);
        }
        if let Some(page) = clicked {
            self.set_page(page);
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                hint(
                    ui,
                    format!(
                        "Hub v{APP_VERSION} · bundle {}",
                        self.bundle_version.as_deref().unwrap_or("—")
                    ),
                );
            });
        });
    }

    pub(super) fn state_summary(&self, ui: &Ui) -> (egui::Color32, &'static str) {
        let palette = palette(ui);
        if let Some(action) = self.pending_action() {
            let text = match action {
                BundleAction::StartProfile { .. } => "Запускаю…",
                BundleAction::StopAll => "Останавливаю…",
                _ => "Выполняю команду…",
            };
            (palette.warning, text)
        } else if self.bundle_update.applying.is_some() {
            (palette.warning, "Обновляю bundle…")
        } else if self.test.running() {
            (palette.warning, "Подбираю профиль…")
        } else if self.service_state() == ServiceState::StopPending {
            (palette.warning, "Служба останавливается…")
        } else if self.runtime_active() {
            (palette.success, "Обход включён")
        } else {
            (palette.muted, "Обход выключен")
        }
    }

    // ----- home -------------------------------------------------------------------------------

    fn page_home(&mut self, ui: &mut Ui) {
        self.home_notices(ui);
        self.home_hero(ui);

        ui.columns(2, |columns| {
            self.home_profile_card(&mut columns[0]);
            self.home_launch_options_card(&mut columns[1]);
        });

        self.activity_card(ui);
    }

    fn home_notices(&mut self, ui: &mut Ui) {
        if !self.bundle_valid {
            callout(ui, Tone::Warning, |ui| {
                ui.label(RichText::new("Bundle zapret не найден").strong());
                ui.label(format!(
                    "Ожидался в {}. Его можно скачать прямо отсюда.",
                    self.bundle_path.display()
                ));
                if primary_button(ui, "Перейти к установке", true).clicked() {
                    self.set_page(Page::Updates);
                }
            });
        }

        if self.app_updated_notice {
            callout(ui, Tone::Info, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Zapret Hub обновился до v{APP_VERSION}."));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Скрыть").clicked() {
                            self.app_updated_notice = false;
                        }
                    });
                });
            });
        }

        if self.bundle_valid && self.bundle_notice_visible() && !self.bundle_update.installing() {
            let tag = self
                .bundle_update
                .status
                .as_ref()
                .map(|status| status.latest.tag.clone())
                .unwrap_or_default();
            callout(ui, Tone::Info, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Доступен bundle {tag}")).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Позже").clicked() {
                            self.dismiss_bundle_notice();
                        }
                        if primary_button(ui, "Обновить", true).clicked() {
                            self.start_bundle_install();
                        }
                    });
                });
            });
        }
        if self.bundle_update.installing() {
            callout(ui, Tone::Info, |ui| {
                ui.label(RichText::new("Обновление bundle").strong());
                theme::progress(
                    ui,
                    self.bundle_update.progress.fraction(),
                    self.bundle_update.stage_text(),
                );
            });
        }

        if self.tg_notice_visible() && self.tg_update.task.is_none() {
            let tag = self
                .tg_update
                .status
                .as_ref()
                .map(|status| status.latest.tag.clone())
                .unwrap_or_default();
            callout(ui, Tone::Info, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Доступен Telegram WS proxy {tag}"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Позже").clicked() {
                            self.dismiss_tg_notice();
                        }
                        if secondary_button(ui, "Обновить", true).clicked() {
                            self.start_tg_install();
                        }
                    });
                });
            });
        }

        if self.service_state() == ServiceState::Running {
            callout(ui, Tone::Warning, |ui| {
                ui.label(RichText::new("Работает служба zapret").strong());
                ui.label(
                    "Обход запущен как служба Windows через service.bat и стартует вместе с системой. \
                     Кнопка ниже остановит её до перезагрузки; удалить службу можно в Настройках.",
                );
            });
        }
    }

    fn home_hero(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        let active = self.runtime_active();
        let busy = self.busy();
        let can_toggle = self.can_toggle();
        let mut toggle_clicked = false;
        let mut restart_clicked = false;

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                let response = power_button(ui, active, self.action.is_some(), can_toggle);
                toggle_clicked = response.clicked();
                ui.add_space(22.0);

                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    let (title, color) = if self.action.is_some() {
                        let (_, text) = self.state_summary(ui);
                        (text, palette.warning)
                    } else if active {
                        ("Обход включён", palette.success)
                    } else {
                        ("Обход выключен", palette.text)
                    };
                    ui.label(RichText::new(title).size(26.0).strong().color(color));

                    let subtitle = if active {
                        let profile = self
                            .running_launch
                            .as_ref()
                            .and_then(|launch| {
                                self.profiles.iter().find(|profile| {
                                    profile.script_name().eq_ignore_ascii_case(&launch.script)
                                })
                            })
                            .map_or_else(|| self.selected_profile_label(), |profile| {
                                profile.label().to_owned()
                            });
                        format!("Профиль {profile}. Discord, YouTube и другие сервисы из списков идут через обход.")
                    } else if !self.bundle_valid {
                        "Сначала установите bundle на странице «Обновления».".to_owned()
                    } else {
                        format!(
                            "Нажмите кнопку, чтобы запустить профиль {}.",
                            self.selected_profile_label()
                        )
                    };
                    ui.add(egui::Label::new(RichText::new(subtitle).color(palette.muted)).wrap());
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        let on = palette.success;
                        let off = palette.muted;
                        pill(
                            ui,
                            if self.status.winws_running { "winws работает" } else { "winws остановлен" },
                            if self.status.winws_running { on } else { off },
                        );
                        if self.config.launch_telegram_proxy_for_profiles || self.status.telegram_proxy_running {
                            pill(
                                ui,
                                if self.status.telegram_proxy_running { "Telegram proxy работает" } else { "Telegram proxy выключен" },
                                if self.status.telegram_proxy_running { on } else { off },
                            );
                        }
                        let (service_text, service_color) = match self.service_state() {
                            ServiceState::Running => ("служба zapret запущена", palette.warning),
                            ServiceState::Stopped => ("служба установлена", off),
                            ServiceState::StopPending => ("служба останавливается", palette.warning),
                            ServiceState::NotInstalled | ServiceState::Unknown => ("", off),
                        };
                        if !service_text.is_empty() {
                            pill(ui, service_text, service_color);
                        }
                    });

                    if self.restart_suggested() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Настройки изменились — нужен перезапуск.")
                                    .color(palette.warning),
                            );
                            if ui.add_enabled(!busy, egui::Button::new("Перезапустить")).clicked() {
                                restart_clicked = true;
                            }
                        });
                    }
                });
            });
        });

        if toggle_clicked {
            self.toggle_runtime();
        }
        if restart_clicked {
            self.restart_runtime();
        }
    }

    fn home_profile_card(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        let mut open_profiles = false;
        card(ui, |ui| {
            card_header(ui, "Профиль", "Стратегия обхода из bundle.");
            let enabled = self.bundle_valid && self.action.is_none() && !self.test.running();
            let current = self.config.main_profile_script_or_legacy().to_owned();
            let mut selected = current.clone();
            ui.add_enabled_ui(enabled, |ui| {
                egui::ComboBox::from_id_salt("home_profile")
                    .width(ui.available_width())
                    .height(360.0)
                    .selected_text(self.selected_profile_label())
                    .show_ui(ui, |ui| {
                        for profile in &self.profiles {
                            ui.selectable_value(
                                &mut selected,
                                profile.script_name().to_owned(),
                                profile.label(),
                            );
                        }
                    });
            });
            if selected != current {
                chosen = Some(selected);
            }
            ui.add_space(2.0);
            hint(
                ui,
                "Не работает Discord или YouTube? Подберите профиль автоматически.",
            );
            if ui
                .add_enabled(
                    self.bundle_valid,
                    egui::Button::new("Подобрать лучший профиль →"),
                )
                .clicked()
            {
                open_profiles = true;
            }
        });
        if let Some(script) = chosen {
            self.set_main_profile(&script);
        }
        if open_profiles {
            self.set_page(Page::Profiles);
        }
    }

    fn home_launch_options_card(&mut self, ui: &mut Ui) {
        let enabled = self.action.is_none();
        card(ui, |ui| {
            card_header(ui, "При запуске", "Применяется при следующем включении.");
            let mut whitelist = self.config.use_builtin_whitelist;
            if toggle_row(
                ui,
                "Встроенный список исключений",
                "Банки, госуслуги и другие сайты, которым обход только мешает.",
                &mut whitelist,
                enabled,
            ) {
                self.config.use_builtin_whitelist = whitelist;
                self.save_config();
            }
            ui.add_space(4.0);
            let mut telegram = self.config.launch_telegram_proxy_for_profiles;
            if toggle_row(
                ui,
                "Telegram WS proxy",
                "Локальный MTProto-прокси для Telegram Desktop.",
                &mut telegram,
                enabled,
            ) {
                self.config.launch_telegram_proxy_for_profiles = telegram;
                self.save_config();
            }
        });
    }

    fn activity_card(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        card(ui, |ui| {
            card_header(ui, "События", "");
            if self.log.is_empty() {
                hint(ui, "Пока ничего не произошло.");
                return;
            }
            let now = Instant::now();
            for entry in self.log.iter().take(6) {
                ui.horizontal_top(|ui| {
                    let color = match entry.kind {
                        LogKind::Info => palette.muted,
                        LogKind::Success => palette.success,
                        LogKind::Error => palette.danger,
                    };
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.add_space(5.0);
                        status_dot(ui, color, 7.0);
                    });
                    ui.allocate_ui(vec2(82.0, 18.0), |ui| {
                        ui.set_min_width(82.0);
                        hint(ui, time_ago(now.duration_since(entry.at).as_secs()));
                    });
                    ui.add(egui::Label::new(RichText::new(&entry.text)).wrap());
                });
            }
            if self.log.len() > 6 {
                egui::CollapsingHeader::new(format!("Ещё {}", self.log.len() - 6))
                    .id_salt("older_log")
                    .show(ui, |ui| {
                        for entry in self.log.iter().skip(6) {
                            hint(ui, &entry.text);
                        }
                    });
            }
        });
    }

    // ----- profiles ---------------------------------------------------------------------------

    fn page_profiles(&mut self, ui: &mut Ui) {
        page_title(
            ui,
            "Профили",
            "Профиль — это набор приёмов обхода. Разным провайдерам подходят разные.",
        );
        if !self.bundle_valid {
            callout_text(
                ui,
                Tone::Warning,
                "Bundle не установлен — профилей пока нет.",
            );
            return;
        }
        self.profiles_list_card(ui);
        self.profile_test_card(ui);
        self.fakes_card(ui);
    }

    fn profiles_list_card(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        let mut make_main = None;
        let mut start = None;
        let can_start = !self.busy() && !self.runtime_active();

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Основной профиль").size(16.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.profile_filter)
                            .hint_text("Поиск")
                            .desired_width(180.0),
                    );
                });
            });
            hint(
                ui,
                "Нажмите на плашку, чтобы сделать профиль основным; ▶ на плашке — запустить сразу.",
            );
            ui.add_space(4.0);

            let filter = self.profile_filter.trim().to_lowercase();
            let main = self.config.main_profile_script_or_legacy();
            let visible: Vec<_> = self
                .profiles
                .iter()
                .filter(|profile| {
                    filter.is_empty() || profile.label().to_lowercase().contains(&filter)
                })
                .collect();
            if visible.is_empty() {
                hint(ui, "Ничего не найдено.");
                return;
            }

            let columns = if ui.available_width() > 640.0 { 3 } else { 2 };
            let width = (ui.available_width() - 8.0 * (columns as f32 - 1.0)) / columns as f32;
            egui::Grid::new("profiles_grid")
                .num_columns(columns)
                .spacing(vec2(8.0, 6.0))
                .show(ui, |ui| {
                    for (index, profile) in visible.iter().enumerate() {
                        let is_main = profile.script_name().eq_ignore_ascii_case(main);
                        // The whole tile is the click target; the fill is painted afterwards
                        // (into this reserved slot) once we know whether it is hovered.
                        let background = ui.painter().add(egui::Shape::Noop);
                        let mut play_rect = egui::Rect::NOTHING;
                        let tile = Frame::new()
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.set_width(width - 22.0);
                                ui.horizontal(|ui| {
                                    let label = RichText::new(profile.label());
                                    let label = if is_main {
                                        label.strong().color(palette.accent)
                                    } else {
                                        label
                                    };
                                    ui.add(egui::Label::new(label).truncate().selectable(false));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let (rect, _) = ui.allocate_exact_size(
                                            vec2(24.0, 24.0),
                                            egui::Sense::hover(),
                                        );
                                        play_rect = rect;
                                    });
                                });
                            });
                        let rect = tile.response.rect;
                        let response = ui
                            .interact(
                                rect,
                                egui::Id::unique(("profile_tile", &profile.script_name())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        let hover = ui.ctx().animate_bool_with_time(
                            response.id.with("hover"),
                            response.hovered(),
                            0.12,
                        );
                        let over_play = can_start
                            && response
                                .hover_pos()
                                .is_some_and(|pos| play_rect.contains(pos));
                        let base = if is_main {
                            palette.accent_soft
                        } else {
                            palette.card_hover
                        };
                        let fill = if is_main {
                            base
                        } else {
                            lerp_fill(base, palette.accent_soft, hover * 0.8)
                        };
                        ui.painter().set(
                            background,
                            egui::Shape::rect_filled(rect, CornerRadius::same(8), fill),
                        );
                        ui.painter().rect_stroke(
                            rect,
                            CornerRadius::same(8),
                            Stroke::new(
                                1.0,
                                if is_main || hover > 0.0 {
                                    palette
                                        .accent
                                        .gamma_multiply(if is_main { 1.0 } else { hover })
                                } else {
                                    palette.border
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        let glyph_color = if !can_start {
                            palette.muted.gamma_multiply(0.4)
                        } else if over_play {
                            palette.success
                        } else if hover > 0.0 {
                            palette.text
                        } else {
                            palette.muted
                        };
                        ui.painter().text(
                            play_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "▶",
                            egui::FontId::proportional(13.0),
                            glyph_color,
                        );
                        if over_play {
                            response
                                .clone()
                                .on_hover_text("Сделать основным и запустить");
                        }
                        if response.clicked() {
                            let on_play = can_start
                                && response
                                    .interact_pointer_pos()
                                    .is_some_and(|pos| play_rect.contains(pos));
                            if on_play {
                                start = Some((*profile).clone());
                            } else {
                                make_main = Some(profile.script_name().to_owned());
                            }
                        }
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        });

        if let Some(script) = make_main {
            self.set_main_profile(&script);
        }
        if let Some(profile) = start {
            self.set_main_profile(profile.script_name());
            self.start_profile(profile);
        }
    }

    fn profile_test_card(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        let mut start = false;
        let mut cancel = false;
        let mut make_main = None;

        card(ui, |ui| {
            card_header(
                ui,
                "Подбор лучшего профиля",
                "По очереди запускает профили и проверяет доступность Discord, YouTube, Google и Cloudflare. Окна не открываются.",
            );

            if self.test.running() {
                let total = self.test.total.max(1);
                let fraction = (self.test.current.saturating_sub(1)) as f32 / total as f32;
                let label = match &self.test.current_label {
                    _ if self.test.cancelling() => "Отменяю и возвращаю всё как было…".to_owned(),
                    Some(label) => match &self.test.current_check {
                        Some(check) => format!(
                            "Профиль {} из {total}: {label} — {check}",
                            self.test.current
                        ),
                        None => format!("Профиль {} из {total}: {label}", self.test.current),
                    },
                    None => "Проверяю curl и права администратора…".to_owned(),
                };
                theme::progress(ui, Some(fraction), label);
                if let Some((leader, ok)) = &self.test.leader {
                    hint(
                        ui,
                        format!("Пока лучший: {leader} ({ok} успешных проверок)"),
                    );
                }
                if secondary_button(ui, "Отменить", !self.test.cancelling()).clicked() {
                    cancel = true;
                }
                return;
            }

            let blocker = self.test_blocker();
            ui.horizontal(|ui| {
                ui.label("Проверка:");
                segmented(
                    ui,
                    &mut self.test.mode,
                    &[
                        (ProfileTestMode::Standard, "Обычная"),
                        (ProfileTestMode::Dpi, "Глубокая (DPI 16–20 КБ)"),
                    ],
                    true,
                );
            });
            if self.test.mode == ProfileTestMode::Dpi {
                hint(
                    ui,
                    "Ловит блокировки «обрывом после 16–20 КБ». Дольше обычной; ipset временно очищается и восстанавливается после.",
                );
            }

            let mut custom = self.test.custom_selection;
            if toggle_row(ui, "Выбрать профили вручную", "", &mut custom, true)
            {
                self.test.custom_selection = custom;
                if custom && self.test.selected_scripts.is_empty() {
                    self.test.selected_scripts = self
                        .profiles
                        .iter()
                        .map(|profile| profile.script_name().to_owned())
                        .collect();
                }
            }
            if self.test.custom_selection {
                ui.horizontal(|ui| {
                    if ui.small_button("Все").clicked() {
                        self.test.selected_scripts = self
                            .profiles
                            .iter()
                            .map(|profile| profile.script_name().to_owned())
                            .collect();
                    }
                    if ui.small_button("Ни одного").clicked() {
                        self.test.selected_scripts.clear();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for profile in &self.profiles {
                        let position =
                            self.test.selected_scripts.iter().position(|script| {
                                script.eq_ignore_ascii_case(profile.script_name())
                            });
                        let mut checked = position.is_some();
                        if ui.checkbox(&mut checked, profile.label()).changed() {
                            match position {
                                Some(position) => {
                                    self.test.selected_scripts.remove(position);
                                }
                                None => self
                                    .test
                                    .selected_scripts
                                    .push(profile.script_name().to_owned()),
                            }
                        }
                    }
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let count = if self.test.custom_selection {
                    self.test.selected_scripts.len()
                } else {
                    self.profiles.len()
                };
                let label = format!("Начать подбор ({count} {})", plural_profiles(count));
                if primary_button(ui, &label, blocker.is_none() && count > 0).clicked() {
                    start = true;
                }
                if let Some(blocker) = blocker {
                    hint(ui, blocker);
                } else {
                    hint(
                        ui,
                        format!("Займёт примерно {} мин.", (count * 25).div_ceil(60).max(1)),
                    );
                }
            });

            if let Some(error) = &self.test.error {
                callout_text(ui, Tone::Danger, error.clone());
            }

            if let Some(report) = &self.test.report {
                ui.add_space(6.0);
                make_main = test_results(
                    ui,
                    report,
                    &self.profiles,
                    self.config.main_profile_script_or_legacy(),
                    palette,
                );
            }
        });

        if start {
            self.start_profile_test();
        }
        if cancel {
            self.cancel_profile_test();
        }
        if let Some(script) = make_main {
            self.set_main_profile(&script);
        }
    }

    fn fakes_card(&mut self, ui: &mut Ui) {
        let enabled = !self.busy() && !self.runtime_active();
        let mut apply = None;
        card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("UDP fake-файлы").size(16.0).strong())
                .id_salt("fakes")
                .show(ui, |ui| {
                    hint(
                        ui,
                        "Для опытных: какой fake-пакет отправлять для голосовых каналов Discord и игр (GameFilter).",
                    );
                    if let Some(error) = &self.fakes.error {
                        callout_text(ui, Tone::Warning, error.clone());
                        return;
                    }
                    let Some(catalog) = &self.fakes.catalog else {
                        ui.spinner();
                        return;
                    };
                    if catalog.entries.is_empty() {
                        hint(ui, "В bin нет .bin файлов.");
                        return;
                    }

                    egui::Grid::new("fakes_grid")
                        .num_columns(3)
                        .spacing(vec2(12.0, 8.0))
                        .show(ui, |ui| {
                            for target in FakeTarget::ALL {
                                let (current, selection) = match target {
                                    FakeTarget::DiscordUdp => {
                                        (&catalog.discord_current, &mut self.fakes.discord_selection)
                                    }
                                    FakeTarget::GameFilterUdp => {
                                        (&catalog.game_current, &mut self.fakes.game_selection)
                                    }
                                };
                                ui.label(target.label());
                                egui::ComboBox::from_id_salt(target.label())
                                    .width(280.0)
                                    .selected_text(
                                        selection.as_deref().unwrap_or("свой / из старой версии"),
                                    )
                                    .show_ui(ui, |ui| {
                                        for entry in &catalog.entries {
                                            ui.selectable_value(
                                                selection,
                                                Some(entry.file_name.clone()),
                                                &entry.file_name,
                                            );
                                        }
                                    });
                                let changed = selection.is_some() && selection != current;
                                if ui
                                    .add_enabled(enabled && changed, egui::Button::new("Применить"))
                                    .clicked()
                                {
                                    apply = selection.clone().map(|file| (target, file));
                                }
                                ui.end_row();
                            }
                        });
                    if !enabled {
                        hint(ui, "Применить можно, когда обход выключен.");
                    }
                });
        });
        if let Some((target, file)) = apply {
            self.apply_fake(target, &file);
        }
    }

    // ----- telegram ---------------------------------------------------------------------------

    fn page_telegram(&mut self, ui: &mut Ui) {
        page_title(
            ui,
            "Telegram",
            "Локальный MTProto-прокси (Flowseal/tg-ws-proxy): ускоряет Telegram через WebSocket.",
        );
        let enabled = self.action.is_none();

        card(ui, |ui| {
            let mut launch = self.config.launch_telegram_proxy_for_profiles;
            if toggle_row(
                ui,
                "Запускать вместе с профилем",
                "Telegram Desktop 6.7.2 и новее обычно работает и без прокси — включайте, если не грузятся сообщения или медиа.",
                &mut launch,
                enabled,
            ) {
                self.config.launch_telegram_proxy_for_profiles = launch;
                self.save_config();
            }
        });

        card(ui, |ui| {
            card_header(ui, "Режим", "");
            let mut mode = self.config.telegram_proxy_mode;
            let changed = segmented(
                ui,
                &mut mode,
                &[
                    (TelegramProxyMode::Standard, "Обычный"),
                    (TelegramProxyMode::CfMedia, "CF media"),
                ],
                enabled,
            );
            if changed {
                self.config.telegram_proxy_mode = mode;
                self.save_config();
            }

            match self.config.telegram_proxy_mode {
                TelegramProxyMode::Standard => hint(
                    ui,
                    "Проксирует дата-центры DC2 и DC4. Первый вариант, если Telegram не подключается.",
                ),
                TelegramProxyMode::CfMedia => {
                    hint(
                        ui,
                        "Для случая «текст идёт, фото и видео — нет»: трафик DC4 идёт через ваш домен в Cloudflare.",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Домен в Cloudflare");
                        let mut domain = self.config.telegram_cf_domain.clone();
                        let response = ui.add_enabled(
                            enabled,
                            egui::TextEdit::singleline(&mut domain)
                                .hint_text("your-domain.example")
                                .desired_width(260.0),
                        );
                        if response.changed() {
                            self.config.telegram_cf_domain = domain.trim().to_owned();
                        }
                        if response.lost_focus() {
                            self.save_config();
                        }
                    });
                    if self.config.telegram_cf_domain.trim().is_empty() {
                        callout_text(
                            ui,
                            Tone::Warning,
                            "Укажите домен, полностью управляемый через Cloudflare (обычный dynamic DNS не подойдёт).",
                        );
                    }
                }
            }
        });

        self.tg_connect_card(ui);
        self.tg_update_card(ui);
    }

    fn tg_connect_card(&mut self, ui: &mut Ui) {
        let mut open = false;
        let mut copy = false;
        card(ui, |ui| {
            card_header(
                ui,
                "Подключение к Telegram",
                "Это локальный MTProto-прокси: 127.0.0.1, порт 1443. Telegram Desktop нужно подключить к нему один раз.",
            );
            let ready = self.tg_link.is_some();
            ui.horizontal(|ui| {
                open = primary_button(ui, "Открыть в Telegram", ready).clicked();
                copy = secondary_button(ui, "Скопировать ссылку", ready).clicked();
            });
            if ready {
                hint(
                    ui,
                    "Telegram предложит добавить прокси — подтвердите. Вручную: Настройки → Продвинутые → Тип соединения → Прокси → MTProto, сервер 127.0.0.1, порт 1443, secret из ссылки.",
                );
            } else {
                hint(
                    ui,
                    "Ссылка появится после первого запуска прокси: включите «Запускать вместе с профилем» и включите обход.",
                );
            }
        });
        if let Some(link) = self.tg_link.clone() {
            if open {
                crate::platform::open_url(&link);
            }
            if copy {
                ui.ctx().copy_text(link);
                self.notify(LogKind::Success, "Ссылка скопирована.");
            }
        }
    }

    fn tg_update_card(&mut self, ui: &mut Ui) {
        let mut install = false;
        let mut check = false;
        card(ui, |ui| {
            card_header(ui, "TgWsProxy", "Обновляется отдельно от bundle.");
            let installed = self
                .tg_update
                .status
                .as_ref()
                .and_then(|status| status.installed_tag.clone())
                .unwrap_or_else(|| "неизвестно".to_owned());
            kv_row(ui, "Установлен", installed);
            if let Some(status) = &self.tg_update.status {
                kv_row(ui, "Последний релиз", status.latest.tag.as_str());
            }

            match &self.tg_update.task {
                Some(TgTask::Install(_)) => {
                    theme::progress(
                        ui,
                        self.tg_update.progress.fraction(),
                        "Скачиваю TgWsProxy…",
                    );
                }
                Some(TgTask::Check(_)) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        hint(ui, "Проверяю…");
                    });
                }
                None => {
                    ui.horizontal(|ui| {
                        if self.tg_update.update_available() {
                            install = primary_button(ui, "Обновить", self.bundle_valid).clicked();
                        } else if self.tg_update.status.is_some() {
                            ui.label(RichText::new("Актуальная версия").color(palette(ui).success));
                        }
                        check = secondary_button(ui, "Проверить", self.bundle_valid).clicked();
                    });
                }
            }
            if let Some(error) = &self.tg_update.error {
                callout_text(ui, Tone::Danger, error.clone());
            }
        });
        if install {
            self.start_tg_install();
        }
        if check {
            self.start_tg_check();
        }
    }

    // ----- updates ----------------------------------------------------------------------------

    fn page_updates(&mut self, ui: &mut Ui) {
        page_title(
            ui,
            "Обновления",
            "Bundle — это сам zapret от Flowseal: winws.exe, профили и списки доменов.",
        );
        self.bundle_update_card(ui);
        self.tg_update_card(ui);
        self.lists_card(ui);
    }

    fn bundle_update_card(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        let mut install = false;
        let mut check = false;
        card(ui, |ui| {
            card_header(ui, "Bundle zapret-discord-youtube", "");
            kv_row(
                ui,
                "Установлен",
                if self.bundle_valid {
                    self.bundle_version
                        .clone()
                        .unwrap_or_else(|| "неизвестная версия".to_owned())
                } else {
                    "не установлен".to_owned()
                },
            );
            if let Some(status) = &self.bundle_update.status {
                kv_row(ui, "Последний релиз", status.latest.tag.as_str());
            }

            if self.bundle_update.installing() {
                theme::progress(
                    ui,
                    self.bundle_update.progress.fraction(),
                    self.bundle_update.stage_text(),
                );
            } else if self.bundle_update.checking() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    hint(ui, "Проверяю GitHub…");
                });
            } else {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let status = self.bundle_update.status.as_ref();
                    match status {
                        Some(status) if !self.bundle_valid => {
                            install = primary_button(
                                ui,
                                &format!("Установить {}", status.latest.tag),
                                true,
                            )
                            .clicked();
                        }
                        Some(status) if status.update_available => {
                            install = primary_button(
                                ui,
                                &format!("Обновить до {}", status.latest.tag),
                                true,
                            )
                            .clicked();
                        }
                        Some(_) => {
                            ui.label(RichText::new("Актуальная версия").color(palette.success));
                        }
                        None => {}
                    }
                    check = secondary_button(ui, "Проверить", true).clicked();
                    if let Some(status) = status {
                        theme::link(ui, "Что нового", &status.latest.release_url);
                    }
                });
                hint(
                    ui,
                    "Ваши списки доменов, исключения и выбранные fake-файлы сохраняются. Если обход включён, он перезапустится сам.",
                );
            }

            if let Some(error) = &self.bundle_update.error {
                callout_text(ui, Tone::Danger, error.clone());
            }
        });
        if install {
            self.start_bundle_install();
        }
        if check {
            self.start_bundle_check(true);
        }
    }

    fn lists_card(&mut self, ui: &mut Ui) {
        let enabled = !self.busy() && !self.runtime_active() && self.bundle_valid;
        let mut refresh = false;
        card(ui, |ui| {
            card_header(
                ui,
                "Списки IP-адресов",
                "Если обход перестал работать после экспериментов с ipset — верните исходный список.",
            );
            ui.horizontal(|ui| {
                refresh = secondary_button(ui, "Восстановить ipset", enabled).clicked();
                if !enabled && self.bundle_valid {
                    hint(ui, "Сначала выключите обход.");
                }
            });
        });
        if refresh {
            self.start_action(BundleAction::RefreshIpset);
        }
    }

    // ----- settings ---------------------------------------------------------------------------

    fn page_settings(&mut self, ui: &mut Ui) {
        page_title(ui, "Настройки", "");
        let has_tray = self.tray.is_some();

        card(ui, |ui| {
            card_header(ui, "Окно и трей", "");
            let mut changed = false;
            changed |= toggle_row(
                ui,
                "Сворачивать в трей при закрытии",
                "Крестик прячет окно, обход продолжает работать. Выход — через меню иконки в трее.",
                &mut self.config.close_to_tray,
                has_tray,
            );
            changed |= toggle_row(
                ui,
                "Сворачивать в трей вместо панели задач",
                "",
                &mut self.config.minimize_to_tray,
                has_tray,
            );
            changed |= toggle_row(
                ui,
                "Выключать обход при выходе",
                "Если выключить, winws.exe продолжит работать после закрытия Zapret Hub.",
                &mut self.config.stop_on_exit,
                true,
            );
            if changed {
                self.save_config();
            }
            if !has_tray {
                hint(ui, "Иконка в трее недоступна в этой системе.");
            }
        });

        let mut autostart_request = None;
        card(ui, |ui| {
            card_header(ui, "Автозапуск", "");
            let known = self.autostart.enabled;
            let mut autostart = known.unwrap_or(self.config.autostart_enabled);
            if toggle_row(
                ui,
                "Запускать вместе с Windows",
                "Через Планировщик заданий, с правами администратора и без запроса UAC. Окно стартует свёрнутым в трей.",
                &mut autostart,
                known.is_some() && !self.autostart.busy(),
            ) {
                autostart_request = Some(autostart);
            }
            let mut launch = self.config.autostart_launch_profile;
            if toggle_row(
                ui,
                "Сразу включать обход",
                "При автозапуске запускать основной профиль.",
                &mut launch,
                autostart,
            ) {
                self.config.autostart_launch_profile = launch;
                self.save_config();
            }
        });
        if let Some(enabled) = autostart_request {
            self.set_autostart(enabled);
        }

        card(ui, |ui| {
            card_header(ui, "Уведомления", "");
            let mut notifications = self.config.startup_notifications_enabled;
            if toggle_row(
                ui,
                "Сообщать о новых версиях",
                "Плашка на главной, когда вышел новый bundle или Telegram proxy.",
                &mut notifications,
                true,
            ) {
                self.config.startup_notifications_enabled = notifications;
                self.save_config();
            }
        });

        self.service_card(ui);
        self.about_card(ui);
    }

    fn service_card(&mut self, ui: &mut Ui) {
        let enabled = !self.busy() && self.bundle_valid;
        let mut action = None;
        card(ui, |ui| {
            card_header(
                ui,
                "Служба Windows",
                "Обход можно установить службой через оригинальный service.bat — тогда он работает без Zapret Hub и стартует с системой.",
            );
            kv_row(
                ui,
                "Служба zapret",
                match self.service_state() {
                    ServiceState::Running => "запущена",
                    ServiceState::Stopped => "установлена, остановлена",
                    ServiceState::StopPending => "останавливается",
                    ServiceState::NotInstalled => "не установлена",
                    ServiceState::Unknown => "неизвестно",
                },
            );
            ui.horizontal(|ui| {
                if secondary_button(ui, "Открыть service.bat", enabled && !self.runtime_active())
                    .on_hover_text("Меню Flowseal: установка службы, Game Filter, диагностика")
                    .clicked()
                {
                    action = Some(BundleAction::OpenServiceManager);
                }
                let installed = matches!(
                    self.service_state(),
                    ServiceState::Running | ServiceState::Stopped | ServiceState::StopPending
                );
                if danger_button(ui, "Удалить службу", enabled && installed).clicked()
                {
                    action = Some(BundleAction::RemoveService);
                }
            });
            if self.runtime_active() && self.service_state() == ServiceState::NotInstalled {
                hint(ui, "Чтобы открыть service.bat, сначала выключите обход.");
            }
        });
        if let Some(action) = action {
            self.start_action(action);
        }
    }

    fn about_card(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            card_header(ui, "О программе", "");
            kv_row(ui, "Версия", format!("{APP_NAME} v{APP_VERSION}"));
            kv_row(ui, "Сборка", BUILD_DATE);
            let author = if APP_AUTHOR == BUILT_BY {
                APP_AUTHOR.to_owned()
            } else {
                format!("{APP_AUTHOR} / {BUILT_BY}")
            };
            kv_row(ui, "Автор", author);
            kv_row(ui, "Bundle", self.bundle_path.display().to_string());
            kv_row(ui, "Источник bundle", self.bundle_source);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.bundle_path.exists(),
                        egui::Button::new("Открыть папку bundle"),
                    )
                    .clicked()
                {
                    open_in_file_manager(&self.bundle_path);
                }
                theme::link(ui, "Zapret Hub", "https://github.com/WETQV/zapret-hub-rs");
                theme::link(ui, "bol-van/zapret", "https://github.com/bol-van/zapret");
                theme::link(
                    ui,
                    "Flowseal/zapret-discord-youtube",
                    "https://github.com/Flowseal/zapret-discord-youtube",
                );
                theme::link(
                    ui,
                    "Flowseal/tg-ws-proxy",
                    "https://github.com/Flowseal/tg-ws-proxy",
                );
            });
            ui.add_space(6.0);
            if danger_button(ui, "Выйти из Zapret Hub", true).clicked() {
                self.request_quit();
            }
        });
    }

    // ----- toasts -----------------------------------------------------------------------------

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let mut dismissed = None;
        egui::Area::new(egui::Id::unique("zapret-hub-toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let palette = palette(ui);
                ui.set_max_width(380.0);
                for (index, toast) in self.toasts.iter().enumerate().rev() {
                    let age = toast.born.elapsed().as_secs_f32();
                    if age < 0.25 {
                        ui.ctx().request_repaint();
                    }
                    ui.set_opacity((age / 0.2).min(1.0));
                    let color = match toast.kind {
                        LogKind::Info => palette.accent,
                        LogKind::Success => palette.success,
                        LogKind::Error => palette.danger,
                    };
                    let response = Frame::new()
                        .fill(palette.card)
                        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(14, 10))
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(60),
                        })
                        .show(ui, |ui| {
                            ui.set_width(350.0);
                            ui.horizontal(|ui| {
                                status_dot(ui, color, 8.0);
                                ui.add(egui::Label::new(&toast.text).wrap());
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_text("Нажмите, чтобы скрыть");
                    if response.clicked() {
                        dismissed = Some(index);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(index) = dismissed {
            self.toasts.remove(index);
        }
    }
}

/// Results table, best first. Returns a script the user asked to make the main profile.
fn test_results(
    ui: &mut Ui,
    report: &ProfileTestReport,
    profiles: &[crate::zapret::bundle::BundleProfile],
    main_script: &str,
    palette: theme::Palette,
) -> Option<String> {
    let mut make_main = None;
    let dpi = report.mode == ProfileTestMode::Dpi;
    let max_ok = report
        .rows
        .iter()
        .map(|row| row.ok)
        .max()
        .unwrap_or(0)
        .max(1);

    let mut rows: Vec<_> = report.rows.iter().collect();
    rows.sort_by(|left, right| {
        right
            .ok
            .cmp(&left.ok)
            .then(left.blocked.cmp(&right.blocked))
            .then(left.errors.cmp(&right.errors))
            .then(right.ping_ok.cmp(&left.ping_ok))
    });

    ui.label(RichText::new("Результаты").strong());
    egui::Grid::new("test_results")
        .num_columns(5)
        .striped(true)
        .spacing(vec2(14.0, 6.0))
        .show(ui, |ui| {
            hint(ui, "Профиль");
            hint(ui, "Доступно");
            hint(ui, if dpi { "Обрывы" } else { "Ошибки" });
            hint(ui, if dpi { "" } else { "Ping" });
            ui.label("");
            ui.end_row();

            for row in rows {
                let best = report.best_script.as_deref() == Some(row.script_name.as_str());
                let label = profiles
                    .iter()
                    .find(|profile| profile.script_name().eq_ignore_ascii_case(&row.script_name))
                    .map_or(row.label.as_str(), |profile| profile.label());
                ui.horizontal(|ui| {
                    ui.label(if best {
                        RichText::new(label).strong()
                    } else {
                        RichText::new(label)
                    });
                    if best {
                        pill(ui, "лучший", palette.success);
                    }
                });
                ui.horizontal(|ui| {
                    let fraction = row.ok as f32 / max_ok as f32;
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(90.0)
                            .desired_height(6.0)
                            .fill(if best {
                                palette.success
                            } else {
                                palette.accent
                            }),
                    );
                    ui.label(row.ok.to_string());
                });
                ui.label(if dpi {
                    row.blocked.to_string()
                } else {
                    row.errors.to_string()
                });
                ui.label(if dpi {
                    String::new()
                } else {
                    format!("{}/{}", row.ping_ok, row.ping_ok + row.ping_failed)
                });
                if row.script_name.eq_ignore_ascii_case(main_script) {
                    hint(ui, "основной");
                } else if ui.small_button("Сделать основным").clicked() {
                    make_main = Some(row.script_name.clone());
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        hint(ui, format!("Отчёт: {}", report.result_path.display()));
    });
    make_main
}

fn lerp_fill(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    egui::Color32::from_rgb(
        lerp(from.r(), to.r()),
        lerp(from.g(), to.g()),
        lerp(from.b(), to.b()),
    )
}

fn time_ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "только что".to_owned(),
        60..=3599 => format!("{} мин назад", seconds / 60),
        _ => format!("{} ч назад", seconds / 3600),
    }
}

fn plural_profiles(count: usize) -> &'static str {
    match (count % 10, count % 100) {
        (1, rest) if rest != 11 => "профиль",
        (2..=4, rest) if !(12..=14).contains(&rest) => "профиля",
        _ => "профилей",
    }
}
