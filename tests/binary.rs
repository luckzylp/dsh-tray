//! Black-box tests for the tray binary: single-instance handoff, graceful
//! shutdown/orphan cleanup, and corrupt-config recovery.
//!
//! Linux-only and skipped when the session has no display (CI), because the tao
//! event loop needs GTK. Each test uses isolated config/data/home directories and
//! a fake dsh installation, so nothing outside the tempdir is touched.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_dsh-tray");

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

struct Sandbox {
    dir: tempfile::TempDir,
    pidfile: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        for sub in ["cfg", "data", "home/.config", "home/.local/share"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("mkdir");
        }
        Self {
            pidfile: dir.path().join("child-pids.txt"),
            dir,
        }
    }

    fn cfg(&self) -> PathBuf {
        self.dir.path().join("cfg")
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn log_file(&self) -> PathBuf {
        self.data().join("logs/dsh-tray.log")
    }

    fn runtime_file(&self) -> PathBuf {
        self.data().join("runtime.json")
    }

    /// A fake dsh installation whose "node" prints the readiness URL, records
    /// its pid in `self.pidfile` and sleeps until it is signalled.
    fn fake_install(&self) -> (PathBuf, PathBuf, PathBuf) {
        let package_root = self.dir.path().join("pkg");
        std::fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        std::fs::write(package_root.join("lib/bin.js"), b"// fake\n").expect("bin.js");
        std::fs::write(
            package_root.join("package.json"),
            br#"{"name":"@deepseek-ai/dsh","version":"9.9.9-test"}"#,
        )
        .expect("package.json");
        let node = self.dir.path().join("fake-node.sh");
        let body = format!(
            concat!(
                "#!/bin/sh\n",
                "echo \"$$\" >> {pidfile}\n",
                "echo 'dsh web: http://127.0.0.1:39999/?token=test-token'\n",
                "trap 'exit 0' TERM INT\n",
                "while :; do sleep 0.05; done\n",
            ),
            pidfile = self.pidfile.display()
        );
        write_executable(&node, &body);
        let npm = self.dir.path().join("fake-npm.sh");
        write_executable(&npm, "#!/bin/sh\nexit 1\n");
        (package_root.join("lib/bin.js"), node, npm)
    }

    fn write_config(&self, start_dsh_on_launch: bool) {
        let (bin_js, node, npm) = self.fake_install();
        let config = format!(
            "{{\"dsh_path\":{:?},\"node_path\":{:?},\"npm_path\":{:?},\"registry\":\"http://127.0.0.1:9\",\"start_dsh_on_launch\":{},\"open_browser_when_ready\":false,\"update_check_hours\":720}}",
            bin_js.display().to_string(),
            node.display().to_string(),
            npm.display().to_string(),
            start_dsh_on_launch
        );
        std::fs::write(self.cfg().join("config.json"), config).expect("config.json");
    }

    fn command(&self, extra_path: bool) -> Command {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("XDG_DATA_HOME", self.home().join(".local/share"))
            .env("DSH_TRAY_CONFIG_DIR", self.cfg())
            .env("DSH_TRAY_DATA_DIR", self.data());
        if let Some(display) = std::env::var_os("DISPLAY") {
            command.env("DISPLAY", display);
        }
        if let Some(display) = std::env::var_os("WAYLAND_DISPLAY") {
            command.env("WAYLAND_DISPLAY", display);
        }
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            command.env("XDG_RUNTIME_DIR", runtime);
        }
        if let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
            command.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        if extra_path {
            if let Some(path) = std::env::var_os("PATH") {
                command.env("PATH", path);
            }
        } else {
            command.env("PATH", "");
        }
        command
    }

    fn tray_command(&self, extra_path: bool) -> Command {
        let mut command = self.command(extra_path);
        command.arg("--hidden");
        command
    }

    fn spawn_tray(&self, extra_path: bool) -> Child {
        let mut command = self.tray_command(extra_path);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tray")
    }

    fn wait_for_log(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(self.log_file()) {
                if text.contains(needle) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn wait_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait().expect("try_wait") {
            Some(status) => return Some(status),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

fn interrupt(child: &mut Child) {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill -INT");
    assert!(status.success(), "kill -INT failed");
}

fn alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Start the tray and wait until its event loop created the icon; `None` when
/// the session has no tray host (CI), in which case the test is skipped.
fn start_tray_or_skip(sandbox: &Sandbox, extra_path: bool) -> Option<Child> {
    let mut child = sandbox.spawn_tray(extra_path);
    if sandbox.wait_for_log("tray icon created", Duration::from_secs(20)) {
        return Some(child);
    }
    if let Some(status) = child.try_wait().expect("try_wait") {
        eprintln!("skipping: the tray exited early ({status}) - no graphical session?");
        return None;
    }
    let _ = child.kill();
    panic!(
        "tray never created its icon; log: {:?}",
        std::fs::read_to_string(sandbox.log_file())
    );
}

#[test]
fn second_launch_hands_over_and_exits_zero() {
    if !has_display() {
        eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY");
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write_config(false);
    let Some(mut primary) = start_tray_or_skip(&sandbox, true) else {
        return;
    };
    // While the primary owns the instance lock, a second launch must exit 0 and
    // say that the app is already running instead of creating another icon.
    let output = sandbox.tray_command(false).output().expect("run second instance");
    assert!(output.status.success(), "second instance exit: {:?}", output.status);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("already running"), "second instance stdout: {text}");
    interrupt(&mut primary);
    let status = wait_exit(&mut primary, Duration::from_secs(25)).expect("primary did not exit on SIGINT");
    assert!(status.success(), "primary exit: {status}");
    assert!(sandbox.log_file().exists());
    let log = std::fs::read_to_string(sandbox.log_file()).expect("log");
    assert!(log.contains("event loop exited with code 0"), "{log}");
}

#[test]
fn quit_stops_the_managed_child_and_clears_the_url() {
    if !has_display() {
        eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY");
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write_config(true);
    let Some(mut tray) = start_tray_or_skip(&sandbox, true) else {
        return;
    };
    assert!(
        sandbox.wait_for_log("dsh web ready at", Duration::from_secs(20)),
        "dsh was never ready; log: {:?}",
        std::fs::read_to_string(sandbox.log_file())
    );
    let pid: i32 = std::fs::read_to_string(&sandbox.pidfile)
        .expect("pidfile")
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .expect("child pid");
    assert!(alive(pid), "fake dsh child {pid} is not alive");
    interrupt(&mut tray);
    let status = wait_exit(&mut tray, Duration::from_secs(25)).expect("tray did not exit on SIGINT");
    assert!(status.success(), "tray exit: {status}");
    // Give the process a moment to be reaped after the tray exited.
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(pid), "child {pid} was orphaned by Quit");
    // A clean shutdown removes the hand-off file; if one remains it must not
    // keep advertising a token URL for a server that is gone.
    if let Ok(runtime) = std::fs::read_to_string(sandbox.runtime_file()) {
        let runtime: serde_json::Value = serde_json::from_str(&runtime).expect("runtime json");
        assert_eq!(runtime["url"], serde_json::Value::Null);
    }
    let log = std::fs::read_to_string(sandbox.log_file()).expect("log");
    assert!(
        log.contains("stopped dsh on shutdown") || log.contains("stopping dsh web"),
        "{log}"
    );
}

#[test]
fn corrupt_config_does_not_prevent_startup() {
    if !has_display() {
        eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY");
        return;
    }
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.cfg().join("config.json"), "{ this is not json").expect("corrupt config");
    // PATH is empty, so discovery fails cleanly instead of launching a real dsh.
    let Some(mut tray) = start_tray_or_skip(&sandbox, false) else {
        return;
    };
    let log = std::fs::read_to_string(sandbox.log_file()).expect("log");
    assert!(
        log.contains("could not parse config"),
        "warning missing from log: {log}"
    );
    interrupt(&mut tray);
    let status = wait_exit(&mut tray, Duration::from_secs(25)).expect("tray did not exit on SIGINT");
    assert!(status.success(), "tray exit: {status}");
}

#[test]
fn check_reports_the_runtime_file_of_a_live_instance() {
    if !has_display() {
        eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY");
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write_config(true);
    let Some(mut tray) = start_tray_or_skip(&sandbox, false) else {
        return;
    };
    assert!(sandbox.wait_for_log("dsh web ready at", Duration::from_secs(20)));
    let output = sandbox
        .command(false)
        .args(["--check", "--json"])
        .output()
        .expect("run --check");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(report["running"]["url"], "http://127.0.0.1:39999/?token=test-token");
    let child_pid: u64 = std::fs::read_to_string(&sandbox.pidfile)
        .expect("pidfile")
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .expect("child pid");
    assert_eq!(
        report["running"]["pid"].as_u64(),
        Some(child_pid),
        "runtime pid must be the dsh child, not the tray"
    );
    interrupt(&mut tray);
    let _ = wait_exit(&mut tray, Duration::from_secs(25));
}
/// SIGTERM (session logout, systemd stop, terminal close) must take the same
/// graceful shutdown path as SIGINT: stop the managed child and clean up state.
#[test]
fn sigterm_stops_the_managed_child() {
    if !has_display() {
        eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY");
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write_config(true);
    let Some(mut tray) = start_tray_or_skip(&sandbox, true) else {
        return;
    };
    assert!(
        sandbox.wait_for_log("dsh web ready at", Duration::from_secs(20)),
        "dsh was never ready"
    );
    let pid: i32 = std::fs::read_to_string(&sandbox.pidfile)
        .expect("pidfile")
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .expect("child pid");
    assert!(alive(pid));
    let status = Command::new("kill")
        .args(["-TERM", &tray.id().to_string()])
        .status()
        .expect("kill -TERM");
    assert!(status.success());
    let _ = wait_exit(&mut tray, Duration::from_secs(25));
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if alive(pid) {
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    }
    assert!(!alive(pid), "child {pid} was orphaned by SIGTERM");
    let log = std::fs::read_to_string(sandbox.log_file()).expect("log");
    assert!(
        log.contains("stopped dsh on shutdown") || log.contains("stopping dsh web"),
        "SIGTERM did not run the shutdown path: {log}"
    );
}
