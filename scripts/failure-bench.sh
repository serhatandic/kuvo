#!/bin/bash
# The multi-computer failure bench (#919). The bridge test guard creates disposable homes.
set -eu
cd "$(dirname "$0")/.."
# Clear inherited settings even when invoked outside combined-check.
while IFS= read -r name; do
    case "$name" in KUVO_*) unset "$name" ;; esac
done < <(compgen -e)
# The general bridge suite can run in shallow clones. The bench requires the previous build.
read -r previous < box/bridge-tests/failure-bench-legacy-ref.txt
if ! git --no-optional-locks cat-file -e "${previous}:swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"; then
    echo "The failure bench needs the previous box build at $previous. Use a checkout with that commit." >&2
    exit 1
fi
exec scripts/test-box.sh bridge '--test-name-pattern=^failure bench:' devices-two-boxes.test.js updates.test.js
