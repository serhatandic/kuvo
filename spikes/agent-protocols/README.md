# Agent protocol spike

Session 0.3 of the [Rust box plan](../../docs/architecture/rust-box.md#2-agents-without-a-javascript-runtime).
The Rust executable launches the real Claude CLI, frames its requests and replies, and passes
its output to the fixture driver. The same driver exercises the pinned SDK through
`query()` and captures the actual CLI traffic through `spawnClaudeCodeProcess`. The mock
Anthropic Messages and MCP endpoints listen only on loopback. Their scripted responses
exercise the CLI and the SDK without a vendor account or a paid request.

## Reproduce on the Mini

Use a fresh, private scratch directory for the executables, dependencies, test homes and
traces. Run the commands below from the repository root with Node, npm and Cargo on PATH.
Point `opencode_bin` at the installed engine executable. The example is for arm64 macOS.

```sh
umask 077
scratch=$(mktemp -d "$HOME/.kuvo-code/tmp/agent-protocols.XXXXXXXX")
spike=spikes/agent-protocols
mkdir -p "$scratch/home" "$scratch/bin" "$scratch/sdk" "$scratch/cursor" "$scratch/cargo"
cp -L "$(command -v claude)" "$scratch/bin/claude"
cp -L "$(command -v agy)" "$scratch/bin/agy"
cp -L "$opencode_bin" "$scratch/bin/opencode"
cp "$spike"/*.mjs box/src/agents/agents/cursor-worker.ts "$scratch/"

env -i HOME="$scratch/home" PATH=/opt/homebrew/bin:/usr/bin:/bin \
  npm --cache "$scratch/npm-cache" --prefix "$scratch/sdk" install --ignore-scripts \
    --omit=optional --no-audit --no-fund @anthropic-ai/claude-agent-sdk@0.3.303 \
    @cursor/sdk@1.0.35 @cursor/sdk-darwin-arm64@1.0.35

# This is the arm64 package URL supplied by Cursor's installer for the tested CLI build.
curl -fsSL https://downloads.cursor.com/lab/2026.11.22-e9042b2/darwin/arm64/agent-cli-package.tar.gz \
  -o "$scratch/cursor.tar.gz"
tar --strip-components=1 -xzf "$scratch/cursor.tar.gz" -C "$scratch/cursor"

CARGO_HOME="$scratch/cargo" CARGO_TARGET_DIR="$scratch/target" \
  scripts/cargo-locked.sh build --locked --manifest-path "$spike/Cargo.toml"
cp "$scratch/target/debug/claude-wire-spike" "$scratch/bin/"

for peer in unsigned unsigned-sdk sdk rust; do
  sh "$spike/safe-run.sh" "$scratch" node "$scratch/claude-probe.mjs" "$peer" || exit
done
for probe in cursor worker agy agy-legacy agy-login; do
  sh "$spike/safe-run.sh" "$scratch" node "$scratch/other-probes.mjs" "$probe" || exit
done
```

The launcher clears inherited variables and overrides HOME, XDG and the agent config
directories. The Seatbelt profile restricts writes to scratch, denies access to the real
home and Keychain, and permits only loopback TCP. Run vendor code through that launcher.
The fixture key and the legacy Antigravity token are placeholders. The Antigravity login
probe closes stdin without entering a code or opening the printed URL.

The Claude assertions check the observable results of both clients. The
`*-claude.json` traces contain unfiltered Claude frames, and the SDK
trace also contains the public iterator frames. Compare by event and request contents
because timestamps, UUIDs, paths and task IDs vary. The recovery scenario repeats
`initialize` on the same live CLI with a pending permission request, responds to the
redelivered request ID, and waits for both message UUIDs. A second reinitialization checks that the CLI cancels an
unanswered hook. Holder transport and bridge
replacement are tested separately in the product.
