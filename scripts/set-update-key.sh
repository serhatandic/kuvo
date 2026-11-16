#!/bin/sh
# Uploads the update signing key for the release workflow.
#   scripts/set-update-key.sh
# Sets SPARKLE_PRIVATE_KEY from the key in your login keychain, the one every
# Kuvo app signs its updates with, after checking that it matches the public key
# the desktop app trusts (scripts/build-desktop-app.sh, REAL_UPDATE_PUBLIC_KEY; 0.7 trusts the
# same). macOS asks you to allow access to the key. Run it yourself, in Terminal.
set -eu
cd "$(dirname "$0")/.."

REPO=${REPO:-serhatandic/kuvo-code}

TOOL="$(./scripts/sparkle.sh)/bin/generate_keys"

EXPECTED=$(sed -n 's/^REAL_UPDATE_PUBLIC_KEY="\(.*\)"$/\1/p' scripts/build-desktop-app.sh)
ACTUAL=$("$TOOL" -p 2>/dev/null || true)
if [ -z "$EXPECTED" ] || [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "The update key in your keychain doesn't match scripts/build-desktop-app.sh. Nothing was uploaded." >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
"$TOOL" -x "$TMP/key"
gh secret set SPARKLE_PRIVATE_KEY --repo "$REPO" < "$TMP/key"
echo "Updated SPARKLE_PRIVATE_KEY on $REPO."
