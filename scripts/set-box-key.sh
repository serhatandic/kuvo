#!/bin/sh
# Sets up signing for what remote machines run (box releases), once. The first time, it makes the
# Ed25519 key and keeps it in your login keychain, as "Kuvo Code box signing key" (beside the
# app's update key); it uploads it as KUVO_BOX_SIGNING_KEY for the release workflow; and it prints
# the public key, which kuvo-link.js pins (UPDATE_KEYS). From then on, every release signs what
# machines run by itself. Run it again on a new Mac, or if the secret is lost: it uploads the same
# key from your keychain.
#   scripts/set-box-key.sh
set -eu
cd "$(dirname "$0")/.."

REPO=${REPO:-serhatandic/kuvo-code}
SERVICE="Kuvo Code box signing key"
ACCOUNT=kuvo-box
PINS=Packages/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js

if ! command -v gh >/dev/null 2>&1; then
    echo "GitHub's command line tool isn't installed: brew install gh, then gh auth login." >&2
    exit 1
fi
if ! gh auth status >/dev/null 2>&1; then
    echo "gh isn't signed in: run gh auth login, then this again." >&2
    exit 1
fi

umask 077
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if KEY=$(security find-generic-password -s "$SERVICE" -a "$ACCOUNT" -w 2>/dev/null); then
    printf '%s' "$KEY" | base64 -d > "$TMP/key.pem"
    echo "Using the box signing key in your keychain."
else
    if ! openssl genpkey -algorithm ed25519 -out "$TMP/key.pem" 2>/dev/null; then
        echo "This openssl can't make Ed25519 keys: brew install openssl, then this again." >&2
        exit 1
    fi
    # Through security's own command input, so the key is never on a command line.
    KEY=$(base64 < "$TMP/key.pem" | tr -d '\n')
    printf 'add-generic-password -s "%s" -a %s -l "%s" -j "%s" -w %s\n' "$SERVICE" "$ACCOUNT" "$SERVICE" \
        "Signs what Kuvo Code's remote machines run (box releases). The release workflow has it as KUVO_BOX_SIGNING_KEY." "$KEY" | security -i >/dev/null
    if ! security find-generic-password -s "$SERVICE" -a "$ACCOUNT" >/dev/null 2>&1; then
        echo "The key couldn't be kept in your keychain. Nothing was uploaded." >&2
        exit 1
    fi
    echo "Made a box signing key and kept it in your login keychain, as \"$SERVICE\"."
fi

PUBLIC=$(openssl pkey -in "$TMP/key.pem" -pubout -outform DER | tail -c 32 | base64)
if [ "$(printf '%s' "$PUBLIC" | base64 -d | wc -c | tr -d ' ')" != 32 ]; then
    echo "The key in your keychain isn't an Ed25519 key. Nothing was uploaded." >&2
    exit 1
fi

gh secret set KUVO_BOX_SIGNING_KEY --repo "$REPO" < "$TMP/key.pem"
echo "Set KUVO_BOX_SIGNING_KEY on $REPO."
if grep -q "\"$PUBLIC\"" "$PINS"; then
    echo "kuvo-link.js pins its public key already: releases carry what machines run, signed."
else
    echo "Its public key, for UPDATE_KEYS in $PINS:"
    echo "  $PUBLIC"
fi
