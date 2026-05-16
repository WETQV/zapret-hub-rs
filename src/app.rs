use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use eframe::egui;
use eframe::egui::{Align, Color32, CornerRadius, RichText, Stroke, Vec2};

use crate::LaunchMode;
use crate::core::autostart;
use crate::core::build_info::{APP_AUTHOR, APP_NAME, APP_VERSION, BUILD_DATE, BUILT_BY};
use crate::core::bundle_metadata::detect_bundle_version;
use crate::core::bundle_update::{
    BundleRelease, BundleUpdateStatus, check_for_update as check_bundle_update,
    install_update as install_bundle_update,
};
use crate::core::config::{
    AppConfig, TelegramProxyMode, ZapretProfile, load_app_config, save_app_config,
};
use crate::core::paths::{ResolvedPaths, is_valid_bundle_dir, resolve_paths};
use crate::core::status::{RuntimeStatus, ServiceState, refresh_runtime_status};
use crate::core::tg_proxy_update::{
    TelegramProxyRelease, TelegramProxyUpdateStatus, check_for_update as check_tg_proxy_update,
    install_update as install_tg_proxy_update,
};
use crate::zapret::bundle::{BundleAction, TelegramProxyLaunchConfig, run_action};

pub(crate) struct ZapretHubApp {
    bundle_path: PathBuf,
    bundle_source: String,
    bundle_version: Option<String>,
    status: RuntimeStatus,
    last_profile: Option<&'static str>,
    last_message: String,
    startup_notices: Vec<StartupNotice>,
    pending_action: Option<PendingAction>,
    bundle_task: Option<PendingBundleUpdateTask>,
    bundle_status: Option<BundleUpdateStatus>,
    bundle_check_error: Option<String>,
    tg_proxy_task: Option<PendingTelegramProxyTask>,
    tg_proxy_status: Option<TelegramProxyUpdateStatus>,
    tg_proxy_check_error: Option<String>,
    status_monitor: StatusMonitor,
    repaint_ctx: egui::Context,
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

struct PendingBundleUpdateTask {
    task: BundleUpdateTask,
    receiver: Receiver<anyhow::Result<BundleUpdateTaskResult>>,
}

enum BundleUpdateTask {
    Check(UpdateCheckReason),
    Install(BundleRelease),
}

enum TelegramProxyTask {
    Check(UpdateCheckReason),
    Install(TelegramProxyRelease),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateCheckReason {
    Startup,
    Manual,
}

#[derive(Clone, Debug)]
struct StartupNotice {
    kind: StartupNoticeKind,
    title: String,
    message: String,
}

#[derive(Clone, Debug)]
enum StartupNoticeKind {
    AppUpdated,
    BundleUpdate(String),
    TelegramProxyUpdate(String),
}

enum StartupNoticeAction {
    Close(usize),
    Dismiss(usize),
    DisableAll,
}

enum BundleUpdateTaskResult {
    Checked(BundleUpdateStatus),
    Installed(String),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppTab {
    Main,
    Profiles,
    Telegram,
    Updates,
    Settings,
}

impl AppTab {
    const ALL: [Self; 5] = [
        Self::Main,
        Self::Profiles,
        Self::Telegram,
        Self::Updates,
        Self::Settings,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Profiles => "profiles",
            Self::Telegram => "telegram",
            Self::Updates => "updates",
            Self::Settings => "settings",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Main => "Главная",
            Self::Profiles => "Профили",
            Self::Telegram => "Telegram",
            Self::Updates => "Обновления",
            Self::Settings => "Настройки",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "profiles" => Self::Profiles,
            "telegram" => Self::Telegram,
            "updates" => Self::Updates,
            "settings" => Self::Settings,
            _ => Self::Main,
        }
    }
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
        let mut startup_notices = Vec::new();
        if app_config.startup_notifications_enabled
            && app_config.last_seen_app_version.as_deref() != Some(APP_VERSION)
        {
            startup_notices.push(StartupNotice {
                kind: StartupNoticeKind::AppUpdated,
                title: "Приложение обновилось".to_owned(),
                message: format!("Zapret Hub теперь v{APP_VERSION}."),
            });
            app_config.last_seen_app_version = Some(APP_VERSION.to_owned());
            let _ = save_app_config(&app_config);
        }

        let mut app = Self {
            bundle_path: bundle_dir,
            bundle_source: source.to_owned(),
            bundle_version,
            status,
            last_profile: None,
            last_message,
            startup_notices,
            pending_action: None,
            bundle_task: None,
            bundle_status: None,
            bundle_check_error: None,
            tg_proxy_task: None,
            tg_proxy_status: None,
            tg_proxy_check_error: None,
            status_monitor,
            repaint_ctx: cc.egui_ctx.clone(),
            close_after_stop: false,
            app_config,
            launch_mode,
            startup_view_applied: false,
        };

        if is_valid_bundle_dir(&app.bundle_path) {
            app.start_tg_proxy_check_with_reason(UpdateCheckReason::Startup);
            app.start_bundle_update_check_with_reason(UpdateCheckReason::Startup);
        }

        app
    }

    fn selected_tab(&self) -> AppTab {
        AppTab::from_id(&self.app_config.selected_tab)
    }

    fn set_selected_tab(&mut self, tab: AppTab) {
        if self.app_config.selected_tab == tab.id() {
            return;
        }

        self.app_config.selected_tab = tab.id().to_owned();
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message = format!("Не удалось сохранить выбранную вкладку: {error}");
        }
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

    fn set_startup_notifications_enabled(&mut self, enabled: bool) {
        if enabled == self.app_config.startup_notifications_enabled {
            return;
        }

        self.app_config.startup_notifications_enabled = enabled;
        if !enabled {
            self.startup_notices.clear();
        }

        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message = format!("Не удалось сохранить настройку уведомлений: {error}");
        } else if enabled {
            self.last_message = "Стартовые уведомления включены.".to_owned();
        } else {
            self.last_message = "Стартовые уведомления выключены.".to_owned();
        }
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

    fn set_main_profile(&mut self, profile: ZapretProfile) {
        if self.app_config.main_profile == profile {
            return;
        }

        self.app_config.main_profile = profile;
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message = format!("Не удалось сохранить основной профиль: {error}");
        } else {
            self.last_message = format!("Основной профиль: {}.", profile.label());
        }
    }

    fn start_selected_profile(&mut self) {
        self.start_action(BundleAction::StartProfile {
            profile: self.app_config.main_profile,
            use_builtin_whitelist: self.app_config.use_builtin_whitelist,
        });
    }

    fn start_action(&mut self, action: BundleAction) {
        if self.pending_action.is_some() {
            self.last_message = "Дождитесь завершения текущей команды.".to_owned();
            return;
        }

        let telegram_proxy = self.telegram_proxy_launch_config();
        if matches!(action, BundleAction::StartProfile { .. }) {
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

        self.last_message = action.in_progress_label();
        self.pending_action = Some(PendingAction { action, receiver });
        self.status_monitor.request_refresh();
    }

    fn start_tg_proxy_check(&mut self) {
        self.start_tg_proxy_check_with_reason(UpdateCheckReason::Manual);
    }

    fn start_tg_proxy_check_with_reason(&mut self, reason: UpdateCheckReason) {
        if self.tg_proxy_task.is_some() || !is_valid_bundle_dir(&self.bundle_path) {
            return;
        }

        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = check_tg_proxy_update(&bundle_path).map(TelegramProxyTaskResult::Checked);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
        });

        self.tg_proxy_check_error = None;
        self.tg_proxy_task = Some(PendingTelegramProxyTask {
            task: TelegramProxyTask::Check(reason),
            receiver,
        });
    }

    fn start_bundle_update_check(&mut self) {
        self.start_bundle_update_check_with_reason(UpdateCheckReason::Manual);
    }

    fn start_bundle_update_check_with_reason(&mut self, reason: UpdateCheckReason) {
        if self.bundle_task.is_some() || !is_valid_bundle_dir(&self.bundle_path) {
            return;
        }

        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = check_bundle_update(&bundle_path).map(BundleUpdateTaskResult::Checked);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
        });

        self.bundle_check_error = None;
        self.bundle_task = Some(PendingBundleUpdateTask {
            task: BundleUpdateTask::Check(reason),
            receiver,
        });
    }

    fn start_bundle_update(&mut self) {
        if self.bundle_task.is_some() || self.runtime_is_active() {
            return;
        }

        let Some(status) = self.bundle_status.clone() else {
            self.last_message = if self.bundle_check_error.is_some() {
                "Проверка обновления bundle завершилась ошибкой. Повторите проверку.".to_owned()
            } else {
                "Сначала дождитесь проверки обновления bundle.".to_owned()
            };
            return;
        };

        if !status.update_available {
            self.last_message = "Bundle уже актуален.".to_owned();
            return;
        }

        let release = status.latest;
        let install_release = release.clone();
        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = install_bundle_update(&bundle_path, &install_release)
                .map(BundleUpdateTaskResult::Installed);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
        });

        self.last_message = format!("Обновляю bundle до {}.", release.tag);
        self.bundle_task = Some(PendingBundleUpdateTask {
            task: BundleUpdateTask::Install(release),
            receiver,
        });
    }

    fn start_tg_proxy_update(&mut self) {
        if self.tg_proxy_task.is_some() || self.runtime_is_active() {
            return;
        }

        let Some(status) = self.tg_proxy_status.clone() else {
            self.last_message = if self.tg_proxy_check_error.is_some() {
                "Проверка обновлений Telegram proxy завершилась ошибкой. Нажмите «Повторить проверку»."
                    .to_owned()
            } else {
                "Сначала дождитесь проверки обновления Telegram proxy.".to_owned()
            };
            return;
        };

        if !status.update_available {
            self.last_message = "Telegram WS proxy уже актуален.".to_owned();
            return;
        }

        let release = status.latest;
        let install_release = release.clone();
        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = install_tg_proxy_update(&bundle_path, &install_release)
                .map(TelegramProxyTaskResult::Installed);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
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

    fn dismiss_bundle_release(&mut self, tag: &str) {
        self.app_config.dismissed_bundle_release_tag = Some(tag.to_owned());
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message =
                format!("Не удалось сохранить напоминание про обновление bundle: {error}");
        } else {
            self.last_message =
                "Напоминание про это обновление bundle скрыто до следующего релиза.".to_owned();
        }
    }

    fn add_bundle_update_notice(&mut self, status: &BundleUpdateStatus) {
        if !self.app_config.startup_notifications_enabled
            || !status.update_available
            || self.app_config.dismissed_bundle_release_tag.as_deref()
                == Some(status.latest.tag.as_str())
        {
            return;
        }

        self.startup_notices.push(StartupNotice {
            kind: StartupNoticeKind::BundleUpdate(status.latest.tag.clone()),
            title: "Новая версия Zapret".to_owned(),
            message: format!(
                "Доступен bundle {}. Можно обновить во вкладке «Обновления».",
                status.latest.tag
            ),
        });
    }

    fn add_tg_proxy_update_notice(&mut self, status: &TelegramProxyUpdateStatus) {
        if !self.app_config.startup_notifications_enabled
            || !status.update_available
            || self.app_config.dismissed_tg_proxy_release_tag.as_deref()
                == Some(status.latest.tag.as_str())
        {
            return;
        }

        self.startup_notices.push(StartupNotice {
            kind: StartupNoticeKind::TelegramProxyUpdate(status.latest.tag.clone()),
            title: "Новая версия Tg proxy".to_owned(),
            message: format!(
                "Доступен TgWsProxy {}. Можно обновить во вкладке «Обновления».",
                status.latest.tag
            ),
        });
    }

    fn dismiss_startup_notice(&mut self, index: usize) {
        if index >= self.startup_notices.len() {
            return;
        }

        let notice = self.startup_notices.remove(index);
        match notice.kind {
            StartupNoticeKind::AppUpdated => {}
            StartupNoticeKind::BundleUpdate(tag) => {
                self.app_config.dismissed_bundle_release_tag = Some(tag);
                let _ = save_app_config(&self.app_config);
            }
            StartupNoticeKind::TelegramProxyUpdate(tag) => {
                self.app_config.dismissed_tg_proxy_release_tag = Some(tag);
                let _ = save_app_config(&self.app_config);
            }
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
                                BundleAction::StartProfile { profile, .. } => Some(profile.label()),
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
                        TelegramProxyTask::Check(reason) => TelegramProxyTask::Check(*reason),
                        TelegramProxyTask::Install(release) => {
                            TelegramProxyTask::Install(release.clone())
                        }
                    };
                    self.tg_proxy_task = None;

                    match result {
                        Ok(TelegramProxyTaskResult::Checked(status)) => {
                            if matches!(task, TelegramProxyTask::Check(UpdateCheckReason::Startup))
                            {
                                self.add_tg_proxy_update_notice(&status);
                            }
                            self.tg_proxy_status = Some(status);
                            self.tg_proxy_check_error = None;
                        }
                        Ok(TelegramProxyTaskResult::Installed(message)) => {
                            self.last_message = message;
                            self.app_config.dismissed_tg_proxy_release_tag = None;
                            let _ = save_app_config(&self.app_config);
                            self.start_tg_proxy_check();
                        }
                        Err(error) => {
                            let was_check = matches!(task, TelegramProxyTask::Check(_));
                            let message = match task {
                                TelegramProxyTask::Check(_) => {
                                    format!(
                                        "Не удалось проверить обновление Telegram proxy: {error}"
                                    )
                                }
                                TelegramProxyTask::Install(_) => {
                                    format!("Не удалось обновить Telegram proxy: {error}")
                                }
                            };
                            if was_check {
                                self.tg_proxy_check_error = Some(error.to_string());
                            }
                            self.last_message = message;
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

    fn poll_bundle_task_completion(&mut self) {
        if let Some(pending) = &self.bundle_task {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let task = match &pending.task {
                        BundleUpdateTask::Check(reason) => BundleUpdateTask::Check(*reason),
                        BundleUpdateTask::Install(release) => {
                            BundleUpdateTask::Install(release.clone())
                        }
                    };
                    self.bundle_task = None;

                    match result {
                        Ok(BundleUpdateTaskResult::Checked(status)) => {
                            if matches!(task, BundleUpdateTask::Check(UpdateCheckReason::Startup)) {
                                self.add_bundle_update_notice(&status);
                            }
                            self.bundle_status = Some(status);
                            self.bundle_check_error = None;
                        }
                        Ok(BundleUpdateTaskResult::Installed(message)) => {
                            self.last_message = message;
                            self.bundle_version = detect_bundle_version(&self.bundle_path);
                            self.app_config.dismissed_bundle_release_tag = None;
                            let _ = save_app_config(&self.app_config);
                            self.start_bundle_update_check();
                            self.start_tg_proxy_check();
                            self.status_monitor.request_refresh();
                        }
                        Err(error) => {
                            let was_check = matches!(task, BundleUpdateTask::Check(_));
                            let message = match task {
                                BundleUpdateTask::Check(_) => {
                                    format!("Не удалось проверить обновление bundle: {error}")
                                }
                                BundleUpdateTask::Install(_) => {
                                    format!("Не удалось обновить bundle: {error}")
                                }
                            };
                            if was_check {
                                self.bundle_check_error = Some(error.to_string());
                            }
                            self.last_message = message;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.bundle_task = None;
                    self.last_message =
                        "Фоновая задача обновления bundle была прервана.".to_owned();
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
            BundleAction::RefreshIpset => {
                format!("{action_message}. Файл lists\\ipset-all.txt заменён из bundled backup.")
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
            self.last_message = format!("Не удалось сохранить режим Telegram proxy: {error}");
            return;
        }

        self.last_message = match mode {
            TelegramProxyMode::Standard => {
                "Telegram proxy переведён в обычный режим: DC2/DC4/DC203.".to_owned()
            }
            TelegramProxyMode::CfMedia => {
                "Telegram proxy переведён в CF media режим. Укажите домен Cloudflare.".to_owned()
            }
        };
    }

    fn set_telegram_cf_domain(&mut self, domain: String) {
        if self.app_config.telegram_cf_domain == domain {
            return;
        }

        self.app_config.telegram_cf_domain = domain;
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message = format!("Не удалось сохранить домен для Telegram proxy: {error}");
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

    fn bundle_update_controls_enabled(&self) -> bool {
        self.pending_action.is_none() && self.bundle_task.is_none() && !self.runtime_is_active()
    }

    fn runtime_toggle_enabled(&self) -> bool {
        self.pending_action.is_none() && self.bundle_task.is_none()
    }

    fn runtime_toggle_label(&self) -> String {
        if self.runtime_is_active() {
            "Выключить".to_owned()
        } else {
            format!("Включить {}", self.app_config.main_profile.label())
        }
    }

    fn toggle_runtime(&mut self) {
        if self.runtime_is_active() {
            self.start_action(BundleAction::StopAll);
        } else {
            self.start_selected_profile();
        }
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
                Self::status_row(ui, "Основной профиль", self.app_config.main_profile.label());
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
                    let start_caption = if self.app_config.use_builtin_whitelist {
                        if self.app_config.launch_telegram_proxy_for_profiles {
                            "Запускает выбранный основной профиль, Telegram WS proxy и добавляет встроенный список исключений."
                        } else {
                            "Запускает выбранный основной профиль и добавляет встроенный список исключений."
                        }
                    } else {
                        if self.app_config.launch_telegram_proxy_for_profiles {
                            "Запускает выбранный основной профиль и Telegram WS proxy."
                        } else {
                            "Запускает выбранный основной профиль без Telegram WS proxy."
                        }
                    };

                    if ui
                        .add_enabled(
                            start_enabled,
                            egui::Button::new(
                                RichText::new(format!(
                                    "Запустить {}",
                                    self.app_config.main_profile.label()
                                ))
                                .strong(),
                            )
                                .min_size(Vec2::new(button_width, 42.0)),
                        )
                        .on_hover_text(start_caption)
                        .clicked()
                    {
                        self.start_selected_profile();
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
        let can_select = self.pending_action.is_none();
        let can_start = self.profile_actions_enabled();
        let column_count = if ui.available_width() > 760.0 { 3 } else { 2 };

        Self::card(
            ui,
            "Профили",
            "Выберите пресет, который будет считаться основным.",
            |ui| {
                if let Some(message) = self.runtime_lock_message() {
                    ui.label(RichText::new(message).color(Color32::from_rgb(198, 120, 0)));
                    ui.add_space(8.0);
                }

                let profiles = ZapretProfile::ALL;
                ui.columns(column_count, |columns| {
                    for (index, profile) in profiles.iter().enumerate() {
                        let column = &mut columns[index % column_count];
                        column.horizontal(|ui| {
                            let selected = self.app_config.main_profile == *profile;
                            let response = ui.add_enabled(
                                can_select,
                                egui::RadioButton::new(selected, profile.label()),
                            );
                            if response.clicked() {
                                self.set_main_profile(*profile);
                            }

                            if selected {
                                Self::state_badge(ui, "основной", Color32::from_rgb(0, 132, 80));
                            }
                        });
                        column.add_space(6.0);
                    }
                });

                ui.add_space(8.0);
                if Self::primary_button(
                    ui,
                    "Запустить выбранный профиль",
                    &self.profile_launch_caption(self.app_config.main_profile.label()),
                    can_start,
                )
                .clicked()
                {
                    self.start_selected_profile();
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
                let mut startup_notifications_enabled =
                    self.app_config.startup_notifications_enabled;
                let response = ui.checkbox(
                    &mut startup_notifications_enabled,
                    "Показывать мягкие уведомления при запуске",
                );
                if response.changed() {
                    self.set_startup_notifications_enabled(startup_notifications_enabled);
                }
                ui.label(
                    RichText::new(
                        "Показывает только стартовые карточки про обновление приложения, Zapret bundle и Tg proxy.",
                    )
                    .color(Color32::from_gray(120)),
                );
            },
        );

        ui.add_space(12.0);
        self.draw_project_links(ui);
    }

    fn draw_project_links(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Ссылки",
            "Оригинальные проекты и репозиторий Zapret Hub.",
            |ui| {
                ui.hyperlink_to("Zapret Hub", "https://github.com/WETQV/zapret-hub-rs");
                ui.hyperlink_to("bol-van/zapret", "https://github.com/bol-van/zapret");
                ui.hyperlink_to(
                    "Flowseal/zapret-discord-youtube",
                    "https://github.com/Flowseal/zapret-discord-youtube",
                );
                ui.hyperlink_to(
                    "Flowseal/tg-ws-proxy",
                    "https://github.com/Flowseal/tg-ws-proxy",
                );
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

    fn draw_startup_notices(&mut self, ui: &mut egui::Ui) {
        if self.startup_notices.is_empty() {
            return;
        }

        let mut action = None;
        Self::card(
            ui,
            "Уведомления",
            "Показываются только после запуска приложения.",
            |ui| {
                for (index, notice) in self.startup_notices.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }

                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&notice.title).strong());
                            ui.label(RichText::new(&notice.message).color(Color32::from_gray(120)));
                        });
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Закрыть").clicked() {
                                action = Some(StartupNoticeAction::Close(index));
                            }
                            if !matches!(notice.kind, StartupNoticeKind::AppUpdated)
                                && ui.button("Не показывать этот релиз").clicked()
                            {
                                action = Some(StartupNoticeAction::Dismiss(index));
                            }
                        });
                    });
                }

                ui.add_space(10.0);
                if ui.button("Отключить все стартовые уведомления").clicked()
                {
                    action = Some(StartupNoticeAction::DisableAll);
                }
            },
        );

        match action {
            Some(StartupNoticeAction::Close(index)) => {
                if index < self.startup_notices.len() {
                    self.startup_notices.remove(index);
                }
            }
            Some(StartupNoticeAction::Dismiss(index)) => self.dismiss_startup_notice(index),
            Some(StartupNoticeAction::DisableAll) => {
                self.set_startup_notifications_enabled(false);
            }
            None => {}
        }
    }

    fn draw_telegram_settings(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Telegram proxy",
            "Отдельные настройки Telegram WS proxy и CF media.",
            |ui| {
                let enabled = self.profile_actions_enabled();

                let mut launch_proxy = self.app_config.launch_telegram_proxy_for_profiles;
                let proxy_checkbox = ui.add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut launch_proxy, "Запускать вместе с профилями"),
                );
                if proxy_checkbox.changed() {
                    self.app_config.launch_telegram_proxy_for_profiles = launch_proxy;
                    if let Err(error) = save_app_config(&self.app_config) {
                        self.last_message =
                            format!("Не удалось сохранить режим запуска Telegram proxy: {error}");
                    }
                }

                ui.add_space(8.0);
                ui.add_enabled_ui(enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Режим");
                        let mut selected_mode = self.app_config.telegram_proxy_mode.clone();
                        egui::ComboBox::from_id_salt("telegram_proxy_mode_tab")
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
                    });

                    if self.app_config.telegram_proxy_mode == TelegramProxyMode::CfMedia {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Cloudflare домен");
                            let mut domain = self.app_config.telegram_cf_domain.clone();
                            let response = ui.add_sized(
                                [280.0, 30.0],
                                egui::TextEdit::singleline(&mut domain)
                                    .hint_text("your-domain.example"),
                            );
                            if response.changed() {
                                self.set_telegram_cf_domain(domain.trim().to_owned());
                            }

                            if ui
                                .add_enabled(
                                    !self.app_config.telegram_cf_domain.trim().is_empty(),
                                    egui::Button::new("Очистить"),
                                )
                                .clicked()
                            {
                                self.reset_telegram_cf_domain_to_default();
                            }
                        });
                    }
                });

                if !enabled {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Остановите текущий профиль, чтобы менять режим Telegram proxy.",
                        )
                        .color(Color32::from_gray(120)),
                    );
                }
            },
        );
    }

    fn draw_updates(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Bundle",
            "Проверка и установка свежего upstream bundle без нового релиза Hub.",
            |ui| {
                if let Some(status) = self.bundle_status.clone() {
                    Self::status_row(
                        ui,
                        "Текущая версия",
                        status
                            .installed_version
                            .as_deref()
                            .unwrap_or("неизвестно")
                            .to_owned(),
                    );
                    Self::status_row(ui, "Последний релиз", status.latest.tag.as_str());
                    Self::status_row(ui, "Страница релиза", status.latest.release_url.as_str());
                    Self::status_row(ui, "Asset", status.latest.asset_name.as_str());

                    if status.update_available {
                        let dismissed = self.app_config.dismissed_bundle_release_tag.as_deref()
                            == Some(status.latest.tag.as_str());

                        ui.add_space(8.0);
                        if dismissed {
                            ui.label(
                                RichText::new(
                                    "Обновление bundle скрыто до следующего upstream релиза.",
                                )
                                .color(Color32::from_gray(120)),
                            );
                        } else {
                            ui.label(
                                RichText::new("Доступен новый bundle. Установка заменит bundle целиком и сохранит пользовательские списки.")
                                    .color(Color32::from_rgb(198, 120, 0)),
                            );
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let can_update = self.bundle_update_controls_enabled();
                            if ui
                                .add_enabled(
                                    can_update && !dismissed,
                                    egui::Button::new("Обновить bundle"),
                                )
                                .on_hover_text("Скачает официальный zip Flowseal, подготовит hub scripts и заменит bundle.")
                                .clicked()
                            {
                                self.start_bundle_update();
                            }

                            if ui
                                .add_enabled(
                                    self.bundle_task.is_none() && !dismissed,
                                    egui::Button::new("Позже"),
                                )
                                .clicked()
                            {
                                self.dismiss_bundle_release(&status.latest.tag);
                            }
                        });

                        if !self.bundle_update_controls_enabled() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Остановите текущий профиль, сервис и Telegram proxy перед обновлением bundle.")
                                    .color(Color32::from_gray(120)),
                            );
                        }
                    } else {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Bundle уже актуален.")
                                .color(Color32::from_rgb(0, 132, 80)),
                        );
                    }
                } else if self.bundle_task.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Проверяю обновления bundle.");
                    });
                } else if let Some(error) = self.bundle_check_error.clone() {
                    ui.label(
                        RichText::new("Не удалось проверить обновления bundle.")
                            .color(Color32::from_rgb(198, 120, 0)),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(error).color(Color32::from_gray(120)));
                } else {
                    ui.label(
                        RichText::new("Проверка обновлений bundle ещё не запускалась.")
                            .color(Color32::from_gray(120)),
                    );
                }

                ui.add_space(10.0);
                if ui
                    .add_enabled(
                        self.bundle_task.is_none(),
                        egui::Button::new("Проверить обновления bundle"),
                    )
                    .clicked()
                {
                    self.start_bundle_update_check();
                }
            },
        );

        ui.add_space(12.0);
        self.draw_lists_update_card(ui);

        ui.add_space(12.0);
        self.draw_tg_proxy_update_card(ui);
    }

    fn draw_lists_update_card(&mut self, ui: &mut egui::Ui) {
        let enabled = self.service_tools_enabled();

        Self::card(
            ui,
            "Списки",
            "Локальное обслуживание hostlist и ipset внутри текущего bundle.",
            |ui| {
                Self::status_row(
                    ui,
                    "ipset",
                    self.bundle_path
                        .join("lists")
                        .join("ipset-all.txt")
                        .display()
                        .to_string(),
                );
                Self::status_row(
                    ui,
                    "Источник",
                    self.bundle_path
                        .join("lists")
                        .join("ipset-all.txt.backup")
                        .display()
                        .to_string(),
                );

                ui.add_space(10.0);
                if ui
                    .add_enabled(enabled, egui::Button::new("Обновить ipset"))
                    .on_hover_text(
                        "Восстановит lists\\ipset-all.txt из bundled ipset-all.txt.backup.",
                    )
                    .clicked()
                {
                    self.start_action(BundleAction::RefreshIpset);
                }

                if !enabled {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Остановите текущий профиль или сервис перед обновлением ipset.",
                        )
                        .color(Color32::from_gray(120)),
                    );
                }
            },
        );
    }

    fn draw_tg_proxy_update_card(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Telegram WS proxy",
            "Независимое обновление TgWsProxy_windows.exe внутри текущего bundle.",
            |ui| {
                let tg_proxy_status = self.tg_proxy_status.clone();
                if let Some(status) = tg_proxy_status {
                    let installed = status.installed_tag.as_deref().unwrap_or("неизвестно");
                    Self::status_row(ui, "Telegram proxy в bundle", installed);
                    Self::status_row(ui, "Последний релиз", status.latest.tag.as_str());

                    if status.update_available {
                        let dismissed = self.app_config.dismissed_tg_proxy_release_tag.as_deref()
                            == Some(status.latest.tag.as_str());

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let can_update = self.tg_proxy_controls_enabled();
                            if ui
                                .add_enabled(
                                    can_update && !dismissed,
                                    egui::Button::new("Обновить Telegram proxy"),
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
                } else if let Some(error) = self.tg_proxy_check_error.clone() {
                    ui.label(
                        RichText::new("Не удалось проверить обновления Telegram proxy.")
                            .color(Color32::from_rgb(198, 120, 0)),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(error).color(Color32::from_gray(120)));
                }

                ui.add_space(10.0);
                if ui
                    .add_enabled(
                        self.tg_proxy_task.is_none(),
                        egui::Button::new("Проверить Telegram proxy"),
                    )
                    .clicked()
                {
                    self.start_tg_proxy_check();
                }
            },
        );
    }

    fn draw_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for tab in AppTab::ALL {
                let selected = self.selected_tab() == tab;
                let response = ui.selectable_label(selected, tab.label());
                if response.clicked() {
                    self.set_selected_tab(tab);
                }
            }
        });
    }

    fn draw_current_tab(&mut self, ui: &mut egui::Ui) {
        match self.selected_tab() {
            AppTab::Main => {
                self.draw_startup_notices(ui);
                if !self.startup_notices.is_empty() {
                    ui.add_space(12.0);
                }
                self.draw_overview(ui);
                ui.add_space(12.0);
                self.draw_primary_actions(ui);
                ui.add_space(12.0);
                self.draw_status_log(ui);
            }
            AppTab::Profiles => self.draw_profiles(ui),
            AppTab::Telegram => {
                self.draw_telegram_settings(ui);
                ui.add_space(12.0);
                self.draw_tg_proxy_update_card(ui);
            }
            AppTab::Updates => self.draw_updates(ui),
            AppTab::Settings => {
                self.draw_service_tools(ui);
                ui.add_space(12.0);
                self.draw_installation_info(ui);
            }
        }
    }
}

impl BundleAction {
    fn in_progress_label(self) -> String {
        match self {
            BundleAction::StartProfile {
                profile,
                use_builtin_whitelist,
            } => {
                if use_builtin_whitelist {
                    format!(
                        "Запускаю основной профиль {} со встроенным списком исключений.",
                        profile.label()
                    )
                } else {
                    format!("Запускаю основной профиль {}.", profile.label())
                }
            }
            BundleAction::StopAll => "Останавливаю bypass, proxy и связанные процессы.".to_owned(),
            BundleAction::RefreshIpset => "Обновляю ipset из bundled backup.".to_owned(),
            BundleAction::InstallService => "Открываю установку сервиса.".to_owned(),
            BundleAction::RemoveService => "Удаляю сервис и останавливаю остатки.".to_owned(),
            BundleAction::OpenServiceManager => "Открываю service.bat.".to_owned(),
        }
    }
}

impl eframe::App for ZapretHubApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_action_completion();
        self.poll_tg_proxy_task_completion();
        self.poll_bundle_task_completion();
        self.poll_status_updates();

        if self.launch_mode.is_autostart() && !self.startup_view_applied {
            self.startup_view_applied = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        if self.close_after_stop && self.pending_action.is_none() && !self.runtime_is_active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.pending_action.is_some()
            || self.tg_proxy_task.is_some()
            || self.bundle_task.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.pending_action.is_some() || self.bundle_task.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.last_message = if self.bundle_task.is_some() {
                    "Сначала дождитесь завершения обновления bundle, потом закрывайте окно."
                        .to_owned()
                } else {
                    "Сначала дождитесь завершения текущей команды, потом закрывайте окно."
                        .to_owned()
                };
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
                        let toggle_hover = if self.runtime_is_active() {
                            "Остановить winws, Telegram proxy, сервис и связанные процессы."
                        } else {
                            "Запустить выбранный основной профиль."
                        };
                        if ui
                            .add_enabled(
                                self.runtime_toggle_enabled(),
                                egui::Button::new(self.runtime_toggle_label()),
                            )
                            .on_hover_text(toggle_hover)
                            .clicked()
                        {
                            self.toggle_runtime();
                        }
                    });
                });
                ui.add_space(10.0);
                self.draw_tab_bar(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.draw_current_tab(ui);
                });
        });
    }
}

impl StatusMonitor {
    fn new(ctx: egui::Context, bundle_path: PathBuf) -> Self {
        let (status_sender, receiver) = mpsc::channel();
        let (command_sender, command_receiver) = mpsc::channel();

        thread::spawn(move || {
            loop {
                let status = refresh_runtime_status(&bundle_path);
                let _ = status_sender.send(status);
                ctx.request_repaint();

                match command_receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(StatusCommand::Refresh) => continue,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
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
    if value { "да" } else { "нет" }
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
