#!/bin/sh
# Prints the Sparkle update feed for one release, or a beta (VERSION like 0.7.0-beta.1, in the
# feed's "beta" channel, which only copies that get betas read); scripts/merge-appcast.py adds
# the newest of the other kind from the feed that's out:
#   scripts/appcast.sh VERSION FILE URL [NOTES] > appcast.xml
# URL is where FILE (the desktop app's disk image) will be downloaded from. The file is signed
# with the update key from SPARKLE_PRIVATE_KEY if set (CI), or else from the login keychain, by
# the sign_update of the Sparkle the app carries (scripts/sparkle.sh). Kuvo Code 0.7 reads the same
# feed and takes these items too: their sparkle:version (scripts/build-version.sh) goes on from
# 0.7's, so 0.7 updates into the desktop app.
# NOTES is a Markdown file the update window shows (see scripts/release-notes.sh).
set -eu

VERSION=$1
FILE=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
URL=$3
NOTES=${4:+$(cd "$(dirname "$4")" && pwd)/$(basename "$4")}
cd "$(dirname "$0")/.."

BUILD=$(./scripts/build-version.sh "$VERSION")
CHANNEL=""
case "$VERSION" in *-beta.*) CHANNEL="
      <sparkle:channel>beta</sparkle:channel>" ;; esac

DESCRIPTION=""
if [ -n "$NOTES" ]; then
    if grep -q ']]>' "$NOTES"; then
        echo "The release notes can't contain ]]>." >&2
        exit 1
    fi
    DESCRIPTION="<description sparkle:format=\"markdown\"><![CDATA[
$(cat "$NOTES")
]]></description>"
fi

TOOL="$(./scripts/sparkle.sh)/bin/sign_update"

if [ -n "${SPARKLE_PRIVATE_KEY:-}" ]; then
    SIGNATURE=$(printf '%s' "$SPARKLE_PRIVATE_KEY" | "$TOOL" --ed-key-file - "$FILE")
else
    SIGNATURE=$("$TOOL" "$FILE")
fi
case "$SIGNATURE" in
    *edSignature=*length=*) ;;
    *) echo "Signing the update failed: $SIGNATURE" >&2; exit 1 ;;
esac

cat <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Kuvo Code</title>
    <item>
      <title>Kuvo $VERSION</title>
      <sparkle:version>$BUILD</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>$CHANNEL
      <sparkle:minimumSystemVersion>26.0</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>https://github.com/serhatandic/kuvo-code/releases/tag/v$VERSION</sparkle:fullReleaseNotesLink>
      $DESCRIPTION
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <enclosure url="$URL" $SIGNATURE type="application/octet-stream"/>
    </item>
  </channel>
</rss>
XML
