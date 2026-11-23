# Contributing to Kuvo Code

Kuvo Code does not accept outside pull requests or code contributions.
Bug reports and ideas are welcome as [GitHub issues](https://github.com/serhatandic/kuvo-code/issues).
Report security vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

This guide is for Kuvo maintainers and their coding sessions. See [CLAUDE.md](CLAUDE.md)
for session rules and [How Kuvo works](docs/architecture/how-kuvo-works.md) for the
current architecture. Keep the desktop native and put shared behavior in the box:

- **Native first.** Build the desktop UI in GPUI.
- **The engine stays opencode, unmodified.** Kuvo Code changes how working with
  an agent looks and feels. Changes to how the agent itself behaves (its tools,
  prompts or providers) belong in [opencode](https://github.com/anomalyco/opencode).
- **Few dependencies.** Lean on the system frameworks.

The desktop app lives in `gpui/`. Its box runs in `box/`, with Kuvo Helper on
macOS in `mac/helper/`.

## Maintainer development setup

Desktop development needs an Apple silicon Mac with macOS 26 or later, Xcode
27 or later, Rust, and the Bun version pinned by `scripts/build-box.sh`. In the
maintainer checkout, run `scripts/merge-setup.sh` once. From `gpui/`, build with
`../scripts/cargo-locked.sh build -p kuvo`.

From the repository root, `scripts/build-desktop-app.sh` packages the desktop app
and `scripts/build-dmg.sh` makes the disk image. See [GPUI local runs](gpui/README.md)
and [releases](docs/releases.md).

A release carries what every box runs, for boxes to update themselves with, signed with the
`KUVO_BOX_SIGNING_KEY` secret (an Ed25519 key; `scripts/set-box-key.sh` makes it once, keeps it in
your login keychain, uploads it and prints the public key to pin). The box builds it itself, from
its own sources, with no app built or run: `scripts/box-release.sh` writes the files
(`box/src/release/files.ts`: the keeper's from `box/src/keeper`, the engine's plugin from
`box/src/plugins`, the catalogs from the table in `box/src/shared/integrations`, the bots' words,
the link, the agents' bridge and the browser as built, and `box.json` with the engine pinned in
`box/src/shared/engine-pin.json`), packs them with each Mac bundle it's given (Kuvo Helper, from
`scripts/build-helper.sh`), and `scripts/sign-box.mjs` signs `boxes.json`, which names the pack.
`scripts/box-release.sh --files <version> <folder>` writes only the files, with no key, to look
at them. An app that carries the two files (`Contents/Resources/box-release`) offers them to the
box on its computer (`kuvo/box/update`). Boxes take only what a key
pinned in the box's link (`UPDATE_KEYS`, in `box/src`) signed; see `protocol/PROTOCOL.md`,
"Updates".

The desktop app as a Mac bundle, for trying it on a Mac: `scripts/build-desktop-app.sh` builds
`build/Kuvo.app` (the app, `kuvo-keys`, the icon, and the box's release with Kuvo Helper in its
pack), always signed ad hoc, and `scripts/check-desktop-app.sh` checks one (`--run`: a dry run
of a copy with another bundle ID, in a home of its own). With Kuvo Code's own bundle ID (the
default) it installs into the real `~/.kuvo-code` when opened, so open it only on a Mac or in an
account kept for that. See [release verification](docs/releases.md) for signing and publication status.

The keeper's files are plain files: edit them in `box/src/keeper`, where the bridge's tests run
`kuvo-wake` from. Where one names the engine's version it says `@ENGINE@`, which the release
writes the pinned version over. `kuvo-start`, `kuvo-launch` and `kuvo-setsid` end without a last
newline, as every release has had them; a newline added there is a change to the file like any
other (a box takes it as a new keeper). What's in
`box/src/shared` (the table, the bots' words, the engine's pin) is among what the agents' bridge
is stamped with, so after a change there `scripts/check-built.sh` asks for
`scripts/build-agents.sh` again (only its last line changes).

### Tests

Read [the testing guide](docs/testing.md) and run focused checks for the area you change and its callers:

- Desktop: `../scripts/cargo-locked.sh` from `gpui/`, with the package and test filter. Builds,
  tests, checks, Clippy and shots all take the one Rust slot, wherever the checkout is; use
  `scripts/heavy.sh rust <command>` for several steps in the same slot. Keep `CARGO_TARGET_DIR`
  per checkout: with a shared target, Cargo can reuse another checkout's executable, which keeps
  that checkout's `CARGO_MANIFEST_DIR`, so shots write to and tests read fixtures from the wrong
  place.
- Rust box: `scripts/cargo-box.sh` runs Cargo in `box/` with its pinned toolchain and its own
  build slot. Use `scripts/cargo-box.sh build --release --locked` for a release build.
- Box and bridge: `scripts/test-box.sh [box|bridge|all] [filter]`. Use
  `scripts/heavy.sh box scripts/test-linux.sh <ssh-host> <test file>` for a Linux
  bridge check. The scripts keep test data in scratch directories.
- Kuvo Helper: `scripts/heavy.sh mac-build swift test --package-path mac/helper`.
- Generated files: `BUN="$HOME/.bun/bin/bun" scripts/check-built.sh`.

The build scheduler gives desktop Rust (`rust`) and box Rust (`rust-box`) one slot
each, and gives `mac-build` and `box` two slots each. `scripts/heavy.sh status` shows their holders and queues.
Set `KUVO_HEAVY_WAIT=0` to exit 75 if a job would wait, or
`KUVO_HEAVY_PRIORITY=high` to move ahead of waiting normal-priority jobs.
`KUVO_HEAVY_LIMIT` sets the command's time limit (`90m`, `600s`, `2h` or
`0` for none). Keep demo and test runs in their own app copies and box homes.

#### Route scenarios

The bridge runs `box/test/fixtures/routes/*.json` (and `protocol/vectors/routes/features.json`) against the machine handler
(`contract.test.js`) and the Mac box handler (`contract-mac.test.js`). The
Mac scenarios use a captured 0.7 bot kit. The engine is a stand-in in both
runners. To add a scenario:

- Add it to the area's file: a `name` and `steps`. A step is `{"given": {<fixture>: {…}}}` (one
  each: `session`, `bot`, `wake`, `watch`, `pause`, `skill`, `trashedSkill`, `settings`, `key`, `credential`,
  `file`, `repository`, `write`, `ask`, `agentsBridge`, `peers`; bots before anything else), a request
  `{"send": "PATCH kuvo/bots/bot_ada", "body": {…}, "expect": {"status", "json"?, "type"?,
  "text"?}, "save": {"name": "bot.id"}}`, or `{"engineGot": {"method", "path", "json"}}`.
  `{"agentsBridge": {"groups": true}}` runs the coding agents' bridge the app ships for real on
  the machine's side (with Bun: `KUVO_TEST_BUN`, or `bun` on your PATH), for its group chats;
  `{"peers": {"boxes", "bots"}}` then tells it who your other computers and their bots are, as the
  box's link does.
- `json` names only what matters. `"<string>"`, `"<number>"`, `"<bool>"`, `"<object>"`,
  `"<array>"`, `"<any>"`, `"<absent>"` stand for volatile values; lists match in order, or with
  `{"<includes>": […]}`, `{"<excludes>": […]}`, `{"<length>": n}`; `{"<contains>": "…"}` in a
  string. `"whole": true` beside `json` names the answer whole: a field it doesn't name fails it,
  so both boxes are held to the very same answer (`alike.json`). `null` matches null or left out.
  `{{name}}` fills in what was saved, the box's places
  (`chats`, `skills`, `work`, `keys`) and `{{now+3600}}`. Both runners match by
  `routes/match/cases.json`, and the bridge's checks each scenario's shape.
- Where the boxes differ on purpose, say so: `"only": "mac"` (or `"machine"`) on a scenario or a
  step, or `"expect": {"mac": {…}, "machine": {…}}`, each with a `"why"`.
- Every answer, whole, is also held to its route's shape: `box/src/shared/routes`, one
  file an area, with each field an answer can have (one ending in `?` may be left out) and its
  type, which the box's code takes its types from too. A new route, or a new field in an answer,
  goes there first, with a one-line comment above the route saying what it's for where its path
  doesn't: routes are documented in their definition, and `scripts/check-built.sh --build` writes
  `protocol/ROUTES.md` for a readable list (not kept in git). Add to `protocol/PROTOCOL.md` only a
  rule a client must know that the route can't say, in a few lines. `./scripts/route-shapes.sh`
  writes `routes/shapes/answers.json` for both runners, and `routes/features.json` from the box's
  advertised capabilities.
  `test/route-shapes.test.ts` checks the generated files. The runners check
  answers against `routes/shapes/cases.json`. A field that can be null is sent as `null`, with the field present, and a refusal
  is JSON, `{}` at least.
- Run the runners. `./scripts/route-vectors.sh` refreshes the integration and
  MCP catalogs in `routes/box` from `box/src/shared/integrations`.

### Project layout

| Path | What's there |
| --- | --- |
| `box/` | The box (TypeScript): its link, built to `kuvo-link.js`, and the agents bridge (Claude Code, Codex, Cursor, Grok and Antigravity in the engine's API) and the browser's keeper and tools, built to `dist/kuvo-agents.js` and `dist/kuvo-browser.js` (its `README.md`); the keeper's shell files (`src/keeper`), the engine's plugin (`src/plugins`) and what writes its release's files (`src/release`); its tests in `test/`, and the bridge's, on Node, in `bridge-tests/` |
| `mac/helper/` | Kuvo Helper (Swift, macOS only): the signed program this Mac's box runs under, and `kuvo-keys`, the one-time hand-over of 0.7's Keychain items to it. Built on its own by `scripts/build-helper.sh`; its tests are `swift test` there |
| `protocol/` | How boxes and clients talk (`PROTOCOL.md`: only what a route can't say), and the vectors every side is held to (`vectors/`) |
| `gpui/` | The GPUI app, in Rust (its `README.md`) |
| `scripts/` | Builds, checks and the test runners |
| `Licenses/` | License texts for code copied into this repository |

### The box browser

Each box's browser is Chrome for Testing, the full browser run in Chrome's own headless mode
(the separate headless shell is one sites take for a bot), with its libraries and fonts, in
`~/.kuvo-code/browser` there. The box sets it up on macOS and Linux, with or without a client
(`box/src/link/browser`, PROTOCOL.md's "The browser"): the build pinned in
`linux.json` and `mac.json` there (`scripts/browser-runtime.py` writes them; on a Mac machine,
Chrome for Testing as it comes, with a `chrome` launcher, as it won't run through a link),
downloaded only from Google's Chrome for Testing and Rocky Linux over HTTPS, each file checked
against its size and SHA-256 before it's unpacked (by the box itself: no `unzip`, `xz` or `rpm`
on the machine). The Mac only shows how it goes (`kuvo/browser`). Its tests serve stand-ins
(`box/bridge-tests/browser-source-stand-in.js`): never the real sources, never a real `~/.kuvo-code`.
One script,
`box/dist/kuvo-browser.js`, built from `box/src/browser`, runs with the
engine's own runtime in three ways:

- **host**, the keeper: it starts Chromium, holds it over a pipe, and serves the DevTools
  protocol to everyone else on a Unix socket in `~/.kuvo-code/run`, to each connection that
  first gives the box's socket key (`run/socket.key`, which an agent's command in the sandbox
  can't read; one that doesn't is told to update Kuvo Code and closed), with a few messages of its own (`Kuvo.*`): the page in use, who's browsing where, what agents ask you for, and which
  page each conversation works in. Only one runs at a time. When an update puts a newer build
  in `current`, it stops once nothing uses the browser, keeping the open pages, so the next
  start runs the new one.
- **mcp**, the agent's browser tools for the machine's engine, talking to the keeper.
- **connect**, the retained direct connector to the keeper. Current desktops use the box browser-view route where the transport supports it.

New clients can open `kuvo/browser/view` through serve (its Basic password) or a paired relay
session. The box translates the keeper's newline stream to JSON facts and binary JPEG frames.
The keeper owns page sizes, captures once per size/quality class and allows two unacknowledged
pictures per viewer. Raw CDP metrics and screencast calls obey the same rule: viewers cannot
resize an agent's working page. The wire format and message budget are in PROTOCOL.md,
“The browser”. Test with `browser-live-view.test.js` and
`browser-view-socket.test.js`: Chrome is stood in at the pipe; no real profile is used.

The first two, finding no browser, have the box set it up and wait for it (`kuvo-link.js browser
install`); a browser that doesn't start because its build isn't whole is set up again once.

The box reads browser asks from `run/browser-asks.json` and handles answers through
`box/src/link/areas/asks.ts`. It can send `Kuvo.answer` to the keeper with the desktop closed.

On a Mac, Chrome's editing keys come from its window's key bindings, which a key sent over DevTools
never reaches: the keeper names what each does (`keyForHere`), so clients send the same keys to
every machine.

Its source is `box/src/browser/kuvo-browser.js`; the scripts it runs in pages
(`page.ts`, with `page/` and browser-use's `buildDomTree.js`), the agents' tools (`tools.ts`) and
what they're told (`instructions.ts`) are filled in by `./scripts/build-browser.sh`, which you run
and commit after changing them (the box suite checks it too). It
writes `KEEPER` too, a hash of the source, so a copy built from changed sources replaces the keeper
that's running. To update the historical browser tools fixture, see
`box/bridge-tests/browser-tools-built.test.js`.

### Compatibility with released Macs

Code that's there only for released Macs (PROTOCOL.md's "For released Macs": Kuvo Code 0.5 to
0.7.2, and 0.8.0-beta.1) carries a marker where it is, on every side, `COMPAT(<version> <item>):
what it does. Goes …` (`#` in a shell script), whose item that section names. `scripts/compat.sh
[0.7 | beta.1]` lists them, so a version's paths go together, and `box/bridge-tests/compat.test.js`
fails on an item without a marker or a marker without an item. Mark what you add for them the same
way. 1.0 keeps two kinds (that section, "What 1.0 keeps of this"): what keeps a box right
while an older Mac still connects, and what an earlier version left, taken in once; three more
wait for #717's next part. `protocol/compat-kinds.txt` names each item's kind, which the test holds
its markers' last sentence to (except in files shipped byte for byte in the box release).

### The icon

`mac/AppIcon.icon` is the Icon Composer source for the desktop bundle and Kuvo
Helper. `scripts/icon.py` generates it and the README icon from artwork. It
uses numpy, OpenCV and Xcode.

### The coding agents

Claude Code, Codex, Cursor, Grok and Antigravity run beside the engine, the way
[T3 Code](https://github.com/pingdotgg/t3code) runs them: each as its maker built it, signed in
as you. `box/src/agents` builds the agents bridge. The keeper runs it with the engine's
Bun, separately from both the engine and the resident link. The bridge serves
the engine-shaped session, message, event, permission, question, model and
sign-in API. The box routes sessions by their agent metadata (`kuvoAgent`) and
retains `kuvo/agents/api/…` for compatible readers.

- **Claude Code**: the Claude Agent SDK, with your `claude` and its sign-in.
- **Codex**: `codex app-server`, JSON-RPC over stdio.
- **Cursor**: Cursor's SDK, signed in through it; its sign-in and agents in `agents/cursor`.
- **Grok** and **Antigravity**: the Agent Client Protocol (`acp.ts`); Antigravity's agent is
  downloaded from Google, checked against its pinned SHA-256, into `agents/antigravity`.

The SDKs are installed from npm the first time each is needed (`runtime/<agent>`), at the
versions in the agents' files, and aren't part of the app. After changing `box/src/agents`, run
`./scripts/build-agents.sh` and commit `box/dist/kuvo-agents.js`, the bundle a box's release
carries. Use `scripts/build-agents.sh --check` to verify it is current.

### Dependencies

The desktop's Rust dependencies and asset credits are in `gpui/LICENSES-DEPS.md`.
Sparkle is pinned by `scripts/sparkle.sh`. Run
`./scripts/third-party-licenses.sh` after changes to included third-party code
or assets and commit its updated notices.

## Maintainer workflow

1. Work on a branch in a separate worktree and keep commits focused. Follow
   [CLAUDE.md](CLAUDE.md) for design decisions, tests, and handover.
2. Run focused local checks. CI is paused for session work. AI review passes
   run only when requested.
3. For changes people notice, add a line under Unreleased in [CHANGELOG.md](CHANGELOG.md)
   and update the matching site docs in a separate local commit. See
   [the docs map](docs/docs-map.md).
4. Commit tested work with short, factual messages. Keep credentials, personal
   paths, session metadata and AI attribution out of commits. Follow the lead's
   instructions for local integration. Sessions never push. The lead pushes
   only on the user's word.

### What branches don't edit by hand

Many branches change the box at once, so what each would bump or count is worked out instead,
and the lists they add to merge line by line:

- **The bridge's version** (`BRIDGE_VERSION`, `box/src/link/version.ts`): never bump
  it. `scripts/build-box.sh` writes in a hash of the link as built, so it changes with every
  change (a machine's `setup` replaces a running bridge whose version isn't its own), and nothing
  compares it: clients check `features`. A note of when something came in names its feature, not
  a bridge version.
- **What a box answers** (`FEATURES` in `box/src/shared/features.ts`; `MAC_FEATURES`
  and `MACHINE_FEATURES` in `shared/routes/mac.ts`): add a name on a line of its own, in order
  (`LC_ALL=C sort`). Names two branches add in different places merge cleanly; git merges
  `features.ts` by union (`.gitattributes`), so two names added side by side both stay too,
  though maybe out of order: the box's tests (`test/features.test.ts`) and `route-shapes.sh`
  refuse a list out of order or a name twice, so put them in order. Change nothing else in
  `features.ts` in a branch that adds a name, and don't remove a name next to one another branch
  adds (union keeps it; the bridge's tests check every name has its route). A name for what only
  a machine answers goes in `MACHINE_FEATURES` too, and one only a Mac answers in `MAC_FEATURES`
  instead: this Mac's box claims every other name (`macFeatures`).
- **Other lists a feature adds to** are one entry a line, in order, so they merge the same way
  (without union): the bridge's test of each name's route (`routes` in
  `box/bridge-tests/bridge.test.js`, whose order it checks), `MAC_FEATURES` and `MACHINE_FEATURES`.
- **Compatibility requirements** live in `protocol/gate-required.txt`. Keep entries sorted and
  check removals after a merge. The TypeScript box reads this list and `kuvo_protocol` generates
  the Rust constant from it. The box decides whether to refuse a request.
- **A branch from before these were one a line** (#396) merges main into itself first, where the
  old lines conflict as they always did, and puts its names back one a line, in order. Merged the
  other way, into main, `features.ts`'s union takes its old line whole beside the new ones: it
  still builds, with names twice, and only the tests and `route-shapes.sh` catch it.
- **What's written from those**: `./scripts/route-shapes.sh` writes `routes/shapes/answers.json`
  and `routes/features.json` (each box's `kuvo/info` features, which both route runners check);
  `./scripts/build-box.sh`, `./scripts/build-agents.sh` and `./scripts/build-browser.sh` write
  `kuvo-link.js`, `kuvo-agents.js` and `kuvo-browser.js` (with its keeper's version, `KEEPER`, a
  hash: never bump it). Never edit them. They stay in git, because what builds the app, the
  release takes them as they are,
  with no Bun. Two branches that both built one would always conflict on it, and neither side is
  right after a merge, so git keeps the side checked out (`merge=kuvo-built` in `.gitattributes`;
  in a rebase or cherry-pick that's the branch replayed onto, not the commit replayed) and goes
  on, saying so for each. **After a merge, rebase or cherry-pick, run `scripts/check-built.sh`**
  (each script's `--check`): it fails on one that isn't what its sources build, and
  `scripts/check-built.sh --build` builds all four again, to commit. The desktop
  bundle build runs the check too.
- **Setting up the merge driver**: git needs the driver named in each clone's own config, once:
  run `scripts/merge-setup.sh` after cloning. It writes `merge.kuvo-built` into `.git/config`,
  which every worktree of that clone shares, so a new worktree needs nothing more. A clone without
  it merges these files as text, and they conflict as they always did: then take either side and
  build again.
- **`CHANGELOG.md`**: add lines; it's merged by union too.

## License

Kuvo Code is source-available for noncommercial use under the
[PolyForm Noncommercial License](LICENSE).
