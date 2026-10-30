# DSH Tray

DSH Tray is a native, cross-platform system tray controller for the
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`) web
UI. It starts, supervises, updates and opens a local `dsh web` session, and
keeps all of that one click away in the system tray.

It is a single Rust binary: no Electron, no webview, no background service. The
only UI is the tray menu (tao + tray-icon/muda, GTK/AppIndicator on Linux).

## Contents

- [Features](#features)
- [Architecture at a glance](#architecture-at-a-glance)
- [Requirements](#requirements)
- [Install](#install)
- [Quick start](#quick-start)
- [Using the tray menu](#using-the-tray-menu)
- [Configuration](#configuration)
- [Channels and updates](#channels-and-updates)
- [Launch at login](#launch-at-login)
- [Diagnostics and debugging](#diagnostics-and-debugging)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)
- [Acknowledgements](#acknowledgements)

## Features

- **Start, stop and restart `dsh web`** — the tray supervises the child process
  it started and never blocks the menu while it works.
- **Open the web UI** — the tray parses the authenticated URL printed by
  `dsh web` and opens it in your default browser, token included.
- **State-coloured whale icon** — the tray icon is the DeepSeek whale: gray when
  dsh is stopped, green while the web UI is running, and red after a failed
  start/stop/update or an unexpected exit.
- **Installed version at a glance** — the menu header shows the installed `dsh`
  version and the effective channel.
- **Channel switching** — choose between the `latest`, `next` and `alpha` npm
  dist-tags without leaving the tray.
- **Update checks and one-click updates** — the tray polls npm dist-tags in the
  background (configurable interval), shows when a newer version exists, and can
  update with `npm install -g` for you.
- **Launch at login** — native autostart integration per platform (XDG
  autostart on Linux, LaunchAgent on macOS, `HKCU\...\Run` on Windows).
- **Logs and configuration in reach** — menu items open the log file and the
  config folder.
- **Single instance** — starting the tray twice hands the second launch over to
  the running instance instead of adding a second icon; if dsh is serving, the
  existing web UI is opened.
- **Only tray-started processes** — `Stop`/`Restart` act only on the `dsh`
  process the tray started; it never attaches to, or stops, another server.
- **English and Simplified Chinese UI** — switch from the **Language** submenu
  (follow system, English, 简体中文); the choice is saved to the config.
- **Diagnostics CLI** — `--check`, `--check --json` and `--smoke` for health
  checks and CI smoke tests.

## Architecture at a glance

`dsh-tray` runs the tao event loop on the main thread, a controller thread that
owns configuration and state, and short-lived worker threads for slow work (npm
network calls, installs, waiting for readiness). The tray renders immutable
snapshots and sends commands back; it never touches the child process directly.

```text
main thread                     controller thread                 workers
-----------                     -----------------                 -------
tao event loop                  owns Config + Snapshot            network / npm / wait
  tray icon + menu   <--snapshot--  command loop                    |
  menu events -> Command -------->                                |
  tray click -> OpenWeb
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the module map and the full
process model.

## Requirements

| Component | Requirement |
| --- | --- |
| Rust (build from source) | 1.85 or newer; edition 2021 |
| Node.js and npm | on `PATH`, or set `node_path` / `npm_path` in the config |
| `dsh` CLI | installed globally from npm: `@deepseek-ai/dsh` |
| Linux runtime | GTK3 and a StatusNotifier/AppIndicator host (see below) |
| Linux build deps | `libgtk-3-dev` and `libayatana-appindicator3-dev` |
| macOS / Windows | no extra runtime dependencies; release binaries are unsigned |

Linux currently gets the most hands-on testing. The macOS and Windows code
paths are compiled and tested by CI, but have seen less manual testing; report
issues rather than assuming they are supported perfectly.

## Install

### Prebuilt binaries

When a maintainer pushes a `v*` tag, the release workflow builds and attaches
these artifacts to the GitHub Release for that tag (use the Releases page of the
repository that hosts this checkout; this README does not hard-code a URL):

| Platform | Artifact |
| --- | --- |
| Linux x86_64 | `dsh-tray-linux-x86_64.tar.gz` |
| Linux aarch64 | `dsh-tray-linux-aarch64.tar.gz` |
| Windows x86_64 | `dsh-tray-windows-x86_64.zip` |
| macOS x86_64 | `dsh-tray-macos-x86_64.tar.gz` |
| macOS Apple Silicon | `dsh-tray-macos-aarch64.tar.gz` |
| macOS application bundle | `DSH-Tray-macos-<arch>.app.tar.gz` |
| Checksums | `checksums.txt` (SHA-256, one line per artifact) |

The tarballs and the zip contain the binary, `LICENSE`, `README.md` and
`README.zh-CN.md`.

Linux:

```bash
tar xzf dsh-tray-linux-x86_64.tar.gz
install -Dm755 dsh-tray "$HOME/.local/bin/dsh-tray"
```

macOS:

```bash
tar xzf DSH-Tray-macos-aarch64.app.tar.gz
mv "DSH Tray.app" /Applications/
xattr -dr com.apple.quarantine "/Applications/DSH Tray.app"   # unsigned build
open "/Applications/DSH Tray.app"
```

Windows: extract the zip and put `dsh-tray.exe` in a directory on `PATH`.

### Build from source

Linux build dependencies first:

```bash
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libayatana-appindicator3-dev
```

Then build or install:

```bash
cargo build --release --locked
# binary: target/release/dsh-tray        (target\release\dsh-tray.exe on Windows)

cargo install --path . --locked
# installs into ~/.cargo/bin
```

### Desktop integration on Linux

From a source checkout, `packaging/install.sh` installs the binary, the icon and
a desktop entry for the current user (no `sudo`):

```bash
packaging/install.sh --binary target/release/dsh-tray
```

It installs to `~/.local/bin`, `~/.local/share/icons/hicolor/256x256/apps` and
`~/.local/share/applications`; override those with `BINDIR`, `ICONDIR` and
`APPDIR`. For manual setups, `packaging/dsh-tray.desktop` is a ready-made entry
that expects `dsh-tray` on `PATH`.

## Quick start

1. Install the `dsh` CLI if you have not already:

   ```bash
   npm install -g @deepseek-ai/dsh@latest
   dsh --version
   ```

2. Start the tray:

   ```bash
   dsh-tray
   ```

3. On first start the tray uses the built-in defaults and, because
   `start_dsh_on_launch` defaults to `true`, starts `dsh web`. Use the tray menu
   to open the web UI or to stop/restart the server. `config.json` is written on
   the next settings change (or when the tray quits).

By default the tray starts `dsh web` with `--no-open` and `--port 0`, so the OS
picks a free port and the tray never has to guess it. This also avoids
collisions with an already-running server.

## Using the tray menu

| Menu item | What it does |
| --- | --- |
| Header row | Installed `dsh` version and effective channel |
| Status row | Running/stopped state; running shows the token-free URL |
| **Open Web UI** | Opens the authenticated URL, starting `dsh web` first if needed |
| **Start DSH** | Starts a managed `dsh web` process |
| **Stop DSH** | Stops the managed process (SIGTERM, then SIGKILL after a grace period) |
| **Restart DSH** | Stops and starts the managed process again |
| Version submenu | Installed version, per-channel target versions, **Check for updates**, **Update to …** |
| Channel submenu | Switch between `latest`, `next` and `alpha` |
| Language submenu | Follow the system locale, English, or 简体中文 (persisted) |
| **Launch tray at login** | Toggles the platform autostart entry |
| **Start DSH when tray starts** | Toggles `start_dsh_on_launch` |
| **Open log** / **Open config folder** | Opens `dsh-tray.log` / the config directory |
| **Quit** | Stops the managed child (when `stop_dsh_on_quit`) and exits |

## Configuration

### Where files live

| OS | Config directory | Data directory |
| --- | --- | --- |
| Linux | `$XDG_CONFIG_HOME/dsh-tray` (default `~/.config/dsh-tray`) | `$XDG_DATA_HOME/dsh-tray` (default `~/.local/share/dsh-tray`) |
| macOS | `~/Library/Application Support/ai.deepseek.dsh-tray` | same |
| Windows | `%APPDATA%\deepseek\dsh-tray\config` | `%APPDATA%\deepseek\dsh-tray\data` |

`DSH_TRAY_CONFIG_DIR` and `DSH_TRAY_DATA_DIR` override the two locations. The
tray creates the directories it needs on startup.

| Path | Purpose |
| --- | --- |
| `config.json` | User settings (config directory) |
| `logs/dsh-tray.log` | Tray log; rotated to `dsh-tray.log.1` at 2 MiB |
| `logs/dsh-web.log` | Raw stdout/stderr of the managed `dsh web` process |
| `runtime.json` | Runtime hand-off state used by a second launch |
| `instance.lock` | Single-instance lock |
| `tray-icon/` | Temporary icon file used by Linux AppIndicator |

### `config.json` fields

All fields are optional on disk: missing or unknown keys fall back to the
defaults below. A corrupt file is ignored (with a warning in the log) instead of
preventing startup.

| Field | Default | Meaning |
| --- | --- | --- |
| `schema_version` | `1` | Config schema revision, reserved for future migrations |
| `channel` | `"latest"` | Preferred npm channel: `latest`, `next` or `alpha` |
| `port` | `0` | Port passed to `dsh web`; `0` lets the OS choose a free port (recommended). A non-zero value pins the port and can collide |
| `host` | `"127.0.0.1"` | IP literal passed to `dsh web --host`; wildcard/non-IP values are replaced with `127.0.0.1` |
| `open_browser_when_ready` | `true` | Open the browser when the tray auto-starts dsh at launch (`--hidden` suppresses it; **Open Web UI** always opens it) |
| `start_dsh_on_launch` | `true` | Start `dsh web` when the tray starts |
| `autostart` | `false` | Launch the tray at login (mirrors the platform autostart entry) |
| `language` | `"auto"` | Tray UI language: `auto` (system locale), `en` or `zh`; also switchable from the **Language** submenu |
| `dsh_path` | `null` | Explicit path to the dsh entry (`bin.js`, shim or symlink); bypasses PATH discovery |
| `node_path` | `null` | Explicit `node` executable |
| `npm_path` | `null` | Explicit `npm` executable |
| `registry` | `null` | npm registry base URL; `null` auto-detects it |
| `update_check_hours` | `6` | Background dist-tags poll interval in hours (clamped to 1–720) |
| `stop_dsh_on_quit` | `true` | Stop a tray-managed `dsh` process when the tray quits |

A complete default file:

```json
{
  "schema_version": 1,
  "channel": "latest",
  "port": 0,
  "host": "127.0.0.1",
  "open_browser_when_ready": true,
  "start_dsh_on_launch": true,
  "autostart": false,
  "language": "auto",
  "dsh_path": null,
  "node_path": null,
  "npm_path": null,
  "registry": null,
  "update_check_hours": 6,
  "stop_dsh_on_quit": true
}
```

Invalid values are repaired on load: a non-IP or wildcard host becomes
`127.0.0.1`, `update_check_hours` is clamped, and an invalid registry URL falls
back to auto-detection. Repairs are reported in the log.

### Environment variables

| Variable | Effect |
| --- | --- |
| `DSH_TRAY_CONFIG_DIR` | Override the config directory |
| `DSH_TRAY_DATA_DIR` | Override the data directory (logs and runtime state) |
| `DSH_TRAY_DEBUG` | Any non-empty value also echoes log lines to stderr |
| `DSH_TRAY_LANG` | Override language detection (`en`/`zh`, locale-style values accepted) |

## Channels and updates

The `dsh` CLI is published on npm as `@deepseek-ai/dsh` with three dist-tags:

| Channel | npm dist-tag | Meaning |
| --- | --- | --- |
| `latest` | `latest` | Stable releases |
| `next` | `next` | Preview releases |
| `alpha` | `alpha` | Experimental releases |

The tray never hard-codes versions. It reads the installed version from the
package's `package.json` (falling back to `dsh --version`) and the available
versions from the npm registry's dist-tags.

- **Effective channel** — an exact version match to a dist-tag wins; when several
  tags point at the same version, the saved preference decides; prerelease
  suffixes are only a last-resort hint.
- **Switching channels** — the preference is always saved. npm is invoked only
  when the target tag points at a different version; the managed process is
  restarted afterwards when it was running.
- **Checking for updates** — fetches dist-tags and compares versions with semver.
- **Updating** — stops a managed child first, runs
  `npm install -g @deepseek-ai/dsh@<tag>`, re-reads the installed version, and
  restarts the child if it was running.
- **Failures** — shown in the tray header and written to the log. npm permission
  errors are reported verbatim with a hint about the global prefix.

Update checks need network access to the configured npm registry; everything
else works offline.

## Launch at login

The menu checkbox **Launch tray at login** writes the platform-native entry:

| OS | Mechanism |
| --- | --- |
| Linux | XDG autostart file `$XDG_CONFIG_HOME/autostart/ai.deepseek.dsh-tray.desktop`, running the tray with `--hidden` |
| macOS | LaunchAgent `~/Library/LaunchAgents/ai.deepseek.dsh-tray.plist` (`RunAtLoad`) |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `dsh-tray` |

The `--hidden` flag marks a login-item launch: the tray starts without opening
the browser by itself. Toggling the checkbox updates `autostart` and the OS
entry together; removing the entry through the system tools is also fine.

## Diagnostics and debugging

| Command | Purpose |
| --- | --- |
| `dsh-tray --check` | Print a health/diagnostic report (dsh installation, node/npm, config, process state) |
| `dsh-tray --check --json` | Same report as machine-readable JSON |
| `dsh-tray --smoke` | Start `dsh web`, wait for the URL, verify it over HTTP, stop it, exit |
| `dsh-tray --version` | Print the tray version |
| `dsh-tray --help` | Print usage |
| `dsh-tray --hidden` | Start the tray without opening the browser (login-item mode) |

Useful debugging switches supported by the environment:

```bash
DSH_TRAY_DEBUG=1 dsh-tray                       # echo log lines to stderr
DSH_TRAY_CONFIG_DIR=/tmp/dsh-tray-config \
DSH_TRAY_DATA_DIR=/tmp/dsh-tray-data dsh-tray --check
```

Logs are written to the data directory (`logs/dsh-tray.log` for the tray,
`logs/dsh-web.log` for the child process). With `DSH_TRAY_DEBUG` set, the same
lines also appear on stderr, which is handy when starting the tray from a
terminal or a CI job. `--check` additionally honours `DSH_TRAY_OFFLINE` to skip
npm registry requests when diagnosing an offline machine.

> **Token caution:** the URL printed by `dsh web` contains an authentication
> token. `dsh-web.log` records the raw child output and `runtime.json` stores the
> URL while the server is running, so both contain the token. Both files are
> created with owner-only permissions on Unix; still, do not paste them into
> public issue trackers.

## Troubleshooting

### The tray says dsh is not installed

Install the CLI globally and make sure it is on `PATH`:

```bash
npm install -g @deepseek-ai/dsh@latest
dsh --version
```

If `dsh` lives in a non-standard prefix, set `dsh_path` (and optionally
`node_path` / `npm_path`) in `config.json`.

### npm fails with EACCES / permission errors

Do not run npm with `sudo`. Check the global prefix and either use a user-owned
prefix or a Node version manager:

```bash
npm config get prefix
npm config set prefix "$HOME/.npm-global"   # then add $HOME/.npm-global/bin to PATH
```

The tray relays npm's error text verbatim into the log and tray notice.

### No tray icon on Linux

The tray needs a StatusNotifier/AppIndicator host in your desktop session. On
GNOME, install and enable the **AppIndicator and KStatusNotifierItem Support**
extension (or an equivalent shell extension), then log out and back in. On
Wayland the icon is shown through the compositor's StatusNotifier bridge; X11
sessions usually provide a host through the panel or a systray implementation.
If the session has no host at all, AppIndicator applications cannot display a
tray icon. `libayatana-appindicator3` must also be installed at runtime.

```bash
sudo apt-get install -y libayatana-appindicator3-1
```

### Port already in use

Keep `port` at `0` so the OS picks a free port — the tray reads the URL printed
by `dsh web` and never needs a fixed port. If you pinned a port, change it back
to `0` or free the port.

### The browser opens but authentication fails

The authenticated URL carries a `?token=…` query and must be used verbatim. Use
**Open Web UI** from the tray rather than retyping the address, and avoid
copying a URL from `dsh-web.log` that has been truncated.

### dsh starts but never becomes ready

That produces a startup-timeout notice. Check `logs/dsh-web.log` and try the
same command manually:

```bash
dsh web --no-open --host 127.0.0.1 --port 0
```

If it fails there too, the problem is in the `dsh` installation, not the tray.
Verify with `dsh-tray --check`.

### An external dsh instance is already running

The tray always starts its own `dsh web` on a free port (`port: 0`), so an
already-running server does not conflict with it. The tray never attaches to or
kills that other server; stop it yourself if you want the tray's instance to be
the only one.

### Reset the configuration

Quit the tray, then remove or rename `config.json` in the config directory. The
next start uses defaults (the file is rewritten on the next settings change).
Logs can be removed the same way.

### macOS: Gatekeeper blocks the app

Release builds are not signed or notarized. If macOS quarantines the downloaded
bundle, either right-click the app and choose **Open**, or clear the quarantine
attribute:

```bash
xattr -dr com.apple.quarantine "/Applications/DSH Tray.app"
```

Only do this for a build you trust.

### Windows: SmartScreen warning

The release executable is unsigned, so SmartScreen may show "Windows protected
your PC". Verify the SHA-256 checksum from `checksums.txt` before choosing
**More info → Run anyway**; build from source if that is not acceptable.

### macOS and Windows are less tested

Linux is the primary development platform. macOS and Windows builds are produced
by CI on every release, but their tray, autostart and process-control paths have
had less manual testing. Bug reports with the relevant log lines are welcome.

## Development

Common commands:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

CI (`.github/workflows/ci.yml`) runs rustfmt and clippy on Linux and the test
suite on Linux, macOS and Windows. The release workflow
(`.github/workflows/release.yml`) builds native Linux (x86_64, aarch64), Windows
and macOS (Intel, Apple Silicon) artifacts, wraps the macOS build in a
`DSH Tray.app` bundle, writes SHA-256 checksums and attaches everything to the
tagged GitHub Release. See [docs/PACKAGING.md](docs/PACKAGING.md) for the
artifact layout and the version-bump checklist.

Icons:

The tray icon is the DeepSeek whale, recoloured per state (gray = stopped,
green = running, red = error). The official whale mask and application icon are
committed under `assets/`; the state variants are generated from them:

```bash
python3 scripts/gen-icons.py                # needs Pillow; regenerates committed assets
scripts/make-macos-icns.sh                  # macOS only; writes build/AppIcon.icns
```

Repository layout:

| Path | Purpose |
| --- | --- |
| `src/` | Rust sources (see `docs/ARCHITECTURE.md`) |
| `assets/` | Tray and application icons |
| `packaging/` | Desktop entry and the per-user install script |
| `scripts/` | Icon generation helpers |
| `docs/` | Architecture and packaging documentation |

Cross-compilation caveat: the GUI stack links platform frameworks. Building
macOS artifacts needs macOS (Xcode command line tools), and Windows artifacts
need the MSVC toolchain; that is why the release workflow builds natively on
each runner instead of cross-compiling. Do not expect a Linux host to produce
working macOS or Windows binaries with a simple `--target` flag.

## License

MIT — see [LICENSE](LICENSE).

## Acknowledgements

- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) — the
  `dsh` runtime and web UI this tray controls.
- [tao](https://github.com/tauri-apps/tao),
  [tray-icon](https://github.com/tauri-apps/tray-icon) and
  [muda](https://github.com/tauri-apps/muda) — window/event-loop, tray and menu
  plumbing.
- [Pillow](https://python-pillow.org/) — used by `scripts/gen-icons.py`.
