#!/usr/bin/env bash
# Build a macOS .icns application icon from a square PNG.
#
# Usage:
#   scripts/make-macos-icns.sh [SOURCE_PNG] [OUTPUT_ICNS]
#
# Defaults: assets/app/icon.png -> build/AppIcon.icns
#
# Requires macOS: sips (image resizing) and iconutil (iconset -> icns).
# The release workflow runs this on the macOS runners only.
set -eu

SOURCE=${1:-assets/app/icon.png}
OUTPUT=${2:-build/AppIcon.icns}

die() {
    printf 'make-macos-icns: %s\n' "$1" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] || die "this script requires macOS (sips and iconutil)"
[ -f "$SOURCE" ] || die "source image not found: $SOURCE"
for tool in sips iconutil; do
    command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done

OUTPUT_DIR=$(dirname "$OUTPUT")
ICONSET="${OUTPUT%.icns}.iconset"

mkdir -p "$OUTPUT_DIR"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

# The @caret suffixes are the names iconutil expects. 1x/2x pairs cover
# 16/32/64/128/256/512/1024 px entries.
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil -c icns "$ICONSET" -o "$OUTPUT"
rm -rf "$ICONSET"
printf 'wrote %s\n' "$OUTPUT"
