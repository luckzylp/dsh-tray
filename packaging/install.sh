#!/bin/sh
# Install dsh-tray for one user as a self-contained tree under
# <home>/.local/dsh-tray:
#
#   bin/dsh-tray
#   assets/app/{icon.ico,icon.png,whale.svg}
#   assets/icons/{tray-*.png,whale-base-32.png}
#   packaging/{dsh-tray.desktop,install.sh}
#
# The desktop entry in packaging/ is generated with absolute Exec/Icon paths.
# Replaced files are backed up under <install-root>/.backups/<timestamp>/.
#
# Usage:
#   packaging/install.sh [--binary PATH] [--prefix DIR] [--home DIR] [--no-desktop]
#
# Defaults:
#   --home    $HOME
#   --prefix  <home>/.local/dsh-tray   (e.g. /home/Kevin/.local/dsh-tray)
#   --binary  ./dsh-tray, target/release/dsh-tray, or <prefix>/bin/dsh-tray
#
# Environment overrides:
#   DSH_TRAY_PREFIX  install root (same as --prefix)
#   DSH_TRAY_HOME    target user home (same as --home)
#   APPDIR           XDG applications directory for the menu entry
#                    (default <home>/.local/share/applications)
set -eu

APP=dsh-tray
TARGET_HOME=${DSH_TRAY_HOME:-${HOME:?HOME is not set}}
PREFIX=${DSH_TRAY_PREFIX:-}
BINARY=
INSTALL_DESKTOP=1
HOME_EXPLICIT=0
if [ -n "${DSH_TRAY_HOME:-}" ]; then
    HOME_EXPLICIT=1
fi

usage() {
    cat <<'EOF'
Usage: packaging/install.sh [OPTIONS]

Installs dsh-tray for one user as a self-contained tree:

  <home>/.local/dsh-tray/
    bin/dsh-tray
    assets/app/icon.ico
    assets/app/icon.png
    assets/app/whale.svg
    assets/icons/tray-error.png       (and @2x)
    assets/icons/tray-running.png     (and @2x)
    assets/icons/tray-stopped.png     (and @2x)
    assets/icons/whale-base-32.png
    packaging/dsh-tray.desktop        (absolute Exec/Icon paths)
    packaging/install.sh              (this installer, for reinstalls)
    .backups/<timestamp>/             (hidden; replaced files only)

The generated desktop entry is also registered in the user's applications
directory unless --no-desktop is given.

Options:
  --home DIR       target user home (default: $HOME)
  --prefix DIR     install root (default: <home>/.local/dsh-tray)
  --binary PATH    dsh-tray binary to install (default: ./dsh-tray,
                   target/release/dsh-tray, or <root>/bin/dsh-tray)
  --no-desktop     only populate the install tree; skip the XDG menu entry
  -h, --help       show this help

Environment: DSH_TRAY_HOME, DSH_TRAY_PREFIX, APPDIR.
Replaced files are backed up under <install-root>/.backups/<timestamp>/.
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
    qe_escaped=$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\$/\\$/g')
    printf '"%s"' "$qe_escaped"
}

write_desktop_entry() {
    wd_output=$1
    wd_executable=$2
    wd_icon=$3
    wd_quoted_exec=$(quote_exec "$wd_executable")
    {
        printf '%s\n' '[Desktop Entry]'
        printf '%s\n' 'Type=Application'
        printf '%s\n' 'Name=DSH Tray'
        printf '%s\n' 'Name[zh_CN]=DSH 托盘'
        printf '%s\n' 'Comment=System tray controller for DeepSeek Harness'
        printf 'Exec=%s\n' "$wd_quoted_exec"
        if [ -n "$wd_icon" ]; then
            printf 'Icon=%s\n' "$wd_icon"
        fi
        printf '%s\n' 'Terminal=false'
        printf '%s\n' 'Categories=Utility;Development;'
        printf '%s\n' 'StartupNotify=false'
        printf '%s\n' 'StartupWMClass=dsh-tray'
    } > "$wd_output"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --home)
            [ "$#" -ge 2 ] || die "--home needs a path"
            TARGET_HOME=$2
            HOME_EXPLICIT=1
            shift 2
            ;;
        --home=*)
            TARGET_HOME=${1#*=}
            HOME_EXPLICIT=1
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

# This script lives in packaging/; the repository root (or the installed tree
# root when the copied script is re-run) is one level up.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
SCRIPT_PATH=$SCRIPT_DIR/$(basename "$0")
ROOT_DIR=$(dirname "$SCRIPT_DIR")

if [ -z "$PREFIX" ]; then
    if [ "$HOME_EXPLICIT" -eq 0 ] \
        && [ -f "$ROOT_DIR/bin/dsh-tray" ] \
        && [ -f "$ROOT_DIR/packaging/install.sh" ]; then
        # Re-running the installer from an installed tree keeps that tree,
        # even if HOME points somewhere else (e.g. sudo/other session).
        PREFIX=$ROOT_DIR
    else
        PREFIX=$TARGET_HOME/.local/$APP
    fi
fi
PREFIX=$(absolute_path "$PREFIX")

if [ -z "$BINARY" ]; then
    for candidate in ./dsh-tray target/release/dsh-tray "$ROOT_DIR/bin/dsh-tray"; do
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

APPDIR=${APPDIR:-$TARGET_HOME/.local/share/applications}
BACKUP_ROOT=$PREFIX/.backups
BACKUP_DIR=

# Back up one file that is about to be replaced. The first call creates a
# hidden timestamped directory inside the install root.
backup_file() {
    bf_source=$1
    bf_label=$2
    [ -e "$bf_source" ] || return 0
    if [ -z "$BACKUP_DIR" ]; then
        bf_stamp=$(date -u +%Y%m%dT%H%M%SZ)
        bf_candidate=$BACKUP_ROOT/$bf_stamp
        bf_counter=1
        while [ -e "$bf_candidate" ]; do
            bf_candidate=$BACKUP_ROOT/$bf_stamp-$bf_counter
            bf_counter=$((bf_counter + 1))
        done
        BACKUP_DIR=$bf_candidate
        mkdir -p "$BACKUP_DIR"
    fi
    bf_destination=$BACKUP_DIR/$bf_label
    mkdir -p "$(dirname "$bf_destination")"
    cp -p "$bf_source" "$bf_destination"
}

# Copy one file into the install tree, preserving an explicit mode and
# backing up a previous version when the content differs.
install_file() {
    if_source=$1
    if_destination=$2
    if_mode=$3
    if_label=$4
    if [ ! -f "$if_source" ]; then
        printf 'install.sh: warning: %s not found; skipping %s\n' "$if_source" "$if_label" >&2
        return 0
    fi
    if [ -f "$if_destination" ] && cmp -s "$if_source" "$if_destination"; then
        chmod "$if_mode" "$if_destination"
        return 0
    fi
    backup_file "$if_destination" "$if_label"
    mkdir -p "$(dirname "$if_destination")"
    cp "$if_source" "$if_destination"
    chmod "$if_mode" "$if_destination"
}

mkdir -p "$PREFIX/bin" "$PREFIX/assets/app" "$PREFIX/assets/icons" "$PREFIX/packaging"

# 1. Executable.
install_file "$BINARY" "$PREFIX/bin/dsh-tray" 0755 "bin/dsh-tray"

# 2. Application/tray assets, mirroring assets/ from the repository.
for name in icon.ico icon.png whale.svg; do
    install_file "$ROOT_DIR/assets/app/$name" "$PREFIX/assets/app/$name" 0644 "assets/app/$name"
done
for name in tray-error.png tray-error@2x.png tray-running.png tray-running@2x.png \
    tray-stopped.png tray-stopped@2x.png whale-base-32.png; do
    install_file "$ROOT_DIR/assets/icons/$name" "$PREFIX/assets/icons/$name" 0644 "assets/icons/$name"
done

# 3. Desktop entry inside the install tree (absolute paths).
PREFIX_DESKTOP=$PREFIX/packaging/dsh-tray.desktop
if [ -f "$PREFIX/assets/app/icon.png" ]; then
    write_desktop_entry "$PREFIX/packaging/.dsh-tray.desktop.tmp" "$PREFIX/bin/dsh-tray" "$PREFIX/assets/app/icon.png"
else
    write_desktop_entry "$PREFIX/packaging/.dsh-tray.desktop.tmp" "$PREFIX/bin/dsh-tray" ""
fi
if [ -f "$PREFIX_DESKTOP" ] && cmp -s "$PREFIX/packaging/.dsh-tray.desktop.tmp" "$PREFIX_DESKTOP"; then
    rm -f "$PREFIX/packaging/.dsh-tray.desktop.tmp"
else
    backup_file "$PREFIX_DESKTOP" "packaging/dsh-tray.desktop"
    mv "$PREFIX/packaging/.dsh-tray.desktop.tmp" "$PREFIX_DESKTOP"
fi
chmod 0644 "$PREFIX_DESKTOP"

# 4. Copy the installer itself so the installed tree can be used to reinstall.
install_file "$SCRIPT_PATH" "$PREFIX/packaging/install.sh" 0755 "packaging/install.sh"

# 5. Optional XDG menu registration. The entry points at the installed assets,
#    so no separate icon-theme install is needed.
if [ "$INSTALL_DESKTOP" -eq 1 ]; then
    mkdir -p "$APPDIR"
    destination=$APPDIR/dsh-tray.desktop
    if [ -f "$destination" ] && cmp -s "$PREFIX_DESKTOP" "$destination"; then
        :
    else
        backup_file "$destination" "applications/dsh-tray.desktop"
        cp "$PREFIX_DESKTOP" "$destination"
    fi
    chmod 0644 "$destination"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
    fi
fi

printf '\nInstalled tree: %s\n' "$PREFIX"
printf '  bin/dsh-tray\n'
printf '  assets/app/{icon.ico,icon.png,whale.svg}\n'
printf '  assets/icons/{tray-*.png,whale-base-32.png}\n'
printf '  packaging/{dsh-tray.desktop,install.sh}\n'
if [ "$INSTALL_DESKTOP" -eq 1 ]; then
    printf '\nMenu entry: %s\n' "$APPDIR/dsh-tray.desktop"
else
    printf '\nXDG menu entry skipped (--no-desktop); use %s\n' "$PREFIX/bin/dsh-tray"
fi
if [ -n "$BACKUP_DIR" ]; then
    printf '\nBackup: %s\n' "$BACKUP_DIR"
fi
printf '\nRun it with:\n  %s\n' "$PREFIX/bin/dsh-tray"
printf 'Reinstall/update with:\n  %s --binary <new-dsh-tray>\n' "$PREFIX/packaging/install.sh"
printf '\nInstall the dsh CLI if needed: npm install -g @deepseek-ai/dsh@latest\n'
