//! Lifecycle of the managed `dsh web` child process.

use std::path::PathBuf;
use std::time::Duration;

use crate::dsh::DshInstall;

/// Events emitted by the reader/wait threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    /// The web server announced its URL (`dsh web: <url>`).
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

/// Owns the child process handle and its event sink.
pub struct ProcessManager {
    install: DshInstall,
}

impl ProcessManager {
    pub fn new(install: DshInstall) -> Self {
        Self { install }
    }

    /// Spawn `node bin.js web --no-open ...`; returns the child pid.
    pub fn spawn(
        &mut self,
        _options: &SpawnOptions,
        _sink: Box<dyn Fn(ProcessEvent) + Send + Sync>,
    ) -> anyhow::Result<u32> {
        let _ = &self.install;
        anyhow::bail!("process spawn not implemented yet")
    }

    /// Terminate the child (SIGTERM/process group, then SIGKILL after `grace`).
    pub fn stop(&mut self, _grace: Duration) -> anyhow::Result<()> {
        anyhow::bail!("process stop not implemented yet")
    }

    pub fn is_running(&self) -> bool {
        false
    }

    pub fn pid(&self) -> Option<u32> {
        None
    }
}

/// Extract a URL from a `dsh web: <url>` stdout line.
pub fn parse_url_line(_line: &str) -> Option<String> {
    None
}

/// Render a URL for display without leaking the auth token query.
pub fn display_url(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    base.trim_end_matches('/').to_string()
}
