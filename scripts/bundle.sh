#!/bin/sh
# Builds dist/JobSearch.app, the desktop app.
#
#   scripts/bundle.sh            build the app into dist/
#   scripts/bundle.sh --install  also copy it into /Applications
set -eu

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/JobSearch.app"

cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/jobsearch "$APP/Contents/MacOS/JobSearch"
# The icon is checked in; scripts/make-icon.sh redraws it.
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>JobSearch</string>
    <key>CFBundleDisplayName</key>
    <string>JobSearch</string>
    <key>CFBundleIdentifier</key>
    <string>com.jobsearch.board</string>
    <key>CFBundleExecutable</key>
    <string>JobSearch</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
</dict>
</plist>
EOF

# Ad-hoc signature, so macOS treats the bundle as one consistent app on this Mac.
codesign --force --sign - "$APP" 2>/dev/null || echo "note: codesign was not available; the app still runs on this Mac"

# Finder caches icons by modification time; this makes it notice a rebuilt bundle.
touch "$APP"

echo "Built $APP"

if [ "${1:-}" = "--install" ]; then
    rm -rf "/Applications/JobSearch.app"
    cp -R "$APP" "/Applications/JobSearch.app"
    echo "Installed /Applications/JobSearch.app"
fi
