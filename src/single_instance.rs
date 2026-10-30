//! Single-instance guard and cross-process runtime handoff.

use std::fs::{self, File, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result};
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

/// Holds the exclusive instance lock for the lifetime of the process.
pub struct InstanceGuard {
    _file: File,
}

/// Try to become the primary tray instance.
///
/// Returns Ok(None) when another instance already owns the lock.
pub fn acquire(path: &Path) -> Result<Option<InstanceGuard>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create directory {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("open instance lock {}", path.display()))?;
    match FileExt::try_lock(&file) {
        Ok(()) => Ok(Some(InstanceGuard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(anyhow::Error::new(error).context("lock the instance lock file")),
    }
}

/// JSON handed to a second launch so it can open the running web UI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeState {
    pub pid: u32,
    pub url: Option<String>,
    pub tray_version: String,
    pub started_at: String,
}

/// Persist the runtime handoff file.
pub fn write_runtime(path: &Path, state: &RuntimeState) -> Result<()> {
    let json = serde_json::to_vec_pretty(state).context("serialize runtime state")?;
    crate::config::write_atomic(path, &json)
}

/// Read the runtime handoff file, if present and parseable.
pub fn read_runtime(path: &Path) -> Option<RuntimeState> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove the runtime handoff file (best effort).
pub fn remove_runtime(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let state = RuntimeState {
            pid: 1234,
            url: Some("http://127.0.0.1:46085/?token=secret".into()),
            tray_version: crate::VERSION.to_string(),
            started_at: "2026-01-01T00:00:00+08:00".into(),
        };
        write_runtime(&path, &state).unwrap();
        let loaded = read_runtime(&path).expect("runtime file");
        assert_eq!(loaded.pid, 1234);
        assert_eq!(loaded.url.as_deref(), Some("http://127.0.0.1:46085/?token=secret"));
        remove_runtime(&path);
        assert!(read_runtime(&path).is_none());
    }

    #[test]
    fn lock_is_exclusive_within_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.lock");
        let first = acquire(&path).unwrap().expect("first lock");
        assert!(acquire(&path).unwrap().is_none(), "second lock must be refused");
        drop(first);
        // File-lock release is immediate on Unix/Windows, but allow a short
        // retry so filesystem/AV latency cannot make this test flaky.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if acquire(&path).unwrap().is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "lock must be reusable after drop");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
