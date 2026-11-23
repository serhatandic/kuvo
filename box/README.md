# The box

What runs on each of your computers beside the engine, the same on this Mac and on a Linux machine.
It's programs of ours, built from this one package:

- **the link** (`src/link`), built to `box/dist/kuvo-link.js`,
  shipped in each box release. The keeper starts `serve --resident` for the
  routes, relay, peer mail, setup, notices and fleet updates.
- **the agents bridge** (`src/agents`), built to `box/dist/kuvo-agents.js`, which a box's
  release carries and each box runs with the engine's Bun: the coding agents, group chats and goals;
- **the browser's keeper and tools** (`src/browser`), built to `box/dist/kuvo-browser.js`,
  which each box runs with the engine's Bun: the browser agents use there, its live view, and the
  agents' browser tools.

The box release installs the three bundles in `bin/`.

**Its release** (protocol/PROTOCOL.md, "Updates") is built from this package alone, with no app
(`scripts/box-release.sh`; `--files <version> <folder>` writes only the files, to look at them):

- `src/release/files.ts` writes every file a version puts in a box's `~/.kuvo-code`, and
  `box.json`, which names them;
- `src/keeper` holds the keeper's shell files as plain files, the one copy: `kuvo-start`,
  `kuvo-launch`, `kuvo-setsid`, `kuvo-wake`, `kuvo-npx` and `runtime/node`. Where one names the
  engine's version it says `@ENGINE@`, written over as the release is built. The first three end
  without a last newline, as every release has had them;
- `src/plugins/kuvo-code.js` is the engine's plugin;
- `src/shared/engine-pin.json` is the engine a version runs: its version, and each system's build
  with npm's integrity for it;
- `src/shared/integrations` is the one table of integrations (`table.ts`) and popular MCP servers
  (`servers.ts`), and `catalogs.ts` writes the three catalogs a box reads from it;
- `src/shared/bot-words.json` is what a box makes its bots with and tells them.

The keeper supervises the link, engine and agents bridge separately. Kuvo
Helper provides native supervision and Keychain access on macOS. The resident
link stays up when the engine or bridge is down. Older release packs retain
split launch for rollback and 0.7 compatibility.

See [the protocol](../protocol/PROTOCOL.md),
[resident launch](../docs/architecture/one-client-process.md) and
[bridge ownership](docs/bridge-ownership.md).

## Building

Run `scripts/build-built.sh` from the repository root or `bun run build` from `box/`.
Use the Bun version in `.bun-version`. The build writes the three bundles to `box/dist/`
and generates the route vectors and `protocol/ROUTES.md`. Commit changes to the sources.
The builders stamp the link's `BRIDGE_VERSION` and the browser's `KEEPER` from their contents.

`bun install --frozen-lockfile` here, then `bun run check`, type-checks the link with Node's
types and the agents bridge with Bun's.

From the repo root, run focused checks through
`scripts/test-box.sh [box|bridge|all] [filter]`. Check box changes on Linux
too. The link bundle uses Node APIs so the Node bridge fixtures can import it,
and installed boxes run it with the engine's Bun. See [testing](../docs/testing.md).

## Where things go

`src/link` is `kuvo-link.js`, every file of it type-checked (`bun run check`, and a test that none
says `@ts-nocheck`); `main.ts` is the command line, and each route's answer has a shape in
`src/shared/routes`, which both boxes are held to:

```
src/shared/      what both programs and the contract suite read: run/ records, features, documents,
                 mail between boxes, group chats' seat mail, link ⇄ agents bridge, notices, route shapes;
                 platform.ts, where macOS and Linux differ (processes, the folders a Mac asks about),
                 each side tested
src/link/
  main.ts        the command line (run, serve, setup, pair, leave, update, box …, selftest)
  home.ts state.ts files.ts   the box's folders, link.json, writing files
  crypto/        keys, Noise sessions and mail, signing
  relay/         frames, the host's sessions, the relay connection
  engine/        asking the engine and the agents bridge, its sessions, its events
  peers/         the other boxes: who they are, their agents and this box's, "&" names, mail,
                 group chats' seats
  documents/     what boxes copy between them: their format, the work of taking and giving them,
                 sending them
  letters/       agents' messages to agents on other boxes: the outbox, and those that come here
  phones.ts      paired device grants and legacy phone contracts
  bots/ personalities/   bots, and the personalities library (yours, and others' copies taken)
  server/        the route table (a path to its area, nothing else), forwarding to the engine and
                 who a prompt names with "&", resident and compatibility servers
  mac/           this Mac's box: its link on the relay, what its app asks of it, its notices, its
                 part among your other computers
  areas/         one module per route family: projects, worktrees, skills, integrations, wakes,
                 groups, config, accounts, asks, artifacts, folders, plugins…
  notices/ updates/ host-interface.ts keeper.ts
  compat/        what's only for released Macs, where it's a whole thing
src/agents/
  main.ts server.ts   the bridge: the engine's own API for the app, over HTTP
  sessions.ts store.ts turn.ts …   its sessions, kept, and a turn's run
  agents/        each coding agent: Claude Code, Codex, Cursor, Grok and Antigravity (the last
                 two through acp.ts, the Agent Client Protocol)
  groups/        group chats: their rules, store, runner, and peers on other boxes
  artifacts/     what agents made, for the Artifacts view
```
