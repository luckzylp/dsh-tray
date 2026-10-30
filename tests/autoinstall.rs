//! A missing dsh CLI must be installed globally with npm before the service
//! starts. Unix-only because the fake npm/node and process-group handling are
//! shell based.
#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsh_tray::app::{self, Activity, Command, ControllerHost, Message, Notice, ServiceState, Snapshot};
use dsh_tray::config::{AppPaths, Config};

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
fn missing_dsh_is_installed_globally_then_started() {
    let dir = tempfile::tempdir().expect("tempdir");
    let package_root = dir.path().join("global/@deepseek-ai/dsh");
    let bin_js = package_root.join("lib/bin.js");
    let npm_log = dir.path().join("npm-argv.log");
    let node = dir.path().join("fake-node.sh");
    write_executable(
        &node,
        concat!(
            "#!/bin/sh\n",
            "echo 'dsh web: http://127.0.0.1:39998/?token=test-token'\n",
            "trap 'exit 0' TERM INT\n",
            "while :; do sleep 0.05; done\n",
        ),
    );
    let npm = dir.path().join("fake-npm.sh");
    write_executable(
        &npm,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" >> {log}\n",
                "case \"$*\" in\n",
                "  *install*g*) ;;\n",
                "  *) exit 0 ;;\n",
                "esac\n",
                "mkdir -p {package_root}/lib\n",
                "printf '%s' '{{\"name\":\"@deepseek-ai/dsh\",\"version\":\"0.2.2\"}}' > {package_root}/package.json\n",
                "printf '%s' '// fake entry' > {package_root}/lib/bin.js\n",
                "exit 0\n",
            ),
            log = npm_log.display(),
            package_root = package_root.display(),
        ),
    );

    let paths = AppPaths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    paths.ensure().expect("ensure dirs");
    let config = Config {
        dsh_path: Some(bin_js),
        node_path: Some(node),
        npm_path: Some(npm),
        registry: Some("http://127.0.0.1:9".to_string()),
        start_dsh_on_launch: false,
        open_browser_when_ready: false,
        update_check_hours: 720,
        ..Config::default()
    };
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let sink = snapshots.clone();
    let on_snapshot = Box::new(move |snapshot: Snapshot| {
        sink.lock().expect("lock").push(snapshot);
    });
    let flag = shutdown.clone();
    let on_shutdown = Box::new(move || flag.store(true, Ordering::SeqCst));
    let tx: Sender<Message> = app::start(
        ControllerHost {
            paths,
            config,
            hidden: true,
        },
        on_snapshot,
        on_shutdown,
    )
    .expect("controller start");

    // Wait for the initial environment snapshot (dsh missing).
    wait_for(&snapshots, Duration::from_secs(15), |snapshot| {
        snapshot.installed_version.is_none()
    })
    .expect("initial snapshot");

    tx.send(Message::Command(Command::StartDsh)).expect("start");
    let installing = wait_for(&snapshots, Duration::from_secs(15), |snapshot| {
        matches!(snapshot.activity, Activity::Installing { .. })
    })
    .expect("install activity was never reported");
    assert_eq!(installing.notice, Some(Notice::Installing));

    let running = wait_for(&snapshots, Duration::from_secs(20), |snapshot| {
        matches!(snapshot.state, ServiceState::Running { .. }) && snapshot.installed_version.as_deref() == Some("0.2.2")
    })
    .expect("service did not start after the global install");
    assert_eq!(running.notice, None);

    let argv = std::fs::read_to_string(&npm_log).expect("npm argv log");
    assert!(
        argv.lines()
            .any(|line| line == "install -g --no-fund --no-audit @deepseek-ai/dsh@latest"),
        "npm global install argv missing: {argv:?}"
    );
    assert!(package_root.join("package.json").is_file());

    tx.send(Message::Command(Command::Quit)).expect("quit");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !shutdown.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(shutdown.load(Ordering::SeqCst), "controller did not shut down");
}

fn wait_for(
    snapshots: &Arc<Mutex<Vec<Snapshot>>>,
    timeout: Duration,
    mut predicate: impl FnMut(&Snapshot) -> bool,
) -> Option<Snapshot> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let snapshots = snapshots.lock().expect("lock");
        if let Some(snapshot) = snapshots.iter().find(|snapshot| predicate(snapshot)) {
            return Some(snapshot.clone());
        }
        drop(snapshots);
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}
