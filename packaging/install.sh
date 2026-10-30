#!/bin/sh
# Install dsh-tray for one user, self-contained under <home>/.local/dsh-tray.
#
# The install root holds the binary plus copies of the icon and desktop entry,
# and keeps backups of every file this script replaces (including the desktop
# entry/icon installed into the user's XDG directories) under
# <install-root>/backups/<timestamp>/.
#
# Usage:
#   packaging/install.sh [--binary PATH] [--prefix DIR] [--home DIR] [--no-desktop]
#
# Defaults:
#   --home    $HOME
#   --prefix  <home>/.local/dsh-tray   (e.g. /home/Kevin/.local/dsh-tray)
#   --binary  ./dsh-tray, falling back to target/release/dsh-tray
#
# Environment overrides:
#   DSH_TRAY_PREFIX  install root (same as --prefix)
#   DSH_TRAY_HOME    target user home (same as --home)
#   APPDIR           XDG applications directory (default <home>/.local/share/applications)
#   ICONDIR          icon-theme directory (default <home>/.local/share/icons/hicolor/256x256/apps)
set -eu

APP=dsh-tray
TARGET_HOME=${DSH_TRAY_HOME:-${HOME:?HOME is not set}}
PREFIX=${DSH_TRAY_PREFIX:-}
BINARY=
INSTALL_DESKTOP=1

usage() {
    cat <<'EOF'
Usage: packaging/install.sh [OPTIONS]

Installs dsh-tray for one user into a self-contained directory:

  <home>/.local/dsh-tray/
    dsh-tray              the executable
    icon.png              application icon copy
    dsh-tray.desktop      launcher entry (absolute Exec/Icon paths)
    backups/<timestamp>/  previous files replaced by this install

The desktop entry and icon are also registered in the user's XDG directories
unless --no-desktop is given.

Options:
  --home DIR       target user home (default: $HOME)
  --prefix DIR     install root (default: <home>/.local/dsh-tray)
  --binary PATH    dsh-tray binary to install (default: ./dsh-tray or
                   target/release/dsh-tray)
  --no-desktop     only populate the install root; skip the XDG menu entry
                   and icon-theme copies
  -h, --help       show this help

Environment: DSH_TRAY_HOME, DSH_TRAY_PREFIX, APPDIR, ICONDIR.
Replaced files are backed up under <install-root>/backups/<timestamp>/.
EOF
}

die() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

# Expand a leading ~ and require an absolute path.
absolute_path() {
    case "$1" in
        "~") printf '%s' "$HOME" ;;
        "~/"*) printf '%s/%s' "$HOME" "${1#~/}" ;;
        /*) printf '%s' "$1" ;;
        *) die "path must be absolute (or start with ~/): $1" ;;
    esac
}

quote_exec() {
    # Desktop Entry Exec quoting: escape \ " and $ inside a double-quoted argument.
    escaped=$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\$/\\$/g')
    printf '"%s"' "$escaped"
}

write_desktop_entry() {
    output=$1
    executable=$2
    icon_path=$3
    quoted_exec=$(quote_exec "$executable")
    {
        printf '%s\n' '[Desktop Entry]'
        printf '%s\n' 'Type=Application'
        printf '%s\n' 'Name=DSH Tray'
        printf '%s\n' 'Name[zh_CN]=DSH 托盘'
        printf '%s\n' 'Comment=System tray controller for DeepSeek Harness'
        printf 'Exec=%s\n' "$quoted_exec"
        if [ -n "$icon_path" ]; then
            printf 'Icon=%s\n' "$icon_path"
        fi
        printf '%s\n' 'Terminal=false'
        printf '%s\n' 'Categories=Utility;Development;'
        printf '%s\n' 'StartupNotify=false'
        printf '%s\n' 'StartupWMClass=dsh-tray'
    } > "$output"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --home)
            [ "$#" -ge 2 ] || die "--home needs a path"
            TARGET_HOME=$2
            shift 2
            ;;
        --home=*)
            TARGET_HOME=${1#*=}
            shift
            ;;
        --prefix)
            [ "$#" -ge 2 ] || die "--prefix needs a path"
            PREFIX=$2
            shift 2
            ;;
        --prefix=*)
            PREFIX=${1#*=}
            shift
            ;;
        --binary)
            [ "$#" -ge 2 ] || die "--binary needs a path"
            BINARY=$2
            shift 2
            ;;
        --binary=*)
            BINARY=${1#*=}
            shift
            ;;
        --no-desktop)
            INSTALL_DESKTOP=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

TARGET_HOME=$(absolute_path "$TARGET_HOME")
if [ -z "$PREFIX" ]; then
    PREFIX=$TARGET_HOME/.local/$APP
fi
PREFIX=$(absolute_path "$PREFIX")

if [ -z "$BINARY" ]; then
    for candidate in ./dsh-tray target/release/dsh-tray; do
        if [ -f "$candidate" ]; then
            BINARY=$candidate
            break
        fi
    done
fi
[ -n "$BINARY" ] || die "no binary found; build one first or pass --binary PATH"
[ -f "$BINARY" ] || die "binary not found: $BINARY"
[ -x "$BINARY" ] || die "binary is not executable: $BINARY"
BINARY_DIR=$(CDPATH= cd -- "$(dirname "$BINARY")" && pwd)
BINARY=$BINARY_DIR/$(basename "$BINARY")

# This script lives in packaging/; the repository root is one level up.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
ROOT_DIR=$(dirname "$SCRIPT_DIR")
ICON_SRC=$ROOT_DIR/assets/app/icon.png

APPDIR=${APPDIR:-$TARGET_HOME/.local/share/applications}
ICONDIR=${ICONDIR:-$TARGET_HOME/.local/share/icons/hicolor/256x256/apps}
BACKUP_ROOT=$PREFIX/backups
BACKUP_DIR=

# Back up one file that is about to be replaced. The first call creates a
# unique timestamped directory inside the install root.
backup_file() {
    source_path=$1
    label=$2
    [ -e "$source_path" ] || return 0
    if [ -z "$BACKUP_DIR" ]; then
        stamp=$(date -u +%Y%m%dT%H%M%SZ)
        candidate=$BACKUP_ROOT/$stamp
        counter=1
        while [ -e "$candidate" ]; do
            candidate=$BACKUP_ROOT/$stamp-$counter
            counter=$((counter + 1))
        done
        BACKUP_DIR=$candidate
        mkdir -p "$BACKUP_DIR"
    fi
    destination=$BACKUP_DIR/$label
    mkdir -p "$(dirname "$destination")"
    cp -p "$source_path" "$destination"
}

mkdir -p "$PREFIX"

# 1. Binary.
binary_target=$PREFIX/dsh-tray
if [ -e "$binary_target" ] && cmp -s "$BINARY" "$binary_target"; then
    printf 'install.sh: binary already current: %s\n' "$binary_target"
else
    backup_file "$binary_target" "prefix/dsh-tray"
    cp "$BINARY" "$binary_target"
fi
chmod 0755 "$binary_target"

# 2. Icon copy inside the install root (this is the desktop-icon backup
#    location: the icon always exists next to the executable).
ICON_READY=0
if [ -f "$ICON_SRC" ]; then
    backup_file "$PREFIX/icon.png" "prefix/icon.png"
    cp "$ICON_SRC" "$PREFIX/icon.png"
    chmod 0644 "$PREFIX/icon.png"
    ICON_READY=1
else
    printf 'install.sh: warning: %s not found; skipping icon install\n' "$ICON_SRC" >&2
fi

# 3. Launcher entry inside the install root, with absolute paths so it works
#    without adding the directory to PATH.
PREFIX_DESKTOP=$PREFIX/dsh-tray.desktop
backup_file "$PREFIX_DESKTOP" "prefix/dsh-tray.desktop"
if [ "$ICON_READY" -eq 1 ]; then
    write_desktop_entry "$PREFIX_DESKTOP.tmp" "$binary_target" "$PREFIX/icon.png"
else
    write_desktop_entry "$PREFIX_DESKTOP.tmp" "$binary_target" ""
fi
mv "$PREFIX_DESKTOP.tmp" "$PREFIX_DESKTOP"
chmod 0644 "$PREFIX_DESKTOP"

# 4. Optional XDG integration; every replaced file is backed up first.
if [ "$INSTALL_DESKTOP" -eq 1 ]; then
    if [ "$ICON_READY" -eq 1 ]; then
        mkdir -p "$ICONDIR"
        backup_file "$ICONDIR/dsh-tray.png" "icons/dsh-tray.png"
        cp "$PREFIX/icon.png" "$ICONDIR/dsh-tray.png"
        chmod 0644 "$ICONDIR/dsh-tray.png"
    fi
    mkdir -p "$APPDIR"
    backup_file "$APPDIR/dsh-tray.desktop" "applications/dsh-tray.desktop"
    cp "$PREFIX_DESKTOP" "$APPDIR/dsh-tray.desktop"
    chmod 0644 "$APPDIR/dsh-tray.desktop"

    # Refresh desktop and icon caches when the helpers exist; failures are harmless.
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
    fi
    if [ "$ICON_READY" -eq 1 ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        ICON_THEME_DIR=$(dirname "$(dirname "$ICONDIR")")
        gtk-update-icon-cache -q -t -f "$ICON_THEME_DIR" >/dev/null 2>&1 || true
    fi
fi

printf '\nInstalled:\n'
printf '  binary:  %s\n' "$binary_target"
printf '  root:    %s\n' "$PREFIX"
if [ "$ICON_READY" -eq 1 ]; then
    printf '  icon:    %s\n' "$PREFIX/icon.png"
fi
printf '  desktop: %s\n' "$PREFIX_DESKTOP"
if [ "$INSTALL_DESKTOP" -eq 1 ]; then
    printf '  menu:    %s\n' "$APPDIR/dsh-tray.desktop"
    if [ "$ICON_READY" -eq 1 ]; then
        printf '  theme:   %s\n' "$ICONDIR/dsh-tray.png"
    fi
fi
if [ -n "$BACKUP_DIR" ]; then
    printf '  backup:  %s\n' "$BACKUP_DIR"
fi

printf '\nRun it with:\n  %s\n' "$binary_target"
if [ "$INSTALL_DESKTOP" -eq 1 ]; then
    printf 'Or launch "DSH Tray" from the desktop application menu.\n'
else
    printf 'XDG menu integration skipped (--no-desktop); use the absolute path above.\n'
fi
printf '\nInstall the dsh CLI if needed: npm install -g @deepseek-ai/dsh@latest\n'
