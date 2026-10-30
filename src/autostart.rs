//! Launch-at-login integration.
//!
//! Implemented per platform:
//! - Windows: `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run`
//! - macOS: `~/Library/LaunchAgents/<APP_ID>.plist`
//! - Linux/BSD: `$XDG_CONFIG_HOME/autostart/<APP_ID>.desktop`

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Current autostart registration state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub path: PathBuf,
}

/// Read the current platform autostart state.
pub fn status() -> AutostartStatus {
    AutostartStatus { enabled: false, path: default_path() }
}

/// Register the tray to start at login (idempotent).
pub fn enable(_exe: &Path) -> Result<()> {
    anyhow::bail!("autostart: not implemented yet")
}

/// Remove the login item (idempotent).
pub fn disable() -> Result<()> {
    anyhow::bail!("autostart: not implemented yet")
}

/// Platform-specific autostart file/registry location.
pub fn default_path() -> PathBuf {
    PathBuf::new()
}
