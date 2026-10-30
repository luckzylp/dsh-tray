//! Single-instance guard and cross-process runtime handoff.

use std::fs::File;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Holds the exclusive instance lock for the lifetime of the process.
pub struct InstanceGuard {
    _file: File,
}

/// Try to become the primary tray instance.
///
/// Returns `Ok(None)` when another instance already owns the lock.
pub fn acquire(_path: &Path) -> anyhow::Result<Option<InstanceGuard>> {
    anyhow::bail!("single-instance lock not implemented yet")
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
pub fn write_runtime(_path: &Path, _state: &RuntimeState) -> anyhow::Result<()> {
    anyhow::bail!("runtime state write not implemented yet")
}

/// Read the runtime handoff file, if present and parseable.
pub fn read_runtime(_path: &Path) -> Option<RuntimeState> {
    None
}
