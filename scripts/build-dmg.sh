#!/bin/sh
# Builds build/Kuvo.dmg from build/Kuvo.app (run scripts/build-app.sh first):
# the app next to a link to Applications, laid out for drag-to-install.
#   SIGN_IDENTITY   if set to a real identity, the disk image is signed too
set -eu
cd "$(dirname "$0")/.."

APP=build/Kuvo.app
DMG=build/Kuvo.dmg
if [ ! -d "$APP" ]; then
    echo "Build the app first: scripts/build-app.sh" >&2
    exit 1
fi

# dmgbuild writes the window layout directly instead of scripting Finder, so it
# also works on headless CI machines. Kept in its own environment under .build.
VENV=.build/dmgbuild
if [ ! -x "$VENV/bin/dmgbuild" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet --disable-pip-version-check "dmgbuild==1.6.5"
fi

rm -f "$DMG"
"$VENV/bin/dmgbuild" -s scripts/dmg-settings.py -D app="$APP" "Kuvo Box" "$DMG"

if [ -n "${SIGN_IDENTITY:-}" ] && [ "$SIGN_IDENTITY" != "-" ]; then
    codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG"
fi
echo "Built $DMG ($(du -h "$DMG" | cut -f1))"
