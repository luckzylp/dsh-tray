//! UI language resolution and localized strings.
//!
//! The tray menu is the only UI, so localization is a small static table.
//! `Lang` is resolved once at startup from the config preference or the
//! process locale.

use crate::config::Language;

/// Concrete UI language after resolving `Language::Auto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    /// Resolve the configured preference against the environment.
    pub fn resolve(preference: Language) -> Self {
        match preference {
            Language::En => Lang::En,
            Language::Zh => Lang::Zh,
            Language::Auto => Self::detect(),
        }
    }

    /// Detect from `DSH_TRAY_LANG`, `LC_ALL`, `LC_MESSAGES` or `LANG`.
    pub fn detect() -> Self {
        for variable in ["DSH_TRAY_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            let Ok(value) = std::env::var(variable) else { continue };
            let value = value.trim().to_ascii_lowercase();
            if value.is_empty() {
                continue;
            }
            if value.starts_with("zh") {
                return Lang::Zh;
            }
            if value.starts_with("en") {
                return Lang::En;
            }
        }
        Lang::En
    }

    /// BCP-47-ish code for logs and diagnostics.
    pub const fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_preferences_win() {
        assert_eq!(Lang::resolve(Language::En), Lang::En);
        assert_eq!(Lang::resolve(Language::Zh), Lang::Zh);
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(Lang::En.code(), "en");
        assert_eq!(Lang::Zh.code(), "zh");
    }
}
