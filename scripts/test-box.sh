#!/bin/bash
# Run the box and machine bridge suites in a slot of heavy.sh's box lane (two runs may go at once).
set -u
root=$(cd "$(dirname "$0")/.." && pwd -P)
mode=${1:-all}
case "$mode" in box|bridge|all) [ $# -eq 0 ] || shift ;; *) echo 'Usage: scripts/test-box.sh [box|bridge|all] [extra test args...]' >&2; exit 64 ;; esac
heavy=${KUVO_HEAVY_SH:-$root/scripts/heavy.sh}
if ! "$heavy" --holds box; then
    exec "$heavy" box "$root/scripts/test-box.sh" "$mode" "$@"
fi
cd "$root" || exit 1
# BUN selects the build runtime. KUVO_TEST_BUN can exercise a different runtime.
build_bun=$(. scripts/build-bun.sh && printf "%s\n" "$BUN") || exit 1
bun=${KUVO_TEST_BUN:-$build_bun}
if [[ "${bun##*/}" == opencode ]]; then export BUN_BE_BUN=1; fi
export KUVO_TEST_BUN="$bun" BUN_RUNTIME_TRANSPILER_CACHE_PATH=0
box="$root/box"
BUN="$build_bun" scripts/build-built.sh || exit 1
if [ ! -f "$box/node_modules/typescript/package.json" ] ||
   [ "$box/bun.lock" -nt "$box/node_modules" ] || [ "$box/package.json" -nt "$box/node_modules" ]; then
    echo 'Installing locked box test dependencies...'
    (cd "$box" && BUN_BE_BUN=1 "$build_bun" install --frozen-lockfile && touch node_modules) || exit 1
fi
logs=$(mktemp -d /tmp/kuvo-test-box.XXXXXXXX) || exit 1
export TMPDIR="$logs" KUVO_TEST_WRAPPER_PID=$$
trap 'rm -rf "$logs"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
box_status=skip bridge_status=skip
if [ "$mode" != bridge ]; then
    echo 'Running box suite...'
    (cd "$box" && "$bun" test "$@") >"$logs/box" 2>&1
    status=$?; cat "$logs/box"
    if [ "$status" -eq 0 ]; then box_status=pass; else box_status=fail; fi
fi
if [ "$mode" != box ]; then
    echo 'Running bridge suite...'
    set +u # macOS Bash 3 treats empty arrays as unset.
    bridge_args=() bridge_files=()
    for arg in "$@"; do
        case "$arg" in
            *.test.js) case "$arg" in */*) bridge_files+=("${arg#box/}") ;; *) bridge_files+=("bridge-tests/$arg") ;; esac ;;
            *) bridge_args+=("$arg") ;;
        esac
    done
    if [ ${#bridge_files[@]} -eq 0 ]; then
        (cd "$box" && node --test "${bridge_args[@]}" bridge-tests/*.test.js) >"$logs/bridge" 2>&1
    else
        (cd "$box" && node --test "${bridge_args[@]}" "${bridge_files[@]}") >"$logs/bridge" 2>&1
    fi
    status=$?; set -u; cat "$logs/bridge"
    if [ "$status" -eq 0 ]; then bridge_status=pass; else bridge_status=fail; fi
fi
printf '\nSummary:\n'
for suite in box bridge; do
    eval "result=\${${suite}_status}"
    printf '%s: %s\n' "$suite" "$result"
    if [ -f "$logs/$suite" ]; then
        if [ "$suite" = box ]; then
            grep -E '^ *[0-9]+ (pass|fail|skip)|^\(fail\)' "$logs/$suite" || true
        else
            grep -E '^(# |ℹ )(tests|pass|fail|skipped) |^not ok |^✖ ' "$logs/$suite" || true
        fi
    fi
done
[ "$box_status" != fail ] && [ "$bridge_status" != fail ]
