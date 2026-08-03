use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    BundleRelease, BundleUpdateOutcome, BundleUpdateStatus, PreparedBundleUpdate,
    apply_prepared_update as apply_bundle_update, check_for_update as check_bundle_update,
    discard_prepared_update as discard_bundle_update, prepare_update as prepare_bundle_update,
};
use crate::core::config::{AppConfig, TelegramProxyMode, load_app_config, save_app_config};
use crate::core::paths::{ResolvedPaths, is_valid_bundle_dir, resolve_paths};
use crate::core::profile_test::{
    ProfileTestEvent, ProfileTestMode, ProfileTestReport, ProfileTestRequest,
    preflight as profile_test_preflight, start as start_profile_test,
};
use crate::core::status::{RuntimeStatus, ServiceState, refresh_runtime_status};
use crate::core::tg_proxy_update::{
    TelegramProxyRelease, TelegramProxyUpdateStatus, check_for_update as check_tg_proxy_update,
    install_update as install_tg_proxy_update,
};
use crate::zapret::bundle::{
    BundleAction, BundleProfile, TelegramProxyLaunchConfig, discover_profiles,
    find_profile_by_script, run_action,
};
use crate::zapret::fakes::{FakeCatalog, FakeTarget, apply_selection, read_catalog};

pub(crate) struct ZapretHubApp {
    bundle_path: PathBuf,
    bundle_source: String,
    bundle_version: Option<String>,
    profiles: Vec<BundleProfile>,
    status: RuntimeStatus,
    last_profile: Option<String>,
    last_message: String,
    startup_notices: Vec<StartupNotice>,
    pending_action: Option<PendingAction>,
    bundle_task: Option<PendingBundleUpdateTask>,
    bundle_status: Option<BundleUpdateStatus>,
    prepared_bundle_update: Option<PreparedBundleUpdate>,
    bundle_check_error: Option<String>,
    bundle_test: BundleTestState,
    fake_catalog: Option<FakeCatalog>,
    fake_catalog_error: Option<String>,
    fake_discord_selection: Option<String>,
    fake_game_selection: Option<String>,
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
    Prepare(BundleRelease),
    Apply(PreparedBundleUpdate),
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
    Prepared(PreparedBundleUpdate),
    Applied(BundleUpdateOutcome),
}

enum TelegramProxyTaskResult {
    Checked(TelegramProxyUpdateStatus),
    Installed(String),
}

struct BundleTestState {
    mode: ProfileTestMode,
    advanced: bool,
    selected_scripts: Vec<String>,
    receiver: Option<Receiver<ProfileTestEvent>>,
    cancellation: Option<Arc<AtomicBool>>,
    current: usize,
    total: usize,
    current_label: Option<String>,
    report: Option<ProfileTestReport>,
    error: Option<String>,
}

impl Default for BundleTestState {
    fn default() -> Self {
        Self {
            mode: ProfileTestMode::Standard,
            advanced: false,
            selected_scripts: Vec::new(),
            receiver: None,
            cancellation: None,
            current: 0,
            total: 0,
            current_label: None,
            report: None,
            error: None,
        }
    }
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

fn reconcile_main_profile_script(app_config: &mut AppConfig, profiles: &[BundleProfile]) -> bool {
    let selected = app_config.main_profile_script_or_legacy().to_owned();
    if profiles.iter().any(|profile| {
        profile
            .script_name()
            .eq_ignore_ascii_case(selected.as_str())
    }) {
        if app_config.main_profile_script.is_some() {
            return false;
        }
        app_config.main_profile_script = Some(selected);
        return true;
    }

    let legacy_script = app_config.main_profile.script_name();
    let fallback = profiles
        .iter()
        .find(|profile| profile.script_name().eq_ignore_ascii_case(legacy_script))
        .or_else(|| profiles.first())
        .map(|profile| profile.script_name().to_owned());

    if app_config.main_profile_script == fallback {
        return false;
    }

    app_config.main_profile_script = fallback;
    true
}

fn warnings_text(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!(" Предупреждение: {}", warnings.join("; "))
    }
}

fn bundle_update_outcome_message(outcome: &BundleUpdateOutcome) -> String {
    format!(
        "{}{}",
        outcome.message,
        warnings_text(outcome.warnings.as_slice())
    )
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
        let profiles = match discover_profiles(&bundle_dir) {
            Ok(profiles) => profiles,
            Err(error) => {
                last_message = format!("Не удалось прочитать профили bundle: {error}");
                Vec::new()
            }
        };
        let (fake_catalog, fake_catalog_error) = match read_catalog(&bundle_dir) {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let fake_discord_selection = fake_catalog
            .as_ref()
            .and_then(|catalog| catalog.discord_current.clone());
        let fake_game_selection = fake_catalog
            .as_ref()
            .and_then(|catalog| catalog.game_current.clone());
        let profile_selection_changed =
            reconcile_main_profile_script(&mut app_config, profiles.as_slice());
        if profile_selection_changed && let Err(error) = save_app_config(&app_config) {
            last_message = format!("Основной профиль выбран, но настройки не сохранены: {error}");
        }
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
            profiles,
            status,
            last_profile: None,
            last_message,
            startup_notices,
            pending_action: None,
            bundle_task: None,
            bundle_status: None,
            prepared_bundle_update: None,
            bundle_check_error: None,
            bundle_test: BundleTestState::default(),
            fake_catalog,
            fake_catalog_error,
            fake_discord_selection,
            fake_game_selection,
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

    fn selected_profile(&self) -> Option<BundleProfile> {
        let selected_script = self.app_config.main_profile_script_or_legacy();
        self.profiles
            .iter()
            .find(|profile| profile.script_name().eq_ignore_ascii_case(selected_script))
            .or_else(|| self.profiles.first())
            .cloned()
    }

    fn selected_profile_label(&self) -> String {
        self.selected_profile()
            .map(|profile| profile.label().to_owned())
            .unwrap_or_else(|| "не выбран".to_owned())
    }

    fn refresh_profiles(&mut self) {
        match discover_profiles(&self.bundle_path) {
            Ok(profiles) => {
                self.profiles = profiles;
                let changed =
                    reconcile_main_profile_script(&mut self.app_config, self.profiles.as_slice());
                if changed && let Err(error) = save_app_config(&self.app_config) {
                    self.last_message =
                        format!("Профили обновлены, но настройки не сохранены: {error}");
                }
            }
            Err(error) => {
                self.last_message = format!("Не удалось обновить список профилей: {error}");
            }
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

    fn set_main_profile(&mut self, profile: &BundleProfile) {
        if self.app_config.main_profile_script.as_deref() == Some(profile.script_name()) {
            return;
        }

        self.app_config.main_profile_script = Some(profile.script_name().to_owned());
        if let Err(error) = save_app_config(&self.app_config) {
            self.last_message = format!("Не удалось сохранить основной профиль: {error}");
        } else {
            self.last_message = format!("Основной профиль: {}.", profile.label());
        }
    }

    fn start_selected_profile(&mut self) {
        let Some(profile) = self.selected_profile() else {
            self.last_message = "В текущем bundle не найдены general*.bat профили.".to_owned();
            return;
        };

        self.start_action(BundleAction::StartProfile {
            profile,
            use_builtin_whitelist: self.app_config.use_builtin_whitelist,
        });
    }

    fn start_bundle_test_run(&mut self) {
        if let Some(blocker) = self.bundle_test_blocker() {
            self.last_message = blocker;
            return;
        }
        let profiles = if self.bundle_test.advanced {
            self.profiles
                .iter()
                .filter(|profile| {
                    self.bundle_test
                        .selected_scripts
                        .iter()
                        .any(|script| script.eq_ignore_ascii_case(profile.script_name()))
                })
                .cloned()
                .collect()
        } else {
            self.profiles.clone()
        };
        let request = ProfileTestRequest {
            mode: self.bundle_test.mode,
            profiles,
        };
        if let Err(error) = profile_test_preflight(&self.bundle_path, &request) {
            self.bundle_test.error = Some(error.to_string());
            self.last_message = format!("Тест не запущен: {error}");
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancellation = Arc::new(AtomicBool::new(false));
        start_profile_test(
            self.bundle_path.clone(),
            request,
            cancellation.clone(),
            sender,
        );
        self.bundle_test.receiver = Some(receiver);
        self.bundle_test.cancellation = Some(cancellation);
        self.bundle_test.current = 0;
        self.bundle_test.total = 0;
        self.bundle_test.current_label = None;
        self.bundle_test.report = None;
        self.bundle_test.error = None;
        self.last_message = "Ищу лучший профиль. Окно PowerShell не откроется.".to_owned();
    }

    fn poll_bundle_test_result(&mut self) {
        let mut completed = false;
        if let Some(receiver) = &self.bundle_test.receiver {
            while let Ok(event) = receiver.try_recv() {
                match event {
                    ProfileTestEvent::Started { total } => self.bundle_test.total = total,
                    ProfileTestEvent::ProfileStarted {
                        current,
                        total,
                        label,
                    } => {
                        self.bundle_test.current = current;
                        self.bundle_test.total = total;
                        self.bundle_test.current_label = Some(label);
                    }
                    ProfileTestEvent::CheckStarted { label } => {
                        self.bundle_test.current_label = Some(format!("Проверка: {label}"));
                    }
                    ProfileTestEvent::ProfileFinished(row) => {
                        self.bundle_test.current_label =
                            Some(format!("{}: проверки завершены", row.label));
                    }
                    ProfileTestEvent::Finished(report) => {
                        let best = report
                            .best_script
                            .clone()
                            .unwrap_or_else(|| "не определён".to_owned());
                        self.bundle_test.report = Some(report);
                        self.last_message = format!("Тест завершён. Лучший профиль: {best}.");
                        completed = true;
                    }
                    ProfileTestEvent::Cancelled => {
                        self.last_message =
                            "Тест отменён, исходное состояние восстановлено.".to_owned();
                        completed = true;
                    }
                    ProfileTestEvent::Failed(error) => {
                        self.bundle_test.error = Some(error.clone());
                        self.last_message = format!("Тест завершился с ошибкой: {error}");
                        completed = true;
                    }
                }
            }
        }
        if completed {
            self.bundle_test.receiver = None;
            self.bundle_test.cancellation = None;
            self.bundle_test.current_label = None;
        }
    }

    fn cancel_bundle_test(&mut self) {
        if let Some(cancellation) = &self.bundle_test.cancellation {
            cancellation.store(true, Ordering::Relaxed);
            self.last_message = "Отменяю тест и восстанавливаю исходное состояние.".to_owned();
        }
    }

    fn start_action(&mut self, action: BundleAction) {
        if self.pending_action.is_some() {
            self.last_message = "Дождитесь завершения текущей команды.".to_owned();
            return;
        }

        let telegram_proxy = self.telegram_proxy_launch_config();
        if matches!(&action, BundleAction::StartProfile { .. })
            && let Err(error) = self.validate_telegram_proxy_config(&telegram_proxy)
        {
            self.last_message = format!("Telegram proxy не запущен: {error}");
            return;
        }

        let in_progress_label = action.in_progress_label();
        let pending_action = action.clone();
        let bundle_path = self.bundle_path.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = run_action(&bundle_path, action, &telegram_proxy);
            let _ = sender.send(result);
        });

        self.last_message = in_progress_label;
        self.pending_action = Some(PendingAction {
            action: pending_action,
            receiver,
        });
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
        if self.bundle_task.is_some() {
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
        let prepare_release = release.clone();
        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = prepare_bundle_update(&bundle_path, &prepare_release)
                .map(BundleUpdateTaskResult::Prepared);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
        });

        self.last_message = format!("Скачиваю и подготавливаю bundle {}.", release.tag);
        self.bundle_task = Some(PendingBundleUpdateTask {
            task: BundleUpdateTask::Prepare(release),
            receiver,
        });
    }

    fn start_apply_prepared_bundle_update(&mut self) {
        if self.bundle_task.is_some() || self.runtime_is_active() {
            return;
        }

        let Some(prepared) = self.prepared_bundle_update.clone() else {
            self.last_message = "Сначала скачайте и подготовьте bundle.".to_owned();
            return;
        };

        let apply_prepared = prepared.clone();
        let bundle_path = self.bundle_path.clone();
        let repaint_ctx = self.repaint_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = apply_bundle_update(&bundle_path, &apply_prepared)
                .map(BundleUpdateTaskResult::Applied);
            let _ = sender.send(result);
            repaint_ctx.request_repaint();
        });

        self.last_message = format!("Применяю bundle {}.", prepared.release.tag);
        self.bundle_task = Some(PendingBundleUpdateTask {
            task: BundleUpdateTask::Apply(prepared),
            receiver,
        });
    }

    fn discard_prepared_bundle_update(&mut self) {
        let Some(prepared) = self.prepared_bundle_update.take() else {
            return;
        };

        match discard_bundle_update(&prepared) {
            Ok(()) => {
                self.last_message =
                    format!("Подготовленный bundle {} удалён.", prepared.release.tag);
            }
            Err(error) => {
                self.last_message = format!("Не удалось удалить подготовленный bundle: {error}");
            }
        }
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
                    let action = pending.action.clone();
                    self.pending_action = None;

                    match result {
                        Ok(message) => {
                            let stopped_runtime = matches!(
                                action,
                                BundleAction::StopAll | BundleAction::RemoveService
                            );
                            self.last_profile = match action {
                                BundleAction::StartProfile { ref profile, .. } => {
                                    Some(profile.label().to_owned())
                                }
                                BundleAction::StopAll | BundleAction::RemoveService => None,
                                _ => self.last_profile.clone(),
                            };
                            self.last_message = self.describe_action_result(action, message);
                            if stopped_runtime {
                                self.status = refresh_runtime_status(&self.bundle_path);
                            }
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
                        BundleUpdateTask::Prepare(release) => {
                            BundleUpdateTask::Prepare(release.clone())
                        }
                        BundleUpdateTask::Apply(prepared) => {
                            BundleUpdateTask::Apply(prepared.clone())
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
                        Ok(BundleUpdateTaskResult::Prepared(prepared)) => {
                            let warning_text = warnings_text(prepared.warnings.as_slice());
                            self.last_message = format!(
                                "Bundle {} скачан и готов к установке.{warning_text}",
                                prepared.release.tag
                            );
                            self.prepared_bundle_update = Some(prepared);
                        }
                        Ok(BundleUpdateTaskResult::Applied(outcome)) => {
                            self.last_message = bundle_update_outcome_message(&outcome);
                            self.prepared_bundle_update = None;
                            self.bundle_version = detect_bundle_version(&self.bundle_path);
                            self.refresh_profiles();
                            self.refresh_fake_catalog();
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
                                BundleUpdateTask::Prepare(_) => {
                                    format!("Не удалось скачать и подготовить bundle: {error}")
                                }
                                BundleUpdateTask::Apply(_) => {
                                    format!("Не удалось применить bundle: {error}")
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

    fn bundle_test_waiting(&self) -> bool {
        self.bundle_test.receiver.is_some()
    }

    fn profile_actions_enabled(&self) -> bool {
        self.pending_action.is_none() && !self.runtime_is_active() && !self.bundle_test_waiting()
    }

    fn stop_action_enabled(&self) -> bool {
        self.pending_action.is_none() && self.runtime_is_active()
    }

    fn service_tools_enabled(&self) -> bool {
        self.pending_action.is_none() && !self.runtime_is_active() && !self.bundle_test_waiting()
    }

    fn runtime_lock_message(&self) -> Option<&'static str> {
        if self.pending_action.is_some() {
            Some("Дождитесь завершения текущей команды.")
        } else if self.bundle_test_waiting() {
            Some("Дождитесь завершения теста профилей.")
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

    fn service_allows_profile_test(state: ServiceState) -> bool {
        matches!(state, ServiceState::NotInstalled | ServiceState::Stopped)
    }

    fn describe_action_result(&self, action: BundleAction, action_message: String) -> String {
        match action {
            BundleAction::StopAll => {
                format!("{action_message}. Проверил, что runtime больше не активен.")
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
                "Запускает {profile_name}, Telegram WS proxy в режиме {} и VRChat preset.",
                self.telegram_proxy_mode_label(self.app_config.telegram_proxy_mode.clone())
            )
        } else {
            format!("Запускает {profile_name} без Telegram WS proxy, с VRChat preset.")
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

    fn bundle_test_blocker(&self) -> Option<String> {
        if self.pending_action.is_some() || self.bundle_task.is_some() {
            return Some("Дождитесь завершения текущей команды.".to_owned());
        }
        if self.runtime_is_active() {
            return Some("Остановите текущий профиль или сервис перед тестом bundle.".to_owned());
        }
        if !Self::service_allows_profile_test(self.status.service_state) {
            return Some(match self.status.service_state {
                ServiceState::Unknown => {
                    "Не удалось определить состояние службы zapret. Обновите статус и повторите."
                }
                _ => "Остановите службу zapret перед тестом профилей.",
            }
            .to_owned());
        }
        if !self.bundle_path.join("bin").join("winws.exe").is_file() {
            return Some("В текущем bundle нет bin\\winws.exe.".to_owned());
        }

        None
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
        self.pending_action.is_none() && self.bundle_task.is_none()
    }

    fn prepared_bundle_apply_enabled(&self) -> bool {
        self.pending_action.is_none() && self.bundle_task.is_none() && !self.runtime_is_active()
    }

    fn runtime_toggle_enabled(&self) -> bool {
        self.pending_action.is_none() && self.bundle_task.is_none() && !self.bundle_test_waiting()
    }

    fn runtime_toggle_label(&self) -> String {
        if self.runtime_is_active() {
            "Выключить".to_owned()
        } else {
            format!("Включить {}", self.selected_profile_label())
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
                Self::status_row(ui, "Основной профиль", self.selected_profile_label());
                if let Some(profile) = &self.last_profile {
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
                            "Запускает выбранный основной профиль, Telegram WS proxy, VRChat preset и добавляет встроенный список исключений."
                        } else {
                            "Запускает выбранный основной профиль, VRChat preset и добавляет встроенный список исключений."
                        }
                    } else {
                        if self.app_config.launch_telegram_proxy_for_profiles {
                            "Запускает выбранный основной профиль, Telegram WS proxy и VRChat preset."
                        } else {
                            "Запускает выбранный основной профиль без Telegram WS proxy, с VRChat preset."
                        }
                    };

                    if ui
                        .add_enabled(
                            start_enabled,
                            egui::Button::new(
                                RichText::new(format!(
                                    "Запустить {}",
                                    self.selected_profile_label()
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
                        "Telegram Desktop 6.7.2 и новее обычно не требуют автоматического запуска Telegram WS proxy вместе с профилем. VRChat preset применяется автоматически.",
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

                if self.profiles.is_empty() {
                    ui.label(
                        RichText::new("В текущем bundle не найдены general*.bat профили.")
                            .color(Color32::from_rgb(198, 120, 0)),
                    );
                    return;
                }

                let profiles = self.profiles.clone();
                ui.columns(column_count, |columns| {
                    for (index, profile) in profiles.iter().enumerate() {
                        let column = &mut columns[index % column_count];
                        column.horizontal(|ui| {
                            let selected = self
                                .app_config
                                .main_profile_script_or_legacy()
                                .eq_ignore_ascii_case(profile.script_name());
                            let response = ui.add_enabled(
                                can_select,
                                egui::RadioButton::new(selected, profile.label()),
                            );
                            if response.clicked() {
                                self.set_main_profile(profile);
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
                    &self.profile_launch_caption(self.selected_profile_label().as_str()),
                    can_start,
                )
                .clicked()
                {
                    self.start_selected_profile();
                }
            },
        );
    }

    fn draw_bundle_tests(&mut self, ui: &mut egui::Ui) {
        Self::card(
            ui,
            "Поиск лучшего профиля",
            "Проверяет профили скрыто и показывает понятный результат прямо в приложении.",
            |ui| {
                let blocker = self.bundle_test_blocker();
                let waiting = self.bundle_test_waiting();

                if let Some(blocker) = &blocker {
                    ui.add_space(8.0);
                    ui.label(RichText::new(blocker).color(Color32::from_rgb(198, 120, 0)));
                }

                ui.add_space(10.0);
                if !waiting {
                    if Self::primary_button(
                        ui,
                        "Найти лучший профиль",
                        "Проверит все профили обычным HTTP, TLS и ping тестом без окна PowerShell.",
                        blocker.is_none(),
                    )
                    .clicked()
                    {
                        self.bundle_test.advanced = false;
                        self.bundle_test.mode = ProfileTestMode::Standard;
                        self.start_bundle_test_run();
                    }

                    let advanced = ui.checkbox(
                        &mut self.bundle_test.advanced,
                        "Дополнительные настройки поиска",
                    );
                    if advanced.changed() && self.bundle_test.advanced {
                        self.bundle_test.selected_scripts = self
                            .profiles
                            .iter()
                            .map(|profile| profile.script_name().to_owned())
                            .collect();
                    }

                    if self.bundle_test.advanced {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Режим:");
                            ui.selectable_value(
                                &mut self.bundle_test.mode,
                                ProfileTestMode::Standard,
                                ProfileTestMode::Standard.label(),
                            );
                            ui.selectable_value(
                                &mut self.bundle_test.mode,
                                ProfileTestMode::Dpi,
                                ProfileTestMode::Dpi.label(),
                            );
                        });
                        if self.bundle_test.mode == ProfileTestMode::Dpi {
                            ui.label(
                                RichText::new(
                                    "DPI-проверка занимает заметно больше времени и временно очищает ipset; он будет восстановлен после завершения или отмены.",
                                )
                                .color(Color32::from_rgb(198, 120, 0)),
                            );
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Выбрать все").clicked() {
                                self.bundle_test.selected_scripts = self
                                    .profiles
                                    .iter()
                                    .map(|profile| profile.script_name().to_owned())
                                    .collect();
                            }
                            if ui.button("Снять выбор").clicked() {
                                self.bundle_test.selected_scripts.clear();
                            }
                        });
                        for profile in self.profiles.clone() {
                            let mut selected =
                                self.bundle_test.selected_scripts.iter().any(|script| {
                                    script.eq_ignore_ascii_case(profile.script_name())
                                });
                            if ui.checkbox(&mut selected, profile.label()).changed() {
                                if selected {
                                    self.bundle_test
                                        .selected_scripts
                                        .push(profile.script_name().to_owned());
                                } else {
                                    self.bundle_test.selected_scripts.retain(|script| {
                                        !script.eq_ignore_ascii_case(profile.script_name())
                                    });
                                }
                            }
                        }
                        if Self::primary_button(
                            ui,
                            "Запустить выбранные профили",
                            "Проверит отмеченные профили с выбранным набором тестов.",
                            blocker.is_none(),
                        )
                        .clicked()
                        {
                            self.start_bundle_test_run();
                        }
                    }
                }

                if waiting {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let current = self.bundle_test.current;
                        let total = self.bundle_test.total.max(1);
                        ui.label(format!("Профиль {current} из {total}"));
                        if let Some(label) = &self.bundle_test.current_label {
                            ui.label(format!("— {label}"));
                        }
                        if ui.button("Отменить").clicked() {
                            self.cancel_bundle_test();
                        }
                    });
                    let progress =
                        self.bundle_test.current as f32 / self.bundle_test.total.max(1) as f32;
                    ui.add(egui::ProgressBar::new(progress).show_percentage());
                    ui.label(
                        RichText::new("Проверяю HTTP, TLS и ping. Консольные окна не открываются.")
                            .color(Color32::from_gray(120)),
                    );
                }

                if let Some(error) = &self.bundle_test.error {
                    ui.add_space(10.0);
                    ui.label(RichText::new(error).color(Color32::from_rgb(198, 120, 0)));
                }

                if let Some(report) = self.bundle_test.report.clone() {
                    ui.add_space(12.0);
                    Self::status_row(ui, "Результат", report.result_path.display().to_string());
                    let best = report.best_script.as_deref().unwrap_or("не определён");
                    Self::status_row(ui, "Лучший профиль", best);

                    if let Some(best_script) = report.best_script.as_deref() {
                        match find_profile_by_script(&self.bundle_path, best_script) {
                            Ok(Some(profile)) => {
                                if ui
                                    .add_enabled(
                                        self.pending_action.is_none(),
                                        egui::Button::new("Сделать основным"),
                                    )
                                    .on_hover_text("Сохранит лучший профиль как основной.")
                                    .clicked()
                                {
                                    self.set_main_profile(&profile);
                                }
                            }
                            Ok(None) => {
                                ui.label(
                                    RichText::new("Лучший профиль не найден в текущем bundle.")
                                        .color(Color32::from_gray(120)),
                                );
                            }
                            Err(error) => {
                                ui.label(
                                    RichText::new(format!("Не удалось проверить профиль: {error}"))
                                        .color(Color32::from_rgb(198, 120, 0)),
                                );
                            }
                        }
                    }

                    ui.add_space(10.0);
                    egui::Grid::new("profile-test-results")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Профиль");
                            ui.strong(if report.mode == ProfileTestMode::Dpi {
                                "OK"
                            } else {
                                "HTTP/TLS"
                            });
                            ui.strong(if report.mode == ProfileTestMode::Dpi {
                                "FAIL"
                            } else {
                                "Ошибки"
                            });
                            ui.strong(if report.mode == ProfileTestMode::Dpi {
                                "BLOCKED"
                            } else {
                                "Ping"
                            });
                            ui.end_row();
                            for row in &report.rows {
                                let winner =
                                    report.best_script.as_deref() == Some(row.script_name.as_str());
                                ui.label(if winner {
                                    RichText::new(&row.label).strong()
                                } else {
                                    RichText::new(&row.label)
                                });
                                ui.label(row.ok.to_string());
                                ui.label(row.errors.to_string());
                                ui.label(if report.mode == ProfileTestMode::Dpi {
                                    row.blocked.to_string()
                                } else {
                                    format!("{}/{}", row.ping_ok, row.ping_failed)
                                });
                                ui.end_row();
                            }
                        });
                }
            },
        );
    }

    fn refresh_fake_catalog(&mut self) {
        match read_catalog(&self.bundle_path) {
            Ok(catalog) => {
                self.fake_discord_selection = catalog.discord_current.clone();
                self.fake_game_selection = catalog.game_current.clone();
                self.fake_catalog = Some(catalog);
                self.fake_catalog_error = None;
            }
            Err(error) => {
                self.fake_catalog = None;
                self.fake_catalog_error = Some(error.to_string());
            }
        }
    }

    fn draw_fake_files(&mut self, ui: &mut egui::Ui) {
        let enabled = self.pending_action.is_none()
            && self.bundle_task.is_none()
            && self.tg_proxy_task.is_none()
            && !self.runtime_is_active()
            && !self.bundle_test_waiting();
        let mut apply = None;
        Self::card(
            ui,
            "UDP fake-файлы",
            "Выберите отдельные fake-файлы для Discord Voice и GameFilter. Применение доступно только при остановленном runtime.",
            |ui| {
                if let Some(error) = &self.fake_catalog_error {
                    ui.label(RichText::new(error).color(Color32::from_rgb(198, 120, 0)));
                    if ui.button("Обновить список").clicked() {
                        self.refresh_fake_catalog();
                    }
                    return;
                }
                let Some(catalog) = self.fake_catalog.clone() else {
                    ui.label("Каталог fake-файлов пока недоступен.");
                    return;
                };
                if catalog.entries.is_empty() {
                    ui.label(
                        RichText::new("В bin не найдены доступные .bin fake-файлы.")
                            .color(Color32::from_rgb(198, 120, 0)),
                    );
                    return;
                }
                for target in FakeTarget::ALL {
                    let (current, selection, id) = match target {
                        FakeTarget::DiscordUdp => (
                            catalog.discord_current.clone(),
                            &mut self.fake_discord_selection,
                            "discord-udp-fake",
                        ),
                        FakeTarget::GameFilterUdp => (
                            catalog.game_current.clone(),
                            &mut self.fake_game_selection,
                            "game-filter-udp-fake",
                        ),
                    };
                    if selection.is_none() {
                        *selection = current.clone();
                    }
                    ui.horizontal(|ui| {
                        ui.label(target.label());
                        egui::ComboBox::from_id_salt(id)
                            .selected_text(
                                selection
                                    .as_deref()
                                    .unwrap_or("Пользовательский или из старой версии"),
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
                        let changed = selection.as_deref() != current.as_deref();
                        if ui
                            .add_enabled(
                                enabled && changed && selection.is_some(),
                                egui::Button::new("Применить"),
                            )
                            .clicked()
                        {
                            apply = selection.clone().map(|file_name| (target, file_name));
                        }
                    });
                    if current.is_none() {
                        ui.label(
                            RichText::new("Текущий ACTIVE-файл не совпадает с каталогом и не будет заменён сам.")
                                .color(Color32::from_gray(120)),
                        );
                    }
                }
                if !enabled {
                    ui.label(
                        RichText::new("Сначала завершите runtime, поиск профиля или обновление.")
                            .color(Color32::from_gray(120)),
                    );
                }
            },
        );
        if let Some((target, file_name)) = apply {
            match apply_selection(&self.bundle_path, target, &file_name) {
                Ok(()) => {
                    self.refresh_fake_catalog();
                    self.last_message = format!("{} переключён на {file_name}.", target.label());
                }
                Err(error) => {
                    self.last_message = format!("Не удалось применить fake-файл: {error}");
                }
            }
        }
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

                    if let Some(prepared) = self.prepared_bundle_update.clone() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!(
                                "Bundle {} скачан и готов к установке.",
                                prepared.release.tag
                            ))
                            .color(Color32::from_rgb(0, 110, 174)),
                        );
                        for warning in &prepared.warnings {
                            ui.label(RichText::new(warning).color(Color32::from_rgb(198, 120, 0)));
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    self.prepared_bundle_apply_enabled(),
                                    egui::Button::new("Применить bundle"),
                                )
                                .on_hover_text("Заменит текущий bundle подготовленной версией.")
                                .clicked()
                            {
                                self.start_apply_prepared_bundle_update();
                            }

                            if ui
                                .add_enabled(
                                    self.bundle_task.is_none(),
                                    egui::Button::new("Удалить подготовку"),
                                )
                                .on_hover_text("Удалит скачанный staged bundle из временной папки.")
                                .clicked()
                            {
                                self.discard_prepared_bundle_update();
                            }
                        });

                        if !self.prepared_bundle_apply_enabled() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Остановите текущий профиль, сервис и Telegram proxy перед заменой bundle.")
                                    .color(Color32::from_gray(120)),
                            );
                        }
                    } else if status.update_available {
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
                                    egui::Button::new("Скачать bundle"),
                                )
                                .on_hover_text(
                                    "Скачает официальный zip Flowseal и подготовит staged bundle.",
                                )
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
            AppTab::Profiles => {
                self.draw_profiles(ui);
                ui.add_space(12.0);
                self.draw_bundle_tests(ui);
                ui.add_space(12.0);
                self.draw_fake_files(ui);
            }
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
    fn in_progress_label(&self) -> String {
        match self {
            BundleAction::StartProfile {
                profile,
                use_builtin_whitelist,
            } => {
                if *use_builtin_whitelist {
                    format!(
                        "Запускаю основной профиль {} со встроенным списком исключений и VRChat preset.",
                        profile.label()
                    )
                } else {
                    format!(
                        "Запускаю основной профиль {} с VRChat preset.",
                        profile.label()
                    )
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
        self.poll_bundle_test_result();
        self.poll_status_updates();

        if self.launch_mode.is_autostart() && !self.startup_view_applied {
            self.startup_view_applied = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        if self.close_after_stop
            && self.pending_action.is_none()
            && !self.runtime_is_active()
            && !self.bundle_test_waiting()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.pending_action.is_some()
            || self.tg_proxy_task.is_some()
            || self.bundle_task.is_some()
            || self.bundle_test_waiting()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.pending_action.is_some()
                || self.bundle_task.is_some()
                || self.bundle_test_waiting()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.last_message = if self.bundle_task.is_some() {
                    "Сначала дождитесь завершения обновления bundle, потом закрывайте окно."
                        .to_owned()
                } else if self.bundle_test_waiting() {
                    self.close_after_stop = true;
                    self.cancel_bundle_test();
                    "Отменяю тест профилей и восстанавливаю его состояние перед закрытием."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopped_service_does_not_block_profile_test() {
        assert!(ZapretHubApp::service_allows_profile_test(
            ServiceState::Stopped
        ));
        assert!(ZapretHubApp::service_allows_profile_test(
            ServiceState::NotInstalled
        ));
        assert!(!ZapretHubApp::service_allows_profile_test(
            ServiceState::Running
        ));
        assert!(!ZapretHubApp::service_allows_profile_test(
            ServiceState::StopPending
        ));
        assert!(!ZapretHubApp::service_allows_profile_test(
            ServiceState::Unknown
        ));
    }
}
