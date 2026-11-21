#!/bin/sh
# Builds build/KuvoCode.dmg from build/Kuvo.app (scripts/build-desktop-app.sh),
# with the app next to a link to Applications for drag-to-install.
#   APP             another desktop app bundle to put in the image
#   DMG             where it goes (default build/KuvoCode.dmg)
#   SIGN_IDENTITY   if set to a real identity, the disk image is signed too
set -eu
cd "$(dirname "$0")/.."

APP="${APP:-build/Kuvo.app}"
DMG="${DMG:-build/KuvoCode.dmg}"
if [ ! -d "$APP" ]; then
    echo "Build the app first: scripts/build-desktop-app.sh" >&2
    exit 1
fi

# dmgbuild writes the window layout directly instead of scripting Finder, so it
# also works on headless CI machines. Kept in its own environment under build.
VENV=build/dmgbuild
if [ ! -x "$VENV/bin/dmgbuild" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet --disable-pip-version-check "dmgbuild==1.6.5"
fi

rm -f "$DMG"
"$VENV/bin/dmgbuild" -s scripts/dmg-settings.py -D app="$APP" "Kuvo Code" "$DMG"

if [ -n "${SIGN_IDENTITY:-}" ] && [ "$SIGN_IDENTITY" != "-" ]; then
    codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG"
fi
echo "Built $DMG ($(du -h "$DMG" | cut -f1))"
