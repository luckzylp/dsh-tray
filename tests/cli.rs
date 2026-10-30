//! Hermetic black-box tests for the diagnostics CLI.
//!
//! Every test runs the real binary with isolated config/data/home directories and
//! a fake dsh installation (package.json + bin.js + fake node/npm scripts), so
//! no network access, real npm prefix or ~/.dsh is touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_dsh-tray");
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        for sub in ["cfg", "data", "home", "home/.config", "home/.local/share", "cache"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("mkdir");
        }
        Self { dir }
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

    fn config_file(&self) -> PathBuf {
        self.cfg().join("config.json")
    }

    /// Base command with a cleared environment (deterministic, no user config).
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("XDG_DATA_HOME", self.home().join(".local/share"))
            .env("TMPDIR", self.dir.path())
            .env("npm_config_cache", self.dir.path().join("cache"))
            .env("DSH_TRAY_CONFIG_DIR", self.cfg())
            .env("DSH_TRAY_DATA_DIR", self.data())
            .env("PATH", "");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run binary")
    }

    fn run_offline(&self, args: &[&str]) -> Output {
        let mut command = self.command(args);
        command.env("DSH_TRAY_OFFLINE", "1");
        command.output().expect("run binary")
    }

    /// A fake dsh installation written into this sandbox; returns its paths.
    fn fake_install(&self) -> (PathBuf, PathBuf, PathBuf) {
        let package_root = self.dir.path().join("node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        std::fs::write(package_root.join("lib/bin.js"), b"// fake\n").expect("bin.js");
        std::fs::write(
            package_root.join("package.json"),
            br#"{"name":"@deepseek-ai/dsh","version":"9.9.9-test"}"#,
        )
        .expect("package.json");
        let node = self.dir.path().join("fake-node.sh");
        write_executable(&node, "#!/bin/sh\nexit 0\n");
        let npm = self.dir.path().join("fake-npm.sh");
        write_executable(&npm, "#!/bin/sh\necho https://registry.npmjs.org\n");
        (package_root.join("lib/bin.js"), node, npm)
    }

    fn write_config(&self, body: &str) {
        std::fs::write(self.config_file(), body).expect("write config.json");
    }

    fn config_for_fake_install(&self) -> (String, PathBuf) {
        let (bin_js, node, npm) = self.fake_install();
        let config = format!(
            "{{\"dsh_path\":{:?},\"node_path\":{:?},\"npm_path\":{:?},\"registry\":\"http://127.0.0.1:9\",\"start_dsh_on_launch\":false}}",
            bin_js.display().to_string(),
            node.display().to_string(),
            npm.display().to_string()
        );
        self.write_config(&config);
        (config, bin_js)
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> serde_json::Value {
    let text = stdout(output);
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {text}"))
}

#[test]
fn version_flag_prints_the_package_version() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--version"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(text.trim(), format!("dsh-tray {VERSION}"));
}

#[test]
fn help_flag_lists_the_diagnostic_actions() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--help"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    for expected in ["USAGE", "--check", "--smoke", "--hidden", "--version"] {
        assert!(text.contains(expected), "help is missing {expected:?}: {text}");
    }
}

#[test]
fn unknown_argument_exits_2() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--bogus"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("unknown argument"), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
}

#[test]
fn json_alone_exits_2_with_a_hint() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--json"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("only valid together with --check"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn conflicting_actions_exit_2() {
    let sandbox = Sandbox::new();
    for args in [
        vec!["--check", "--smoke"],
        vec!["--hidden", "--check"],
        vec!["--version", "--help"],
        vec!["--check", "--check"],
    ] {
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(2), "args {args:?}: {}", stderr(&output));
        assert!(
            stderr(&output).contains("conflicting arguments"),
            "args {args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn check_json_is_hermetic_and_describes_the_fake_install() {
    let sandbox = Sandbox::new();
    sandbox.config_for_fake_install();
    let output = sandbox.run_offline(&["--check", "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = json(&output);
    assert_eq!(report["tray_version"], VERSION);
    assert_eq!(report["channel"], "latest");
    assert_eq!(report["dsh"]["found"], true);
    assert_eq!(report["dsh"]["version"], "9.9.9-test");
    assert_eq!(report["installed_version"], "9.9.9-test");
    assert_eq!(report["npm"]["found"], true);
    assert_eq!(report["npm"]["registry"], "http://127.0.0.1:9");
    assert_eq!(report["config_exists"], true);
    assert_eq!(report["config_warning"], serde_json::Value::Null);
    assert_eq!(report["update_available"], serde_json::Value::Null);
    assert_eq!(report["running"]["url"], serde_json::Value::Null);
    assert_eq!(report["running"]["pid"], serde_json::Value::Null);
    let offline = report["dist_tags"]["error"].as_str().unwrap_or_default();
    assert!(offline.contains("DSH_TRAY_OFFLINE"), "{offline}");
    assert_eq!(
        report["log_file"],
        sandbox.data().join("logs/dsh-tray.log").display().to_string()
    );
    // The report must be pure: no log file may be created by --check.
    assert!(!sandbox.data().join("logs/dsh-tray.log").exists());
    // No user-visible files are created by the fake run either.
    assert!(!sandbox.home().join(".dsh").exists());
}

#[test]
fn check_human_output_matches_the_json_report() {
    let sandbox = Sandbox::new();
    sandbox.config_for_fake_install();
    let output = sandbox.run_offline(&["--check"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    for expected in [
        "diagnostics",
        "dsh           9.9.9-test",
        "channel       latest (installed 9.9.9-test, target ?)",
        "autostart     disabled",
        "running       no",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn check_without_node_or_npm_exits_1_with_actionable_advice() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--check"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("dsh is not usable"), "{message}");
    assert!(message.contains("npm not found on PATH"), "{message}");
    assert!(message.contains("nodejs.org"), "{message}");
    assert!(message.contains("npm_path"), "{message}");
}

#[test]
fn check_json_without_node_or_npm_still_prints_the_report_and_exits_1() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--check", "--json"]);
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    let report = json(&output);
    assert_eq!(report["dsh"]["found"], false);
    assert!(report["dsh"]["error"]
        .as_str()
        .unwrap_or_default()
        .contains("npm not found"));
    assert_eq!(report["installed_version"], serde_json::Value::Null);
    assert_eq!(report["channel_target"], serde_json::Value::Null);
    assert!(stderr(&output).contains("dsh is not usable"), "{}", stderr(&output));
}

#[test]
fn check_recovers_from_a_corrupt_config_and_reports_it() {
    let sandbox = Sandbox::new();
    sandbox.write_config("{ this is not json");
    let output = sandbox.run_offline(&["--check", "--json"]);
    let report = json(&output);
    assert_eq!(report["config_exists"], true);
    let warning = report["config_warning"].as_str().unwrap_or_default();
    assert!(warning.contains("could not parse config"), "{warning}");
    // Defaults are used, so the report still renders in full.
    assert_eq!(report["channel"], "latest");
    assert_eq!(report["preferred_port"], 0);
    assert_eq!(report["host"], "127.0.0.1");
}

#[test]
fn check_repairs_invalid_config_values_and_reports_it() {
    let sandbox = Sandbox::new();
    let (bin_js, node, npm) = sandbox.fake_install();
    let config = format!(
        "{{\"dsh_path\":{:?},\"node_path\":{:?},\"npm_path\":{:?},\"registry\":\"ftp://invalid\",\"host\":\"0.0.0.0\",\"update_check_hours\":0}}",
        bin_js.display().to_string(),
        node.display().to_string(),
        npm.display().to_string()
    );
    sandbox.write_config(&config);
    let output = sandbox.run_offline(&["--check", "--json"]);
    let report = json(&output);
    let warning = report["config_warning"].as_str().unwrap_or_default();
    assert!(warning.contains("wildcard"), "{warning}");
    assert!(warning.contains("invalid registry"), "{warning}");
    assert!(warning.contains("clamped"), "{warning}");
    assert_eq!(report["host"], "127.0.0.1");
}

#[cfg(unix)]
#[test]
fn autostart_path_is_hermetic_and_platform_correct() {
    let sandbox = Sandbox::new();
    sandbox.config_for_fake_install();
    let output = sandbox.run_offline(&["--check", "--json"]);
    let report = json(&output);
    let path = report["autostart"]["path"].as_str().unwrap_or_default();
    assert_eq!(report["autostart"]["enabled"], false);
    let expected = if cfg!(target_os = "macos") {
        sandbox.home().join("Library/LaunchAgents/ai.deepseek.dsh-tray.plist")
    } else {
        sandbox.home().join(".config/autostart/ai.deepseek.dsh-tray.desktop")
    };
    assert_eq!(path, expected.display().to_string());
    assert!(!expected.exists(), "the check must not enable autostart");
}
