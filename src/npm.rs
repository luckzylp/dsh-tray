//! npm registry queries and global installs.

use std::path::PathBuf;

use crate::channel::{Channel, DistTags};
use crate::config::Config;

/// A resolved npm executable + registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmClient {
    /// `npm` / `npm.cmd` path.
    pub program: PathBuf,
    /// Registry base URL without trailing slash.
    pub registry: String,
}

impl NpmClient {
    /// Resolve npm and the registry to query.
    pub fn discover(_config: &Config) -> anyhow::Result<Self> {
        anyhow::bail!("npm discovery not implemented yet")
    }

    /// Fetch the package's `dist-tags` map.
    pub fn dist_tags(&self) -> anyhow::Result<DistTags> {
        anyhow::bail!("dist-tags not implemented yet")
    }

    /// Install the latest version of `channel` globally, streaming output.
    pub fn install(&self, _channel: Channel, _log: &mut dyn FnMut(&str)) -> anyhow::Result<()> {
        anyhow::bail!("npm install not implemented yet")
    }
}
