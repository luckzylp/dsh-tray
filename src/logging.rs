//! Minimal thread-safe file logger.
//!
//! The tray is usually started by a desktop session, so stderr is not visible:
//! everything worth keeping goes to `<data>/logs/dsh-tray.log`. Builds with
//! `DSH_TRAY_DEBUG=1` also echo to stderr, which is useful when running from a
//! terminal.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use chrono::Local;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// A file logger with a single append handle.
pub struct Logger {
    file: Mutex<File>,
    path: PathBuf,
    echo: bool,
}

impl Logger {
    /// Open (or create) `path`, rotating it first when it exceeds 2 MiB.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create log directory {}", parent.display()))?;
        }
        rotate(path);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open log file {}", path.display()))?;
        Ok(Self {
            file: Mutex::new(file),
            path: path.to_path_buf(),
            echo: std::env::var_os("DSH_TRAY_DEBUG").is_some(),
        })
    }

    /// Path of the log file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one formatted line.
    pub fn line(&self, level: &str, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S%:z");
        let line = format!("[{timestamp}] {level:<5} {message}\n");
        if let Ok(mut file) = self.file.lock() {
            if file.write_all(line.as_bytes()).is_err() {
                // Logging must never take the application down.
                eprintln!("{line}");
            }
            let _ = file.flush();
        }
        if self.echo {
            eprint!("{line}");
        }
    }

    /// Log a line emitted by the child dsh process.
    pub fn child_line(&self, stream: &str, message: &str) {
        self.line("WEB", &format!("[{stream}] {message}"));
    }
}

fn rotate(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else { return };
    if metadata.len() <= MAX_LOG_BYTES {
        return;
    }
    let backup = path.with_file_name(format!(
        "{}.1",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("dsh-tray.log")
    ));
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(path, &backup);
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Install the process-wide logger (idempotent; later calls are ignored).
pub fn init(path: &Path) -> Result<()> {
    let logger = Logger::open(path)?;
    let _ = GLOBAL.set(logger);
    Ok(())
}

/// Path of the process-wide log file, if logging was initialised.
pub fn path() -> Option<PathBuf> {
    GLOBAL.get().map(|logger| logger.path().to_path_buf())
}

fn write_global(level: &str, message: &str) {
    match GLOBAL.get() {
        Some(logger) => logger.line(level, message),
        None => eprintln!("[{level}] {message}"),
    }
}

/// Log an informational line.
pub fn info(message: impl AsRef<str>) {
    write_global("INFO", message.as_ref());
}

/// Log a warning.
pub fn warn(message: impl AsRef<str>) {
    write_global("WARN", message.as_ref());
}

/// Log an error.
pub fn error(message: impl AsRef<str>) {
    write_global("ERROR", message.as_ref());
}

/// Log a line produced by the managed dsh process.
pub fn child_line(stream: &str, message: &str) {
    match GLOBAL.get() {
        Some(logger) => logger.child_line(stream, message),
        None => eprintln!("[WEB {stream}] {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_timestamped_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        let logger = Logger::open(&path).unwrap();
        logger.line("INFO", "hello");
        logger.child_line("stdout", "dsh web: http://127.0.0.1:1/");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("INFO  hello") || content.contains("INFO hello"));
        assert!(content.contains("[stdout] dsh web"));
    }

    #[test]
    fn rotates_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 10) as usize]).unwrap();
        let _logger = Logger::open(&path).unwrap();
        assert!(dir.path().join("big.log.1").exists());
        assert!(fs::metadata(&path).unwrap().len() < MAX_LOG_BYTES);
    }
}
