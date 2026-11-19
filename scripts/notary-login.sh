#!/bin/sh
# Saves your Apple notary credentials in the keychain, where
# `scripts/bundle.sh --release` looks for them. Needed once per Mac.
#
#   scripts/notary-login.sh            reads your Team ID from your Developer ID certificate
#   scripts/notary-login.sh TEAMID     or takes it from you
#
# Run it yourself, in a terminal. It asks for your Apple ID and for an app-specific
# password, which you create at account.apple.com under Sign-In and Security. The
# password goes to Apple's own notarytool, which checks it with Apple and stores it
# in your keychain. This script never sees or keeps it.
set -eu

PROFILE=${JOBSEARCH_NOTARY_PROFILE:-jobsearch-notary}
TEAM=${1:-}

# The Team ID is the code in brackets at the end of the certificate's name. Look in
# the keychain first, then at a certificate downloaded but not yet installed.
if [ -z "$TEAM" ]; then
    TEAM=$(security find-identity -v -p codesigning \
        | sed -n 's/.*"Developer ID Application: .*(\([A-Z0-9]*\))".*/\1/p' | head -1)
fi
if [ -z "$TEAM" ]; then
    for file in "$HOME"/Downloads/developerID_application*.cer; do
        [ -f "$file" ] || continue
        TEAM=$(openssl x509 -inform der -in "$file" -noout -subject 2>/dev/null \
            | sed -n 's/.*OU *= *\([A-Z0-9]*\).*/\1/p')
        [ -n "$TEAM" ] && break
    done
fi
if [ -z "$TEAM" ]; then
    echo "error: no Developer ID certificate to read your Team ID from." >&2
    echo "       Create the certificate first, or give the Team ID yourself:" >&2
    echo "       scripts/notary-login.sh TEAMID   (it is under Membership details at developer.apple.com/account)" >&2
    exit 1
fi

echo "Team ID: $TEAM"
printf 'Apple ID (the email you sign in to developer.apple.com with): '
read -r APPLE_ID
[ -n "$APPLE_ID" ] || { echo "error: no Apple ID given." >&2; exit 1; }
echo "Next it asks for the app-specific password. Paste it; nothing shows as you type."
exec xcrun notarytool store-credentials "$PROFILE" --apple-id "$APPLE_ID" --team-id "$TEAM"
