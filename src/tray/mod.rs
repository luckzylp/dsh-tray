//! Tray rendering (tao event loop + tray-icon/muda).
//!
//! The tray icon and its menu are created on the thread that runs the tao
//! event loop (a platform requirement, in particular on macOS), and rebuilt
//! from [`Snapshot`] values as they arrive. All decision making lives in the
//! pure [`menu_spec`] model; this module only translates that model into muda
//! items and maps menu ids back to [`Command`]s.
//!
//! Menu ownership: [`tray_icon`] takes ownership of the [`Menu`] passed to
//! [`TrayIcon::set_menu`]. The temporary leaf items created while rendering are
//! cloned into the menu tree by muda before they are dropped, so they are safe
//! to allocate locally.

pub mod menu_spec;

use anyhow::{anyhow, Context as _};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::app::{Command, Notice, Snapshot};
use crate::channel::Channel;
use crate::config::{AppPaths, Language};
use crate::i18n;
use crate::logging;
use menu_spec::{ItemKind, MenuSpec, MenuSpecItem};

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

/// Which whale icon the tray shows for the current state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    /// Nothing is running (or starting/stopping): neutral gray whale.
    Idle,
    /// The dsh web service is running: green whale.
    Running,
    /// The last operation failed or the service crashed: red whale.
    Error,
}

/// Map a snapshot to the coloured status icon.
///
/// A running service wins so a healthy instance stays green even when an
/// unrelated notice is present; otherwise lifecycle/update/config failures
/// show the red whale, and everything else is the neutral gray one.
pub fn icon_kind(snapshot: &Snapshot) -> IconKind {
    if snapshot.state.is_running() {
        return IconKind::Running;
    }
    let failed = matches!(
        snapshot.notice,
        Some(
            Notice::StartFailed
                | Notice::StopFailed
                | Notice::StartupTimedOut
                | Notice::UpdateFailed
                | Notice::ConfigFailed
                | Notice::AutostartFailed
        )
    );
    if failed {
        IconKind::Error
    } else {
        IconKind::Idle
    }
}

/// Owns the tray icon and rebuilds its menu on every snapshot.
pub struct TrayUi {
    icon: TrayIcon,
    icon_kind: IconKind,
}

impl TrayUi {
    /// Create the tray icon on the event-loop thread.
    ///
    /// Must be called after tao has initialised GTK (i.e. from
    /// [`StartCause::Init`](tao::event::StartCause::Init)). Returns an error instead of
    /// panicking when the platform has no tray host or the icon cannot be
    /// rendered.
    pub fn new(snapshot: &Snapshot, paths: &AppPaths) -> anyhow::Result<Self> {
        let icon_kind = icon_kind(snapshot);
        let icon = load_icon(icon_kind).context("load embedded tray icon")?;
        let menu = render_menu(&menu_spec::build(snapshot)).context("build tray menu")?;
        let tooltip = tooltip_text(snapshot);

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_temp_dir_path(paths.tray_temp_dir())
            .with_icon(icon)
            .build()
            .context("create tray icon")?;
        logging::info(format!("tray icon: {icon_kind:?}"));
        Ok(Self { icon, icon_kind })
    }

    /// Replace the tray menu and tooltip with the ones derived from [`snapshot`].
    ///
    /// Failures are logged and swallowed: a stale menu must never take the
    /// event loop down.
    pub fn update(&mut self, snapshot: &Snapshot) {
        match render_menu(&menu_spec::build(snapshot)) {
            // tray-icon keeps ownership of the menu from here on.
            Ok(menu) => self
                .icon
                .set_menu(Some(Box::new(menu) as Box<dyn tray_icon::menu::ContextMenu>)),
            Err(error) => logging::warn(format!("could not rebuild tray menu: {error:#}")),
        }

        if let Err(error) = self.icon.set_tooltip(Some(tooltip_text(snapshot))) {
            logging::warn(format!("could not update tray tooltip: {error}"));
        }

        // Swapping the icon is cheap but visible; only do it when the state
        // class actually changes (the controller emits frequent snapshots).
        let icon_kind = icon_kind(snapshot);
        if icon_kind != self.icon_kind {
            match load_icon(icon_kind) {
                Ok(icon) => match self.icon.set_icon(Some(icon)) {
                    Ok(()) => {
                        self.icon_kind = icon_kind;
                        logging::info(format!("tray icon: {icon_kind:?}"));
                    }
                    Err(error) => logging::warn(format!("could not update tray icon: {error}")),
                },
                Err(error) => logging::warn(format!("could not load tray icon: {error:#}")),
            }
        }
    }
}

/// Map a menu item id back to a controller command.
pub fn command_for(id: &MenuId) -> Option<Command> {
    let id = id.as_ref();
    match id {
        menu_spec::ids::OPEN_WEB => Some(Command::OpenWeb),
        menu_spec::ids::START => Some(Command::StartDsh),
        menu_spec::ids::STOP => Some(Command::StopDsh),
        menu_spec::ids::RESTART => Some(Command::RestartDsh),
        menu_spec::ids::CHECK_UPDATES => Some(Command::CheckUpdates),
        menu_spec::ids::UPDATE_NOW => Some(Command::UpdateCurrentChannel),
        menu_spec::ids::AUTOSTART => Some(Command::ToggleAutostart),
        menu_spec::ids::START_ON_LAUNCH => Some(Command::ToggleStartOnLaunch),
        menu_spec::ids::OPEN_LOG => Some(Command::OpenLog),
        menu_spec::ids::OPEN_CONFIG => Some(Command::OpenConfigFolder),
        menu_spec::ids::QUIT => Some(Command::Quit),
        _ => id
            .strip_prefix(menu_spec::ids::LANGUAGE_PREFIX)
            .and_then(Language::parse)
            .map(Command::SetLanguage)
            .or_else(|| {
                id.strip_prefix(menu_spec::ids::CHANNEL_PREFIX)
                    .and_then(Channel::parse)
                    .map(Command::SwitchChannel)
            }),
    }
}

/// Tray tooltip for [`snapshot`] (never exposes the auth token).
fn tooltip_text(snapshot: &Snapshot) -> String {
    i18n::tooltip(snapshot.lang, &snapshot.state)
}

/// Decode the embedded 64px asset for the requested state.
fn load_icon(kind: IconKind) -> anyhow::Result<Icon> {
    let bytes: &[u8] = match kind {
        IconKind::Idle => &include_bytes!("../../assets/icons/tray-stopped@2x.png")[..],
        IconKind::Running => &include_bytes!("../../assets/icons/tray-running@2x.png")[..],
        IconKind::Error => &include_bytes!("../../assets/icons/tray-error@2x.png")[..],
    };
    decode_icon(bytes).with_context(|| format!("decode {kind:?} tray icon"))
}

fn decode_icon(bytes: &[u8]) -> anyhow::Result<Icon> {
    let image = image::load_from_memory(bytes).context("decode PNG")?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height)
        .map_err(|error| anyhow!("invalid {width}×{height} RGBA icon: {error}"))
}

/// Append an item to either a root [`Menu`] or a [`Submenu`].
trait ItemSink {
    fn push_item(&self, item: &dyn IsMenuItem) -> anyhow::Result<()>;
}

impl ItemSink for Menu {
    fn push_item(&self, item: &dyn IsMenuItem) -> anyhow::Result<()> {
        self.append(item).map_err(anyhow::Error::from)
    }
}

impl ItemSink for Submenu {
    fn push_item(&self, item: &dyn IsMenuItem) -> anyhow::Result<()> {
        self.append(item).map_err(anyhow::Error::from)
    }
}

fn render_menu(spec: &MenuSpec) -> anyhow::Result<Menu> {
    let menu = Menu::new();
    append_items(&menu, &spec.items)?;
    Ok(menu)
}

fn append_items(sink: &dyn ItemSink, items: &[MenuSpecItem]) -> anyhow::Result<()> {
    for item in items {
        append_item(sink, item)?;
    }
    Ok(())
}

fn append_item(sink: &dyn ItemSink, item: &MenuSpecItem) -> anyhow::Result<()> {
    match item {
        MenuSpecItem::Separator => sink.push_item(&PredefinedMenuItem::separator()),
        // Information lines are disabled items without an id: they can never
        // emit a menu event that maps to a command.
        MenuSpecItem::Info(label) => sink.push_item(&MenuItem::new(label, false, None)),
        MenuSpecItem::Entry(entry) => match &entry.kind {
            ItemKind::Normal => {
                sink.push_item(&MenuItem::with_id(entry.id.as_str(), &entry.label, entry.enabled, None))
            }
            ItemKind::Check { checked } => sink.push_item(&CheckMenuItem::with_id(
                entry.id.as_str(),
                &entry.label,
                entry.enabled,
                *checked,
                None,
            )),
        },
        MenuSpecItem::Submenu { label, items } => {
            let submenu = Submenu::new(label, true);
            append_items(&submenu, items)?;
            sink.push_item(&submenu)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Activity, ServiceState};
    use crate::channel::Channel;
    use crate::i18n::Lang;
    use crate::tray::menu_spec;

    fn snapshot() -> Snapshot {
        Snapshot {
            lang: Lang::En,
            language: Language::Auto,
            app_version: crate::VERSION,
            installed_version: Some("0.2.1".to_string()),
            channel: Channel::Latest,
            state: ServiceState::Stopped,
            activity: Activity::Idle,
            tags: None,
            update_available: None,
            autostart: false,
            start_dsh_on_launch: true,
            notice: None,
            detail: None,
            busy: false,
        }
    }

    fn id(value: &str) -> MenuId {
        MenuId::new(value)
    }

    #[test]
    fn maps_every_static_id_to_a_command() {
        assert_eq!(command_for(&id("open-web")), Some(Command::OpenWeb));
        assert_eq!(command_for(&id("start-dsh")), Some(Command::StartDsh));
        assert_eq!(command_for(&id("stop-dsh")), Some(Command::StopDsh));
        assert_eq!(command_for(&id("restart-dsh")), Some(Command::RestartDsh));
        assert_eq!(command_for(&id("check-updates")), Some(Command::CheckUpdates));
        assert_eq!(command_for(&id("update-now")), Some(Command::UpdateCurrentChannel));
        assert_eq!(command_for(&id("toggle-autostart")), Some(Command::ToggleAutostart));
        assert_eq!(
            command_for(&id("toggle-start-on-launch")),
            Some(Command::ToggleStartOnLaunch)
        );
        assert_eq!(command_for(&id("open-log")), Some(Command::OpenLog));
        assert_eq!(command_for(&id("open-config")), Some(Command::OpenConfigFolder));
        assert_eq!(command_for(&id("quit")), Some(Command::Quit));
    }

    #[test]
    fn maps_channel_ids() {
        for channel in Channel::ALL {
            assert_eq!(
                command_for(&id(&format!("channel:{}", channel.tag()))),
                Some(Command::SwitchChannel(channel))
            );
        }
        // Aliases accepted by Channel::parse still work.
        assert_eq!(
            command_for(&id("channel:stable")),
            Some(Command::SwitchChannel(Channel::Latest))
        );
    }

    #[test]
    fn maps_language_ids() {
        assert_eq!(
            command_for(&id("language:auto")),
            Some(Command::SetLanguage(Language::Auto))
        );
        assert_eq!(
            command_for(&id("language:en")),
            Some(Command::SetLanguage(Language::En))
        );
        assert_eq!(
            command_for(&id("language:zh")),
            Some(Command::SetLanguage(Language::Zh))
        );
        assert_eq!(command_for(&id("language:fr")), None);
    }

    #[test]
    fn unknown_ids_are_ignored() {
        for unknown in ["", "header", "status", "channel:", "channel:nightly", "nope"] {
            assert_eq!(command_for(&id(unknown)), None, "{unknown:?}");
        }
    }

    #[test]
    fn every_rendered_entry_has_a_command() {
        // Cross-check the pure menu model against the id -> command mapping:
        // an entry the command handler does not know would be a dead menu item.
        let states = [
            ServiceState::Stopped,
            ServiceState::Starting,
            ServiceState::Stopping,
            ServiceState::Running {
                pid: 1,
                url: "http://127.0.0.1:3080/".into(),
                managed: true,
            },
            ServiceState::Running {
                pid: 2,
                url: "http://127.0.0.1:3080/".into(),
                managed: false,
            },
        ];
        let mut snapshots = Vec::new();
        for state in states {
            for lang in Lang::ALL {
                let mut snapshot = snapshot();
                snapshot.lang = lang;
                snapshot.state = state.clone();
                snapshots.push(snapshot);
            }
        }

        for snapshot in snapshots {
            let spec = menu_spec::build(&snapshot);
            for entry_id in spec.entry_ids() {
                assert!(
                    command_for(&MenuId::new(entry_id)).is_some(),
                    "menu id {entry_id} has no command"
                );
            }
        }
    }

    #[test]
    fn all_state_icons_decode() {
        for kind in [IconKind::Idle, IconKind::Running, IconKind::Error] {
            let icon = load_icon(kind).unwrap_or_else(|error| panic!("{kind:?}: {error:#}"));
            drop(icon);
        }
    }

    #[test]
    fn state_icons_have_expected_colours() {
        let assets = [
            (
                IconKind::Idle,
                &include_bytes!("../../assets/icons/tray-stopped@2x.png")[..],
                (138u8, 143, 152),
            ),
            (
                IconKind::Running,
                &include_bytes!("../../assets/icons/tray-running@2x.png")[..],
                (34u8, 197, 94),
            ),
            (
                IconKind::Error,
                &include_bytes!("../../assets/icons/tray-error@2x.png")[..],
                (239u8, 68, 68),
            ),
        ];
        for (kind, bytes, expected) in assets {
            let image = image::load_from_memory(bytes)
                .unwrap_or_else(|error| panic!("{kind:?} does not decode: {error}"))
                .into_rgba8();
            let mut counts: std::collections::HashMap<(u8, u8, u8), usize> = std::collections::HashMap::new();
            for pixel in image.pixels() {
                if pixel[3] > 200 {
                    *counts.entry((pixel[0], pixel[1], pixel[2])).or_insert(0) += 1;
                }
            }
            let dominant = counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(colour, _)| colour)
                .expect("opaque pixels");
            assert_eq!(dominant, expected, "{kind:?} has the wrong colour");
        }
    }

    #[test]
    fn icon_kind_tracks_service_state_and_errors() {
        let mut snapshot = snapshot();
        assert_eq!(icon_kind(&snapshot), IconKind::Idle);

        snapshot.state = ServiceState::Starting;
        assert_eq!(icon_kind(&snapshot), IconKind::Idle, "starting is not green yet");

        snapshot.state = ServiceState::Running {
            pid: 1,
            url: "http://127.0.0.1:3080/".into(),
            managed: true,
        };
        assert_eq!(icon_kind(&snapshot), IconKind::Running);

        // A running service stays green; a stale unrelated notice must not
        // repaint it red.
        snapshot.notice = Some(Notice::UpdateFailed);
        assert_eq!(icon_kind(&snapshot), IconKind::Running);

        snapshot.state = ServiceState::Stopped;
        assert_eq!(icon_kind(&snapshot), IconKind::Error, "failed update is abnormal");

        snapshot.notice = Some(Notice::CheckFailed);
        assert_eq!(
            icon_kind(&snapshot),
            IconKind::Idle,
            "a network/update-check failure is not a service failure"
        );

        snapshot.notice = Some(Notice::StartFailed);
        assert_eq!(icon_kind(&snapshot), IconKind::Error, "crashed start is abnormal");

        snapshot.notice = None;
        assert_eq!(icon_kind(&snapshot), IconKind::Idle);
    }
}
