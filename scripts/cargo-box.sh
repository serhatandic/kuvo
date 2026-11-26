#!/bin/sh
# Run Cargo in the box workspace with its own build lane.
set -eu
scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$scripts/../box"
# Several worktrees build at once; incremental caches cost 7–14 GB each.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

# A test run keeps away from the real Kuvo Code, as the TypeScript tests do (box/bridge-tests/test-safety.js).
# A shell an agent runs in carries Kuvo Code's own variables, KUVO_WAKES among them, naming the real
# folders, and the box reads them: a move test once paused a chat's wake-ups in the real
# ~/.kuvo-code/wakes. So they go, with the coding agents' folders and sockets, and what you set on
# purpose to run tests stays (KUVO_TEST_* and the other *TEST* switches, KUVO_RS<n>_*, the TypeScript
# bundles and golden folders a test compares with, and the lane's own KUVO_HEAVY_*). A temporary folder that's Kuvo Code's is swapped
# for the system's, as a box treats one as its own.
case "${1-}" in
test | nextest)
	for name in $(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p'); do
		case "$name" in
		*TEST* | KUVO_HEAVY_* | KUVO_RS[0-9]* | KUVO_OWNER_PARTS_TS | KUVO_MOVE_GOLDEN_ROOT | KUVO_CURSOR_SDK) ;;
		KUVO_* | OPENCODE_* | XDG_* | CLAUDE_CODE_* | CLAUDE_CONFIG_DIR | CLAUDECODE | CODEX_HOME | GEMINI_HOME | \
			CURSOR_DATA_DIR | CURSOR_SANDBOX_POLICY_DIR | CUA_APP_SUPPORT_DIR | GROK_HOME | SSH_AUTH_SOCK | TMUX | TMUX_PANE)
			unset "$name"
			;;
		esac
	done
	case "${TMPDIR-}/" in
	*/.kuvo-code/* | */run.kuvo.code*/*)
		if [ "$(uname)" = Darwin ]; then TMPDIR=$(/usr/bin/getconf DARWIN_USER_TEMP_DIR); else TMPDIR=/tmp; fi
		export TMPDIR
		;;
	esac
	;;
esac
exec "$scripts/heavy.sh" rust-box cargo "$@"
