# The box in Rust

Design, 23 November 2026. Source references are at `971e89b71`. The box (`box/src`, about
184,000 lines of TypeScript run by the engine's Bun) is rewritten in Rust as one program for macOS,
Linux and Windows, both native and WSL2. It replaces the TypeScript box in one step: no public
release mixes the two. The Rust box speaks today's protocol, reads every layout a release or the
user's own betas left, and does the move from older releases itself.

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
crates will be shared by path. `gpui/kuvo_box` will depend on `box/crates/kuvo_protocol` and
`box/crates/kuvo_wire`. Both are sans-IO: no runtime, and only serde, serde_json and the crypto crates
`kuvo_box` already uses. That keeps them resolvable under either lockfile. A change to them also
runs `kuvo_box`'s tests. Box crates get their own `scripts/heavy.sh` lane (`rust-box`) so parallel
box sessions don't queue behind desktop builds. Run `scripts/cargo-box.sh` from the repository
root. The workspace uses edition 2024, Cargo resolver 3, and the same Rust pin as GPUI. Rust's
`unsafe_op_in_unsafe_fn` and Clippy's `undocumented_unsafe_blocks` are denied. Release builds
keep speed optimization (level 3), use thin LTO and strip symbols for size.

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
| `kuvo_engine` | OpenCode on demand, `api/…` passthrough (WebSocket and PTY included), sessions, queues, send-once receipts, asks, search. After 4.7 and 9.0 only the sessions half stays, renamed `kuvo_sessions`. |
| `kuvo_agents` | Holders, roster, accounts and sign-in, the tools relay and MCP, one module per agent, subagents, reviews, handover. |
| `kuvo_work` | Bots, wake-ups and watches, the Work Queue, group chats and seats, autopilot. |
| `kuvo_services` | Usage, limits and Insights, Library, artifacts, marketplace, integrations, skills, settings pages, vault, files, Git, Changes, worktrees, the browser keeper. |
| `kuvo_keeper` | Supervision, release selection and rollback, fleet updates, install and uninstall. |
| `kuvo_move` | The move from 0.5–0.7.2 and 0.8.0-beta.1 into `~/.kuvo-code`, and what those releases' Macs still need (section 4). Its own crate so it can go whole. |
| `kuvo-box` (bin) | `main`: the modes below. Each crate registers its own routes with one call. |

**Libraries.** tokio (two worker threads; SQLite, fsync and Git go to the blocking pool, so the
request loop never waits); hyper 1 for serving, reqwest with rustls for fetching, and
tokio-tungstenite; x25519-dalek, aes-gcm, sha2, hkdf and ed25519-dalek; rusqlite (bundled);
notify, with polling for WSL's Windows drives; tokio's named pipes and `windows-sys` for Job
Objects, ACLs and start identity; rustix on Unix; the `landlock` crate; `rmcp` for MCP;
`agent-client-protocol` for ACP; jiff for time zones; pulldown-cmark; tracing. Git stays the `git`
command, as today. Terminals are the box's own (row 4.7c): `kuvo_os::pty` with `openpty` on Unix
and ConPTY on Windows, served by `kuvo_services` at today's `api/pty` paths and frames.

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
`scripts/small-computer-check.sh`. Phase 0.4's info-only resident measured 1.38 MiB physical
footprint on macOS, 1.16 MiB PSS on Linux, and 0.58 MiB private working set on native Windows.

### Phase 0.4 measurements

The stripped release binary ran in a fresh home (`KUVO_HOME`), answered an authenticated
`GET /kuvo/info` over `127.0.0.1`, and idled for two seconds before sampling. It only advertises
`info`, so the desktop compatibility gate holds it. The existing desktop process check learns
the Rust command when the local server lands in phase 1.4.

| Computer | Idle memory | Source |
|---|---:|---|
| Apple silicon Mac | 1,408 KiB | Physical footprint, `vmmap -summary` |
| lokum, Linux x86-64 | 1,184 KiB | PSS, `/proc/<pid>/smaps_rollup`, using a static musl binary built on the Mac |
| Native Windows x64 | 592 KiB | Private working set from `Win32_PerfFormattedData_PerfProc_Process`. Total working set was 5,212 KiB. |

The Windows release executable was cross-compiled on the Mac with portable LLVM MinGW
(`x86_64-pc-windows-gnullvm`, with static libunwind) and run natively on Windows build 26300.9974.
The Win32 probes ran over OpenSSH in a scratch folder:

- **Named pipe.** A protected DACL with one ACE for the account SID and `PIPE_REJECT_REMOTE_CLIENTS`.
  Two independent SSH connections with different logon SIDs both read the reply as the same user.
  [Named pipe access rights](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
- **Job Object.** The owner held the only job handle with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
  Terminating that owner stopped its live child within the five-second observation window.
  [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- **File replacement.** Flushing the writable temp file with `FlushFileBuffers` passed. Flushing
  the read-only handle and calling `MoveFileExW` with replace and write-through both returned
  Windows error 5 (`ERROR_ACCESS_DENIED`). On the same volume, `FileRenameInfoEx` with replace
  and POSIX flags succeeded while the original reader held `FILE_SHARE_DELETE`. The existing
  handle read the old contents and a new open read the replacement. Use this for the plain
  selector file. Power-loss recovery is still part of the keeper checks.
  [Windows replacement semantics](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)

## 2. Agents without a JavaScript runtime

| Agent | Today (`box/src/agents/agents/`) | Rust box decision |
|---|---|---|
| Codex | `codex app-server`, JSON-RPC on stdio | Same protocol in Rust. |
| Grok | ACP on stdio, plus `_x.ai/*` extensions | `agent-client-protocol`, with the extensions as ext methods. |
| Antigravity | ACP through `agy acp` or `agy_acp_server.par` for older sign-ins. Follows the agent's transcript file. | ACP in Rust. Keep both launches until a valid legacy profile can use `agy acp` without signing in again. |
| Claude Code | `@anthropic-ai/claude-agent-sdk` drives the user's Claude CLI. | Speak stream-json and the CLI's control protocol directly. No tested feature needs the SDK sidecar. |
| Cursor | `@cursor/sdk`, with `cursor-worker.ts` already running it under a holder. | Run the pinned SDK in the engine's Bun sidecar. The headless CLI's event stream lacks parity. |
| OpenCode | The engine itself, over its HTTP server and `api/…` passthrough. | One more ACP agent (`opencode acp`), one process per computer, on 4.3's client (4.7, [design](opencode-agent.md)). |

Session 0.3 ran the Mini's Claude Code 2.1.330 and Antigravity 1.5.14, plus a scratch
installation of Cursor CLI 2026.11.22-e9042b2. All ran with private homes, Keychain access
blocked and vendor networking disabled. Claude's real CLI ran once from Rust and once
through SDK 0.3.303 with the same scripted loopback model and MCP endpoints. The probes are
kept by the lead, out of the repository; their results are this section.

### Claude Code

| What Kuvo uses | Result |
|---|---|
| Initialize and model catalog | **Works.** `initialize` returns models and agents without a turn. `get_context_usage` with `detail: "summary"` returns `maxTokens`. |
| System prompt, custom agents, skills and settings | **Works with two configuration paths.** The preset uses the CLI's default prompt plus `appendSystemPrompt` in `initialize`, alongside `agents`, hook registration and `forwardSubagentText`. The loopback model received the append. `--plugin-dir-no-mcp` loaded the local plugin and skipped its MCP server. `--settings` supplied the memory directory, skill overrides and sandbox override. |
| PreToolUse, permission prompts, questions and Plan exit | **Works.** The CLI sent `hook_callback` and `can_use_tool`. A block prevented a write, an approval allowed a read, updated input answered `AskUserQuestion`, and a denied `ExitPlanMode` kept Plan mode. Rust replies with the request ID and keeps tool-use IDs distinct from callback IDs. Keep the SDK's callback cancellation and permission deduplication semantics. SDK field mappings such as `permission_suggestions` to `suggestions` belong at this boundary. |
| Model, effort and permission mode between turns | **Works.** `set_model`, `apply_flag_settings` (`effortLevel`), and `set_permission_mode` changed the live process. The next model request used Opus at high effort in Plan mode. A null `effortLevel` cleared the override. From Sonnet, omitting `model` returned to the account catalog’s resolved default (Opus on the fixture). |
| MCP | **Works.** `mcp_set_servers` connected and removed both an HTTP server with headers and a stdio server with command, args and environment. Server status showed both fixture tool lists. Keep connection details on the pipe. Kuvo uses no in-process SDK MCP servers. |
| Streaming, messages, steering and interruption | **Works.** Partial text, assistant frames, tool results, base64 image blocks and `priority: "next"` messages passed. Turn results can acknowledge several user UUIDs. `interrupt` during a streamed reply produced `aborted_streaming`. Raw stdio also exposes control traffic that the SDK consumes internally. |
| Transcript resume, live reconnection and clear | **Works at the CLI.** `--session-id` then `--resume` reused the transcript. A second `initialize` on an open permission ask returned the same request in `response.pending_permission_requests` (beside the inner payload). Rust answered the redelivery and both queued UUIDs completed. The SDK handled that repeat with one permission callback. Reinitializing with an unanswered hook sent `control_cancel_request` for the old callback. `/clear` emitted `conversation_reset` and the next turn used the new session ID. The TypeScript-to-Rust switch ends running turns. A live Haiku turn held on an approval survived a Rust-to-Rust keeper switch on macOS and WSL, with the same holder and Claude process (4.4). |
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

## 3. The contract the Rust box meets

The TypeScript box is the reference while porting, not a peer: the two never run in one public
setup, so nothing has to match byte for byte between them on the wire. Live checks between two
computers run Rust with Rust; the TypeScript box is compared through the conformance cases and
side-by-side runs. Rust boxes don't interoperate with TypeScript betas after beta.1 (adding,
moves, backups restored across them): only the user's four computers ran those, and they switch
together (user, 11-23). Released 0.5–0.7.2 Macs and 0.8.0-beta.1 stay supported.

**Route shapes: TypeScript stays the source until the TypeScript box is removed.** The shapes are
already exported as data (`protocol/vectors/routes/shapes/answers.json`, from
`scripts/route-shapes.sh`). `kuvo_protocol` generates Rust types and the checker from that file
into Cargo's build directory. It reads the advertised names from `protocol/vectors/routes/features.json`
and the update gate requirements from `protocol/gate-required.txt`. Its checker is held to `box/test/fixtures/routes/shapes/cases.json`, like the
JavaScript one. The TypeScript box remains the reference and keeps getting fixes, so a source of
truth anywhere else would mean two places to edit for months. At removal (session 9.4) this flips:
the shapes become hand-written Rust, and `answers.json` is generated from them for the
conformance suite and other clients.

**Conformance suite.** `protocol/conformance/` launches the selected box's `serve` in a scratch
home, reads `run/serve.json`, and uses authenticated loopback HTTP and the NDJSON line. The
source fixtures contain 95 route scenarios and 436 requests, including the shared features
case. The suite also runs the 17 Mac variants covered by the old Mac route runner. Given steps
seed files or configure the stand-ins. The actual TypeScript agents bridge runs separately for
group-chat cases until Rust serves those routes in the resident.

Run one area against both local builds, even if the first fails. Area names are the route fixture
stems, plus `resident` and `two-boxes`.

```sh
for bin in box/dist/kuvo-link.js box/target/debug/kuvo-box; do KUVO_BOX_BIN="$bin" KUVO_CONFORMANCE_AREA=bots node --test protocol/conformance; done
```

Run the [differential runner](../../protocol/conformance/differential/compare.mjs) with an offline
scratch fixture and a JSON list of requests.

```sh
KUVO_BOX_BIN_A=box/dist/kuvo-link.js KUVO_BOX_BIN_B=box/target/debug/kuvo-box node protocol/conformance/differential seed-home requests.json
```

Both runs start from the same home snapshot and each gets a fresh engine stand-in. The runner
reports status, JSON and home differences by path. Its normalisation table names each variable
field and why it varies. The Insights database is compared by its committed schema and rows,
including changes in its WAL. Seed an existing identity for tests of an existing setup so that keys
and signed documents compare. See [testing](../testing.md#box-conformance) for inputs and exit codes.

**Process tests to convert.** Paths in this table are under `box/bridge-tests/` except where shown.
"Converted" identifies the process test now in the suite. Cases marked later remain in the
TypeScript suite until their respective Rust phase.

| File | What it tests | Conversion |
|---|---|---|
| `two-boxes.test.js` | Membership and documents travel between computers. | Converted as `two-boxes` with two residents, installer enrollment and a replicated rename. The old phone notification and app-host paths stay with TS. |
| `setup-two-boxes.test.js` | Both computers report the same setup, and the member requests numbers from the main. | Later, 2.1 and 5.2. |
| `setup-handover*.test.js` | Lease, takeover, removal, offline saves and recovery across failures. | Later, 2.1, 2.3, 2.5 and 9.3. Keep the unconverted setup cases with TS. |
| `setup-mac.test.js` | Legacy Mac leadership, identity resets, and setup events. | App-host and retired authority stay with TS. Current events and restart later, 2.1. |
| `setup-judge.test.js` | Setup and attach verdicts for fresh, known and older homes. | Later, 2.3 and 2.6. Retired app-folder probes stay with TS. |
| `installer-enrollment.test.js` | Enrollment, merge, lost receipts, replay and credential selection. | Later, 2.3. |
| `devices.test.js` | Local grants, device list and revocation. | Later, 2.2. |
| `devices-two-boxes.test.js` | A paired device reaches both computers, with revocation and reconnect catch-up. | Later, 1.5 and 2.2. |
| `moves.test.js` | Moving sessions, bots and groups without losing queued work or attachments. | Later, 2.4. |
| `moves-races.test.js` | Failed or duplicate move offers, partial arrivals and concurrent session work. | Later, 2.4. |
| `moves-recover.test.js` | Retries, restart recovery and settling copies and mail. | Later, 2.4. |
| `moves-lanes.test.js` | Native agent move payloads, accounts and relay transfers. | Later, 4.6 and 5.1. |
| `moves-lanes-races.test.js` | Edits during native packing, interrupted native transfers and retained copies. | Later, 4.6 and 5.1. Shared transfer and retry behavior remains in 2.4. |
| `moves-elsewhere.test.js` | Listing and expiring moves between other computers. | Later, 2.4. Keep internal clock and announcer checks with TS. |
| `work-queue-across.test.js` | Peer views, number leases, forwarding, handoff and deduplication. | Later, 5.2. Keep retired numbering rules and internal projections with TS. |
| `work-queue-two-boxes.test.js` | Cross-computer items, numbering, delete and restore. | Later, 5.2. |
| `seats.test.js` | A group uses another computer's bots, with seat mail and recovery. | Later, 5.3. |
| `ssh-pipe.test.js` | Local SSH forwarding, streaming, deadlines and shutdown. | Later, 1.4. |
| `ssh-route.test.js` | HTTP, relay and SSH give the same answers and keep operation identity. | Later, 1.4 and 1.5. |
| `serve.test.js` | HTTP auth, requests and streams, heartbeat and interrupted bodies. | Converted as `resident` for discovery, auth and streaming. HTTP heartbeat and interrupted-body cases later, 1.4. The legacy app's stdin lifetime stays with TS. |
| `resident-serve.test.js` | One listener, vault, lease and paired relay survive restart. | Converted as `resident` for one listener and restart, with line replay. Vault and paired relay cases later, 1.5, 2.2 and 6.2. Split launch stays with TS. |
| `resident-supervision.test.js` | Resident demand and engine/bridge supervision. | Later, 3.1 and 7.1. The separate bridge stays with TS. |
| `resident-update-pilot.test.js` | An active sandboxed turn survives update and rollback. | Later, 7.2 and 9.3. |
| `fleet-switch.test.js` | Prepare, switch, retry, offline catch-up and progress. | Later, 7.2. |
| `keeper-pause.test.js` | Pausing the keeper and releasing work safely. | Later, 7.1. |
| `keeper-route.test.js` | The SSH launcher chooses the keeper's installed release. | Later, 7.1. |
| `keeper-rollback.test.js` | A failed version rolls back code while retaining writes. | Later, 7.1. Old trial and Mac app-folder cases stay with TS. |
| `updates.test.js` | Signed releases, packaging, staging, rollback records. | Later, 7.1 and 7.2 for behavior. Bundle contents and internal version helpers stay with TS. |
| `update-engine-turn.test.js` | Engine turns and commands across a release switch. | Later, 7.2. |
| `update-follows-peers.test.js` | Peer release discovery and scheduled updates. | Later, 7.2. The app update prompt stays with TS. |
| `update-from-before.test.js` | A released predecessor updates into the new layout. | Later, 7.3 and 9.3. |
| `update-real-keeper.test.js` | Real keeper upgrade from an app-owned baseline. | Later, release rehearsal for supported predecessors. Retired keeper paths stay with TS. |
| `update-stops-nothing.test.js` | A running holder survives a switch or rollback. | Later, 4.1 and 7.2. Bridge-specific handover stays with TS. |
| `own-home-mac-serve-update.test.js` | A helper-owned resident restarts on a new release. | Later, 7.2 and 9.3. |
| `gpui/kuvo_box/tests/real_box.rs` | Desktop transport pairs with a real box, asks a route and follows it. | Later, 1.5, using the selected binary in the existing client test. |

**TypeScript reference gaps.** The machine resident still answers the retired `GET kuvo/hosts`
route. The case is skipped for TypeScript with a reason pointing to TS reference green.
The generated `kuvo/info` shape omits the resident's `serverGeneration` and `serverBuild`.
Until TS reference green updates the shape, the suite checks those two fields separately and
keeps checking the rest of each reply against the generated shape.

**Added:**

- byte vectors for what 0.5–0.7.2 and 0.8.0-beta.1 Macs send and read (their SSH verbs, mail,
  beta.1's peers list), since those Macs still reach machines;
- `hold/1` transcripts (Rust holders keep turns running across Rust-to-Rust updates);
- canonical-JSON digest vectors for signed documents;
- **golden homes**: a 0.5–0.7.2 Mac's app folder and a machine it set up, a 0.8.0-beta.1 Mac and
  machine, and a home at the current TypeScript beta's layout with every stored family. The Rust
  box opens each and ends at the layout it writes.
- Seatbelt profile and Landlock policy text from the same inputs;
- the keeper's records (`serve.json`, `link.pid`, demand markers).

The relay, link and Noise contracts are already vector-tested (`noise.json`, `link.json`) and stay
byte for byte, for older Macs and between Rust versions.

**Stored formats.** The Rust box writes the files at the current TypeScript beta's layout (23),
frozen there with no more layout steps; later changes are new paths or per-file versions, as today.
It reads older homes through the move (section 4). A folder stopped mid-move resumes the move.
Every per-file version check in [stored formats](stored-formats.md) is kept. Nothing it writes has
to stay readable by a TypeScript box: going back is a restore (section 4).

**Features.** At switch-over, the Rust box advertises exactly the `FEATURES` of the TypeScript
version it replaces, so the desktop's gate needs no change. Between Rust releases, mixed versions
follow the normal rule. `kuvo/info`'s `bridge` and `pieces` name the
resident's own version.

## 4. Switch-over

- **Release.** 0.8 ships the Rust box on macOS, Linux, WSL2 and native Windows. The signed
  `box/1` manifest gains, additively, one pack per target (`aarch64` and
  `x86_64` macOS, `x86_64` and `aarch64` Linux built with musl, `x86_64` Windows) and a `launch`
  field; `kuvo-start` launches `bin/kuvo-box serve` and no separate bridge when `launch` says so.
  Engine pins don't change. Once 0.8 ships, the feed points to Rust. Returning TypeScript beta
  computers can still take the signed bridge beta described below.
- **Keeper.** On Mac and Linux the shell keeper stays for the first Rust releases: its tested
  switch-and-judge path (rename `bin`, wait for `kuvo/info`, rename back) keeps Rust-to-Rust
  rollback safe. `kuvo-box keep` is native Windows' keeper from the start, with a plain selector
  file instead of a junction (trial check 18), and replaces the shell keeper everywhere later.
- **0.5–0.7.2 and 0.8.0-beta.1 → 0.8: the move is the Rust box's own.** `kuvo-install`, or the
  Rust box's first start, finds the older data (a Mac's app folder, a machine without a bridge, a
  beta.1 home) and moves it into `~/.kuvo-code` at the frozen layout, automatically and resumable.
  `kuvo_move` ports rehome (5,250 lines) and the items PROTOCOL.md "For released Macs" lists for
  those releases, re-checked against `protocol/compat-kinds.txt` and the code:
  - **`stays`** (0.7: app-agents-stopped, engine-stop, files, keeper, keeper-number,
    leave-running, visits; beta.1: box-read-change, deleted-bots, mac-moves, peers-list): ported
    as served behaviour, because a Rust machine can now be visited by a Mac that hasn't updated.
    `files` is the hard one: a 0.7 Mac still copies its own TypeScript engine, bridge and keeper
    over the machine's, and the Rust keeper must notice and repair from its release.
  - **`next`** (bot-json-job, limits-own-numbers, own-envelope): #717 decides them before 7.3;
    the plan counts them as ported.
  - **Taken in once** (0.7: settings, worktrees, bot-kit, agents-runtime, mac-group-chats,
    mac-git-settings, box-branch-prefix, kuvo-keys, move, pages-addresses; beta.1: mac-letters,
    outbox, mac-agents, connected): steps of the move.
  - **Not ported:** beta.30's three items (only TypeScript betas after beta.1 had them) and
    layout.4 setup (no public release wrote a setup; the user's mains are converted, which the
    golden home checks). Of the 23 layout steps only step 1's work (the staged import) runs,
    inside the move: 11 and 17 rewrite setup records no public release wrote, and the user's
    computers are past them.
- **The user's four computers** are the only ones on a TypeScript 0.8 beta, and switch once:
  - **Bridge beta.** The four computers first take one final TypeScript beta through their
    normal updater. That beta still runs TypeScript and knows how to prepare and start the
    signed Rust pack for its target. A computer still on beta.48 which reaches a Rust
    fleet takes the signed bridge beta first, from its main computer if necessary, then takes
    Rust. The handoff runs automatically before the returning computer takes on fleet work.
    Include the bridge's signed `boxes.json` with `sign-box.mjs --bridge` in each Rust manifest
    while these betas can still return. Keep its pack at its signed URL so the main computer can
    fetch it for the returning computers.
  - Before the first start, the Rust box copies the whole `~/.kuvo-code` into
    `~/.kuvo-code/backups/before-rust`, kept until the user says to delete it.
  - It reads the TypeScript beta's home in place, rewriting only what differs from its own layout.
  - All four are prepared, then switched together by the normal fleet update. A computer that's
    off updates when it comes back, before it rejoins.
  - **Running turns are not carried across this one switch** (user, 11-23: "I can handle 1
    interruption"). The TypeScript box stops its turns; the Rust box starts fresh. Every later
    update, Rust to Rust, keeps running turns: the Rust resident adopts its own holders and the
    running engine. No TypeScript-to-Rust holder adoption is built.
  - **No automatic rollback** of this switch (user, 11-23). If it goes wrong, the lead restores
    `backups/before-rust` and reinstalls the last TypeScript beta by hand. Backups made by the
    TypeScript betas stay in that copy; Rust doesn't restore them.
  - A computer that was off during the switch reaches the Rust computers and updates itself
    when it comes back: that update path is the only TypeScript-to-Rust interplay built.
- **The TypeScript box's code** stays in the repository until four weeks after the user's
  computers switched, then is removed (9.4).
- **Auto and the sandbox.** On a Mac it's `sandbox-exec` with the same generated profile. On Linux,
  Landlock is applied by `kuvo-box fence-exec`. That exec starts single-threaded, so it calls
  `unshare` itself and drops the `bun:ffi` script and the util-linux dependency. Windows, native
  and WSL2, stays "coming soon", with Ask First as the default.
- **Secrets.** On a Mac they stay in the Keychain through Kuvo Helper. Linux and native Windows
  both use the wrapped-key files: Credential Manager isn't reachable from an SSH sign-in (trial).

## 5. What not to port

About 11,500 TypeScript lines, plus the bridge's process glue. Nothing skipped here is needed by a
public release: the compatibility older Macs need is ported (section 4).

| Skip | ~Lines | Evidence |
|---|---:|---|
| Phone app: `phones.ts`, phone sync, push, legacy sign-in route | 810 | The phone link was never on in a release (`PROTOCOL.md`, "For released Macs"). Retired (CHANGELOG). |
| The Swift app's Mac front: `link/mac/*`, `toApp`, `kuvo/machines`, `kuvo/hosts` | 2,170 | The app source is gone. No caller in `gpui` or `mac/`. Its beta.1 letters are taken in by the move. |
| App-folder Mac box (`APP_FOLDER`, `mac-kit`) | 400 | Nothing starts `KUVO_BOX=mac`. |
| Old update path: trial, engine swap, `keep-up`, renewal, `pick()` | 1,700 | Does nothing once `bin` is a link (`kuvo-start:41`). |
| Unconverted setup authority and standby receipts | 750 | Every main converts on its first lease; no public release wrote a setup. The Rust box refuses an unconverted one. |
| Split launch (`run` with `serve --kept`, `box-services`) | 400 | The resident owns services ([one client process](one-client-process.md), step 7). |
| Folder layout steps as steps (20 of 23 empty) | 400 | Frozen layout; the move does step 1's work. |
| `kuvo/data/edits*` adapters and non-session owner reads, with the bot settings they edit (`bots/offline.ts`) | 175 | No desktop caller. Storage and the ordinary session owner reads stay, and a bot keeps the `offlineEdits` and `offlineJob` a TypeScript box wrote. |
| Goal cards: `link/work/goals.ts`, `agents/goals.ts`, `shared/goals.ts` | 2,320 | Removed from the TypeScript box on 23 November. |
| Typed keywords and slash commands | 310 | Removed on 22 November. Composer draft delivery and the 0.7.x `/typed` compatibility route remain. |
| Letters' `peerOf` and `moved` | 100 | Ported onto `ownerOf` instead. |
| The bridge as its own process: its API, `forward-version`, handover, `BRIDGE_VERSION`, tool forwarder, `kuvo-fence.js` | 2,500 | Folded into the resident and `kuvo-box` modes. |

Kept, despite being older: both SSH verbs, which one-client-process decision 3 keeps, and the
`serve.json`/`link.pid` records.

## 6. Phase plan

Pace: about 132,000 TypeScript code lines are ported (139,000 code lines without comments, less
the skipped ones; the move and older Macs' compatibility are about 7,000 of them). In Rust, with
tests, that's about 158,000 lines. At the measured 500–1,000 tested lines an hour, that's 160–320
hours. The table, with harness, spikes, Windows and rehearsal included, comes to **about 330
hours: a little over two Codex weeks**, or three calendar weeks with four lanes.
Each row is one task in its own worktree, done when its area's conformance tests pass against
`kuvo-box`. Rows marked ×2 are two consecutive sessions. **Lanes:** A link and setup, B engine and
agents, C work and services, D OS, panel and keeper. Phases 0–1 run first; then the lanes run in
parallel.

| # | Session | Crate | After | Hours | Lane |
|---|---|---|---|---:|---|
| 0.1 | **Done.** Binary conformance for the routes, resident and two computers, with side-by-side response and home comparison. | conformance | — | 6 | A |
| 0.2 | **Done.** Tagged Mac and machine fixtures, every stored family in the current-beta home, legacy SSH and mail, `hold/1` transcripts, and document digests. [Fixture coverage](../../protocol/conformance/compat/README.md) | conformance | 0.1 | 6 | A |
| 0.3 | **Spike:** Claude Code's control protocol from Rust against the SDK (every feature in section 2); `cursor-agent` stream output; `agy` without the `.par` | — | — | 4 | B |
| 0.4 | Workspace and info-only `serve`. Idle memory was 1.38 MiB on Mac, 1.16 MiB on Linux and 0.58 MiB on Windows. The Windows SID pipe, Job Object and open-file replacement probes passed. [Measurements](#phase-04-measurements) | bin, os | — | 5 | D |
| 1.1 | **Done.** `kuvo_protocol` generates Rust route types and the checker from the TypeScript export, passes the shared shape cases, and supplies the desktop features and update gate. | protocol | 0.4 | 5 | B |
| 1.2 | **Done.** `kuvo_store`: home paths, envelopes, atomic writes, locks, layout 23 gate, policy declarations and SQLite. [Coverage](rust-store-coverage.md) | store | 0.4, 0.2 | 6 | C |
| 1.3 | **Done.** `kuvo_os` provides platform paths and login environments, owned process groups and jobs with start identities, private local IPC, shared watches with polling, and Seatbelt and Landlock launchers. | os | 0.4 | 6 | D |
| 1.4 | **Done.** `kuvo_server` provides typed dispatch, resident HTTP and discovery, the replaying line and SSH IPC. Addressed SSH authenticates through the pairing provider that setup supplies in 2.1. Paired route version checks come from `devices-two-boxes.test.js`. | server | 1.1, 1.3 | 6 ×2 | B |
| 1.5 | **Done.** The desktop shares `kuvo_wire`. `kuvo_peers` owns relay connections, paired sessions, mail v1, and durable letter state. [Coverage and follow-ups](rust-wire-coverage.md) | wire, peers | 1.1 | 5 ×2 | A |
| 2.1 | **Done.** Current setup lease, takeover, signed membership and documents, shared saves, time zone and personalities. [Coverage and owner phases](rust-setup-coverage.md)<br>Also covers `peers.test.js`, `owner-mutations.test.js`. | setup | 1.4, 1.5 | 5 ×2 | A |
| 2.2 | Done. Peer reach, merged lists, record lookup, notice history, signed desktop grants and cross-computer calls/rows. Installer enrollment and rename pass on both implementations. [Coverage](rust-peers-coverage.md)<br>Inherits `setup-lists.test.ts`, `peers.test.js`, `mac-peers.test.js`, `devices.test.js`, `devices-two-boxes.test.js`, `reach-elsewhere.test.js`, `reach-elsewhere-rules.test.js`. Setup cases also come from `setup-two-boxes.test.js`, `documents.test.js`, `owner-mutations.test.js`, and `setup-handover.test.js`. | peers | 1.5 | 6 | C |
| 2.3 | Installer codes, fresh SSH, selected machine credentials, cancellation receipts, discovery, local seed intake, signed machine unlink, stopped standalone uninstall, and wipe of a standalone Unix resident without a keeper or configured backup are implemented. Members forward installer, fresh SSH, and plain unlink requests. Re-enrollment compares retained work, including SQLite snapshots. SSH recovery, release and credential integration, Mac Helper keys, member and managed wipes, and live and remote uninstall remain open. [Coverage](rust-adding-coverage.md)<br>Adding (SSH, installer code, seed), unlink and wipe, removal, re-enrollment. Admission, roster changes, removal and re-enrollment from `setup-merge.test.ts`, `setup-two-boxes.test.js`, `setup-judge.test.js`, `setup-mac.test.js`, `documents.test.js`, `devices-two-boxes.test.js`, and `setup-handover.test.js`.<br>Also covers `peers.test.js` and `devices.test.js`.<br>2.3 cannot close until setup merge reserves Work Queue numbers on the current relay lease with a durable source-bound approval, then connects the queue import, retained aliases and Markdown reference rewrite. | setup | 2.1 | 5 ×2 | A |
| 2.4 | **Done.** Ordinary OpenCode chat moves carry queues, wake-ups, files, notes and replay fences. Durable receipts and authenticated owner requests support transfer retry and cleanup. Letters, delayed notes and transcript replies follow the owner. The owner snapshot follows a move, receipt pages keep their cursor across issuer replacement, unlink and restore preserve the ownership fence, and moving or moved conversations fence session actions, live ask replies and attachment retention. Verified across macOS, WSL2 GNU, static Linux musl and native Windows. [Coverage](rust-moves-coverage.md) Ownership and receipt cases also come from `offline-merge.test.ts` and `setup-handover.test.js`.<br>Inherits `setup-lists.test.ts`, `peers.test.js`, `owner-mutations.test.js`, `mac-peers.test.js`, `reach-elsewhere.test.js`, `reach-elsewhere-rules.test.js`. | peers | 2.2 | 5 ×2 | C |
| 2.5 | **Done.** Version-one encrypted archives, disk, SSH and Relay recovery, journal fences, held-work review, vault routing, and pre-switch and wipe hooks. [Coverage and owner phases](rust-backups-coverage.md) | setup | 2.1 | 6 | A |
| 2.6 | **Done.** Released Mac and machine homes reach layout 23 through a resumable move. Both tagged Mac destinations match, and the credential move is verified through the Helper interface stand-in. [Coverage](rust-move-coverage.md) | move | 1.2, 2.1 | 6 ×2 | C |
| 3.1 | **Done.** On-demand OpenCode, idle parking, process adoption, and authenticated HTTP and PTY passthrough. See [coverage and remaining cases](rust-engine-coverage.md). | engine | 1.4 | 6 | B |
| 3.2 | **Done.** Sessions, queues, send-once receipts, asks, waiting, models and search. [Coverage and remaining owners](rust-engine-coverage.md#implementation-and-remaining-parity-work). | engine | 3.1 | 6 ×2 | B |
| 4.1a | **Done.** Rust holder command, private IPC, journal, recovery records and shared Rust controller. [Coverage](rust-agents-coverage.md) | agents | 1.3 | — | B |
| 4.1b | **Done.** Resident agent runtime and holder adoption through keeper updates, roster, accounts and sign-in, session and model integration. Authenticated resident and keeper proof is owned by 4.4; the carried cases are named in the [coverage](rust-agents-coverage.md). | agents | 3.2, 4.1a | — | B |
| 4.1c | **Done.** Tools relay, `mcp` mode, shared RPC and the Bun sidecar installer. Every resident gives sandboxed or held agents the forwarder, Macs included. Adapter rows pass the relay to their factory; 6.1 brings the sandbox launcher, 6.4 the browser tools program and engine-own-tools. [Coverage](rust-agents-coverage.md) | agents | 4.1a | — | B |
| 4.2 | **Done.** Codex over its app-server: account, sign-in, models, turns, asks, background terminals and holder adoption through keeper updates, with the shared program updater and its turn hold. It also serves the `kuvo/agents/` routes the app uses that had no Rust route: Settings › Accounts' rows, install, program updates and notices, a conversation's run state, read and model, and the agents' feed (`kuvo/agents/api/event`). Every `codex.test.ts` case is ported or has an owner. Passes on macOS, Linux, WSL and native Windows, and live on GPT-6-Luna across a keeper switch (macOS). [Coverage](rust-agents-coverage.md#codex-42) | agents | 4.1 | 5 | B |
| 4.3 | Grok and Antigravity over ACP, with extensions and the Antigravity legacy launch. One shared ACP client with a small flavour per agent, carrying several sessions on one agent process under one holder, so OpenCode (4.7) can run one process per computer. | agents | 4.1 | 7 | C |
| 4.4 | **Done.** Claude Code over stream-json and control stdio. 4.4a covers account checks, sign-in, legacy account migration and model discovery. 4.4b covers turns, holder handoff, steer receipts, refused sign-ins and the live keeper proof (macOS and WSL). Both pass on macOS, Linux, WSL and native Windows. Per-scenario parity for the remaining `claude.test.ts` groups and the complete home comparison remain. [Coverage](rust-agents-coverage.md#claude-code-44) | agents | 4.1, 0.3 | 6 | B |
| 4.5 | Cursor through the Bun sidecar for runs, account/sign-in and model catalog | agents | 4.1, 0.3 | 3 | C |
| 4.7 | OpenCode as one more coding agent, on 4.3's ACP client ([design](opencode-agent.md)). 4.7a: parity spike, ACP or HTTP (2 h). 4.7b: adapter, one held process per computer, catalog cache, sign-in paths, MCP config read by the box, bots and wake-ups pointed at it (6 ×2). 4.7c: the box's own terminals, `kuvo_os::pty` and `api/pty` in `kuvo_services`, lane D, any time after 1.3 and before 4.7d (5 h). 4.7d: existing conversations listed, read and continued, then the engine place removed: every `Place::Engine` branch and the engine's start triggers (5 h). | agents | 4.3, 3.2 | 24 | B |
| 4.6 | Subagents, reviews, side questions, carry and handover, snapshots, transcripts | agents | 4.2–4.4, 4.7 | 4 ×2 | B |
| 4.8 | **In progress.** [Idle work](#48-idle-work): agent discovery and version checks made lazy; Library, Marketplace, session and Settings follow changes; keeper, work and peer idle rounds are event-driven. Setup leases are demand-driven, with fenced shared numbering; cross-platform gates remain. | all | 4.2, 4.4, 5.5, 6.1, 7.1 | — | B |
| 5.1 | **Done.** Bots: making (on the engine and on coding agents, from New Bot and Add Agent), changing, deleting, telling and settling them, held edits for your other computers and those computers' bots; wake-ups and watches fired by the resident, for the engine's conversations and the coding agents'; approvals, work settings, oversight, notices and their holds, the work watcher, Stop and Stop All, work for leads on your other computers, the agents' work tools, wake tools and letters; and the app's routes from the audit (`kuvo/asks`, shell output and Stop, Add Agent). Verified on macOS (both boxes), static Linux musl under WSL2, and native Windows. Not ported: offline bot setting edits, with the record-edit adapters (section 5). Open elsewhere: an engine bot switched to a coding agent's model (4.6) and group chats as a bot is deleted (5.3). [Coverage](rust-work-coverage.md)<br>Bots, wake-ups and watches fired by the resident, notes, attention, approvals. Bot retell and wake consumers from `personalities-box.test.js`, `time-zone.test.ts`, `time-zone.test.js`, and `setup-handover.test.js`. Bot templates in the personality route fixtures.<br>Also covers `setup-lists.test.ts`, `notices-rule.test.ts`, `notices-plain.test.ts`, `mac-peers.test.js`, `devices.test.js`.<br>Use `LocalSessions::activity()` for the local OpenCode rows in background work and peer activity. | work | 3.2 | 5 ×2 | C |
| 5.2 | **Done for the resident contract.** Work Queue: store, numbers and lease, labels, statuses, peer exchange, GitHub. Verified on Mac (both boxes), native Windows and static Linux musl under WSL. [Coverage and integration dependencies](rust-workqueue-coverage.md) Queue cases from `setup-shared.test.ts`, `setup-merge.test.ts`, `work-queue-numbering.test.ts`, `offline-merge.test.ts`, `setup-two-boxes.test.js`, `documents.test.js`, `owner-mutations.test.js`, and `setup-handover.test.js`.<br>Also covers `setup-lists.test.ts`.<br>Reuse 3.2's `kuvo_peers::memory_read` transport for authenticated read-only requests without a disk spool.<br>2.3 cannot close until setup merge reserves Work Queue numbers on the current relay lease with a durable source-bound approval, then connects the queue import, retained aliases and Markdown reference rewrite. | work | 2.2 | 5 ×2 | C |
| 5.3 | Group chats and seats, autopilot. Group clock consumer in `time-zone.test.ts`.<br>Also covers `mac-peers.test.js`. | work | 4.6, 2.2 | 4 ×2 | B |
| 5.4 | **In progress:** the local usage settings routes, accepted Claude Code/Codex turn readings, native Claude/Codex/Grok, Cursor/Antigravity readings, engine credential plans, local Codex reset confirmation/replay and work asks, menu/notice consistency, bundled pricing resolution and a startup/file-watch SQLite Insights index with portable mutation cursors and native JSONL offsets, peer limit merge, passive Codex log readings that never start an app-server, read-only engine SQLite/Antigravity history, native usage calendar aggregation, Codex fallback and price catalog refresh have verified slices. Peer history and Insight cursor exchange (including mixed-version mail), normalized SQL summary and changed-header engine lineage have verified slices. Claude file-owned grants, engine ChatGPT and Cursor account-wide history, and expired-notice peer-state handoff now have four-platform verified scenarios. Peer reset spending and shared Always Allow decisions, Antigravity local quotas/refusal journal/independent holds, and billed-channel deduplication/removal have verified slices. Engine ChatGPT credits, aged and concurrent reset settlement, merged agent-tool words, Codex live/archive and multi-home history, exact native-log pricing, and native Windows own-profile Antigravity discovery have four-platform verified scenarios. Provider edge cases, reset settlement/decisions, additional notice-state cases, historical replay and full Insights parity are open. [Coverage](rust-usage-coverage.md).<br>Also covers `notices-usage.test.ts`. | services | 4.1, 2.2 | 5 ×2 | C |
| 5.5 | **Done.** Library, artifacts, attachments, setup-list peer views (cases 17, 23, 28), marketplace, integrations, skills, MCP catalog and `kuvo/programs` ported to `kuvo_services` (plus additive `kuvo_setup` marketplace installations helper) and verified on macOS, Linux musl and native Windows. Live bot creation from Marketplace Add Agent stays with 5.1b; Settings pages stay with 6.1. [Coverage](rust-library-coverage.md) | services | 2.2 | 4 ×2 | A |
| 6.1 | Settings pages and flows, permissions. 6.1b serves the settings sections, forms, revision-guarded writes, events, Git migration, Custom Auto lists and peer archive copies/actions. [Coverage and remaining owner gates](rust-settings-coverage.md). Auto mode, its reviewer and the sandbox are postponed past 0.8 (user 11-25); every computer answers as one without the sandbox. | services | 4.1 | 5 ×2 | D |
| 6.2 | **Done.** (Mac wipe end to end is verified in 2.3; Helper supervision cases are 7.2's, browser and settings cases 6.4 and 6.1.) Vault: Helper Keychain, wrapped-key files, broker and SSH agent. The released move uses the TypeScript `security` path for custom Claude Code credentials, verified with a disposable Mac Keychain. Installed Helper and Keychain cases in `setup-mac.test.js` remain open. See the [coverage and remaining gates](rust-vault-coverage.md). | services | 1.3, 4.1, 6.1 | 6 | D |
| 6.3 | Files, Git, Changes, worktrees, projects, folders. Golden-home projects in `protocol/conformance/compat.test.mjs`. 6.3a files and folders: a file's info, text and bytes, folder lists, a new folder, a folder's tree, finding files and suggested folders ported (`kuvo_services::files`, `files` conformance area, every TS case covered); **part A done** on macOS and native Windows (15 of 16 there, unreadable files skipped: no mode bits); Linux left ([coverage](rust-files-coverage.md)). 6.3b1 projects list: `GET kuvo/projects` ported (`kuvo_services::git`, `git` conformance area, macOS, Windows, Linux); 6.3b2 safe git: `ownArgs`/`askedArgs` ported (`kuvo_services::git::safe`), the argument table matching TS and a hostile repository starting nothing through either box; TS gaps fixed in both: `write-tree`, and a repository's transports and askPass (the box's own git reaches only local paths). 6.3c: project edit and every Git/Changes route the desktop calls are served; conformance 16/16 in TypeScript and Rust on macOS, and in Rust 15/15 applicable on Linux and WSL, 13/13 applicable on native Windows. Windows folder queries fixed in both; Mac login cache and native Trash deadline matched. Differential matches the Git/project answers and files, not unrelated startup files; remaining scenario and worktree lifecycle gaps in the [coverage map](rust-git-coverage.md). | services | 1.4 | 5 ×2 | D |
| 6.4 | Browser keeper: Chrome for Testing, CDP pipe, browser tools, live view, downloads | services | 4.1 | 5 ×2 | A |
| 7.1 | **Done.** Portable `kuvo-box keep`, atomic selection and rollback, Windows keeper handover, and `kuvo-start` launching by manifest. [Coverage and remaining owner phases](rust-keeper-coverage.md) | keeper | 1.3 | 5 ×2 | D |
| 7.2 | **Done.** Fleet update with per-target packs, `kuvo-install` starting the move, uninstall, footprint. Keep each Rust release's `box.json` beside `bin` with a top-level `launch: "rust"`, including when the previous TypeScript stager moves only `bin`. TypeScript beta interoperability is limited to the deployed beta.48 taking the bridge and then Rust. [Coverage and carried cases](rust-updates-coverage.md). | keeper | 7.1, 2.1, 2.6 | 4 ×2 | A |
| 7.3 | **Done.** Released Macs (0.7.2, beta.1) beside Rust computers: historical SSH verbs through the pack's delegate, unsigned beta.1 roster and agents mail both ways, removing an away beta.1 Mac, the `stays` and `next` items, and repairing a machine a 0.7 Mac overwrote; signed-Mac cases are TypeScript-beta only. [Coverage, every handed case and its owner](rust-released-coverage.md).<br>Covers `documents.test.js`, `setup-mac.test.js`, `time-zone.test.ts`, `personalities-box.test.js`, `setup-handover.test.js`, tagged `compat.test.mjs`, `peers.test.js`, `mac-peers.test.js`, `devices.test.js`, `devices-two-boxes.test.js`. | move | 7.1, 2.4 | 6 | D |
| 8.1 | **Done.** Native Windows: profile ACLs and private secret folders, logon task and starter (sign-out and restart proven), Git Bash for Kuvo's commands, path vectors and long paths, services by process tree, the last computer's wipe and reinstall, and a fresh install on the administrator test account (no standard-account run, at the user's choice). [Coverage](rust-windows-coverage.md) | os, keeper | 7.1 | 5 ×2 | D |
| 8.2 | The OpenCode agent on Windows, browser and the other five agents; `windows-box-check.sh` runs the suite on unsigned builds | all | 8.1, 4.5, 6.4 | 6 ×2 | D |
| 9.0 | Consolidation before the switch (build, not review): remove duplication across crates, split files over ~1,000 lines (engine queue, move driver, setup actor), a typed command line for `kuvo-box`, collapse the coverage maps into one short reference. Delete what's left of the engine in `kuvo_engine` (`launch`, `lifetime`, `proxy`, `api`, `release`, `model_refresh`: about 2,425 lines) and rename the crate `kuvo_sessions`; 3.2's queues, receipts, delivery, asks, search and list stay. Every change stays green against both boxes; measure lines before and after. | all | all of 2–8 | 4 | B |
| 9.1 | The switch from the TypeScript beta: backup, first-start take-over of its home (running turns end once), and the update of a computer that was off. Cross-process link writer handoff from `setup-mac.test.js` and `setup-handover.test.js` | move, keeper | all | 6 | A |
| 9.2 | Budget check and a 24-hour soak with turns | — | 9.1 | 4 | B |
| 9.3 | Rehearsal: the user's four computers (Mac, Linux, WSL2, native Windows) from the TypeScript beta, plus a 0.7.2 Mac and a 0.8.0-beta.1 machine straight to Rust. Rust-to-Rust mixed versions and rollback, real Helper and released Mac cases from `setup-handover.test.js`, `setup-mac.test.js`, and `setup-two-boxes.test.js`. | — | 9.2 | 7 ×2 | A |
| 9.4 | Four weeks after the user's computers switched: remove the TypeScript box, shapes source moves to Rust, docs | all | 9.3 | 6 | A |

Totals by phase: 0: 21 · 1: 39 · 2: 54 · 3: 18 · 4: 64 · 5: 46 · 6: 36 · 7: 24 · 8: 22 · 9: 30.
**The total is 354 hours.** By lane: A 100, B 100, C 74, D 80. Lanes A and B are the longest chains.

While the plan runs, the TypeScript box keeps taking fixes. Each fix lands with a route scenario or
conformance test, so the Rust lane for that area takes it in. Windows sessions 4–11 in
[windows.md](windows.md) are replaced by 0.4, 1.3, 7.1 and 8.1–8.2. The native box is the Rust
box.

### 4.8 Idle work

The resident should do nothing until there is useful work. The following intentionally change when
work happens, not which computer owns it or the route shapes:

- **Agent accounts and model lists:** Codex reads its native sign-in metadata and its last catalog
  without starting an app-server; it refreshes the catalog when the app-server starts for work or
  a sign-in completes. On first use it can read Codex's own model cache; without either cache the
  catalog is incomplete (the resident must not pretend an unlisted model has gone). Claude reads
  native sign-in metadata and a saved catalog, or offers its usual provisional models. Its first
  real turn or sign-in discovers the complete catalog and context limits. Both keep disposable,
  per-account catalogs under `run/agents` across resident restarts. The Rust-only timing cases in
  `agents-codex` and `agents-claude` fail on the previous binary; the ordinary catalog cases exercise
  the same account and model answers after work. Claim labels available only from a native probe
  may be less specific until the agent does work. Plan usage probes belong to row 5.4.
- **Agent program checks:** no version or installer check at resident startup. The agents settings
  asks on first use or refresh; otherwise the check is daily. A local executable's canonical path,
  modification time and length identify the last successful version read even after a resident
  restart. Package release lookups keep their daily result. The `agents-updates` case checks that
  reopening Settings and restarting do not run the unchanged executable, and a changed executable
  gives its new version.

- **Library and Outputs:** independent content hashes and per-recipient ledgers mean an unchanged
  Library is not sent with a changed output. Stores, file-parent and bot-description watches and
  peer membership/reconnect events update the copies. Modern computers request the initial copy
  after their own receiver is ready (`ask: "library-copy"`); new and returning computers get a full
  copy without relying on a periodic resend to recover startup mail.
  Rust peers advertise `data.library-change-copy` and no longer age out Library content merely
  because it hasn't changed. A released peer without that capability still gets the old 60-second
  refresh while connected (its standing rules require it); no such timer runs with no peers or
  with only change-driven peers. The `library` conformance case pins an output without rewriting
  the peer's Library, then removes an untracked file and checks that the peer sees it disappear.

- **Marketplace:** installation records, setup view and vault/settings changes trigger the existing
  projection instead of a five-second round. A fresh empty installation performs no projection or
  vault scan. A previous installation may still need cleanup of projected tools or credentials when
  unlocked. `library` tests an installed MCP server following an external config edit, without a
  second route read or a maintenance round. The `vault` conformance cases still use the same routes.

- **Resident inventory and Settings:** the complete session inventory has no minute refresh; a
  missed file-watch notification or an actual engine event-feed reconnection triggers recovery.
  A parked engine waits for a state change, not a two-second retry. The `idle_inventory` scenario
  advances the clock by a minute, checks that a silent fixture edit has not read the engine, then
  sends an inventory event and sees the changed list. Followed Settings pages listen to their
  specific files, setup/peer views, sessions, engine/catalog/agent events, vault and measurements.
  The Git settings case edits the config file outside a route; Permissions and Archived Sessions
  cases follow local and remote mutations. Peer archives are rebuilt on archive revision, roster,
  time zone or move ownership changes, not every ten seconds.
- **Keeper, work and peers:** the keeper follows selection and stop/handover files and a native
  exact-process exit notification (Darwin kqueue, Linux pidfd, Windows process handle). A kernel
  without the exit facility uses the old fallback; real release trials and restart backoff still
  have deadlines. A supervised Windows resident receives a private local socket notification and
  verifies its durable stop identity; a file watch accepts the older keeper's request. Native
  process and keeper scenarios exercise the exit notification and matching stop identity.
  A fast release switch can precede the holder's batched acknowledgement; saved Codex terminal
  outputs retain their last applied holder line per item, so replay does not append those bytes
  twice. The existing idle-terminal adoption scenario and a deterministic replay test cover it. Update
  rounds follow setup, peers, files and engine, and sleep until the next check when there is no
  release to prepare; active release progress and switch barriers still have a short deadline.
  A failed direct release download can wait for a holder announcement during preparation and
  chooses the next holder from the current peer view after a transfer fails, rather than from a
  frozen snapshot. Fleet rounds follow changes in release facts rather than each transfer chunk;
  authenticated part mail reaches transfer subscribers without queuing a durable peer-list rebuild
  for each part. A newly promoted main's started feed request gets a one-shot old-round grace
  before replacing its resident, without waiting for the feed's response. The preparation task
  settles its outcome before that switch, even if its ready-file event arrives first. The failover and
  alternate-holder progress cases cover these active paths.
  Wake-ups sleep until their next due time and watch their folders, with a minute deadline only
  for active watches or blocked work. Windows uses recursive watches on the stable wake-up roots
  rather than handles inside a removable bot; the bot deletion scenarios exercise moving those
  directories to Trash. Approval changes use live channels/file watches with a
  one-shot grace for initial conversation writes. Peer lists follow local and ownership changes;
  the old copy-standing heartbeat remains only with connected peers. The relay's departure push
  (`0x07`, already in the wire contract) marks the departing computer away rather than waiting
  for another send to be rejected; the member update-check scenario exercises that push. Letters follow the outbox,
  replies and peer changes; retries only run while work is kept. A release admission announcement
  is sent before the newly admitted peer work. The Library uses the resident's existing engine
  event feed instead of opening another one. Existing `bots`, `engine-sessions`, `library`,
  `peers`, `moves`, `work-elsewhere`, and `updates` cases exercise these paths; the quiet member
  release case compares the signed preparing row after the peer's target is known.

- **Setup lease and Work Queue numbers:** a setup with just its own computer no longer reads or
  renews a relay lease. A live installer invitation starts the lease, and enrollment is fenced
  until its relay high-water mark covers the local number ledger, including the persisted number
  reservation, used and granted ranges, pending reservations and merge allocations. Solo numbering
  uses a durable local ceiling, with the last relay floor carried forward after removing the final
  peer. That removal first confirms the changed roster at the relay; a missing or unconfirmed
  roster observation keeps the fence. With members, the main renews twenty seconds before expiry
  (normally about every forty seconds). Authenticated change mail prompts an authoritative read;
  a missed hint or an older main is recovered at lease expiry, with takeover and transfer deadlines
  retained. Real pending setup edits, handovers and removal deliveries still retry; no quiet
  setup maintenance round runs. `two-boxes` checks that solo startup, numbering and restart do
  not call the relay and that a first invitation carries a reserved merge range before both
  computers allocate distinct numbers. `adding`, `ssh-adding`, `removal`, `setup-judge`,
  `work-queue` and `updates` keep the enrollment, fencing, failover and release contracts. Fixtures
  that waited for a solo lease before an invitation now wait after the invitation; fixtures that
  only number locally retain the wait for the TypeScript reference or an already grouped roster.
  The pair case also checks that no relay calls occur in a settled three-second interval and that
  the final unlink is fenced at the relay before solo numbering resumes. The backup fixture uses
  a current approval record with `since` so independent normalization does not race its exact-byte
  assertion.

The conformance harness creates private NTFS homes before testing installer rejection or stopped
uninstall. Windows Node's forced process termination exercises the stale discovery-record restart
case, rather than the Unix graceful shutdown assertion. The vault plaintext fixture tolerates a
disappearing file during enumeration and Windows's lock on the transient Insights WAL index;
it still checks the database, WAL and other files. Seen once in the cross runs: the Codex repeated
Autopilot question and the native Windows move's withheld-part resend; both passed on focused
reruns (the latter completed the full lost-part and switch recovery in 177 seconds).

Mac baseline (debug binary, isolated home with a signed-in scripted Codex, one computer, local
relay stand-in). For each sample: three seconds to settle, then eleven `top -l` samples a second
apart; IDLEW differences divided by ten. RSS is `ps`'s resident pages, including shared pages,
not PSS. The connected sample includes a persistent line plus model and account requests.

| Run | No client: RSS / processes / idle wakes/s | One client: total RSS / processes / idle wakes/s |
|---|---|---|
| Before 4.8, `27ed3c8e6` | 79,904 KiB / 1 / 2.8 | 144,720 KiB / 3 / 8.2 |
| After the agent catalog and program changes | 77,440 KiB / 1 / 0.0 | 81,984 KiB / 1 / 0.0 |
| After all eight idle changes (debug resident) | 80,800 KiB / 1 / 0.0 | 85,648 KiB / 1 / 0.0 |
| After all eight changes, with keeper as well | 99,408 KiB / 2 / 0.0 | 104,304 KiB / 2 / 0.0 |
| Final optimized (opt-level 1, stripped) resident | 48,096 KiB / 1 / 0.0 | 46,400 KiB / 1 / 0.0 |
| Final optimized resident and keeper | 61,024 KiB / 2 / 0.0 | 65,520 KiB / 2 / 0.0 |

The baseline's other two live processes were the holder (18,288 KiB) and its scripted app-server
(44,016 KiB); neither starts on the new route reads. The version command can run briefly when the
agents settings is opened, not at startup. The wake readings are short OS samples on a busy Mac;
zero reported idle wakes is not a count of all application timer callbacks. The final keeper sample
uses a lease-capable stand-in and records zero relay lease calls before and after connecting the
client, with no native agent launched. The direct resident is the like-for-like memory comparison:
its connected RSS fell 40.8%, and its unconnected RSS is within 1.2% of baseline. The keeper sample
is supplemental (there was no before-keeper sample). The separate optimized sample is the
same final code in the build used for SSH Add and cross-platform checks, not a like-for-like
comparison against the debug baseline.

Eight-change checkpoint verification (Rust, applicable scenarios; OS-specific scenarios are
skipped where their platform or optional fixture is absent):

| Area | Mac | Linux musl | WSL | Native Windows |
|---|---:|---:|---:|---:|
| Engine sessions | 81 | 81 | 81 | 81 |
| Library | 18 | 18 | 18 | 18 |
| Settings / bots | 17 / 40 | 17 / 40 | 17 / 40 | 17 / 40 |
| Adding / removal / setup judge / SSH adding | 7 / 7 / 2 / 1 | 7 / 7 / 2 / 1 | 7 / 7 / 2 / 1 | 7 / 5 / 2 / skipped |
| Two computers / number allocation and failover | 2 / 14 | 2 / 14 | 2 / 14 | 2 / 14 |
| Moves | 68 | 68 | 68 | 67 |
| Updates | 84 | 65 | 65 | 51 |

The repeatable Linux/WSL release-promote failure and the Linux terminal replay issue were
corrected and passed focused reruns; the Windows root watcher and fixture corrections passed
their affected areas. Mac conformance also ran the TypeScript reference, including engine
sessions 81/81 and the number allocation/failover cases 14/14. Workspace test checking and the
TypeScript check passed. The remaining 4.8 work inventories other idle timers and measures
actual timer callbacks, rather than relying on the operating system's idle-wake estimate alone.

## 7. Release decisions

1. **0.8.** Ship with the Rust box, including native Windows and WSL2.
2. **One box.** No public release mixes TypeScript and Rust boxes. The Rust box ports the move
   from 0.5–0.7.2 and 0.8.0-beta.1 and what their Macs still need; no TypeScript stepping stone.
3. **Light freeze on the TypeScript box.** Keep taking fixes and small features with scenarios.
   Start larger areas in Rust or wait.
4. **Windows signing.** 8.2 builds are unsigned. Buy the code-signing certificate or service just
   before release and wire it into the release pipeline then.
5. **The TypeScript code** stays in the repository until four weeks after the user's computers
   switched, then is removed.
