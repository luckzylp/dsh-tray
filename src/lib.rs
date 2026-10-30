#![doc = include_str!("../docs/ARCHITECTURE.md")]
#![allow(dead_code)] // scaffold: removed once every module is implemented

/// Human-facing application name (binary, tray tooltip, config dir).
pub const APP_NAME: &str = "dsh-tray";
/// Reverse-DNS id used by autostart entries and notifications.
pub const APP_ID: &str = "ai.deepseek.dsh-tray";
/// npm package that ships the dsh CLI.
pub const DSH_PACKAGE: &str = "@deepseek-ai/dsh";
/// Version of the tray binary itself.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod app;
pub mod autostart;
pub mod channel;
pub mod cli;
pub mod config;
pub mod dsh;
pub mod i18n;
pub mod logging;
pub mod npm;
pub mod process;
pub mod single_instance;
pub mod tray;
