#!/bin/sh
# Builds a release Terrarium.app into ./build. No Xcode project required.
set -eu
cd "$(dirname "$0")/.."

VERSION="${VERSION:-0.1.0}"
APP="build/Terrarium.app"

swift build -c release --arch arm64
BIN="$(swift build -c release --arch arm64 --show-bin-path)/Terrarium"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Terrarium"
strip -rSTx "$APP/Contents/MacOS/Terrarium"
[ -f Resources/AppIcon.icns ] && cp Resources/AppIcon.icns "$APP/Contents/Resources/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Terrarium</string>
    <key>CFBundleDisplayName</key><string>Terrarium</string>
    <key>CFBundleIdentifier</key><string>dev.terrarium.app</string>
    <key>CFBundleExecutable</key><string>Terrarium</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>LSArchitecturePriority</key><array><string>arm64</string></array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --sign - --entitlements Terrarium.entitlements "$APP"
echo "Built $APP ($(du -sh "$APP" | cut -f1)), binary $(du -h "$APP/Contents/MacOS/Terrarium" | cut -f1)"
