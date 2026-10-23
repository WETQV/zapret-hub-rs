mod chrome;
#[cfg(debug_assertions)]
mod screenshot;
mod theme;
mod views;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui;

use crate::core::autostart;
use crate::core::build_info::APP_VERSION;
use crate::core::bundle_metadata::detect_bundle_version;
use crate::core::bundle_update::{
    BundleUpdateOutcome, BundleUpdateStatus, PreparedBundleUpdate, apply_prepared_update,
    check_for_update as check_bundle_update, discard_prepared_update, ensure_hub_scripts,
    prepare_update as prepare_bundle_update,
};
use crate::core::config::{AppConfig, TelegramProxyMode, load_app_config, save_app_config};
use crate::core::github::DownloadProgress;
use crate::core::paths::{is_valid_bundle_dir, resolve_paths};
use crate::core::process::try_run_hidden_command;
use crate::core::profile_test::{
    ProfileTestEvent, ProfileTestMode, ProfileTestReport, ProfileTestRequest,
    preflight as profile_test_preflight, start as start_profile_test,
};
use crate::core::status::{RuntimeStatus, ServiceState, ZAPRET_SERVICE, refresh_runtime_status};
use crate::core::tg_proxy_update::{
    TelegramProxyUpdateStatus, check_for_update as check_tg_proxy_update,
    install_update as install_tg_proxy_update,
};
use crate::platform::SingleInstance;
use crate::tray::{self, Tray, TrayCommand};
use crate::zapret::bundle::{
    BundleAction, BundleProfile, TelegramProxyLaunchConfig, discover_profiles,
    restart_telegram_proxy, run_action, stop_runtime, stop_telegram_proxy, telegram_proxy_link,
};
use crate::zapret::fakes::{FakeCatalog, FakeTarget, apply_selection, read_catalog};
use crate::{AppIcon, LaunchMode};

const TOAST_LIFETIME: Duration = Duration::from_secs(5);
const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(9);
const LOG_CAPACITY: usize = 60;

/// Result of a closure run on a worker thread; the UI polls it every frame.
pub(crate) struct Task<T> {
    receiver: Receiver<Result<T>>,
}

impl<T: Send + 'static> Task<T> {
    fn spawn(ctx: &egui::Context, job: impl FnOnce() -> Result<T> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(job());
            ctx.request_repaint();
        });
        Self { receiver }
    }

    fn poll(&self) -> Option<Result<T>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(anyhow!("фоновая задача аварийно завершилась")))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Page {
    Home,
    Profiles,
    Telegram,
    Updates,
    Settings,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Home,
        Self::Profiles,
        Self::Telegram,
        Self::Updates,
        Self::Settings,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Home => "main",
            Self::Profiles => "profiles",
            Self::Telegram => "telegram",
            Self::Updates => "updates",
            Self::Settings => "settings",
        }
    }

    fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|page| page.id() == id)
            .unwrap_or(Self::Home)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Главная",
            Self::Profiles => "Профили",
            Self::Telegram => "Telegram",
            Self::Updates => "Обновления",
            Self::Settings => "Настройки",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Home => "🏠",
            Self::Profiles => "🔧",
            Self::Telegram => "✈",
            Self::Updates => "🔄",
            Self::Settings => "⚙",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogKind {
    Info,
    Success,
    Error,
}

pub(crate) struct LogEntry {
    pub(crate) kind: LogKind,
    pub(crate) text: String,
    pub(crate) at: Instant,
}

struct Toast {
    born: Instant,
    kind: LogKind,
    text: String,
    expires: Instant,
}

struct PendingAction {
    action: BundleAction,
    task: Task<String>,
}

#[derive(Default)]
pub(crate) struct BundleUpdateState {
    pub(crate) status: Option<BundleUpdateStatus>,
    pub(crate) error: Option<String>,
    pub(crate) progress: DownloadProgress,
    checking: Option<Task<BundleUpdateStatus>>,
    downloading: Option<Task<PreparedBundleUpdate>>,
    /// Downloaded and staged, waiting for running commands to finish before the swap.
    ready: Option<PreparedBundleUpdate>,
    applying: Option<Task<(BundleUpdateOutcome, bool)>>,
}

impl BundleUpdateState {
    pub(crate) fn checking(&self) -> bool {
        self.checking.is_some()
    }

    pub(crate) fn installing(&self) -> bool {
        self.downloading.is_some() || self.ready.is_some() || self.applying.is_some()
    }

    pub(crate) fn stage_text(&self) -> &'static str {
        if self.applying.is_some() {
            "Устанавливаю: останавливаю обход и заменяю файлы…"
        } else if self.ready.is_some() {
            "Скачано, жду завершения текущей команды…"
        } else {
            "Скачиваю архив с GitHub…"
        }
    }

    pub(crate) fn update_available(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.update_available)
    }
}

pub(crate) enum TgTask {
    Check(Task<TelegramProxyUpdateStatus>),
    Install(Task<String>),
}

#[derive(Default)]
pub(crate) struct TgUpdateState {
    pub(crate) status: Option<TelegramProxyUpdateStatus>,
    pub(crate) error: Option<String>,
    pub(crate) progress: DownloadProgress,
    pub(crate) task: Option<TgTask>,
}

impl TgUpdateState {
    pub(crate) fn update_available(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.update_available)
    }

    pub(crate) fn installing(&self) -> bool {
        matches!(self.task, Some(TgTask::Install(_)))
    }
}

pub(crate) struct ProfileTestState {
    pub(crate) mode: ProfileTestMode,
    pub(crate) custom_selection: bool,
    pub(crate) selected_scripts: Vec<String>,
    receiver: Option<Receiver<ProfileTestEvent>>,
    cancellation: Option<Arc<AtomicBool>>,
    pub(crate) current: usize,
    pub(crate) total: usize,
    pub(crate) current_label: Option<String>,
    pub(crate) current_check: Option<String>,
    /// Best finished profile so far: (label, successful checks).
    pub(crate) leader: Option<(String, usize)>,
    pub(crate) report: Option<ProfileTestReport>,
    pub(crate) error: Option<String>,
}

impl Default for ProfileTestState {
    fn default() -> Self {
        Self {
            mode: ProfileTestMode::Standard,
            custom_selection: false,
            selected_scripts: Vec::new(),
            receiver: None,
            cancellation: None,
            current: 0,
            total: 0,
            current_label: None,
            current_check: None,
            leader: None,
            report: None,
            error: None,
        }
    }
}

impl ProfileTestState {
    pub(crate) fn running(&self) -> bool {
        self.receiver.is_some()
    }

    pub(crate) fn cancelling(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
pub(crate) struct FakesState {
    pub(crate) catalog: Option<FakeCatalog>,
    pub(crate) error: Option<String>,
    pub(crate) discord_selection: Option<String>,
    pub(crate) game_selection: Option<String>,
    loading: Option<Task<FakeCatalog>>,
}

#[derive(Default)]
pub(crate) struct AutostartState {
    pub(crate) enabled: Option<bool>,
    task: Option<Task<bool>>,
}

impl AutostartState {
    pub(crate) fn busy(&self) -> bool {
        self.task.is_some()
    }
}

struct StatusMonitor {
    receiver: Receiver<RuntimeStatus>,
    refresh: Sender<()>,
}

impl StatusMonitor {
    /// Polls Win32 directly; repaints only when something actually changed. Polls faster for a
    /// few seconds after `request_refresh` so the UI catches start/stop transitions quickly.
    fn new(ctx: egui::Context) -> Self {
        let (status_sender, receiver) = mpsc::channel();
        let (refresh, refresh_receiver) = mpsc::channel::<()>();

        thread::spawn(move || {
            let mut last = None;
            let mut fast_until = Instant::now() + Duration::from_secs(5);
            loop {
                let status = refresh_runtime_status();
                if last != Some(status) {
                    last = Some(status);
                    if status_sender.send(status).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }

                let interval = if Instant::now() < fast_until {
                    Duration::from_millis(400)
                } else {
                    Duration::from_secs(2)
                };
                match refresh_receiver.recv_timeout(interval) {
                    Ok(()) => fast_until = Instant::now() + Duration::from_secs(10),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });

        Self { receiver, refresh }
    }

    fn request_refresh(&self) {
        let _ = self.refresh.send(());
    }
}

pub(crate) struct ZapretHubApp {
    ctx: egui::Context,
    bundle_path: PathBuf,
    bundle_source: &'static str,
    bundle_valid: bool,
    bundle_version: Option<String>,
    profiles: Vec<BundleProfile>,
    config: AppConfig,
    page: Page,

    status: RuntimeStatus,
    status_monitor: StatusMonitor,
    action: Option<PendingAction>,

    bundle_update: BundleUpdateState,
    tg_update: TgUpdateState,
    test: ProfileTestState,
    fakes: FakesState,
    autostart: AutostartState,

    app_updated_notice: bool,
    toasts: Vec<Toast>,
    log: VecDeque<LogEntry>,
    profile_filter: String,

    tray: Option<Tray>,
    tray_receiver: Receiver<TrayCommand>,
    _instance: SingleInstance,
    window_visible: bool,
    tray_hint_shown: bool,
    quit_requested: bool,
    pending_autostart_profile: bool,
    /// Start the main profile as soon as nothing else is running (after an update or a restart).
    restart_when_idle: bool,
    page_changed: Instant,
    tg_link: Option<String>,
    tg_link_checked: Option<Instant>,
    /// What the current bypass was started with, to tell when settings need a restart.
    running_launch: Option<LaunchSnapshot>,
    #[cfg(debug_assertions)]
    screenshot_tour: Option<screenshot::Tour>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchSnapshot {
    script: String,
    whitelist: bool,
    telegram_proxy: bool,
}

impl ZapretHubApp {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        launch_mode: LaunchMode,
        instance: SingleInstance,
        icon: Option<AppIcon>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx);

        let mut startup_log = Vec::new();
        let paths = resolve_paths();
        let mut config = load_app_config().unwrap_or_else(|error| {
            startup_log.push(format!(
                "Настройки не прочитаны, использую значения по умолчанию: {error}"
            ));
            AppConfig::default()
        });

        let (tray_sender, tray_receiver) = mpsc::channel();
        let show_sender = tray_sender.clone();
        let show_ctx = ctx.clone();
        instance.listen_for_show(move || {
            let _ = show_sender.send(TrayCommand::Show);
            show_ctx.request_repaint();
        });

        let tray = icon.as_ref().and_then(|icon| {
            match tray::create(ctx.clone(), tray_sender, &icon.tray_rgba, icon.tray_size) {
                Ok(tray) => Some(tray),
                Err(error) => {
                    if cfg!(windows) {
                        startup_log.push(format!("Иконка в трее недоступна: {error}"));
                    }
                    None
                }
            }
        });

        // Only after a real upgrade, not on the very first launch.
        let app_updated_notice = config.startup_notifications_enabled
            && config
                .last_seen_app_version
                .as_deref()
                .is_some_and(|seen| seen != APP_VERSION);
        if config.last_seen_app_version.as_deref() != Some(APP_VERSION) {
            config.last_seen_app_version = Some(APP_VERSION.to_owned());
            let _ = save_app_config(&config);
        }

        let mut app = Self {
            ctx: ctx.clone(),
            bundle_path: paths.bundle_dir,
            bundle_source: paths.source,
            bundle_valid: false,
            bundle_version: None,
            profiles: Vec::new(),
            page: Page::from_id(&config.selected_tab),
            status: refresh_runtime_status(),
            status_monitor: StatusMonitor::new(ctx.clone()),
            action: None,
            bundle_update: BundleUpdateState::default(),
            tg_update: TgUpdateState::default(),
            test: ProfileTestState::default(),
            fakes: FakesState::default(),
            autostart: AutostartState::default(),
            app_updated_notice,
            toasts: Vec::new(),
            log: VecDeque::new(),
            profile_filter: String::new(),
            window_visible: !launch_mode.is_autostart(),
            tray_hint_shown: false,
            quit_requested: false,
            pending_autostart_profile: launch_mode.is_autostart()
                && config.autostart_launch_profile,
            restart_when_idle: false,
            page_changed: Instant::now(),
            tg_link: None,
            tg_link_checked: None,
            running_launch: None,
            #[cfg(debug_assertions)]
            screenshot_tour: screenshot::Tour::from_env(),
            tray,
            tray_receiver,
            _instance: instance,
            config,
        };

        for message in startup_log {
            app.push_log(LogKind::Error, message);
        }

        app.reload_bundle();
        if !app.bundle_valid {
            app.page = Page::Updates;
        }

        // Hidden start without a tray icon would leave the user with no way back in.
        if !app.window_visible && app.tray.is_none() {
            app.show_window();
        }

        cleanup_stale_update_dirs(&app.bundle_path);
        app.start_autostart_query();
        app.start_bundle_check(false);
        if app.bundle_valid {
            app.start_tg_check();
        }

        app
    }

    // ----- bundle -----------------------------------------------------------------------------

    fn reload_bundle(&mut self) {
        self.bundle_valid = is_valid_bundle_dir(&self.bundle_path);
        if !self.bundle_valid {
            self.bundle_version = None;
            self.profiles.clear();
            return;
        }

        if let Err(error) = ensure_hub_scripts(&self.bundle_path) {
            self.push_log(
                LogKind::Error,
                format!("Не удалось обновить служебные скрипты: {error}"),
            );
        }
        self.bundle_version = detect_bundle_version(&self.bundle_path);
        match discover_profiles(&self.bundle_path) {
            Ok(profiles) => self.profiles = profiles,
            Err(error) => {
                self.profiles.clear();
                self.push_log(
                    LogKind::Error,
                    format!("Не удалось прочитать профили: {error}"),
                );
            }
        }
        self.reconcile_main_profile();
        self.start_fake_catalog_load();
    }

    fn reconcile_main_profile(&mut self) {
        let selected = self.config.main_profile_script_or_legacy().to_owned();
        if self
            .profiles
            .iter()
            .any(|profile| profile.script_name().eq_ignore_ascii_case(&selected))
        {
            if self.config.main_profile_script.is_none() {
                self.config.main_profile_script = Some(selected);
                self.save_config();
            }
            return;
        }

        let fallback = self
            .profiles
            .iter()
            .find(|profile| {
                profile
                    .script_name()
                    .eq_ignore_ascii_case(self.config.main_profile.script_name())
            })
            .or_else(|| self.profiles.first())
            .map(|profile| profile.script_name().to_owned());
        if self.config.main_profile_script != fallback && fallback.is_some() {
            self.config.main_profile_script = fallback;
            self.save_config();
        }
    }

    pub(crate) fn selected_profile(&self) -> Option<&BundleProfile> {
        let selected = self.config.main_profile_script_or_legacy();
        self.profiles
            .iter()
            .find(|profile| profile.script_name().eq_ignore_ascii_case(selected))
            .or_else(|| self.profiles.first())
    }

    pub(crate) fn selected_profile_label(&self) -> String {
        self.selected_profile().map_or_else(
            || "не выбран".to_owned(),
            |profile| profile.label().to_owned(),
        )
    }

    fn set_main_profile(&mut self, script_name: &str) {
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.script_name().eq_ignore_ascii_case(script_name))
            .cloned()
        else {
            return;
        };
        if self.config.main_profile_script.as_deref() == Some(profile.script_name()) {
            return;
        }
        self.config.main_profile_script = Some(profile.script_name().to_owned());
        self.save_config();
        self.notify(
            LogKind::Info,
            format!("Основной профиль: {}.", profile.label()),
        );
    }

    // ----- runtime actions --------------------------------------------------------------------

    pub(crate) fn busy(&self) -> bool {
        self.action.is_some() || self.bundle_update.applying.is_some() || self.test.running()
    }

    pub(crate) fn runtime_active(&self) -> bool {
        self.status.is_active()
    }

    pub(crate) fn can_toggle(&self) -> bool {
        !self.busy() && (self.bundle_valid || self.runtime_active())
    }

    pub(crate) fn pending_action(&self) -> Option<&BundleAction> {
        self.action.as_ref().map(|pending| &pending.action)
    }

    pub(crate) fn toggle_runtime(&mut self) {
        if self.runtime_active() {
            self.start_action(BundleAction::StopAll);
        } else {
            self.start_main_profile();
        }
    }

    fn start_main_profile(&mut self) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.notify(
                LogKind::Error,
                "В bundle не найдено ни одного профиля general*.bat.",
            );
            return;
        };
        self.start_profile(profile);
    }

    /// Stop, then start the main profile again with the current settings.
    pub(crate) fn restart_runtime(&mut self) {
        if self.busy() {
            return;
        }
        self.restart_when_idle = true;
        self.start_action(BundleAction::StopAll);
    }

    /// The bypass runs with a different profile or options than currently selected.
    pub(crate) fn restart_suggested(&self) -> bool {
        let Some(running) = &self.running_launch else {
            return false;
        };
        self.runtime_active()
            && (!running
                .script
                .eq_ignore_ascii_case(self.config.main_profile_script_or_legacy())
                || running.whitelist != self.config.use_builtin_whitelist
                || running.telegram_proxy != self.config.launch_telegram_proxy_for_profiles)
    }

    pub(crate) fn start_profile(&mut self, profile: BundleProfile) {
        if self.status.service_state.is_active() {
            self.notify(
                LogKind::Error,
                "Работает служба zapret из service.bat. Удалите её в Настройках, чтобы управлять обходом отсюда.",
            );
            return;
        }
        self.start_action(BundleAction::StartProfile {
            profile,
            use_builtin_whitelist: self.config.use_builtin_whitelist,
        });
    }

    pub(crate) fn start_action(&mut self, action: BundleAction) {
        if self.action.is_some() {
            self.notify(LogKind::Info, "Дождитесь завершения текущей команды.");
            return;
        }

        let telegram_proxy = self.telegram_proxy_launch_config();
        if matches!(action, BundleAction::StartProfile { .. })
            && let Err(error) = validate_telegram_proxy_config(&telegram_proxy)
        {
            self.notify(LogKind::Error, format!("Telegram proxy: {error}"));
            self.page = Page::Telegram;
            return;
        }

        let bundle_path = self.bundle_path.clone();
        let job_action = action.clone();
        let task = Task::spawn(&self.ctx, move || {
            run_action(&bundle_path, job_action, &telegram_proxy)
        });
        self.action = Some(PendingAction { action, task });
        self.status_monitor.request_refresh();
    }

    fn poll_action(&mut self) {
        let Some(result) = self.action.as_ref().and_then(|pending| pending.task.poll()) else {
            return;
        };
        let action = self.action.take().map(|pending| pending.action);
        // Native status queries are cheap, so refresh right away instead of waiting a tick.
        self.status = refresh_runtime_status();
        self.status_monitor.request_refresh();

        match result {
            Ok(message) => {
                match &action {
                    Some(BundleAction::StartProfile {
                        profile,
                        use_builtin_whitelist,
                    }) => {
                        self.running_launch = Some(LaunchSnapshot {
                            script: profile.script_name().to_owned(),
                            whitelist: *use_builtin_whitelist,
                            telegram_proxy: self.config.launch_telegram_proxy_for_profiles,
                        });
                    }
                    Some(BundleAction::StopAll | BundleAction::RemoveService) => {
                        self.running_launch = None;
                    }
                    _ => {}
                }
                self.notify(LogKind::Success, message);
            }
            Err(error) => {
                self.restart_when_idle = false;
                let what = match action {
                    Some(BundleAction::StartProfile { .. }) => "Не удалось запустить профиль",
                    Some(BundleAction::StopAll) => "Не удалось остановить обход",
                    _ => "Команда завершилась ошибкой",
                };
                self.notify(LogKind::Error, format!("{what}: {error:#}"));
                if self.quit_requested {
                    self.quit_requested = false;
                    self.show_window();
                }
            }
        }
    }

    pub(crate) fn telegram_proxy_launch_config(&self) -> TelegramProxyLaunchConfig {
        TelegramProxyLaunchConfig {
            enabled: self.config.launch_telegram_proxy_for_profiles,
            mode: self.config.telegram_proxy_mode,
            cf_domain: self.config.telegram_cf_domain.clone(),
        }
    }

    // ----- bundle updates ---------------------------------------------------------------------

    pub(crate) fn start_bundle_check(&mut self, manual: bool) {
        if self.bundle_update.checking.is_some() || self.bundle_update.installing() {
            return;
        }
        let bundle_path = self.bundle_path.clone();
        self.bundle_update.error = None;
        self.bundle_update.checking = Some(Task::spawn(&self.ctx, move || {
            check_bundle_update(&bundle_path)
        }));
        if manual {
            self.push_log(LogKind::Info, "Проверяю обновления bundle…".to_owned());
        }
    }

    /// One click: download → wait for idle → stop bypass → swap → start the profile again.
    pub(crate) fn start_bundle_install(&mut self) {
        if self.bundle_update.installing() {
            return;
        }
        let Some(status) = self.bundle_update.status.clone() else {
            self.start_bundle_check(true);
            return;
        };

        let bundle_path = self.bundle_path.clone();
        let release = status.latest.clone();
        let progress = self.bundle_update.progress.clone();
        self.bundle_update.error = None;
        self.bundle_update.downloading = Some(Task::spawn(&self.ctx, move || {
            prepare_bundle_update(&bundle_path, &release, &progress)
        }));
        self.push_log(
            LogKind::Info,
            format!("Скачиваю bundle {}…", status.latest.tag),
        );
    }

    fn poll_bundle_update(&mut self) {
        if let Some(result) = self.bundle_update.checking.as_ref().and_then(Task::poll) {
            self.bundle_update.checking = None;
            match result {
                Ok(status) => {
                    if !status.update_available {
                        self.push_log(LogKind::Info, "Bundle актуален.".to_owned());
                    }
                    self.bundle_update.status = Some(status);
                }
                Err(error) => {
                    self.bundle_update.error = Some(format!("{error:#}"));
                    self.push_log(
                        LogKind::Error,
                        format!("Проверка обновлений bundle: {error:#}"),
                    );
                }
            }
        }

        if let Some(result) = self.bundle_update.downloading.as_ref().and_then(Task::poll) {
            self.bundle_update.downloading = None;
            match result {
                Ok(prepared) => self.bundle_update.ready = Some(prepared),
                Err(error) => {
                    self.bundle_update_failed(format!("Не удалось скачать bundle: {error:#}"))
                }
            }
        }

        if self.bundle_update.ready.is_some() && self.action.is_none() && !self.test.running() {
            let prepared = self.bundle_update.ready.take().expect("checked above");
            let bundle_path = self.bundle_path.clone();
            self.bundle_update.applying = Some(Task::spawn(&self.ctx, move || {
                let status = refresh_runtime_status();
                let service_was_running = status.service_state.is_active();
                // A service install comes back as the service; a manual launch as the profile.
                let profile_was_running = status.is_active() && !service_was_running;
                if status.is_active()
                    && let Err(error) = stop_runtime(&bundle_path)
                {
                    let _ = discard_prepared_update(&prepared);
                    return Err(error.context("перед обновлением не удалось остановить обход"));
                }
                let outcome = apply_prepared_update(&bundle_path, &prepared);
                if outcome.is_err() {
                    let _ = discard_prepared_update(&prepared);
                }
                if service_was_running {
                    try_run_hidden_command("sc", &["start", ZAPRET_SERVICE], &bundle_path);
                }
                Ok((outcome?, profile_was_running))
            }));
            self.status_monitor.request_refresh();
        }

        if let Some(result) = self.bundle_update.applying.as_ref().and_then(Task::poll) {
            self.bundle_update.applying = None;
            self.status = refresh_runtime_status();
            self.status_monitor.request_refresh();
            match result {
                Ok((outcome, was_active)) => {
                    let mut message = outcome.message;
                    for warning in &outcome.warnings {
                        message.push_str(&format!(" Внимание: {warning}."));
                    }
                    self.notify(LogKind::Success, message);
                    self.config.dismissed_bundle_release_tag = None;
                    self.save_config();
                    self.reload_bundle();
                    self.bundle_update.status = None;
                    self.start_bundle_check(false);
                    self.start_tg_check();
                    self.restart_when_idle = was_active;
                }
                Err(error) => {
                    self.bundle_update_failed(format!("Не удалось установить bundle: {error:#}"))
                }
            }
        }

        if self.restart_when_idle && !self.busy() {
            self.restart_when_idle = false;
            if !self.runtime_active() {
                self.start_main_profile();
            }
        }
    }

    fn bundle_update_failed(&mut self, message: String) {
        self.bundle_update.error = Some(message.clone());
        self.notify(LogKind::Error, message);
    }

    pub(crate) fn dismiss_bundle_notice(&mut self) {
        if let Some(status) = &self.bundle_update.status {
            self.config.dismissed_bundle_release_tag = Some(status.latest.tag.clone());
            self.save_config();
        }
    }

    pub(crate) fn bundle_notice_visible(&self) -> bool {
        self.config.startup_notifications_enabled
            && self.bundle_update.status.as_ref().is_some_and(|status| {
                status.update_available
                    && self.config.dismissed_bundle_release_tag.as_deref()
                        != Some(status.latest.tag.as_str())
            })
    }

    // ----- Telegram proxy updates -------------------------------------------------------------

    pub(crate) fn start_tg_check(&mut self) {
        if self.tg_update.task.is_some() || !self.bundle_valid {
            return;
        }
        let bundle_path = self.bundle_path.clone();
        self.tg_update.error = None;
        self.tg_update.task = Some(TgTask::Check(Task::spawn(&self.ctx, move || {
            check_tg_proxy_update(&bundle_path)
        })));
    }

    pub(crate) fn start_tg_install(&mut self) {
        if self.tg_update.task.is_some() {
            return;
        }
        let Some(status) = self.tg_update.status.clone() else {
            self.start_tg_check();
            return;
        };

        let bundle_path = self.bundle_path.clone();
        let progress = self.tg_update.progress.clone();
        self.tg_update.error = None;
        self.tg_update.task = Some(TgTask::Install(Task::spawn(&self.ctx, move || {
            // The running exe is locked; stop it for the swap and bring it back afterwards.
            let was_running = stop_telegram_proxy(&bundle_path);
            let result = install_tg_proxy_update(&bundle_path, &status.latest, &progress);
            if was_running {
                let _ = restart_telegram_proxy(&bundle_path);
            }
            result
        })));
    }

    fn poll_tg_update(&mut self) {
        let result = match &self.tg_update.task {
            Some(TgTask::Check(task)) => task.poll().map(|result| result.map(Ok)),
            Some(TgTask::Install(task)) => task.poll().map(|result| result.map(Err)),
            None => None,
        };
        let Some(result) = result else {
            return;
        };
        self.tg_update.task = None;

        match result {
            Ok(Ok(status)) => self.tg_update.status = Some(status),
            Ok(Err(message)) => {
                self.notify(LogKind::Success, message);
                self.config.dismissed_tg_proxy_release_tag = None;
                self.save_config();
                self.status_monitor.request_refresh();
                self.start_tg_check();
            }
            Err(error) => {
                let message = format!("Telegram proxy: {error:#}");
                self.tg_update.error = Some(message.clone());
                self.push_log(LogKind::Error, message);
            }
        }
    }

    pub(crate) fn tg_notice_visible(&self) -> bool {
        self.config.startup_notifications_enabled
            && self.config.launch_telegram_proxy_for_profiles
            && self.tg_update.status.as_ref().is_some_and(|status| {
                status.update_available
                    && self.config.dismissed_tg_proxy_release_tag.as_deref()
                        != Some(status.latest.tag.as_str())
            })
    }

    pub(crate) fn dismiss_tg_notice(&mut self) {
        if let Some(status) = &self.tg_update.status {
            self.config.dismissed_tg_proxy_release_tag = Some(status.latest.tag.clone());
            self.save_config();
        }
    }

    // ----- profile test -----------------------------------------------------------------------

    pub(crate) fn test_blocker(&self) -> Option<&'static str> {
        if !self.bundle_valid {
            Some("Сначала установите bundle.")
        } else if self.action.is_some() || self.bundle_update.installing() {
            Some("Дождитесь завершения текущей команды.")
        } else if self.runtime_active() {
            Some("На время подбора обход будет выключен — остановите его кнопкой на главной.")
        } else {
            None
        }
    }

    pub(crate) fn start_profile_test(&mut self) {
        if let Some(blocker) = self.test_blocker() {
            self.notify(LogKind::Info, blocker);
            return;
        }
        let profiles: Vec<BundleProfile> = if self.test.custom_selection {
            self.profiles
                .iter()
                .filter(|profile| {
                    self.test
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
            mode: self.test.mode,
            profiles,
        };
        if let Err(error) = profile_test_preflight(&self.bundle_path, &request) {
            self.test.error = Some(error.to_string());
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let cancellation = Arc::new(AtomicBool::new(false));
        let repaint = self.ctx.clone();
        let (forward_sender, forward_receiver) = mpsc::channel();
        // Forwarder so every progress event wakes the UI up.
        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if forward_sender.send(event).is_err() {
                    return;
                }
                repaint.request_repaint();
            }
        });
        start_profile_test(
            self.bundle_path.clone(),
            request,
            cancellation.clone(),
            sender,
        );
        self.test.receiver = Some(forward_receiver);
        self.test.cancellation = Some(cancellation);
        self.test.current = 0;
        self.test.total = 0;
        self.test.current_label = None;
        self.test.current_check = None;
        self.test.leader = None;
        self.test.report = None;
        self.test.error = None;
        self.status_monitor.request_refresh();
    }

    pub(crate) fn cancel_profile_test(&mut self) {
        if let Some(cancellation) = &self.test.cancellation {
            cancellation.store(true, Ordering::Relaxed);
        }
    }

    fn poll_profile_test(&mut self) {
        let Some(receiver) = &self.test.receiver else {
            return;
        };
        let mut finished = None;
        loop {
            match receiver.try_recv() {
                Ok(ProfileTestEvent::Started { total }) => self.test.total = total,
                Ok(ProfileTestEvent::ProfileStarted {
                    current,
                    total,
                    label,
                }) => {
                    self.test.current = current;
                    self.test.total = total;
                    self.test.current_label = Some(label);
                }
                Ok(ProfileTestEvent::CheckStarted { label }) => {
                    self.test.current_check = Some(label)
                }
                Ok(ProfileTestEvent::ProfileFinished(row)) => {
                    self.test.current_check = None;
                    if self.test.leader.as_ref().is_none_or(|(_, ok)| row.ok > *ok) {
                        self.test.leader = Some((row.label, row.ok));
                    }
                }
                Ok(ProfileTestEvent::Finished(report)) => {
                    finished = Some(Ok(Some(report)));
                    break;
                }
                Ok(ProfileTestEvent::Cancelled) => {
                    finished = Some(Ok(None));
                    break;
                }
                Ok(ProfileTestEvent::Failed(error)) => {
                    finished = Some(Err(error));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err("подбор аварийно завершился".to_owned()));
                    break;
                }
            }
        }

        let Some(outcome) = finished else {
            return;
        };
        self.test.receiver = None;
        self.test.cancellation = None;
        self.test.current_label = None;
        self.status_monitor.request_refresh();
        match outcome {
            Ok(Some(report)) => {
                let best = report
                    .best_script
                    .as_deref()
                    .and_then(|script| {
                        self.profiles
                            .iter()
                            .find(|profile| profile.script_name().eq_ignore_ascii_case(script))
                    })
                    .map_or_else(
                        || "не определён".to_owned(),
                        |profile| profile.label().to_owned(),
                    );
                self.notify(
                    LogKind::Success,
                    format!("Подбор завершён. Лучший профиль: {best}."),
                );
                self.test.report = Some(report);
            }
            Ok(None) => self.notify(LogKind::Info, "Подбор профиля отменён."),
            Err(error) => {
                self.notify(LogKind::Error, format!("Подбор профиля: {error}"));
                self.test.error = Some(error);
            }
        }
    }

    // ----- fake files -------------------------------------------------------------------------

    fn start_fake_catalog_load(&mut self) {
        let bundle_path = self.bundle_path.clone();
        self.fakes.loading = Some(Task::spawn(&self.ctx, move || read_catalog(&bundle_path)));
    }

    fn poll_fakes(&mut self) {
        let Some(result) = self.fakes.loading.as_ref().and_then(Task::poll) else {
            return;
        };
        self.fakes.loading = None;
        match result {
            Ok(catalog) => {
                self.fakes.discord_selection = catalog.discord_current.clone();
                self.fakes.game_selection = catalog.game_current.clone();
                self.fakes.catalog = Some(catalog);
                self.fakes.error = None;
            }
            Err(error) => {
                self.fakes.catalog = None;
                self.fakes.error = Some(error.to_string());
            }
        }
    }

    pub(crate) fn apply_fake(&mut self, target: FakeTarget, file_name: &str) {
        match apply_selection(&self.bundle_path, target, file_name) {
            Ok(()) => {
                self.notify(
                    LogKind::Success,
                    format!(
                        "{} → {file_name}. Перезапустите обход, чтобы применить.",
                        target.label()
                    ),
                );
                self.start_fake_catalog_load();
            }
            Err(error) => self.notify(LogKind::Error, format!("Fake-файл не применён: {error}")),
        }
    }

    // ----- autostart --------------------------------------------------------------------------

    fn start_autostart_query(&mut self) {
        self.autostart.task = Some(Task::spawn(&self.ctx, autostart::is_enabled));
    }

    pub(crate) fn set_autostart(&mut self, enabled: bool) {
        if self.autostart.task.is_some() {
            return;
        }
        self.autostart.task = Some(Task::spawn(&self.ctx, move || {
            autostart::set_enabled(enabled)?;
            Ok(enabled)
        }));
    }

    fn poll_autostart(&mut self) {
        let Some(result) = self.autostart.task.as_ref().and_then(Task::poll) else {
            return;
        };
        self.autostart.task = None;
        match result {
            Ok(enabled) => {
                let changed = self
                    .autostart
                    .enabled
                    .is_some_and(|previous| previous != enabled);
                self.autostart.enabled = Some(enabled);
                if self.config.autostart_enabled != enabled {
                    self.config.autostart_enabled = enabled;
                    self.save_config();
                }
                if changed {
                    self.notify(
                        LogKind::Success,
                        if enabled {
                            "Zapret Hub будет запускаться вместе с Windows."
                        } else {
                            "Автозапуск выключен."
                        },
                    );
                }
            }
            Err(error) => self.notify(LogKind::Error, format!("Автозапуск: {error:#}")),
        }
    }

    // ----- window / tray ----------------------------------------------------------------------

    fn show_window(&mut self) {
        self.window_visible = true;
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // While hidden, eframe only ticks on repaint requests; commands land on the next tick.
        self.ctx.request_repaint();
    }

    fn hide_window(&mut self) {
        self.window_visible = false;
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
        if !self.tray_hint_shown {
            self.tray_hint_shown = true;
            self.push_log(
                LogKind::Info,
                "Zapret Hub свёрнут в трей. Выход — правой кнопкой по иконке.".to_owned(),
            );
        }
    }

    fn handle_tray_commands(&mut self) {
        while let Ok(command) = self.tray_receiver.try_recv() {
            match command {
                TrayCommand::Show => self.show_window(),
                TrayCommand::Toggle => {
                    if self.can_toggle() {
                        self.toggle_runtime();
                    }
                }
                TrayCommand::Quit => self.request_quit(),
            }
        }
    }

    pub(crate) fn request_quit(&mut self) {
        self.quit_requested = true;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        self.ctx.request_repaint();
    }

    /// Everything that must finish before the process can exit.
    fn ready_to_quit(&self) -> bool {
        self.action.is_none()
            && !self.test.running()
            && self.bundle_update.applying.is_none()
            && !(self.config.stop_on_exit && self.runtime_active())
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }

        if !self.quit_requested && self.config.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_window();
            return;
        }

        self.quit_requested = true;
        if self.ready_to_quit() {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.test.running() {
            self.cancel_profile_test();
        } else if self.action.is_none()
            && self.bundle_update.applying.is_none()
            && self.config.stop_on_exit
            && self.runtime_active()
        {
            self.start_action(BundleAction::StopAll);
            self.push_log(
                LogKind::Info,
                "Останавливаю обход перед выходом…".to_owned(),
            );
        }
    }

    fn sync_tray(&mut self) {
        let active = self.runtime_active();
        let busy = self.busy();
        let text = if busy {
            "выполняется команда…".to_owned()
        } else if active {
            format!("включён · {}", self.selected_profile_label())
        } else {
            "обход выключен".to_owned()
        };
        if let Some(tray) = &mut self.tray {
            tray.sync(active, busy, &text);
        }
    }

    // ----- notifications ----------------------------------------------------------------------

    pub(crate) fn notify(&mut self, kind: LogKind, text: impl Into<String>) {
        let text = text.into();
        let lifetime = if kind == LogKind::Error {
            ERROR_TOAST_LIFETIME
        } else {
            TOAST_LIFETIME
        };
        self.toasts.retain(|toast| toast.text != text);
        self.toasts.push(Toast {
            born: Instant::now(),
            kind,
            text: text.clone(),
            expires: Instant::now() + lifetime,
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        self.push_log(kind, text);
        self.ctx.request_repaint_after(lifetime);
    }

    fn push_log(&mut self, kind: LogKind, text: String) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_back();
        }
        self.log.push_front(LogEntry {
            kind,
            text,
            at: Instant::now(),
        });
    }

    pub(crate) fn save_config(&mut self) {
        if let Err(error) = save_app_config(&self.config) {
            self.notify(LogKind::Error, format!("Настройки не сохранены: {error:#}"));
        }
    }

    /// The proxy writes its secret into config.json on first start; re-read it now and then
    /// while the Telegram page is open.
    fn refresh_tg_link(&mut self) {
        if self.page != Page::Telegram {
            return;
        }
        let stale = self
            .tg_link_checked
            .is_none_or(|checked| checked.elapsed() > Duration::from_secs(2));
        if stale {
            self.tg_link = telegram_proxy_link();
            self.tg_link_checked = Some(Instant::now());
            self.ctx.request_repaint_after(Duration::from_secs(2));
        }
    }

    pub(crate) fn set_page(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.page_changed = Instant::now();
            self.config.selected_tab = page.id().to_owned();
            self.save_config();
        }
    }

    pub(crate) fn service_state(&self) -> ServiceState {
        self.status.service_state
    }
}

/// Staging folders left behind by a crash or power loss mid-update.
fn cleanup_stale_update_dirs(bundle_path: &std::path::Path) {
    let Some(parent) = bundle_path.parent().map(ToOwned::to_owned) else {
        return;
    };
    thread::spawn(move || {
        let Ok(entries) = std::fs::read_dir(&parent) else {
            return;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".zapret-hub-update-")
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
}

pub(crate) fn validate_telegram_proxy_config(config: &TelegramProxyLaunchConfig) -> Result<()> {
    if config.enabled
        && config.mode == TelegramProxyMode::CfMedia
        && config.cf_domain.trim().is_empty()
    {
        anyhow::bail!("для режима CF media укажите свой домен в Cloudflare");
    }
    Ok(())
}

impl eframe::App for ZapretHubApp {
    /// Runs before every frame and also while the window is hidden in the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(status) = self.status_monitor.receiver.try_recv() {
            self.status = status;
        }
        if !self.runtime_active() && self.action.is_none() {
            self.running_launch = None;
        }
        self.handle_tray_commands();
        self.poll_action();
        self.poll_bundle_update();
        self.poll_tg_update();
        self.poll_profile_test();
        self.poll_fakes();
        self.poll_autostart();
        self.refresh_tg_link();

        if self.pending_autostart_profile && !self.busy() && self.bundle_valid {
            self.pending_autostart_profile = false;
            if !self.runtime_active() {
                self.start_main_profile();
            }
        }

        if self.window_visible
            && self.config.minimize_to_tray
            && self.tray.is_some()
            && ctx.input(|input| input.viewport().minimized == Some(true))
        {
            self.hide_window();
        }

        self.handle_close_request(ctx);
        if self.quit_requested && self.ready_to_quit() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ctx.request_repaint();
        }

        self.sync_tray();
        #[cfg(debug_assertions)]
        self.advance_screenshot_tour(ctx);

        let now = Instant::now();
        self.toasts.retain(|toast| toast.expires > now);
        if self.bundle_update.installing() || self.tg_update.installing() {
            // Download progress comes from atomics, not events.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
