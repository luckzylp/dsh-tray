//! npm registry queries and global installs.
//!
//! Two responsibilities:
//!
//! * resolve the npm program and the registry to query (config, environment,
//!   npm itself, with the public registry as the last resort);
//! * fetch the dsh dist-tags from the registry and perform "npm install -g"
//!   with streamed output.
//!
//! Every network and process operation is bounded; failures are returned as
//! errors that include the URL or exit code. Nothing here panics on bad input.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::channel::{Channel, DistTags};
use crate::config::Config;
use crate::dsh;
use crate::logging;

/// Public npm registry, used when nothing else is configured.
const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";
/// Total timeout for one registry HTTP request.
const DIST_TAGS_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout for "npm config get registry".
const REGISTRY_CONFIG_TIMEOUT: Duration = Duration::from_secs(8);
/// Hard cap for a global install (it normally takes seconds, not minutes).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long to wait for the child after its output streams closed.
const INSTALL_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Response body cap for registry requests (a packument can be large).
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Cap for one output line forwarded to the log callback.
const MAX_LINE_CHARS: usize = 4096;
/// How many trailing output lines are kept for the error message.
const OUTPUT_TAIL: usize = 20;
/// npm's abbreviated packument media type.
const REGISTRY_ACCEPT: &str = "application/vnd.npm.install-v1+json";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A resolved npm executable + registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmClient {
    /// npm / npm.cmd path.
    pub program: PathBuf,
    /// Registry base URL without trailing slash.
    pub registry: String,
}

impl NpmClient {
    /// Resolve npm and the registry to query.
    ///
    /// The registry is resolved once here and cached in the client; later
    /// dist-tags calls never shell out to npm again.
    pub fn discover(config: &Config) -> Result<Self> {
        let program = resolve_program(config)?;
        let registry = resolve_registry(&program, config)?;
        Ok(Self { program, registry })
    }

    /// Fetch the package dist-tags map.
    ///
    /// Tries the dedicated dist-tags endpoint first and falls back to the
    /// abbreviated packument (Accept header), which some registries/proxies
    /// only expose.
    pub fn dist_tags(&self) -> Result<DistTags> {
        let primary = build_dist_tags_url(&self.registry, crate::DSH_PACKAGE);
        match fetch_json(&primary, None) {
            Ok((status, body)) => {
                if (200..300).contains(&status) {
                    match parse_dist_tags_json(&body) {
                        Ok(tags) => return validate_tags(tags, &primary),
                        Err(error) => logging::warn(format!(
                            "dist-tags endpoint {primary} returned unusable JSON ({error:#}); trying the packument"
                        )),
                    }
                } else {
                    logging::warn(format!(
                        "dist-tags endpoint {primary} returned HTTP {status}; trying the packument"
                    ));
                }
            }
            Err(error) => logging::warn(format!(
                "dist-tags request to {primary} failed ({error:#}); trying the packument"
            )),
        }

        let fallback = build_packument_url(&self.registry, crate::DSH_PACKAGE);
        let (status, body) = fetch_json(&fallback, Some(REGISTRY_ACCEPT))
            .map_err(|error| anyhow!("could not reach the npm registry at {fallback}: {error:#}"))?;
        if !(200..300).contains(&status) {
            bail!("npm registry returned HTTP {status} for {fallback}");
        }
        let tags = parse_packument_json(&body).with_context(|| format!("could not read dist-tags from {fallback}"))?;
        validate_tags(tags, &fallback)
    }

    /// Install the latest version of channel globally, streaming output lines.
    pub fn install(&self, channel: Channel, log: &mut dyn FnMut(&str)) -> Result<()> {
        let args = install_args(channel);
        log(&format!("running {} {}", self.program.display(), args.join(" ")));

        let mut command = npm_command(&self.program, &args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().with_context(|| {
            format!(
                "could not run npm ({}) to install {}@{}",
                self.program.display(),
                crate::DSH_PACKAGE,
                channel.tag()
            )
        })?;

        let (sender, receiver) = mpsc::channel::<String>();
        let mut readers: Vec<JoinHandle<()>> = Vec::new();
        if let Some(stream) = child.stdout.take() {
            readers.push(spawn_line_thread(stream, sender.clone()));
        }
        if let Some(stream) = child.stderr.take() {
            readers.push(spawn_line_thread(stream, sender.clone()));
        }
        drop(sender);

        let mut tail: VecDeque<String> = VecDeque::with_capacity(OUTPUT_TAIL);
        let started = Instant::now();
        let exit_code: Option<i32> = loop {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    log(&line);
                    push_tail(&mut tail, line);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => match wait_for_exit(&mut child, INSTALL_DRAIN_TIMEOUT) {
                    Some(code) => break code,
                    None => bail!(
                        "npm install stopped producing output but did not exit; it was killed. \
                             Run 'npm install -g {}@{}' manually to see what happens",
                        crate::DSH_PACKAGE,
                        channel.tag()
                    ),
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if started.elapsed() >= INSTALL_TIMEOUT {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!(
                            "npm install timed out after {}s; check the network and run \
                             'npm install -g {}@{}' manually",
                            INSTALL_TIMEOUT.as_secs(),
                            crate::DSH_PACKAGE,
                            channel.tag()
                        );
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            // The child is gone; forward whatever is still buffered.
                            while let Ok(line) = receiver.recv_timeout(Duration::from_millis(200)) {
                                log(&line);
                                push_tail(&mut tail, line);
                            }
                            break status.code();
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            bail!("waiting for npm install failed: {error}");
                        }
                    }
                }
            }
        };

        for reader in readers {
            let _ = reader.join();
        }

        match exit_code {
            Some(0) => {
                log(&format!("installed {}@{}", crate::DSH_PACKAGE, channel.tag()));
                Ok(())
            }
            Some(code) => Err(install_error(code, channel, &tail)),
            None => Err(anyhow!(
                "npm install -g {}@{} finished without an exit code",
                crate::DSH_PACKAGE,
                channel.tag()
            )),
        }
    }
}

/// Resolve the npm program: config override, then PATH.
fn resolve_program(config: &Config) -> Result<PathBuf> {
    if let Some(configured) = config.npm_path.as_deref() {
        if !configured.is_file() {
            bail!("npm_path {} does not exist", configured.display());
        }
        return Ok(configured.to_path_buf());
    }
    dsh::find_in_path("npm").ok_or_else(|| {
        anyhow!(
            "npm not found on PATH; install Node.js from https://nodejs.org/ \
             or set npm_path in the config"
        )
    })
}

/// Resolve the registry with the documented precedence.
fn resolve_registry(program: &Path, config: &Config) -> Result<String> {
    let env_value = std::env::var("npm_config_registry")
        .ok()
        .or_else(|| std::env::var("NPM_CONFIG_REGISTRY").ok());
    let npm_value = if config.registry.is_none() {
        npm_config_registry(program)
    } else {
        None
    };
    pick_registry(config.registry.as_deref(), env_value.as_deref(), npm_value.as_deref())
}

/// Pure registry precedence: config > env > 'npm config get registry' > default.
fn pick_registry(config_value: Option<&str>, env_value: Option<&str>, npm_value: Option<&str>) -> Result<String> {
    if let Some(value) = config_value {
        return normalize_registry(value).ok_or_else(|| anyhow!("registry {value:?} is not an http(s) URL"));
    }
    if let Some(normalized) = env_value.and_then(normalize_registry) {
        return Ok(normalized);
    }
    if env_value.is_some() {
        logging::warn("ignoring npm_config_registry: it is not an http(s) URL");
    }
    if let Some(normalized) = npm_value.and_then(normalize_registry) {
        return Ok(normalized);
    }
    if npm_value.is_some() {
        logging::warn("ignoring 'npm config get registry': it is not an http(s) URL");
    }
    Ok(DEFAULT_REGISTRY.to_string())
}

/// Ask npm which registry it would use (bounded; cached by the caller).
fn npm_config_registry(program: &Path) -> Option<String> {
    let captured = dsh::run_with_timeout(program, ["config", "get", "registry"], REGISTRY_CONFIG_TIMEOUT).ok()?;
    if captured.timed_out || !captured.success {
        return None;
    }
    captured
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Normalise a registry URL: trim, strip trailing slashes, require http(s).
fn normalize_registry(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    let scheme = if lower.starts_with("https://") {
        "https"
    } else if lower.starts_with("http://") {
        "http"
    } else {
        return None;
    };
    let rest = trimmed[scheme.len() + 3..].trim_end_matches('/');
    if rest.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

/// URL of the registry dist-tags endpoint (scoped package slash encoded).
fn build_dist_tags_url(registry: &str, package: &str) -> String {
    format!(
        "{}/-/package/{}/dist-tags",
        registry.trim_end_matches('/'),
        encode_package(package)
    )
}

/// URL of the package packument (scoped package slash encoded).
fn build_packument_url(registry: &str, package: &str) -> String {
    format!("{}/{}", registry.trim_end_matches('/'), encode_package(package))
}

/// Encode a package name for a registry URL path: @scope/name -> @scope%2Fname.
fn encode_package(package: &str) -> String {
    package.replace('/', "%2F")
}

/// GET a registry URL and return (status, body). Transport failures are errors.
fn fetch_json(url: &str, accept: Option<&str>) -> Result<(u16, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(DIST_TAGS_TIMEOUT))
        .build()
        .into();
    let mut request = agent.get(url);
    if let Some(accept) = accept {
        request = request.header("Accept", accept);
    }
    let mut response = request.call().with_context(|| format!("GET {url}"))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .with_context(|| format!("read the response body of {url}"))?;
    Ok((status, body))
}

/// Parse the dedicated dist-tags endpoint response.
fn parse_dist_tags_json(text: &str) -> Result<DistTags> {
    let value: Value = serde_json::from_str(text).context("parse JSON")?;
    dist_tags_from_value(&value, "response")
}

/// Parse the dist-tags object out of a packument.
fn parse_packument_json(text: &str) -> Result<DistTags> {
    let value: Value = serde_json::from_str(text).context("parse JSON")?;
    let tags = value
        .get("dist-tags")
        .ok_or_else(|| anyhow!("response has no 'dist-tags' object"))?;
    dist_tags_from_value(tags, "'dist-tags'")
}

/// Extract a string map of tag -> version, ignoring malformed entries.
fn dist_tags_from_value(value: &Value, label: &str) -> Result<DistTags> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("{label} is not a JSON object"))?;
    let mut tags = DistTags::new();
    for (tag, version) in object {
        let tag = tag.trim().to_ascii_lowercase();
        let Some(version) = version.as_str() else { continue };
        let version = version.trim();
        if tag.is_empty() || version.is_empty() {
            continue;
        }
        tags.insert(tag, version.to_string());
    }
    Ok(tags)
}

/// Reject responses that cannot be used to resolve the latest channel.
fn validate_tags(mut tags: DistTags, source: &str) -> Result<DistTags> {
    tags.retain(|tag, version| !tag.is_empty() && !version.is_empty());
    let latest_usable = tags.get("latest").map(|version| !version.is_empty()).unwrap_or(false);
    if !latest_usable {
        bail!("registry response from {source} has no usable 'latest' dist-tag");
    }
    Ok(tags)
}

/// npm argv for a global install of a channel.
fn install_args(channel: Channel) -> Vec<String> {
    vec![
        "install".to_string(),
        "-g".to_string(),
        "--no-fund".to_string(),
        "--no-audit".to_string(),
        format!("{}@{}", crate::DSH_PACKAGE, channel.tag()),
    ]
}

/// Build the install command (Windows must go through cmd.exe for npm.cmd).
fn npm_command(program: &Path, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program.as_os_str()).args(args);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(program);
        command.args(args);
        command
    }
}

/// Forward child output lines to the log callback from a helper thread.
fn spawn_line_thread<R>(stream: R, sender: mpsc::Sender<String>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let line = truncate_line(line.trim_end_matches('\r'));
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Clamp one output line so a pathological child cannot balloon the log.
fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{truncated}...")
}

/// Keep only the last OUTPUT_TAIL lines.
fn push_tail(tail: &mut VecDeque<String>, line: String) {
    if tail.len() >= OUTPUT_TAIL {
        tail.pop_front();
    }
    tail.push_back(line);
}

/// Wait for the child to exit, killing it after timeout.
///
/// Returns None when the child had to be killed or the wait failed.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<Option<i32>> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code()),
            Ok(None) => {}
            Err(_) => return None,
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Build the error returned for a failed npm install, with a permission hint.
fn install_error(code: i32, channel: Channel, tail: &VecDeque<String>) -> anyhow::Error {
    let output = tail.iter().cloned().collect::<Vec<_>>().join("\n");
    let lower = output.to_ascii_lowercase();
    let hint = if lower.contains("eacces") || lower.contains("eperm") || lower.contains("permission denied") {
        "\nhint: npm cannot write to its global prefix. Fix it with \
         'npm config set prefix <writable-directory>' (and add that directory's bin to PATH), \
         or reinstall Node.js for the current user."
    } else {
        ""
    };
    anyhow!(
        "npm install -g {}@{} failed with exit code {code}{hint}\n{}",
        crate::DSH_PACKAGE,
        channel.tag(),
        if output.is_empty() {
            "(no output)"
        } else {
            output.as_str()
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_registry_urls() {
        assert_eq!(
            normalize_registry(" https://registry.npmjs.org/ "),
            Some("https://registry.npmjs.org".to_string())
        );
        assert_eq!(
            normalize_registry("http://localhost:4873//"),
            Some("http://localhost:4873".to_string())
        );
        assert_eq!(
            normalize_registry("HTTPS://Registry.Example.test/npm"),
            Some("https://Registry.Example.test/npm".to_string())
        );
        assert_eq!(normalize_registry("ftp://example.test"), None);
        assert_eq!(normalize_registry("registry.npmjs.org"), None);
        assert_eq!(normalize_registry("https://"), None);
        assert_eq!(normalize_registry(""), None);
    }

    #[test]
    fn registry_precedence_is_config_then_env_then_npm_then_default() {
        assert_eq!(
            pick_registry(
                Some("https://config.test/"),
                Some("https://env.test"),
                Some("https://npm.test")
            )
            .expect("config wins"),
            "https://config.test"
        );
        assert_eq!(
            pick_registry(None, Some("https://env.test/"), Some("https://npm.test")).expect("env wins"),
            "https://env.test"
        );
        assert_eq!(
            pick_registry(None, None, Some("https://npm.test/")).expect("npm wins"),
            "https://npm.test"
        );
        assert_eq!(pick_registry(None, None, None).expect("default"), DEFAULT_REGISTRY);
    }

    #[test]
    fn registry_invalid_config_is_an_error() {
        let error = pick_registry(Some("not-a-url"), None, None).expect_err("must fail");
        assert!(format!("{error}").contains("http(s)"), "{error}");
    }

    #[test]
    fn registry_invalid_env_and_npm_output_are_ignored() {
        assert_eq!(
            pick_registry(None, Some("ftp://env.test"), Some("garbage")).expect("fallback"),
            DEFAULT_REGISTRY
        );
        assert_eq!(
            pick_registry(None, Some("ftp://env.test"), Some("https://npm.test")).expect("npm"),
            "https://npm.test"
        );
    }

    #[test]
    fn builds_registry_urls() {
        assert_eq!(
            build_dist_tags_url("https://registry.npmjs.org/", "@deepseek-ai/dsh"),
            "https://registry.npmjs.org/-/package/@deepseek-ai%2Fdsh/dist-tags"
        );
        assert_eq!(
            build_packument_url("https://registry.npmjs.org", "@deepseek-ai/dsh"),
            "https://registry.npmjs.org/@deepseek-ai%2Fdsh"
        );
        assert_eq!(
            build_packument_url("http://localhost:4873", "plain"),
            "http://localhost:4873/plain"
        );
    }

    #[test]
    fn parses_dist_tags_response() {
        let tags = parse_dist_tags_json(r#"{"latest":"0.2.1-rc.5","next":"0.2.1-rc.5","alpha":"0.2.2-alpha.2"}"#)
            .expect("parse");
        assert_eq!(tags.get("latest").map(String::as_str), Some("0.2.1-rc.5"));
        assert_eq!(tags.get("alpha").map(String::as_str), Some("0.2.2-alpha.2"));
    }

    #[test]
    fn parses_packument_response() {
        let tags = parse_packument_json(r#"{"name":"@deepseek-ai/dsh","dist-tags":{"latest":"1.2.3"},"versions":{}}"#)
            .expect("parse");
        assert_eq!(tags.get("latest").map(String::as_str), Some("1.2.3"));
    }

    #[test]
    fn rejects_malformed_registry_json() {
        assert!(parse_dist_tags_json("{ not json").is_err());
        assert!(parse_dist_tags_json("[1,2,3]").is_err());
        assert!(parse_packument_json(r#"{"name":"x"}"#).is_err());
        assert!(parse_packument_json(r#"{"dist-tags":[1]}"#).is_err());
    }

    #[test]
    fn tag_validation_requires_latest() {
        let mut tags = DistTags::new();
        tags.insert("next".to_string(), "0.2.1-rc.5".to_string());
        assert!(validate_tags(tags.clone(), "test").is_err());
        tags.insert("latest".to_string(), "0.2.1".to_string());
        let validated = validate_tags(tags, "test").expect("valid");
        assert_eq!(validated.len(), 2);
    }

    #[test]
    fn install_argv_is_exact() {
        assert_eq!(
            install_args(Channel::Latest),
            vec![
                "install".to_string(),
                "-g".to_string(),
                "--no-fund".to_string(),
                "--no-audit".to_string(),
                "@deepseek-ai/dsh@latest".to_string(),
            ]
        );
        assert_eq!(
            install_args(Channel::Alpha).last().map(String::as_str),
            Some("@deepseek-ai/dsh@alpha")
        );
    }

    #[test]
    fn line_truncation_is_bounded() {
        let short = "x".repeat(MAX_LINE_CHARS);
        assert_eq!(truncate_line(&short), short);
        let long = "y".repeat(MAX_LINE_CHARS + 10);
        let truncated = truncate_line(&long);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.chars().count(), MAX_LINE_CHARS + 3);
    }

    #[test]
    fn tail_keeps_only_recent_lines() {
        let mut tail = VecDeque::new();
        for index in 0..(OUTPUT_TAIL + 5) {
            push_tail(&mut tail, format!("line {index}"));
        }
        assert_eq!(tail.len(), OUTPUT_TAIL);
        assert_eq!(tail.front().map(String::as_str), Some("line 5"));
        assert_eq!(tail.back().map(String::as_str), Some("line 24"));
    }

    #[test]
    fn install_error_mentions_permissions() {
        let mut tail = VecDeque::new();
        push_tail(&mut tail, "npm error code EACCES".to_string());
        push_tail(&mut tail, "npm error syscall mkdir".to_string());
        let error = format!("{}", install_error(243, Channel::Latest, &tail));
        assert!(error.contains("EACCES"));
        assert!(error.contains("hint"));
        assert!(error.contains("@deepseek-ai/dsh@latest"));
    }

    #[test]
    fn install_error_without_permission_problem_has_no_hint() {
        let mut tail = VecDeque::new();
        push_tail(&mut tail, "npm error network down".to_string());
        let error = format!("{}", install_error(1, Channel::Next, &tail));
        assert!(!error.contains("hint"));
        assert!(error.contains("exit code 1"));
    }

    #[test]
    fn discover_uses_config_override_without_running_npm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let npm = dir.path().join("npm");
        std::fs::write(&npm, b"#!/bin/sh\nexit 1\n").expect("write npm");
        let config = Config {
            npm_path: Some(npm),
            registry: Some("https://registry.test/".to_string()),
            ..Config::default()
        };
        let client = NpmClient::discover(&config).expect("discover");
        assert_eq!(client.registry, "https://registry.test");
    }

    #[test]
    fn discover_rejects_missing_npm_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Config {
            npm_path: Some(dir.path().join("missing")),
            ..Config::default()
        };
        let error = NpmClient::discover(&config).expect_err("must fail");
        assert!(format!("{error}").contains("npm_path"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn install_surfaces_nonzero_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let npm = dir.path().join("npm");
        std::fs::write(&npm, b"#!/bin/sh\necho 'npm error boom' >&2\nexit 7\n").expect("write npm");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let client = NpmClient {
            program: npm,
            registry: DEFAULT_REGISTRY.to_string(),
        };
        let mut lines: Vec<String> = Vec::new();
        let error = client
            .install(Channel::Latest, &mut |line| lines.push(line.to_string()))
            .expect_err("must fail");
        let message = format!("{error}");
        assert!(message.contains("exit code 7"), "{message}");
        assert!(message.contains("npm error boom"), "{message}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("running") && line.contains("@deepseek-ai/dsh@latest")),
            "command line must be logged: {lines:?}"
        );
        assert!(lines.iter().any(|line| line.contains("npm error boom")), "{lines:?}");
    }

    #[cfg(unix)]
    #[test]
    fn install_succeeds_and_streams_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let npm = dir.path().join("npm");
        std::fs::write(&npm, b"#!/bin/sh\necho 'added 1 package'\nexit 0\n").expect("write npm");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let client = NpmClient {
            program: npm,
            registry: DEFAULT_REGISTRY.to_string(),
        };
        let mut lines: Vec<String> = Vec::new();
        client
            .install(Channel::Next, &mut |line| lines.push(line.to_string()))
            .expect("install");
        assert!(lines.iter().any(|line| line.contains("added 1 package")), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("installed @deepseek-ai/dsh@next")),
            "{lines:?}"
        );
    }
}
