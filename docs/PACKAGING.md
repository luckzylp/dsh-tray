# Packaging and releases

This document describes how `dsh-tray` release artifacts are built, how to
reproduce them locally, and what to check before tagging a release.

## Artifacts at a glance

Every tagged release (`v*`) is built by
[.github/workflows/release.yml](../.github/workflows/release.yml) and attached to
the GitHub Release for that tag:

| Platform | Runner | Artifact | Contents |
| --- | --- | --- | --- |
| Linux x86_64 | `ubuntu-latest` | `dsh-tray-linux-x86_64.tar.gz` | binary, `LICENSE`, `README.md`, `README.zh-CN.md` |
| Linux aarch64 | `ubuntu-24.04-arm` | `dsh-tray-linux-aarch64.tar.gz` | same layout |
| Windows x86_64 | `windows-latest` | `dsh-tray-windows-x86_64.zip` | `dsh-tray.exe` (blue whale icon embedded by `build.rs`), `LICENSE`, `README.md`, `README.zh-CN.md` |
| macOS x86_64 | `macos-13` | `dsh-tray-macos-x86_64.tar.gz` | same layout as Linux |
| macOS aarch64 | `macos-14` | `dsh-tray-macos-aarch64.tar.gz` | same layout as Linux |
| macOS bundle | `macos-13` / `macos-14` | `DSH-Tray-macos-<arch>.app.tar.gz` | `DSH Tray.app` bundle |
| All | — | `checksums.txt` | SHA-256, one line per artifact |

Artifacts are built **natively** on each runner (no cross-compilation): the GUI
stack (tao/tray-icon) links platform frameworks, so a macOS binary must be built
on macOS and a Windows binary on Windows.

## How CI builds the artifacts

### Continuous integration

[.github/workflows/ci.yml](../.github/workflows/ci.yml) runs on pushes to `main`
and on pull requests, with `contents: read` permission:

- **fmt** — Linux, `cargo fmt --all -- --check`.
- **clippy** — Linux, with GTK/AppIndicator development packages installed,
  `cargo clippy --all-targets --all-features -- -D warnings`.
- **test** — matrix over Ubuntu, Windows and macOS, `cargo test --all-targets`,
  with `fail-fast: false` so every platform reports.

Linux jobs install the build dependencies first:

```bash
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libayatana-appindicator3-dev
```

### Release workflow

The release workflow runs on a push of a `v*` tag and can also be triggered
manually (`workflow_dispatch`; a manual run builds and uploads workflow
artifacts but does not publish a GitHub Release, because `github.ref` is not a
tag). Each `build` job reads the package version from `Cargo.toml` into a step
output:

```bash
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
```

Each build job then:

1. installs the Rust toolchain and caches Cargo,
2. runs `cargo build --release --locked`,
3. stages the binary plus `LICENSE`, `README.md` and `README.zh-CN.md`,
4. on macOS, additionally builds the application bundle (see below),
5. writes a per-platform `checksums-<matrix name>.txt`,
6. uploads its files with `actions/upload-artifact`.

The separate `release` job downloads every artifact, merges the per-platform
checksum files into a single `checksums.txt`, verifies all hashes with
`sha256sum -c`, and attaches the files to the GitHub Release with
`softprops/action-gh-release`. Release notes are generated from the tag history;
no repository URL is hard-coded anywhere.

### macOS application bundle

macOS artifacts are produced in two forms: the plain `dsh-tray-macos-<arch>.tar.gz`
(for users who want just the binary) and a `DSH Tray.app` bundle. The bundle is
assembled by hand in the workflow:

```text
DSH Tray.app/
└── Contents/
    ├── Info.plist
    ├── MacOS/
    │   └── dsh-tray
    └── Resources/
        └── AppIcon.icns
```

The icon is generated from `assets/app/icon.png` by
[scripts/make-macos-icns.sh](../scripts/make-macos-icns.sh), which uses `sips`
to render an `.iconset` (16/32/64/128/256/512/1024 px) and `iconutil` to
convert it.

`Info.plist` keys:

| Key | Value |
| --- | --- |
| `CFBundleIdentifier` | `ai.deepseek.dsh-tray` |
| `CFBundleName` | `DSH Tray` |
| `CFBundleDisplayName` | `DSH Tray` |
| `CFBundleExecutable` | `dsh-tray` |
| `CFBundlePackageType` | `APPL` |
| `CFBundleShortVersionString` | package version from `Cargo.toml` |
| `CFBundleVersion` | same package version |
| `CFBundleIconFile` | `AppIcon` |
| `LSUIElement` | `true` (tray-only app: no Dock icon) |
| `NSHighResolutionCapable` | `true` |

The workflow prints
`/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString"` as a sanity
check before packing the bundle into
`DSH-Tray-macos-<arch>.app.tar.gz`.

## Manual local packaging

### Linux

```bash
cargo build --release --locked
mkdir -p dist/dsh-tray
cp target/release/dsh-tray dist/dsh-tray/
cp LICENSE README.md README.zh-CN.md dist/dsh-tray/
tar -czf dsh-tray-linux-$(uname -m).tar.gz -C dist/dsh-tray .
shasum -a 256 dsh-tray-linux-$(uname -m).tar.gz > checksums.txt
```

For a per-user install with desktop integration, use the install script from the
checkout:

```bash
packaging/install.sh --binary target/release/dsh-tray
```

The default install root is `~/.local/dsh-tray` and mirrors the repository
layout: `bin/dsh-tray`, `assets/app/{icon.ico,icon.png,whale.svg}`,
`assets/icons/` (all tray PNGs plus `whale-base-32.png`), and `packaging/` with a
generated `dsh-tray.desktop` (absolute `Exec`/`Icon` paths) and a copy of
`install.sh`. Re-running the copied installer updates that tree; the menu entry
is registered in `~/.local/share/applications` unless `--no-desktop` is passed.
`--home DIR` targets another user's home, `--prefix DIR` sets a custom root, and
`APPDIR` overrides the applications directory. Every replaced file is backed up
under the hidden `<install-root>/.backups/<timestamp>/` before it is overwritten,
so the desktop icon/assets always have a backup inside the install tree.

### macOS

```bash
cargo build --release --locked
app="dist/DSH Tray.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/dsh-tray "$app/Contents/MacOS/dsh-tray"
scripts/make-macos-icns.sh assets/app/icon.png "$app/Contents/Resources/AppIcon.icns"
# Write Contents/Info.plist with the keys listed above, then:
codesign --force --deep --sign - "$app"   # optional ad-hoc signature, not notarization
tar -czf DSH-Tray-macos-$(uname -m).app.tar.gz -C dist "DSH Tray.app"
```

CI does **not** sign or notarize the bundle; an ad-hoc signature is only useful
for local testing.

### Windows

```powershell
cargo build --release --locked
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\dsh-tray.exe dist\
Copy-Item LICENSE, README.md, README.zh-CN.md dist\
Compress-Archive -Path dist\* -DestinationPath dsh-tray-windows-x86_64.zip
Get-FileHash dsh-tray-windows-x86_64.zip -Algorithm SHA256
```

## Version bump checklist

1. Update `version` in `Cargo.toml` (the only place the package version is
   defined; the macOS `Info.plist` reads it from here during CI).
2. Run `cargo build --locked` so `Cargo.lock` records the new version, then
   `cargo test --all-targets`.
3. Run `cargo fmt --all -- --check` and
   `cargo clippy --all-targets --all-features -- -D warnings`.
4. Make sure the CI workflow is green on the release commit.
5. Commit the bump, then tag and push:

   ```bash
   git tag -a v0.2.0 -m "dsh-tray 0.2.0"
   git push origin v0.2.0
   ```

6. Watch the release workflow: all five build jobs must succeed and the `release`
   job must attach exactly the artifacts listed above.
7. Verify the release: download each artifact, check `checksums.txt` with
   `sha256sum -c checksums.txt` (or `shasum -a 256 -c`), and smoke-test at least
   one binary (`dsh-tray --version`, `dsh-tray --check`).

The tag name (`v0.2.0`) and the Cargo version (`0.2.0`) should match; the
workflow uses `github.ref_name` for the release title and does not derive the
version from the tag, so a mismatch would produce confusing artifact metadata.

## Platform notes and caveats

### Linux: runtime dependencies and the AppIndicator host

Building needs `libgtk-3-dev` and `libayatana-appindicator3-dev`. Running needs
the GTK3 runtime and `libayatana-appindicator3`, plus a StatusNotifier/
AppIndicator host in the desktop session. GNOME requires the **AppIndicator and
KStatusNotifierItem Support** extension; sessions without any host will not show
the icon (this affects every AppIndicator application).

### macOS: unsigned builds and Gatekeeper

CI produces unsigned, non-notarized bundles. Gatekeeper may quarantine them.
Users can right-click the app and choose **Open** or clear the attribute:

```bash
xattr -dr com.apple.quarantine "/Applications/DSH Tray.app"
```

Do not describe release artifacts as signed or notarized until a signing step is
actually added to the workflow.

### Windows: unsigned builds and SmartScreen

The executable is unsigned, so SmartScreen may warn. Users should verify
`checksums.txt` before choosing **More info → Run anyway**. Code signing is not
configured in CI.

### Unverified platforms

Linux is the primary development platform. macOS and Windows builds are compiled
by CI, but the tray, autostart and process-control paths on those platforms have
had less manual testing. When changing platform code, run the relevant CI job
before tagging.

## Where configuration and logs live

| OS | Config | Data (logs, runtime state) |
| --- | --- | --- |
| Linux | `$XDG_CONFIG_HOME/dsh-tray` (default `~/.config/dsh-tray`) | `$XDG_DATA_HOME/dsh-tray` (default `~/.local/share/dsh-tray`) |
| macOS | `~/Library/Application Support/ai.deepseek.dsh-tray` | same |
| Windows | `%APPDATA%\deepseek\dsh-tray\config` | `%APPDATA%\deepseek\dsh-tray\data` |

Logs are `logs/dsh-tray.log` (tray, rotated at 2 MiB) and `logs/dsh-web.log`
(raw output of the managed `dsh web` child, including the authenticated URL — do
not publish it). `runtime.json` and `instance.lock` also live in the data
directory. `DSH_TRAY_CONFIG_DIR` and `DSH_TRAY_DATA_DIR` override both paths,
which is useful for portable packaging and tests.

## Icons

- `scripts/gen-icons.py` regenerates the committed assets under `assets/`
  (requires Pillow; the application icon additionally needs Python GI Rsvg and
  pycairo). The official DeepSeek whale sources are
  `assets/icons/whale-base-32.png` (silhouette mask for the state-coloured tray
  icons) and `assets/app/whale.svg` (the blue `#4D6BFE` application-icon logo).
  Outputs: `assets/icons/tray-{stopped,running,error}.png` and their `@2x`
  versions, plus `assets/app/icon.png` (1024px blue whale) and
  `assets/app/icon.ico`.
- Tray state colours: gray = stopped/starting, green = running, red = failed
  start/stop/update, startup timeout, config/autostart error or unexpected
  child exit.
- `build.rs` embeds `assets/app/icon.ico` into native Windows executables via
  the `winresource` build dependency, so Explorer/taskbar shortcuts show the
  blue whale. Linux uses the installed `.desktop` icon and macOS uses the
  `.apps` bundle icon generated from `assets/app/icon.png`.
- `scripts/make-macos-icns.sh` builds `build/AppIcon.icns` from
  `assets/app/icon.png` on macOS.

Keep the committed icons in sync with the generator when the artwork changes.
