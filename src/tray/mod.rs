//! Tray rendering (tao event loop + tray-icon/muda).

pub mod menu_spec;

use tray_icon::menu::{MenuEvent, MenuId};

use crate::app::{Command, Snapshot};
use crate::config::AppPaths;

/// Events forwarded into the tao event loop.
#[derive(Debug)]
pub enum UserEvent {
    /// New controller state to render.
    Snapshot(Snapshot),
    /// A menu entry was activated.
    Menu(MenuEvent),
    /// The tray icon itself was clicked.
    Tray(tray_icon::TrayIconEvent),
    /// The controller finished shutting down; exit the loop.
    Shutdown,
}

/// Owns the tray icon and rebuilds its menu on every snapshot.
pub struct TrayUi {
    #[allow(dead_code)]
    icon: tray_icon::TrayIcon,
}

impl TrayUi {
    /// Create the tray icon on the event-loop thread.
    pub fn new(_snapshot: &Snapshot, _paths: &AppPaths) -> anyhow::Result<Self> {
        anyhow::bail!("tray not implemented yet")
    }

    /// Replace the tray menu with the one derived from `snapshot`.
    pub fn update(&mut self, _snapshot: &Snapshot) {
        // implemented by the tray workstream
    }
}

/// Map a menu item id back to a controller command.
pub fn command_for(_id: &MenuId) -> Option<Command> {
    None
}
