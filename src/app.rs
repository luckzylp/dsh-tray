//! Controller: owns mutable state, executes commands, emits snapshots.
//!
//! The controller runs on its own thread (see start). The tray never touches
//! state directly: it renders Snapshot values and publishes Command values
//! through the returned channel.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;

use crate::autostart;
use crate::channel::{self, Channel, DistTags};
use crate::config::{AppPaths, Config};
use crate::dsh::{self, DshInstall};
use crate::i18n::Lang;
use crate::logging;
use crate::npm::NpmClient;
use crate::process::{self, ProcessEvent, ProcessManager, SpawnOptions};
use crate::single_instance::{self, RuntimeState};

/// How often the controller wakes up to run periodic work.
const TICK: Duration = Duration::from_secs(1);
/// Maximum time to wait for the dsh web URL line after spawning.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Grace period used when stopping the child process.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// A user action requested from the tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the web UI in the default browser (starting dsh if needed).
    OpenWeb,
    /// Start the managed dsh web process.
    StartDsh,
    /// Stop the managed dsh web process.
    StopDsh,
    /// Stop (if running) and start again.
    RestartDsh,
    /// Refresh npm dist-tags.
    CheckUpdates,
    /// Update the installed package to the current channel's latest version.
    UpdateCurrentChannel,
    /// Persist a new preferred channel; update the package when it differs.
    SwitchChannel(Channel),
    /// Toggle the platform autostart entry.
    ToggleAutostart,
    /// Toggle "start dsh when the tray starts".
    ToggleStartOnLaunch,
    /// Open the tray log file.
    OpenLog,
    /// Open the config directory.
    OpenConfigFolder,
    /// Stop the managed child (if configured) and exit.
    Quit,
}

/// Result of a successful npm update.
#[derive(Clone, Debug)]
pub struct UpdateOutcome {
    /// Version installed after the update.
    pub version: String,
    /// Re-resolved installation (new bin/node paths are normally identical).
    pub install: DshInstall,
}

/// Results of long-running worker jobs.
#[derive(Debug)]
pub enum WorkerEvent {
    ProcessReady {
        pid: u32,
        url: String,
    },
    ProcessExited {
        pid: u32,
        code: Option<i32>,
    },
    ProcessStartupFailed {
        pid: u32,
        message: String,
    },
    /// A stop request finished (successful or not).
    StopFinished {
        result: Result<(), String>,
    },
    /// Registry dist-tags fetched.
    TagsFetched {
        result: Result<DistTags, String>,
    },
    /// npm global install finished.
    UpdateFinished {
        result: Result<UpdateOutcome, String>,
        channel: Channel,
        was_running: bool,
    },
}

/// Messages processed by the controller loop.
#[derive(Debug)]
pub enum Message {
    Command(Command),
    Worker(WorkerEvent),
}

/// Lifecycle of the managed dsh process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running { pid: u32, url: String, managed: bool },
    Stopping,
}

impl ServiceState {
    pub fn is_running(&self) -> bool {
        matches!(self, ServiceState::Running { .. })
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            ServiceState::Running { url, .. } => Some(url),
            _ => None,
        }
    }
}

/// What the controller is busy with (menu progress hints).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Checking,
    Updating { channel: Channel, target: String },
    Restarting,
}

impl Activity {
    pub fn is_idle(&self) -> bool {
        matches!(self, Activity::Idle)
    }
}

/// Localizable status/diagnostic notice for the tray header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Starting,
    Stopping,
    Checking,
    Updating,
    Restarting,
    UpdateSucceeded,
    UpdateFailed,
    CheckFailed,
    StartFailed,
    StopFailed,
    StartupTimedOut,
    ExternalInstance,
    AlreadyRunning,
    AutostartFailed,
    ConfigFailed,
    OpenFailed,
}

/// Immutable view of the world rendered by the tray.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub lang: Lang,
    pub app_version: &'static str,
    pub installed_version: Option<String>,
    pub channel: Channel,
    pub state: ServiceState,
    pub activity: Activity,
    pub tags: Option<DistTags>,
    pub update_available: Option<String>,
    pub autostart: bool,
    pub start_dsh_on_launch: bool,
    pub notice: Option<Notice>,
    pub detail: Option<String>,
    pub busy: bool,
}

impl Snapshot {
    /// Placeholder snapshot used while the controller boots.
    pub fn initial(config: &Config, _paths: &AppPaths) -> Self {
        Self {
            lang: Lang::resolve(config.language),
            app_version: crate::VERSION,
            installed_version: None,
            channel: config.channel,
            state: ServiceState::Stopped,
            activity: Activity::Idle,
            tags: None,
            update_available: None,
            autostart: config.autostart,
            start_dsh_on_launch: config.start_dsh_on_launch,
            notice: None,
            detail: None,
            busy: false,
        }
    }
}

/// Inputs required to start the controller.
pub struct ControllerHost {
    pub paths: AppPaths,
    pub config: Config,
    /// Launched by a login item: start dsh, but do not open a browser window.
    pub hidden: bool,
}

/// Callback invoked whenever state changes.
pub type SnapshotFn = Box<dyn Fn(Snapshot) + Send + Sync>;
/// Callback invoked when the controller has finished shutting down.
pub type ShutdownFn = Box<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopReason {
    User,
    Restart,
    Update,
}

#[derive(Clone, Debug)]
struct UpdatePlan {
    channel: Channel,
    was_running: bool,
}

struct Controller {
    paths: AppPaths,
    config: Config,
    lang: Lang,
    hidden: bool,
    tx: Sender<Message>,
    on_snapshot: SnapshotFn,
    on_shutdown: ShutdownFn,
    dsh: Option<DshInstall>,
    npm: Option<NpmClient>,
    npm_error: Option<String>,
    state: ServiceState,
    activity: Activity,
    tags: Option<DistTags>,
    update_available: Option<String>,
    notice: Option<Notice>,
    detail: Option<String>,
    last_check: Option<Instant>,
    check_in_flight: bool,
    autostart: bool,
    startup_deadline: Option<Instant>,
    open_when_ready: bool,
    child_pid: Option<u32>,
    process: Option<Arc<Mutex<ProcessManager>>>,
    stop_reason: Option<StopReason>,
    pending_update: Option<UpdatePlan>,
    channel_adopted: bool,
    started_at: String,
    shutdown_done: bool,
}

impl Controller {
    fn new(host: ControllerHost, tx: Sender<Message>, on_snapshot: SnapshotFn, on_shutdown: ShutdownFn) -> Self {
        let ControllerHost {
            paths,
            mut config,
            hidden,
        } = host;
        if let Some(warning) = config.normalize() {
            logging::warn(format!("config: {warning}"));
        }
        let autostart = autostart::status();
        if config.autostart != autostart.enabled {
            config.autostart = autostart.enabled;
            if let Err(error) = config.save(&paths.config_file()) {
                logging::warn(format!("could not persist autostart state: {error}"));
            }
        }
        Self {
            lang: Lang::resolve(config.language),
            config,
            paths,
            hidden,
            tx,
            on_snapshot,
            on_shutdown,
            dsh: None,
            npm: None,
            npm_error: None,
            state: ServiceState::Stopped,
            activity: Activity::Idle,
            tags: None,
            update_available: None,
            notice: None,
            detail: None,
            last_check: None,
            check_in_flight: false,
            autostart: autostart.enabled,
            startup_deadline: None,
            open_when_ready: false,
            child_pid: None,
            process: None,
            stop_reason: None,
            pending_update: None,
            channel_adopted: false,
            started_at: Local::now().to_rfc3339(),
            shutdown_done: false,
        }
    }

    fn busy(&self) -> bool {
        !matches!(self.activity, Activity::Idle | Activity::Checking)
            || matches!(self.state, ServiceState::Starting | ServiceState::Stopping)
    }

    fn activity_blocks_service(&self) -> bool {
        matches!(self.activity, Activity::Updating { .. }) || matches!(self.state, ServiceState::Stopping)
    }

    fn publish(&self) {
        let snapshot = Snapshot {
            lang: self.lang,
            app_version: crate::VERSION,
            installed_version: self.dsh.as_ref().map(|install| install.version.clone()),
            channel: self.config.channel,
            state: self.state.clone(),
            activity: self.activity.clone(),
            tags: self.tags.clone(),
            update_available: self.update_available.clone(),
            autostart: self.autostart,
            start_dsh_on_launch: self.config.start_dsh_on_launch,
            notice: self.notice,
            detail: self.detail.clone(),
            busy: self.busy(),
        };
        (self.on_snapshot)(snapshot);
    }

    fn save_config(&mut self) {
        if let Err(error) = self.config.save(&self.paths.config_file()) {
            logging::error(format!("could not save config: {error}"));
            self.notice = Some(Notice::ConfigFailed);
            self.detail = Some(format!("could not save config: {error}"));
        }
    }

    fn write_runtime(&self) {
        match &self.state {
            ServiceState::Running { pid, url, .. } => self.write_runtime_entry(*pid, url),
            // Nothing is serving: a stale hand-off URL is worse than no file.
            _ => single_instance::remove_runtime(&self.paths.runtime_file()),
        }
    }

    fn write_runtime_entry(&self, pid: u32, url: &str) {
        let state = RuntimeState {
            pid,
            url: Some(url.to_string()),
            tray_version: crate::VERSION.to_string(),
            started_at: self.started_at.clone(),
        };
        if let Err(error) = single_instance::write_runtime(&self.paths.runtime_file(), &state) {
            logging::warn(format!("could not write runtime state: {error}"));
        }
    }

    fn clear_notice(&mut self) {
        self.notice = None;
        self.detail = None;
    }

    fn recompute_update_available(&mut self) {
        let installed = self.dsh.as_ref().map(|install| install.version.as_str());
        let target = self
            .tags
            .as_ref()
            .and_then(|tags| channel::tag_version(tags, self.config.channel))
            .map(str::to_string);
        self.update_available = match (installed, target) {
            (Some(installed), Some(target)) => newer_version(installed, &target),
            _ => None,
        };
    }

    fn refresh_environment(&mut self) {
        match DshInstall::discover(&self.config) {
            Ok(install) => {
                logging::info(format!("dsh {} at {}", install.version, install.bin_js.display()));
                self.dsh = Some(install);
            }
            Err(error) => {
                logging::warn(format!("dsh discovery failed: {error}"));
                self.dsh = None;
                self.detail = Some(error.to_string());
            }
        }
        match NpmClient::discover(&self.config) {
            Ok(npm) => {
                logging::info(format!("npm {} (registry {})", npm.program.display(), npm.registry));
                self.npm = Some(npm);
                self.npm_error = None;
            }
            Err(error) => {
                logging::warn(format!("npm discovery failed: {error}"));
                self.npm = None;
                self.npm_error = Some(error.to_string());
            }
        }
        self.recompute_update_available();
        self.publish();
    }

    fn run(mut self, rx: Receiver<Message>) {
        logging::info(format!("controller started (pid {})", std::process::id()));
        self.publish();
        self.refresh_environment();
        self.maybe_auto_start();
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Message::Command(command)) => {
                    if self.handle_command(command) {
                        break;
                    }
                }
                Ok(Message::Worker(event)) => self.handle_worker(event),
                Err(RecvTimeoutError::Timeout) => self.on_tick(),
                Err(RecvTimeoutError::Disconnected) => {
                    logging::warn("UI channel disconnected; stopping");
                    self.shutdown(true);
                    break;
                }
            }
        }
        logging::info("controller stopped");
    }

    fn maybe_auto_start(&mut self) {
        if self.config.start_dsh_on_launch && self.dsh.is_some() && self.state == ServiceState::Stopped {
            let open_browser = self.config.open_browser_when_ready && !self.hidden;
            logging::info("start_dsh_on_launch: starting dsh");
            self.start_dsh(open_browser);
        }
    }

    fn on_tick(&mut self) {
        if let Some(deadline) = self.startup_deadline {
            if Instant::now() >= deadline && matches!(self.state, ServiceState::Starting) {
                self.handle_startup_timeout();
                return;
            }
        }
        if self.activity.is_idle()
            && !matches!(self.state, ServiceState::Starting | ServiceState::Stopping)
            && self.dsh.is_some()
        {
            let due = match self.last_check {
                Some(last) => {
                    last.elapsed() >= Duration::from_secs(self.config.update_check_hours.saturating_mul(3600))
                }
                None => true,
            };
            if due {
                self.check_updates(false);
            }
        }
    }

    fn handle_command(&mut self, command: Command) -> bool {
        logging::info(format!("command {command:?}"));
        match command {
            Command::Quit => {
                self.shutdown(true);
                return true;
            }
            Command::OpenWeb => self.open_web(),
            Command::StartDsh => {
                self.clear_notice();
                self.start_dsh(false);
            }
            Command::StopDsh => {
                self.clear_notice();
                self.stop_user();
            }
            Command::RestartDsh => {
                self.clear_notice();
                self.restart();
            }
            Command::CheckUpdates => {
                self.clear_notice();
                self.check_updates(true);
            }
            Command::UpdateCurrentChannel => {
                self.clear_notice();
                self.update_current();
            }
            Command::SwitchChannel(channel) => {
                self.clear_notice();
                self.switch_channel(channel);
            }
            Command::ToggleAutostart => self.toggle_autostart(),
            Command::ToggleStartOnLaunch => self.toggle_start_on_launch(),
            Command::OpenLog => {
                let path = self.paths.log_file();
                self.open_path(&path);
            }
            Command::OpenConfigFolder => {
                let path = self.paths.config_dir.clone();
                self.open_path(&path);
            }
        }
        false
    }

    fn open_web(&mut self) {
        match self.state.clone() {
            ServiceState::Running { url, .. } => self.open_url(&url),
            ServiceState::Starting => {
                self.open_when_ready = true;
                self.detail = Some("the browser will open when dsh is ready".into());
                self.publish();
            }
            ServiceState::Stopping => {
                self.detail = Some("dsh is stopping".into());
                self.publish();
            }
            ServiceState::Stopped => {
                if self.activity_blocks_service() {
                    self.detail = Some("another operation is in progress".into());
                    self.publish();
                } else {
                    self.start_dsh(true);
                }
            }
        }
    }

    fn open_url(&mut self, url: &str) {
        match open::that(url) {
            Ok(()) => logging::info(format!("opened browser at {}", process::display_url(url))),
            Err(error) => {
                logging::error(format!("could not open browser: {error}"));
                self.notice = Some(Notice::OpenFailed);
                self.detail = Some(format!("could not open browser: {error}"));
            }
        }
        self.publish();
    }

    fn open_path(&mut self, path: &std::path::Path) {
        match open::that(path) {
            Ok(()) => logging::info(format!("opened {}", path.display())),
            Err(error) => {
                logging::error(format!("could not open {}: {error}", path.display()));
                self.notice = Some(Notice::OpenFailed);
                self.detail = Some(format!("could not open {}: {error}", path.display()));
            }
        }
        self.publish();
    }

    fn start_dsh(&mut self, open_browser: bool) {
        if matches!(self.state, ServiceState::Starting | ServiceState::Running { .. }) {
            self.notice = Some(Notice::AlreadyRunning);
            self.publish();
            return;
        }
        if self.activity_blocks_service() {
            self.detail = Some("another operation is in progress".into());
            self.publish();
            return;
        }
        let Some(install) = self.dsh.clone() else {
            self.notice = Some(Notice::StartFailed);
            self.detail = Some(match &self.npm_error {
                Some(error) => format!("dsh is not installed: {error}"),
                None => "dsh is not installed; run: npm install -g @deepseek-ai/dsh@latest".to_string(),
            });
            logging::error("cannot start dsh: not installed");
            self.publish();
            return;
        };
        let options = SpawnOptions {
            host: self.config.host.clone(),
            port: self.config.port,
            log_path: self.paths.web_log_file(),
        };
        let tx = self.tx.clone();
        let sink: Box<dyn Fn(ProcessEvent) + Send + Sync> = Box::new(move |event| {
            let message = match event {
                ProcessEvent::Ready { pid, url } => WorkerEvent::ProcessReady { pid, url },
                ProcessEvent::Exited { pid, code } => WorkerEvent::ProcessExited { pid, code },
                ProcessEvent::StartupFailed { pid, message } => WorkerEvent::ProcessStartupFailed { pid, message },
            };
            let _ = tx.send(Message::Worker(message));
        });
        let mut process = ProcessManager::new(install);
        match process.spawn(&options, sink) {
            Ok(pid) => {
                logging::info(format!("spawning dsh web (pid {pid})"));
                self.process = Some(Arc::new(Mutex::new(process)));
                self.child_pid = Some(pid);
                self.state = ServiceState::Starting;
                self.startup_deadline = Some(Instant::now() + STARTUP_TIMEOUT);
                self.open_when_ready = open_browser;
                self.notice = Some(Notice::Starting);
                self.detail = None;
                self.write_runtime();
                self.publish();
            }
            Err(error) => {
                logging::error(format!("could not spawn dsh web: {error}"));
                self.process = None;
                self.child_pid = None;
                self.state = ServiceState::Stopped;
                self.notice = Some(Notice::StartFailed);
                self.detail = Some(error.to_string());
                self.write_runtime();
                self.publish();
            }
        }
    }

    fn stop_user(&mut self) {
        if matches!(self.state, ServiceState::Running { managed: false, .. }) {
            self.notice = Some(Notice::StartFailed);
            self.detail = Some("this dsh instance was not started by the tray, so it cannot be stopped here".into());
            self.publish();
            return;
        }
        self.stop_with(StopReason::User);
    }

    fn stop_with(&mut self, reason: StopReason) {
        if !matches!(self.state, ServiceState::Running { .. } | ServiceState::Starting) {
            self.stop_reason = Some(reason);
            self.handle_stopped();
            return;
        }
        let Some(process) = self.process.clone() else {
            self.state = ServiceState::Stopped;
            self.child_pid = None;
            self.stop_reason = Some(reason);
            self.handle_stopped();
            return;
        };
        self.stop_reason = Some(reason);
        self.state = ServiceState::Stopping;
        self.notice = Some(Notice::Stopping);
        self.detail = None;
        self.startup_deadline = None;
        self.publish();
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("stop-dsh".into()).spawn(move || {
            let result = match process.lock() {
                Ok(mut manager) => manager.stop(STOP_GRACE).map_err(|error| error.to_string()),
                Err(_) => Err("process lock poisoned".to_string()),
            };
            let _ = tx.send(Message::Worker(WorkerEvent::StopFinished { result }));
        });
        if let Err(error) = spawned {
            logging::error(format!("could not spawn stop thread: {error}"));
            self.stop_reason = None;
            self.state = ServiceState::Stopped;
            self.notice = Some(Notice::StopFailed);
            self.detail = Some(error.to_string());
            self.publish();
        }
    }

    fn restart(&mut self) {
        if self.activity_blocks_service() {
            self.detail = Some("another operation is in progress".into());
            self.publish();
            return;
        }
        if matches!(self.state, ServiceState::Running { .. } | ServiceState::Starting) {
            self.stop_with(StopReason::Restart);
        } else {
            self.activity = Activity::Idle;
            self.publish();
            self.start_dsh(false);
        }
    }

    fn switch_channel(&mut self, channel: Channel) {
        if !self.activity.is_idle() {
            self.detail = Some("another operation is in progress".into());
            self.publish();
            return;
        }
        if self.config.channel == channel && self.update_available.is_none() {
            self.publish();
            return;
        }
        self.config.channel = channel;
        self.channel_adopted = true;
        self.save_config();
        self.recompute_update_available();
        self.publish();
        let installed = self.dsh.as_ref().map(|install| install.version.clone());
        let target = self
            .tags
            .as_ref()
            .and_then(|tags| channel::tag_version(tags, channel))
            .map(str::to_string);
        let needs_update = match (installed.as_deref(), target.as_deref()) {
            (Some(installed), Some(target)) => installed != target,
            _ => true,
        };
        if !needs_update {
            logging::info(format!("channel set to {channel}; installed version is current"));
            self.clear_notice();
            self.publish();
            return;
        }
        if matches!(self.state, ServiceState::Running { managed: false, .. }) {
            self.notice = Some(Notice::UpdateFailed);
            self.detail = Some("stop the externally started dsh before switching channels".into());
            self.publish();
            return;
        }
        let was_running = matches!(self.state, ServiceState::Running { managed: true, .. });
        self.start_update(channel, was_running);
    }

    fn update_current(&mut self) {
        if !self.activity.is_idle() {
            self.detail = Some("another operation is in progress".into());
            self.publish();
            return;
        }
        if matches!(self.state, ServiceState::Running { managed: false, .. }) {
            self.notice = Some(Notice::UpdateFailed);
            self.detail = Some("stop the externally started dsh before updating".into());
            self.publish();
            return;
        }
        if self.update_available.is_none() {
            self.notice = Some(Notice::UpdateSucceeded);
            self.detail = Some(format!("already on the newest {} version", self.config.channel));
            self.publish();
            return;
        }
        let channel = self.config.channel;
        let was_running = matches!(self.state, ServiceState::Running { managed: true, .. });
        self.start_update(channel, was_running);
    }

    fn start_update(&mut self, channel: Channel, was_running: bool) {
        if !self.activity.is_idle() {
            return;
        }
        let plan = UpdatePlan { channel, was_running };
        if was_running {
            self.pending_update = Some(plan);
            self.stop_with(StopReason::Update);
        } else {
            self.begin_update(plan);
        }
    }

    fn begin_update(&mut self, plan: UpdatePlan) {
        let Some(npm) = self.npm.clone() else {
            self.notice = Some(Notice::UpdateFailed);
            self.detail = Some(match &self.npm_error {
                Some(error) => format!("npm is not available: {error}"),
                None => "npm is not available".to_string(),
            });
            self.activity = Activity::Idle;
            self.publish();
            return;
        };
        let target = self
            .tags
            .as_ref()
            .and_then(|tags| channel::tag_version(tags, plan.channel))
            .map(str::to_string)
            .unwrap_or_else(|| plan.channel.tag().to_string());
        logging::info(format!("updating dsh to channel {} (target {target})", plan.channel));
        self.activity = Activity::Updating {
            channel: plan.channel,
            target,
        };
        self.notice = Some(Notice::Updating);
        self.detail = None;
        self.publish();
        let config = self.config.clone();
        let tx = self.tx.clone();
        let channel = plan.channel;
        let was_running = plan.was_running;
        let spawned = thread::Builder::new().name("npm-update".into()).spawn(move || {
            let mut logger = |line: &str| logging::info(format!("npm: {line}"));
            let result = match npm.install(channel, &mut logger) {
                Ok(()) => match DshInstall::discover(&config) {
                    Ok(install) => {
                        let version = install.version.clone();
                        Ok(UpdateOutcome { version, install })
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(Message::Worker(WorkerEvent::UpdateFinished {
                result,
                channel,
                was_running,
            }));
        });
        if let Err(error) = spawned {
            logging::error(format!("could not spawn update thread: {error}"));
            self.activity = Activity::Idle;
            self.notice = Some(Notice::UpdateFailed);
            self.detail = Some(error.to_string());
            self.publish();
        }
    }

    fn check_updates(&mut self, explicit: bool) {
        if self.check_in_flight {
            if explicit {
                self.detail = Some("an update check is already running".into());
                self.publish();
            }
            return;
        }
        if !self.activity.is_idle() {
            if explicit {
                self.detail = Some("another operation is in progress".into());
                self.publish();
            }
            return;
        }
        // Record the attempt up front: a failing registry/npm must not turn the
        // periodic tick into a tight retry loop that blocks the controller.
        self.last_check = Some(Instant::now());
        let npm = match self.npm.clone() {
            Some(npm) => npm,
            None => match NpmClient::discover(&self.config) {
                Ok(npm) => {
                    self.npm = Some(npm.clone());
                    self.npm_error = None;
                    npm
                }
                Err(error) => {
                    self.npm_error = Some(error.to_string());
                    if explicit {
                        self.notice = Some(Notice::CheckFailed);
                        self.detail = Some(format!("npm is not available: {error}"));
                        self.publish();
                    }
                    return;
                }
            },
        };
        if explicit {
            self.activity = Activity::Checking;
            self.clear_notice();
            self.publish();
        }
        self.check_in_flight = true;
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("check-updates".into()).spawn(move || {
            let result = npm.dist_tags().map_err(|error| error.to_string());
            let _ = tx.send(Message::Worker(WorkerEvent::TagsFetched { result }));
        });
        if let Err(error) = spawned {
            logging::error(format!("could not spawn update-check thread: {error}"));
            self.check_in_flight = false;
            self.activity = Activity::Idle;
            if explicit {
                self.notice = Some(Notice::CheckFailed);
                self.detail = Some(error.to_string());
            }
            self.publish();
        }
    }

    fn handle_worker(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::ProcessReady { pid, url } => self.handle_process_ready(pid, url),
            WorkerEvent::ProcessExited { pid, code } => self.handle_process_exited(pid, code),
            WorkerEvent::ProcessStartupFailed { pid, message } => self.handle_startup_failed(pid, message),
            WorkerEvent::StopFinished { result } => self.handle_stop_finished(result),
            WorkerEvent::TagsFetched { result } => self.handle_tags(result),
            WorkerEvent::UpdateFinished {
                result,
                channel,
                was_running,
            } => self.handle_update_finished(result, channel, was_running),
        }
    }

    fn handle_process_ready(&mut self, pid: u32, url: String) {
        if self.child_pid != Some(pid) || !matches!(self.state, ServiceState::Starting) {
            return;
        }
        self.state = ServiceState::Running {
            pid,
            url: url.clone(),
            managed: true,
        };
        self.startup_deadline = None;
        self.clear_notice();
        self.write_runtime();
        logging::info(format!("dsh web ready at {}", process::display_url(&url)));
        if self.open_when_ready {
            self.open_when_ready = false;
            self.open_url(&url);
        } else {
            self.publish();
        }
    }

    fn handle_process_exited(&mut self, pid: u32, code: Option<i32>) {
        if self.child_pid != Some(pid) {
            return;
        }
        let unexpected =
            self.stop_reason.is_none() && matches!(self.state, ServiceState::Starting | ServiceState::Running { .. });
        if unexpected {
            let detail = match code {
                Some(code) => format!("dsh web exited unexpectedly (exit code {code})"),
                None => "dsh web exited unexpectedly (killed by a signal)".to_string(),
            };
            logging::warn(detail.clone());
            self.notice = Some(Notice::StartFailed);
            self.detail = Some(detail);
        }
        self.child_pid = None;
        self.process = None;
        self.state = ServiceState::Stopped;
        self.startup_deadline = None;
        self.write_runtime();
        self.handle_stopped();
    }

    fn handle_startup_failed(&mut self, pid: u32, message: String) {
        if self.child_pid != Some(pid) {
            return;
        }
        logging::error(format!("dsh web failed to start: {message}"));
        self.child_pid = None;
        self.process = None;
        self.state = ServiceState::Stopped;
        self.startup_deadline = None;
        self.open_when_ready = false;
        self.activity = Activity::Idle;
        self.notice = Some(Notice::StartFailed);
        self.detail = Some(message);
        self.write_runtime();
        self.publish();
    }

    fn handle_stop_finished(&mut self, result: Result<(), String>) {
        // A stop worker from an earlier process can finish after the controller
        // already observed Exited and started a replacement (Restart/Update).
        // Ignoring such stale completions keeps the new child alive.
        if self.stop_reason.is_none() && !matches!(self.state, ServiceState::Stopping) {
            logging::info("ignoring stale stop result");
            return;
        }
        match result {
            Ok(()) => {
                logging::info("dsh stopped");
                if matches!(self.state, ServiceState::Stopping) {
                    self.state = ServiceState::Stopped;
                }
                self.child_pid = None;
                self.process = None;
                self.write_runtime();
                self.handle_stopped();
            }
            Err(error) => {
                logging::error(format!("could not stop dsh: {error}"));
                self.stop_reason = None;
                self.pending_update = None;
                self.child_pid = None;
                self.process = None;
                self.state = ServiceState::Stopped;
                self.startup_deadline = None;
                self.activity = Activity::Idle;
                self.notice = Some(Notice::StopFailed);
                self.detail = Some(error);
                self.write_runtime();
                self.publish();
            }
        }
    }

    fn handle_stopped(&mut self) {
        let reason = self.stop_reason.take();
        self.state = ServiceState::Stopped;
        self.child_pid = None;
        self.process = None;
        self.startup_deadline = None;
        self.open_when_ready = false;
        self.write_runtime();
        match reason {
            Some(StopReason::Update) => {
                self.activity = Activity::Idle;
                self.publish();
                match self.pending_update.take() {
                    Some(plan) => self.begin_update(plan),
                    None => logging::warn("update requested but no update plan was recorded"),
                }
            }
            Some(StopReason::Restart) => {
                self.activity = Activity::Idle;
                self.publish();
                self.start_dsh(false);
            }
            Some(StopReason::User) | None => {
                if self.notice == Some(Notice::Stopping) {
                    self.clear_notice();
                }
                self.activity = Activity::Idle;
                self.publish();
            }
        }
    }

    fn handle_startup_timeout(&mut self) {
        logging::error("dsh web did not report a URL before the startup timeout");
        self.notice = Some(Notice::StartupTimedOut);
        self.detail = Some("dsh did not report a URL in time; see the log".into());
        if let Some(process) = self.process.take() {
            if let Ok(mut manager) = process.lock() {
                if let Err(error) = manager.stop(Duration::from_secs(2)) {
                    logging::warn(format!("could not stop timed-out dsh: {error}"));
                }
            }
        }
        self.stop_reason = None;
        self.child_pid = None;
        self.state = ServiceState::Stopped;
        self.startup_deadline = None;
        self.open_when_ready = false;
        self.activity = Activity::Idle;
        self.write_runtime();
        self.publish();
    }

    fn handle_tags(&mut self, result: Result<DistTags, String>) {
        self.check_in_flight = false;
        match result {
            Ok(tags) => {
                logging::info(format!("dist-tags: {}", channel::tags_summary(&tags)));
                self.tags = Some(tags);
                self.last_check = Some(Instant::now());
                if matches!(self.activity, Activity::Checking) {
                    self.activity = Activity::Idle;
                }
                // A later success must clear a previous "check failed" notice,
                // including failures recorded by background checks.
                if self.notice == Some(Notice::CheckFailed) {
                    self.clear_notice();
                }
                self.adopt_channel_if_needed();
                self.recompute_update_available();
                self.publish();
            }
            Err(error) => {
                logging::warn(format!("update check failed: {error}"));
                if matches!(self.activity, Activity::Checking) {
                    self.activity = Activity::Idle;
                }
                self.notice = Some(Notice::CheckFailed);
                self.detail = Some(error);
                self.publish();
            }
        }
    }

    fn adopt_channel_if_needed(&mut self) {
        if self.channel_adopted {
            return;
        }
        let Some(installed) = self.dsh.as_ref().map(|install| install.version.clone()) else {
            return;
        };
        let Some(tags) = self.tags.clone() else { return };
        let detected = dsh::detect_channel(&installed, &tags, self.config.channel);
        self.channel_adopted = true;
        if detected != self.config.channel {
            logging::info(format!(
                "installed version {installed} looks like the {detected} channel; adopting it"
            ));
            self.config.channel = detected;
            self.save_config();
            self.recompute_update_available();
        }
    }

    fn handle_update_finished(&mut self, result: Result<UpdateOutcome, String>, channel: Channel, was_running: bool) {
        match result {
            Ok(outcome) => {
                logging::info(format!("updated dsh to {} ({channel})", outcome.version));
                self.dsh = Some(outcome.install);
                self.config.channel = channel;
                self.channel_adopted = true;
                self.save_config();
                self.activity = Activity::Idle;
                self.notice = Some(Notice::UpdateSucceeded);
                self.detail = Some(format!("installed {}", outcome.version));
                self.recompute_update_available();
                if was_running {
                    self.activity = Activity::Idle;
                    self.publish();
                    self.start_dsh(false);
                } else {
                    self.publish();
                    self.check_updates(false);
                }
            }
            Err(error) => {
                logging::error(format!("update failed: {error}"));
                self.activity = Activity::Idle;
                self.notice = Some(Notice::UpdateFailed);
                self.detail = Some(error);
                if was_running {
                    self.activity = Activity::Idle;
                    self.publish();
                    self.start_dsh(false);
                } else {
                    self.publish();
                }
            }
        }
    }

    fn toggle_autostart(&mut self) {
        let result = if self.autostart {
            autostart::disable()
        } else {
            autostart::enable(&self.paths.exe)
        };
        match result {
            Ok(()) => {
                let status = autostart::status();
                self.autostart = status.enabled;
                self.config.autostart = status.enabled;
                self.save_config();
                self.clear_notice();
                logging::info(format!(
                    "autostart {} ({})",
                    if status.enabled { "enabled" } else { "disabled" },
                    status.path.display()
                ));
            }
            Err(error) => {
                logging::error(format!("could not toggle autostart: {error}"));
                self.notice = Some(Notice::AutostartFailed);
                self.detail = Some(error.to_string());
            }
        }
        self.publish();
    }

    fn toggle_start_on_launch(&mut self) {
        self.config.start_dsh_on_launch = !self.config.start_dsh_on_launch;
        self.save_config();
        self.clear_notice();
        self.publish();
    }

    fn shutdown(&mut self, stop_child: bool) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        logging::info("shutting down controller");
        let should_stop = stop_child
            && self.config.stop_dsh_on_quit
            && matches!(
                self.state,
                ServiceState::Running { managed: true, .. } | ServiceState::Starting
            );
        // With stop_dsh_on_quit=false the child survives the tray, so keep a
        // usable hand-off entry (child pid + authenticated URL) on disk.
        let kept = if should_stop {
            None
        } else {
            match &self.state {
                ServiceState::Running { pid, url, .. } => Some((*pid, url.clone())),
                _ => None,
            }
        };
        if should_stop {
            if let Some(process) = self.process.take() {
                match process.lock() {
                    Ok(mut manager) => match manager.stop(STOP_GRACE) {
                        Ok(()) => logging::info("stopped dsh on shutdown"),
                        Err(error) => logging::warn(format!("could not stop dsh on shutdown: {error}")),
                    },
                    Err(_) => logging::warn("process lock poisoned while shutting down"),
                }
            }
        }
        self.state = ServiceState::Stopped;
        self.child_pid = None;
        self.process = None;
        self.stop_reason = None;
        self.save_config();
        match kept {
            Some((pid, url)) => self.write_runtime_entry(pid, &url),
            None => single_instance::remove_runtime(&self.paths.runtime_file()),
        }
        (self.on_shutdown)();
    }
}

/// Spawn the controller thread; returns the sender used by the UI.
pub fn start(host: ControllerHost, on_snapshot: SnapshotFn, on_shutdown: ShutdownFn) -> Result<Sender<Message>> {
    let (tx, rx) = mpsc::channel();
    let controller_tx = tx.clone();
    thread::Builder::new()
        .name("controller".into())
        .spawn(move || {
            let controller = Controller::new(host, controller_tx, on_snapshot, on_shutdown);
            controller.run(rx);
        })
        .map_err(|error| anyhow::anyhow!("could not spawn controller thread: {error}"))?;
    Ok(tx)
}

/// Return target when it is strictly newer than installed.
fn newer_version(installed: &str, target: &str) -> Option<String> {
    let installed = semver::Version::parse(installed).ok()?;
    let target = semver::Version::parse(target).ok()?;
    (target > installed).then(|| target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_compares_semver() {
        assert_eq!(
            newer_version("0.2.1-rc.5", "0.2.2-alpha.2"),
            Some("0.2.2-alpha.2".to_string())
        );
        assert_eq!(newer_version("0.2.1-rc.5", "0.2.1-rc.5"), None);
        assert_eq!(newer_version("0.2.2-alpha.2", "0.2.1-rc.5"), None);
        assert_eq!(newer_version("not-a-version", "0.2.1"), None);
    }

    #[test]
    fn snapshot_initial_reflects_config() {
        let config = Config::default();
        let paths = AppPaths::with_dirs("/tmp/cfg".into(), "/tmp/data".into());
        let snapshot = Snapshot::initial(&config, &paths);
        assert_eq!(snapshot.channel, Channel::Latest);
        assert_eq!(snapshot.state, ServiceState::Stopped);
        assert!(!snapshot.busy);
        assert_eq!(snapshot.app_version, crate::VERSION);
    }

    #[test]
    fn busy_ignores_pure_checks() {
        // Checking for updates must not disable Start/Stop.
        let busy = |activity: Activity, state: ServiceState| {
            !matches!(activity, Activity::Idle | Activity::Checking)
                || matches!(state, ServiceState::Starting | ServiceState::Stopping)
        };
        assert!(!busy(Activity::Checking, ServiceState::Stopped));
        assert!(busy(
            Activity::Updating {
                channel: Channel::Alpha,
                target: "0.2.2-alpha.2".into()
            },
            ServiceState::Stopped
        ));
        assert!(busy(Activity::Idle, ServiceState::Starting));
    }
}
