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
| `kuvo_engine` | OpenCode on demand, `api/…` passthrough (WebSocket and PTY included), sessions, queues, send-once receipts, asks, search. |
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
| Transcript resume, live reconnection and clear | **Works at the CLI.** `--session-id` then `--resume` reused the transcript. A second `initialize` on an open permission ask returned the same request in `response.pending_permission_requests` (beside the inner payload). Rust answered the redelivery and both queued UUIDs completed. The SDK handled that repeat with one permission callback. Reinitializing with an unanswered hook sent `control_cancel_request` for the old callback. `/clear` emitted `conversation_reset` and the next turn used the new session ID. Taking over a TypeScript holder's live turn is still to test (4.1, 4.4). |
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
| `moves-lanes.test.js` | Native agent move payloads, accounts and relay transfers. | Later, 2.4 and 4.6. |
| `moves-lanes-races.test.js` | Edits during packing, interrupted transfers and retained copies. | Later, 2.4 and 4.6. |
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
  Engine pins don't change. The feed holds no TypeScript build after 0.8.0-beta.1.
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
| `kuvo/data/owner`, `data/messages`, `data/edits*` adapters | 60 | No desktop caller. Storage stays. |
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
| 2.3 | Installer codes, fresh SSH, selected machine credentials, cancellation receipts, discovery, local seed intake, signed machine unlink, stopped standalone uninstall, and wipe of a standalone Unix resident without a keeper or configured backup are implemented. Members forward installer, fresh SSH, and plain unlink requests. Re-enrollment compares retained work, including SQLite snapshots. SSH recovery, release and credential integration, Mac Helper keys, member and managed wipes, and live and remote uninstall remain open. [Coverage](rust-adding-coverage.md)<br>Adding (SSH, installer code, seed), unlink and wipe, removal, re-enrollment. Admission, roster changes, removal and re-enrollment from `setup-merge.test.ts`, `setup-two-boxes.test.js`, `setup-judge.test.js`, `setup-mac.test.js`, `documents.test.js`, `devices-two-boxes.test.js`, and `setup-handover.test.js`.<br>Also covers `peers.test.js` and `devices.test.js`. | setup | 2.1 | 5 ×2 | A |
| 2.4 | Move journal and outcome reads, peer progress snapshots, manifest v0–v3, replay of committed switch receipts, and an OpenCode export and rollback seam. Live moves, handoffs and letters on `ownerOf` remain open. [Coverage](rust-moves-coverage.md) Ownership and receipt cases also come from `offline-merge.test.ts` and `setup-handover.test.js`.<br>Inherits `setup-lists.test.ts`, `peers.test.js`, `owner-mutations.test.js`, `mac-peers.test.js`, `reach-elsewhere.test.js`, `reach-elsewhere-rules.test.js`. | peers | 2.2 | 5 ×2 | C |
| 2.5 | **Done.** Version-one encrypted archives, disk, SSH and Relay recovery, journal fences, held-work review, vault routing, and pre-switch and wipe hooks. [Coverage and owner phases](rust-backups-coverage.md) | setup | 2.1 | 6 | A |
| 2.6 | **Done.** Released Mac and machine homes reach layout 23 through a resumable move. Both tagged Mac destinations match, and the credential move is verified through the Helper interface stand-in. [Coverage](rust-move-coverage.md) | move | 1.2, 2.1 | 6 ×2 | C |
| 3.1 | **Done.** On-demand OpenCode, idle parking, process adoption, and authenticated HTTP and PTY passthrough. See [coverage and remaining cases](rust-engine-coverage.md). | engine | 1.4 | 6 | B |
| 3.2 | **Open.** Local OpenCode sessions, queues, draft receipts, asks, waiting, model readiness and search have landed in 3.2a and 3.2b. See [coverage and remaining parity work](rust-engine-coverage.md#phase-32a-local-opencode-conversations). Session cases in `offline-merge.test.ts`, `owner-mutations.test.js`, `documents.test.js`, and `setup-handover.test.js`.<br>Inherits `setup-lists.test.ts`, `peers.test.js`, `devices.test.js`, `devices-two-boxes.test.js`, `start-elsewhere.test.js`, `start-elsewhere-rules.test.js`, `reach-elsewhere.test.js`, `reach-elsewhere-rules.test.js`. | engine | 3.1 | 6 ×2 | B |
| 4.1 | Holders (adopted across Rust-to-Rust updates; not TypeScript ones), roster, accounts and sign-in, tools relay, `mcp` mode, sidecar | agents | 3.2, 1.3 | 11 | B |
| 4.2 | Codex app-server | agents | 4.1 | 5 | B |
| 4.3 | Grok and Antigravity over ACP, with extensions and the Antigravity legacy launch | agents | 4.1 | 6 | C |
| 4.4 | Claude Code over stream-json and control stdio, with holder handoff and signed-in checks | agents | 4.1, 0.3 | 6 | B |
| 4.5 | Cursor through the Bun sidecar for runs, account/sign-in and model catalog | agents | 4.1, 0.3 | 3 | C |
| 4.6 | Subagents, reviews, side questions, carry and handover, snapshots, transcripts | agents | 4.2–4.4 | 4 ×2 | B |
| 5.1 | Bots, wake-ups and watches fired by the resident, notes, attention, approvals. Bot retell and wake consumers from `personalities-box.test.js`, `time-zone.test.ts`, `time-zone.test.js`, and `setup-handover.test.js`. Bot templates in the personality route fixtures.<br>Also covers `setup-lists.test.ts`, `notices-rule.test.ts`, `notices-plain.test.ts`, `mac-peers.test.js`, `devices.test.js`. | work | 3.2 | 5 ×2 | C |
| 5.2 | Work Queue: store, numbers and lease, labels, statuses, peer exchange, GitHub. Queue cases from `setup-shared.test.ts`, `setup-merge.test.ts`, `work-queue-numbering.test.ts`, `offline-merge.test.ts`, `setup-two-boxes.test.js`, `documents.test.js`, `owner-mutations.test.js`, and `setup-handover.test.js`.<br>Also covers `setup-lists.test.ts`. | work | 2.2 | 5 ×2 | C |
| 5.3 | Group chats and seats, autopilot. Group clock consumer in `time-zone.test.ts`.<br>Also covers `mac-peers.test.js`. | work | 4.6, 2.2 | 4 ×2 | B |
| 5.4 | Usage, limits, Insights (SQLite), pricing<br>Also covers `notices-usage.test.ts`. | services | 4.1, 2.2 | 5 ×2 | C |
| 5.5 | Library, artifacts, marketplace, integrations, skills, MCP catalog. Attachment cases in `offline-merge.test.ts` and `setup-handover.test.js`.<br>Also covers `setup-lists.test.ts`. | services | 2.2 | 4 ×2 | A |
| 6.1 | Settings pages and flows, permissions, sandbox (Seatbelt, `fence-exec`) | services | 4.1 | 5 ×2 | D |
| 6.2 | Vault: Helper Keychain, wrapped-key files, broker, SSH agent. Installed Helper and Keychain in `setup-mac.test.js`. Supply `kuvo_move::mac::KeychainItems` through Helper and verify that a custom Claude Code account's credentials survive a released move on a real Mac. | services | 1.3 | 6 | D |
| 6.3 | Files, Git, Changes, worktrees, projects, folders. Golden-home projects in `protocol/conformance/compat.test.mjs`. 6.3a files and folders: a file's info, text and bytes, folder lists, a new folder, a folder's tree, finding files and suggested folders ported (`kuvo_services::files`, `files` conformance area); `box route` bytes and Windows/Linux conformance left ([coverage](rust-files-coverage.md)). | services | 1.4 | 5 ×2 | D |
| 6.4 | Browser keeper: Chrome for Testing, CDP pipe, browser tools, live view, downloads | services | 4.1 | 5 ×2 | A |
| 7.1 | `kuvo-box keep`: supervision, selector, rollback; `kuvo-start` launching by manifest. Link startup markers and legacy handoff from `bridge.test.js` and `setup-mac.test.js`. | keeper | 1.3 | 5 ×2 | D |
| 7.2 | Fleet update with per-target packs, `kuvo-install` starting the move, uninstall, footprint | keeper | 7.1, 2.1, 2.6 | 4 ×2 | A |
| 7.3 | For released Macs: the `stays` and `next` items, served; repairing a machine a 0.7 Mac overwrote. Released Mac coexistence, legacy roster mail and leave-running integration from `documents.test.js`, `setup-mac.test.js`, `time-zone.test.ts`, `personalities-box.test.js`, `setup-handover.test.js`, and tagged `compat.test.mjs`.<br>Also covers `peers.test.js`, `mac-peers.test.js`, `devices.test.js`, `devices-two-boxes.test.js`. | move | 7.1, 2.4 | 6 | D |
| 8.1 | Native Windows: profile ACLs, logon task and starter, Git Bash, path vectors, long paths.<br>native Windows Add: the started resident must outlive the SSH session (logon task or starter) | os, keeper | 7.1 | 5 ×2 | D |
| 8.2 | Native engine, browser and the five agents; `windows-box-check.sh` runs the suite on unsigned builds | all | 8.1, 4.5, 6.4 | 6 ×2 | D |
| 9.1 | The switch from the TypeScript beta: backup, first-start take-over of its home (running turns end once), and the update of a computer that was off. Cross-process link writer handoff from `setup-mac.test.js` and `setup-handover.test.js` | move, keeper | all | 6 | A |
| 9.2 | Budget check and a 24-hour soak with turns | — | 9.1 | 4 | B |
| 9.3 | Rehearsal: the user's four computers (Mac, Linux, WSL2, native Windows) from the TypeScript beta, plus a 0.7.2 Mac and a 0.8.0-beta.1 machine straight to Rust. Rust-to-Rust mixed versions and rollback, real Helper and released Mac cases from `setup-handover.test.js`, `setup-mac.test.js`, and `setup-two-boxes.test.js`. | — | 9.2 | 7 ×2 | A |
| 9.4 | Four weeks after the user's computers switched: remove the TypeScript box, shapes source moves to Rust, docs | all | 9.3 | 6 | A |

Totals by phase: 0: 21 · 1: 39 · 2: 54 · 3: 18 · 4: 39 · 5: 46 · 6: 36 · 7: 24 · 8: 22 · 9: 30.
**The total is 329 hours.** By lane: A 100, B 81, C 73, D 75. Lane A is the longest chain.

While the plan runs, the TypeScript box keeps taking fixes. Each fix lands with a route scenario or
conformance test, so the Rust lane for that area takes it in. Windows sessions 4–11 in
[windows.md](windows.md) are replaced by 0.4, 1.3, 7.1 and 8.1–8.2. The native box is the Rust
box.

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
