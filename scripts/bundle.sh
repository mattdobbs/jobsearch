#!/bin/sh
# Builds dist/JobSearch.app, the desktop app.
#
#   scripts/bundle.sh              build the app into dist/, signed for this Mac only
#   scripts/bundle.sh --install    also copy it into /Applications
#   scripts/bundle.sh --universal  build for Intel Macs as well as Apple silicon
#   scripts/bundle.sh --release    sign it with your Developer ID, have Apple notarize
#                                  it, and make dist/JobSearch-<version>.dmg, which
#                                  opens on other people's Macs without a warning
#
# --release needs two things set up once on this Mac (the README has the steps):
#   - a "Developer ID Application" certificate in your keychain;
#   - your notary credentials, saved by running scripts/notary-login.sh.
#
# Environment, for --release:
#   JOBSEARCH_SIGN_IDENTITY   the certificate to sign with, when the keychain has several
#   JOBSEARCH_NOTARY_PROFILE  the name the notary credentials were saved under
#                             (default: jobsearch-notary)
set -eu

cd "$(dirname "$0")/.."

fail() {
    echo "error: $*" >&2
    exit 1
}

INSTALL=no
UNIVERSAL=no
RELEASE=no
for option in "$@"; do
    case "$option" in
        --install) INSTALL=yes ;;
        --universal) UNIVERSAL=yes ;;
        --release) RELEASE=yes ;;
        *) fail "unknown option $option (see the top of scripts/bundle.sh)" ;;
    esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/JobSearch.app"
DMG="dist/JobSearch-$VERSION.dmg"
NOTARY_PROFILE=${JOBSEARCH_NOTARY_PROFILE:-jobsearch-notary}

# Settled before the build, so a missing certificate is reported straight away.
IDENTITY=-
if [ "$RELEASE" = yes ]; then
    IDENTITY=${JOBSEARCH_SIGN_IDENTITY:-}
    if [ -z "$IDENTITY" ]; then
        FOUND=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | sort -u)
        case $(printf '%s' "$FOUND" | grep -c . || true) in
            0) fail "no \"Developer ID Application\" certificate in your keychain. It comes with
       Apple Developer Program membership; see \"Signing for other people's Macs\" in the README." ;;
            1) IDENTITY=$FOUND ;;
            *) fail "your keychain has several Developer ID certificates. Set JOBSEARCH_SIGN_IDENTITY to one of:
$FOUND" ;;
        esac
    fi
    echo "Signing as: $IDENTITY"
fi

# The oldest macOS the app runs on; Info.plist below says the same.
export MACOSX_DEPLOYMENT_TARGET=11.0

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "$UNIVERSAL" = yes ]; then
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        rustup target list --installed | grep -qx "$target" \
            || fail "the Rust target $target is not installed. Add it with: rustup target add $target"
        cargo build --release --target "$target"
    done
    lipo -create -output "$APP/Contents/MacOS/JobSearch" \
        target/aarch64-apple-darwin/release/jobsearch target/x86_64-apple-darwin/release/jobsearch
else
    cargo build --release
    cp target/release/jobsearch "$APP/Contents/MacOS/JobSearch"
fi
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
    <string>${MACOSX_DEPLOYMENT_TARGET}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
</dict>
</plist>
EOF

# Every build runs under the hardened runtime, which notarization requires, so a
# build for this Mac behaves the same as a released one. The app needs no
# entitlements: its web view does its work in a separate system process.
if [ "$RELEASE" = yes ]; then
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
    codesign --verify --strict --verbose=2 "$APP"
else
    # Ad-hoc signature, so macOS treats the bundle as one consistent app on this Mac.
    codesign --force --options runtime --sign - "$APP" 2>/dev/null \
        || echo "note: codesign was not available; the app still runs on this Mac"
fi

# Finder caches icons by modification time; this makes it notice a rebuilt bundle.
touch "$APP"

echo "Built $APP"

# Sends an archive to Apple's notary service and waits for the verdict.
notarize() {
    LOG=$(mktemp)
    xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1 | tee "$LOG"
    if ! grep -Eiq 'status"?[[:space:]]*:[[:space:]]*"?Accepted' "$LOG"; then
        rm -f "$LOG"
        fail "Apple did not notarize $1. If it gave a submission id above, see why with:
       xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE
       If it could not sign in, save your credentials first: scripts/notary-login.sh
       If it says an agreement is missing, accept it at developer.apple.com/account"
    fi
    rm -f "$LOG"
}

if [ "$RELEASE" = yes ]; then
    STAGE=$(mktemp -d)
    # Cleared however the script ends, a refusal from Apple included.
    trap 'rm -rf "$STAGE"' EXIT

    # The app first: with its own ticket attached it opens on a Mac that is offline,
    # wherever it is copied to. Only a zip of an app can be submitted.
    ditto -c -k --keepParent "$APP" "$STAGE/JobSearch.zip"
    notarize "$STAGE/JobSearch.zip"
    rm -f "$STAGE/JobSearch.zip"
    xcrun stapler staple "$APP"

    # Then the disk image people download: the app beside a shortcut to Applications.
    ditto "$APP" "$STAGE/JobSearch.app"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG"
    hdiutil create -quiet -volname "JobSearch" -srcfolder "$STAGE" -format UDZO "$DMG"
    codesign --force --timestamp --sign "$IDENTITY" "$DMG"
    notarize "$DMG"
    xcrun stapler staple "$DMG"

    # What Gatekeeper will say on someone else's Mac.
    spctl --assess --type execute --verbose=2 "$APP"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
    echo "Ready to share: $DMG"
fi

if [ "$INSTALL" = yes ]; then
    rm -rf "/Applications/JobSearch.app"
    cp -R "$APP" "/Applications/JobSearch.app"
    echo "Installed /Applications/JobSearch.app"
fi
