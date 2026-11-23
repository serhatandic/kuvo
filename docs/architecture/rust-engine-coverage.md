# Rust engine coverage

Phase 3.1 runs OpenCode from the installed pin, owns its process, and routes
the engine's API through the resident. The stand-in scenarios are in
[`kuvo_engine/tests/resident.rs`](../../box/crates/kuvo_engine/tests/resident.rs).
They use the real resident, dispatcher, process launcher, discovery file and
WebSocket bridge. The real-engine smoke exercises the same entry points with
OpenCode 2.2.6 on macOS, Linux, WSL2 and native Windows.

## TypeScript reference scenarios

| Source | Cases | Rust coverage |
| --- | ---: | --- |
| `box/test/engine-lifetime.test.ts` | 1 | Cold API demand, concurrent demand, passive event/active reads, busy deferral, parking and wake. Creating a conversation through `kuvo/sessions/new` follows in 3.2. |
| `box/test/pty-socket.test.ts` | 3 | Local password, engine password, directory and query, text and binary in both directions, client close code, and upstream refusal. |
| `box/test/engine-context.test.ts` | 1 | Latest context measurement and summary invalidation belong with session reads in 3.2. |
| `box/test/engine-swap-inboxes.test.ts` | 2 | Pending inbox pagination, incomplete inventory and interrupted ordered transfer belong with queues in 3.2 and release recovery in 7.2. |
| `box/test/engine-own-tools.test.ts` | 4 | Tool registration, compatibility settings, old app-folder exclusion, and generated launch tools follow in 4.1, 6.1 and 7.3. |
| `box/bridge-tests/own-box-engine.test.js` | 3 | Installed-pin startup and adoption cover the worker lifecycle. Helper launch notes, normalization of the old shell launcher’s HOME, and keeper configuration refusal follow in 7.1 and 7.3. |
| `box/bridge-tests/start-session.test.js` | 5 | Session ancestry, marking/refusal, worktree admission, new worktree filing and sibling-folder filing follow in 3.2 and 6.3. |
| `box/bridge-tests/engine-swap.test.js` | 4 | Held turns and ordered sends, rollback after bad launch, bounded hold expiry, and the downgrade fence are the old engine swap. The replacement release paths follow in 3.2 and 7.2. |
| `box/bridge-tests/update-engine-turn.test.js` | 3 | A resident restart keeping the engine is covered. A full release switch with a live turn, waiting on a background shell at release switch, and a bridge turn finishing through handover follow in 4.1 and 7.2. |
| `box/bridge-tests/bridge.test.js`, API forwarding and path vectors | 2 | Authenticated body and header passthrough, a forged client password, refused paths, event streaming/cancellation and all 100 forwarded engine path vectors. Phone message parts and named-agent rewriting, integration enrichment and agent-bridge routes are gaps. |
| Same file, engine unavailable and info timestamp | 2 | Passive and stopped status, retry wording, and the discovery timestamp surfaced in `kuvo/info`. |
| Same file, Start/Restart and update admission | 2 | Install-stage rejection, download-stage admission, an older Stop marker, and typed Start/Restart. Shell-launcher and bridge survival assertions follow in 4.1/7.1. |
| Same file, engine restart using a Mac’s settings and info naming its last launcher | 2 | The installed pin and own settings path replace the old start command. Mac-supplied launch settings and attribution follow in 6.1/7.3. |
| Same file, paired terminal | 1 | Paired caller and path admission are shared with the local transport. Noise request IDs, socket frames and cancellation still need the paired resident transport. |

This maps 35 reference scenarios. The local PTY tests and engine unavailable
case have direct counterparts. The lifecycle, raw API, timestamp and control
cases have coverage at the Rust boundary, with the route-specific gaps
above. Session/context, tools, handover and the old swap cases still need
their owning phases. Model-folder refresh tests use the same engine activity
endpoints, but refresh itself is part of 3.2. The sandbox worktree test
belongs to 6.1.

## Conformance

The TypeScript and Rust residents both pass the authenticated discovery and
event-line restart scenarios. Two mixed routing cases pass against TypeScript
and stop at deferred behavior on Rust. The prompt reaches OpenCode, and the
phone message decoration waits for 7.3. The update scenario starts by asking
for `kuvo/update`, which follows in 7.2. The Rust resident test separately
covers the engine Start route and the update fence.

## Differences in implementation

A Tokio task owns the engine, shares one cold start among requests, and runs
idle probes independently of normal requests. Request leases pin active
work. Event streams stay passive. Only the task commits a park, after
checking that the process and activity observed by its probe are current.

Launch uses the installed OpenCode binary directly. It writes the compatible
discovery record atomically and adds the process start identity. If the
resident is interrupted, the next resident verifies the executable and
identity and checks authenticated HTTP readiness before giving out the
address. A usable legacy record can still supply the upstream address. The resident
only stops an engine whose executable it can identify as its own. Unix uses the process
seam for process groups and adoption. Windows associates the worker with a
recoverable Job Object.

HTTP streams the response and keeps the existing header allowlist. Upstream
redirects stay on the engine's origin. The local WebSocket and future paired
transport use the same dispatcher admission and bounded message channels.

## Phase 3.2a: local OpenCode conversations

The `engine-sessions` and `engine-worktree` conformance areas use the selected
resident binary, a scratch home, and the same loopback engine stand-in.
`kuvo_engine/tests/resident.rs` additionally interrupts an inbox transfer
after the first message, restarts the resident, and checks delivery order.
The reference inventory below covers 207 named test cases and the 38
message-parts vectors. A row with partial coverage records the gaps.

| TypeScript source | Cases | Rust scenario and gaps |
| --- | ---: | --- |
| `box/test/message-parts.test.ts` and `protocol/vectors/message-parts.json` | 18 cases, 38 vectors | The legacy parts route checks all 13 read vectors. The send scenario checks text and metadata in 23 compose vectors for fields, quotes, widgets, comments, file notes and phone decoration. Same-ID replay and Undo are covered separately. The full attachment-copy path, model-specific file handling, setup-wide naming, forwarding to the agents runtime, and the validation and refusal variants still need checks. |
| `box/test/engine-context.test.ts` | 1 | Latest step occupancy, cached model window and completed-summary invalidation are covered together. |
| `box/test/engine-swap-inboxes.test.ts` | 2 | The Rust resident test covers strict inventory pagination, refusal of incomplete reads, and ordered recovery across restart with a new send arriving during an interrupted transfer. |
| `box/test/queue-stop.test.ts` | 6 | An arriving removal is covered across Stop, send and removal. Timing of concurrent Stop and engine submission, named arrivals across restart or Resume, and the unlisted arrival cases still need their own scenarios. |
| `box/test/queue-items.test.ts` | 25 | Busy steering, edit/removal, relative order, paused queue persistence, old removal after Undo and arriving removal run through HTTP. Reordering and Send Now are checked through the queue topic and the engine inbox. Fault injection, attachment references, interrupted Undo, pending handoff races, wake summaries, limit release and queue-topic reconnect coverage are incomplete. |
| `box/test/resident-delivery.test.ts` | 2 | Local OpenCode delivery has a restart scenario. Other coding agents and the Work Queue attachment check remain in their owning phases. |
| `box/test/session-actions.test.ts` | 26 | Rename, stale revision, legacy unarchive preserving other notes, delete and subagent refusal run together. Pin, archived menus and project deletion warnings, stopped asks, wake-ups, move authority, bulk delete, Settings and concurrent revisions need further parity checks. |
| `box/test/session-pages.test.ts` | 3 | The local-list topic publishes new chats. Project paging and nested subagent ordering and delivery facts need dedicated cases. |
| `box/test/session-lists.test.ts` | 47 | A new local row reaches the line. Other list changes, change races, recovery from an old snapshot, filesystem changes, worktree filing and activity from the engine database are gaps. Asks, bots, background watches and Work Queue counts follow in their owning phases. |
| `box/test/session-notes.test.ts` and `box/test/session-notes-writers.test.ts` | 15 | Archive after rename preserves existing notes. Simultaneous writers, cancellation, and compatibility with the old cross-process locks remain uncovered. |
| `box/bridge-tests/queue.test.js` and `queue-live.test.js` | 57 | Covers busy steering without interruption, stop and queued replay, removal, Undo, and transfer recovery. The pinned-engine smoke includes start, rename, stop, queued-send replay, removal and delete. Full multi-turn resume, remaining Send Now variants, failure and limit handling, liveness, carry, file references, and the bridge-specific paths remain incomplete. |
| `box/bridge-tests/start-session.test.js` | 5 | The worktree conformance case uses Git and covers ancestry from a subagent, refusal of an unrelated folder and filing under the linked worktree's project from creation. Marking failure, wake promises, worktree creation and sibling-folder filing still need their original scenarios. |

### Implementation and remaining parity work

The Rust queue uses the TypeScript index and waiting payload paths. A
per-session turn orders writes and an owned permit serializes durable index
replacement off the resident executor. Removal and Undo leave a spent token
so a retry cannot take a new occurrence. Direct sends journal the original
request before posting. On retry the resident checks the saved answer, the
box queue, the OpenCode inbox and then the exact transcript ID. The journal has
its own `run/prompt-receipts` family. Fresh session facts publish to the peer
publisher through an in-memory watch and retain a compatible list on disk.

The differential runner can stop the resident at engine acceptance and resume
with a new resident on the same copied home. In the ten-step desktop sequence,
the transcript and inbox matched, and nine route replies matched. On retry
after the forced restart, TypeScript answered 409 for its interrupted owner
claim and Rust settled the recorded prompt as 200 from the transcript. The
direct-send run without desktop headers also matched the final reply. The
complete home inventory differs. TypeScript starts agents, vault, Insights, Work Queue,
settings and setup runtimes that are ported in other phases. Rust writes the
prompt journal. List cursor sequences, queue scanning and some approval and
removal bookkeeping differed in the comparison. The current-beta fixture's
saved queue and two OpenCode session and inbox reads matched. The pinned engine
returned HTTP 500 for the complete inventory on both current-beta copies, so
that fixture did not establish full list parity.

The peer outbox still needs the durable delivery entry point and receipt topics
for addressed sends. The legacy direct routes await engine confirmation.
Keep and release of inboxes are implemented as engine operations. The keeper
still has to coordinate the hold across the release switch. Setup-wide naming,
attachment retention, tool-step display fields, full project and worktree
admission, and approval and archive integration need the remaining matching
behavior described above. Asks, waiting, models and search remain 3.2b.
