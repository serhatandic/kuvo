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
pins GPUI, and joining it would make every box build resolve and compile GPUI. Two small
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
| `kuvo_work` | Bots, wake-ups and watches, the Work Queue, group chats and seats. |
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
| 2.3 | Installer codes, fresh SSH and account reservation recovery, selected installer credentials, cancellation receipts, discovery, local seed intake, and signed unlink are implemented. Members forward installer, fresh SSH, and plain unlink requests. Re-enrollment compares retained work, including SQLite snapshots. Unix member and app-owned Mac wipes, the last native Windows computer's wipe and reinstall, configured disk checks, local Unix uninstall and managed uninstall from a saved SSH address have conformance checks. SSH credential transfer, legacy remote uninstall, and the remaining wipe and re-enrollment cases are open. [Coverage](rust-adding-coverage.md)<br>Adding (SSH, installer code, seed), unlink and wipe, removal, re-enrollment. Admission, roster changes, removal and re-enrollment from `setup-merge.test.ts`, `setup-two-boxes.test.js`, `setup-judge.test.js`, `setup-mac.test.js`, `documents.test.js`, `devices-two-boxes.test.js`, and `setup-handover.test.js`.<br>Also covers `peers.test.js` and `devices.test.js`.<br>2.3 cannot close until setup merge reserves Work Queue numbers on the current relay lease with a durable source-bound approval, then connects the queue import, retained aliases and Markdown reference rewrite. | setup | 2.1 | 5 ×2 | A |
| 2.4 | **Done.** Ordinary OpenCode chat moves carry queues, wake-ups, files, notes and replay fences. Durable receipts and authenticated owner requests support transfer retry and cleanup. Letters, delayed notes and transcript replies follow the owner. The owner snapshot follows a move, receipt pages keep their cursor across issuer replacement, unlink and restore preserve the ownership fence, and moving or moved conversations fence session actions, live ask replies and attachment retention. Verified across macOS, WSL2 GNU, static Linux musl and native Windows. [Coverage](rust-moves-coverage.md) Ownership and receipt cases also come from `offline-merge.test.ts` and `setup-handover.test.js`.<br>Inherits `setup-lists.test.ts`, `peers.test.js`, `owner-mutations.test.js`, `mac-peers.test.js`, `reach-elsewhere.test.js`, `reach-elsewhere-rules.test.js`. | peers | 2.2 | 5 ×2 | C |
| 2.5 | **Done.** Version-one encrypted archives, disk, SSH and Relay recovery, journal fences, held-work review, vault routing, and pre-switch and wipe hooks. [Coverage and owner phases](rust-backups-coverage.md) | setup | 2.1 | 6 | A |
| 2.6 | **Done.** Released Mac and machine homes reach layout 23 through a resumable move. Both tagged Mac destinations match, and the credential move is verified through the Helper interface stand-in. [Coverage](rust-move-coverage.md) | move | 1.2, 2.1 | 6 ×2 | C |
| 3.1 | **Done.** On-demand OpenCode, idle parking, process adoption, and authenticated HTTP and PTY passthrough. See [coverage and remaining cases](rust-engine-coverage.md). | engine | 1.4 | 6 | B |
| 3.2 | **Done.** Sessions, queues, send-once receipts, asks, waiting, models and search. [Coverage and remaining owners](rust-engine-coverage.md#implementation-and-remaining-parity-work). | engine | 3.1 | 6 ×2 | B |
| 4.1a | **Done.** Rust holder command, private IPC, journal, recovery records and shared Rust controller. [Coverage](rust-agents-coverage.md) | agents | 1.3 | — | B |
| 4.1b | **Done.** Resident agent runtime and holder adoption through keeper updates, roster, accounts and sign-in, session and model integration. Authenticated resident and keeper proof is owned by 4.4; the carried cases are named in the [coverage](rust-agents-coverage.md). | agents | 3.2, 4.1a | — | B |
| 4.1c | **Done.** Tools relay, `mcp` mode, shared RPC and the Bun sidecar installer. Every resident gives sandboxed or held agents the forwarder, Macs included. Adapter rows pass the relay to their factory; 6.1 brings the sandbox launcher, 6.4 the browser tools program and engine-own-tools. [Coverage](rust-agents-coverage.md) | agents | 4.1a | — | B |
| 4.2 | **Done.** Codex over its app-server: account, sign-in, models, turns, asks, background terminals and holder adoption through keeper updates, with the shared program updater and its turn hold. It also serves the `kuvo/agents/` routes the app uses that had no Rust route: Settings › Accounts' rows, install, program updates and notices, a conversation's run state, read and model, and the agents' feed (`kuvo/agents/api/event`). Every `codex.test.ts` case is ported or has an owner. Passes on macOS, Linux, WSL and native Windows, and live on GPT-6-Luna across a keeper switch (macOS). [Coverage](rust-agents-coverage.md#codex-42) | agents | 4.1 | 5 | B |
| 4.3 | **Done.** Grok and Antigravity over ACP, with extensions and the Antigravity legacy launch: one shared ACP client carrying several sessions per process under one holder, verified on all four platforms and live on Antigravity (Gemini 4 Hydrogen); Grok's live turn stays pending (no subscription). [Coverage](rust-agents-coverage.md#grok-and-antigravity-over-acp-43) | agents | 4.1 | 7 | C |
| 4.4 | **Done.** Claude Code over stream-json and control stdio. 4.4a covers account checks, sign-in, legacy account migration and model discovery. 4.4b covers turns, holder handoff, steer receipts, refused sign-ins and the live keeper proof (macOS and WSL). Both pass on macOS, Linux, WSL and native Windows. Per-scenario parity for the remaining `claude.test.ts` groups and the complete home comparison remain. [Coverage](rust-agents-coverage.md#claude-code-44) | agents | 4.1, 0.3 | 6 | B |
| 4.5 | Cursor through the Bun sidecar for runs, account/sign-in and model catalog | agents | 4.1, 0.3 | 3 | C |
| 4.7 | OpenCode as one more coding agent, on 4.3's ACP client ([design](opencode-agent.md)). 4.7a: **done**, ACP: it carries everything the app shows; a message sent mid-turn originally waited for the turn's end, as for Grok and Antigravity (superseded by 4.11) ([decision](opencode-agent.md#1-how-the-box-runs-it-acp-one-process-per-computer)). 4.7b: **done**, new conversations on OpenCode's models run on the OpenCode agent, one process per computer, and opening the app starts no OpenCode ([coverage](rust-agents-coverage.md#opencode-as-a-coding-agent-47b)). 4.7c: **done**, the app's terminals are the box's own (`kuvo_os::pty`, `api/pty` in `kuvo_services`) at today's paths and frames, and nothing runs for them until one is opened ([coverage](rust-agents-coverage.md#the-boxs-own-terminals-47c)). 4.7d: existing conversations listed, read and continued (**done**), bots on OpenCode's models on the agent (**done**, [coverage](rust-agents-coverage.md#opencodes-older-conversations-and-bots-47d-in-progress)); then, after 4.7g, the engine place removed: every `Place::Engine` branch, the engine's start triggers (worktree making moves to the agents' path) and the `api/…` passthrough except for released 0.7.x Macs (5 h). Bots on OpenCode's models move to the agent in the same switch (new ones still get engine conversations until then); 4.7d rewrites the four `bots` cases that make one ("made from a template waits", "told who it is… marked as its", Mac and machine) on the pinned OpenCode with the stand-in model. | agents | 4.3, 3.2 | 24 | B |
| 4.7g | The engine-* conformance areas (engine-sessions, -asks, -delivery, -models, -search, -typed, -worktree) on agent conversations: a stand-in ACP agent for the deterministic cases on both boxes, one live run on the pinned OpenCode. Their queue, receipt, delivery, asks, search and list features stay (`kuvo_sessions`); only the engine's start-up queue scan goes. Before 4.7d's engine removal. | agents | 4.7d (steps 1–4) | 6 | B |
| 4.6 | Subagents, reviews, side questions, carry and handover, snapshots, transcripts | agents | 4.2–4.4, 4.7 | 4 ×2 | B |
| 4.8 | **Done.** [Idle work](#48-idle-work): lazy agent discovery, version checks and artifact store; event-driven Library, Marketplace, sessions, Settings, keeper, work and peer services. Demand-driven setup leases with fenced shared numbering. A shared native watcher and lossless relay backpressure. Mac, Linux, WSL and native Windows verified; zero idle application timer callbacks. | all | 4.2, 4.4, 5.5, 6.1, 7.1 | — | B |
| 4.9 | **Done.** [Background work priority](rust-agents-coverage.md#background-work-priority-49): agent process trees use utility QoS and nice 20 on Mac, nice 15 and idle I/O where allowed on Linux, and below-normal jobs on Windows; resident, keeper, holders and user-opened terminals remain ordinary. New background work waits for critical memory pressure to clear, with cancellable agent starts. No live promotion: undoing nice requires privilege and would give new children the wrong class. Mac timing and priority readback, WSL and native Windows verified. | all | 4.8 | — | B |
| 4.11 | **Done.** Agent letters default to Send Now; explicit next-step delivery fires at a real agent step, with native steering or the shared cancel/resume path, and explicit after-turn delivery waits. Verified on macOS, static Linux musl under WSL, and native Windows. Cursor adds the same scenario in 4.5. [Coverage](rust-agents-coverage.md#411--letters-now-and-next-step-delivery). | agents, work | 4.1–4.4, 4.7, 5.1 | — | B |
| 5.1 | **Done.** Bots: making (on the engine and on coding agents, from New Bot and Add Agent), changing, deleting, telling and settling them, held edits for your other computers and those computers' bots; wake-ups and watches fired by the resident, for the engine's conversations and the coding agents'; approvals, work settings, oversight, notices and their holds, the work watcher, Stop and Stop All, work for leads on your other computers, the agents' work tools, wake tools and letters; and the app's routes from the audit (`kuvo/asks`, shell output and Stop, Add Agent). Verified on macOS (both boxes), static Linux musl under WSL2, and native Windows. Not ported: offline bot setting edits, with the record-edit adapters (section 5). Open elsewhere: an engine bot switched to a coding agent's model (4.6) and group chats as a bot is deleted (5.3). [Coverage](rust-work-coverage.md)<br>Bots, wake-ups and watches fired by the resident, notes, attention, approvals. Bot retell and wake consumers from `personalities-box.test.js`, `time-zone.test.ts`, `time-zone.test.js`, and `setup-handover.test.js`. Bot templates in the personality route fixtures.<br>Also covers `setup-lists.test.ts`, `notices-rule.test.ts`, `notices-plain.test.ts`, `mac-peers.test.js`, `devices.test.js`.<br>Use `LocalSessions::activity()` for the local OpenCode rows in background work and peer activity. | work | 3.2 | 5 ×2 | C |
| 5.2 | **Done for the resident contract.** Work Queue: store, numbers and lease, labels, statuses, peer exchange, GitHub. Verified on Mac (both boxes), native Windows and static Linux musl under WSL. [Coverage and integration dependencies](rust-workqueue-coverage.md) Queue cases from `setup-shared.test.ts`, `setup-merge.test.ts`, `work-queue-numbering.test.ts`, `offline-merge.test.ts`, `setup-two-boxes.test.js`, `documents.test.js`, `owner-mutations.test.js`, and `setup-handover.test.js`.<br>Also covers `setup-lists.test.ts`.<br>Reuse 3.2's `kuvo_peers::memory_read` transport for authenticated read-only requests without a disk spool.<br>2.3 cannot close until setup merge reserves Work Queue numbers on the current relay lease with a durable source-bound approval, then connects the queue import, retained aliases and Markdown reference rewrite. | work | 2.2 | 5 ×2 | C |
| 5.3 | Group chats and seats. Group clock consumer in `time-zone.test.ts`.<br>Also covers `mac-peers.test.js`. | work | 4.6, 2.2 | 4 ×2 | B |
| 5.4 | **Done.** Usage, limits and Insights: accepted plan readings, signed settings, reset spending and permissions, merged notices and history, pricing, and incremental SQLite indexes with watches and cursors. All ordinary-use and common-failure cases in the [coverage map](rust-usage-coverage.md) are proven on macOS, capped Linux musl, WSL and native Windows; individually listed rare cases are deferred until after 0.8. Also covers `notices-usage.test.ts`. | services | 4.1, 2.2 | 5 ×2 | C |
| 5.5 | **Done.** Library, artifacts, attachments, setup-list peer views (cases 17, 23, 28), marketplace, integrations, skills, MCP catalog and `kuvo/programs` ported to `kuvo_services` (plus additive `kuvo_setup` marketplace installations helper) and verified on macOS, Linux musl and native Windows. Live bot creation from Marketplace Add Agent stays with 5.1b; Settings pages stay with 6.1. [Coverage](rust-library-coverage.md) | services | 2.2 | 4 ×2 | A |
| 6.1 | Settings pages and flows, permissions. 6.1b serves the settings sections, forms, revision-guarded writes, events, Git migration, Custom Auto lists and peer archive copies/actions. [Coverage and remaining owner gates](rust-settings-coverage.md). Auto mode, its reviewer and the sandbox are postponed past 0.8 (user 11-25); every computer answers as one without the sandbox. | services | 4.1 | 5 ×2 | D |
| 6.2 | **Done.** (Mac wipe end to end is verified in 2.3; Helper supervision cases are 7.2's, browser and settings cases 6.4 and 6.1.) Vault: Helper Keychain, wrapped-key files, broker and SSH agent. The released move uses the TypeScript `security` path for custom Claude Code credentials, verified with a disposable Mac Keychain. Installed Helper and Keychain cases in `setup-mac.test.js` remain open. See the [coverage and remaining gates](rust-vault-coverage.md). | services | 1.3, 4.1, 6.1 | 6 | D |
| 6.3 | **Done.** Files, folders, projects, Git and Changes, worktree creation, session filing and branch instructions, naming, archive/removal/restore, Settings and project removal ported. Every called route is served; unreachable routes and deferred rare variants are named in the [Git and worktree coverage map](rust-git-coverage.md). [Files coverage](rust-files-coverage.md). Both boxes pass the ordinary shared cases on macOS, including the golden-home projects; scoped differential answers and records match. Rust verified on macOS, capped Linux musl, WSL2 and native Windows, including the live OpenCode worktree turn on macOS. | services | 1.4 | 5 ×2 | D |
| 6.4 | **Done for the resident browser contract.** Pinned Chrome for Testing download, CDP pipe, browser and show tools, settings/history/sites, live view, asks and downloads. Verified on Mac (both boxes), Linux with real Chrome, WSL and native Windows (browser unavailable). Remaining parity and scenario gaps, including passkey notices and Mac editing shortcuts, in the [coverage map](rust-browser-coverage.md). | services | 4.1 | 5 ×2 | A |
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
repeated question and the native Windows move's withheld-part resend; both passed on focused
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

**Debug timer measurement.** `scripts/measure-idle-box.mjs` uses a private fixture home, signed-in
scripted Codex, and real local relay connection. Build a debug executable, then run
`KUVO_BOX_BIN=<debug binary> scripts/heavy.sh box node scripts/measure-idle-box.mjs` (optionally
`--keeper` or `--follow`). It samples exact fixture PIDs after a three-second settle for 65 seconds
in each condition. The debug-only signal handler writes cumulative per-subsystem counters on
request; it does not create a recurring reporter. A counter records a completed timer future,
not timer registration/cancellation or ordinary file/channel/relay notification. Release builds
have no counters or reporter. Native watcher fallbacks on unsupported filesystems are a separate
platform mechanism, not application timer callbacks.

The counter baseline, after the original eight changes and the merged 5.1/5.4 work, is the
optimized debug build of `4d245022b` with instrumentation. It shows why the earlier short OS
idle-wake estimate alone was insufficient:

| Sample | RSS KiB | Processes | Timer callbacks in 65s | Callbacks/s |
|---|---:|---:|---:|---:|
| Resident, no client | 49,264 | 1 | 267 | 4.11 |
| Resident, one client | 54,144 | 1 | 283 | 4.35 |
| Keeper + resident, no client | 61,936 | 2 | 267 | 4.11 |
| Keeper + resident, one client | 66,656 | 2 | 283 | 4.35 |
| Resident, one client following hot pages | 55,504 | 1 | 391 | 6.01 |

The four largest sources were remote work (130 rounds plus 13 row checks), moves (65), Work Queue
exchange (32–33), and durable delivery (13), per 65 seconds. Smaller unconditional rounds came
from bot held edits, peer waiting, engine lifetime, sign-in expiry and daily backup checks.
Transport heartbeats are distinct: the local relay fired two, and the connected line thirteen.
The usage service's actual connected-client demand deadline fired once only with a client.
The direct fixture recorded zero native-agent starts; the keeper added no timer callbacks.
The hot-page sample follows sessions, bots, work, background and the Work Queue on one line;
the extra timer fires were background/work (32 each), bots and Work Queue follow (22 each).

**Work Queue exchange (follow-up).** The resident recovers once, then watches the queue's
owner files, numbers, labels, peer copies and pending edit outbox. A successful writer also
notifies the exchange immediately. Setup, session inventory, relevant peer routing and actual
lease reservation changes are events; a quiet lease renewal is not a reason to reread the queue.
The number request, unacknowledged snapshot, offline edit and handing each keep their own retry
or expiry deadline only while there is such work. A snapshot is computed on change and its
unchanged acknowledged edition is never retold. Bot descriptions are watched at their stable
parent, without holding removable child folders. The followed list uses those changes, the
actual next date/peer-standing boundary and due GitHub readings; it no longer reprojects every
three seconds. The GitHub read itself remains throttled by the visible issue's cache expiry or
the host's backoff, and a read's completion notifies followers. The new Rust debug conformance
case both proves that an empty queue has no two-second exchange timer and writes an unnumbered
item outside a route to prove the file event numbers it. It failed on the instrumented baseline
(two exchange callbacks in the observation window). After the change the 65-second no-client
sample had 235 callbacks (3.61/s, 48,864 KiB), and the client following hot pages had 336 (5.17/s,
55,632 KiB). Both Work Queue counters were zero in both samples, compared with 32–33 exchange
callbacks and 22 follow callbacks before. There was still one resident process and no agent start.
The Mac Work Queue suite passed 147 cases in its first run; the one failing explicit GitHub
refresh case and three related cases passed after restricting the passive lock retry delay to
followers (an explicit read can immediately try a newly freed lock). TypeScript passed 145,
with three conditional skips. Both new watcher/deadline unit cases passed.

**Remote work (follow-up).** The call owner recovers once, then responds to actual tool calls,
row publications, relevant relay mail, routing/ownership changes and changes to durable calls
or kept results. A dropped tool waiter notifies its owner. The wait for a tool answer, retries
of outstanding calls or unacknowledged rows/answers, and expiry of retained results each have
their own real deadline. An away computer still receives the oldest pending call first; an
acknowledged unchanged row has no retry. No calls, results or remote rows means no timer.
The row producer no longer probes engine file times or rereads all sessions each minute: it
uses local session/engine events and work/move/home record watches, reads no engine at all when
there are no peers, retains the last successful rows if the engine is unavailable, and retries
only an actual failed read of an otherwise running engine. The Rust debug conformance case
proves both timers park when there are no calls or remote leads. The old binary fired thirteen
round callbacks in 6.5 seconds and the replacement fired none.

The existing `work-elsewhere` Auto refusal wording case is inherited, owned by the wording pass
(the earlier Rust binary already says “is in Ask First”, not the frozen TypeScript Auto wording;
Auto is postponed for 0.8). Neither its fixture nor assertion is changed by this work.
Mac conformance: both mixed and all-Rust `work-elsewhere` runs pass four of five (the single
inherited refusal above); TypeScript passes four with the debug-only case skipped. All three
`agent-tools` cases pass against both boxes. The away-peer retry case also checks its precise
deadline and that removing both pending calls removes the deadline. The 65-second measurement
now has 92 callbacks without a client (1.41/s, 49,392 KiB) and 193 with hot-page follows
(2.97/s, 56,160 KiB), down from 235 and 336 respectively. Both remote-work counters are zero,
there is one resident process, and no agent process starts.

**Move driver (follow-up).** One recovery pass on startup, then transfer commands, move mail,
local journal/decision changes, engine state and relevant peer routing changes wake the driver.
The source's check, offer, switch, cleanup and drop retries; destination recovery/held-copy
expiry; waiting badge thresholds; missing part window; orphan staging and retained backup expiry
are individual deadlines only when such work exists. A destination that is known away is not
asked for its retained backup's run proof until it returns. A part or its acknowledgement
advances the current window immediately, not a full scan of unrelated transfers. Native journal
watch notifications trigger recovery on external writes or missed notifications. The evidence
index advances in bounded batches only while an actual requested index is incomplete. The new
debug case tests an empty transfer journal directory and the deadline unit cases test the
missing-part retry and one waiting badge transition. The old binary fired three round callbacks
in 3.5 seconds; the replacement fired none. The 65-second measurement dropped from 92 to 27
callbacks without a client (0.42/s, 50,080 KiB) and from 193 to 128 with hot-page follows
(1.97/s, 57,296 KiB); the move round counter was zero in both, one resident and no agent starts.
The two deadline cases pass, as do the lost transfer window, restart, held-copy, switch and
cleanup cases. Mac moves: Rust 68 passed, one conditional skip, one inherited failure; TypeScript
65 passed/four conditional skips. The inherited receipt-page boundary case returns 409 from
`backups/match`, before any transfer journal exists; the instrumented `4d245022b` baseline
already fails identically, and the original checkpoint passes. An exact `dab93b674` build
also fails: while its setup lease is fresh, the setup actor panics in historical Insights
origin projection (`model.rs`, indexing an absent `removals` field). The 5.4 owner is fixing
the optional field lookup; this case is not a move-driver regression. The fixture now includes
its process log and lease expiry in the failure diagnostic.
The setup-owner fix `01bf21c8e` has since been merged: the same receipt-page boundary case
passes on the delivery checkpoint, as does the held-copy durable receipt case.

The peer move notice publication also has its own monotonic one-minute deadline only while
there are active moves. Progress bytes alone still do not announce a new stage. An explicit
agents refresh sends the current move copy immediately, even if it is empty; a reconnect
retells once, and an unchanged cleared list has no deadline. Unrelated agent/copy heartbeats
no longer reopen the move journals. The relay conformance case advances elapsed virtual time
as well as its injected wall clock, and verifies minute progress, immediate request, clear and
reconnect behavior (it previously waited for a removed periodic round). The undriven transfer
fixture also advances elapsed time, and the incomplete-copy fixture now delivers the relay's
connection event to the resident rather than consuming it itself. All fourteen selected move
relay cases pass.

**Durable delivery (follow-up).** One recovery reads the saved outbox on startup. A write or
watch notification wakes the worker; session/engine state, accepted setup grants, current
owner and peer reach are changes rather than a new inventory every five seconds. An entirely
held or known-away owner waits for a routing event; an actual unsuccessful contact or durable
write has a retry deadline, preserving the same per-conversation order and handoff fence.
Receipt follows react to receipt and authority changes; the local delivery queue follows its
store, engine state and queue records rather than a one-second fallback. The old binary's
debug idle case fired a recovery callback in 6.5 seconds; the replacement fired none. Mac
delivery conformance is five of five on Rust, four of four on the TypeScript reference (the
debug-only assertion is skipped there). Addressed receipts, take-back recovery, restart, Steer
and Send Now retain their answers.
The whole Mac engine-session conformance area remains green (81/81 Rust).

**Remaining idle and followed-page owners.** Held bot edits, engine supervision, queue limits,
late work answers, peer waiting, retained sign-ins, native agent holders and tools, Git,
backup and skill expiry, Vault SSH listeners and followed work/bot/engine/Library lists now
use store, session, reach, process and native filesystem changes. Only an actual expiry,
work retry or compatibility requirement schedules another callback. A pristine resident does
not open the artifact store or write an empty background inventory; the first real artifact
use opens it, including a genuine edit baseline. The new conformance cases prove that empty
resident and followed-page owners park and that an external background record, oversight
expiry, and disappearing Library source file still update an active view. Existing route
validation and refusal results are unchanged. A released computer may open empty artifact
infrastructure for the first time after unlink without that read becoming new retained work.
Both boxes retain the historical baseline and compare an additional empty-store variant only
when the store has no real artifacts. The re-enrollment case refuses an actual late artifact
record, then rejoins after its removal. The disposable-index rewrite case explicitly opens
storage before unlink; it no longer relies on eager startup. The mixed Rust-departure/TS-rejoin
case fails on the previous TS binary and passes with this fix.

The [timer inventory](rust-idle-timers.md) gives each owner's actual deadline or event and the
remaining deliberate compatibility/transport waits. It also identifies the platform
process-launch seam for the next priority pass.

**Final Mac sweep measurement.** Same optimized debug profile, isolated fixture, 65-second
windows and scripted native-agent account as the baseline above. The initial counter baseline
did not yet label the old queue-limit round (one missed count per 65 seconds), or the old
five-minute late-answer and command-server expiry rounds (outside the sample window); the
final counter covers those branches as well. Baseline figures are the observed counts,
not an estimate of those omitted callbacks.

| Sample | RSS KiB | Processes | Timer callbacks in 65s | Callbacks/s |
|---|---:|---:|---:|---:|
| Resident, no client | 51,952 | 1 | 2 | 0.031 |
| Resident, one client | 53,776 | 1 | 16 | 0.246 |
| Keeper + resident, no client | 65,232 | 2 | 2 | 0.031 |
| Keeper + resident, one client | 70,176 | 2 | 16 | 0.246 |
| Resident, one client following hot pages | 58,224 | 1 | 16 | 0.246 |

All measured application-service callbacks are zero in the quiet fixture; the two no-client
callbacks are relay keepalives. A connected line adds 13 wire heartbeats and one actual
usage demand deadline. Following hot pages adds no periodic work. No agent or engine process
starts in any sample. The resident-only raw timer count falls 267 to 2 (99.25% fewer), and
the hot-page count 391 to 16 (95.9% fewer), including the retained transport callbacks.
RSS is measured, not inferred: the resident-only no-client sample is 2,688 KiB higher than
the optimized baseline, while the connected resident is 368 KiB lower. The keeper and resident
combined connected sample is 3,520 KiB higher. A separate paired `vmmap -summary` after the
same three-second startup settle measured RSS 48,944 to 52,256 KiB, resident executable/shared
text 165.1 to 166.8 MiB, resident stack 1,216 to 2,032 KiB (37 to 64 stack regions), and malloc
resident 11.1 to 11.8 MiB (allocated bytes 8,123 to 8,704 KiB). Private physical footprint was
15.9 to 17.5 MiB. The extra memory is therefore consistent with larger mapped code and the
native watch subscriptions, stacks and tables, not a multi-megabyte eager catalog cache.
The artifact database and page subscriptions already wait for use; the resident's remaining
watchers must be ready for external writes. The shared watcher change below removes duplicate
native streams without restoring polling. All fixture processes exited.
The final merged binary, including the retained-work fix, repeated the keeper sample: 65,232
KiB without a client (2 callbacks) and 72,128 KiB following the hot pages (16 callbacks), with
zero agent starts, engines, or setup lease calls. The exact owned fixture PIDs all exited.

The full Mac sweep passed the Work Queue (148/148), moves (70 passed, one mixed-binary
skip), engine sessions (81/81), delivery (5/5), and the new resident (5/5) and Library
checks (20/20). The shell-keeper update case saw the up-to-date row before its peer SHA
**seen once**, then passed on the focused rerun with the same binary. The final external-record
follow case also timed out **seen once**, then passed in 8.3 seconds on the same binary. The inherited
Auto-mode refusal wording is owned by the wording pass, not a timing regression. The
peer-announcement unit fixture advances the actual elapsed copy-heartbeat deadline as well as
its injected wall clock, and checks the announced chat project already present in the contract.
The stale first-inventory conformance still requires exactly two reads on Rust; TypeScript can
filter its in-flight page using the deletion fence and reread only the affected ID, so it may
complete in one or two whole reads. Both assert the same no-resurrection answer.

**Combined platform gate.** Tests run from isolated fixture homes; the Mac runs the TypeScript
reference and Rust, and the cross executables run as the Rust resident and Rust peers. The
initial Linux/WSL peer and three verified-process SSH fixtures had no Bun runtime; repeating
with an explicit Rust lead and the isolated fixture Bun runtime proved the actual cases.

| Area | Linux musl | WSL musl | Native Windows |
|---|---:|---:|---:|
| Resident / delivery / Work Queue | 5 / 5 / 148 | 5 / 5 / 148 | 3 (+2 debug skips) / 4 (+1 debug skip) / 146 (+2 skips) |
| Work elsewhere / agent tools / moves | 4 (+1 inherited Auto) / 3 / 70 (+1 skip) | same | 3 (+1 inherited Auto, 1 debug skip) / 3 / 68 (+3 skips; focused receipt-index rerun green) |
| Bots / held edits / engine sessions / asks | 40 / 1 / 81 / 9 | same | same |
| Library / Vault / backups / Git / holder | 20 / 9 (+3 skip) / 9 / 15 (+1 skip) / 12 | same | 20 / 6 (+6 skips) / 7 (+2 skips) / 13 (+3 skips) / 12 |
| Codex / Claude / Claude turns / Grok / ACP | 99 / 20 / 58 (+1 TODO) / 8 / 10 | same | 99 / 20 / 57 (+1 skip, 1 TODO) / 8 / 10 |
| Agent updates / session reads / settings / peers | 10 / 4 / 17 / 2 | same | 9 (+1 skip) / 4 / 17 / 2 |
| Adding / removal / setup judge / SSH Add / two-boxes | 11 / 19 (+4 skip) / 5 / 24 (+1 skip) / 3 | 10 (+1 unresolved) / 19 (+4 skip) / 5 / 24 (+1 skip) / 3 | 11 / 11 (+12 skips) / 4 (+1 skip) / 4 (+21 skips) / 3 |
| Updates / Helper menu | 64 (+37 skip, 1 inherited wipe timeout) / 4 | same | 50 (+51 skips, 1 inherited wipe timeout) / 4 |

The Grok question fixture on WSL read an incomplete trailing JSONL record **seen once**;
the full focused Grok rerun passed 8/8. Native Windows's immediate local Work Queue
handoff/update history case returned no item **seen once**; two focused reruns passed. The
Windows cold move receipt-index retry failed repeatedly, with eight unread records remaining
after both attempts. The diagnostic checkpoint showed identical directory identity and write
time but a newer Windows ChangeTime after startup reapplied ACLs. Index revisions now use the
existing platform retained-change stamp: Unix inode change time, Windows directory write time
plus identity. Receipt entries are atomically replaced, so real additions, deletions and
replacements still invalidate the bounded scan. A checkpoint written before this correction
may do one bounded rebuild; it no longer repeats one after each restart. The native focused gate
passed all three receipt/replay/forwarding cases. The new 2.3e standalone wipe preparation takes the
wipe lock before it marks the resident ready to stop; the earlier installer test deliberately
holds that lock until the resident exits. Both Linux and WSL reproduced the inherited timeout;
only the exact fixture-owned resident PIDs were terminated. 2.3e owns the fix (`8dcb3c7af`,
old binary red and new binary green on Linux). **Excluded on native: inherited 2.3e wipe lock,
being fixed in 2.3e** (lead-approved acceptance-gate exception). Node 26 still executed the
case because the negative test-name filter matched its ancestor; the same timeout occurred,
and only its exact fixture-owned PID was terminated. All 50 other applicable cases passed.
The provisional-installer retirement test on WSL failed at several stages (observer
replication, former issuer retirement, and replacement enrollment); Mac and Linux passed
11/11. The same WSL fixture passed all three initial runs of the adding owner's binary and
failed all three runs of the idle sweep binary. During a further run, the latter's residents
held 37, 12 and 49 inotify instances, with the user-wide total reaching 120 of the Linux
default limit of 128. The control binary peaked at 61 and passed.

**Shared file watches.** Each runtime now owns just one native watcher, lazily opened on
first subscription (one inotify instance on Linux, one FSEvents stream on Mac, one Windows
notification registry), and one shared polling fallback only for unsupported paths. Logical
folder subscriptions retain their own relative-path filtering and reconnection grace. A
recursive ancestor covers its descendants; on release, independently subscribed children are
rearmed before reporting a rescan, not lost when inotify removes the ancestor's descriptors.
Root changes and native errors invalidate registrations, so a retry watches the new folder
rather than a removed inode. FSEvents path updates are batched; affected subscriptions get a
rescan when registration changes can lose changes. The isolated OS scenario subscribes to
140 distinct roots, asserts just one additional /proc/self/fd inotify descriptor on Linux,
and proves notification routing and child coverage before and after a recursive ancestor is
added and released. The new scenario and existing watch/reconnect/poll scenarios pass on
Mac, and the 140-root scenario passes on Linux, WSL and Windows; sampled WSL residents
held at most one inotify instance even in the five-computer retirement case.

The shared watcher sample (65 seconds, same fixture) records resident RSS of 49,696 KiB
without a client and 54,544 KiB with one, versus 51,952/53,776 KiB before sharing.
Keeper plus resident uses 63,488 KiB alone and 69,904 KiB following the hot pages
(previous final sample 65,232/72,128 KiB). The timer counts remain 2/16:
two relay protocol heartbeats, and with a client 13 line heartbeats and one usage demand
deadline. No idle application-service timer, agent start, engine or solo lease request.
The original timer baseline was 267/283 callbacks in the same interval. In a paired
three-second vmmap sample the resident has 19 stack regions (768 KiB resident), down from
36 (1,280 KiB) in the timer baseline and 64 (2,032 KiB) before watcher sharing.
Physical footprint is 15.5 MiB versus 16.0 MiB baseline; malloc remains 11.1 MiB,
so the remaining RSS difference is mainly mapped code, not an eager data cache.
All fixture processes exited.

A native watch reconfiguration can require a real rescan, but it must not invalidate
unrelated engine inventory. Project/background file notices reproject the saved inventory;
agent notices compare the actual agent rows before invalidating it. The Mac refiling case
failed twice with the first shared watcher (two unnecessary engine reads), then passed
focused and engine-sessions 81/81 after separating the projection. On Windows a recursive
ancestor also reports metadata changes to a logical watched directory; these are not
changes to its named children. The isolated setup test clock now wakes the lease only
when its actual offset changes, and production has no test-clock watch.

**Relay pressure.** A single native watcher removed the Linux instance pressure, but the
WSL retirement case still failed with one inotify descriptor per resident (also when
pinned to one CPU). The diagnostic relay recorded 34 installer retries successfully
written by the applicant, all to an offline issuer, although its HTTP lease still renewed.
The relay's bounded durable-event queue formerly ended the entire connection owner when
its 128 slots filled. The real loopback regression filled it with authenticated peer mail:
the old owner closed on event 129; the new owner parks input until consumer capacity,
preserves queued event order, and continues trust/mail/close commands so a consumer
awaiting its own command cannot deadlock. A new session start during backpressure receives
a transient busy answer instead of adding an unbounded backlog. An actual socket failure
drains accepted events, resets sessions and reconnects, servicing offline commands meanwhile.
The new pressure case drains 130 ordered events and sends again on the same socket.
Peer publication also coalesces the latest projection without losing explicit per-peer
refresh, Connect, move and reconnect intents. Connect waits for its published state before
answering 202; a cached remote read's background reach remains best-effort.

The TypeScript relay is driven directly by WebSocket callbacks and has no corresponding
bounded event owner. The other Rust bounded-queue exits examined were deliberate
connection-local backpressure (SSH agent), slow line attachment detachment with numbered
replay, or an actually closed consumer; durable holder output pumps from its journal.
Dirty wake tokens are coalescible. The Work Queue exchange, remote work, move driver,
delivery recovery and letter outbox retain actual outstanding retries and rearm on
receipt/route changes; there is no need to restore their idle rounds. The peer update sender clears its saved request after transport acceptance, so its
bounded fleet mailbox now coalesces a shared check and the latest retry per peer
until capacity is available, without blocking pack replies.

**Final relay and watcher gate.** The former WSL retirement failure passed three
consecutive times after relay backpressure recovery, then three further times with the
final watcher and inventory corrections. Every sampled resident held at most one
inotify instance; the final three samples had 16, 14 and 19 observations. The full
adding, removal, two-computer (including quiet lease and numbering), peers, resident,
engine-sessions (81/81), delivery, library, Settings, setup judge, SSH adding, receipt
index, Work Queue follow and update check/retry/wipe gates passed on WSL. Native Windows
passed those areas plus engine asks and bot/held edits, with the standalone wipe case
now passing after the removal owner's fix. Native update check/retry had three
applicable cases and one Unix-only skip. Linux passed the same pressure and setup
areas; the final resident and engine inventory retests passed on Linux and WSL.
Mac passed engine-sessions 81/81 on both boxes, the Rust relay tests (28 applicable,
one live mixed-box test ignored), pressure and pending-check regressions, and the
final initial-inventory case three additional times.

A rescan of unchanged agent metadata during the *first* in-flight engine page now
compares against a separately retained, sorted agent projection, shared by reference
with the complete inventory. The deletion fence still requires exactly two engine
reads in the initial inventory case. Linux reports a completed in-place write as
an access notification; the shared watcher preserves this close-write event while
still ignoring ordinary reads and opens. Its scenario consumes create and modify,
then requires the completed-write notification. The external-record follow fixture
also used to overwrite the live engine's place during initial publication; it now
acts as a separate writer under the same background lock and uses atomic replacement,
keeping the other places intact. The Work Queue follow fixture waits for the intended
title change instead of mistaking an intervening numbering/liveness update for it.
Neither change relaxes the required visible answer. The external-record scenario
passed three focused Mac Rust runs and its TypeScript reference, and the full final
resident area passed on both Linux environments.

The final Mac sample repeats the instrumented 65-second conditions:

| Sample | Before RSS KiB | Final RSS KiB | Processes before → final | Timer callbacks before → final | Final callbacks/s |
|---|---:|---:|---:|---:|---:|
| Resident, no client | 49,264 | 50,256 | 1 → 1 | 267 → 2 | 0.031 |
| Resident, one client | 54,144 | 47,216 | 1 → 1 | 283 → 16 | 0.246 |
| Keeper + resident, no client | 61,936 | 63,808 | 2 → 2 | 267 → 2 | 0.031 |
| Keeper + resident, one client following hot pages | 66,656 (plain client) | 69,904 | 2 → 2 | 283 → 16 | 0.246 |

The final two idle callbacks are relay keepalives; the line adds thirteen wire
heartbeats and one connected usage demand. No application-service timer fires, no
agent starts, and no idle engine. Resident-only unconnected RSS rises 992 KiB in
this run; the connected resident uses 6,928 KiB less. The shared watcher reduces
stack regions and private physical footprint as measured above. RSS includes
shared mapped pages and varies between samples, so it is not a private allocation
measurement. All owned fixture processes exited; both PC process lists were clear,
its scratch and the cross target directories were removed, and the Linux test
container exited.

**Follow-on for 4.9 (background priority).** Most native programs use the common
`kuvo_os::process::Spawn` and its Unix/Windows platform implementations. Agent entry
points are `kuvo_agents`' Codex/Claude `program`, ACP `local`, `hold/worker` and
`hold/client`, `sign_in/command`; version checks and installers enter through
`program_updates` and `runtime_packages`. Engine processes use `kuvo_engine::launch`.
These seams distinguish starting a foreground turn from passive discovery: discovery
no longer launches a native agent, and background version checks have explicit demand
and daily scheduling. Direct platform-specific child commands, including Codex's
Keychain/sign-in helpers, should be reviewed separately by 4.9 rather than assuming
all subprocesses are spawned through the common wrapper.


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
