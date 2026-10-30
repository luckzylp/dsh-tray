//! Pure menu model derived from a `Snapshot`.
//!
//! Keeping this a plain data structure makes menu behavior unit-testable
//! without initializing GTK/AppIndicator.

use crate::app::Snapshot;

/// Stable menu item ids shared with `tray::command_for`.
pub mod ids {
    pub const OPEN_WEB: &str = "open-web";
    pub const START: &str = "start-dsh";
    pub const STOP: &str = "stop-dsh";
    pub const RESTART: &str = "restart-dsh";
    pub const CHECK_UPDATES: &str = "check-updates";
    pub const UPDATE_NOW: &str = "update-now";
    pub const CHANNEL_PREFIX: &str = "channel:";
    pub const AUTOSTART: &str = "toggle-autostart";
    pub const START_ON_LAUNCH: &str = "toggle-start-on-launch";
    pub const OPEN_LOG: &str = "open-log";
    pub const OPEN_CONFIG: &str = "open-config";
    pub const QUIT: &str = "quit";
}

/// Visual/semantic kind of a leaf menu entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Normal,
    Check { checked: bool },
}

/// A clickable menu entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub kind: ItemKind,
}

/// One node of the menu tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuSpecItem {
    Separator,
    Entry(MenuEntry),
    Submenu { label: String, items: Vec<MenuSpecItem> },
}

/// The complete tray menu.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MenuSpec {
    pub items: Vec<MenuSpecItem>,
}

/// Build the menu for `snapshot` (labels localized via `i18n`).
pub fn build(_snapshot: &Snapshot) -> MenuSpec {
    MenuSpec::default()
}
