//! Launch-at-login integration.
//!
//! Implemented per platform:
//! - Windows: HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! - macOS: ~/Library/LaunchAgents/<APP_ID>.plist (plus best-effort launchctl)
//! - Linux/BSD: $XDG_CONFIG_HOME/autostart/<APP_ID>.desktop (fallback ~/.config)
//!
//! The generated file/registry contents are built by pure functions so they can
//! be unit-tested on every host; the platform entry points only do I/O.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::APP_ID;

/// Registry key (relative to HKEY_CURRENT_USER) holding login items on Windows.
pub const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Registry value name used by this application.
pub const WINDOWS_RUN_VALUE: &str = "dsh-tray";

/// Current autostart registration state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub path: PathBuf,
}

/// Read the current platform autostart state.
pub fn status() -> AutostartStatus {
    #[cfg(windows)]
    {
        windows_status()
    }
    #[cfg(not(windows))]
    {
        let path = default_path();
        AutostartStatus {
            enabled: path.is_file(),
            path,
        }
    }
}

/// Register the tray to start at login (idempotent).
pub fn enable(exe: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        windows_enable(exe)
    }
    #[cfg(target_os = "macos")]
    {
        macos_enable(exe)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        linux_enable(exe)
    }
}

/// Remove the login item (idempotent).
pub fn disable() -> Result<()> {
    #[cfg(windows)]
    {
        windows_disable()
    }
    #[cfg(target_os = "macos")]
    {
        macos_disable()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        linux_disable()
    }
}

/// Platform-specific autostart file/registry location.
pub fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        windows_run_key_path()
    }
    #[cfg(target_os = "macos")]
    {
        launch_agent_path(&home_dir())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        desktop_entry_path(&config_home())
    }
}

/// XDG desktop-entry path inside an explicit config directory.
pub fn desktop_entry_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(format!("{APP_ID}.desktop"))
}

/// macOS LaunchAgent path inside an explicit home directory.
pub fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{APP_ID}.plist"))
}

/// Pseudo path describing the Windows Run-key value (for diagnostics/UI).
pub fn windows_run_key_path() -> PathBuf {
    PathBuf::from(format!(r"HKCU\{WINDOWS_RUN_KEY}\{WINDOWS_RUN_VALUE}"))
}

/// Contents of the XDG autostart desktop entry.
pub fn desktop_entry_content(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=DSH Tray\n\
         Comment=DeepSeek Harness system tray controller\n\
         Exec={} --hidden\n\
         Icon=dsh-tray\n\
         Terminal=false\n\
         Categories=Utility;\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_exec(exe)
    )
}

/// Quote a path for the desktop entry Exec key.
pub fn desktop_exec(exe: &Path) -> String {
    quote_desktop_arg(&exe.to_string_lossy())
}

/// Quote a single Exec argument per the Desktop Entry specification.
///
/// Reserved characters are wrapped in double quotes; inside a quoted argument
/// backslash, double quote, backtick and dollar must be escaped, and a literal
/// percent sign (a field code introducer) is doubled.
fn quote_desktop_arg(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if character == '%' {
            quoted.push_str("%%");
        } else if is_desktop_special(character) {
            quoted.push('\\');
            quoted.push(character);
        } else {
            quoted.push(character);
        }
    }
    quoted.push('"');
    quoted
}

/// Characters that must be backslash-escaped inside a quoted Exec argument.
fn is_desktop_special(character: char) -> bool {
    matches!(character, '"' | '$' | '\\') || character == char::from(96)
}

/// Contents of the macOS LaunchAgent property list.
pub fn launch_agent_content(exe: &Path) -> String {
    let program = xml_escape(&exe.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{APP_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>--hidden</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#
    )
}

/// Escape text for XML character data (paths may contain &, <, quotes...).
fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Command line stored in the Windows Run key.
pub fn windows_run_command(exe: &Path) -> String {
    format!("\"{}\" --hidden", exe.to_string_lossy())
}

#[cfg(not(windows))]
fn home_dir() -> PathBuf {
    env_path("HOME")
        .or_else(|| env_path("USERPROFILE"))
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create autostart directory {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))
            .with_context(|| format!("set permissions on {}", path.display()))?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn linux_enable(exe: &Path) -> Result<()> {
    let path = default_path();
    write_file(&path, &desktop_entry_content(exe))?;
    crate::logging::info(format!("autostart entry written: {}", path.display()));
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn linux_disable() -> Result<()> {
    let path = default_path();
    remove_file_if_exists(&path)?;
    crate::logging::info(format!("autostart entry removed: {}", path.display()));
    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl(arguments: &[&std::ffi::OsStr]) -> Option<std::process::Output> {
    match std::process::Command::new("launchctl").args(arguments).output() {
        Ok(output) => Some(output),
        Err(error) => {
            crate::logging::warn(format!("could not run launchctl: {error}"));
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_domain() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    format!("gui/{uid}")
}

#[cfg(target_os = "macos")]
fn macos_enable(exe: &Path) -> Result<()> {
    let path = default_path();
    write_file(&path, &launch_agent_content(exe))?;
    let domain = macos_domain();
    let service = format!("{domain}/{APP_ID}");
    // Best effort: replacing an already-loaded agent requires a bootout first.
    let _ = launchctl(&[std::ffi::OsStr::new("bootout"), std::ffi::OsStr::new(&service)]);
    if let Some(output) = launchctl(&[
        std::ffi::OsStr::new("bootstrap"),
        std::ffi::OsStr::new(&domain),
        path.as_os_str(),
    ]) {
        if !output.status.success() {
            crate::logging::warn(format!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    crate::logging::info(format!("launch agent written: {}", path.display()));
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_disable() -> Result<()> {
    let path = default_path();
    let service = format!("{}/{APP_ID}", macos_domain());
    let _ = launchctl(&[std::ffi::OsStr::new("bootout"), std::ffi::OsStr::new(&service)]);
    remove_file_if_exists(&path)?;
    crate::logging::info(format!("launch agent removed: {}", path.display()));
    Ok(())
}

#[cfg(windows)]
fn windows_status() -> AutostartStatus {
    let path = default_path();
    let enabled = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(WINDOWS_RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(WINDOWS_RUN_VALUE))
        .is_ok();
    AutostartStatus { enabled, path }
}

#[cfg(windows)]
fn windows_enable(exe: &Path) -> Result<()> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(WINDOWS_RUN_KEY).context("open HKCU Run key")?;
    let command = windows_run_command(exe);
    key.set_value(WINDOWS_RUN_VALUE, &command.as_str())
        .context("write HKCU Run value")?;
    crate::logging::info("autostart registry value written");
    Ok(())
}

#[cfg(windows)]
fn windows_disable() -> Result<()> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey(WINDOWS_RUN_KEY) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(anyhow::Error::from(error).context("open HKCU Run key")),
    };
    match key.delete_value(WINDOWS_RUN_VALUE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::Error::from(error).context("delete HKCU Run value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_has_required_fields() {
        let content = desktop_entry_content(Path::new("/opt/dsh-tray/dsh-tray"));
        assert!(content.starts_with("[Desktop Entry]\n"));
        assert!(content.contains("Type=Application\n"));
        assert!(content.contains("Name=DSH Tray\n"));
        assert!(content.contains("Terminal=false\n"));
        assert!(content.contains("Categories=Utility;\n"));
        assert!(content.contains("X-GNOME-Autostart-enabled=true\n"));
        assert!(content.contains("Exec=\"/opt/dsh-tray/dsh-tray\" --hidden\n"));
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn desktop_exec_quotes_paths_with_spaces_and_reserved_characters() {
        assert_eq!(
            desktop_exec(Path::new("/opt/My App/dsh-tray")),
            "\"/opt/My App/dsh-tray\""
        );
        // Double quote, dollar and backslash are escaped; percent is doubled.
        assert_eq!(
            desktop_exec(Path::new(r#"/tmp/a"b$d\e/f%g"#)),
            r#""/tmp/a\"b\$d\\e/f%%g""#
        );
    }

    #[test]
    fn launch_agent_plist_has_required_keys() {
        let content = launch_agent_content(Path::new("/Applications/DSH Tray.app/Contents/MacOS/dsh-tray"));
        assert!(content.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(content.contains("<string>ai.deepseek.dsh-tray</string>"));
        assert!(content.contains("<string>/Applications/DSH Tray.app/Contents/MacOS/dsh-tray</string>"));
        assert!(content.contains("<string>--hidden</string>"));
        assert!(content.contains("<key>RunAtLoad</key>"));
        assert!(content.contains("<key>KeepAlive</key>"));
        assert!(content.contains("<false/>"));
    }

    #[test]
    fn launch_agent_escapes_xml() {
        let content = launch_agent_content(Path::new("/Users/a&b/<dsh>/\"quoted\"/x'y"));
        assert!(content.contains("a&amp;b"));
        assert!(content.contains("&lt;dsh&gt;"));
        assert!(content.contains("&quot;quoted&quot;"));
        assert!(content.contains("x&apos;y"));
        assert!(!content.contains("a&b"));
    }

    #[test]
    fn windows_run_command_quotes_the_executable() {
        assert_eq!(
            windows_run_command(Path::new(r"C:\Program Files\DSH Tray\dsh-tray.exe")),
            r#""C:\Program Files\DSH Tray\dsh-tray.exe" --hidden"#
        );
    }

    #[test]
    fn paths_live_under_the_expected_platform_directories() {
        let desktop = desktop_entry_path(Path::new("/home/user/.config"));
        assert_eq!(
            desktop,
            PathBuf::from("/home/user/.config/autostart/ai.deepseek.dsh-tray.desktop")
        );
        let agent = launch_agent_path(Path::new("/Users/user"));
        assert_eq!(
            agent,
            PathBuf::from("/Users/user/Library/LaunchAgents/ai.deepseek.dsh-tray.plist")
        );
        let run_key = windows_run_key_path();
        assert!(run_key.to_string_lossy().contains("CurrentVersion"));
        assert!(run_key.to_string_lossy().ends_with("dsh-tray"));
    }

    #[test]
    fn default_path_matches_the_platform() {
        let path = default_path();
        #[cfg(windows)]
        assert_eq!(path, windows_run_key_path());
        #[cfg(target_os = "macos")]
        assert!(path.ends_with("Library/LaunchAgents/ai.deepseek.dsh-tray.plist"));
        #[cfg(not(any(windows, target_os = "macos")))]
        assert!(path.ends_with("autostart/ai.deepseek.dsh-tray.desktop"));
    }

    #[cfg(unix)]
    #[test]
    fn write_and_remove_file_cycle() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = desktop_entry_path(dir.path());
        write_file(&path, "x").expect("write");
        assert!(path.is_file());
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o644
        );
        remove_file_if_exists(&path).expect("remove");
        assert!(!path.exists());
        // Idempotent: removing a missing file is fine.
        remove_file_if_exists(&path).expect("remove again");
    }

    #[cfg(unix)]
    #[test]
    fn write_file_creates_missing_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a/b/c/entry.desktop");
        write_file(&path, "hello").expect("write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "hello");
    }
}
