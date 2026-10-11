#!/bin/sh
# Uploads the notarization login for the release workflow, after checking with
# Apple that it works.
#   scripts/set-notary-secrets.sh
# Sets APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD. The password is an
# app-specific password from account.apple.com, not your Apple ID password.
set -eu

REPO=${REPO:-serhatandic/kuvo}

printf 'Apple ID (email): '
IFS= read -r APPLE_ID
printf 'Team ID [C8Y523R7T5]: '
IFS= read -r TEAM_ID
TEAM_ID=${TEAM_ID:-C8Y523R7T5}
printf 'App-specific password: '
stty -echo
IFS= read -r PASSWORD
stty echo
echo

# Trim surrounding whitespace, the usual cause of "invalid credentials".
APPLE_ID=$(printf '%s' "$APPLE_ID" | tr -d '[:space:]')
TEAM_ID=$(printf '%s' "$TEAM_ID" | tr -d '[:space:]')
PASSWORD=$(printf '%s' "$PASSWORD" | tr -d '[:space:]')

if ! xcrun notarytool history --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$PASSWORD" >/dev/null 2>&1; then
    echo "Apple didn't accept that login. Nothing was uploaded." >&2
    echo "Check the Apple ID and team ID, or create a new app-specific password at account.apple.com." >&2
    exit 1
fi
echo "Apple accepted the login."

printf '%s' "$APPLE_ID" | gh secret set APPLE_ID --repo "$REPO"
printf '%s' "$TEAM_ID" | gh secret set APPLE_TEAM_ID --repo "$REPO"
printf '%s' "$PASSWORD" | gh secret set APPLE_APP_PASSWORD --repo "$REPO"
echo "Updated APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD on $REPO."
