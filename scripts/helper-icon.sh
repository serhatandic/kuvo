#!/bin/sh
# Compiles Kuvo Code's icon (swift/KuvoCode/AppIcon.icon, Icon Composer's format) into
# <folder>/AppIcon.icns, for Kuvo Helper (scripts/build-helper.sh's ICON: what Login Items and
# macOS's questions show for it), with no app built: the same file Xcode made in the Mac app.
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
work=$(mktemp -d "${TMPDIR:-/tmp}/kuvo-helper-icon.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
xcrun actool swift/KuvoCode/AppIcon.icon --compile "$work" --app-icon AppIcon --platform macosx \
    --minimum-deployment-target "${MIN_SYSTEM:-26.0}" --output-partial-info-plist "$work/partial.plist" >/dev/null
test -s "$work/AppIcon.icns" || { echo "actool made no AppIcon.icns from swift/KuvoCode/AppIcon.icon." >&2; exit 1; }
mv -f "$work/AppIcon.icns" "$FOLDER/AppIcon.icns"
echo "$FOLDER/AppIcon.icns"
