# The box in Rust

Design, 23 November 2026. Source references are at `971e89b71`. The box (`box/src`, about
184,000 lines of TypeScript run by the engine's Bun) is rewritten in Rust as one program for macOS,
Linux and Windows, both native and WSL2. The Rust box speaks today's protocol and reads today's
files, so the normal fleet update can switch computers one at a time. Computers on either box keep
working together while that happens.

Why: the box's own memory and CPU use (the resident alone is 240 MiB PSS when idle, see
[small computer](small-computer.md)). Rust is also the desktop's language, so the two can share
types, rules and test vectors. Native Windows needs process ownership, named pipes, locked files
and durability rules that the [Windows trial](windows.md) showed Bun handles badly. The result is
one small program on each computer.

## 1. Shape

**Where it lives.** `box/` becomes a Cargo workspace (`box/Cargo.toml`, `box/crates/*`) with its
own `Cargo.lock` and the same toolchain file as `gpui/`. The TypeScript stays in `box/src` until
it's removed, so the folder never moves. It isn't a member of the `gpui` workspace. That workspace
pins Zed's GPUI, and joining it would make every box build resolve and compile GPUI. Two small
crates are shared by path. `gpui/kuvo_box` depends on `../box/crates/kuvo_protocol` and
`kuvo_wire`. Both are sans-IO: no runtime, and only serde, serde_json and the crypto crates
`kuvo_box` already uses. That keeps them resolvable under either lockfile. A change to them also
runs `kuvo_box`'s tests. Box crates get their own `scripts/heavy.sh` lane (`rust-box`) so parallel
box sessions don't queue behind desktop builds.

**Crates** (one per area, so parallel sessions edit different crates):

| Crate | Holds |
|---|---|
| `kuvo_protocol` | Route types and the shape checker, generated from the route shapes (section 3); `FEATURES`; refusal shapes; format-version constants. Shared with the desktop. |
| `kuvo_wire` | Relay frames, link frames, Noise IK and K (`Noise_*_25519_AESGCM_SHA256`), keys, the peer-mail v1 envelope, signed documents (Ed25519), canonical JSON for digests. Moved out of `kuvo_box/src/link/{noise,frames,keys}.rs`, then given the responder side and K. Shared. |
| `kuvo_os` | Platform facts and paths; spawning, process groups and Job Objects; PID plus start identity; local IPC (Unix sockets, named pipes restricted to the user's SID); file watching; the sandbox launchers. |
| `kuvo_store` | Home layout, versioned files, atomic writes with a durability rule for each OS, locks, the layout gate, data declarations, SQLite. |
| `kuvo_server` | One dispatcher; local HTTP with its password and `run/serve.json`; the route-version header; the line (`kuvo/line`); the SSH pipe (`route`, `route-to`); paired relay sessions. |
| `kuvo_setup` | Lease, leadership and takeover, membership, shared saves, devices and pairing, adding, unlinking and wiping a computer, backups. |
| `kuvo_peers` | Relay client, mail and outbox, reach, merged lists, `ownerOf`, moves and handoffs, receipts, letters, notices. |
| `kuvo_engine` | OpenCode on demand, `api/…` passthrough (WebSocket and PTY included), sessions, queues, send-once receipts, asks, search. |
| `kuvo_agents` | Holders, roster, accounts and sign-in, the tools relay and MCP, one module per agent, subagents, reviews, handover. |
| `kuvo_work` | Bots, wake-ups and watches, the Work Queue, group chats and seats, autopilot. |
| `kuvo_services` | Usage, limits and Insights, Library, artifacts, marketplace, integrations, skills, settings pages, vault, files, Git, Changes, worktrees, the browser keeper. |
| `kuvo_keeper` | Supervision, release selection and rollback, fleet updates, install and uninstall. |
| `kuvo-box` (bin) | `main`: the modes below. Each crate registers its own routes with one call. |

**Libraries.** tokio (two worker threads; SQLite, fsync and Git go to the blocking pool, so the
request loop never waits); hyper 1 for serving, reqwest with rustls for fetching, and
tokio-tungstenite; x25519-dalek, aes-gcm, sha2, hkdf and ed25519-dalek; rusqlite (bundled);
notify, with polling for WSL's Windows drives; tokio's named pipes and `windows-sys` for Job
Objects, ACLs and start identity; rustix on Unix; the `landlock` crate; `rmcp` for MCP;
`agent-client-protocol` for ACP; jiff for time zones; pulldown-cmark; tracing. Git stays the `git`
command, as today. No PTY crate: terminals are the engine's (`api/pty`), and the box passes them on.

**Processes.** The single binary `kuvo-box` has these modes:

- `serve`: the resident, one per computer. It runs the link, the server, setup and peers, **the
  agents bridge**, services, wake-ups, and engine and browser supervision. The bridge becomes part
  of the resident. Holders already keep agents alive across a restart, so the separate bridge
  process, its HTTP hop, `BRIDGE_VERSION` and the bridge handover all go. `kuvo/agents/…` keeps its
  paths, now answered in-process.
- `hold`: one per agent process, speaking today's `hold/1`.
- `keep`: the keeper.
- `route` and `route-to`: the SSH entries.
- `mcp`: the stdio MCP forwarder that agents start, replacing `kuvo-browser.js mcp` and the
  `bun -e` tool forwarder.
- `fence-exec`: the Linux sandbox shim.
- `install`: installation.

Separate, unchanged programs: OpenCode, unmodified and started on demand; Chrome for Testing; the
agents' own CLIs; Kuvo Helper on a Mac. The relay service (`relay/`) isn't part of this rewrite.

**Budget:** the resident at most 40 MiB PSS idle, a holder at most 3 MiB, and the whole box
without the engine at most 50 MiB. The binary is at most 20 MB stripped. Both are checked with
`scripts/small-computer-check.sh`.

## 2. Agents without a JavaScript runtime

| Agent | Today (`box/src/agents/agents/`) | Rust box decision |
|---|---|---|
| Codex | `codex app-server`, JSON-RPC on stdio | Same protocol in Rust. |
| Grok | ACP on stdio, plus `_x.ai/*` extensions | `agent-client-protocol`, with the extensions as ext methods. |
| Antigravity | ACP through `agy acp` or `agy_acp_server.par` for older sign-ins. Follows the agent's transcript file. | ACP in Rust. Keep both launches until a valid legacy profile can use `agy acp` without signing in again. |
| Claude Code | `@anthropic-ai/claude-agent-sdk` drives the user's Claude CLI. | Speak stream-json and the CLI's control protocol directly. No tested feature needs the SDK sidecar. |
| Cursor | `@cursor/sdk`, with `cursor-worker.ts` already running it under a holder. | Run the pinned SDK in the engine's Bun sidecar. The headless CLI's event stream lacks parity. |

Session 0.3 ran the Mini's Claude Code 2.1.330 and Antigravity 1.5.14, plus a scratch
installation of Cursor CLI 2026.11.22-e9042b2. All ran with private homes, Keychain access
blocked and vendor networking disabled. Claude's real CLI ran once from Rust and once
through SDK 0.3.303 with the same scripted loopback model and MCP endpoints. The
[spike crate and reproduction steps](../../spikes/agent-protocols/README.md) record both
the SDK's outbound wire and what its public iterator returns.

### Claude Code

| What Kuvo uses | Result |
|---|---|
| Initialize and model catalog | **Works.** `initialize` returns models and agents without a turn. `get_context_usage` with `detail: "summary"` returns `maxTokens`. |
| System prompt, custom agents, skills and settings | **Works with two configuration paths.** The preset uses the CLI's default prompt plus `appendSystemPrompt` in `initialize`, alongside `agents`, hook registration and `forwardSubagentText`. The loopback model received the append. `--plugin-dir-no-mcp` loaded the local plugin and skipped its MCP server. `--settings` supplied the memory directory, skill overrides and sandbox override. |
| PreToolUse, permission prompts, questions and Plan exit | **Works.** The CLI sent `hook_callback` and `can_use_tool`. A block prevented a write, an approval allowed a read, updated input answered `AskUserQuestion`, and a denied `ExitPlanMode` kept Plan mode. Rust replies with the request ID and keeps tool-use IDs distinct from callback IDs. Keep the SDK's callback cancellation and permission deduplication semantics. SDK field mappings such as `permission_suggestions` to `suggestions` belong at this boundary. |
| Model, effort and permission mode between turns | **Works.** `set_model`, `apply_flag_settings` (`effortLevel`), and `set_permission_mode` changed the live process. The next model request used Opus at high effort in Plan mode. A null `effortLevel` cleared the override. From Sonnet, omitting `model` returned to the account catalog’s resolved default (Opus on the fixture). |
| MCP | **Works.** `mcp_set_servers` connected and removed both an HTTP server with headers and a stdio server with command, args and environment. Server status showed both fixture tool lists. Keep connection details on the pipe. Kuvo uses no in-process SDK MCP servers. |
| Streaming, messages, steering and interruption | **Works.** Partial text, assistant frames, tool results, base64 image blocks and `priority: "next"` messages passed. Turn results can acknowledge several user UUIDs. `interrupt` during a streamed reply produced `aborted_streaming`. Raw stdio also exposes control traffic that the SDK consumes internally. |
| Transcript resume, live reconnection and clear | **Works at the CLI.** `--session-id` then `--resume` reused the transcript. A second `initialize` on an open permission ask returned the same request in `response.pending_permission_requests` (beside the inner payload). Rust answered the redelivery and both queued UUIDs completed. The SDK handled that repeat with one permission callback. Reinitializing with an unanswered hook sent `control_cancel_request` for the old callback. `/clear` emitted `conversation_reset` and the next turn used the new session ID. The actual holder handoff still needs mixed-box coverage. |
| Usage, limits and compaction | **Works for tested events.** Assistant and result usage, cumulative cost, context window, `api_retry` on a scripted HTTP 429, and `/compact` status and `compact_boundary` came over the same wire. Automatic compaction's environment and threshold policy stay in the box port. Subscription `rate_limit_event`, OAuth renewal and account-specific limits still need a signed-in test account. |
| Subagents and background work | **Works.** The custom agent used its selected model and forwarded parent-linked assistant text. The CLI emitted task lifecycle and background-list events. A background shell returned `get_task_output` and reported `stopped` after `stop_task`. |
| One-shot generation, auth and process lifecycle | **Works for local paths.** The tool-free, one-turn Haiku request returned a result with session persistence and hooks disabled. A fresh unsigned CLI initialized and returned a sign-in error on its first prompt. Match the SDK's `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` to receive session-state frames and handle process exit and stderr alongside result frames. Signed-in auth events, credential refresh and provider-specific failures need a private account. |

The adapter also handles local-command output, notices, hook and plugin failures, model refusals,
memory recall, and session-scoped permission updates. Keep those frame and reply fields from the
SDK reference. The fixture did not force every diagnostic or permission suggestion variant.

### Cursor

The signed-out headless invocation exited 1 with an authentication error on stderr and
empty stdout. It supports resume, model overrides and partial text, but its documented
print stream suppresses thinking. Its public stream also lacks the SDK's local agent
store and run contract, nested task updates and per-turn usage shape that Kuvo consumes.
A successful CLI stream, SDK turn and model/account responses need a signed-in test
account. [Cursor CLI output format](https://prod.cursor.com/docs/cli/reference/output-format)

OpenCode 2.2.6's embedded Bun 1.4.4 (`BUN_BE_BUN=1`) ran the existing worker with
`@cursor/sdk` 1.0.35, created an agent, reopened its JSONL store by ID, and returned
structured errors for missing agents and offline sends. Use this line protocol under a
holder. Install the pinned SDK on first use as in `deps.ts`.

| Direction | Frame |
|---|---|
| Request | `{id, method, params}` on one line. `create {options, resume?}` returns `{agentId}` or a null ID for a missing resume. Store options carry `local.store.root`. |
| Request | `send {agent, run, message, options}`, `cancel {run}`, and `close {agent}` acknowledge with `{}`. `message` can include text and images. |
| Notification | `delta {run, update}` forwards the SDK update. `ended {run, result}` or `ended {run, error}` settles the run. Subscribe by run ID before sending or replaying holder lines. |
| Reply and error | `{id, result}` or `{id, error: {code, message, data}}`. Preserve the SDK's error name, code and status for sign-in and limit handling. |

The worker covers turns. Phase 4.5 also needs sidecar calls for the SDK-backed browser login
and cancellation, credential-store access, account lookup and model catalog. Preserve separate
account and held-worker state. Signed-in testing must cover those SDK calls, successful streams,
active cancellation, and holder replay. Cursor remains the only SDK sidecar in this design.

### Antigravity

`agy acp --gemini-dir … --app-data-dir antigravity-acp` answered ACP protocol 1. It
advertised load/resume, image prompts, system-prompt append and four auth methods
(`oauth-personal`, `oauth-business`, `gemini-api-key`, and `agent-platform`).
An unsigned `session/new` returned `-32000 Authentication required`. Under the adapter's
SSH-style environment, ACP `authenticate` directed the caller to `agy acp --login`.
That command printed a Google URL and asked for a code. Kuvo currently offers personal OAuth.

A profile with a synthetic legacy `acp_token.json` still returned authentication required.
This cannot establish whether `agy` accepts a valid 1.3.0 token. Keep the legacy launch
for existing sign-ins and held sessions. Verify personal OAuth completion, authenticated
ACP sessions and models, and a valid legacy-only profile before deleting the fallback.
The other advertised methods still need their own isolated accounts or credentials. Neither launch
needs a JavaScript sidecar.

## 3. The contract both boxes meet

**Route shapes: TypeScript stays the source until the TypeScript box is removed.** The shapes are
already exported as data (`protocol/vectors/routes/shapes/answers.json`, from
`scripts/route-shapes.sh`). `kuvo_protocol` generates Rust types and the checker from that file
into a built, never-hand-edited `routes.rs`. Its checker is held to `shapes/cases.json`, like the
JavaScript one. The TypeScript box remains the reference and keeps getting fixes, so a source of
truth anywhere else would mean two places to edit for months. At removal (session 9.4) this flips:
the shapes become hand-written Rust, and `answers.json` is generated from them for the
conformance suite and other clients.

**Conformance suite** (`protocol/conformance/`, `node --test`, run against any binary named by
`KUVO_BOX_BIN`). Today the route scenarios (199 scenarios, 436 requests) call the bundle's
`engineHandler` in-process, as 118 of the 221 bridge tests do. Session 0.1 adds a box-under-test
adapter. It starts `serve` in a scratch home with stand-in engine and relay addresses, finds it
through `serve.json`, and speaks HTTP. `given` steps write files or configure the stand-ins, never
call into the box. The TypeScript box passes the suite first. Then come the process-level tests
already written as such or worth converting: two boxes, setup, devices, moves, Work Queue across
computers, seats, the SSH pipe and route, serve and resident, fleet switch, keeper, updates, and
`kuvo_box`'s `real_box` tests. Tests of TypeScript internals, the Mac app or retired paths stay
with the TypeScript box and go with it. **Added:**

- byte vectors for each peer-mail kind;
- `hold/1` transcripts, with adoption in both directions;
- canonical-JSON digest vectors, for forwarded decisions;
- a **golden home**: a folder at the current layout with every stored family. Each box reads the
  other's writes.
- Seatbelt profile and Landlock policy text from the same inputs;
- the keeper's records (`serve.json`, `link.pid`, demand markers).

The relay, link and Noise contracts are already vector-tested (`noise.json`, `link.json`) and stay
byte for byte.

**Stored formats.** The Rust box reads and writes the same files at the layout the last TypeScript
release writes. The layout number is frozen there, with no layout steps. A folder below it, or one
stopped mid-step, is refused (section 4). Every per-file version check in
[stored formats](stored-formats.md) is kept. Until the TypeScript box leaves the rollback window,
the Rust box writes nothing the last TypeScript release can't read. Change rules continue as today
afterward: new paths or per-file versions.

**Features.** The Rust box advertises exactly the `FEATURES` of the TypeScript version it
replaces, so the update gate needs no change. `kuvo/info`'s `bridge` and `pieces` name the
resident's own version.

## 4. Switch-over

- **Release.** The signed `box/1` manifest gains, additively, one pack per target (`aarch64` and
  `x86_64` macOS, `x86_64` and `aarch64` Linux built with musl, `x86_64` Windows) and a `launch`
  field. The last TypeScript release learns both before any Rust release is offered. Its
  `kuvo-start` launches `bin/kuvo-box serve` and no separate bridge when `launch` says so. Engine
  pins don't change.
- **Keeper.** On Mac and Linux the shell keeper stays for the first Rust releases. Its tested
  switch-and-judge path (rename `bin`, wait for `kuvo/info`, rename back) is what makes a rollback
  safe. `kuvo-box keep` is native Windows' keeper from the start, with a plain selector file
  instead of a junction (trial check 18). It replaces the shell keeper everywhere once the
  TypeScript box is gone.
- **Live work survives both ways.** The Rust resident adopts TypeScript holders, and the
  TypeScript bridge adopts Rust holders. The running engine is adopted through `run/engine.json`.
- **Rollback.** `bin` goes back to the TypeScript release, still installed beside it. That release
  reads everything the Rust box wrote.
- **Mixed setups** meet only through mail v1, signed documents, the lease and Noise sessions. All
  of these are vector-held and tested pairwise in session 9.1.
- **Staging gate.** The Rust release stages only on a computer whose folder is at the frozen
  layout and that no Mac from before 0.8 has visited in 30 days. Any other computer says why it
  can't stage and stays on the TypeScript box. The fleet update already allows that.
- **0.7.2 → 0.8.** The last TypeScript 0.8.x stays in the feed as the **stepping stone**.
  `kuvo-install` picks it when it finds 0.5–0.7 data (a Mac's app folder, or a machine without a
  bridge). The stone runs the move into `~/.kuvo-code`, then the ordinary fleet update brings the
  Rust box. So the Rust box carries no rehome and no "For released Macs" compatibility.
- **Auto and the sandbox.** On a Mac it's `sandbox-exec` with the same generated profile. On Linux,
  Landlock is applied by `kuvo-box fence-exec`. That exec starts single-threaded, so it calls
  `unshare` itself and drops the `bun:ffi` script and the util-linux dependency. Windows, native
  and WSL2, stays "coming soon", with Ask First as the default.
- **Secrets.** On a Mac they stay in the Keychain through Kuvo Helper. Linux and native Windows
  both use the wrapped-key files: Credential Manager isn't reachable from an SSH sign-in (trial).

## 5. What not to port

About 16,000 TypeScript lines, plus the bridge's process glue. The skipped compatibility stays in
the TypeScript box for the computers that still need it.

| Skip | ~Lines | Evidence |
|---|---:|---|
| Phone app: `phones.ts`, phone sync, push, legacy sign-in route | 810 | The phone link was never on in a release (`PROTOCOL.md`, "For released Macs"). Retired (CHANGELOG). |
| The Swift app's Mac front: `link/mac/*`, `toApp`, `kuvo/machines`, `kuvo/hosts` | 2,170 | The app source is gone. No caller in `gpui` or `mac/`. |
| `link/compat/*` and the 0.7 "stays" items in `protocol/compat-kinds.txt` | 1,300 | Served only to Macs from before 0.8 (staging gate). |
| Rehome, the 0.5–0.7 move | 5,250 | Only for app-folder data (stepping stone). |
| App-folder Mac box (`APP_FOLDER`, `mac-kit`) | 400 | Nothing starts `KUVO_BOX=mac`. |
| Old update path: trial, engine swap, `keep-up`, renewal, `pick()` | 1,700 | Does nothing once `bin` is a link (`kuvo-start:41`). |
| Unconverted setup authority and standby receipts | 750 | Every main converts on its first lease. The Rust box refuses an unconverted setup and leaves it to TypeScript. |
| Split launch (`run` with `serve --kept`, `box-services`) | 400 | The resident owns services ([one client process](one-client-process.md), step 7). |
| Folder layout steps (20 of 23 empty) | 500 | Frozen layout, per-file versions. |
| `kuvo/data/owner`, `data/messages`, `data/edits*` adapters | 60 | No desktop caller. Storage stays. |
| Goal cards: `link/work/goals.ts`, `agents/goals.ts`, `shared/goals.ts` | 2,320 | A dropped feature, being removed from the TypeScript box. |
| Typed keywords and slash commands: `areas/typed.ts`, `routes/typed.ts`, and their leftovers | 310 | Removed on 22 November. |
| Letters' `peerOf` and `moved` | 100 | Ported onto `ownerOf` instead. |
| The bridge as its own process: its API, `forward-version`, handover, `BRIDGE_VERSION`, tool forwarder, `kuvo-fence.js` | 2,500 | Folded into the resident and `kuvo-box` modes. |

Kept, despite being older: both SSH verbs, which one-client-process decision 3 keeps, and the
`serve.json`/`link.pid` records.

## 6. Phase plan

Pace: about 125,000 TypeScript code lines are ported (139,000 code lines without comments, less
the skipped ones). In Rust, with tests, that's about 150,000 lines. At the measured 500–1,000
tested lines an hour, that's 150–300 hours. The table, with harness, spikes, Windows and rehearsal
included, comes to **about 310 hours: two Codex weeks**, or three calendar weeks with four lanes.
Each row is one task in its own worktree, done when its area's conformance tests pass against
`kuvo-box`. Rows marked ×2 are two consecutive sessions. **Lanes:** A link and setup, B engine and
agents, C work and services, D OS, panel and keeper. Phases 0–1 run first; then the lanes run in
parallel.

| # | Session | Crate | After | Hours | Lane |
|---|---|---|---|---:|---|
| 0.1 | Box-under-test adapter; route scenarios through a binary; the TypeScript box passes | conformance | — | 6 | A |
| 0.2 | Peer conformance through binaries: mail-kind vectors, `hold/1`, golden home, digests | conformance | 0.1 | 6 | A |
| 0.3 | **Spike:** Claude Code's control protocol from Rust against the SDK (every feature in section 2); `cursor-agent` stream output; `agy` without the `.par` | — | — | 4 | B |
| 0.4 | Workspace, all crates empty and registered, `serve` answering `kuvo/info`; idle PSS on Mac and Linux; named pipe, Job Object and durability probe on Windows | bin, os | — | 5 | D |
| 1.1 | `kuvo_protocol`: codegen from shapes, checker held to `cases.json`, `FEATURES`; desktop takes the features and gate from it | protocol | 0.4 | 5 | B |
| 1.2 | `kuvo_store`: paths, versioned files, durability, locks, layout gate; reads the golden home | store | 0.4, 0.2 | 6 | C |
| 1.3 | `kuvo_os`: processes, start identity, groups and Job Objects, IPC, watching, platform facts | os | 0.4 | 6 | D |
| 1.4 | Server: dispatcher, local HTTP, refusals, the line with replay, SSH `route` and `route-to` | server | 1.1, 1.3 | 6 ×2 | B |
| 1.5 | `kuvo_wire` (shared with the desktop), relay client, paired sessions, mail v1, outbox | wire, peers | 1.1 | 5 ×2 | A |
| 2.1 | Lease and takeover, membership, signed documents, shared saves, time zone, personalities | setup | 1.4, 1.5 | 5 ×2 | A |
| 2.2 | Reach, merged lists, `ownerOf`, notices, devices and pairing | peers | 1.5 | 6 | C |
| 2.3 | Adding (SSH, installer code, seed), unlink and wipe, removal, re-enrollment | setup | 2.1 | 5 ×2 | A |
| 2.4 | Moves, handoffs, receipts (manifest v0–v3), letters on `ownerOf` | peers | 2.2 | 5 ×2 | C |
| 2.5 | Backups and restore (disk, SSH, relay store) | setup | 2.1 | 6 | A |
| 3.1 | Engine on demand, park, adoption; `api/…` passthrough with WebSocket and PTY | engine | 1.4 | 6 | B |
| 3.2 | Sessions, queues, send-once receipts, asks, waiting, models, search | engine | 3.1 | 6 ×2 | B |
| 4.1 | Holders (adopting the TypeScript ones), roster, accounts and sign-in, tools relay, `mcp` mode, sidecar | agents | 3.2, 1.3 | 6 ×2 | B |
| 4.2 | Codex app-server | agents | 4.1 | 5 | B |
| 4.3 | Grok and Antigravity over ACP, with extensions and the Antigravity legacy launch | agents | 4.1 | 6 | C |
| 4.4 | Claude Code over stream-json and control stdio, with holder handoff and signed-in checks | agents | 4.1, 0.3 | 6 | B |
| 4.5 | Cursor through the Bun sidecar for runs, account/sign-in and model catalog | agents | 4.1, 0.3 | 3 | C |
| 4.6 | Subagents, reviews, side questions, carry and handover, snapshots, transcripts | agents | 4.2–4.4 | 4 ×2 | B |
| 5.1 | Bots, wake-ups and watches fired by the resident, notes, attention, approvals | work | 3.2 | 5 ×2 | C |
| 5.2 | Work Queue: store, numbers and lease, labels, statuses, peer exchange, GitHub | work | 2.2 | 5 ×2 | C |
| 5.3 | Group chats and seats, autopilot | work | 4.6, 2.2 | 4 ×2 | B |
| 5.4 | Usage, limits, Insights (SQLite), pricing | services | 4.1, 2.2 | 5 ×2 | C |
| 5.5 | Library, artifacts, marketplace, integrations, skills, MCP catalog | services | 2.2 | 4 ×2 | A |
| 6.1 | Settings pages and flows, permissions, sandbox (Seatbelt, `fence-exec`) | services | 4.1 | 5 ×2 | D |
| 6.2 | Vault: Helper Keychain, wrapped-key files, broker, SSH agent | services | 1.3 | 6 | D |
| 6.3 | Files, Git, Changes, worktrees, projects, folders | services | 1.4 | 5 ×2 | D |
| 6.4 | Browser keeper: Chrome for Testing, CDP pipe, browser tools, live view, downloads | services | 4.1 | 5 ×2 | A |
| 7.1 | `kuvo-box keep`: supervision, selector, rollback; `kuvo-start` launching by manifest | keeper | 1.3 | 5 ×2 | D |
| 7.2 | Fleet update with per-target packs, staging gate, stepping stone in `kuvo-install`, uninstall, footprint | keeper | 7.1, 2.1 | 5 ×2 | A |
| 8.1 | Native Windows: profile ACLs, logon task and starter, Git Bash, path vectors, long paths | os, keeper | 7.1 | 5 ×2 | D |
| 8.2 | Native engine, browser and the five agents; `windows-box-check.sh` runs the suite | all | 8.1, 4.5, 6.4 | 6 ×2 | D |
| 9.1 | Mixed fleet: TypeScript and Rust pairs for lease, moves, numbers, update and rollback, holders in both directions | conformance | all | 6 | A |
| 9.2 | Budget check and a 24-hour soak with turns | — | 9.1 | 4 | B |
| 9.3 | Rehearsal on four computers (Mac, Linux, WSL2, native Windows), including a 0.7.2 Mac through the stone; rollback drill | — | 9.2 | 6 ×2 | A |
| 9.4 | After the rollback window: remove the TypeScript box, shapes source moves to Rust, docs | all | 9.3 | 6 | A |

Totals by phase: 0: 21 · 1: 39 · 2: 42 · 3: 18 · 4: 40 · 5: 46 · 6: 36 · 7: 20 · 8: 22 · 9: 28.
**The total is 312 hours.** By lane: A 100, B 82, C 61, D 69. Lane A is the longest chain.

While the plan runs, the TypeScript box keeps taking fixes. Each fix lands with a route scenario or
conformance test, so the Rust lane for that area takes it in. Windows sessions 4–11 in
[windows.md](windows.md) are replaced by 0.4, 1.3, 7.1 and 8.1–8.2. The native box is the Rust
box.

## 7. Decisions for the user

1. **0.8 and native Windows.** Ship 0.8 with the TypeScript box and WSL2, which is done. Native
   Windows comes with the Rust box, and native work on the TypeScript box stops. *Recommended.*
2. **Drop 0.7 Mac compatibility in the Rust box.** Computers that a 0.7 Mac still visits stay on the
   TypeScript box until it's gone, and the stepping stone carries 0.7.2 upgrades. *Recommended*:
   the alternative ports about 8,000 lines of rehome and compatibility code that expire three
   months after 0.8.
3. **Light freeze on the TypeScript box during the rewrite.** Allow fixes and small features, each
   with a scenario. Larger new areas wait, or start in Rust. *Recommended*: otherwise the port
   chases a moving box.
4. **Signing the Windows binary.** A code-signing certificate or signing service, for SmartScreen
   and Defender. This joins D4 in [windows.md](windows.md). *Recommended* before session 8.2.
5. **The rollback window.** Keep the TypeScript box installed beside the Rust box, and its source
   in the repo, for four weeks after the last computer switches. *Recommended.*
