//! Command-line parsing and non-GUI diagnostics.
//!
//! The default action starts the tray. The extra modes exist for scripts,
//! packaging checks and end-to-end verification:
//! - --check [--json]: print installation/channel/autostart diagnostics.
//! - --smoke: start dsh, wait for its URL, verify HTTP, stop it.

use std::io::{Read as _, Write as _};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::app;
use crate::autostart;
use crate::channel;
use crate::config::{AppPaths, Config};
use crate::dsh::DshInstall;
use crate::i18n::Lang;
use crate::logging;
use crate::npm::NpmClient;
use crate::process::{self, ProcessEvent, ProcessManager, SpawnOptions};

/// What the binary should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Run the tray (default), optionally launched by an autostart entry.
    Tray { hidden: bool },
    /// Print a health/diagnostic report and exit.
    Check { json: bool },
    /// Start dsh, wait for readiness, verify HTTP, stop it, then exit.
    Smoke,
    /// Print the version.
    Version,
    /// Print usage.
    Help,
}

/// Parse CLI arguments (without the executable name).
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Action> {
    let mut action: Option<Action> = None;
    for arg in args {
        if arg == "--json" {
            match action.as_mut() {
                Some(Action::Check { json }) => *json = true,
                _ => bail!("--json is only valid together with --check"),
            }
            continue;
        }
        if action.is_some() {
            bail!("conflicting arguments; pass only one action flag\n\n{}", usage());
        }
        action = Some(match arg.as_str() {
            "-h" | "--help" => Action::Help,
            "-V" | "--version" => Action::Version,
            "--hidden" | "--autostart" => Action::Tray { hidden: true },
            "--check" => Action::Check { json: false },
            "--smoke" => Action::Smoke,
            other => bail!("unknown argument {other:?}\n\n{}", usage()),
        });
    }
    Ok(action.unwrap_or(Action::Tray { hidden: false }))
}

/// Usage text.
pub fn usage() -> &'static str {
    concat!(
        "dsh-tray ",
        env!("CARGO_PKG_VERSION"),
        "\n\n",
        "USAGE:\n",
        "    dsh-tray                 run the system tray (default)\n",
        "    dsh-tray --hidden        run the tray as a login item (no browser auto-open)\n",
        "    dsh-tray --check         print installation / channel / autostart diagnostics\n",
        "    dsh-tray --check --json  same, as JSON\n",
        "    dsh-tray --smoke         start dsh, verify the web UI responds, stop it\n",
        "    dsh-tray --version       print the tray version\n",
        "    dsh-tray --help          show this help\n\n",
        "The tray is the only UI: use its menu to start/stop dsh, open the web UI,\n",
        "inspect versions, switch npm channels (latest/next/alpha) and update dsh.\n"
    )
}

/// Run the diagnostics report. Returns an error when dsh itself is not usable.
pub fn run_check(as_json: bool) -> Result<()> {
    let paths = AppPaths::resolve();
    let config_path = paths.config_file();
    let (mut config, config_warning) = Config::load(&config_path);
    if let Some(warning) = config.normalize() {
        logging::warn(format!("config: {warning}"));
    }
    let lang = Lang::resolve(config.language);

    let dsh = DshInstall::discover(&config);
    // Prefer the npm that belongs to the Node installation containing dsh;
    // fall back to PATH/known-npm discovery when dsh is missing.
    let npm = match &dsh {
        Ok(install) => {
            NpmClient::discover_with_program(&config, install.npm.clone()).or_else(|_| NpmClient::discover(&config))
        }
        Err(_) => NpmClient::discover(&config),
    };
    let offline = std::env::var_os("DSH_TRAY_OFFLINE").is_some();
    let tags = if offline {
        Err("skipped (DSH_TRAY_OFFLINE is set)".to_string())
    } else {
        match &npm {
            Ok(client) => client.dist_tags().map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        }
    };
    let autostart = autostart::status();
    let runtime = crate::single_instance::read_runtime(&paths.runtime_file());

    let installed = dsh.as_ref().ok().map(|install| install.version.clone());
    let (current_target, update) = match (&installed, &tags) {
        (Some(_), Ok(tags)) => {
            let target = channel::tag_version(tags, config.channel).map(str::to_string);
            let update = match (&installed, &target) {
                (Some(installed), Some(target)) => {
                    match (semver::Version::parse(installed), semver::Version::parse(target)) {
                        (Ok(installed), Ok(target)) => (target > installed).then(|| target.to_string()),
                        _ => None,
                    }
                }
                _ => None,
            };
            (target, update)
        }
        _ => (None, None),
    };

    let report = json!({
        "tray_version": crate::VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "language": lang.code(),
        "config_path": config_path.display().to_string(),
        "config_exists": config_path.exists(),
        "config_warning": config_warning,
        "data_dir": paths.data_dir.display().to_string(),
        "log_file": paths.log_file().display().to_string(),
        "channel": config.channel.tag(),
        "preferred_port": config.port,
        "host": config.host,
        "dsh": match &dsh {
            Ok(install) => json!({
                "found": true,
                "version": install.version,
                "bin_js": install.bin_js.display().to_string(),
                "package_root": install.package_root.display().to_string(),
                "node": install.node.display().to_string(),
                "npm": install.npm.display().to_string(),
            }),
            Err(error) => json!({ "found": false, "error": error.to_string() }),
        },
        "npm": match &npm {
            Ok(client) => json!({
                "found": true,
                "program": client.program.display().to_string(),
                "registry": client.registry,
            }),
            Err(error) => json!({ "found": false, "error": error.to_string() }),
        },
        "dist_tags": match &tags {
            Ok(tags) => json!(tags),
            Err(error) => json!({ "error": error }),
        },
        "installed_version": installed,
        "channel_target": current_target,
        "update_available": update,
        "autostart": {
            "enabled": autostart.enabled,
            "path": autostart.path.display().to_string(),
        },
        "running": {
            "url": runtime.as_ref().and_then(|state| state.url.clone()),
            "pid": runtime.as_ref().map(|state| state.pid),
        },
    });

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let dsh_line = match &report["dsh"] {
            Value::Object(_) if report["dsh"]["found"] == json!(true) => format!(
                "{} ({})",
                report["dsh"]["version"].as_str().unwrap_or("?"),
                report["dsh"]["bin_js"].as_str().unwrap_or("?")
            ),
            _ => format!(
                "not found: {}",
                report["dsh"]["error"].as_str().unwrap_or("unknown error")
            ),
        };
        let npm_line = match &report["npm"] {
            Value::Object(_) if report["npm"]["found"] == json!(true) => format!(
                "{} (registry {})",
                report["npm"]["program"].as_str().unwrap_or("?"),
                report["npm"]["registry"].as_str().unwrap_or("?")
            ),
            _ => format!(
                "not found: {}",
                report["npm"]["error"].as_str().unwrap_or("unknown error")
            ),
        };
        let tags_line = match &tags {
            Ok(tags) => channel::tags_summary(tags),
            Err(error) => format!("unavailable: {error}"),
        };
        let update_line = update
            .as_deref()
            .map(|version| format!(" (update available: {version})"))
            .unwrap_or_default();
        let running_line = runtime
            .as_ref()
            .and_then(|state| state.url.as_deref())
            .map(process::display_url)
            .unwrap_or_else(|| "no".to_string());
        println!("dsh-tray {} diagnostics", crate::VERSION);
        println!("  os            {} ({})", std::env::consts::OS, std::env::consts::ARCH);
        println!(
            "  config        {} (exists: {})",
            config_path.display(),
            config_path.exists()
        );
        if let Some(warning) = &config_warning {
            println!("  config issue  {warning}");
        }
        println!("  data          {}", paths.data_dir.display());
        println!("  log           {}", paths.log_file().display());
        println!("  language      {}", lang.code());
        println!("  dsh           {dsh_line}");
        println!("  npm           {npm_line}");
        println!(
            "  channel       {} (installed {}, target {}){update_line}",
            config.channel,
            installed.as_deref().unwrap_or("?"),
            current_target.as_deref().unwrap_or("?")
        );
        println!("  dist-tags     {tags_line}");
        println!(
            "  autostart     {} ({})",
            if autostart.enabled { "enabled" } else { "disabled" },
            autostart.path.display()
        );
        println!("  running       {running_line}");
    }

    match dsh {
        Ok(_) => Ok(()),
        Err(error) => Err(anyhow::Error::msg(format!("dsh is not usable: {error}"))),
    }
}

/// Start dsh, wait for the printed URL, verify HTTP, then stop it.
pub fn run_smoke() -> Result<()> {
    let paths = AppPaths::resolve();
    paths.ensure()?;
    let _ = logging::init(&paths.log_file());
    let (mut config, warning) = Config::load(&paths.config_file());
    if let Some(warning) = warning {
        logging::warn(format!("config: {warning}"));
    }
    if let Some(warning) = config.normalize() {
        logging::warn(format!("config: {warning}"));
    }
    let install = DshInstall::discover(&config).context("discover dsh")?;
    println!("dsh         {} ({})", install.version, install.bin_js.display());
    println!("log         {}", paths.web_log_file().display());
    let mut manager = ProcessManager::new(install);
    let (tx, rx) = mpsc::channel();
    let sink: Box<dyn Fn(ProcessEvent) + Send + Sync> = Box::new(move |event| {
        let _ = tx.send(event);
    });
    let options = SpawnOptions {
        host: config.host.clone(),
        port: 0,
        log_path: paths.web_log_file(),
    };
    let pid = manager.spawn(&options, sink).context("spawn dsh web")?;
    println!("spawned     pid {pid}; waiting for the URL line (max 60s)");

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut url = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
            Ok(ProcessEvent::Ready { url: ready_url, .. }) => {
                url = Some(ready_url);
                break;
            }
            Ok(ProcessEvent::StartupFailed { message, .. }) => {
                let _ = manager.stop(app::STOP_GRACE);
                bail!("dsh failed to start: {message}");
            }
            Ok(ProcessEvent::Exited { code, .. }) => {
                let _ = manager.stop(Duration::from_secs(1));
                bail!("dsh exited before it announced a URL (code {code:?})");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = manager.stop(Duration::from_secs(1));
                bail!("dsh process event channel closed unexpectedly");
            }
        }
    }
    let Some(url) = url else {
        let _ = manager.stop(Duration::from_secs(2));
        bail!(
            "timed out waiting for the dsh web URL line; see {}",
            paths.web_log_file().display()
        );
    };
    println!("ready       {}", process::display_url(&url));

    let status = probe_http(&url)?;
    println!("http        {status} (server responded; token URLs may redirect or reject anonymous requests)");
    manager.stop(app::STOP_GRACE).context("stop dsh web")?;
    println!("stopped     dsh web");
    Ok(())
}

/// Minimal HTTP probe: accept any status below 500.
///
/// The authenticated dsh URL is token-bearing and normally answers with a
/// redirect (or a 401 when the browser cookie is missing); a status line is
/// enough proof that the web server is listening. Avoids depending on a
/// high-level client's redirect/status policy for the smoke test.
fn probe_http(url: &str) -> Result<u16> {
    let rest = url
        .strip_prefix("http://")
        .context("--smoke only probes http:// URLs")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let addr = authority
        .to_socket_addrs()
        .with_context(|| format!("resolve {authority}"))?
        .next()
        .context("the dsh web URL resolved to no address")?;
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).with_context(|| format!("connect to {addr}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while buffer.len() < 4096 {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    let status_line = text.lines().next().context("empty HTTP response")?;
    let code = status_line
        .split_whitespace()
        .nth(1)
        .context("malformed HTTP status line")?;
    let code: u16 = code.parse().context("non-numeric HTTP status code")?;
    if code >= 500 {
        bail!("dsh web returned HTTP {code}");
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn defaults_to_tray() {
        assert_eq!(parse(args(&[])).unwrap(), Action::Tray { hidden: false });
    }

    #[test]
    fn parses_hidden_and_diagnostics() {
        assert_eq!(parse(args(&["--hidden"])).unwrap(), Action::Tray { hidden: true });
        assert_eq!(parse(args(&["--check"])).unwrap(), Action::Check { json: false });
        assert_eq!(
            parse(args(&["--check", "--json"])).unwrap(),
            Action::Check { json: true }
        );
        assert_eq!(parse(args(&["--smoke"])).unwrap(), Action::Smoke);
        assert_eq!(parse(args(&["--version"])).unwrap(), Action::Version);
        assert_eq!(parse(args(&["--help"])).unwrap(), Action::Help);
    }

    #[test]
    fn rejects_conflicts_and_unknown_flags() {
        assert!(parse(args(&["--check", "--smoke"])).is_err());
        assert!(parse(args(&["--hidden", "--check"])).is_err());
        assert!(parse(args(&["--json"])).is_err());
        assert!(parse(args(&["--nope"])).is_err());
    }
}
