#!/bin/sh
# Compiles Kuvo Code's icon (mac/AppIcon.icon, Icon Composer's format) into
# <folder>/AppIcon.icns, for Kuvo Helper (scripts/build-helper.sh's ICON: what Login Items and
# macOS's questions show for it), without building the desktop bundle.
# Needs Xcode (actool).
#   scripts/helper-icon.sh <folder>
set -eu

if [ $# -ne 1 ] || [ -z "$1" ]; then
    echo "Usage: scripts/helper-icon.sh <folder>" >&2
    exit 2
fi
mkdir -p "$1"
FOLDER=$(cd "$1" && pwd -P)
cd "$(dirname "$0")/.."
temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-helper-icon.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
xcrun actool mac/AppIcon.icon --compile "$work" --app-icon AppIcon --platform macosx \
    --minimum-deployment-target "${MIN_SYSTEM:-26.0}" --output-partial-info-plist "$work/partial.plist" >/dev/null
test -s "$work/AppIcon.icns" || { echo "actool made no AppIcon.icns from mac/AppIcon.icon." >&2; exit 1; }
mv -f "$work/AppIcon.icns" "$FOLDER/AppIcon.icns"
echo "$FOLDER/AppIcon.icns"
