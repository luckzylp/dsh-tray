#!/bin/sh
# Install dsh-tray for the current user (no sudo, no system-wide changes).
#
# Usage:
#   packaging/install.sh [--binary PATH]
#
# Environment overrides:
#   BINDIR   directory for the binary       (default: $HOME/.local/bin)
#   ICONDIR  directory for the icon         (default: $HOME/.local/share/icons/hicolor/256x256/apps)
#   APPDIR   applications directory         (default: $HOME/.local/share/applications)
set -eu

BINDIR=${BINDIR:-$HOME/.local/bin}
ICONDIR=${ICONDIR:-$HOME/.local/share/icons/hicolor/256x256/apps}
APPDIR=${APPDIR:-$HOME/.local/share/applications}
BINARY=./dsh-tray

usage() {
    cat <<'EOF'
Usage: packaging/install.sh [--binary PATH]

Installs dsh-tray for the current user:
  binary -> $HOME/.local/bin/dsh-tray
  icon   -> $HOME/.local/share/icons/hicolor/256x256/apps/dsh-tray.png
  entry  -> $HOME/.local/share/applications/dsh-tray.desktop

Options:
  --binary PATH  dsh-tray binary to install (default: ./dsh-tray)
  -h, --help     show this help
EOF
}

die() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary)
            [ "$#" -ge 2 ] || die "--binary needs a path"
            BINARY=$2
            shift 2
            ;;
        --binary=*)
            BINARY=${1#*=}
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

[ -f "$BINARY" ] || die "binary not found: $BINARY"
[ -x "$BINARY" ] || die "binary is not executable: $BINARY"

# This script lives in packaging/; the repository root is one level up.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
ROOT_DIR=$(dirname "$SCRIPT_DIR")
ICON_SRC=$ROOT_DIR/assets/app/icon.png
DESKTOP_SRC=$SCRIPT_DIR/dsh-tray.desktop

mkdir -p "$BINDIR"
cp "$BINARY" "$BINDIR/dsh-tray"
chmod 0755 "$BINDIR/dsh-tray"

INSTALLED_ICON=
if [ -f "$ICON_SRC" ]; then
    mkdir -p "$ICONDIR"
    cp "$ICON_SRC" "$ICONDIR/dsh-tray.png"
    chmod 0644 "$ICONDIR/dsh-tray.png"
    INSTALLED_ICON=$ICONDIR/dsh-tray.png
else
    printf 'install.sh: warning: %s not found; skipping icon install\n' "$ICON_SRC" >&2
fi

INSTALLED_DESKTOP=
if [ -f "$DESKTOP_SRC" ]; then
    mkdir -p "$APPDIR"
    cp "$DESKTOP_SRC" "$APPDIR/dsh-tray.desktop"
    chmod 0644 "$APPDIR/dsh-tray.desktop"
    INSTALLED_DESKTOP=$APPDIR/dsh-tray.desktop
else
    printf 'install.sh: warning: %s not found; skipping desktop entry\n' "$DESKTOP_SRC" >&2
fi

# Refresh desktop and icon caches when the helpers exist; failures are harmless.
if [ -n "$INSTALLED_DESKTOP" ] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
fi
if [ -n "$INSTALLED_ICON" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
    ICON_THEME_DIR=$(dirname "$(dirname "$ICONDIR")")
    gtk-update-icon-cache -q -t -f "$ICON_THEME_DIR" >/dev/null 2>&1 || true
fi

printf '\nInstalled:\n'
printf '  binary:  %s\n' "$BINDIR/dsh-tray"
if [ -n "$INSTALLED_ICON" ]; then
    printf '  icon:    %s\n' "$INSTALLED_ICON"
fi
if [ -n "$INSTALLED_DESKTOP" ]; then
    printf '  desktop: %s\n' "$INSTALLED_DESKTOP"
fi

printf '\nNext steps:\n'
case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *) printf '  ! %s is not on PATH; add it to your shell profile.\n' "$BINDIR" ;;
esac
printf '  1. Install the dsh CLI if needed: npm install -g @deepseek-ai/dsh@latest\n'
printf '  2. Start the tray: dsh-tray\n'
printf '\nThe desktop entry runs "dsh-tray" by name, so the binary must be on PATH.\n'
