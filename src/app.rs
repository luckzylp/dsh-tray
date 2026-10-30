//! Controller: owns mutable state, executes commands, emits snapshots.
//!
//! The controller runs on its own thread (see `start`). The tray never touches
//! state directly: it renders `Snapshot` values and publishes `Command`s.

use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::channel::{Channel, DistTags};
use crate::config::{AppPaths, Config};
use crate::i18n::Lang;

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

/// Results of long-running worker jobs.
#[derive(Debug)]
pub enum WorkerEvent {
    ProcessReady { pid: u32, url: String },
    ProcessExited { pid: u32, code: Option<i32> },
    ProcessStartupFailed { pid: u32, message: String },
    StartupTimedOut { pid: u32 },
    /// A stop request finished (successful or not).
    StopFinished { result: Result<(), String> },
    /// Registry dist-tags fetched.
    TagsFetched { result: Result<DistTags, String> },
    /// npm global install finished.
    UpdateFinished {
        result: Result<String, String>,
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
}

/// Callback invoked whenever state changes.
pub type SnapshotFn = Box<dyn Fn(Snapshot) + Send + Sync>;
/// Callback invoked when the controller has finished shutting down.
pub type ShutdownFn = Box<dyn Fn() + Send + Sync>;

/// Spawn the controller thread; returns the sender used by the UI.
pub fn start(_host: ControllerHost, _on_snapshot: SnapshotFn, _on_shutdown: ShutdownFn) -> anyhow::Result<Sender<Message>> {
    anyhow::bail!("controller not implemented yet")
}

/// Grace period used when stopping the child process.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
