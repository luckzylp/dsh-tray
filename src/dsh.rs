//! Locate the installed dsh CLI and read its version.
//!
//! Discovery order (first hit wins):
//!
//! 1. config.dsh_path -- a lib/bin.js, a symlink to it, a dsh shim, or the
//!    package directory itself.
//! 2. dsh on PATH (on Windows dsh.cmd/dsh.exe via PATHEXT). Symlinked shims are
//!    resolved to <npm-prefix>/.../node_modules/@deepseek-ai/dsh/lib/bin.js.
//! 3. "npm root -g" + /@deepseek-ai/dsh (bounded spawn, never hangs).
//! 4. Well-known Node version-manager and system global roots (fnm, nvm, volta,
//!    asdf, pnpm, bun, /usr/local, /usr, Homebrew, Windows %APPDATA%\npm).
//!    GUI sessions usually do not source the shell's fnm/nvm setup, so PATH is
//!    often incomplete when the tray is started from a desktop launcher.
//! 5. node/npm come from the discovered installation's prefix first (so the
//!    CLI runs on the Node version it was installed for), then config, then
//!    PATH.
//!
//! The version is read from the package package.json and only falls back to a
//! bounded "--version" spawn when that file is missing or unreadable.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use crate::channel::{Channel, DistTags, tag_version};
use crate::config::Config;

/// Relative path of the CLI entry point inside the npm package.
const BIN_REL: &str = "lib/bin.js";
/// Timeout for "npm root -g".
const NPM_ROOT_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout for the --version fallback; it must never hang the tray.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval while waiting for a bounded child process.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Upper bound for a captured stdout/stderr stream (bytes).
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// A resolved dsh installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DshInstall {
    /// Installed package version (from package.json).
    pub version: String,
    /// .../node_modules/@deepseek-ai/dsh
    pub package_root: PathBuf,
    /// .../lib/bin.js (the actual CLI entry point).
    pub bin_js: PathBuf,
    /// Node.js executable used to run bin.js.
    pub node: PathBuf,
    /// npm executable used for installs.
    pub npm: PathBuf,
}

impl DshInstall {
    /// Discover the installation from config overrides, PATH, npm and known
    /// Node version-manager installs.
    pub fn discover(config: &Config) -> Result<Self> {
        // Validate explicit node/npm overrides first so a typo is reported
        // immediately, even when the dsh package itself cannot be found.
        if let Some(configured) = config.node_path.as_deref() {
            if !configured.is_file() {
                bail!("node_path {} does not exist", configured.display());
            }
        }
        if let Some(configured) = config.npm_path.as_deref() {
            if !configured.is_file() {
                bail!("npm_path {} does not exist", configured.display());
            }
        }

        let package_root = match config.dsh_path.as_deref() {
            Some(configured) => {
                let (package_root, bin_js) = resolve_install_path(configured).ok_or_else(|| {
                    anyhow!(
                        "dsh_path {} does not look like a dsh installation (expected \
                         <npm-root>/@deepseek-ai/dsh/lib/bin.js, a dsh executable/shim, \
                         or the package directory)",
                        configured.display()
                    )
                })?;
                if !bin_js.is_file() {
                    bail!(
                        "dsh_path {} resolves to {}, but that file does not exist",
                        configured.display(),
                        bin_js.display()
                    );
                }
                package_root
            }
            None => discover_on_system(config).ok_or_else(install_hint)?,
        };
        let bin_js = package_root.join(BIN_REL);
        let node = resolve_node(config, &package_root)?;
        let npm = resolve_npm(config, &package_root)?;

        let version = read_package_version(&package_root)
            .or_else(|| version_from_cli(&node, &bin_js))
            .ok_or_else(|| {
                anyhow!(
                    "found dsh at {} but could not determine its version \
                     (no readable package.json and '--version' printed nothing)",
                    package_root.display()
                )
            })?;

        Ok(Self {
            version,
            package_root,
            bin_js,
            node,
            npm,
        })
    }
}

/// Actionable error used when nothing could be discovered.
fn install_hint() -> anyhow::Error {
    anyhow!(
        "dsh not found; install it with: npm install -g {}@latest \
         (or set dsh_path in the config to <npm-root>/{}/lib/bin.js)",
        crate::DSH_PACKAGE,
        crate::DSH_PACKAGE
    )
}

/// Resolve npm: config override, the Node install next to dsh, then PATH.
fn resolve_npm(config: &Config, package_root: &Path) -> Result<PathBuf> {
    if let Some(configured) = config.npm_path.as_deref() {
        return Ok(configured.to_path_buf());
    }
    if let Some(found) = npm_for_installation(package_root) {
        return Ok(found);
    }
    find_in_path("npm").ok_or_else(|| {
        anyhow!(
            "npm not found on PATH; install Node.js from https://nodejs.org/ \
             or set npm_path in the config"
        )
    })
}

/// Resolve node: config override, the Node install next to dsh, then PATH.
fn resolve_node(config: &Config, package_root: &Path) -> Result<PathBuf> {
    if let Some(configured) = config.node_path.as_deref() {
        return Ok(configured.to_path_buf());
    }
    if let Some(found) = node_for_installation(package_root) {
        return Ok(found);
    }
    if let Some(found) = find_in_path("node") {
        return Ok(found);
    }
    #[cfg(windows)]
    {
        // Node.js ships node.exe next to npm.cmd in the same installation dir.
        if let Some(npm) = npm_for_installation(package_root).or_else(|| find_in_path("npm")) {
            if let Some(dir) = npm.parent() {
                for name in ["node.exe", "node"] {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    bail!(
        "node not found on PATH; install Node.js from https://nodejs.org/ \
         or set node_path in the config"
    )
}

/// Discovery when no override is configured: PATH shim, npm's global root, then
/// the well-known version-manager/system locations.
fn discover_on_system(config: &Config) -> Option<PathBuf> {
    if let Some(shim) = find_in_path("dsh") {
        if let Some((package_root, bin_js)) = resolve_install_path(&shim) {
            if bin_js.is_file() {
                return Some(package_root);
            }
        }
        crate::logging::warn(format!(
            "found {} on PATH but could not resolve it to a dsh package; trying npm",
            shim.display()
        ));
    }

    if let Some(npm) = config.npm_path.clone().or_else(|| find_in_path("npm")) {
        if let Some(package_root) = npm_global_install(&npm) {
            return Some(package_root);
        }
    }

    best_known_installation()
}

/// Ask npm where global packages live and look for dsh there.
fn npm_global_install(npm: &Path) -> Option<PathBuf> {
    let captured = match run_with_timeout(npm, ["root", "-g"], NPM_ROOT_TIMEOUT) {
        Ok(captured) => captured,
        Err(error) => {
            crate::logging::warn(format!("could not run 'npm root -g': {error:#}"));
            return None;
        }
    };
    if captured.timed_out {
        crate::logging::warn(format!("'npm root -g' timed out after {}s", NPM_ROOT_TIMEOUT.as_secs()));
        return None;
    }
    if !captured.success {
        crate::logging::warn(format!("'npm root -g' failed: {}", captured.stderr.trim()));
        return None;
    }
    let root_dir = captured.stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    let package_root = PathBuf::from(root_dir).join(crate::DSH_PACKAGE);
    package_root.join(BIN_REL).is_file().then_some(package_root)
}

/// Return the Node installation prefix that owns a global npm package:
/// <prefix>/lib/node_modules/@scope/pkg (Unix) or <prefix>/node_modules/@scope/pkg.
fn installation_prefix(package_root: &Path) -> Option<PathBuf> {
    let modules = package_root.parent()?.parent()?;
    if modules.file_name()? != "node_modules" {
        return None;
    }
    let parent = modules.parent()?;
    if parent.file_name()? == "lib" {
        parent.parent().map(Path::to_path_buf)
    } else {
        Some(parent.to_path_buf())
    }
}

fn node_for_installation(package_root: &Path) -> Option<PathBuf> {
    let prefix = installation_prefix(package_root)?;
    #[cfg(unix)]
    {
        let candidate = prefix.join("bin/node");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    #[cfg(windows)]
    {
        for candidate in [prefix.join("node.exe"), prefix.join("bin/node.exe")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn npm_for_installation(package_root: &Path) -> Option<PathBuf> {
    let prefix = installation_prefix(package_root)?;
    #[cfg(unix)]
    {
        for candidate in [prefix.join("bin/npm"), prefix.join("npm")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    #[cfg(windows)]
    {
        for candidate in [prefix.join("npm.cmd"), prefix.join("npm.exe"), prefix.join("npm")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Best dsh package found under known per-user Node installations.
fn best_known_installation() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home_dir() {
        collect_known_for_home(&home, &mut candidates);
    }
    collect_system_installations(&mut candidates);
    let best = pick_newest_installation(candidates);
    if let Some(package_root) = &best {
        crate::logging::info(format!(
            "found dsh via a local Node installation: {}",
            package_root.display()
        ));
    }
    best
}

fn home_dir() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable).map(PathBuf::from)
}

/// Candidate dsh package roots for one user's home (fnm/nvm/volta/asdf/pnpm/bun
/// and user-level npm prefixes).
fn known_installation_candidates(home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    collect_known_for_home(home, &mut candidates);
    candidates.sort();
    candidates.dedup();
    candidates
}

fn collect_known_for_home(home: &Path, out: &mut Vec<PathBuf>) {
    // fnm (Linux/macOS and Windows layouts).
    for base in [
        home.join(".local/share/fnm/node-versions"),
        home.join(".fnm/node-versions"),
    ] {
        collect_versioned(&base, &["installation", "lib", "node_modules"], out);
        collect_versioned(&base, &["installation", "node_modules"], out);
    }
    // nvm, volta, asdf, pnpm, bun.
    collect_versioned(&home.join(".nvm/versions/node"), &["lib", "node_modules"], out);
    collect_versioned(&home.join(".volta/tools/image/node"), &["lib", "node_modules"], out);
    collect_versioned(
        &home.join(".asdf/installs/nodejs"),
        &[".npm-global", "lib", "node_modules"],
        out,
    );
    collect_versioned(&home.join(".local/share/pnpm/global"), &["node_modules"], out);
    collect_versioned(&home.join("AppData/Local/pnpm/global"), &["node_modules"], out);
    collect_versioned(&home.join("AppData/Roaming/nvm"), &["node_modules"], out);
    add_installation(&home.join(".bun/install/global/node_modules"), out);
    add_installation(&home.join(".npm-global/lib/node_modules"), out);
    add_installation(&home.join("AppData/Roaming/npm/node_modules"), out);
}

fn collect_system_installations(out: &mut Vec<PathBuf>) {
    #[cfg(unix)]
    {
        for modules in [
            "/usr/local/lib/node_modules",
            "/usr/lib/node_modules",
            "/opt/homebrew/lib/node_modules",
            "/opt/local/lib/node_modules",
        ] {
            add_installation(Path::new(modules), out);
        }
    }
    #[cfg(windows)]
    {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(prefix) = std::env::var_os(variable) {
                add_installation(&Path::new(&prefix).join("nodejs").join("node_modules"), out);
            }
        }
    }
}

fn collect_versioned(base: &Path, rest: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let mut modules = entry.path();
        for part in rest {
            modules.push(part);
        }
        add_installation(&modules, out);
    }
}

fn add_installation(node_modules: &Path, out: &mut Vec<PathBuf>) {
    let package_root = node_modules.join(crate::DSH_PACKAGE);
    if package_root.join(BIN_REL).is_file() {
        out.push(package_root);
    }
}

fn installation_version_rank(package_root: &Path) -> Option<Version> {
    read_package_version(package_root).and_then(|version| parse_version_lenient(&version))
}

fn pick_newest_installation(mut candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .max_by_key(|package_root| installation_version_rank(package_root))
}

/// Find a usable npm from well-known Node installations when PATH does not
/// include the version manager (common for applications started by the desktop).
pub(crate) fn find_known_npm() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home_dir() {
        known_npm_candidates(&home, &mut candidates);
    }
    #[cfg(unix)]
    {
        for candidate in ["/usr/local/bin/npm", "/opt/homebrew/bin/npm", "/usr/bin/npm"] {
            let path = Path::new(candidate);
            if path.is_file() {
                candidates.push(path.to_path_buf());
            }
        }
    }
    #[cfg(windows)]
    {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(prefix) = std::env::var_os(variable) {
                let candidate = Path::new(&prefix).join("nodejs").join("npm.cmd");
                if candidate.is_file() {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates.into_iter().max_by_key(|npm| npm_version_rank(npm))
}

fn known_npm_candidates(home: &Path, out: &mut Vec<PathBuf>) {
    for base in [
        home.join(".local/share/fnm/node-versions"),
        home.join(".fnm/node-versions"),
    ] {
        #[cfg(unix)]
        collect_versioned_file(&base, &["installation", "bin", "npm"], out);
        #[cfg(windows)]
        {
            collect_versioned_file(&base, &["installation", "npm.cmd"], out);
            collect_versioned_file(&base, &["installation", "bin", "npm.cmd"], out);
        }
    }
    #[cfg(unix)]
    {
        collect_versioned_file(&home.join(".nvm/versions/node"), &["bin", "npm"], out);
        collect_versioned_file(&home.join(".volta/tools/image/node"), &["bin", "npm"], out);
        collect_versioned_file(&home.join(".asdf/installs/nodejs"), &[".npm-global", "bin", "npm"], out);
        add_npm_path(&home.join(".npm-global/bin/npm"), out);
        add_npm_path(&home.join(".local/bin/npm"), out);
    }
    #[cfg(windows)]
    {
        collect_versioned_file(&home.join("AppData/Roaming/nvm"), &["npm.cmd"], out);
        collect_versioned_file(&home.join("AppData/Local/fnm/node-versions"), &["npm.cmd"], out);
        add_npm_path(&home.join("AppData/Roaming/npm/npm.cmd"), out);
    }
}

fn collect_versioned_file(base: &Path, rest: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let mut path = entry.path();
        for part in rest {
            path.push(part);
        }
        if path.is_file() {
            out.push(path);
        }
    }
}

fn add_npm_path(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
    }
}

fn npm_version_rank(npm: &Path) -> Option<Version> {
    npm.ancestors()
        .filter_map(|ancestor| ancestor.file_name()?.to_str())
        .filter_map(parse_version_lenient)
        .max()
}

/// Normalise anything that points at a dsh installation into (package_root, bin_js).
///
/// Accepts the package directory, the lib/bin.js entry point, a symlink to it,
/// and a dsh shim whose install prefix contains the npm package.
fn resolve_install_path(path: &Path) -> Option<(PathBuf, PathBuf)> {
    if path.is_dir() {
        let bin_js = path.join(BIN_REL);
        if bin_js.is_file() {
            return Some((path.to_path_buf(), bin_js));
        }
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(package_root) = package_root_from_bin_js(&canonical) {
        return Some((package_root, canonical));
    }
    for candidate in shim_package_candidates(path) {
        if candidate.is_file() {
            if let Some(package_root) = package_root_from_bin_js(&candidate) {
                return Some((package_root, candidate));
            }
        }
    }
    None
}

/// Turn .../dsh/lib/bin.js into .../dsh.
fn package_root_from_bin_js(bin_js: &Path) -> Option<PathBuf> {
    let is_entry = bin_js.file_name()? == "bin.js" && bin_js.parent()?.file_name()? == "lib";
    if !is_entry {
        return None;
    }
    Some(bin_js.parent()?.parent()?.to_path_buf())
}

/// Package entry points implied by a dsh shim install prefix.
fn shim_package_candidates(shim: &Path) -> Vec<PathBuf> {
    let Some(dir) = shim.parent() else { return Vec::new() };
    [
        // Windows: <prefix>/dsh.cmd -> <prefix>/node_modules/@deepseek-ai/dsh
        dir.join("node_modules").join(crate::DSH_PACKAGE).join(BIN_REL),
        // Unix: <prefix>/lib/node_modules/@deepseek-ai/dsh
        dir.join("lib")
            .join("node_modules")
            .join(crate::DSH_PACKAGE)
            .join(BIN_REL),
        // Unix: <prefix>/bin/dsh -> <prefix>/lib/node_modules/@deepseek-ai/dsh
        dir.join("..")
            .join("lib")
            .join("node_modules")
            .join(crate::DSH_PACKAGE)
            .join(BIN_REL),
        // Portable installs that keep node_modules next to bin/.
        dir.join("..")
            .join("node_modules")
            .join(crate::DSH_PACKAGE)
            .join(BIN_REL),
    ]
    .into_iter()
    .map(|candidate| candidate.components().collect())
    .collect()
}

/// Read the version field from <package_root>/package.json.
fn read_package_version(package_root: &Path) -> Option<String> {
    let path = package_root.join("package.json");
    let text = fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let version = value.get("version")?.as_str()?.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Bounded fallback: run "node bin.js --version" and take the first non-empty line.
fn version_from_cli(node: &Path, bin_js: &Path) -> Option<String> {
    let captured = run_with_timeout(node, [bin_js.as_os_str(), OsStr::new("--version")], VERSION_TIMEOUT).ok()?;
    if captured.timed_out {
        crate::logging::warn(format!(
            "'{} --version' timed out after {}s",
            bin_js.display(),
            VERSION_TIMEOUT.as_secs()
        ));
        return None;
    }
    if !captured.success {
        crate::logging::warn(format!(
            "'{} --version' failed: {}",
            bin_js.display(),
            captured.stderr.trim()
        ));
        return None;
    }
    let line = captured.stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.to_string())
}

/// Find an executable on PATH (Windows: also PATHEXT variants like npm.cmd).
pub(crate) fn find_in_path(program: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    // Only Windows consults PATHEXT; treating .cmd files as executable on Unix
    // would match shell shims that cannot be spawned directly.
    let pathext = if cfg!(windows) {
        std::env::var_os("PATHEXT").or_else(|| Some(OsString::from(".COM;.EXE;.BAT;.CMD")))
    } else {
        None
    };
    find_in_path_in(program, &path_var, pathext.as_deref())
}

/// Testable core of find_in_path.
fn find_in_path_in(program: &str, path_var: &OsStr, pathext: Option<&OsStr>) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let direct = dir.join(program);
        if is_executable(&direct) {
            return Some(direct);
        }
        if let Some(pathext) = pathext {
            for extension in pathext_extensions(pathext) {
                let candidate = dir.join(format!("{program}{extension}"));
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Split a PATHEXT value (".COM;.EXE;.BAT;.CMD") into normalised extensions.
fn pathext_extensions(pathext: &OsStr) -> Vec<String> {
    let list = pathext.to_string_lossy();
    list.split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            if extension.starts_with('.') {
                extension.to_string()
            } else {
                format!(".{extension}")
            }
        })
        .collect()
}

/// A file that this platform can execute directly.
fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        match fs::metadata(path) {
            Ok(metadata) => metadata.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Result of a bounded child process run.
#[derive(Debug)]
pub(crate) struct Captured {
    /// Raw stdout (bounded to MAX_CAPTURE_BYTES).
    pub stdout: String,
    /// Raw stderr (bounded to MAX_CAPTURE_BYTES).
    pub stderr: String,
    /// The process exited successfully.
    pub success: bool,
    /// The process was killed because it exceeded the timeout.
    pub timed_out: bool,
}

/// Run a program with args, capture output, and kill it after the timeout.
///
/// Used for the few CLI probes where a stuck child would freeze the tray:
/// npm root -g, npm config get registry and --version.
pub(crate) fn run_with_timeout<I, S>(program: &Path, args: I, timeout: Duration) -> Result<Captured>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Own process group: a timeout kills shell wrappers and their children,
        // otherwise a grandchild could keep the output pipes open past the
        // deadline.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run {}", program.display()))?;
    let pid = child.id();

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                kill_tree(pid);
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).with_context(|| format!("waiting for {}", program.display()));
            }
        }
        if started.elapsed() >= timeout {
            kill_tree(pid);
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Captured {
                stdout: String::new(),
                stderr: String::new(),
                success: false,
                timed_out: true,
            });
        }
        thread::sleep(POLL_INTERVAL);
    }

    // The child is gone, so draining the pipes terminates. These probes print a
    // few short lines, far below the pipe buffer size.
    let output = child
        .wait_with_output()
        .with_context(|| format!("collecting the output of {}", program.display()))?;
    Ok(Captured {
        stdout: bounded(String::from_utf8_lossy(&output.stdout).into_owned()),
        stderr: bounded(String::from_utf8_lossy(&output.stderr).into_owned()),
        success: output.status.success(),
        timed_out: false,
    })
}

/// Clamp captured text to MAX_CAPTURE_BYTES characters.
fn bounded(text: String) -> String {
    if text.chars().count() <= MAX_CAPTURE_BYTES {
        return text;
    }
    text.chars().take(MAX_CAPTURE_BYTES).collect()
}

/// Force-kill a bounded probe and everything in its process group.
#[cfg(unix)]
fn kill_tree(pid: u32) {
    let _ = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn kill_tree(_pid: u32) {}

/// Decide which channel the installed version most plausibly belongs to.
///
/// 1. The installed version exactly matches a tag version -> that channel.
/// 2. Several tags share that version (today latest == next) -> the persisted
///    preference when it is one of them, otherwise the most stable one.
/// 3. No exact match -> the prerelease suffix is a hint (-alpha -> Alpha,
///    -rc/-beta/-next -> Next), otherwise the preference.
pub fn detect_channel(version: &str, tags: &DistTags, preferred: Channel) -> Channel {
    let matches: Vec<Channel> = Channel::ALL
        .iter()
        .copied()
        .filter(|channel| {
            tag_version(tags, *channel)
                .map(|tagged| tagged.trim() == version.trim())
                .unwrap_or(false)
        })
        .collect();
    if matches.len() == 1 {
        return matches[0];
    }
    if matches.len() > 1 {
        return if matches.contains(&preferred) {
            preferred
        } else {
            matches[0]
        };
    }
    match parse_version_lenient(version)
        .map(|parsed| parsed.pre.as_str().to_string())
        .and_then(|pre| pre.split('.').next().map(str::to_string))
    {
        Some(pre) if pre == "alpha" => Channel::Alpha,
        Some(pre) if matches!(pre.as_str(), "rc" | "beta" | "next" | "pre" | "preview") => Channel::Next,
        _ => preferred,
    }
}

/// Parse a version leniently: tolerates a leading v/V or = and whitespace.
pub fn parse_version_lenient(value: &str) -> Option<Version> {
    let trimmed = value.trim();
    let trimmed = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_prefix('=').unwrap_or(trimmed).trim();
    Version::parse(trimmed).ok()
}

/// Compare the installed version with the candidate a dist-tag points at.
///
/// Returns (is_newer, candidate_version); ordering is semver, never string
/// comparison. An unparseable candidate yields (false, None); an unparseable
/// installed version is treated as "update available" because it cannot be
/// verified (for example a local/dev build).
pub fn is_update_available(installed: &str, candidate: &str) -> (bool, Option<Version>) {
    let Some(candidate_version) = parse_version_lenient(candidate) else {
        return (false, None);
    };
    match parse_version_lenient(installed) {
        Some(installed_version) => (candidate_version > installed_version, Some(candidate_version)),
        None => (true, Some(candidate_version)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }

    #[cfg(unix)]
    fn make_file(path: &Path, executable: bool) {
        fs::write(path, b"x").expect("write file");
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    /// Canonical form of a path (macOS /var -> /private/var, Windows verbatim
    /// prefixes and 8.3 short names, symlinked temp dirs).
    fn canonical(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|error| panic!("canonicalize {}: {error}", path.display()))
    }

    fn tags(pairs: &[(&str, &str)]) -> DistTags {
        pairs
            .iter()
            .map(|(tag, version)| ((*tag).to_string(), (*version).to_string()))
            .collect()
    }

    /// Create a fake global package below the given node_modules directory.
    fn fake_package(node_modules: &Path, version: &str) -> PathBuf {
        let package_root = node_modules.join(crate::DSH_PACKAGE);
        fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        fs::write(package_root.join("lib/bin.js"), b"// entry\n").expect("bin.js");
        fs::write(
            package_root.join("package.json"),
            format!(r#"{{"name":"{}","version":"{version}"}}"#, crate::DSH_PACKAGE),
        )
        .expect("package.json");
        package_root
    }

    #[test]
    fn find_in_path_respects_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = dir.path().join("a");
        let second = dir.path().join("b");
        fs::create_dir_all(&first).expect("mkdir");
        fs::create_dir_all(&second).expect("mkdir");
        make_executable(&first.join("tool"));
        make_executable(&second.join("tool"));
        let path_var = std::env::join_paths([&first, &second]).expect("join paths");
        let found = find_in_path_in("tool", &path_var, None).expect("found");
        assert_eq!(found, first.join("tool"));
        assert_eq!(find_in_path_in("missing", &path_var, None), None);
    }

    #[test]
    fn find_in_path_uses_pathext() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        make_executable(&bin.join("dsh.cmd"));
        let path_var = std::env::join_paths([&bin]).expect("join paths");
        let found = find_in_path_in("dsh", &path_var, Some(OsStr::new(".COM;.EXE;.BAT;.cmd"))).expect("shim found");
        assert_eq!(found, bin.join("dsh.cmd"));
    }

    #[test]
    fn pathext_is_normalised() {
        let extensions = pathext_extensions(OsStr::new(" .COM ; EXE ;; .cmd "));
        assert_eq!(
            extensions,
            vec![".COM".to_string(), ".EXE".to_string(), ".cmd".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn find_in_path_skips_non_executables() {
        let dir = tempfile::tempdir().expect("tempdir");
        make_file(&dir.path().join("tool"), false);
        let path_var = std::env::join_paths([dir.path()]).expect("join paths");
        assert_eq!(find_in_path_in("tool", &path_var, None), None);
    }

    #[test]
    fn package_root_is_derived_from_entry_point() {
        let bin_js = Path::new("/opt/npm/lib/node_modules/@deepseek-ai/dsh/lib/bin.js");
        assert_eq!(
            package_root_from_bin_js(bin_js),
            Some(PathBuf::from("/opt/npm/lib/node_modules/@deepseek-ai/dsh"))
        );
        assert_eq!(package_root_from_bin_js(Path::new("/opt/npm/dsh/lib/other.js")), None);
        assert_eq!(package_root_from_bin_js(Path::new("/opt/npm/dsh/bin.js")), None);
    }

    #[test]
    fn resolve_install_path_accepts_package_directory_and_entry_point() {
        let dir = tempfile::tempdir().expect("tempdir");
        let package_root = dir.path().join("lib/node_modules/@deepseek-ai/dsh");
        fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        fs::write(package_root.join("lib/bin.js"), b"// entry\n").expect("write bin.js");
        let resolved_dir = resolve_install_path(&package_root).expect("package dir");
        assert_eq!(resolved_dir.0, package_root);
        let resolved_js = resolve_install_path(&package_root.join("lib/bin.js")).expect("bin.js");
        assert_eq!(resolved_js.1, canonical(&package_root.join("lib/bin.js")));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_install_path_follows_symlinked_shim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let package_root = dir.path().join("lib/node_modules/@deepseek-ai/dsh");
        fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        fs::write(package_root.join("lib/bin.js"), b"// entry\n").expect("write bin.js");
        let shim = dir.path().join("bin/dsh");
        fs::create_dir_all(shim.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(package_root.join("lib/bin.js"), &shim).expect("symlink");
        let (root, bin_js) = resolve_install_path(&shim).expect("resolved");
        assert_eq!(root, canonical(&package_root));
        assert_eq!(bin_js, canonical(&package_root.join("lib/bin.js")));
    }

    #[test]
    fn resolve_install_path_finds_windows_shim_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let package_root = dir.path().join("node_modules/@deepseek-ai/dsh");
        fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        fs::write(package_root.join("lib/bin.js"), b"// entry\n").expect("write bin.js");
        let shim = dir.path().join("dsh.cmd");
        fs::write(&shim, br#"@node "%~dp0node_modules\@deepseek-ai\dsh\lib\bin.js" %*"#).expect("write shim");
        let (root, bin_js) = resolve_install_path(&shim).expect("resolved");
        assert_eq!(root, package_root);
        assert_eq!(bin_js, package_root.join("lib/bin.js"));
    }

    #[test]
    fn reads_package_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("package.json"),
            br#"{"name":"@deepseek-ai/dsh","version":"0.2.1-rc.5"}"#,
        )
        .expect("write package.json");
        assert_eq!(read_package_version(dir.path()), Some("0.2.1-rc.5".to_string()));
    }

    #[test]
    fn missing_or_corrupt_package_json_yields_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_package_version(dir.path()), None);
        fs::write(dir.path().join("package.json"), b"{ not json").expect("write package.json");
        assert_eq!(read_package_version(dir.path()), None);
        fs::write(dir.path().join("package.json"), br#"{"version":""}"#).expect("write package.json");
        assert_eq!(read_package_version(dir.path()), None);
    }

    #[test]
    fn discover_uses_config_overrides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let package_root = dir.path().join("node_modules/@deepseek-ai/dsh");
        fs::create_dir_all(package_root.join("lib")).expect("mkdir");
        fs::write(package_root.join("lib/bin.js"), b"// entry\n").expect("write bin.js");
        fs::write(package_root.join("package.json"), br#"{"version":"9.9.9-test"}"#).expect("write package.json");
        let node = dir.path().join("node");
        let npm = dir.path().join("npm");
        make_executable(&node);
        make_executable(&npm);
        let config = Config {
            dsh_path: Some(package_root.join("lib/bin.js")),
            node_path: Some(node.clone()),
            npm_path: Some(npm.clone()),
            ..Config::default()
        };
        let install = DshInstall::discover(&config).expect("discover");
        assert_eq!(install.version, "9.9.9-test");
        assert_eq!(install.package_root, canonical(&package_root));
        assert_eq!(install.bin_js, canonical(&package_root.join("lib/bin.js")));
        assert_eq!(install.node, node);
        assert_eq!(install.npm, npm);
    }

    #[test]
    fn discover_reports_bad_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let node = dir.path().join("node");
        let npm = dir.path().join("npm");
        make_executable(&node);
        make_executable(&npm);
        let config = Config {
            dsh_path: Some(dir.path().join("no-such-file")),
            node_path: Some(node),
            npm_path: Some(npm),
            ..Config::default()
        };
        let error = DshInstall::discover(&config).expect_err("must fail");
        assert!(format!("{error}").contains("dsh_path"), "{error}");
    }

    #[test]
    fn discover_rejects_missing_node_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Config {
            dsh_path: Some(dir.path().join("missing")),
            node_path: Some(dir.path().join("no-node")),
            ..Config::default()
        };
        let error = DshInstall::discover(&config).expect_err("must fail");
        assert!(format!("{error}").contains("node_path"), "{error}");
    }

    #[test]
    fn installation_prefix_handles_unix_and_windows_layouts() {
        assert_eq!(
            installation_prefix(Path::new("/usr/local/lib/node_modules/@deepseek-ai/dsh")),
            Some(PathBuf::from("/usr/local"))
        );
        assert_eq!(
            installation_prefix(Path::new("/home/u/AppData/Roaming/npm/node_modules/@deepseek-ai/dsh")),
            Some(PathBuf::from("/home/u/AppData/Roaming/npm"))
        );
        assert_eq!(installation_prefix(Path::new("/tmp/not-a-global-package")), None);
    }

    #[cfg(unix)]
    #[test]
    fn node_and_npm_are_taken_from_the_install_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let prefix = dir.path().join("node-v26");
        let package_root = fake_package(&prefix.join("lib/node_modules"), "0.2.1-rc.5");
        fs::create_dir_all(prefix.join("bin")).expect("mkdir bin");
        make_executable(&prefix.join("bin/node"));
        make_executable(&prefix.join("bin/npm"));

        let config = Config {
            dsh_path: Some(package_root.join("lib/bin.js")),
            ..Config::default()
        };
        let install = DshInstall::discover(&config).expect("discover");
        let prefix = canonical(&prefix);
        assert_eq!(install.node, prefix.join("bin/node"));
        assert_eq!(install.npm, prefix.join("bin/npm"));
    }

    #[test]
    fn known_candidates_cover_fnm_and_nvm_layouts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let fnm = fake_package(
            &home.join(".local/share/fnm/node-versions/v26.4.0/installation/lib/node_modules"),
            "0.2.1-rc.5",
        );
        let nvm = fake_package(&home.join(".nvm/versions/node/v22.0.0/lib/node_modules"), "0.2.1-rc.5");
        let candidates = known_installation_candidates(home);
        assert!(candidates.contains(&fnm), "fnm package missing: {candidates:?}");
        assert!(candidates.contains(&nvm), "nvm package missing: {candidates:?}");
    }

    #[cfg(unix)]
    #[test]
    fn known_npm_candidates_include_fnm_npm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let npm = home.join(".local/share/fnm/node-versions/v26.4.0/installation/bin/npm");
        fs::create_dir_all(npm.parent().expect("parent")).expect("mkdir");
        make_executable(&npm);
        let mut candidates = Vec::new();
        known_npm_candidates(home, &mut candidates);
        assert!(candidates.contains(&npm), "{candidates:?}");
        assert_eq!(npm_version_rank(&npm), Some(Version::parse("26.4.0").expect("version")));
    }

    #[test]
    fn newest_known_installation_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let older = fake_package(
            &home.join(".local/share/fnm/node-versions/v24.0.0/installation/lib/node_modules"),
            "0.2.1",
        );
        let newer = fake_package(
            &home.join(".local/share/fnm/node-versions/v26.4.0/installation/lib/node_modules"),
            "0.3.0",
        );
        let candidates = known_installation_candidates(home);
        assert_eq!(pick_newest_installation(candidates.clone()), Some(newer.clone()));
        assert_ne!(pick_newest_installation(candidates), Some(older));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_captures_output() {
        let captured = run_with_timeout(
            Path::new("/bin/sh"),
            ["-c", "printf 'hello\\n'; printf 'oops\\n' >&2"],
            Duration::from_secs(5),
        )
        .expect("run");
        assert!(captured.success);
        assert!(!captured.timed_out);
        assert_eq!(captured.stdout, "hello\n");
        assert_eq!(captured.stderr, "oops\n");
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_kills_slow_children() {
        let started = Instant::now();
        let captured =
            run_with_timeout(Path::new("/bin/sh"), ["-c", "sleep 30"], Duration::from_millis(250)).expect("run");
        assert!(captured.timed_out);
        assert!(!captured.success);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn detect_channel_exact_match_wins() {
        let tags = tags(&[("latest", "0.2.1"), ("next", "0.3.0-rc.1"), ("alpha", "0.4.0-alpha.2")]);
        assert_eq!(detect_channel("0.3.0-rc.1", &tags, Channel::Latest), Channel::Next);
        assert_eq!(detect_channel("0.4.0-alpha.2", &tags, Channel::Latest), Channel::Alpha);
        assert_eq!(detect_channel("0.2.1", &tags, Channel::Alpha), Channel::Latest);
    }

    #[test]
    fn detect_channel_ties_use_preference() {
        let tags = tags(&[("latest", "0.2.1"), ("next", "0.2.1"), ("alpha", "0.3.0-alpha.1")]);
        assert_eq!(detect_channel("0.2.1", &tags, Channel::Next), Channel::Next);
        assert_eq!(detect_channel("0.2.1", &tags, Channel::Latest), Channel::Latest);
        // preference not among the tied channels -> most stable one
        assert_eq!(detect_channel("0.2.1", &tags, Channel::Alpha), Channel::Latest);
    }

    #[test]
    fn detect_channel_uses_prerelease_hint() {
        let tags = tags(&[("latest", "0.3.0"), ("next", "0.3.0"), ("alpha", "0.4.0-alpha.1")]);
        assert_eq!(detect_channel("0.3.1-rc.2", &tags, Channel::Latest), Channel::Next);
        assert_eq!(detect_channel("0.3.1-alpha.2", &tags, Channel::Latest), Channel::Alpha);
        assert_eq!(detect_channel("0.3.1-beta.1", &tags, Channel::Latest), Channel::Next);
        assert_eq!(detect_channel("0.3.1-dev.1", &tags, Channel::Alpha), Channel::Alpha);
    }

    #[test]
    fn detect_channel_falls_back_to_preference() {
        let tags = tags(&[("latest", "0.2.1"), ("next", "0.3.0-rc.1")]);
        assert_eq!(detect_channel("9.9.9", &tags, Channel::Next), Channel::Next);
        assert_eq!(detect_channel("not-a-version", &tags, Channel::Alpha), Channel::Alpha);
        assert_eq!(
            detect_channel("0.2.1", &DistTags::new(), Channel::Latest),
            Channel::Latest
        );
    }

    #[test]
    fn lenient_version_parsing() {
        assert_eq!(
            parse_version_lenient("0.2.1-rc.5"),
            Some(Version::parse("0.2.1-rc.5").expect("v"))
        );
        assert_eq!(
            parse_version_lenient(" v0.2.1-rc.5 "),
            Some(Version::parse("0.2.1-rc.5").expect("v"))
        );
        assert_eq!(
            parse_version_lenient("=0.2.1"),
            Some(Version::parse("0.2.1").expect("v"))
        );
        assert_eq!(parse_version_lenient("garbage"), None);
        assert_eq!(parse_version_lenient("0.2"), None);
    }

    #[test]
    fn update_comparison_uses_semver() {
        let (newer, version) = is_update_available("0.2.1-rc.5", "0.2.1-rc.6");
        assert!(newer);
        assert_eq!(version, Some(Version::parse("0.2.1-rc.6").expect("v")));
        assert_eq!(
            is_update_available("0.2.1", "0.2.1"),
            (false, Some(Version::parse("0.2.1").expect("v")))
        );
        assert!(is_update_available("0.2.1-rc.5", "0.2.1").0);
        assert!(!is_update_available("0.2.1-rc.1", "0.2.1-alpha.2").0);
        assert!(is_update_available("0.2.1-alpha.2", "0.2.1-rc.1").0);
        assert!(is_update_available("0.2.1", "0.2.2-alpha.1").0);
        assert_eq!(is_update_available("0.2.1", "nightly"), (false, None));
        assert!(is_update_available("local-build", "0.2.2").0);
    }
}
