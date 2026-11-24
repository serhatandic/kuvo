# Rust agent foundation coverage

Phase 4.1 is split into holder transport (4.1a), resident runtime and accounts
(4.1b), and tools and sidecar (4.1c). The five vendor adapters are 4.2–4.5.
The OpenCode process stays under the engine supervisor. Its agent tool entry
and the model and session projections share the resident routes.

## Holder transport

| Case | Boundary and outcome |
|---|---|
| H1 | [Agents conformance](../../protocol/conformance/holder.test.mjs). A successor replays an active turn, receives an acknowledged open ask again, answers once, and takes the same child to its end. Wrong keys and older generations are refused. The old controller is fenced. The finished holder removes its journal and record. |
| H2 | A five-megabyte line and the following line reach the current controller with a tiny journal. A complete replay request for the pruned range is refused, and the current controller can still write. |
| H3 | The live controller receives a control request in the Claude framing mode. The replay omits it so the CLI can redeliver it on reinitialization. |
| H4 | A pinned turn streams over five megabytes, replays each line in order, and releases the pin and compacts on the successor's command. |
| H5 | Two successors offer the same generation. Only one can send input after taking control. |
| H6 | An orphan closes its agent's stdin and removes its recovery files. |
| H7 | A detached agent finishes. The next controller reads its last output and exit without starting another child. |
| H8 | Stopping the agent also closes the listener owned by its subprocess. |
| H9 | Backpressure from an old controller cannot block a successor from replaying the journal and reading new output. |
| H10 | A raw worker keeps blank lines and carriage returns when streaming and when replaying. |
| H11 | A disconnected holder with a full, pinned journal still stops its agent when it times out. |
| H12 | An agent exits while a descendant in its own session keeps its output open. The controller hears the exit within the TypeScript holder's two-second allowance. |
| R1 | [Rust controller process test](../../box/crates/kuvo-box/tests/agent_holders.rs). Kill the original controller while the stand-in agent waits for a reply. A replacement controller launched from another release directory reads the record, replays the turn, answers the ask, and checks that both PIDs are unchanged. This tests the process and wire boundary used by a resident replacement. The test uses the same Rust build in both directories. |
| R2 | The Rust controller scans a scratch recovery directory with a stale Rust record and a TypeScript record. It returns neither as adoptable and leaves the TypeScript record untouched. Unix removes the stale Rust files. Native Windows preserves files when it cannot match the recorded start identity. |

The holder uses the existing OS socket and process-group abstractions. Windows
uses a same-SID named pipe derived from the computer's persisted socket key.
The holder uses a separate secret to authenticate the controller. The wire
remains `hold/1`. The recovery record includes the Rust holder format marker and OS process start identity, and only those records
are adoption candidates. The Rust controller gets exclusive socket halves
and the holder gives each direction bounded queues. Replay reads the disk
journal through its index. The adapter supplies the rule for which
outstanding request methods can survive a transfer.

## Verification

| Platform | TypeScript holder conformance | Rust holder conformance | Rust controller process tests |
|---|---:|---:|---:|
| macOS | 12/12 | 12/12 | 2/2 |
| Linux x86-64, lokum | — | 11/11 | 2/2 |
| WSL2, Ubuntu 24.04 | — | 11/11 | 2/2 |
| Native Windows x64 | — | 11/11 | 2/2 |

Linux and WSL used the same static musl build compiled on the Mac. The native
Windows run used static libunwind and the pinned Node 24 runner. The workspace
compile check, targeted Clippy with warnings denied, and TypeScript check passed.

The two H1 transcript files matched byte for byte after substituting the PIDs.
They include wire messages in both directions, the on-disk journal text, the
updated recovery metadata and generation, and the absence of the temporary
files after exit. To reproduce, set `KUVO_CONFORMANCE_AREA=agents` and
`KUVO_CONFORMANCE_HOLDER_TRANSCRIPT` to a new file for each binary before
running `node --test protocol/conformance`, then compare those two files.

The Rust holder applies a size limit to local frames and bootstrap data. It
also cleans up its owned process tree after the agent exits. Windows uses job
termination for a stop and a numeric exit status. POSIX signals remain
available on Unix.

## Resident runtime progress (4.1b)

The account index uses the saved `agents/accounts.json` format. The tests
read a legacy array, retain an added account's original home and unknown
fields, and refuse to rewrite a future-version index. A new account gets
its own folder and provider ID. The default account keeps its provider ID
and leaves home selection to the adapter.

The shared roster instantiates a factory for each configured account. It
keeps an adapter and its cached answers when the account label changes.
Account checks keep the last sign-in after a transient failure and ignore
an answer begun before credentials changed. A failed model lookup keeps
only the same account's previous catalog. The sign-in attempt book scopes
status, completion and cancellation to the initiating provider and accepts
a late-page code when the adapter says a code is needed.

The Rust controller replacement scenario now calls `Runtime::activate`.
It builds the stand-in adapter's roster, uses the recorded provider to find
it, and starts complete replay at the oldest active turn's cursor. The
controller API is unchanged. The saved session header reader supplies the
working directory without opening the transcript. The keeper switch case
passed on macOS, Linux (lokum), WSL2 and native Windows. It replaced the
resident during a running fixture turn and checked that the same holder
and child finished it. Authenticated keeper and resident proof belongs to
4.4.

On all four platforms, the foundation passed seven account tests, two
resident dispatcher tests, three holder process tests and twelve binary
conformance cases. The account fixture ran against both boxes on macOS.
The normalized HTTP replies and saved files matched, including the
legacy account home.

The vendor entry point is `Agent::open_thread`. Each thread receives a
new prompt or attaches the resident's sink to a recovered turn through
`Thread::start`. The sink handles transcript events, vendor-state
checkpoints and independent permission and question promises.

Saved-session checks read the bridge's header, native conversation state,
legacy JSON transcript and SQLite transcript. The binary transcript case
pages a saved conversation in both directions on both boxes. The replies
and unchanged transcript files match. On the first write Rust stages a
legacy import beside the original array and publishes it after commit.
Updates keep the existing message's position and carry the bridge's
usage and artifact projections. A late checkpoint or transcript write
after deletion cannot recreate the conversation.

The resident turn sink waits for a transcript writer to acknowledge each
event and saves native state before acknowledging that checkpoint.
Permission and form requests are scoped to their session and turn.
Tests cover withdrawing an ask, duplicate request IDs in separate
sessions, denying a late ask, and withholding acknowledgement until a
checkpoint has been written.

The transcript and sink checkpoint passed on macOS, Linux (lokum), WSL2
and native Windows. The two-box transcript fixture compared five HTTP
responses and the saved SQLite bytes, and the account fixture compared
the normalized replies and account index.

4.1b owns the read-only Blocks loader, command analysis and preflight
evaluation. The Rust tests cover the four command parsing cases in
`bridge.test.ts` and the `Sessions.checkPermission` cases for Blocks,
unavailable settings, native Plan mode, Auto outside the sandbox and
paths outside the project. Row 6.1 owns editing Blocks, the Settings
pages and sandbox setup and can reuse this evaluator.

The resident starts and interrupts a saved coding-agent session through
the compatibility routes. The macOS binary case follows a stand-in
through two turns and checks the saved conversation and vendor state.
The keeper case also checks that its replacement resident connects the
recovered adapter to the shared sink and saves the replayed text and
final outcome. The holder and child PIDs are unchanged. The controller
contract and roster factory signature are unchanged.

The compatibility permission and form routes now use the resident's
ask book. The macOS fixture sees a running coding-agent session in the
shared session list, changes Blocks while that turn is asking, and
verifies that an attempted approval cannot release the action. It then
rejects the ask and checks that a delayed reply cannot answer it again.
Unified ask cards and delivery routes remain open.

The shared session actions work on coding-agent conversations too.
`kuvo/sessions/<id>` with its `actions`, `rename`, `pin` and `archive`
finds the conversation with the agents before asking the engine. The
saved header keeps the bridge's own notes (`kuvoAgent` and a subagent's
`kuvo.messages`) when a client replaces the metadata, and reads carry the
computer's Summarize setting for Claude Code, Codex and Grok. Delete stops
a running turn first, refuses new turns meanwhile, and removes the
subagents' conversations unless a fork's steps still show them. Archive
stops a turn that waits on an approval or a question. The binary case
(`agent-sessions`) passes on both boxes and their normalized replies
match. Delete's confirmation now counts the coding agents' rows in a
worktree, as the TypeScript list does. Holding the box queue on Archive
and Stop belongs with the delivery work below. The engine's `Auto` run
that a session forgets on delete stays with Auto.

The following integration work remains open:

- Production vendor factory registration as the adapters land.
- Unified sign-in and account status, default-home migration, and shared session, queue and approval routing.
- Differential and golden-home runs for the remaining routes, with platform verification.

## TypeScript scenario inventory

### Holders, reload and repair

Case numbers use the order in the cited source file.

| TypeScript source | Port and remaining owner |
|---|---|
| [hold.test.ts](../../box/test/agents/hold.test.ts), 19 | 1 → R1, diagnostic events checked through H7. 2 → H1. 3 → H7. 4–5 → H2. 6–7 → H4. 8 → managed program install cleanup (4.3). 9 → H4. 10 → H2. 11 → H8. 12 → H9. 13 → H5. 14 → R2. 15 → H6 and H11. 16 → H1. 17 → H3. 18 → socket path fallback integration (4.1b). 19 → startup cancellation and fallback (4.1b). |
| [held-resume.test.ts](../../box/test/agents/held-resume.test.ts), 3 | 1–3 → session recovery and open approval reconciliation (4.1b). R1 covers the underlying process and replay. |
| [hold-claude.test.ts](../../box/test/agents/hold-claude.test.ts), 1 | H3 covers holder suppression, with real CLI reconnection and request cancellation in 4.4. |
| [bridge-update.test.ts](../../box/test/agents/bridge-update.test.ts), 2 and [bridge-update-cursor.test.ts](../../box/test/agents/bridge-update-cursor.test.ts), 2 | Holder continuity → R1. Adapter readiness, same conversation and next-turn reuse → 4.1b, 4.2, 4.4 and 4.5. |
| [agents-renewal.test.js](../../box/bridge-tests/agents-renewal.test.js), 4 and [update-stops-nothing.test.js](../../box/bridge-tests/update-stops-nothing.test.js), 5 | R1 covers the holder portion of replacement. Resident admission, actual keeper selection, readiness and rollback → 4.1b and 7.2. The historical bridge-specific standby, bridge build, pre-holder switch and quiet-stop rules belong to the TypeScript transition in 9.1. |
| [folder-version.test.js](../../box/bridge-tests/folder-version.test.js), lines 287, 301 and 326 | Runtime startup relies on the resident's layout gate. The agent-runtime assertion belongs to 4.1b. Keeper and release trial rollback → 7.1 and 7.2. |
| [peers.test.js](../../box/bridge-tests/peers.test.js), 17 | Local live coding-agent status feeds the resident's peer status (4.1b). Legacy machine roster → 2.6 and 7.3. |

### Accounts, roster, tools and sidecar

| TypeScript source | Remaining owner |
|---|---|
| [accounts-check](../../box/test/agents/accounts-check.test.ts), 20, [account-refresh](../../box/test/agents/account-refresh.test.ts), 8, [added-account-route](../../box/test/agents/added-account-route.test.ts), 3, [own-homes](../../box/test/agents/own-homes.test.ts), 4, and [signin-mode](../../box/test/agents/signin-mode.test.ts), 3 | 4.1b owns account index, home selection, checks, catalog cache and sign-in state. Vendor sign-in commands and authentication messages → 4.2–4.5. Accounts-check 18 covers Work Queue GitHub authorization in 5.2. |
| [sign-in.test.js](../../box/bridge-tests/sign-in.test.js), 11 | Agent route cases 2–4, 8 and 10–11 → 4.1b. OpenCode methods and status → 3.2. Setup and remote transport assertions use the resident dispatcher. |
| [renewal](../../box/test/agents/renewal.test.ts), 14, [sandbox-claude-renewal](../../box/test/sandbox-claude-renewal.test.ts), [program-updates](../../box/test/agents/program-updates.test.ts), 9, [update-notices](../../box/test/agents/update-notices.test.ts), 4, and [claude-mac-update](../../box/test/agents/claude-mac-update.test.ts), 3 | 4.1b owns shared renewal scheduling and roster updates. Provider commands and policies → 4.2–4.5, sandbox → 6.1, full release workflow → 7.2. |
| [toolrelay](../../box/test/agents/toolrelay.test.ts), 9, and [engine-own-tools](../../box/test/engine-own-tools.test.ts), 4 | 4.1c owns persisted address and keys, session attribution, reconnect and cancel, and engine tool registration. Tool handlers remain with their service or work area. Compatibility removal and sandbox allowlists → 7.3 and 6.1. |
| [bridge](../../box/test/agents/bridge.test.ts), 45, and [bridge.test.js](../../box/bridge-tests/bridge.test.js) | Resident route, roster and agent runtime checks → 4.1b. Tool transport and sidecar launch → 4.1c. Bridge-only HTTP hop and split process lifecycle retire with the TypeScript bridge in 9.1. |
| [sessions](../../box/test/agents/sessions.test.ts), 158 | Shared agent session store, queue, and event projection → 4.1b. Vendor wire, transcript specifics → 4.2–4.5. Carry, handover, review and subagent projection → 4.6. |
| [codex](../../box/test/agents/codex.test.ts), 121, [acp](../../box/test/agents/acp.test.ts), 73, [claude](../../box/test/agents/claude.test.ts), 134, and [cursor](../../box/test/agents/cursor.test.ts), 31 | 4.2–4.5 own vendor protocols and per-agent tests. The adapter-independent JSON-RPC seam and runtime package install belong to 4.1c. |
| [agent-tools.test.js](../../box/bridge-tests/agent-tools.test.js), [artifacts](../../box/test/artifacts.test.ts), [subagents](../../box/test/agents/subagents.test.ts), [carry-agents](../../box/test/agents/carry-agents.test.ts), [to-agents](../../box/bridge-tests/to-agents.test.js) and [agents-handover](../../box/bridge-tests/agents-handover.test.js) | Agent registration and tools dispatch share 4.1b/c. Tool handler semantics span 2.4 ownership and messages, 4.6 subagents and transfers, 5.1 starts and bot work, 5.5 artifacts, and 6.1 permissions. |
| Agent account imports in [usage tests](../../box/test/usage), and [settings-agents-live](../../box/bridge-tests/settings-agents-live.test.js) | 5.4 owns usage and limits. 6.1 owns the engine Agents settings file and the live engine settings page. Both consume 4.1b account state. |

### Cases carried from the engine coverage map

All of the following remain with the shared resident-facing runtime (4.1b),
with worktree mechanics in 6.3 and tool relay integration in 4.1c. Details
and existing partial coverage remain in [the engine map](rust-engine-coverage.md).

| TypeScript case | Number(s) |
|---|---|
| message-parts, queue-items, resident-delivery | 7; 25; 1 |
| session-actions, session-pages, session-lists | 2 and 10; 3; 14 |
| queue.test.js, start-session, typed | 44–50; 1; 5 |
| conversation-activity, session-asks | 2; 2 |
| models-offered, model-aliases | 1–2, 5, 8–9, 11–12, 14–15; 1–3 |
| worktrees | 12, 30 |
| attachment-paths, attachments-go | 1; 1–4 |
| session-models | 6–9, 11, 15–16, 18, 21, 23–24, 26–27, 31–32 |

The saved-home checks for the account index, agent sessions and legacy
`run/agents.json` projection remain in 4.1b. Authenticated vendor runs, live
resident adoption during a fleet rollout, route-level differential comparison,
and golden-home compatibility are still open.
