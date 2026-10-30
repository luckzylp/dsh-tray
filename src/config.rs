//! Persistent user configuration and platform paths.

use std::fs;
use std::io::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::channel::Channel;

/// UI language preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Follow the process/system locale.
    #[default]
    Auto,
    /// English.
    En,
    /// Simplified Chinese.
    Zh,
}

impl Language {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Language::Auto),
            "en" | "english" => Some(Language::En),
            "zh" | "zh-cn" | "cn" | "chinese" => Some(Language::Zh),
            _ => None,
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Language::Auto => "auto",
            Language::En => "en",
            Language::Zh => "zh",
        })
    }
}

/// User settings, persisted as JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema revision, for future migrations.
    pub schema_version: u32,
    /// Preferred npm release channel.
    pub channel: Channel,
    /// Port passed to `dsh web`; `0` lets the OS pick a free port (recommended).
    pub port: u16,
    /// Bind address passed to `dsh web`.
    pub host: String,
    /// Open the browser as soon as a start initiated from the tray becomes ready.
    pub open_browser_when_ready: bool,
    /// Start dsh when the tray starts.
    pub start_dsh_on_launch: bool,
    /// Start the tray itself at login (mirrors the platform autostart entry).
    pub autostart: bool,
    /// UI language preference.
    pub language: Language,
    /// Explicit path to the dsh `bin.js` (or to a `dsh` shim); bypasses discovery.
    pub dsh_path: Option<PathBuf>,
    /// Explicit path to the `node` executable.
    pub node_path: Option<PathBuf>,
    /// Explicit path to the `npm` executable.
    pub npm_path: Option<PathBuf>,
    /// npm registry base URL; `None` = auto-detect.
    pub registry: Option<String>,
    /// How often to poll npm dist-tags in the background (hours).
    pub update_check_hours: u64,
    /// Stop a tray-managed dsh process when the tray quits.
    pub stop_dsh_on_quit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            channel: Channel::Latest,
            port: 0,
            host: "127.0.0.1".to_string(),
            open_browser_when_ready: true,
            start_dsh_on_launch: true,
            autostart: false,
            language: Language::Auto,
            dsh_path: None,
            node_path: None,
            npm_path: None,
            registry: None,
            update_check_hours: 6,
            stop_dsh_on_quit: true,
        }
    }
}

impl Config {
    /// Load configuration, returning defaults plus a warning when unreadable.
    ///
    /// A corrupt file never prevents startup: the defaults are used, and the
    /// caller is expected to surface the warning (log + tray notice).
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Config::default(), None);
            }
            Err(error) => {
                return (
                    Config::default(),
                    Some(format!("could not read config {}: {error}", path.display())),
                );
            }
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(mut config) => {
                let warning = config.normalize();
                (config, warning)
            }
            Err(error) => (
                Config::default(),
                Some(format!(
                    "could not parse config {}: {error}; using defaults",
                    path.display()
                )),
            ),
        }
    }

    /// Clamp/repair values and return a warning when something was changed.
    pub fn normalize(&mut self) -> Option<String> {
        let mut warnings = Vec::new();
        if self.host.parse::<IpAddr>().is_err() {
            warnings.push(format!("invalid host {:?} (needs an IP literal); using 127.0.0.1", self.host));
            self.host = "127.0.0.1".to_string();
        }
        if self.host.parse::<IpAddr>().map(|ip| ip.is_unspecified()).unwrap_or(false) {
            warnings.push("wildcard bind addresses are not supported by dsh; using 127.0.0.1".to_string());
            self.host = "127.0.0.1".to_string();
        }
        if self.update_check_hours == 0 || self.update_check_hours > 24 * 30 {
            self.update_check_hours = self.update_check_hours.clamp(1, 24 * 30);
            warnings.push(format!("update_check_hours clamped to {}", self.update_check_hours));
        }
        if let Some(registry) = &self.registry {
            let trimmed = registry.trim().trim_end_matches('/').to_string();
            if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                warnings.push(format!("invalid registry {registry:?}; using auto-detect"));
                self.registry = None;
            } else if trimmed != *registry {
                self.registry = Some(trimmed);
            }
        }
        if self.port == 0 {
            // auto-select; nothing to do
        }
        if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        }
    }

    /// Persist atomically (write temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config directory {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("serialize config")?;
        write_atomic(path, json.as_bytes())
    }
}

/// Write `bytes` to `path` atomically.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create directory {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dsh-tray.tmp");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("create temporary file {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write temporary file {}", tmp.display()))?;
        file.sync_all().ok();
    }
    #[cfg(windows)]
    if path.exists() {
        // Windows cannot rename over an existing file.
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// Filesystem locations used by the tray.
#[derive(Clone, Debug)]
pub struct AppPaths {
    /// Directory holding `config.json`.
    pub config_dir: PathBuf,
    /// Directory holding logs, runtime state and the instance lock.
    pub data_dir: PathBuf,
    /// Path of the running executable (used for autostart entries).
    pub exe: PathBuf,
}

impl AppPaths {
    /// Resolve platform directories, honouring the `DSH_TRAY_CONFIG_DIR` and
    /// `DSH_TRAY_DATA_DIR` environment overrides (used by tests and portable
    /// setups).
    pub fn resolve() -> Self {
        let project = ProjectDirs::from("ai", "deepseek", crate::APP_NAME);
        let config_dir = env_path("DSH_TRAY_CONFIG_DIR")
            .or_else(|| project.as_ref().map(|dirs| dirs.config_dir().to_path_buf()))
            .unwrap_or_else(fallback_dir);
        let data_dir = env_path("DSH_TRAY_DATA_DIR")
            .or_else(|| project.as_ref().map(|dirs| dirs.data_dir().to_path_buf()))
            .unwrap_or_else(|| config_dir.clone());
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(crate::APP_NAME));
        Self { config_dir, data_dir, exe }
    }

    /// Explicit directories (tests, portable mode).
    pub fn with_dirs(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(crate::APP_NAME));
        Self { config_dir, data_dir, exe }
    }

    /// Create all directories the app writes into.
    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("create {}", self.config_dir.display()))?;
        fs::create_dir_all(self.log_dir())
            .with_context(|| format!("create {}", self.log_dir().display()))?;
        fs::create_dir_all(self.tray_temp_dir())
            .with_context(|| format!("create {}", self.tray_temp_dir().display()))?;
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn log_file(&self) -> PathBuf {
        self.log_dir().join("dsh-tray.log")
    }

    pub fn web_log_file(&self) -> PathBuf {
        self.log_dir().join("dsh-web.log")
    }

    pub fn runtime_file(&self) -> PathBuf {
        self.data_dir.join("runtime.json")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join("instance.lock")
    }

    /// Writable directory for the tray icon PNG (AppIndicator needs a file).
    pub fn tray_temp_dir(&self) -> PathBuf {
        self.data_dir.join("tray-icon")
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn fallback_dir() -> PathBuf {
    std::env::temp_dir().join(crate::APP_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.channel, Channel::Latest);
        assert_eq!(config.port, 0);
        assert_eq!(config.host, "127.0.0.1");
        assert!(config.open_browser_when_ready);
        assert!(config.start_dsh_on_launch);
        assert_eq!(config.update_check_hours, 6);
    }

    #[test]
    fn roundtrip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = Config::default();
        config.channel = Channel::Alpha;
        config.port = 41029;
        config.language = Language::Zh;
        config.save(&path).unwrap();
        let (loaded, warning) = Config::load(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.channel, Channel::Alpha);
        assert_eq!(loaded.port, 41029);
        assert_eq!(loaded.language, Language::Zh);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (config, warning) = Config::load(&dir.path().join("nope.json"));
        assert!(warning.is_none());
        assert_eq!(config.channel, Channel::Latest);
    }

    #[test]
    fn corrupt_file_reports_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{ not json").unwrap();
        let (config, warning) = Config::load(&path);
        assert!(warning.is_some());
        assert_eq!(config.channel, Channel::Latest);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"channel":"next","future_option":true}"#).unwrap();
        let (config, warning) = Config::load(&path);
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(config.channel, Channel::Next);
        assert!(config.open_browser_when_ready);
    }

    #[test]
    fn normalize_repairs_bad_values() {
        let mut config = Config::default();
        config.host = "0.0.0.0".into();
        config.update_check_hours = 0;
        config.registry = Some("https://registry.npmjs.org/".into());
        let warning = config.normalize().expect("warning");
        assert!(warning.contains("wildcard"));
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.update_check_hours, 1);
        assert_eq!(config.registry.as_deref(), Some("https://registry.npmjs.org"));
    }

    #[test]
    fn app_paths_are_derived_from_dirs() {
        let paths = AppPaths::with_dirs(PathBuf::from("/tmp/cfg"), PathBuf::from("/tmp/data"));
        assert_eq!(paths.config_file(), PathBuf::from("/tmp/cfg/config.json"));
        assert_eq!(paths.log_file(), PathBuf::from("/tmp/data/logs/dsh-tray.log"));
        assert_eq!(paths.web_log_file(), PathBuf::from("/tmp/data/logs/dsh-web.log"));
        assert_eq!(paths.lock_file(), PathBuf::from("/tmp/data/instance.lock"));
        assert_eq!(paths.tray_temp_dir(), PathBuf::from("/tmp/data/tray-icon"));
    }
}
