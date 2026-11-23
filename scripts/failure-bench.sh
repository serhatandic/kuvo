#!/bin/bash
# The multi-computer failure bench (#919). The bridge test guard creates disposable homes.
set -eu
cd "$(dirname "$0")/.."
# Clear inherited settings even when invoked outside combined-check.
while IFS= read -r name; do
    case "$name" in KUVO_*) unset "$name" ;; esac
done < <(compgen -e)
# This commit is part of main's history. A shallow checkout may need it explicitly.
resource=swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js
for pin in failure-bench-legacy-ref recovery-neighbor-ref; do
    read -r previous < "box/bridge-tests/$pin.txt"
    [[ "$previous" =~ ^[0-9a-f]{40}$ ]] || { echo "Invalid build reference in $pin.txt" >&2; exit 1; }
    if ! git --no-optional-locks cat-file -e "${previous}:$resource" 2>/dev/null; then
        git --no-optional-locks fetch --no-tags --depth=1 origin "$previous"
        git --no-optional-locks cat-file -e "${previous}:$resource"
    fi
done
if [[ "${1:-}" == --prepare ]]; then exit 0; fi
# Each file launches its own subprocesses and servers. Bound the number of suites
# competing for time with the combined check's other lanes.
exec scripts/test-box.sh bridge --test-concurrency=2 '--test-name-pattern=^failure bench:' \
    computer-backups.test.js failure-bench-legacy.test.js folder-version.test.js \
    box/bridge-tests/setup-handover*.test.js devices-two-boxes.test.js updates.test.js \
    box/bridge-tests/computers-add*.test.js installer-enrollment.test.js install.test.js ssh-route.test.js
