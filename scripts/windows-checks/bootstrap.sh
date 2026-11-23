# Sent as the SSH command. stdin is the checkout archive.
set -euo pipefail
umask 077
base=$HOME/kuvo-checks
[ ! -L "$base" ] || { echo "Refusing linked checks root: $base" >&2; exit 1; }
mkdir -p "$base"
run=$(mktemp -d "$base/$1-$(date -u +%Y%m%d-%H%M%S)-XXXXXX")
echo "Run: $run"
cleanup() {
    status=$?
    trap '' PIPE
    cd /
    du -sk -- "$run" | awk '{ print "Run footprint before cleanup: " $1 " KiB" }' || true
    rm -rf -- "$run" || { if [ "$status" -eq 0 ]; then status=1; fi; }
    printf 'Remaining in %s:\n' "$base"
    ls -A -- "$base"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$run/repo"
tar -xzf - -C "$run/repo"
[ -f "$run/repo/.windows-checks/ready" ] || { echo "Checkout staging failed." >&2; exit 1; }
bash "$run/repo/.windows-checks/wsl.sh" "$run" "$base/tools" "$2"
