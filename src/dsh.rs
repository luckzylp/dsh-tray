//! Locate the installed dsh CLI and read its version.

use std::path::PathBuf;

use crate::channel::{Channel, DistTags};
use crate::config::Config;

/// A resolved dsh installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DshInstall {
    /// Installed package version (from package.json).
    pub version: String,
    /// `.../node_modules/@deepseek-ai/dsh`.
    pub package_root: PathBuf,
    /// `.../lib/bin.js` (the actual CLI entry point).
    pub bin_js: PathBuf,
    /// Node.js executable used to run `bin.js`.
    pub node: PathBuf,
    /// npm executable used for installs.
    pub npm: PathBuf,
}

impl DshInstall {
    /// Discover the installation from config overrides + PATH + npm.
    pub fn discover(_config: &Config) -> anyhow::Result<Self> {
        anyhow::bail!("dsh discovery not implemented yet")
    }
}

/// Decide which channel the installed version most plausibly belongs to.
pub fn detect_channel(_version: &str, _tags: &DistTags, _preferred: Channel) -> Channel {
    Channel::Latest
}
