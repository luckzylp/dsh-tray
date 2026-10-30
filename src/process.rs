//! Lifecycle of the managed "dsh web" child process.
//!
//! The tray starts "node <bin.js> web --no-open --host <ip> --port <port>",
//! reads stdout/stderr line by line, detects the readiness line
//! ("dsh web: <url>"), mirrors every line into the web log, and waits for the
//! child on a background thread.
//!
//! On Unix the child gets its own process group so the whole node tree can be
//! signalled at once (SIGTERM, then SIGKILL after the grace period); on Windows
//! the tree is killed with "taskkill /T /F" and the console window is hidden.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::dsh::DshInstall;
use crate::logging;

/// How often the waiter thread polls the child.
const WAIT_POLL: Duration = Duration::from_millis(100);
/// How long to keep waiting after a forced kill before giving up.
const KILL_GRACE: Duration = Duration::from_secs(2);
/// Cap for one child output line mirrored to the log.
const MAX_LINE_CHARS: usize = 4096;
/// How many stderr lines are attached to a startup failure.
const STDERR_TAIL: usize = 20;
/// How long the waiter lets the output readers drain after the child exited.
const READER_DRAIN: Duration = Duration::from_millis(500);
/// Hide the console window of the child on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Events emitted by the reader/wait threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    /// The web server announced its URL ("dsh web: <url>").
    Ready { pid: u32, url: String },
    /// The child exited.
    Exited { pid: u32, code: Option<i32> },
    /// Spawn/readiness failed before the URL was announced.
    StartupFailed { pid: u32, message: String },
}

/// Options for spawning the web server.
#[derive(Clone, Debug)]
pub struct SpawnOptions {
    pub host: String,
    pub port: u16,
    pub log_path: PathBuf,
}

/// Event sink shared with the reader/waiter threads.
type Sink = Arc<dyn Fn(ProcessEvent) + Send + Sync>;
/// Append handle for the per-start web log.
type LogHandle = Arc<Mutex<File>>;
/// Bounded tail of stderr lines used for startup failure messages.
type StderrTail = Arc<Mutex<VecDeque<String>>>;

/// State shared with the reader/waiter threads.
struct Inner {
    child: Mutex<Option<Child>>,
    pid: u32,
    exited: AtomicBool,
    ready: AtomicBool,
    /// Reader threads that are still draining one of the child's pipes.
    readers: AtomicUsize,
}

/// Owns the child process handle and its event sink.
pub struct ProcessManager {
    install: DshInstall,
    shared: Option<Arc<Inner>>,
    workers: Vec<JoinHandle<()>>,
}

impl ProcessManager {
    pub fn new(install: DshInstall) -> Self {
        Self {
            install,
            shared: None,
            workers: Vec::new(),
        }
    }

    /// Spawn "node bin.js web --no-open ..."; returns the child pid.
    pub fn spawn(&mut self, options: &SpawnOptions, sink: Box<dyn Fn(ProcessEvent) + Send + Sync>) -> Result<u32> {
        if self.is_running() {
            bail!("dsh web is already running (pid {})", self.pid().unwrap_or_default());
        }
        // A previous child has exited; drop its (finished) thread handles.
        self.workers.clear();
        self.shared = None;

        let log_file = open_web_log(&self.install, options);
        let sink: Sink = Arc::from(sink);

        let mut command = Command::new(&self.install.node);
        command
            .arg(&self.install.bin_js)
            .args(["web", "--no-open", "--host", options.host.as_str(), "--port"])
            .arg(options.port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Keep the readiness line free of ANSI decoration.
            .env("NO_COLOR", "1");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // Own process group: the whole node tree can be signalled at once.
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "could not start dsh web ({} {})",
                self.install.node.display(),
                self.install.bin_js.display()
            )
        })?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let reader_count = stdout.is_some() as usize + stderr.is_some() as usize;

        let inner = Arc::new(Inner {
            child: Mutex::new(Some(child)),
            pid,
            exited: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            readers: AtomicUsize::new(reader_count),
        });
        let tail: StderrTail = Arc::new(Mutex::new(VecDeque::new()));

        if let Some(stream) = stdout {
            self.workers.push(spawn_reader(ReaderConfig {
                stream,
                name: "stdout",
                inner: inner.clone(),
                sink: sink.clone(),
                log: log_file.clone(),
                tail: None,
            }));
        }
        if let Some(stream) = stderr {
            self.workers.push(spawn_reader(ReaderConfig {
                stream,
                name: "stderr",
                inner: inner.clone(),
                sink: sink.clone(),
                log: log_file.clone(),
                tail: Some(tail.clone()),
            }));
        }
        self.workers.push(spawn_waiter(inner.clone(), sink, tail, log_file));
        self.shared = Some(inner);

        logging::info(format!(
            "started dsh web pid {pid}: node {} web --no-open --host {} --port {}",
            self.install.bin_js.display(),
            options.host,
            options.port
        ));
        Ok(pid)
    }

    /// Terminate the child (SIGTERM/process group, then SIGKILL after grace).
    ///
    /// Idempotent; never blocks longer than grace + KILL_GRACE.
    pub fn stop(&mut self, grace: Duration) -> Result<()> {
        let Some(inner) = self.shared.clone() else {
            return Ok(());
        };
        if inner.exited.load(Ordering::SeqCst) {
            return Ok(());
        }
        let pid = inner.pid;
        logging::info(format!("stopping dsh web pid {pid}"));
        terminate(pid, false);
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if reap(&inner) {
                return Ok(());
            }
            thread::sleep(WAIT_POLL);
        }

        logging::warn(format!("dsh web pid {pid} ignored the stop request; killing it"));
        terminate(pid, true);
        let deadline = Instant::now() + KILL_GRACE;
        while Instant::now() < deadline {
            if reap(&inner) {
                return Ok(());
            }
            thread::sleep(WAIT_POLL);
        }
        bail!(
            "dsh web (pid {pid}) did not exit after SIGTERM and SIGKILL (or taskkill); \
             it may still be running"
        )
    }

    /// Whether the managed child is still alive.
    pub fn is_running(&self) -> bool {
        let Some(inner) = self.shared.as_ref() else {
            return false;
        };
        if inner.exited.load(Ordering::SeqCst) {
            return false;
        }
        let mut guard = match inner.child.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    inner.exited.store(true, Ordering::SeqCst);
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            },
            None => false,
        }
    }

    /// Pid of the live child, if any.
    pub fn pid(&self) -> Option<u32> {
        if self.is_running() {
            self.shared.as_ref().map(|inner| inner.pid)
        } else {
            None
        }
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        // Never leave an orphaned dsh web behind when the manager goes away.
        if self.is_running() {
            let _ = self.stop(Duration::from_millis(500));
        }
    }
}

/// One output stream reader thread configuration.
struct ReaderConfig<R> {
    stream: R,
    name: &'static str,
    inner: Arc<Inner>,
    sink: Sink,
    log: Option<LogHandle>,
    tail: Option<StderrTail>,
}

/// Read one child stream line by line: log it, keep a stderr tail and detect
/// the readiness line exactly once.
fn spawn_reader<R>(config: ReaderConfig<R>) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(config.stream);
        for raw in reader.lines() {
            let Ok(raw) = raw else { break };
            let line = truncate_line(raw.trim_end_matches('\r'));
            logging::child_line(config.name, &line);
            write_log(config.log.as_ref(), &line);
            if let Some(tail) = &config.tail {
                if let Ok(mut tail) = tail.lock() {
                    if tail.len() >= STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line.clone());
                }
            }
            if let Some(url) = parse_url_line(&line) {
                if !config.inner.ready.swap(true, Ordering::SeqCst) {
                    (config.sink)(ProcessEvent::Ready {
                        pid: config.inner.pid,
                        url,
                    });
                }
            }
        }
        // Also reached when a read error ends the loop, so the waiter cannot
        // wait forever.
        config.inner.readers.fetch_sub(1, Ordering::SeqCst);
    })
}

/// Wait for the child to exit; emit StartupFailed (when it never became ready)
/// and exactly one Exited event.
fn spawn_waiter(inner: Arc<Inner>, sink: Sink, tail: StderrTail, log: Option<LogHandle>) -> JoinHandle<()> {
    thread::spawn(move || {
        let code: Option<i32> = loop {
            let outcome = {
                let mut guard = match inner.child.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                match guard.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => Some(status.code()),
                        Ok(None) => None,
                        Err(_) => Some(None),
                    },
                    None => Some(None),
                }
            };
            if let Some(exit_code) = outcome {
                break exit_code;
            }
            thread::sleep(WAIT_POLL);
        };

        inner.exited.store(true, Ordering::SeqCst);
        // Let the output readers drain what the child wrote before it died: the
        // startup failure message must carry the stderr tail and the web log
        // must be complete before Exited is announced. Bounded in case a
        // descendant keeps a pipe open.
        let drain_deadline = Instant::now() + READER_DRAIN;
        while inner.readers.load(Ordering::SeqCst) > 0 && Instant::now() < drain_deadline {
            thread::sleep(Duration::from_millis(5));
        }

        let code_text = code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        if !inner.ready.load(Ordering::SeqCst) {
            let tail_text = tail
                .lock()
                .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            let message = if tail_text.is_empty() {
                format!(
                    "dsh web (pid {}) exited with code {code_text} before reporting its URL",
                    inner.pid
                )
            } else {
                format!(
                    "dsh web (pid {}) exited with code {code_text} before reporting its URL; last output:\n{tail_text}",
                    inner.pid
                )
            };
            (sink)(ProcessEvent::StartupFailed {
                pid: inner.pid,
                message,
            });
        }

        write_log(log.as_ref(), &format!("# --- dsh web exit code {code_text} ---"));
        (sink)(ProcessEvent::Exited { pid: inner.pid, code });
    })
}

/// Reap the child if it has exited; returns true once it is gone.
fn reap(inner: &Inner) -> bool {
    let mut guard = match inner.child.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                inner.exited.store(true, Ordering::SeqCst);
                true
            }
            Ok(None) => false,
            Err(_) => {
                inner.exited.store(true, Ordering::SeqCst);
                true
            }
        },
        None => true,
    }
}

/// Create the per-start web log with a header line.
fn open_web_log(install: &DshInstall, options: &SpawnOptions) -> Option<LogHandle> {
    if let Some(parent) = options.log_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            logging::warn(format!(
                "could not create the web log directory {}: {error}",
                parent.display()
            ));
            return None;
        }
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&options.log_path) {
        Ok(file) => file,
        Err(error) => {
            logging::warn(format!(
                "could not open the web log {}: {error}",
                options.log_path.display()
            ));
            return None;
        }
    };
    // The child prints its authenticated URL, token included.
    crate::config::restrict_file(&file);
    let header = format!(
        "# --- dsh web start {} host={} port={} version={} node={} bin={} ---\n",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z"),
        options.host,
        options.port,
        install.version,
        install.node.display(),
        install.bin_js.display()
    );
    if let Err(error) = file.write_all(header.as_bytes()) {
        logging::warn(format!("could not write the web log header: {error}"));
        return None;
    }
    let _ = file.flush();
    Some(Arc::new(Mutex::new(file)))
}

/// Append one line to the web log (best effort).
fn write_log(log: Option<&LogHandle>, line: &str) {
    let Some(log) = log else { return };
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Clamp one child output line so a pathological child cannot balloon the log.
fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{truncated}...")
}

/// Send the platform-appropriate termination signal to the child tree.
#[cfg(unix)]
fn terminate(pid: u32, force: bool) {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    // The child was spawned in its own process group (pgid == pid), so this
    // signals the whole node tree. Fall back to the single pid if the group is
    // already gone.
    let result = unsafe { libc::killpg(pid as libc::pid_t, signal) };
    if result != 0 {
        let _ = unsafe { libc::kill(pid as libc::pid_t, signal) };
    }
}

/// Kill the child tree on Windows (console children ignore WM_CLOSE).
#[cfg(windows)]
fn terminate(pid: u32, _force: bool) {
    use std::os::windows::process::CommandExt as _;
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command.creation_flags(CREATE_NO_WINDOW);
    let _ = command.status();
}

#[cfg(not(any(unix, windows)))]
fn terminate(_pid: u32, _force: bool) {}

/// Extract a URL from a "dsh web: <url>" stdout line.
///
/// Handles CRLF, ignores the browser-opening notice and anything that is not an
/// http(s) URL, and refuses lines with embedded whitespace.
pub fn parse_url_line(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\r', '\n']);
    let rest = line.strip_prefix("dsh web: ")?;
    let url = rest.trim();
    if url.is_empty() || url.contains(char::is_whitespace) {
        return None;
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some(url.to_string())
}

/// Render a URL for display without leaking the auth token query.
pub fn display_url(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    base.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_readiness_lines() {
        assert_eq!(
            parse_url_line("dsh web: http://127.0.0.1:46085/?token=abc"),
            Some("http://127.0.0.1:46085/?token=abc".to_string())
        );
        assert_eq!(
            parse_url_line("dsh web: https://example.test/x\r"),
            Some("https://example.test/x".to_string())
        );
        assert_eq!(parse_url_line("dsh web: opening the default browser..."), None);
        assert_eq!(parse_url_line("dsh web:"), None);
        assert_eq!(parse_url_line("dsh web: "), None);
        assert_eq!(parse_url_line("  dsh web: http://x"), None);
        assert_eq!(parse_url_line("dsh web: ftp://x"), None);
        assert_eq!(parse_url_line("dsh web: http://a b"), None);
        assert_eq!(parse_url_line("loading..."), None);
    }

    #[test]
    fn display_url_hides_tokens() {
        assert_eq!(
            display_url("http://127.0.0.1:46085/?token=M0_5v23aOJ9v3ShJyNC1HBqXQIp0vqBkcgpLnKGWVYQ"),
            "http://127.0.0.1:46085"
        );
        assert_eq!(display_url("http://127.0.0.1:3080/"), "http://127.0.0.1:3080");
        assert_eq!(display_url("https://example.test/ui/"), "https://example.test/ui");
        assert_eq!(display_url("https://example.test/#frag"), "https://example.test");
    }

    #[test]
    fn line_truncation_is_bounded() {
        assert_eq!(truncate_line("short"), "short");
        let long = "x".repeat(MAX_LINE_CHARS + 5);
        let truncated = truncate_line(&long);
        assert_eq!(truncated.chars().count(), MAX_LINE_CHARS + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn stopping_before_spawn_is_a_no_op() {
        let install = fake_install(PathBuf::from("/nonexistent/node"));
        let mut manager = ProcessManager::new(install);
        assert!(!manager.is_running());
        assert_eq!(manager.pid(), None);
        manager.stop(Duration::from_millis(10)).expect("no-op stop");
    }

    fn fake_install(node: PathBuf) -> DshInstall {
        DshInstall {
            version: "0.0.0-test".to_string(),
            package_root: PathBuf::from("/nonexistent/@deepseek-ai/dsh"),
            bin_js: PathBuf::from("/nonexistent/@deepseek-ai/dsh/lib/bin.js"),
            node,
            npm: PathBuf::from("/nonexistent/npm"),
        }
    }

    #[test]
    fn spawn_reports_missing_node() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut manager = ProcessManager::new(fake_install(dir.path().join("no-node")));
        let options = SpawnOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            log_path: dir.path().join("web.log"),
        };
        let error = manager.spawn(&options, Box::new(|_| {})).expect_err("must fail");
        assert!(format!("{error}").contains("could not start dsh web"), "{error}");
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt as _;
        use std::sync::mpsc::{self, Receiver};

        fn write_script(path: &std::path::Path, body: &str) {
            std::fs::write(path, body).expect("write script");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        fn install_with_script(dir: &std::path::Path, body: &str) -> DshInstall {
            let package_root = dir.join("node_modules/@deepseek-ai/dsh");
            std::fs::create_dir_all(package_root.join("lib")).expect("mkdir");
            std::fs::write(package_root.join("lib/bin.js"), b"// fake entry point\n").expect("write bin.js");
            let node = dir.join("fake-node.sh");
            write_script(&node, body);
            DshInstall {
                version: "0.0.0-test".to_string(),
                package_root,
                bin_js: std::path::PathBuf::from("/nonexistent/bin.js"),
                node,
                npm: std::path::PathBuf::from("/nonexistent/npm"),
            }
        }

        fn wait_for(
            receiver: &Receiver<ProcessEvent>,
            timeout: Duration,
            mut predicate: impl FnMut(&ProcessEvent) -> bool,
        ) -> Option<ProcessEvent> {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        if predicate(&event) {
                            return Some(event);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                }
            }
            None
        }

        #[test]
        fn spawn_ready_stop_and_exit() {
            let started = Instant::now();
            let dir = tempfile::tempdir().expect("tempdir");
            let install = install_with_script(
                dir.path(),
                "#!/bin/sh\necho 'dsh web: http://127.0.0.1:43219/?token=test-token'\necho 'fake web ready' >&2\nexec sleep 60\n",
            );
            let log_path = dir.path().join("logs/web.log");
            let mut manager = ProcessManager::new(install);
            let (sender, receiver) = mpsc::channel();
            let pid = manager
                .spawn(
                    &SpawnOptions {
                        host: "127.0.0.1".to_string(),
                        port: 0,
                        log_path: log_path.clone(),
                    },
                    Box::new(move |event| {
                        let _ = sender.send(event);
                    }),
                )
                .expect("spawn");
            assert!(pid > 0);

            let ready = wait_for(&receiver, Duration::from_secs(3), |event| {
                matches!(event, ProcessEvent::Ready { .. })
            })
            .expect("ready event");
            match ready {
                ProcessEvent::Ready { pid: ready_pid, url } => {
                    assert_eq!(ready_pid, pid);
                    assert_eq!(url, "http://127.0.0.1:43219/?token=test-token");
                }
                other => panic!("unexpected event: {other:?}"),
            }
            assert!(manager.is_running());
            assert_eq!(manager.pid(), Some(pid));

            manager.stop(Duration::from_secs(3)).expect("stop");
            let exited = wait_for(&receiver, Duration::from_secs(2), |event| {
                matches!(event, ProcessEvent::Exited { .. })
            })
            .expect("exited event");
            match exited {
                ProcessEvent::Exited { pid: exited_pid, code } => {
                    assert_eq!(exited_pid, pid);
                    assert!(
                        code.is_none() || code == Some(0) || code == Some(143),
                        "unexpected code: {code:?}"
                    );
                }
                other => panic!("unexpected event: {other:?}"),
            }
            assert!(!manager.is_running());
            assert_eq!(manager.pid(), None);

            // Idempotent.
            manager.stop(Duration::from_millis(100)).expect("second stop");

            let log = std::fs::read_to_string(&log_path).expect("read web log");
            assert!(log.contains("# --- dsh web start"), "missing header: {log}");
            assert!(
                log.contains("dsh web: http://127.0.0.1:43219/?token=test-token"),
                "missing url: {log}"
            );
            assert!(log.contains("fake web ready"), "missing stderr: {log}");
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "test too slow: {:?}",
                started.elapsed()
            );
        }

        #[test]
        fn early_exit_reports_startup_failure_then_exit() {
            let dir = tempfile::tempdir().expect("tempdir");
            let install = install_with_script(dir.path(), "#!/bin/sh\necho 'boom: cannot bind' >&2\nexit 3\n");
            let log_path = dir.path().join("web.log");
            let mut manager = ProcessManager::new(install);
            let (sender, receiver) = mpsc::channel();
            let pid = manager
                .spawn(
                    &SpawnOptions {
                        host: "127.0.0.1".to_string(),
                        port: 0,
                        log_path: log_path.clone(),
                    },
                    Box::new(move |event| {
                        let _ = sender.send(event);
                    }),
                )
                .expect("spawn");

            let failed = wait_for(&receiver, Duration::from_secs(3), |event| {
                matches!(event, ProcessEvent::StartupFailed { .. })
            })
            .expect("startup failure");
            match failed {
                ProcessEvent::StartupFailed {
                    pid: failed_pid,
                    message,
                } => {
                    assert_eq!(failed_pid, pid);
                    assert!(message.contains("boom: cannot bind"), "message: {message}");
                    assert!(message.contains("exit"), "message: {message}");
                }
                other => panic!("unexpected event: {other:?}"),
            }

            let exited = wait_for(&receiver, Duration::from_secs(2), |event| {
                matches!(event, ProcessEvent::Exited { .. })
            })
            .expect("exited event");
            match exited {
                ProcessEvent::Exited { pid: exited_pid, code } => {
                    assert_eq!(exited_pid, pid);
                    assert_eq!(code, Some(3));
                }
                other => panic!("unexpected event: {other:?}"),
            }
            assert!(!manager.is_running());
        }
    }
}
