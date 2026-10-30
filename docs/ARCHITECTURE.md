# dsh-tray architecture

`dsh-tray` is a native (Rust) desktop tray application that supervises a local
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`) web
runtime installed from npm. It is a single binary; the only UI is a system tray
menu (tao + tray-icon/muda, GTK/AppIndicator on Linux).

## Verified upstream facts

- npm package: `@deepseek-ai/dsh`, binary shim `dsh` -> `lib/bin.js`.
- dist-tags today: `latest`, `next`, `alpha` (queried via the registry
  dist-tags endpoint; do not hardcode versions).
- `dsh web --host <ip> --port <port> --no-open` serves the browser UI and,
  once the loader tree settles, prints exactly one line:
  `dsh web: <authenticated-url>` (default `printUrl: true`). The URL may carry
  a `?token=` query; it must be used verbatim to open the browser.
- Default bind is `127.0.0.1:3080`; passing `--port 0` lets the OS choose,
  which avoids collisions with any already-running instance. The tray parses
  the printed URL, so it never has to guess the port.
- `dsh --version` prints the bare version (fast). The same value is also
  readable from `<npm-prefix>/node_modules/@deepseek-ai/dsh/package.json`,
  which is preferred when available.
- Installing/updating/switching channel is `npm install -g @deepseek-ai/dsh@<tag>`.

## Process model

```text
main thread                     controller thread                 workers
-----------                     -----------------                 -------
tao event loop                  owns Config + Snapshot            network / npm / wait
  tray icon + menu   <--UserEvent::Snapshot--  recv Message          |
  MenuEvent -> Command  -------Message::Command------>              |
  TrayIcon left click -> OpenWeb                                    |
```

- The tray is created inside the tao event loop on `StartCause::Init` (macOS/Windows
  requirement; tao's Linux backend is GTK so AppIndicator menus share the loop).
- Snapshots are plain data (`app::Snapshot`); the tray rebuilds the entire menu from
  `tray::menu_spec::build(&snapshot)`. Menu items are addressed by stable string ids
  and mapped back to commands in `tray::command_for`.
- The controller is a dedicated thread. It receives `Message::Command` from the UI and
  `Message::Worker` from worker threads, mutates state, and pushes snapshots through a
  callback (which forwards into the event loop via `EventLoopProxy`).
- Long operations (npm update, registry fetch, readiness wait) always run on worker
  threads; the controller never blocks the UI. `stop` is the one bounded blocking call
  (SIGTERM + grace period), and it is only invoked from the controller.
- Quit: UI sends `Command::Quit`; controller stops the managed child, persists config,
  invokes `on_shutdown`; UI exits the loop. A hard timeout in the UI prevents hangs.

## Modules and ownership

| File | Responsibility |
| --- | --- |
| `src/lib.rs` | crate constants, module wiring |
| `src/channel.rs` | `Channel` enum, dist-tag names, `DistTags` helpers |
| `src/config.rs` | `Config` (serde, atomic save), `AppPaths` (dirs + env overrides), `Language` |
| `src/logging.rs` | global file logger, rotation, child-output lines |
| `src/dsh.rs` | locate dsh/node/npm, read installed version, channel guess |
| `src/npm.rs` | registry dist-tags fetch (ureq), npm global install, registry resolution |
| `src/process.rs` | spawn `node bin.js web ...`, parse URL line, wait/exit, kill tree |
| `src/app.rs` | controller: state machine, commands, snapshots, workers |
| `src/i18n.rs` | `Lang` resolution (en/zh) and typed message keys |
| `src/tray/menu_spec.rs` | pure menu model derived from a snapshot (unit-testable) |
| `src/tray/mod.rs` | muda menu rendering, tray icon, id <-> command mapping |
| `src/autostart.rs` | Windows Run key, macOS LaunchAgent, Linux XDG autostart |
| `src/single_instance.rs` | exclusive lock file + second-launch handoff |
| `src/cli.rs` | argument parsing, `--check`/`--smoke` diagnostics |
| `src/main.rs` | composition root: paths, logger, single instance, tao loop |

## Public interfaces (frozen for parallel work)

```rust,ignore
// channel.rs
pub enum Channel { Latest, Next, Alpha }
impl Channel {
    pub const ALL: [Channel; 3];
    pub fn tag(self) -> &'static str;              // "latest" | "next" | "alpha"
    pub fn parse(s: &str) -> Option<Self>;
}
pub type DistTags = std::collections::BTreeMap<String, String>;
pub fn tag_version<'a>(tags: &'a DistTags, ch: Channel) -> Option<&'a str>;

// config.rs
pub enum Language { Auto, En, Zh }
pub struct Config { /* channel, port, host, autostart, start_dsh_on_launch, ... */ }
impl Config {
    pub fn load(path: &Path) -> (Self, Option<String>); // config + warning
    pub fn save(&self, path: &Path) -> anyhow::Result<()>; // atomic
}
pub struct AppPaths { pub config_dir: PathBuf, pub data_dir: PathBuf, pub exe: PathBuf }
impl AppPaths {
    pub fn resolve() -> Self;
    pub fn with_dirs(config_dir: PathBuf, data_dir: PathBuf) -> Self;
    pub fn ensure(&self) -> anyhow::Result<()>;
    pub fn config_file(&self) -> PathBuf;
    pub fn log_file(&self) -> PathBuf;
    pub fn web_log_file(&self) -> PathBuf;
    pub fn runtime_file(&self) -> PathBuf;
    pub fn lock_file(&self) -> PathBuf;
    pub fn tray_temp_dir(&self) -> PathBuf;
}

// dsh.rs
pub struct DshInstall {
    pub version: String,
    pub package_root: PathBuf,
    pub bin_js: PathBuf,
    pub node: PathBuf,
    pub npm: PathBuf,
}
impl DshInstall { pub fn discover(cfg: &Config) -> anyhow::Result<Self>; }
pub fn detect_channel(version: &str, tags: &DistTags, preferred: Channel) -> Channel;

// npm.rs
pub struct NpmClient { pub program: PathBuf, pub registry: String }
impl NpmClient {
    pub fn discover(cfg: &Config) -> anyhow::Result<Self>;
    pub fn dist_tags(&self) -> anyhow::Result<DistTags>;
    pub fn install(&self, channel: Channel, log: &mut dyn FnMut(&str)) -> anyhow::Result<()>;
}

// process.rs
pub enum ProcessEvent { Ready { pid: u32, url: String }, Exited { pid: u32, code: Option<i32> }, StartupFailed { pid: u32, message: String } }
pub struct SpawnOptions { pub host: String, pub port: u16, pub log_path: PathBuf }
pub struct ProcessManager { /* ... */ }
impl ProcessManager {
    pub fn new(install: DshInstall) -> Self;
    pub fn spawn(&mut self, opts: &SpawnOptions, sink: Box<dyn Fn(ProcessEvent) + Send + Sync>) -> anyhow::Result<u32>;
    pub fn stop(&mut self, grace: Duration) -> anyhow::Result<()>;
    pub fn is_running(&self) -> bool;
    pub fn pid(&self) -> Option<u32>;
}

// app.rs
pub enum Command { OpenWeb, StartDsh, StopDsh, RestartDsh, CheckUpdates, UpdateCurrentChannel,
                   SwitchChannel(Channel), ToggleAutostart, ToggleStartOnLaunch,
                   OpenLog, OpenConfigFolder, Quit }
pub enum Message { Command(Command), Worker(WorkerEvent) }
pub struct Snapshot { pub lang: Lang, pub installed_version: Option<String>, pub channel: Channel,
                      pub state: ServiceState, pub activity: Activity, pub tags: Option<DistTags>,
                      pub update_available: Option<String>, pub autostart: bool,
                      pub start_on_launch: bool, pub last_error: Option<String>, /* paths for UI */ }
pub fn start(host: ControllerHost, on_snapshot: SnapshotFn, on_shutdown: ShutdownFn) -> anyhow::Result<Sender<Message>>;

// tray/
pub struct TrayUi;
impl TrayUi {
    pub fn new(snapshot: &Snapshot, paths: &AppPaths) -> anyhow::Result<Self>;
    pub fn update(&mut self, snapshot: &Snapshot);
}
pub fn command_for(id: &muda::MenuId) -> Option<Command>;
```

## Update / channel semantics

- Channel preference is persisted (`Config::channel`). The *installed* version is read
  from package.json; the effective channel is `detect_channel(installed, tags, preferred)`:
  exact tag match wins; when tags share a version (latest == next today) the persisted
  preference disambiguates; prerelease suffixes are a last-resort hint.
- "Switch channel" only runs npm when the target tag version differs from the installed
  version; otherwise it just persists the preference and restarts if running.
- "Check for updates" fetches dist-tags (registry HTTP, npm fallback) and compares with
  semver. Updating stops a managed child first, installs, re-reads the version, then
  restarts it if it was running.
- Failure paths surface in the tray header + log file; npm permission errors are
  reported verbatim with a hint to fix the global prefix.
