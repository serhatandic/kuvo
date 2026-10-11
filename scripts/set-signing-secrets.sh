#!/bin/sh
# Uploads the Developer ID certificate for the release workflow, after checking
# that the .p12 and its password import exactly the way CI imports them.
#   scripts/set-signing-secrets.sh path/to/DeveloperID.p12
# Sets MACOS_CERTIFICATE_P12 and MACOS_CERTIFICATE_PASSWORD. Use it again when
# the certificate is renewed.
set -eu

P12=${1:?usage: scripts/set-signing-secrets.sh path/to/DeveloperID.p12}
REPO=${REPO:-serhatandic/kuvo}

printf 'Password you chose when exporting %s: ' "$(basename "$P12")"
stty -echo
IFS= read -r PASSWORD
stty echo
echo

TMP=$(mktemp -d)
KEYCHAIN="$TMP/check.keychain-db"
trap 'security delete-keychain "$KEYCHAIN" 2>/dev/null; rm -rf "$TMP"' EXIT
security create-keychain -p check "$KEYCHAIN"
security unlock-keychain -p check "$KEYCHAIN"

if ! security import "$P12" -k "$KEYCHAIN" -P "$PASSWORD" -T /usr/bin/codesign >/dev/null 2>&1; then
    echo "That password doesn't open the .p12. Nothing was uploaded." >&2
    exit 1
fi
if ! security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "Developer ID Application"; then
    echo "The .p12 has no Developer ID Application certificate with its private key." >&2
    echo "Export it from Keychain Access › login › My Certificates. Nothing was uploaded." >&2
    exit 1
fi
echo "The certificate imports correctly."

base64 -i "$P12" | gh secret set MACOS_CERTIFICATE_P12 --repo "$REPO"
printf '%s' "$PASSWORD" | gh secret set MACOS_CERTIFICATE_PASSWORD --repo "$REPO"
echo "Updated MACOS_CERTIFICATE_P12 and MACOS_CERTIFICATE_PASSWORD on $REPO."
echo "Delete $P12 when you're done; it contains your private key."
