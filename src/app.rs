use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use eframe::egui;
use eframe::egui::{Align, Color32, CornerRadius, RichText, Stroke, Vec2};

use crate::core::autostart;
use crate::core::bundle_metadata::detect_bundle_version;
use crate::core::build_info::{APP_AUTHOR, APP_NAME, APP_VERSION, BUILD_DATE, BUILT_BY};
use crate::core::config::{
    load_app_config, save_app_config, AppConfig, TelegramProxyMode,
};
use crate::core::paths::{is_valid_bundle_dir, resolve_paths, ResolvedPaths};
use crate::core::status::{refresh_runtime_status, RuntimeStatus, ServiceState};
use crate::core::tg_proxy_update::{
    check_for_update as check_tg_proxy_update, install_update as install_tg_proxy_update,
    TelegramProxyRelease, TelegramProxyUpdateStatus,
};
use crate::zapret::bundle::{
    run_action, BundleAction, TelegramProxyLaunchConfig, TELEGRAM_PROXY_LAUNCH_LOG_FILE_NAME,
    TELEGRAM_PROXY_LOG_FILE_NAME,
};
use crate::LaunchMode;

pub(crate) struct ZapretHubApp {
    bundle_path: PathBuf,
    bundle_source: String,
    bundle_version: Option<String>,
    status: RuntimeStatus,
    last_profile: Option<&'static str>,
    last_message: String,
    pending_action: Option<PendingAction>,
    tg_proxy_task: Option<PendingTelegramProxyTask>,
    tg_proxy_status: Option<TelegramProxyUpdateStatus>,
    status_monitor: StatusMonitor,
    close_after_stop: bool,
    app_config: AppConfig,
    launch_mode: LaunchMode,
    startup_view_applied: bool,
}

struct PendingAction {
    action: BundleAction,
    receiver: Receiver<anyhow::Result<String>>,
}

struct PendingTelegramProxyTask {
    task: TelegramProxyTask,
    receiver: Receiver<anyhow::Result<TelegramProxyTaskResult>>,
}

enum TelegramProxyTask {
    Check,
    Install(TelegramProxyRelease),
}

enum TelegramProxyTaskResult {
    Checked(TelegramProxyUpdateStatus),
    Installed(String),
}

struct StatusMonitor {
    receiver: Receiver<RuntimeStatus>,
    command_sender: Sender<StatusCommand>,
}

enum StatusCommand {
    Refresh,
}

impl ZapretHubApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, launch_mode: LaunchMode) -> Self {
        apply_custom_style(&cc.egui_ctx);

        let ResolvedPaths { bundle_dir, source } = resolve_paths();
        let status = refresh_runtime_status(&bundle_dir);
        let status_monitor = StatusMonitor::new(cc.egui_ctx.clone(), bundle_dir.clone());
        let mut last_message = if is_valid_bundle_dir(&bundle_dir) {
            "Готово. Запустите основной профиль или остановите уже активный.".to_owned()
        } else {
            "Bundle не найден рядом с приложением. Проверьте установку.".to_owned()
        };
        let mut app_config = match load_app_config() {
            Ok(config) => config,
            Err(error) => {
                last_message = format!(
                    "Настройки не удалось прочитать, использую значения по умолчанию: {error}"
                );
                AppConfig::default()
            }
        };

        match autostart::is_enabled() {
            Ok(system_value) => {
                if app_config.autostart_enabled != system_value {
                    app_config.autostart_enabled = system_value;
                    if let Err(error) = save_app_config(&app_config) {
                        last_message = format!(
                            "Автозапуск определён, но настройки не удалось сохранить: {error}"
                        );
                    }
                }
            }
            Err(error) => {
                last_message = format!("Не удалось проверить автозапуск Windows: {error}");
            }
        }

        let bundle_version = detect_bundle_version(&bundle_dir);

        let mut app = Self {
            bundle_path: bundle_dir,
            bundle_source: source.to_owned(),
            bundle_version,
            status,
            last_profile: None,
            last_message,
            pending_action: None,
            tg_proxy_task: None,
            tg_proxy_status: None,
            status_monitor,
            close_after_stop: false,
            app_config,
            launch_mode,
            startup_view_applied: false,
        };

        if is_valid_bundle_dir(&app.bundle_path) {
            app.start_tg_proxy_check();
        }

        app
    }

    fn set_autostart_enabled(&mut self, enabled: bool) {
        if enabled == self.app_config.autostart_enabled {
            return;
        }

        if let Err(error) = autostart::set_enabled(enabled) {
            self.last_message = format!("Не удалось изменить автозапуск Windows: {error}");
            return;
        }

        self.app_config.autostart_enabled = enabled;
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Автозапуск Windows обновлён, но настройки не удалось сохранить: {error}");
            return;
        }

        self.last_message = if enabled {
            "Автозапуск приложения вместе с Windows включён.".to_owned()
        } else {
            "Автозапуск приложения вместе с Windows выключен.".to_owned()
        };
    }

    fn set_builtin_whitelist_enabled(&mut self, enabled: bool) {
        if enabled == self.app_config.use_builtin_whitelist {
            return;
        }

        self.app_config.use_builtin_whitelist = enabled;
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Не удалось сохранить настройку встроенного списка исключений: {error}");
            return;
        }

        self.last_message = if enabled {
            "Встроенный список исключений будет применяться при запуске основного профиля."
                .to_owned()
        } else {
            "Встроенный список исключений не будет применяться при запуске основного профиля."
                .to_owned()
        };
    }

    fn start_action(&mut self, action: BundleAction) {
        if self.pending_action.is_some() {
            self.last_message = "Дождитесь завершения текущей команды.".to_owned();
            return;
        }

        let telegram_proxy = self.telegram_proxy_launch_config();
        if matches!(
            action,
            BundleAction::StartMainProfile
                | BundleAction::StartMainProfileWithWhitelist
                | BundleAction::StartAlt11
                | BundleAction::StartFakeTlsAutoAlt3
                | BundleAction::StartAlt7
        ) {
            if let Err(error) = self.validate_telegram_proxy_config(&telegram_proxy) {
                self.last_message = format!("Telegram proxy не запущен: {error}");
                return;
            }
        }

        let bundle_path = self.bundle_path.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = run_action(&bundle_path, action, &telegram_proxy);
            let _ = sender.send(result);
        });

        self.last_message = action.in_progress_label().to_owned();
        self.pending_action = Some(PendingAction { action, receiver });
        self.status_monitor.request_refresh();
    }

    fn start_tg_proxy_check(&mut self) {
        if self.tg_proxy_task.is_some() || !is_valid_bundle_dir(&self.bundle_path) {
            return;
        }

        let bundle_path = self.bundle_path.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = check_tg_proxy_update(&bundle_path).map(TelegramProxyTaskResult::Checked);
            let _ = sender.send(result);
        });

        self.tg_proxy_task = Some(PendingTelegramProxyTask {
            task: TelegramProxyTask::Check,
            receiver,
        });
    }

    fn start_tg_proxy_update(&mut self) {
        if self.tg_proxy_task.is_some() || self.runtime_is_active() {
            return;
        }

        let Some(status) = self.tg_proxy_status.clone() else {
            self.last_message = "Сначала дождитесь проверки обновления Telegram proxy.".to_owned();
            return;
        };

        if !status.update_available {
            self.last_message = "Telegram WS proxy уже актуален.".to_owned();
            return;
        }

        let release = status.latest;
        let install_release = release.clone();
        let bundle_path = self.bundle_path.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = install_tg_proxy_update(&bundle_path, &install_release)
                .map(TelegramProxyTaskResult::Installed);
            let _ = sender.send(result);
        });

        self.last_message = format!("Обновляю Telegram WS proxy до {}.", release.tag);
        self.tg_proxy_task = Some(PendingTelegramProxyTask {
            task: TelegramProxyTask::Install(release),
            receiver,
        });
    }

    fn dismiss_tg_proxy_release(&mut self, tag: &str) {
        self.app_config.dismissed_tg_proxy_release_tag = Some(tag.to_owned());
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Не удалось сохранить настройку напоминания про Telegram proxy: {error}");
        } else {
            self.last_message =
                "Напоминание про это обновление Telegram proxy скрыто до следующего релиза."
                    .to_owned();
        }
    }

    fn poll_action_completion(&mut self) {
        if let Some(pending) = &self.pending_action {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let action = pending.action;
                    self.pending_action = None;

                    match result {
                        Ok(message) => {
                            self.last_profile = match action {
                                BundleAction::StartMainProfile
                                | BundleAction::StartMainProfileWithWhitelist => {
                                    Some("SIMPLE FAKE ALT2")
                                }
                                BundleAction::StartAlt11 => Some("ALT11"),
                                BundleAction::StartFakeTlsAutoAlt3 => Some("FAKE TLS AUTO ALT3"),
                                BundleAction::StartAlt7 => Some("ALT7"),
                                BundleAction::StopAll | BundleAction::RemoveService => None,
                                _ => self.last_profile,
                            };
                            self.last_message = self.describe_action_result(action, message);
                            self.status_monitor.request_refresh();
                        }
                        Err(error) => {
                            self.last_message = format!("Ошибка: {error}");
                            self.status_monitor.request_refresh();
                            self.close_after_stop = false;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending_action = None;
                    self.last_message = "Фоновое действие было прервано.".to_owned();
                    self.status_monitor.request_refresh();
                    self.close_after_stop = false;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn poll_tg_proxy_task_completion(&mut self) {
        if let Some(pending) = &self.tg_proxy_task {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let task = match &pending.task {
                        TelegramProxyTask::Check => TelegramProxyTask::Check,
                        TelegramProxyTask::Install(release) => {
                            TelegramProxyTask::Install(release.clone())
                        }
                    };
                    self.tg_proxy_task = None;

                    match result {
                        Ok(TelegramProxyTaskResult::Checked(status)) => {
                            self.tg_proxy_status = Some(status);
                        }
                        Ok(TelegramProxyTaskResult::Installed(message)) => {
                            self.last_message = message;
                            self.app_config.dismissed_tg_proxy_release_tag = None;
                            let _ = save_app_config(&self.app_config);
                            self.start_tg_proxy_check();
                        }
                        Err(error) => {
                            self.last_message = match task {
                                TelegramProxyTask::Check => {
                                    format!("Не удалось проверить обновление Telegram proxy: {error}")
                                }
                                TelegramProxyTask::Install(_) => {
                                    format!("Не удалось обновить Telegram proxy: {error}")
                                }
                            };
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tg_proxy_task = None;
                    self.last_message = "Фоновая задача Telegram proxy была прервана.".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn poll_status_updates(&mut self) {
        while let Ok(status) = self.status_monitor.receiver.try_recv() {
            self.status = status;
        }
    }

    fn overall_state(&self) -> (&'static str, Color32) {
        if matches!(self.status.service_state, ServiceState::StopPending) {
            ("Останавливается", Color32::from_rgb(198, 120, 0))
        } else if self.status.winws_running || self.status.telegram_proxy_running {
            ("Активен", Color32::from_rgb(0, 132, 80))
        } else if matches!(self.status.service_state, ServiceState::Running) {
            ("Сервис активен", Color32::from_rgb(0, 110, 174))
        } else {
            ("Остановлен", Color32::from_rgb(124, 88, 0))
        }
    }

    fn runtime_is_active(&self) -> bool {
        self.status.winws_running
            || self.status.telegram_proxy_running
            || matches!(
                self.status.service_state,
                ServiceState::Running | ServiceState::StopPending
            )
    }

    fn profile_actions_enabled(&self) -> bool {
        self.pending_action.is_none() && !self.runtime_is_active()
    }

    fn stop_action_enabled(&self) -> bool {
        self.pending_action.is_none() && self.runtime_is_active()
    }

    fn service_tools_enabled(&self) -> bool {
        self.pending_action.is_none() && !self.runtime_is_active()
    }

    fn runtime_lock_message(&self) -> Option<&'static str> {
        if self.pending_action.is_some() {
            Some("Дождитесь завершения текущей команды.")
        } else if self.runtime_is_active() {
            Some("Сначала остановите текущий профиль.")
        } else {
            None
        }
    }

    fn service_text(state: ServiceState) -> &'static str {
        match state {
            ServiceState::Running => "запущен",
            ServiceState::Stopped => "установлен, остановлен",
            ServiceState::StopPending => "останавливается",
            ServiceState::NotInstalled => "не установлен",
            ServiceState::Unknown => "неизвестно",
        }
    }

    fn describe_action_result(&self, action: BundleAction, action_message: String) -> String {
        match action {
            BundleAction::StopAll => {
                format!(
                    "{action_message}. Если что-то ещё останавливается, статус обновится автоматически."
                )
            }
            BundleAction::RemoveService => {
                format!("{action_message}. Удаление сервиса и очистка WinDivert были запрошены.")
            }
            BundleAction::InstallService | BundleAction::OpenServiceManager => {
                format!("{action_message}. Завершите действие в открывшемся окне.")
            }
            _ => format!("{action_message}. Статус обновится автоматически через пару секунд."),
        }
    }

    fn profile_launch_caption(&self, profile_name: &str) -> String {
        if self.app_config.launch_telegram_proxy_for_profiles {
            format!(
                "Запускает {profile_name} и Telegram WS proxy в режиме {}.",
                self.telegram_proxy_mode_label(self.app_config.telegram_proxy_mode.clone())
            )
        } else {
            format!("Запускает {profile_name} без Telegram WS proxy.")
        }
    }

    fn telegram_proxy_launch_config(&self) -> TelegramProxyLaunchConfig {
        TelegramProxyLaunchConfig {
            enabled: self.app_config.launch_telegram_proxy_for_profiles,
            mode: self.app_config.telegram_proxy_mode.clone(),
            cf_domain: self.app_config.telegram_cf_domain.clone(),
        }
    }

    fn validate_telegram_proxy_config(
        &self,
        telegram_proxy: &TelegramProxyLaunchConfig,
    ) -> anyhow::Result<()> {
        if !telegram_proxy.enabled {
            return Ok(());
        }

        if telegram_proxy.mode == TelegramProxyMode::CfMedia
            && telegram_proxy.cf_domain.trim().is_empty()
        {
            anyhow::bail!("для CF media режима укажите домен в Cloudflare")
        }

        Ok(())
    }

    fn set_telegram_proxy_mode(&mut self, mode: TelegramProxyMode) {
        if self.app_config.telegram_proxy_mode == mode {
            return;
        }

        self.app_config.telegram_proxy_mode = mode.clone();
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Не удалось сохранить режим Telegram proxy: {error}");
            return;
        }

        self.last_message = match mode {
            TelegramProxyMode::Standard => {
                "Telegram proxy переведён в обычный режим: DC2/DC4/DC203.".to_owned()
            }
            TelegramProxyMode::CfMedia => {
                "Telegram proxy переведён в CF media режим. Укажите домен Cloudflare."
                    .to_owned()
            }
        };
    }

    fn set_telegram_cf_domain(&mut self, domain: String) {
        if self.app_config.telegram_cf_domain == domain {
            return;
        }

        self.app_config.telegram_cf_domain = domain;
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Не удалось сохранить домен для Telegram proxy: {error}");
        } else {
            self.last_message = format!(
                "Домен для CF media сохранён: {}.",
                self.app_config.telegram_cf_domain
            );
        }
    }

    fn reset_telegram_cf_domain_to_default(&mut self) {
        self.set_telegram_cf_domain(String::new());
        if self.app_config.telegram_cf_domain.is_empty() {
            self.last_message =
                "Домен CF media очищен. Перед запуском укажите свой Cloudflare-домен.".to_owned();
        }
    }

    fn telegram_proxy_mode_label(&self, mode: TelegramProxyMode) -> &'static str {
        match mode {
            TelegramProxyMode::Standard => "обычный",
            TelegramProxyMode::CfMedia => "CF media",
        }
    }

    fn tg_proxy_controls_enabled(&self) -> bool {
        self.pending_action.is_none() && self.tg_proxy_task.is_none() && !self.runtime_is_active()
    }

    fn state_badge(ui: &mut egui::Ui, text: &str, color: Color32) {
        egui::Frame::new()
            .fill(color.linear_multiply(0.14))
            .stroke(Stroke::new(1.0, color.linear_multiply(0.55)))
            .corner_radius(CornerRadius::same(255))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.label(RichText::new(text).strong().color(color));
            });
    }

    fn card(ui: &mut egui::Ui, title: &str, subtitle: &str, add_body: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.label(RichText::new(title).strong().size(18.0));
                ui.add_space(4.0);
                ui.label(RichText::new(subtitle).color(Color32::from_gray(120)));
                ui.add_space(14.0);
                add_body(ui);
            });
    }

    fn primary_button(
        ui: &mut egui::Ui,
        title: &str,
        caption: &str,
        enabled: bool,
    ) -> egui::Response {
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(title).strong())
                .min_size(Vec2::new(ui.available_width(), 42.0)),
        )
        .on_hover_text(caption)
    }

    fn status_row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
        ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(label).strong());
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(value.into());
            });
        });
    }

    fn draw_overview(&mut self, ui: &mut egui::Ui) {
        let (state_label, state_color) = self.overall_state();

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Состояние").strong().size(18.0));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Главная информация о работе приложения.")
                                .color(Color32::from_gray(120)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::TOP), |ui| {
                        if self.pending_action.is_some() {
                            ui.horizontal(|ui| {
                                ui.label("Выполняется команда.");
                                ui.spinner();
                            });
                            ui.add_space(8.0);
                        }
                        Self::state_badge(ui, state_label, state_color);
                    });
                });

                ui.add_space(14.0);
                Self::status_row(ui, "winws.exe", yes_no(self.status.winws_running));
                Self::status_row(
                    ui,
                    "Telegram WS proxy",
                    yes_no(self.status.telegram_proxy_running),
                );
                Self::status_row(
                    ui,
                    "Сервис zapret",
                    Self::service_text(self.status.service_state),
                );
                if let Some(profile) = self.last_profile {
                    Self::status_row(ui, "Последний профиль", profile);
                }
            });
    }

    fn draw_primary_actions(&mut self, ui: &mut egui::Ui) {
        let start_enabled = self.profile_actions_enabled();
        let stop_enabled = self.stop_action_enabled();

        Self::card(
            ui,
            "Основные действия",
            "Главные действия для обычного пользователя.",
            |ui| {
                if let Some(message) = self.runtime_lock_message() {
                    ui.label(RichText::new(message).color(Color32::from_rgb(198, 120, 0)));
                    ui.add_space(8.0);
                }

                ui.horizontal(|ui| {
                    let button_width = (ui.available_width() - 370.0).max(180.0);
                    let start_action = if self.app_config.use_builtin_whitelist {
                        BundleAction::StartMainProfileWithWhitelist
                    } else {
                        BundleAction::StartMainProfile
                    };
                    let start_caption = if self.app_config.use_builtin_whitelist {
                        if self.app_config.launch_telegram_proxy_for_profiles {
                            "Запускает SIMPLE FAKE ALT2, Telegram WS proxy и добавляет встроенный список исключений."
                        } else {
                            "Запускает SIMPLE FAKE ALT2 и добавляет встроенный список исключений."
                        }
                    } else {
                        if self.app_config.launch_telegram_proxy_for_profiles {
                            "Запускает SIMPLE FAKE ALT2 и Telegram WS proxy."
                        } else {
                            "Запускает SIMPLE FAKE ALT2 без Telegram WS proxy."
                        }
                    };

                    if ui
                        .add_enabled(
                            start_enabled,
                            egui::Button::new(RichText::new("Запустить основной профиль").strong())
                                .min_size(Vec2::new(button_width, 42.0)),
                        )
                        .on_hover_text(start_caption)
                        .clicked()
                    {
                        self.start_action(start_action);
                    }

                    let mut use_builtin_whitelist = self.app_config.use_builtin_whitelist;
                    let checkbox_response = ui.add_enabled(
                        start_enabled,
                        egui::Checkbox::new(&mut use_builtin_whitelist, "Белый список"),
                    );
                    if checkbox_response.changed() {
                        self.set_builtin_whitelist_enabled(use_builtin_whitelist);
                    }
                    let checkbox_hover = if start_enabled {
                        "Добавляет в list-exclude-user.txt встроенный список доменов, которые лучше не пропускать через zapret."
                    } else {
                        "Остановите текущий профиль, чтобы изменить этот режим."
                    };
                    checkbox_response.on_hover_text(checkbox_hover);

                    let mut launch_proxy = self.app_config.launch_telegram_proxy_for_profiles;
                    let proxy_checkbox = ui.add_enabled(
                        start_enabled,
                        egui::Checkbox::new(&mut launch_proxy, "Telegram proxy"),
                    );
                    if proxy_checkbox.changed() {
                        self.app_config.launch_telegram_proxy_for_profiles = launch_proxy;
                        if let Err(error) = save_app_config(&self.app_config) {
                            self.last_message = format!(
                                "Не удалось сохранить режим запуска Telegram proxy: {error}"
                            );
                        } else if launch_proxy {
                            self.last_message = format!(
                                "Профили теперь будут запускаться вместе с Telegram WS proxy в режиме {}.",
                                self.telegram_proxy_mode_label(
                                    self.app_config.telegram_proxy_mode.clone()
                                )
                            );
                        } else {
                            self.last_message = "Профили теперь запускаются без Telegram WS proxy. Для Telegram Desktop 6.7.2+ это обычно достаточно.".to_owned();
                        }
                    }
                    let proxy_hover = if start_enabled {
                        "Включайте это только для старого Telegram Desktop или если нужен отдельный WS proxy. Для Telegram Desktop 6.7.2+ обычно не требуется."
                    } else {
                        "Остановите текущий профиль, чтобы изменить этот режим."
                    };
                    proxy_checkbox.on_hover_text(proxy_hover);
                });

                ui.add_space(8.0);

                if self.app_config.launch_telegram_proxy_for_profiles {
                    ui.add_enabled_ui(start_enabled, |ui| {
                        ui.horizontal(|ui| {
                            let mut selected_mode = self.app_config.telegram_proxy_mode.clone();
                            egui::ComboBox::from_id_salt("telegram_proxy_mode")
                                .selected_text(self.telegram_proxy_mode_label(selected_mode.clone()))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut selected_mode,
                                        TelegramProxyMode::Standard,
                                        "Обычный",
                                    );
                                    ui.selectable_value(
                                        &mut selected_mode,
                                        TelegramProxyMode::CfMedia,
                                        "CF media",
                                    );
                                });
                            if selected_mode != self.app_config.telegram_proxy_mode {
                                self.set_telegram_proxy_mode(selected_mode);
                            }

                            if self.app_config.telegram_proxy_mode == TelegramProxyMode::CfMedia {
                                let mut domain = self.app_config.telegram_cf_domain.clone();
                                let response = ui.add_sized(
                                    [260.0, 28.0],
                                    egui::TextEdit::singleline(&mut domain)
                                        .hint_text("your-domain.example"),
                                );
                                if response.changed() {
                                    self.set_telegram_cf_domain(domain.trim().to_owned());
                                }
                                response.on_hover_text(
                                    "Полностью управляемый домен в Cloudflare. Обычный dynamic DNS сюда не подходит.",
                                );

                                if ui
                                    .add_enabled(
                                        !self.app_config.telegram_cf_domain.trim().is_empty(),
                                        egui::Button::new("Очистить"),
                                    )
                                    .on_hover_text(
                                        "Очистить Cloudflare-домен для CF media режима.",
                                    )
                                    .clicked()
                                {
                                    self.reset_telegram_cf_domain_to_default();
                                }
                            }
                        });
                    });

                    ui.add_space(8.0);
                    match self.app_config.telegram_proxy_mode {
                        TelegramProxyMode::Standard => {
                            ui.label(
                                RichText::new(
                                    "Обычный режим запускает Telegram proxy с DC2/DC4/DC203. Это первый вариант для старого Telegram Desktop.",
                                )
                                .color(Color32::from_gray(120)),
                            );
                        }
                        TelegramProxyMode::CfMedia => {
                            ui.label(
                                RichText::new(
                                    "CF media режим запускает Telegram proxy через ваш Cloudflare-домен и только с DC4. Нужен именно для кейса, когда текст идёт, а фото и видео нет.",
                                )
                                .color(Color32::from_rgb(198, 120, 0)),
                            );
                        }
                    }
                }

                if Self::primary_button(
                    ui,
                    "Остановить всё",
                    "Останавливает winws, Telegram proxy, сервис и связанные процессы.",
                    stop_enabled,
                )
                .clicked()
                {
                    self.start_action(BundleAction::StopAll);
                }

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Telegram Desktop 6.7.2 и новее обычно не требуют автоматического запуска Telegram WS proxy вместе с профилем.",
                    )
                    .color(Color32::from_gray(120)),
                );
            },
        );
    }

    fn draw_profiles(&mut self, ui: &mut egui::Ui) {
        let enabled = self.profile_actions_enabled();

        Self::card(
            ui,
            "Дополнительные профили",
            "Запасные пресеты, если основной профиль не подходит.",
            |ui| {
                if let Some(message) = self.runtime_lock_message() {
                    ui.label(RichText::new(message).color(Color32::from_rgb(198, 120, 0)));
                    ui.add_space(8.0);
                }

                if Self::primary_button(
                    ui,
                    "ALT11",
                    &self.profile_launch_caption("ALT11"),
                    enabled,
                )
                    .clicked()
                {
                    self.start_action(BundleAction::StartAlt11);
                }

                ui.add_space(8.0);

                if Self::primary_button(
                    ui,
                    "FAKE TLS AUTO ALT3",
                    &self.profile_launch_caption("FAKE TLS AUTO ALT3"),
                    enabled,
                )
                .clicked()
                {
                    self.start_action(BundleAction::StartFakeTlsAutoAlt3);
                }

                ui.add_space(8.0);

                if Self::primary_button(
                    ui,
                    "ALT7",
                    &self.profile_launch_caption("ALT7"),
                    enabled,
                )
                    .clicked()
                {
                    self.start_action(BundleAction::StartAlt7);
                }
            },
        );
    }

    fn draw_service_tools(&mut self, ui: &mut egui::Ui) {
        let enabled = self.service_tools_enabled();

        Self::card(
            ui,
            "Сервис и настройка",
            "Редкие действия вынесены отдельно, чтобы не мешать основному сценарию.",
            |ui| {
                if let Some(message) = self.runtime_lock_message() {
                    ui.label(RichText::new(message).color(Color32::from_rgb(198, 120, 0)));
                    ui.add_space(8.0);
                }

                if Self::primary_button(
                    ui,
                    "Установить сервис",
                    "Открывает сценарий установки сервиса.",
                    enabled,
                )
                .clicked()
                {
                    self.start_action(BundleAction::InstallService);
                }

                ui.add_space(8.0);

                if Self::primary_button(
                    ui,
                    "Удалить сервис",
                    "Останавливает текущий runtime и удаляет сервис zapret.",
                    enabled,
                )
                .clicked()
                {
                    self.start_action(BundleAction::RemoveService);
                }

                ui.add_space(8.0);

                if Self::primary_button(
                    ui,
                    "Открыть service.bat",
                    "Открывает оригинальный менеджер сервиса.",
                    enabled,
                )
                .clicked()
                {
                    self.start_action(BundleAction::OpenServiceManager);
                }
            },
        );
    }

    fn draw_installation_info(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Установка",
            "Откуда взят bundle и какая сборка сейчас запущена.",
            |ui| {
                ui.label(self.bundle_path.display().to_string());
                ui.add_space(8.0);
                Self::status_row(ui, "Источник bundle", self.bundle_source.as_str());
                Self::status_row(
                    ui,
                    "Версия bundle",
                    self.bundle_version.as_deref().unwrap_or("неизвестно"),
                );
                Self::status_row(ui, "Версия приложения", format!("v{APP_VERSION}"));
                Self::status_row(ui, "Дата сборки", BUILD_DATE);
                Self::status_row(ui, "Автор / сборка", author_build_label());
                Self::status_row(
                    ui,
                    "Автозапуск Windows",
                    yes_no(self.app_config.autostart_enabled),
                );
                ui.add_space(12.0);

                let mut autostart_enabled = self.app_config.autostart_enabled;
                let response =
                    ui.checkbox(&mut autostart_enabled, "Включить автозапуск приложения");
                if response.changed() {
                    self.set_autostart_enabled(autostart_enabled);
                }

                ui.label(
                    RichText::new(
                        "Включает только запуск приложения через Планировщик заданий Windows. Профиль, proxy и сервис сами не стартуют.",
                    )
                    .color(Color32::from_gray(120)),
                );

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                let tg_proxy_status = self.tg_proxy_status.clone();
                if let Some(status) = tg_proxy_status {
                    let installed = status.installed_tag.as_deref().unwrap_or("неизвестно");
                    Self::status_row(ui, "Telegram proxy в bundle", installed);
                    Self::status_row(ui, "Последний релиз", status.latest.tag.as_str());
                    Self::status_row(
                        ui,
                        "Режим Telegram proxy",
                        self.telegram_proxy_mode_label(self.app_config.telegram_proxy_mode.clone()),
                    );
                    Self::status_row(
                        ui,
                        "Лог Telegram proxy",
                        self.bundle_path
                            .join(TELEGRAM_PROXY_LOG_FILE_NAME)
                            .display()
                            .to_string(),
                    );
                    Self::status_row(
                        ui,
                        "Launch-лог Telegram proxy",
                        self.bundle_path
                            .join(TELEGRAM_PROXY_LAUNCH_LOG_FILE_NAME)
                            .display()
                            .to_string(),
                    );
                    if self.app_config.telegram_proxy_mode == TelegramProxyMode::CfMedia
                        && !self.app_config.telegram_cf_domain.trim().is_empty()
                    {
                        Self::status_row(
                            ui,
                            "CF домен",
                            self.app_config.telegram_cf_domain.trim().to_owned(),
                        );
                    }

                    if status.update_available {
                        let dismissed = self
                            .app_config
                            .dismissed_tg_proxy_release_tag
                            .as_deref()
                            == Some(status.latest.tag.as_str());

                        if dismissed {
                            ui.label(
                                RichText::new(
                                    "Обновление Telegram proxy скрыто до следующего релиза.",
                                )
                                .color(Color32::from_gray(120)),
                            );
                        } else {
                            ui.label(
                                RichText::new(
                                    "Доступно обновление Telegram WS proxy. Старые версии хранить не нужно: бинарь заменяется на месте.",
                                )
                                .color(Color32::from_rgb(198, 120, 0)),
                            );
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let can_update = self.tg_proxy_controls_enabled();
                            if ui
                                .add_enabled(
                                    can_update && !dismissed,
                                    egui::Button::new("Обновить Telegram proxy сейчас"),
                                )
                                .on_hover_text(
                                    "Скачивает последний TgWsProxy_windows.exe и заменяет текущий файл без хранения старой версии.",
                                )
                                .clicked()
                            {
                                self.start_tg_proxy_update();
                            }

                            if ui
                                .add_enabled(
                                    self.tg_proxy_task.is_none() && !dismissed,
                                    egui::Button::new("Позже"),
                                )
                                .clicked()
                            {
                                self.dismiss_tg_proxy_release(&status.latest.tag);
                            }
                        });

                        if !self.tg_proxy_controls_enabled() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    "Остановите текущий профиль или proxy перед обновлением Telegram proxy.",
                                )
                                .color(Color32::from_gray(120)),
                            );
                        }
                    } else {
                        ui.label(
                            RichText::new("Telegram WS proxy уже актуален.")
                                .color(Color32::from_rgb(0, 132, 80)),
                        );
                    }
                } else if self.tg_proxy_task.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Проверяю обновления Telegram proxy.");
                    });
                } else {
                    ui.label(
                        RichText::new("Проверка обновлений Telegram proxy ещё не завершилась.")
                            .color(Color32::from_gray(120)),
                    );
                }
            },
        );
    }

    fn draw_status_log(&self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Последний результат",
            "Короткое объяснение, что произошло после последнего действия.",
            |ui| {
                ui.label(&self.last_message);
                if self.close_after_stop {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Приложение закроется само после завершения остановки.")
                            .color(Color32::from_rgb(0, 110, 174)),
                    );
                }
                if matches!(self.status.service_state, ServiceState::StopPending) {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Сервис всё ещё в STOP_PENDING. Это временно, интерфейс обновится сам.",
                        )
                        .color(Color32::from_rgb(198, 120, 0)),
                    );
                }
            },
        );
    }
}

impl BundleAction {
    fn in_progress_label(self) -> &'static str {
        match self {
            BundleAction::StartMainProfile => "Запускаю основной профиль.",
            BundleAction::StartMainProfileWithWhitelist => {
                "Запускаю основной профиль со встроенным списком исключений."
            }
            BundleAction::StartAlt11 => "Запускаю ALT11.",
            BundleAction::StartFakeTlsAutoAlt3 => "Запускаю FAKE TLS AUTO ALT3.",
            BundleAction::StartAlt7 => "Запускаю ALT7.",
            BundleAction::StopAll => "Останавливаю bypass, proxy и связанные процессы.",
            BundleAction::InstallService => "Открываю установку сервиса.",
            BundleAction::RemoveService => "Удаляю сервис и останавливаю остатки.",
            BundleAction::OpenServiceManager => "Открываю service.bat.",
        }
    }
}

impl eframe::App for ZapretHubApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_action_completion();
        self.poll_tg_proxy_task_completion();
        self.poll_status_updates();

        if self.launch_mode.is_autostart() && !self.startup_view_applied {
            self.startup_view_applied = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        if self.close_after_stop && self.pending_action.is_none() && !self.runtime_is_active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.pending_action.is_some() || self.tg_proxy_task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.pending_action.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.last_message =
                    "Сначала дождитесь завершения текущей команды, потом закрывайте окно."
                        .to_owned();
            } else if self.runtime_is_active() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_after_stop = true;
                self.start_action(BundleAction::StopAll);
                self.last_message =
                    "Перед закрытием останавливаю bypass, proxy и сервис.".to_owned();
            }
        }

        egui::TopBottomPanel::top("top_panel")
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(16)))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(APP_NAME);
                    ui.label(
                        RichText::new(format!("v{APP_VERSION}")).color(Color32::from_gray(110)),
                    );
                    ui.add_space(12.0);
                    let (state_label, state_color) = self.overall_state();
                    Self::state_badge(ui, state_label, state_color);
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Обновить статус").clicked() {
                            self.status_monitor.request_refresh();
                            self.last_message = "Запросил обновление статуса.".to_owned();
                        }
                    });
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let stacked = ui.available_width() < 980.0;

                    if stacked {
                        self.draw_overview(ui);
                        ui.add_space(12.0);
                        self.draw_primary_actions(ui);
                        ui.add_space(12.0);
                        self.draw_profiles(ui);
                        ui.add_space(12.0);
                        self.draw_service_tools(ui);
                        ui.add_space(12.0);
                        self.draw_installation_info(ui);
                        ui.add_space(12.0);
                        self.draw_status_log(ui);
                    } else {
                        ui.columns(2, |columns| {
                            self.draw_overview(&mut columns[0]);
                            self.draw_primary_actions(&mut columns[1]);
                        });
                        ui.add_space(12.0);
                        ui.columns(2, |columns| {
                            self.draw_profiles(&mut columns[0]);
                            self.draw_service_tools(&mut columns[1]);
                        });
                        ui.add_space(12.0);
                        ui.columns(2, |columns| {
                            self.draw_installation_info(&mut columns[0]);
                            self.draw_status_log(&mut columns[1]);
                        });
                    }
                });
        });
    }
}

impl StatusMonitor {
    fn new(ctx: egui::Context, bundle_path: PathBuf) -> Self {
        let (status_sender, receiver) = mpsc::channel();
        let (command_sender, command_receiver) = mpsc::channel();

        thread::spawn(move || loop {
            let status = refresh_runtime_status(&bundle_path);
            let _ = status_sender.send(status);
            ctx.request_repaint();

            match command_receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(StatusCommand::Refresh) => continue,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        });

        Self {
            receiver,
            command_sender,
        }
    }

    fn request_refresh(&self) {
        let _ = self.command_sender.send(StatusCommand::Refresh);
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "да"
    } else {
        "нет"
    }
}

fn author_build_label() -> String {
    if APP_AUTHOR == BUILT_BY {
        APP_AUTHOR.to_owned()
    } else {
        format!("{APP_AUTHOR} / {BUILT_BY}")
    }
}

fn apply_custom_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let dark_mode = style.visuals.dark_mode;

    style.spacing.item_spacing = Vec2::new(12.0, 12.0);
    style.spacing.button_padding = Vec2::new(16.0, 12.0);
    style.spacing.window_margin = egui::Margin::same(14);

    let accent = if dark_mode {
        Color32::from_rgb(96, 154, 255)
    } else {
        Color32::from_rgb(32, 104, 196)
    };
    let accent_hover = if dark_mode {
        Color32::from_rgb(122, 172, 255)
    } else {
        Color32::from_rgb(48, 122, 214)
    };
    let base_fill = if dark_mode {
        Color32::from_rgb(19, 23, 31)
    } else {
        Color32::from_rgb(250, 251, 253)
    };
    let soft_fill = if dark_mode {
        Color32::from_rgb(28, 34, 44)
    } else {
        Color32::from_rgb(242, 245, 249)
    };
    let text_color = if dark_mode {
        Color32::from_rgb(232, 237, 244)
    } else {
        Color32::from_rgb(24, 31, 42)
    };
    let muted_text_color = if dark_mode {
        Color32::from_rgb(170, 179, 191)
    } else {
        Color32::from_rgb(87, 96, 112)
    };
    let widget_stroke = if dark_mode {
        Color32::from_rgb(62, 72, 90)
    } else {
        Color32::from_gray(220)
    };

    style.visuals.override_text_color = Some(text_color);
    style.visuals.hyperlink_color = accent;
    style.visuals.faint_bg_color = soft_fill;
    style.visuals.extreme_bg_color = if dark_mode {
        Color32::from_rgb(12, 15, 21)
    } else {
        Color32::from_rgb(255, 255, 255)
    };
    style.visuals.code_bg_color = if dark_mode {
        Color32::from_rgb(22, 27, 36)
    } else {
        Color32::from_rgb(246, 248, 252)
    };
    style.visuals.selection.bg_fill = accent;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.active.bg_fill = accent;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.hovered.bg_fill = accent_hover;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, text_color);
    style.visuals.widgets.inactive.bg_fill = soft_fill;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_color);
    style.visuals.widgets.open.bg_fill = soft_fill;
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, text_color);
    style.visuals.window_fill = base_fill;
    style.visuals.panel_fill = style.visuals.window_fill;
    style.visuals.widgets.noninteractive.bg_fill = soft_fill;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, widget_stroke);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_color);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, widget_stroke);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_hover);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    style.visuals.weak_text_color = Some(muted_text_color);
    style.visuals.window_corner_radius = CornerRadius::same(14);
    style.visuals.menu_corner_radius = CornerRadius::same(12);

    ctx.set_style(style);
}
