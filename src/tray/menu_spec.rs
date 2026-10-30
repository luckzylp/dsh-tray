//! Pure menu model derived from a [`Snapshot`].
//!
//! Keeping this a plain data structure makes menu behavior unit-testable
//! without initializing GTK/AppIndicator. The tray renderer in
//! [`crate::tray`] turns a [`MenuSpec`] into muda items; nothing in this module
//! touches the platform UI.

use crate::app::{ServiceState, Snapshot};
use crate::channel::{tag_version, Channel};
use crate::i18n::{self, Key};

/// Stable menu item ids shared with [`command_for`](crate::tray::command_for).
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

/// Maximum number of characters of [`Snapshot::detail`] echoed in the status line.
pub const DETAIL_MAX_CHARS: usize = 60;

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
    /// Non-interactive information line (header, status, version rows).
    /// Rendered as a disabled item without an id, so it can never map to a
    /// [`Command`](crate::app::Command).
    Info(String),
    Entry(MenuEntry),
    Submenu {
        label: String,
        items: Vec<MenuSpecItem>,
    },
}

/// The complete tray menu.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MenuSpec {
    pub items: Vec<MenuSpecItem>,
}

impl MenuSpec {
    /// Find an entry by its stable id, searching submenus as well.
    pub fn entry(&self, id: &str) -> Option<&MenuEntry> {
        find_entry(&self.items, id)
    }

    /// Find a submenu by its exact (localized) label.
    pub fn submenu(&self, label: &str) -> Option<&[MenuSpecItem]> {
        find_submenu(&self.items, label)
    }

    /// All clickable entry ids in menu order (depth-first).
    pub fn entry_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();
        collect_ids(&self.items, &mut ids);
        ids
    }

    /// All information-line labels in menu order (depth-first).
    pub fn info_labels(&self) -> Vec<&str> {
        let mut labels = Vec::new();
        collect_info(&self.items, &mut labels);
        labels
    }
}

/// Build the menu for [`snapshot`] (labels localized via [`i18n`]).
pub fn build(snapshot: &Snapshot) -> MenuSpec {
    let lang = snapshot.lang;
    let busy = snapshot.busy;
    let mut items = Vec::with_capacity(16);

    items.push(MenuSpecItem::Info(header_label(snapshot)));
    items.push(MenuSpecItem::Info(status_label(snapshot)));
    items.push(MenuSpecItem::Separator);

    // Always usable: the controller starts dsh when needed before opening.
    items.push(entry(ids::OPEN_WEB, i18n::t(lang, Key::MenuOpenWeb), true));

    match &snapshot.state {
        ServiceState::Running { managed: true, .. } => {
            items.push(entry(ids::STOP, i18n::t(lang, Key::MenuStop), !busy));
            items.push(entry(ids::RESTART, i18n::t(lang, Key::MenuRestart), !busy));
        }
        // External instances are not ours to stop or restart.
        ServiceState::Running { managed: false, .. } => {}
        // A start that hangs before printing its URL must be cancellable.
        ServiceState::Starting => {
            items.push(entry(ids::STOP, i18n::t(lang, Key::MenuStop), true));
        }
        // Not running: offer a start, even while busy, so the menu stays stable.
        _ => items.push(entry(ids::START, i18n::t(lang, Key::MenuStart), !busy)),
    }

    items.push(MenuSpecItem::Separator);
    items.push(version_submenu(snapshot));
    items.push(channel_submenu(snapshot));
    items.push(MenuSpecItem::Separator);

    items.push(entry_check(
        ids::AUTOSTART,
        i18n::t(lang, Key::MenuAutostart),
        true,
        snapshot.autostart,
    ));
    items.push(entry_check(
        ids::START_ON_LAUNCH,
        i18n::t(lang, Key::MenuStartOnLaunch),
        true,
        snapshot.start_dsh_on_launch,
    ));

    items.push(MenuSpecItem::Separator);
    items.push(entry(ids::OPEN_LOG, i18n::t(lang, Key::MenuOpenLog), true));
    items.push(entry(ids::OPEN_CONFIG, i18n::t(lang, Key::MenuOpenConfig), true));

    items.push(MenuSpecItem::Separator);
    items.push(entry(ids::QUIT, i18n::t(lang, Key::MenuQuit), true));

    MenuSpec { items }
}

/// First line: installed version and channel.
fn header_label(snapshot: &Snapshot) -> String {
    let version = snapshot
        .installed_version
        .as_deref()
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or("—");
    format!("DSH {version} · {}", snapshot.channel.tag())
}

/// Second line: what the service is doing right now.
///
/// Busy activity wins (it is the most actionable information), then a
/// transient notice, then the plain lifecycle state.
fn status_label(snapshot: &Snapshot) -> String {
    let lang = snapshot.lang;
    if let Some(activity) = i18n::activity(lang, &snapshot.activity) {
        return activity;
    }
    if let Some(notice) = snapshot.notice {
        let mut label = i18n::notice(lang, notice).to_string();
        if let Some(detail) = detail_text(snapshot.detail.as_deref()) {
            label.push_str(" · ");
            label.push_str(&detail);
        }
        return label;
    }
    i18n::state_label(lang, &snapshot.state)
}

/// Trimmed, length-capped single-line detail text.
fn detail_text(detail: Option<&str>) -> Option<String> {
    let detail = detail?.trim();
    if detail.is_empty() {
        return None;
    }
    Some(truncate_chars(detail, DETAIL_MAX_CHARS))
}

/// Truncate to at most [`max`] characters (not bytes), appending an ellipsis.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// Version submenu: installed version, dist-tag versions, update actions.
fn version_submenu(snapshot: &Snapshot) -> MenuSpecItem {
    let lang = snapshot.lang;
    let mut items = Vec::with_capacity(9);

    items.push(MenuSpecItem::Info(i18n::installed(
        lang,
        snapshot.installed_version.as_deref(),
    )));
    items.push(MenuSpecItem::Separator);

    for channel in Channel::ALL {
        let version = snapshot
            .tags
            .as_ref()
            .and_then(|tags| tag_version(tags, channel))
            .unwrap_or("—");
        items.push(MenuSpecItem::Info(format!("{}  {version}", channel.tag())));
    }

    items.push(MenuSpecItem::Separator);
    items.push(entry(
        ids::CHECK_UPDATES,
        i18n::t(lang, Key::MenuCheckUpdates),
        !snapshot.busy,
    ));
    items.push(entry(
        ids::UPDATE_NOW,
        i18n::update_label(lang, snapshot.channel, snapshot.update_available.as_deref()),
        snapshot.update_available.is_some() && !snapshot.busy,
    ));

    MenuSpecItem::Submenu {
        label: i18n::t(lang, Key::MenuVersion).to_string(),
        items,
    }
}

/// Channel submenu: one checked entry per npm dist-tag.
fn channel_submenu(snapshot: &Snapshot) -> MenuSpecItem {
    let lang = snapshot.lang;
    let items = Channel::ALL
        .iter()
        .map(|channel| {
            MenuSpecItem::Entry(MenuEntry {
                id: format!("{}{}", ids::CHANNEL_PREFIX, channel.tag()),
                label: channel.tag().to_string(),
                enabled: !snapshot.busy,
                kind: ItemKind::Check {
                    checked: snapshot.channel == *channel,
                },
            })
        })
        .collect();

    MenuSpecItem::Submenu {
        label: i18n::t(lang, Key::MenuChannel).to_string(),
        items,
    }
}

fn entry(id: &str, label: impl Into<String>, enabled: bool) -> MenuSpecItem {
    MenuSpecItem::Entry(MenuEntry {
        id: id.to_string(),
        label: label.into(),
        enabled,
        kind: ItemKind::Normal,
    })
}

fn entry_check(id: &str, label: impl Into<String>, enabled: bool, checked: bool) -> MenuSpecItem {
    MenuSpecItem::Entry(MenuEntry {
        id: id.to_string(),
        label: label.into(),
        enabled,
        kind: ItemKind::Check { checked },
    })
}

fn find_entry<'a>(items: &'a [MenuSpecItem], id: &str) -> Option<&'a MenuEntry> {
    items.iter().find_map(|item| match item {
        MenuSpecItem::Entry(entry) if entry.id == id => Some(entry),
        MenuSpecItem::Submenu { items, .. } => find_entry(items, id),
        _ => None,
    })
}

fn find_submenu<'a>(items: &'a [MenuSpecItem], label: &str) -> Option<&'a [MenuSpecItem]> {
    items.iter().find_map(|item| match item {
        MenuSpecItem::Submenu {
            label: candidate,
            items,
        } if candidate == label => Some(items.as_slice()),
        MenuSpecItem::Submenu { items, .. } => find_submenu(items, label),
        _ => None,
    })
}

fn collect_ids<'a>(items: &'a [MenuSpecItem], ids: &mut Vec<&'a str>) {
    for item in items {
        match item {
            MenuSpecItem::Entry(entry) => ids.push(entry.id.as_str()),
            MenuSpecItem::Submenu { items, .. } => collect_ids(items, ids),
            _ => {}
        }
    }
}

fn collect_info<'a>(items: &'a [MenuSpecItem], labels: &mut Vec<&'a str>) {
    for item in items {
        match item {
            MenuSpecItem::Info(label) => labels.push(label.as_str()),
            MenuSpecItem::Submenu { items, .. } => collect_info(items, labels),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Activity, Notice};
    use crate::channel::DistTags;
    use crate::i18n::Lang;

    fn sample() -> Snapshot {
        Snapshot {
            lang: Lang::En,
            app_version: crate::VERSION,
            installed_version: Some("0.2.1-rc.5".to_string()),
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

    fn tags() -> DistTags {
        let mut tags = DistTags::new();
        tags.insert("latest".into(), "0.2.1-rc.5".into());
        tags.insert("next".into(), "0.2.1-rc.5".into());
        tags.insert("alpha".into(), "0.2.2-alpha.2".into());
        tags
    }

    fn version_items(spec: &MenuSpec) -> &[MenuSpecItem] {
        spec.submenu(i18n::t(Lang::En, Key::MenuVersion))
            .expect("version submenu")
    }

    fn channel_items(spec: &MenuSpec) -> &[MenuSpecItem] {
        spec.submenu(i18n::t(Lang::En, Key::MenuChannel))
            .expect("channel submenu")
    }

    fn label_of<'a>(spec: &'a MenuSpec, id: &str) -> &'a str {
        &spec.entry(id).expect("entry").label
    }

    #[test]
    fn stopped_menu_has_expected_entries_in_order() {
        let spec = build(&sample());
        assert_eq!(
            spec.entry_ids(),
            vec![
                "open-web",
                "start-dsh",
                "check-updates",
                "update-now",
                "channel:latest",
                "channel:next",
                "channel:alpha",
                "toggle-autostart",
                "toggle-start-on-launch",
                "open-log",
                "open-config",
                "quit",
            ]
        );

        // Header + status, then a separator.
        assert!(matches!(&spec.items[0], MenuSpecItem::Info(label) if label == "DSH 0.2.1-rc.5 · latest"));
        assert!(matches!(&spec.items[1], MenuSpecItem::Info(label) if label == "○ Stopped"));
        assert!(matches!(&spec.items[2], MenuSpecItem::Separator));

        // Last item is Quit.
        assert!(matches!(
            spec.items.last(),
            Some(MenuSpecItem::Entry(entry)) if entry.id == "quit"
        ));

        assert!(spec.entry("open-web").expect("open web").enabled);
        assert!(spec.entry("start-dsh").expect("start").enabled);
        assert!(spec.entry("stop-dsh").is_none());
        assert!(spec.entry("restart-dsh").is_none());
        assert!(!spec.entry("update-now").expect("update now").enabled);
        assert_eq!(label_of(&spec, "update-now"), i18n::t(Lang::En, Key::MenuUpToDate));
        assert_eq!(
            spec.entry("toggle-start-on-launch").expect("toggle").kind,
            ItemKind::Check { checked: true }
        );
        assert_eq!(
            spec.entry("toggle-autostart").expect("autostart").kind,
            ItemKind::Check { checked: false }
        );
    }

    #[test]
    fn running_managed_menu_offers_stop_and_restart() {
        let mut snapshot = sample();
        snapshot.state = ServiceState::Running {
            pid: 42,
            url: "http://127.0.0.1:41029/?token=secret".into(),
            managed: true,
        };
        let spec = build(&snapshot);
        assert_eq!(
            spec.entry_ids(),
            vec![
                "open-web",
                "stop-dsh",
                "restart-dsh",
                "check-updates",
                "update-now",
                "channel:latest",
                "channel:next",
                "channel:alpha",
                "toggle-autostart",
                "toggle-start-on-launch",
                "open-log",
                "open-config",
                "quit",
            ]
        );
        assert!(spec.entry("stop-dsh").expect("stop").enabled);
        assert!(spec.entry("restart-dsh").expect("restart").enabled);
        assert!(spec.entry("start-dsh").is_none());
        let status = spec.info_labels();
        assert!(status[1].contains("Running"));
        assert!(status[1].contains("127.0.0.1:41029"));
        assert!(!status[1].contains("secret"));
    }

    #[test]
    fn running_external_instance_has_no_lifecycle_actions() {
        let mut snapshot = sample();
        snapshot.state = ServiceState::Running {
            pid: 7,
            url: "http://127.0.0.1:3080/".into(),
            managed: false,
        };
        snapshot.notice = Some(Notice::ExternalInstance);
        let spec = build(&snapshot);
        assert_eq!(
            spec.entry_ids(),
            vec![
                "open-web",
                "check-updates",
                "update-now",
                "channel:latest",
                "channel:next",
                "channel:alpha",
                "toggle-autostart",
                "toggle-start-on-launch",
                "open-log",
                "open-config",
                "quit",
            ]
        );
        assert!(spec.entry("stop-dsh").is_none());
        assert!(spec.entry("restart-dsh").is_none());
        assert!(spec.entry("start-dsh").is_none());
        assert!(spec.info_labels()[1].contains("external"));
    }

    #[test]
    fn starting_and_stopping_disable_mutations() {
        for state in [ServiceState::Starting, ServiceState::Stopping] {
            let mut snapshot = sample();
            snapshot.state = state.clone();
            snapshot.busy = true;
            let spec = build(&snapshot);
            if matches!(snapshot.state, ServiceState::Starting) {
                // A hung start must be cancellable from the menu.
                assert!(spec.entry("stop-dsh").expect("stop while starting").enabled);
                assert!(spec.entry("start-dsh").is_none());
            } else {
                assert!(!spec.entry("start-dsh").expect("start").enabled);
                assert!(spec.entry("stop-dsh").is_none());
            }
            assert!(!spec.entry("check-updates").expect("check").enabled);
            assert!(!spec.entry("update-now").expect("update").enabled);
            for channel in Channel::ALL {
                let id = format!("channel:{}", channel.tag());
                assert!(!spec.entry(&id).expect("channel").enabled);
            }
            let status = spec.info_labels()[1].to_string();
            assert!(status.starts_with("Starting") || status.starts_with("Stopping"));
        }
    }

    #[test]
    fn busy_update_shows_activity_and_disables_actions() {
        let mut snapshot = sample();
        snapshot.activity = Activity::Updating {
            channel: Channel::Next,
            target: "0.2.2".into(),
        };
        snapshot.busy = true;
        let spec = build(&snapshot);
        let status = spec.info_labels()[1].to_string();
        assert!(status.contains("next"), "{status}");
        assert!(status.contains("0.2.2"), "{status}");
        assert!(!spec.entry("check-updates").expect("check").enabled);
        assert_eq!(
            spec.entry("channel:next").expect("channel").kind,
            ItemKind::Check { checked: false }
        );
    }

    #[test]
    fn update_available_enables_update_entry() {
        let mut snapshot = sample();
        snapshot.tags = Some(tags());
        snapshot.update_available = Some("0.2.2-alpha.2".into());
        let spec = build(&snapshot);
        let entry = spec.entry("update-now").expect("update now");
        assert!(entry.enabled);
        assert_eq!(entry.label, "Update latest to 0.2.2-alpha.2");
    }

    #[test]
    fn version_submenu_lists_installed_and_tags() {
        let mut snapshot = sample();
        snapshot.tags = Some(tags());
        let spec = build(&snapshot);

        let infos: Vec<&str> = version_items(&spec)
            .iter()
            .filter_map(|item| match item {
                MenuSpecItem::Info(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            infos,
            vec![
                "Installed: 0.2.1-rc.5",
                "latest  0.2.1-rc.5",
                "next  0.2.1-rc.5",
                "alpha  0.2.2-alpha.2",
            ]
        );

        // Unknown tags render as placeholders instead of leaking "None".
        let mut without_tags = sample();
        without_tags.installed_version = None;
        let spec = build(&without_tags);
        let infos: Vec<String> = version_items(&spec)
            .iter()
            .filter_map(|item| match item {
                MenuSpecItem::Info(label) => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(infos[0], "Not installed");
        assert!(infos[1].ends_with("—"), "{:?}", infos[1]);
        assert_eq!(spec.items[0], MenuSpecItem::Info("DSH — · latest".to_string()));
    }

    #[test]
    fn channel_submenu_checks_the_current_channel() {
        let mut snapshot = sample();
        snapshot.channel = Channel::Alpha;
        snapshot.tags = Some(tags());
        let spec = build(&snapshot);
        let checked: Vec<String> = channel_items(&spec)
            .iter()
            .filter_map(|item| match item {
                MenuSpecItem::Entry(entry) if entry.kind == ItemKind::Check { checked: true } => Some(entry.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, vec!["channel:alpha".to_string()]);
        assert_eq!(
            version_items(&spec)
                .iter()
                .find(|item| matches!(item, MenuSpecItem::Entry(entry) if entry.id == "update-now"))
                .map(|item| match item {
                    MenuSpecItem::Entry(entry) => entry.label.as_str(),
                    _ => unreachable!(),
                }),
            Some("No update available")
        );
    }

    #[test]
    fn notice_and_detail_are_shown_and_capped() {
        let mut snapshot = sample();
        snapshot.notice = Some(Notice::UpdateFailed);
        snapshot.detail = Some("npm ERR! permission denied while writing /usr/local/lib/node_modules".into());
        let spec = build(&snapshot);
        let status = spec.info_labels()[1].to_string();
        assert!(status.starts_with("Update failed"), "{status}");
        assert!(status.contains("npm ERR!"), "{status}");
        // Notice + separator + capped detail.
        let detail = status.split(" · ").nth(1).expect("detail");
        assert!(detail.chars().count() <= DETAIL_MAX_CHARS);
        assert!(detail.ends_with('…'));

        // Short details are not truncated.
        snapshot.detail = Some("EACCES".into());
        let spec = build(&snapshot);
        assert_eq!(spec.info_labels()[1], "Update failed · EACCES");
    }

    #[test]
    fn chinese_menu_is_localized() {
        let mut snapshot = sample();
        snapshot.lang = Lang::Zh;
        let spec = build(&snapshot);
        assert_eq!(label_of(&spec, "quit"), i18n::t(Lang::Zh, Key::MenuQuit));
        assert_eq!(label_of(&spec, "open-web"), i18n::t(Lang::Zh, Key::MenuOpenWeb));
        assert_eq!(
            label_of(&spec, "toggle-autostart"),
            i18n::t(Lang::Zh, Key::MenuAutostart)
        );
        assert_eq!(spec.info_labels()[1], "○ 已停止");
    }

    #[test]
    fn every_entry_id_is_unique_and_known() {
        // Guards against two entries sharing an id (command_for would be
        // ambiguous) and against ids that no command handles.
        let mut snapshot = sample();
        snapshot.tags = Some(tags());
        let spec = build(&snapshot);
        let ids = spec.entry_ids();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "duplicate ids in {ids:?}");

        for id in ids {
            assert!(
                id.starts_with("channel:")
                    || [
                        "open-web",
                        "start-dsh",
                        "stop-dsh",
                        "restart-dsh",
                        "check-updates",
                        "update-now",
                        "toggle-autostart",
                        "toggle-start-on-launch",
                        "open-log",
                        "open-config",
                        "quit",
                    ]
                    .contains(&id),
                "unexpected id {id}"
            );
        }
    }

    #[test]
    fn truncate_chars_is_utf8_safe() {
        assert_eq!(truncate_chars("héllo…", 10), "héllo…");
        let long = "启动失败的错误信息".repeat(20);
        let truncated = truncate_chars(&long, DETAIL_MAX_CHARS);
        assert_eq!(truncated.chars().count(), DETAIL_MAX_CHARS);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn state_running_uses_display_url() {
        let mut snapshot = sample();
        snapshot.state = ServiceState::Running {
            pid: 1,
            url: "http://127.0.0.1:1234/path?token=abc".into(),
            managed: true,
        };
        let spec = build(&snapshot);
        assert_eq!(
            spec.info_labels()[1],
            format!(
                "● Running · {}",
                crate::process::display_url("http://127.0.0.1:1234/path?token=abc")
            )
        );
    }
}
