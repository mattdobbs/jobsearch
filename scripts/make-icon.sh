#!/bin/sh
# Regenerates the app icon (assets/AppIcon.icns) and the browser-tab icon
# (assets/favicon.png) from the drawing in scripts/icon.swift.
#
# Only needed after changing the drawing: the generated files are checked in, so
# building the app does not need this.
set -eu

cd "$(dirname "$0")/.."

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

swiftc -O scripts/icon.swift -o "$WORK/icon"
"$WORK/icon" "$WORK/icon-1024.png"

# macOS wants each size at 1x and 2x.
SET="$WORK/AppIcon.iconset"
mkdir "$SET"
for size in 16 32 128 256 512; do
    double=$((size * 2))
    sips -z "$size" "$size" "$WORK/icon-1024.png" --out "$SET/icon_${size}x${size}.png" >/dev/null
    sips -z "$double" "$double" "$WORK/icon-1024.png" --out "$SET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o assets/AppIcon.icns

# The browser-tab icon is the tile without the margin around it, so it fills the tab.
sips -c 824 824 "$WORK/icon-1024.png" --out "$WORK/tile.png" >/dev/null
sips -z 64 64 "$WORK/tile.png" --out assets/favicon.png >/dev/null

echo "Wrote assets/AppIcon.icns and assets/favicon.png"
