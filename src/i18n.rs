//! UI language resolution and localized strings.
//!
//! The tray menu is the only UI, so localization is a small static table.
//! [`Lang`] is resolved once at startup from the config preference or the
//! process locale.
//!
//! Everything user visible lives in [`Key`]: a typed enum with one exhaustive
//! translation per language. Parameterized labels are rendered through
//! [`fmt`] (or the small convenience helpers below), which substitutes
//! `{name}` placeholders.

use crate::app::{Activity, Notice, ServiceState};
use crate::channel::Channel;
use crate::config::Language;
use crate::process;

/// Concrete UI language after resolving [`Language::Auto`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    /// All concrete languages, in table order.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Zh];

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
        detect_from(|variable| std::env::var(variable).ok())
    }

    /// BCP-47-ish code for logs and diagnostics.
    pub const fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }
}

/// Look up locale variables with an injectable getter so tests do not have to
/// mutate the process environment.
fn detect_from(mut lookup: impl FnMut(&str) -> Option<String>) -> Lang {
    for variable in ["DSH_TRAY_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(raw) = lookup(variable) else { continue };
        let value = raw.trim().to_ascii_lowercase();
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

/// Every localized string the tray can render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    // Branding.
    AppTitle,
    // Service state (shown in the tray header).
    StateRunning,
    StateStopped,
    StateStarting,
    StateStopping,
    // In-flight activity.
    ActivityChecking,
    ActivityUpdating,
    ActivityUpdatingSimple,
    ActivityRestarting,
    // Transient notices / diagnostics.
    NoticeUpdateSucceeded,
    NoticeUpdateFailed,
    NoticeCheckFailed,
    NoticeStartFailed,
    NoticeStopFailed,
    NoticeStartupTimedOut,
    NoticeExternalInstance,
    NoticeAlreadyRunning,
    NoticeAutostartFailed,
    NoticeConfigFailed,
    NoticeOpenFailed,
    // Menu entries.
    MenuOpenWeb,
    MenuStart,
    MenuStop,
    MenuRestart,
    MenuVersion,
    MenuChannel,
    MenuLanguage,
    MenuLanguageAuto,
    MenuLanguageEn,
    MenuLanguageZh,
    MenuCheckUpdates,
    MenuUpdateTo,
    MenuUpToDate,
    MenuInstalled,
    MenuNotInstalled,
    MenuAutostart,
    MenuStartOnLaunch,
    MenuOpenLog,
    MenuOpenConfig,
    MenuQuit,
    // Tray tooltip.
    TooltipRunning,
    TooltipStopped,
}

impl Key {
    /// Every key, for exhaustiveness tests and diagnostics.
    pub const ALL: [Key; 42] = [
        Key::AppTitle,
        Key::StateRunning,
        Key::StateStopped,
        Key::StateStarting,
        Key::StateStopping,
        Key::ActivityChecking,
        Key::ActivityUpdating,
        Key::ActivityUpdatingSimple,
        Key::ActivityRestarting,
        Key::NoticeUpdateSucceeded,
        Key::NoticeUpdateFailed,
        Key::NoticeCheckFailed,
        Key::NoticeStartFailed,
        Key::NoticeStopFailed,
        Key::NoticeStartupTimedOut,
        Key::NoticeExternalInstance,
        Key::NoticeAlreadyRunning,
        Key::NoticeAutostartFailed,
        Key::NoticeConfigFailed,
        Key::NoticeOpenFailed,
        Key::MenuOpenWeb,
        Key::MenuStart,
        Key::MenuStop,
        Key::MenuRestart,
        Key::MenuVersion,
        Key::MenuChannel,
        Key::MenuLanguage,
        Key::MenuLanguageAuto,
        Key::MenuLanguageEn,
        Key::MenuLanguageZh,
        Key::MenuCheckUpdates,
        Key::MenuUpdateTo,
        Key::MenuUpToDate,
        Key::MenuInstalled,
        Key::MenuNotInstalled,
        Key::MenuAutostart,
        Key::MenuStartOnLaunch,
        Key::MenuOpenLog,
        Key::MenuOpenConfig,
        Key::MenuQuit,
        Key::TooltipRunning,
        Key::TooltipStopped,
    ];
}

/// Localized text for `key`; exhaustive for every language.
pub fn t(lang: Lang, key: Key) -> &'static str {
    match key {
        Key::AppTitle => match lang {
            Lang::En => "DSH Tray",
            Lang::Zh => "DSH 托盘",
        },
        Key::StateRunning => match lang {
            Lang::En => "● Running",
            Lang::Zh => "● 运行中",
        },
        Key::StateStopped => match lang {
            Lang::En => "○ Stopped",
            Lang::Zh => "○ 已停止",
        },
        Key::StateStarting => match lang {
            Lang::En => "Starting…",
            Lang::Zh => "正在启动…",
        },
        Key::StateStopping => match lang {
            Lang::En => "Stopping…",
            Lang::Zh => "正在停止…",
        },
        Key::ActivityChecking => match lang {
            Lang::En => "Checking for updates…",
            Lang::Zh => "正在检查更新…",
        },
        Key::ActivityUpdating => match lang {
            Lang::En => "Updating {channel} to {target}…",
            Lang::Zh => "正在将 {channel} 更新到 {target}…",
        },
        Key::ActivityUpdatingSimple => match lang {
            Lang::En => "Updating…",
            Lang::Zh => "正在更新…",
        },
        Key::ActivityRestarting => match lang {
            Lang::En => "Restarting…",
            Lang::Zh => "正在重启…",
        },
        Key::NoticeUpdateSucceeded => match lang {
            Lang::En => "Updated successfully",
            Lang::Zh => "更新成功",
        },
        Key::NoticeUpdateFailed => match lang {
            Lang::En => "Update failed",
            Lang::Zh => "更新失败",
        },
        Key::NoticeCheckFailed => match lang {
            Lang::En => "Could not check for updates",
            Lang::Zh => "检查更新失败",
        },
        Key::NoticeStartFailed => match lang {
            Lang::En => "Could not start DSH",
            Lang::Zh => "启动 DSH 失败",
        },
        Key::NoticeStopFailed => match lang {
            Lang::En => "Could not stop DSH",
            Lang::Zh => "停止 DSH 失败",
        },
        Key::NoticeStartupTimedOut => match lang {
            Lang::En => "DSH did not become ready in time",
            Lang::Zh => "DSH 启动超时",
        },
        Key::NoticeExternalInstance => match lang {
            Lang::En => "Using an external DSH instance",
            Lang::Zh => "正在使用外部 DSH 实例",
        },
        Key::NoticeAlreadyRunning => match lang {
            Lang::En => "DSH is already running",
            Lang::Zh => "DSH 已在运行",
        },
        Key::NoticeAutostartFailed => match lang {
            Lang::En => "Could not update the login item",
            Lang::Zh => "无法更新登录项",
        },
        Key::NoticeConfigFailed => match lang {
            Lang::En => "Could not save settings",
            Lang::Zh => "无法保存设置",
        },
        Key::NoticeOpenFailed => match lang {
            Lang::En => "Could not open the requested item",
            Lang::Zh => "无法打开目标",
        },
        Key::MenuOpenWeb => match lang {
            Lang::En => "Open Web UI",
            Lang::Zh => "打开 Web 界面",
        },
        Key::MenuStart => match lang {
            Lang::En => "Start DSH",
            Lang::Zh => "启动 DSH",
        },
        Key::MenuStop => match lang {
            Lang::En => "Stop DSH",
            Lang::Zh => "停止 DSH",
        },
        Key::MenuRestart => match lang {
            Lang::En => "Restart DSH",
            Lang::Zh => "重启 DSH",
        },
        Key::MenuVersion => match lang {
            Lang::En => "Version",
            Lang::Zh => "版本",
        },
        Key::MenuChannel => match lang {
            Lang::En => "Channel",
            Lang::Zh => "发布通道",
        },
        Key::MenuLanguage => match lang {
            Lang::En => "Language",
            Lang::Zh => "语言",
        },
        Key::MenuLanguageAuto => match lang {
            Lang::En => "Follow system",
            Lang::Zh => "跟随系统",
        },
        Key::MenuLanguageEn => "English",
        Key::MenuLanguageZh => "简体中文",
        Key::MenuCheckUpdates => match lang {
            Lang::En => "Check for updates",
            Lang::Zh => "检查更新",
        },
        Key::MenuUpdateTo => match lang {
            Lang::En => "Update {channel} to {version}",
            Lang::Zh => "更新 {channel} 到 {version}",
        },
        Key::MenuUpToDate => match lang {
            Lang::En => "No update available",
            Lang::Zh => "暂无可用更新",
        },
        Key::MenuInstalled => match lang {
            Lang::En => "Installed: {version}",
            Lang::Zh => "已安装：{version}",
        },
        Key::MenuNotInstalled => match lang {
            Lang::En => "Not installed",
            Lang::Zh => "未安装",
        },
        Key::MenuAutostart => match lang {
            Lang::En => "Launch tray at login",
            Lang::Zh => "登录时启动托盘",
        },
        Key::MenuStartOnLaunch => match lang {
            Lang::En => "Start DSH when tray starts",
            Lang::Zh => "托盘启动时启动 DSH",
        },
        Key::MenuOpenLog => match lang {
            Lang::En => "Open log",
            Lang::Zh => "打开日志",
        },
        Key::MenuOpenConfig => match lang {
            Lang::En => "Open config folder",
            Lang::Zh => "打开配置文件夹",
        },
        Key::MenuQuit => match lang {
            Lang::En => "Quit",
            Lang::Zh => "退出",
        },
        Key::TooltipRunning => match lang {
            Lang::En => "Running on {url}",
            Lang::Zh => "正在运行：{url}",
        },
        Key::TooltipStopped => match lang {
            Lang::En => "DSH stopped",
            Lang::Zh => "DSH 已停止",
        },
    }
}

/// Substitute `{name}` placeholders in a localized template.
///
/// Unknown placeholders are left untouched; missing arguments never panic.
pub fn fmt(lang: Lang, key: Key, args: &[(&str, &str)]) -> String {
    let mut rendered = t(lang, key).to_string();
    for (name, value) in args {
        rendered = rendered.replace(&format!("{{{name}}}"), value);
    }
    rendered
}

/// Header label for a lifecycle state ([`ServiceState::Running`] includes the
/// display URL without its auth token).
pub fn state_label(lang: Lang, state: &ServiceState) -> String {
    match state {
        ServiceState::Stopped => t(lang, Key::StateStopped).to_string(),
        ServiceState::Starting => t(lang, Key::StateStarting).to_string(),
        ServiceState::Stopping => t(lang, Key::StateStopping).to_string(),
        ServiceState::Running { url, .. } => {
            let display = process::display_url(url);
            format!("{} · {display}", t(lang, Key::StateRunning))
        }
    }
}

/// Status text for a transient [`Notice`].
pub fn notice(lang: Lang, notice: Notice) -> &'static str {
    t(
        lang,
        match notice {
            Notice::Starting => Key::StateStarting,
            Notice::Stopping => Key::StateStopping,
            Notice::Checking => Key::ActivityChecking,
            Notice::Updating => Key::ActivityUpdatingSimple,
            Notice::Restarting => Key::ActivityRestarting,
            Notice::UpdateSucceeded => Key::NoticeUpdateSucceeded,
            Notice::UpdateFailed => Key::NoticeUpdateFailed,
            Notice::CheckFailed => Key::NoticeCheckFailed,
            Notice::StartFailed => Key::NoticeStartFailed,
            Notice::StopFailed => Key::NoticeStopFailed,
            Notice::StartupTimedOut => Key::NoticeStartupTimedOut,
            Notice::ExternalInstance => Key::NoticeExternalInstance,
            Notice::AlreadyRunning => Key::NoticeAlreadyRunning,
            Notice::AutostartFailed => Key::NoticeAutostartFailed,
            Notice::ConfigFailed => Key::NoticeConfigFailed,
            Notice::OpenFailed => Key::NoticeOpenFailed,
        },
    )
}

/// Progress text for a busy [`Activity`], if there is one to show.
pub fn activity(lang: Lang, activity: &Activity) -> Option<String> {
    match activity {
        Activity::Idle => None,
        Activity::Checking => Some(t(lang, Key::ActivityChecking).to_string()),
        Activity::Updating { channel, target } => Some(fmt(
            lang,
            Key::ActivityUpdating,
            &[("channel", channel.tag()), ("target", target)],
        )),
        Activity::Restarting => Some(t(lang, Key::ActivityRestarting).to_string()),
    }
}

/// `Installed: <version>` or `Not installed`.
pub fn installed(lang: Lang, version: Option<&str>) -> String {
    match version.map(str::trim).filter(|version| !version.is_empty()) {
        Some(version) => fmt(lang, Key::MenuInstalled, &[("version", version)]),
        None => t(lang, Key::MenuNotInstalled).to_string(),
    }
}

/// `Update <channel> to <version>` or `No update available`.
pub fn update_label(lang: Lang, channel: Channel, version: Option<&str>) -> String {
    match version.map(str::trim).filter(|version| !version.is_empty()) {
        Some(version) => fmt(
            lang,
            Key::MenuUpdateTo,
            &[("channel", channel.tag()), ("version", version)],
        ),
        None => t(lang, Key::MenuUpToDate).to_string(),
    }
}

/// Menu label for a language preference.
///
/// Concrete languages use their native names; "follow system" is localized and
/// carries the language detection currently resolves to, so the effect of
/// switching back to automatic mode is visible before clicking it.
pub fn language_label(lang: Lang, preference: Language) -> String {
    match preference {
        Language::Auto => {
            let detected = Lang::detect();
            let name = match detected {
                Lang::En => t(lang, Key::MenuLanguageEn),
                Lang::Zh => t(lang, Key::MenuLanguageZh),
            };
            format!("{} ({name})", t(lang, Key::MenuLanguageAuto))
        }
        Language::En => t(lang, Key::MenuLanguageEn).to_string(),
        Language::Zh => t(lang, Key::MenuLanguageZh).to_string(),
    }
}

/// Tray tooltip for the current state; URLs are shown without their query
/// string so an auth token is never exposed.
pub fn tooltip(lang: Lang, state: &ServiceState) -> String {
    match state.url() {
        Some(url) => {
            let display = process::display_url(url);
            fmt(lang, Key::TooltipRunning, &[("url", &display)])
        }
        None => t(lang, Key::TooltipStopped).to_string(),
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

    #[test]
    fn every_key_has_text_in_both_languages() {
        for lang in Lang::ALL {
            for key in Key::ALL {
                let text = t(lang, key);
                assert!(!text.trim().is_empty(), "empty {key:?} for {lang:?}");
            }
        }
    }

    #[test]
    fn translations_actually_differ() {
        // A key that was copied but not translated is a bug; the most obvious
        // offenders are checked here so a forgotten translation cannot slip by.
        assert_ne!(t(Lang::En, Key::MenuQuit), t(Lang::Zh, Key::MenuQuit));
        assert_ne!(t(Lang::En, Key::MenuStartOnLaunch), t(Lang::Zh, Key::MenuStartOnLaunch));
        assert_ne!(
            t(Lang::En, Key::NoticeExternalInstance),
            t(Lang::Zh, Key::NoticeExternalInstance)
        );
        assert_ne!(t(Lang::En, Key::NoticeOpenFailed), t(Lang::Zh, Key::NoticeOpenFailed));
    }

    #[test]
    fn language_labels_use_native_names_and_show_the_detected_locale() {
        assert_eq!(language_label(Lang::En, Language::En), "English");
        assert_eq!(language_label(Lang::En, Language::Zh), "简体中文");
        assert_eq!(language_label(Lang::Zh, Language::Zh), "简体中文");
        let auto = language_label(Lang::En, Language::Auto);
        assert!(auto.starts_with("Follow system ("), "{auto}");
        assert!(auto.contains("English") || auto.contains("简体中文"), "{auto}");
        let auto_zh = language_label(Lang::Zh, Language::Auto);
        assert!(auto_zh.starts_with("跟随系统 ("), "{auto_zh}");
    }

    #[test]
    fn templates_are_rendered() {
        let en = fmt(
            Lang::En,
            Key::MenuUpdateTo,
            &[("channel", "next"), ("version", "0.2.2")],
        );
        assert_eq!(en, "Update next to 0.2.2");
        let zh = fmt(
            Lang::Zh,
            Key::MenuUpdateTo,
            &[("channel", "next"), ("version", "0.2.2")],
        );
        assert!(zh.contains("next") && zh.contains("0.2.2"));
        assert!(!zh.contains('{'), "placeholder left in {zh}");
    }

    #[test]
    fn unknown_placeholder_is_left_alone() {
        let rendered = fmt(Lang::En, Key::MenuQuit, &[("other", "x")]);
        assert_eq!(rendered, "Quit");
    }

    #[test]
    fn installed_and_update_helpers() {
        assert_eq!(installed(Lang::En, Some("1.2.3")), "Installed: 1.2.3");
        assert_eq!(installed(Lang::En, None), "Not installed");
        assert_eq!(installed(Lang::En, Some("  ")), "Not installed");
        assert_eq!(
            update_label(Lang::En, Channel::Latest, Some("1.2.3")),
            "Update latest to 1.2.3"
        );
        assert_eq!(update_label(Lang::En, Channel::Latest, None), "No update available");
    }

    #[test]
    fn status_label_hides_the_auth_token() {
        let running = ServiceState::Running {
            pid: 1,
            url: "http://127.0.0.1:41029/?token=secret".into(),
            managed: true,
        };
        let label = state_label(Lang::En, &running);
        assert!(label.contains("127.0.0.1:41029"));
        assert!(!label.contains("secret"));
        assert!(label.contains("Running"));

        let tooltip_text = tooltip(Lang::En, &running);
        assert!(!tooltip_text.contains("secret"));
        assert_eq!(tooltip(Lang::En, &ServiceState::Stopped), "DSH stopped");
    }

    #[test]
    fn activity_texts_use_channel_and_target() {
        assert_eq!(activity(Lang::En, &Activity::Idle), None);
        let text = activity(
            Lang::En,
            &Activity::Updating {
                channel: Channel::Alpha,
                target: "0.3.0-alpha.1".into(),
            },
        )
        .expect("text");
        assert!(text.contains("alpha"));
        assert!(text.contains("0.3.0-alpha.1"));
        assert_eq!(
            activity(Lang::En, &Activity::Checking).as_deref(),
            Some("Checking for updates…")
        );
    }

    #[test]
    fn every_notice_has_text() {
        for lang in Lang::ALL {
            for variant in [
                Notice::Starting,
                Notice::Stopping,
                Notice::Checking,
                Notice::Updating,
                Notice::Restarting,
                Notice::UpdateSucceeded,
                Notice::UpdateFailed,
                Notice::CheckFailed,
                Notice::StartFailed,
                Notice::StopFailed,
                Notice::StartupTimedOut,
                Notice::ExternalInstance,
                Notice::AlreadyRunning,
                Notice::AutostartFailed,
                Notice::ConfigFailed,
                Notice::OpenFailed,
            ] {
                assert!(!notice(lang, variant).trim().is_empty(), "empty {variant:?}");
            }
        }
    }

    #[test]
    fn detect_honours_priority_and_locale_prefixes() {
        let zh = detect_from(|name| (name == "LANG").then(|| "zh_CN.UTF-8".to_string()));
        assert_eq!(zh, Lang::Zh);

        let forced_en = detect_from(|name| match name {
            "DSH_TRAY_LANG" => Some("en".to_string()),
            "LANG" => Some("zh_CN.UTF-8".to_string()),
            _ => None,
        });
        assert_eq!(forced_en, Lang::En);

        let fallback = detect_from(|_| Some("fr_FR.UTF-8".to_string()));
        assert_eq!(fallback, Lang::En);

        let empty = detect_from(|name| (name == "LC_ALL").then(String::new));
        assert_eq!(empty, Lang::En);

        let missing = detect_from(|_| None);
        assert_eq!(missing, Lang::En);
    }
}
