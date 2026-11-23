#!/bin/sh
# Run all vendor code with blank credentials, private state and loopback-only networking.
set -eu
scratch=$(cd "$1" && pwd -P)
shift
real_home=$(cd "$HOME" && pwd -P)
case "$scratch" in
    "$real_home"/.kuvo-code/tmp/agent-protocols.*) ;;
    *) echo "Expected a private agent-protocols scratch folder" >&2; exit 1 ;;
esac
profile="$(cd "$(dirname "$0")" && pwd -P)/sandbox.sb"
mkdir -p "$scratch/home" "$scratch/work" "$scratch/tmp" "$scratch/claude" "$scratch/gemini" "$scratch/cursor-config"
cd "$scratch/work"
exec /usr/bin/env -i PATH=/opt/homebrew/bin:/usr/bin:/bin HOME="$scratch/home" \
    TMPDIR="$scratch/tmp" XDG_CONFIG_HOME="$scratch/home/.config" XDG_DATA_HOME="$scratch/home/.local/share" \
    XDG_CACHE_HOME="$scratch/home/.cache" XDG_STATE_HOME="$scratch/home/.local/state" \
    CLAUDE_CONFIG_DIR="$scratch/claude" CLAUDE_CODE_TMPDIR="$scratch/tmp" \
    DISABLE_AUTOUPDATER=1 DISABLE_TELEMETRY=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    GEMINI_HOME="$scratch/gemini" AGY_CLI_DISABLE_AUTO_UPDATE=1 AGY_ACP_FORCE_FILE_STORAGE=1 \
    SSH_CONNECTION="127.0.0.1 0 127.0.0.1 0" CURSOR_CONFIG_DIR="$scratch/cursor-config" \
    BROWSER=/usr/bin/false AGENT_CLI_CREDENTIAL_STORE=file SCRATCH="$scratch" \
    /usr/bin/sandbox-exec -D REAL_HOME="$real_home" -D SCRATCH="$scratch" -f "$profile" "$@"
