#!/bin/sh
# Builds a release Kuvo.app into ./build. No Xcode project required.
#   VERSION         version to stamp into the app (default 0.1.0)
#   SIGN_IDENTITY   codesigning identity, e.g. "Developer ID Application: …";
#                   the default "-" signs ad hoc, for local use
#   FEED_URL        update feed (default: the newest GitHub release); for
#                   testing updates against a local server
set -eu
cd "$(dirname "$0")/.."

VERSION="${VERSION:-0.1.0}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
APP="build/Kuvo.app"

# Updates: Sparkle reads the feed from the newest GitHub release and only
# installs updates signed with the private half of this key.
FEED_URL="${FEED_URL:-https://github.com/serhatandic/kuvo-box/releases/latest/download/appcast.xml}"
UPDATE_PUBLIC_KEY="uG/YbzL4kf+DAYCiTTfK8ztEOBo96hkEAuKk9rw9BE4="

swift build -c release --arch arm64
BIN="$(swift build -c release --arch arm64 --show-bin-path)/Kuvo"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Kuvo"
strip -rSTx "$APP/Contents/MacOS/Kuvo"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/Kuvo"

# Sparkle, trimmed to what Kuvo uses: Apple silicon only, no XPC services
# (only sandboxed apps need them), no headers, English only.
SPARKLE=$(find .build/artifacts -path "*macos*" -name Sparkle.framework -type d | head -n 1)
FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
mkdir -p "$APP/Contents/Frameworks"
ditto "$SPARKLE" "$FRAMEWORK"
(
    cd "$FRAMEWORK"
    rm -rf XPCServices Headers PrivateHeaders Modules \
        Versions/B/XPCServices Versions/B/Headers Versions/B/PrivateHeaders Versions/B/Modules
    find Versions/B/Resources -name '*.lproj' ! -name 'Base.lproj' ! -name 'en.lproj' -exec rm -rf {} +
    for binary in Versions/B/Sparkle Versions/B/Autoupdate Versions/B/Updater.app/Contents/MacOS/Updater; do
        lipo "$binary" -thin arm64 -output "$binary.arm64" && mv "$binary.arm64" "$binary"
    done
)
# The icon, compiled from Resources/AppIcon.icon by scripts/build-icon.sh.
cp Resources/Icon/Assets.car Resources/Icon/AppIcon.icns "$APP/Contents/Resources/"
# The license requires these notices to travel with every copy.
cp LICENSE NOTICE THIRD_PARTY_LICENSES.txt "$APP/Contents/Resources/"
# Finder, the Dock and Spotlight show a bundle's file name unless it declares a
# localized display name, so the app reads "Kuvo Box" while staying Kuvo.app. The
# unlocalized display name in Info.plist must match the file name, or macOS takes
# the file as renamed by its owner and shows the file name.
mkdir -p "$APP/Contents/Resources/en.lproj"
printf '"CFBundleDisplayName" = "Kuvo Box";\n"CFBundleName" = "Kuvo Box";\n' > "$APP/Contents/Resources/en.lproj/InfoPlist.strings"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Kuvo Box</string>
    <key>CFBundleDisplayName</key><string>Kuvo</string>
    <key>LSHasLocalizedDisplayName</key><true/>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleIdentifier</key><string>run.kuvo.app</string>
    <key>CFBundleExecutable</key><string>Kuvo</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIconName</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>SUFeedURL</key><string>$FEED_URL</string>
    <key>SUPublicEDKey</key><string>$UPDATE_PUBLIC_KEY</string>
    <key>SUEnableAutomaticChecks</key><true/>
    <key>LSArchitecturePriority</key><array><string>arm64</string></array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Sign inside out: Sparkle's helpers, the framework, then the app. Notarization
# requires the hardened runtime and a secure timestamp on every executable.
if [ "$SIGN_IDENTITY" = "-" ]; then
    set -- --force --sign -
else
    set -- --force --sign "$SIGN_IDENTITY" --options runtime --timestamp
fi
codesign "$@" "$FRAMEWORK/Versions/B/Autoupdate"
codesign "$@" "$FRAMEWORK/Versions/B/Updater.app"
codesign "$@" "$FRAMEWORK"
codesign "$@" --entitlements Kuvo.entitlements "$APP"
echo "Built $APP ($(du -sh "$APP" | cut -f1)), binary $(du -h "$APP/Contents/MacOS/Kuvo" | cut -f1)"
