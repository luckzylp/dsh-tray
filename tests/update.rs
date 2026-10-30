//! Hermetic controller tests for the channel/update flow.
#![cfg(unix)]
//!
//! A tiny in-process HTTP server stands in for the npm registry, and a fake
//! npm script copies a prepared package.json, so the whole channel/update path
//! runs without network access and without touching the real npm prefix.

use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsh_tray::app::{self, Activity, Command, ControllerHost, Message, ServiceState, Snapshot};
use dsh_tray::channel::Channel;
use dsh_tray::config::{AppPaths, Config};

const INSTALLED: &str = "0.2.1";

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// Start a registry stub that answers every GET with the given body.
fn registry_server(tags_json: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind registry");
    let address = listener.local_addr().expect("addr");
    let body = tags_json.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line == "\r\n" || line == "\n" => break,
                    Ok(_) => {}
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{address}")
}

struct Fixture {
    dir: tempfile::TempDir,
    package_json: PathBuf,
    node: PathBuf,
}

impl Fixture {
    /// Create a fake installation + fake npm. `npm_sleep_ms` slows the install
    /// so tests can observe the in-flight activity state.
    fn new(npm_sleep_ms: u64) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let package_root = dir.path().join("pkg");
        std::fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        std::fs::write(package_root.join("lib/bin.js"), b"// fake entry point\n").expect("bin.js");
        let package_json = package_root.join("package.json");
        write_package_version(&package_json, INSTALLED);
        for (name, version) in [("latest", "0.2.2"), ("next", "0.2.3-rc.1"), ("alpha", "0.4.0-alpha.1")] {
            write_package_version(&dir.path().join(format!("pkg-{name}.json")), version);
        }
        let npm_log = dir.path().join("npm-argv.log");
        let npm = dir.path().join("fake-npm.sh");
        let body = format!(
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" >> {log}\n",
                "case \"$*\" in\n",
                "  *install*g*) ;;\n",
                "  *) exit 0 ;;\n",
                "esac\n",
                "sleep 0.{sleep}\n",
                "code=$(cat {exitfile} 2>/dev/null || echo 0)\n",
                "if [ \"$code\" != \"0\" ]; then echo \"npm error simulated failure\" >&2; exit \"$code\"; fi\n",
                "last=\"\"\n",
                "for arg in \"$@\"; do last=\"$arg\"; done\n",
                "case \"$last\" in\n",
                "  *@alpha) cp {alpha} {pkg} ;;\n",
                "  *@next) cp {next} {pkg} ;;\n",
                "  *) cp {latest} {pkg} ;;\n",
                "esac\n",
                "exit $(cat {exitfile} 2>/dev/null || echo 0)\n",
            ),
            log = npm_log.display(),
            sleep = npm_sleep_ms,
            alpha = dir.path().join("pkg-alpha.json").display(),
            next = dir.path().join("pkg-next.json").display(),
            latest = dir.path().join("pkg-latest.json").display(),
            pkg = package_json.display(),
            exitfile = dir.path().join("npm-exit").display()
        );
        write_executable(&npm, &body);
        // Fake "node": prints the readiness line and then waits to be killed.
        let node = dir.path().join("fake-node.sh");
        write_executable(
            &node,
            concat!(
                "#!/bin/sh\n",
                "echo 'dsh web: http://127.0.0.1:39999/?token=test-token'\n",
                "trap 'exit 0' TERM INT\n",
                "while :; do sleep 0.05; done\n",
            ),
        );
        Self {
            dir,
            package_json,
            node,
        }
    }

    fn config(&self, registry: String) -> Config {
        Config {
            dsh_path: Some(self.dir.path().join("pkg/lib/bin.js")),
            node_path: Some(self.node.clone()),
            npm_path: Some(self.dir.path().join("fake-npm.sh")),
            registry: Some(registry),
            start_dsh_on_launch: false,
            open_browser_when_ready: false,
            update_check_hours: 720,
            channel: Channel::Latest,
            ..Config::default()
        }
    }
}

fn write_package_version(path: &Path, version: &str) {
    std::fs::write(path, format!(r#"{{"name":"@deepseek-ai/dsh","version":"{version}"}}"#)).expect("package.json");
}

struct Harness {
    _fixture: Fixture,
    paths: AppPaths,
    snapshots: Arc<Mutex<Vec<Snapshot>>>,
    shutdown: Arc<AtomicBool>,
    tx: Sender<Message>,
}

impl Harness {
    fn new(fixture: Fixture, registry: String) -> Self {
        let paths = AppPaths::with_dirs(fixture.dir.path().join("cfg"), fixture.dir.path().join("data"));
        paths.ensure().expect("ensure dirs");
        let config = fixture.config(registry);
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
                paths: paths.clone(),
                config,
                hidden: true,
            },
            on_snapshot,
            on_shutdown,
        )
        .expect("controller start");
        Self {
            _fixture: fixture,
            paths,
            snapshots,
            shutdown,
            tx,
        }
    }

    fn send(&self, command: Command) {
        self.tx.send(Message::Command(command)).expect("send");
    }

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

#[cfg(unix)]
#[test]
fn update_current_channel_runs_npm_and_rediscovers_the_version() {
    let fixture = Fixture::new(0);
    let registry = registry_server(r#"{"latest":"0.2.2","next":"0.2.3-rc.1","alpha":"0.4.0-alpha.1"}"#);
    let npm_log = fixture.dir.path().join("npm-argv.log");
    let package_json = fixture.package_json.clone();
    let harness = Harness::new(fixture, registry);
    let mark = harness.mark();
    let (_, ready) = harness
        .wait_new(mark, Duration::from_secs(15), |snapshot| {
            snapshot.update_available.as_deref() == Some("0.2.2")
        })
        .expect("background update check never reported an update");
    assert_eq!(ready.installed_version.as_deref(), Some(INSTALLED));
    assert_eq!(ready.channel, Channel::Latest);
    let mark = harness.mark();
    harness.send(Command::UpdateCurrentChannel);
    let (_, succeeded) = harness
        .wait_new(mark, Duration::from_secs(30), |snapshot| {
            snapshot.installed_version.as_deref() == Some("0.2.2") && snapshot.activity == Activity::Idle
        })
        .expect("update never finished");
    assert_eq!(succeeded.channel, Channel::Latest);
    assert!(
        succeeded.update_available.is_none(),
        "update_available: {:?}",
        succeeded.update_available
    );
    assert_eq!(succeeded.state, ServiceState::Stopped);
    assert_eq!(succeeded.notice, Some(dsh_tray::app::Notice::UpdateSucceeded));
    let invocations = std::fs::read_to_string(&npm_log).expect("npm argv log");
    assert!(
        invocations.contains("install -g --no-fund --no-audit @deepseek-ai/dsh@latest"),
        "npm invocation: {invocations}"
    );
    let package = std::fs::read_to_string(&package_json).expect("package.json");
    assert!(package.contains("0.2.2"), "{package}");
    let config = std::fs::read_to_string(harness.paths.config_file()).expect("config.json");
    assert!(config.contains(r#""channel": "latest""#), "{config}");
    harness.quit();
}

#[cfg(unix)]
#[test]
fn switch_channel_installs_the_tag_and_persists_the_preference() {
    let fixture = Fixture::new(0);
    let registry = registry_server(r#"{"latest":"0.2.1-rc.5","next":"0.2.3-rc.1","alpha":"0.4.0-alpha.1"}"#);
    let npm_log = fixture.dir.path().join("npm-argv.log");
    let harness = Harness::new(fixture, registry);
    let mark = harness.mark();
    harness
        .wait_new(mark, Duration::from_secs(15), |snapshot| snapshot.tags.is_some())
        .expect("dist-tags were never fetched");
    let mark = harness.mark();
    harness.send(Command::SwitchChannel(Channel::Alpha));
    let (_, switched) = harness
        .wait_new(mark, Duration::from_secs(30), |snapshot| {
            snapshot.channel == Channel::Alpha
                && snapshot.installed_version.as_deref() == Some("0.4.0-alpha.1")
                && snapshot.activity == Activity::Idle
        })
        .expect("channel switch never completed");
    assert_eq!(switched.update_available, None);
    let invocations = std::fs::read_to_string(&npm_log).expect("npm argv log");
    assert!(
        invocations.contains("@deepseek-ai/dsh@alpha"),
        "npm invocation: {invocations}"
    );
    let config = std::fs::read_to_string(harness.paths.config_file()).expect("config.json");
    assert!(config.contains(r#""channel": "alpha""#), "{config}");
    harness.quit();
}

#[cfg(unix)]
#[test]
fn failed_update_is_reported_and_keeps_the_installed_version() {
    let fixture = Fixture::new(0);
    std::fs::write(fixture.dir.path().join("npm-exit"), b"7").expect("npm-exit");
    let registry = registry_server(r#"{"latest":"0.2.2","next":"0.2.3-rc.1","alpha":"0.4.0-alpha.1"}"#);
    let package_json = fixture.package_json.clone();
    let harness = Harness::new(fixture, registry);
    let mark = harness.mark();
    harness
        .wait_new(mark, Duration::from_secs(15), |snapshot| {
            snapshot.update_available.as_deref() == Some("0.2.2")
        })
        .expect("update check never reported an update");
    let mark = harness.mark();
    harness.send(Command::UpdateCurrentChannel);
    let (_, failed) = harness
        .wait_new(mark, Duration::from_secs(30), |snapshot| {
            snapshot.notice == Some(dsh_tray::app::Notice::UpdateFailed) && snapshot.activity == Activity::Idle
        })
        .expect("failed update was not reported");
    assert_eq!(failed.installed_version.as_deref(), Some(INSTALLED));
    let detail = failed.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("exit code 7"), "detail: {detail}");
    assert!(detail.contains("npm error simulated failure"), "detail: {detail}");
    let package = std::fs::read_to_string(&package_json).expect("package.json");
    assert!(!package.contains("0.2.2"), "{package}");
    harness.quit();
}

// ---------------------------------------------------------------------------
// Regression tests for stale worker events
// ---------------------------------------------------------------------------

/// Regression: a stale StopFinished event from the stopped child must not reset
/// the in-flight update activity to Idle while npm install is still running.
#[cfg(unix)]
#[test]
fn update_activity_is_not_clobbered_while_npm_runs() {
    let fixture = Fixture::new(900);
    let registry = registry_server(r#"{"latest":"0.2.2","next":"0.2.3-rc.1","alpha":"0.4.0-alpha.1"}"#);
    let harness = Harness::new(fixture, registry);
    let mark = harness.mark();
    harness
        .wait_new(mark, Duration::from_secs(15), |snapshot| {
            snapshot.update_available.as_deref() == Some("0.2.2")
        })
        .expect("update check never reported an update");
    let mark = harness.mark();
    harness.send(Command::StartDsh);
    harness
        .wait_new(mark, Duration::from_secs(15), |snapshot| {
            matches!(snapshot.state, ServiceState::Running { .. })
        })
        .expect("dsh never started");
    let mark = harness.mark();
    harness.send(Command::UpdateCurrentChannel);
    harness
        .wait_new(mark, Duration::from_secs(10), |snapshot| {
            matches!(snapshot.activity, Activity::Updating { .. })
        })
        .expect("update never started");
    // The fake npm sleeps for 0.9 s; sample the state while it is still running.
    std::thread::sleep(Duration::from_millis(400));
    let during = harness.latest();
    assert!(
        matches!(during.activity, Activity::Updating { .. }),
        "update activity was reset while npm install was still running: activity={:?} state={:?} notice={:?}",
        during.activity,
        during.state,
        during.notice
    );

    // The update must finish (UpdateSucceeded) and restart the child that was
    // running before the update.
    let (next, succeeded) = harness
        .wait_new(mark, Duration::from_secs(30), |snapshot| {
            snapshot.installed_version.as_deref() == Some("0.2.2")
                && snapshot.notice == Some(dsh_tray::app::Notice::UpdateSucceeded)
        })
        .expect("update never reported success");
    assert!(matches!(succeeded.activity, Activity::Idle));
    let (_, restarted) = harness
        .wait_new(next, Duration::from_secs(15), |snapshot| {
            matches!(snapshot.state, ServiceState::Running { .. })
        })
        .expect("update did not restart the previously running dsh");
    assert_eq!(restarted.installed_version.as_deref(), Some("0.2.2"));
    harness.quit();
}
