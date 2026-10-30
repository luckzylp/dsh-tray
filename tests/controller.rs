//! Integration tests for the controller state machine (no GUI, no network).
//!
//! These tests drive app::start with a fake dsh installation: a shell script
//! standing in for node prints the readiness line, records its pid, then
//! sleeps until it is signalled. Every test is hermetic: config/data dirs live
//! in a tempdir, npm points at a script that always fails, and the registry
//! points at a closed port so a background update check fails immediately.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsh_tray::app::{self, Activity, Command, ControllerHost, Message, Notice, ServiceState, Snapshot};
use dsh_tray::config::{AppPaths, Config, Language};
use dsh_tray::i18n::Lang;
use dsh_tray::tray::menu_spec;

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// A fake dsh installation whose "node" prints the readiness URL, records its
/// pid and then waits to be terminated.
fn fake_install(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let package_root = dir.join("node_modules/@deepseek-ai/dsh");
    std::fs::create_dir_all(package_root.join("lib")).expect("mkdir");
    std::fs::write(package_root.join("lib/bin.js"), b"// fake entry point\n").expect("bin.js");
    std::fs::write(
        package_root.join("package.json"),
        br#"{"name":"@deepseek-ai/dsh","version":"0.0.0-test"}"#,
    )
    .expect("package.json");
    let pidfile = dir.join("child-pids.txt");
    // $1 is bin.js; the readiness line must match process.rs exactly.
    let node = dir.join("fake-node.sh");
    let body = format!(
        concat!(
            "#!/bin/sh\n",
            "echo \"$$\" >> {pidfile}\n",
            "if [ \"$1\" = \"--version\" ]; then echo 0.0.0-test; exit 0; fi\n",
            "echo 'dsh web: http://127.0.0.1:39999/?token=test-token'\n",
            "trap 'exit 0' TERM INT\n",
            "while :; do sleep 0.05; done\n",
        ),
        pidfile = pidfile.display()
    );
    write_executable(&node, &body);
    let npm = dir.join("fake-npm.sh");
    write_executable(&npm, "#!/bin/sh\nexit 1\n");
    (package_root, node, npm)
}

fn fake_config(dir: &Path) -> (Config, PathBuf) {
    let (package_root, node, npm) = fake_install(dir);
    let config = Config {
        dsh_path: Some(package_root.join("lib/bin.js")),
        node_path: Some(node),
        npm_path: Some(npm),
        registry: Some("http://127.0.0.1:9".to_string()),
        start_dsh_on_launch: false,
        open_browser_when_ready: false,
        update_check_hours: 720,
        ..Config::default()
    };
    (config, dir.join("child-pids.txt"))
}

struct Harness {
    _dir: tempfile::TempDir,
    snapshots: Arc<Mutex<Vec<Snapshot>>>,
    shutdown: Arc<AtomicBool>,
    tx: Sender<Message>,
    pidfile: PathBuf,
    config_file: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let (config, pidfile) = fake_config(dir.path());
        let paths = AppPaths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
        paths.ensure().expect("ensure dirs");
        let config_file = paths.config_file();
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let sink = snapshots.clone();
        let on_snapshot = Box::new(move |snapshot: Snapshot| {
            sink.lock().expect("snapshot lock").push(snapshot);
        });
        let flag = shutdown.clone();
        let on_shutdown = Box::new(move || flag.store(true, Ordering::SeqCst));
        let tx = app::start(
            ControllerHost {
                paths,
                config,
                hidden: true,
            },
            on_snapshot,
            on_shutdown,
        )
        .expect("controller start");
        Self {
            _dir: dir,
            snapshots,
            shutdown,
            tx,
            pidfile,
            config_file,
        }
    }

    fn send(&self, command: Command) {
        self.tx.send(Message::Command(command)).expect("send");
    }

    /// Number of snapshots recorded so far (used as a cursor for wait_new).
    fn mark(&self) -> usize {
        self.snapshots.lock().expect("lock").len()
    }

    fn latest(&self) -> Snapshot {
        self.snapshots
            .lock()
            .expect("lock")
            .last()
            .cloned()
            .expect("a snapshot")
    }

    /// Wait for a snapshot published after `cursor` that satisfies `predicate`.
    fn wait_new(
        &self,
        cursor: usize,
        timeout: Duration,
        mut predicate: impl FnMut(&Snapshot) -> bool,
    ) -> Option<(usize, Snapshot)> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let snapshots = self.snapshots.lock().expect("lock");
            for (index, snapshot) in snapshots.iter().enumerate().skip(cursor) {
                if predicate(snapshot) {
                    return Some((index + 1, snapshot.clone()));
                }
            }
            drop(snapshots);
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn wait_running(&self, cursor: usize, timeout: Duration) -> (usize, Snapshot) {
        self.wait_new(cursor, timeout, |snapshot| {
            matches!(snapshot.state, ServiceState::Running { .. })
        })
        .unwrap_or_else(|| panic!("dsh never became running; last snapshot: {:?}", self.latest().state))
    }

    #[allow(dead_code)]
    fn child_pids(&self) -> Vec<i32> {
        std::fs::read_to_string(&self.pidfile)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }

    fn quit(&self) {
        self.send(Command::Quit);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !self.shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            self.shutdown.load(Ordering::SeqCst),
            "controller did not finish shutdown"
        );
    }
}

fn alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn pid_of(snapshot: &Snapshot) -> u32 {
    match snapshot.state {
        ServiceState::Running { pid, .. } => pid,
        ref other => panic!("expected Running, got {other:?}"),
    }
}

#[test]
fn starts_and_quits_without_leaving_the_child_behind() {
    let harness = Harness::new();
    let cursor = harness.mark();
    harness.send(Command::StartDsh);
    let (_, running) = harness.wait_running(cursor, Duration::from_secs(10));
    let pid = pid_of(&running);
    assert!(alive(pid as i32), "managed child must be alive at pid {pid}");
    harness.quit();
    assert!(!alive(pid as i32), "child pid {pid} survived Quit");
}

#[test]
fn restart_keeps_a_live_managed_child() {
    let harness = Harness::new();
    let mut cursor = harness.mark();
    harness.send(Command::StartDsh);
    let (next, running) = harness.wait_running(cursor, Duration::from_secs(10));
    cursor = next;
    let mut current = running;
    let mut failures = Vec::new();
    for attempt in 0..6 {
        let old_pid = pid_of(&current);
        harness.send(Command::RestartDsh);
        let restarted = harness.wait_new(cursor, Duration::from_secs(8), |snapshot| match snapshot.state {
            ServiceState::Running { pid, .. } => pid != old_pid,
            _ => false,
        });
        match restarted {
            Some((next, snapshot)) => {
                cursor = next;
                let new_pid = pid_of(&snapshot);
                current = snapshot;
                // The restarted child must survive: a stale stop event from the
                // previous generation must not kill it (nor silently stop the
                // service). Give asynchronous stop events time to arrive.
                std::thread::sleep(Duration::from_millis(1200));
                let settled = harness.latest();
                let still_running = matches!(settled.state, ServiceState::Running { pid, .. } if pid == new_pid);
                if !still_running || !alive(new_pid as i32) {
                    failures.push(format!(
                        "attempt {attempt}: restarted pid {new_pid} was killed shortly after start (state {:?}, alive {})",
                        settled.state,
                        alive(new_pid as i32)
                    ));
                    let mark = harness.mark();
                    harness.send(Command::StartDsh);
                    if let Some((next, snapshot)) = harness.wait_new(mark, Duration::from_secs(8), |s| {
                        matches!(s.state, ServiceState::Running { .. })
                    }) {
                        cursor = next;
                        current = snapshot;
                    } else {
                        break;
                    }
                }
            }
            None => {
                failures.push(format!(
                    "attempt {attempt}: old={old_pid} last={:?}",
                    harness.latest().state
                ));
                let mark = harness.mark();
                harness.send(Command::StartDsh);
                match harness.wait_new(mark, Duration::from_secs(8), |s| {
                    matches!(s.state, ServiceState::Running { .. })
                }) {
                    Some((next, snapshot)) => {
                        cursor = next;
                        current = snapshot;
                    }
                    None => break,
                }
            }
        }
    }
    harness.quit();
    assert!(failures.is_empty(), "restart left dsh stopped: {failures:#?}");
}
#[test]
fn stop_clears_the_stopping_notice() {
    let harness = Harness::new();
    let mut cursor = harness.mark();
    harness.send(Command::StartDsh);
    let (next, _) = harness.wait_running(cursor, Duration::from_secs(10));
    cursor = next;
    harness.send(Command::StopDsh);
    let (_, stopped) = harness
        .wait_new(cursor, Duration::from_secs(10), |snapshot| {
            matches!(snapshot.state, ServiceState::Stopped) && snapshot.activity == Activity::Idle
        })
        .expect("StopDsh never reached the stopped state");
    // Regression: the transient Stopping notice used to survive a successful stop.
    assert_eq!(stopped.notice, None, "stale notice after a successful stop");
    harness.quit();
}

#[test]
fn restart_leaves_the_service_running_and_idle() {
    let harness = Harness::new();
    let mut cursor = harness.mark();
    harness.send(Command::StartDsh);
    let (next, running) = harness.wait_running(cursor, Duration::from_secs(10));
    cursor = next;
    let old_pid = pid_of(&running);
    harness.send(Command::RestartDsh);
    let (_, restarted) = harness
        .wait_new(cursor, Duration::from_secs(10), |snapshot| match snapshot.state {
            ServiceState::Running { pid, .. } => pid != old_pid,
            _ => false,
        })
        .expect("RestartDsh never produced a new running process");
    assert_eq!(restarted.activity, Activity::Idle);
    assert_eq!(restarted.notice, None);
    assert!(!restarted.busy);
    // A stale stop result from the old generation must not kill the new child.
    std::thread::sleep(Duration::from_millis(1200));
    let settled = harness.latest();
    assert!(
        matches!(settled.state, ServiceState::Running { pid, .. } if pid == pid_of(&restarted)),
        "new child did not survive: {:?}",
        settled.state
    );
    assert!(alive(pid_of(&restarted) as i32));
    harness.quit();
}

#[test]
fn language_preference_switches_the_ui_and_persists() {
    let harness = Harness::new();
    // Wait until the environment snapshot settles.
    harness
        .wait_new(0, Duration::from_secs(15), |snapshot| {
            snapshot.installed_version.is_some()
        })
        .expect("controller did not initialise");

    let mark = harness.mark();
    harness.send(Command::SetLanguage(Language::Zh));
    let (_, zh) = harness
        .wait_new(mark, Duration::from_secs(10), |snapshot| {
            snapshot.language == Language::Zh && snapshot.lang == Lang::Zh
        })
        .expect("language did not switch to Chinese");
    assert_eq!(zh.lang, Lang::Zh);
    let saved = std::fs::read_to_string(&harness.config_file).expect("saved config");
    assert!(saved.contains("\"language\": \"zh\""), "{saved}");

    let mark = harness.mark();
    harness.send(Command::SetLanguage(Language::Auto));
    let expected = Lang::resolve(Language::Auto);
    harness
        .wait_new(mark, Duration::from_secs(10), |snapshot| {
            snapshot.language == Language::Auto && snapshot.lang == expected
        })
        .expect("language did not switch back to automatic");
    let saved = std::fs::read_to_string(&harness.config_file).expect("saved config");
    assert!(saved.contains("\"language\": \"auto\""), "{saved}");
    harness.quit();
}

/// While the service is Starting (up to the 60 s startup timeout) the menu
/// offers Stop so a hung start can be cancelled without quitting the tray.
#[test]
fn menu_offers_stop_while_starting() {
    let snapshot = Snapshot {
        lang: dsh_tray::i18n::Lang::En,
        language: Language::Auto,
        app_version: dsh_tray::VERSION,
        installed_version: Some("0.2.1".to_string()),
        channel: dsh_tray::channel::Channel::Latest,
        state: ServiceState::Starting,
        activity: Activity::Idle,
        tags: None,
        update_available: None,
        autostart: false,
        start_dsh_on_launch: true,
        notice: Some(Notice::Starting),
        detail: None,
        busy: true,
    };
    let spec = menu_spec::build(&snapshot);
    let stop = spec
        .entry(menu_spec::ids::STOP)
        .expect("Stop must be reachable while Starting so a hung start can be cancelled");
    assert!(stop.enabled, "Stop must be enabled while Starting");
    assert!(spec.entry(menu_spec::ids::START).is_none());
    assert!(
        spec.entry(menu_spec::ids::RESTART).is_none(),
        "Restart must not be reachable while Starting"
    );
}
