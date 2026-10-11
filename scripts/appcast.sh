#!/bin/sh
# Prints the Sparkle update feed for one release:
#   scripts/appcast.sh VERSION ZIP URL > appcast.xml
# URL is where ZIP will be downloaded from. The zip is signed with the update
# key from SPARKLE_PRIVATE_KEY if set (CI), or else from the login keychain.
set -eu

VERSION=$1
ZIP=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
URL=$3
cd "$(dirname "$0")/.."

TOOL=$(find .build/artifacts -path '*/Sparkle/bin/sign_update' -type f | head -n 1)
if [ -z "$TOOL" ]; then
    echo "sign_update not found; build the app first." >&2
    exit 1
fi

if [ -n "${SPARKLE_PRIVATE_KEY:-}" ]; then
    SIGNATURE=$(printf '%s' "$SPARKLE_PRIVATE_KEY" | "$TOOL" --ed-key-file - "$ZIP")
else
    SIGNATURE=$("$TOOL" "$ZIP")
fi
case "$SIGNATURE" in
    *edSignature=*length=*) ;;
    *) echo "Signing the update failed: $SIGNATURE" >&2; exit 1 ;;
esac

cat <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Kuvo</title>
    <item>
      <title>Kuvo $VERSION</title>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>https://github.com/serhatandic/kuvo/releases/tag/v$VERSION</sparkle:fullReleaseNotesLink>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <enclosure url="$URL" $SIGNATURE type="application/octet-stream"/>
    </item>
  </channel>
</rss>
XML
