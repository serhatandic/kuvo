#!/bin/sh
# Compiles Resources/AppIcon.icon into Resources/Icon: Assets.car for macOS 26
# and later (Liquid Glass), and AppIcon.icns for older systems. Needs Xcode 26+.
# The results are committed, so building the app doesn't need this.
set -eu
cd "$(dirname "$0")/.."

OUT=Resources/Icon
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

xcrun actool Resources/AppIcon.icon \
    --compile "$TMP" \
    --platform macosx \
    --minimum-deployment-target 15.0 \
    --app-icon AppIcon \
    --output-partial-info-plist "$TMP/partial.plist" \
    --errors --warnings --notices --output-format human-readable-text

mkdir -p "$OUT"
cp "$TMP/Assets.car" "$TMP/AppIcon.icns" "$OUT/"
echo "Compiled $OUT ($(du -sh "$OUT" | cut -f1))"
