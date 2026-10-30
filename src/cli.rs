//! Command-line parsing and non-GUI diagnostics.

/// What the binary should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Run the tray (default), optionally launched by an autostart entry.
    Tray { hidden: bool },
    /// Print a health/diagnostic report and exit.
    Check { json: bool },
    /// Start dsh, wait for readiness, verify HTTP, stop it, then exit.
    Smoke,
    /// Print `dsh-tray <version>`.
    Version,
    /// Print usage.
    Help,
}

/// Parse CLI arguments (without the executable name).
pub fn parse(_args: impl IntoIterator<Item = String>) -> anyhow::Result<Action> {
    anyhow::bail!("cli parsing not implemented yet")
}

/// Usage text.
pub fn usage() -> &'static str {
    "dsh-tray [--hidden] [--check [--json]] [--smoke] [--version] [--help]"
}
