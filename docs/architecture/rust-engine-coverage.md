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
`engine-live` uses the installed engine supplied in `KUVO_CONFORMANCE_OPENCODE`
and a real OpenCode Go question to put subsequent messages in its inbox.
`kuvo_engine/tests/resident.rs` additionally interrupts an inbox transfer
after the first message, restarts the resident, and checks delivery order.
### TypeScript scenario map

Numbers are one-based test declarations in each file at `a21b64fc5`.
The first inventory recorded 294 cases. Across both inventories, 236 of
411 declarations are covered, 68 are partial and 107 are gaps.
One gap is a retired phone scenario. Row 3.2 retains 12 partial or gap declarations.
Covered cases have direct Rust evidence. Partial cases exercise only part of
the original result or use the local OpenCode boundary where the original
also uses a bridge, peer, settings route, or failure injection. Multiple
owners on the same case each own the part described by their phase.

- Message parts covers all 13 read and all 25 compose vectors. The original
  conformance case filtered out the two `kept` compose vectors because the
  Rust sender did not retain the inline attachment bytes. Both vectors now
  use real inline payloads and check the recovered bytes, the prompt files,
  and both kinds of attachment notes. Validation and error recovery are separate.
- Context and inbox transfer use the resident scenarios above. Send recovery covers
  both sides of engine acceptance, including the live inbox and completed transcript.
- Queue coverage is through the five session queue scenarios and the pinned engine
  smoke. Partial cases still need their missing refusal, restart, lifecycle, or
  concurrency assertion.
- Session actions and notes share the rename, archive, and deletion conformance case.
  The new-session case checks the local and published list. Worktree cases use Git
  for the linked folder and a newly created worktree.
- The local typed and durable delivery tests exercise the complete draft envelope,
  an accepted handoff with its reply lost, and the receipt and queue routes on
  retry. The scoped and addressed variants have separate ownership and recovery cases.
- The worktree failure scenario checks both sides of the engine's create response.
  Its transport-close counterpart and independently managed worktree routes remain open.

| Source | Covered | Partial | Gap | Owners for partial and gap cases |
| --- | --- | --- | --- | --- |
| `box/test/message-parts.test.ts` (18) | 1–6, 8–18 | — | 7 | 4.1 · 7 |
| `box/test/engine-context.test.ts` (1) | 1 | — | — | — |
| `box/test/engine-swap-inboxes.test.ts` (2) | 1–2 | — | — | — |
| `box/test/queue-stop.test.ts` (6) | 1–6 | — | — | — |
| `box/test/queue-items.test.ts` (25) | 1–24 | — | 25 | 4.1 · 25 |
| `box/test/resident-delivery.test.ts` (2) | — | 1 | 2 | 4.1 · 1<br>5.2 · 2 |
| `box/test/session-actions.test.ts` (26) | 1, 4, 6–9, 14–16, 20, 24–25 | 3, 5, 10, 23, 26 | 2, 11–13, 17–19, 21–22 | 4.1 · 2, 10<br>2.4 · 11–13, 19<br>5.1 · 3, 5<br>6.1 · 17–19, 21–23, 26 |
| `box/test/session-pages.test.ts` (3) | 1–2 | 3 | — | 4.1 · 3 |
| `box/test/session-lists.test.ts` (47) | 1, 3–13, 15–17, 19–22, 24, 26–28, 47 | 14, 23, 29, 46 | 2, 18, 25, 30–45 | 3.2 · 2, 46<br>4.1 · 14<br>5.1 · 18, 23, 25, 29–45<br>6.3 · 46<br>2.4 · 45 |
| `box/test/session-notes.test.ts` (8) | 1–8 | — | — | — |
| `box/test/session-notes-writers.test.ts` (7) | 3–5, 7 | 1–2, 6 | — | 7.3 · 6<br>5.1 · 1<br>5.3 · 1–2<br>6.1 · 1 |
| `box/bridge-tests/queue.test.js` (56) | 1–2, 4–34, 36–39, 43, 51–56 | 3 | 35, 40–42, 44–50 | 4.1 · 44–50<br>4.6 · 40–42<br>dropped: phone app retired · 35<br>5.1 · 3 |
| `box/bridge-tests/queue-live.test.js` (1) | 1 | — | — | — |
| `box/bridge-tests/start-session.test.js` (5) | — | 1, 3–5 | 2 | 3.2 · 2<br>5.1 · 2<br>6.3 · 3–5<br>4.1 · 1 |
| `box/test/typed.test.ts` (9) | 1–4, 6–9 | — | 5 | 4.1 · 5 |
| `box/test/draft-orphan.test.ts` (5) | 1–4 | 5 | — | 2.4 · 5 |
| `box/test/conversation.test.ts` (3) | 1–3 | — | — | — |
| `box/test/conversation-activity.test.ts` (2) | — | 1 | 2 | 4.1 · 2<br>5.1 · 1–2 |
| `box/test/message-words.test.ts` (11) | 1–6 | 7–9 | 10–11 | 5.1 · 8–10<br>5.2 · 7<br>5.3 · 11<br>6.1 · 7<br>6.4 · 7 |
| `box/test/session-news-kinds.test.ts` (5) | — | — | 1–5 | 5.1 · 1–5 |
| `box/test/projects-worktrees.test.ts` (6) | — | 1–4 | 5–6 | 3.2 · 3–4<br>6.3 · 1–6 |
| `box/test/start-cancelled-worktree.test.ts` (2) | 1–2 | — | — | — |
| `box/bridge-tests/worktrees.test.js` (32) | 8 | 1, 9, 13–14, 17–18 | 2–7, 10–12, 15–16, 19–32 | 3.2 · 24–30<br>4.1 · 12, 30<br>6.3 · 1–7, 9–32<br>7.3 · 15–16 |
| `box/bridge-tests/worktrees-where.test.js` (12) | — | — | 1–12 | 6.3 · 1–11<br>5.5 · 12 |

### Additional 3.2b reference cases

The following 117 declarations extend the original 294-case inventory.
Each row uses the same one-based numbering and ownership rule.

| Source | Covered | Partial | Gap | Owners for partial and gap cases |
| --- | --- | --- | --- | --- |
| `box/test/session-asks.test.ts` (19) | 1, 3–5, 10–11, 14–18 | 2, 6–9, 12–13, 19 | — | 4.1 · 2<br>5.1 · 8<br>5.3 · 9<br>6.1 · 6–7, 12–13, 19<br>2.4 · 19 |
| `box/test/model-readiness.test.ts` (1) | 1 | — | — | — |
| `box/test/models-offered.test.ts` (15) | 3–4, 6–7, 10, 13 | 1–2, 5, 8–9, 11–12, 14–15 | — | 4.1 · 1–2, 5, 8–9, 11–12, 14–15<br>5.1 · 8<br>5.4 · 14–15 |
| `box/test/models-starred.test.ts` (7) | 1–2, 4–7 | — | 3 | 7.3 · 3 |
| `box/test/models-speed.test.ts` (3) | 2–3 | 1 | — | 4.2 · 1 |
| `box/test/model-aliases.test.ts` (3) | — | — | 1–3 | 4.1 · 1–3<br>5.4 · 3 |
| `box/test/model-names.test.ts` (3) | 1–3 | — | — | — |
| `box/test/search-snippet.test.ts` (4) | 1–4 | — | — | — |
| `box/test/attachment-paths.test.ts` (2) | — | 1–2 | — | 2.4 · 1–2<br>2.5 · 1–2<br>4.1 · 1 |
| `box/test/add-address.test.ts` (3) | — | — | 1–3 | 2.3 · 1–3 |
| `box/bridge-tests/archive-waiting.test.js` (3) | 1 | — | 2–3 | 5.1 · 2<br>6.3 · 3 |
| `box/bridge-tests/attachments-go.test.js` (6) | 6 | 1–5 | — | 4.1 · 1–4<br>2.4 · 5<br>7.3 · 1, 3 |
| `box/bridge-tests/search.test.js` (11) | 1–11 | — | — | — |
| `box/bridge-tests/session-models.test.js` (33) | 1–5, 10, 12–14, 17, 19–20, 22, 25, 28–30, 33 | 9, 16, 18, 23–24, 26–27, 32 | 6–8, 11, 15, 21, 31 | 4.1 · 6–9, 11, 15–16, 18, 21, 23–24, 26–27, 31–32 |
| `box/bridge-tests/models-refresh.test.js` (4) | 1–2, 4 | 3 | — | 6.1 · 3 |

### Implementation and remaining parity work

The Rust queue uses the TypeScript index and waiting payload paths and the
shared Rust atomic file writer. A Tokio task orders writers by session or
receipt key. A dropped caller releases its turn, and an unrelated session
can write while one is waiting on OpenCode. Forwarded session PATCH requests
use the same turn as resident writers. The turn also claims the older
per-session directory lock. The process test holds that lock and the
earlier global lock, checks stale lock cleanup, and releases both in turn.
The resident's `NoteWriter` keeps one guard across related edits. Its test
changes nested and top-level notes and permissions while a separate client
waits for its own turn. The note writer is shared by the feature owners.
An owned permit serializes durable
queue index replacement off the resident executor. Removal and Undo leave a spent token
so a retry cannot take a new occurrence. An inbox Edit keeps its original
draft and neighbor through a failed read. It checks the inbox and exact
transcript ID before returning the recovery copy and avoids repeating a
confirmed removal. Edit includes retained files alongside carried files.
A queued handoff records its native POST in the queue index. A reply and
matching inbox or transcript establish acceptance. The view hides a
delivered message while its handoff backup persists. The failure cases
block the payload or index on disk, verify the refusal and retry, and
check that an accepted handoff is offered only once. The shared file writer
syncs both the temporary file and its parent before returning success.
Direct sends journal
the original request before posting. On retry the resident checks the saved answer, the
box queue, the OpenCode inbox and then the exact transcript ID. The journal has
its own `run/prompt-receipts` family. Fresh session facts publish to the peer
publisher through an in-memory watch and retain a compatible list on disk.

The ten-step desktop differential stops the resident just after engine
acceptance, then resends the same ID on the restarted resident. The protocol
requires confirmation of a proven acceptance. A direct-send HTTP 409 is a
refusal in GPUI. The old TypeScript owner claim returned 409 even when the
engine already had the message. The TypeScript owner now settles that claim
using the local queue, engine inbox or exact transcript ID. The two residents
agree on all ten replies and the final engine state. Tests also cut the
original send before engine acceptance and while the prompt is still in the
engine inbox. [Send-once contract](../../protocol/PROTOCOL.md),
[GPUI send client](../../gpui/kuvo_box/src/client.rs).

OpenCode's diagnostic for the current-beta second inventory page was
"Path is not absolute". The shared fixture had replaced only "@HOME@",
leaving literal "@S@" paths in several saved session rows.
The generator now resolves the stored locations, URLs, repository IDs,
and path-derived project IDs before opening either box. Both residents
read all five recorded pages using the pinned OpenCode API and preserve the
beta home’s paused queue and message ID. Direct reads of the copied
homes also match for the queue and sampled session and inbox responses.

The local composer uses one persisted decision before send and settles it
through the message journal. Its no-wait route writes a compatible standalone
outbox record and returns the pending receipt before OpenCode replies. A
resident task delivers saved drafts in order and reports its durable answers
through the receipt topic. Taking a saved draft back keeps a terminal receipt
and its recovery copy until the composer acknowledges it. Steer and Send Now
persist alongside the original draft. Setup-scoped claims bind the original
owner, listing computer or pairing, and original draft digest. The address
index rebuilds from both receipt record versions in bounded batches. Outcome
reads revalidate the current issuer and keep a page incomplete if one of its
computers cannot answer.
The conformance case checks that a draft holding its turn leaves other HTTP
requests responsive. Phase 2.4 owns the timed probe of the shared journal's
on-disk claim lock.

The box attaches the ownership handle before registering the engine routes,
starts an authenticated request receiver after it has the local session view,
and announces submission recovery to its peers. Drafts and addressed prompts
are durable before the first reply. The owner applies the ordinary typed or
prompt adapter under the same message identity on every retry. The peer
transport supports inline payloads up to 5 MiB for an addressed draft and
128 KiB for a scoped submission. Phase 2.4 owns the shared move journal,
move admission fences and receipt handoff. Phase 5.3 owns the group
transcript used by addressed group messages.

The engine's event reader owns the local ask cache. Its session list and
waiting count use the same snapshot. Normal questions and permissions are
answered in their original session, and archiving holds the inbox and stops
a run waiting on a question or permission. Global MCP forms follow the first
active session using their server and are dismissed in that folder. Permission
grants reread the ask, device and local Blocks at the native boundary.
`ExternalAsks` carries cards, responses and waiting changes from the vault
and sandbox owners. The vault adapter reads and answers a child’s actual
vault ask through the resident holder. The conformance case changes the ask
and a local Block during the final permission read. The Rust resident test
also changes the pairing and ownership state during that read. The vault
card scenario checks a lasting grant for a project, an account folder's
one-time choices, and rejection from an unrelated conversation. Sandbox
cards go through the tested adapter seam for 6.1. The bare 404 fallback in the
TypeScript holder dates to `e521e0998`, after beta.1. The Rust dispatcher
reads the asks and vault holder in the same resident and returns 503 when
OpenCode is unavailable. The sandbox and vault process paths stay with 6.1
and 6.2. Local bot activity includes delegation
and the Work Queue counts newer copies once; `PeerWaiting` takes the live
peer-activity snapshot from 5.1. The wake pause route and project removal
remain in 5.1 and 6.3. A worktree whose session creation gets a failure
response is removed only after a complete inventory shows no conversation
using it. The client can disconnect during creation and the resident still
settles the worktree from OpenCode's result. Ambiguous transport failures
leave the worktree kept. Explicit and delegated starts retain their
project when they run in a sibling folder or an external Git worktree.

Model readiness reads the conversation folder's OpenCode catalog and
remembers its names, including removed models from signed-in providers.
An unloaded catalog leaves readiness unset. A complete catalog without a
runnable default falls back to its first runnable model, or says none when
no models run. Concurrent clients share reads by engine and folder.
Credential and model events invalidate saved reads, and reopening the picker
within two seconds reuses its refresh. The picker applies model order, availability, defaults,
and preferences. Speed and standalone stars share the existing preference
file. The setup actor saves stars as a signed `SetupStarredModels` value.
A member sends the change to the current main through the setup authority
mail. Each computer reoffers its older local list, and the main records
which stars it has seen from that source so a later import preserves removals.
`AgentModelSource` supplies the catalog and session directory from 4.1,
with account plans from 5.4. Provider refresh keeps active folders and
settles a persisted GitLab refresh after work ends. The provider compatibility
layer rewrite is part of 6.1. Search caches local engine transcripts by
session revision. Setup search merges live and local answers, filters stale
moved copies, and reports each computer that could not answer. The
`kuvo_peers::memory_read` task uses authenticated multipart mail and keeps
delivery failures apart from reach observations through the relay's
ordered self-echoes. No request or reply is stored in the forward spool.
Projection of a coding-agent transcript waits for 4.1.
The local `Engine::transcript` seam reads OpenCode's newest and older
pages, stops at an Undo boundary and bounds the answer for a local agent
or peer letter. The resident scenario covers a failed older page with
and without a known Undo boundary, accent-insensitive query matches and
failed automatic and manual summaries. Phase 2.4 owns the letter caller
and 4.1 owns reads from the coding-agent runtime.
Raw prompts and commands use the same attachment ingest as the composer,
with the raw prompt's own send-once response cache. Expiry preserves moved
payloads and transcript pictures. `AttachmentSource` carries the other
runtime's complete inventory and move protections. Removal waits until both
runtimes can account for any branch references. The resident fixture covers
carry, Windows attachment paths and refusal by either runtime's inventory.
The coding-agent caller belongs to 4.1. Phase 7.3 owns the released Mac copy
location and its handover.
Path remapping is exercised through the backup and move consumers in 2.4
and 2.5, with coding-agent carry in 4.1. Installer address parsing belongs
to enrollment in 2.3.
Engine-swap inbox transfer is independent of the release keeper in 7.2.
Queue limit stops keep their reason even if OpenCode's inbox is unavailable.
The resident recovers missed failures from the transcript and looks up the
current engine before it releases a reset. Its next-due task checks the
persisted state again under the session turn, so a manual Resume or Stop
wins. The same release can retry a refused send or an in-progress removal.
An explicit Resume opens the queue while a turn is still active and leaves
its head in place until OpenCode finishes. The Stop path verifies which
items are still in the inbox after changing delivery. The overview publishes
an initial snapshot, the hold and the later resume. Automatic delivery
preserves the time of the last explicit Resume. A failure that arrives during
the first handoff after restart can then hold the next saved draft.
Resume preflights every waiting payload and returns the protocol format
refusal before changing state if a saved queue is newer. Readers share a
background inbox request, keeping the last complete view during malformed
or unavailable responses. The feed publishes reasoning after two seconds
and clears it after a tool or text break lasts a second and a half, without
reading the session again. Inventory changes invalidate rows fetched before
the event. At most one complete inventory is in flight, and changes received
during it schedule a fresh read. On-disk project and worktree changes refile
the latest complete inventory while OpenCode is busy. Old lists without
Needs you derive it from their saved facts.
The CLI route uses the resident's saved cursor without rewriting its inventory.
The line publishes the same changed rows and tombstones as a cursor read.
Worktree creation sanitizes the computer's configured branch prefix, and
new conversations in a recorded worktree receive its branch instructions.
The worktree owner can call `Engine::tell_worktree` after recording a rename.
Session Delete and Delete All read the live and restorable records, check
which independent conversations use each tree, and tell the client what
goes with their sessions. Delete checks the same confirmation inside its
note turn. Phase 6.3 owns naming, removal, restore and project discovery.
On execution success, a completed standalone summary preserves the read
marker if the preceding turn was read. In-turn summaries and later replies
preserve their unread state. A saved wake-up shows the same summary from
OpenCode's inbox or a waiting payload and keeps the full prompt for Edit.
The retired phone app's Send Now with a paired phone prompt is dropped.
The released Swift Mac clients use the engine inbox and the GPUI client
uses the composer and Undo routes. The box keeper still uses queue Add with
its saved wake-up text.
The malformed session path is refused by both residents' shared route
guard. Keeping the last coding-agent rows while their bridge starts
belongs to that holder in 4.1. The local OpenCode list publishes independently.
Message pages and single-message reads add the existing tool words and preserve
the original failure details. The port uses the assistant vectors and the same
56 tool labels. The MCP tool owners supply their `tools/list` descriptors.

### Platform verification

The test builds include `main` through `d967478b2`. Clippy with warnings
denied passed for the engine, OS and resident crates on macOS, native
Windows's cross target, WSL GNU and the x86-64 musl build.

| Platform | Binary scenarios | Additional checks |
| --- | --- | --- |
| macOS | 27 TypeScript and 27 Rust | Rust resident and session move tests, beta inventory and queue, ten-step differential |
| WSL GNU | 26 Rust | Rust resident test |
| Native Windows | 26 Rust | Rust resident test |
| Linux on Lokum | 26 Rust | Mac-built static musl binary in a foreground container with one CPU, 1 GB and automatic removal |

The live queue test used OpenCode 2.2.6 in scratch homes on all four
platforms. Lokum's first capped attempt reached the worktree scenarios
and stopped because the staged Git executable had the wrong basename.
The passing attempt reused the same prebuilt Rust binary with that
fixture corrected. The container and both Lokum scratch homes were gone
afterward. The 16 targeted TypeScript reference tests also passed on macOS.

### Phase 3.2c verification

The live test uses the pinned OpenCode 2.2.6 and
`opencode-go/deepseek-v4.1-flash`. A real question keeps the turn active
while another message is saved, edited and recovered from its delivery
receipt. The test answers the question and checks for one copy of the
draft in the transcript. The deterministic tests use the loopback
OpenCode API fixture to place a restart on either side of acceptance.

The model preference differential agreed on five replies and
`box/models.json`. The desktop differential agreed on ten replies and the
final engine transcript, with one forced restart during send. Both runs
also compared the full home. Rust keeps a bounded in-memory answer cache for raw
engine prompts and a durable journal for the desktop send. A Tokio
worker settles model refreshes off the engine event reader. The outbox
commits the terminal receipt before deleting the active draft, and
publishes receipt changes through a watch channel.

| Platform | Phase 3.2c checks |
| --- | --- |
| macOS | TypeScript passed 128 deterministic cases and the recorded beta home. Rust passed 127 deterministic cases and the recorded beta home before the new Delete and Delete All worktree scenario was added. Both passed the OpenCode Go live scenario. The Rust resident test also verifies the owner adapters, revoked delivery and bounded transcript reads. |
| WSL GNU | 75 deterministic Rust cases after merging the move fences, plus the earlier OpenCode Go live scenario, with Mac-built executables. The held-inventory, old waiting-list, and concurrent-delete scenarios also passed. |
| Native Windows | 119 deterministic Rust cases at the post-enrollment checkpoint, including two-resident setup search, signed stars, scoped and multipart addressed delivery, vault asks, the native project key, attachment paths and the delete race. An early project watch scenario failed there and passed on macOS after the inventory change. The staged OpenCode pin reports version 2.2.6. |
| Linux musl | 82 deterministic Rust cases with the Mac-built static executable under WSL. An earlier 35 cases passed on Lokum. |

The additional OpenCode Go run passed on both macOS boxes and the GNU
Rust build under WSL. It checks Stop during a live turn, an inbox draft and
a resident draft waiting together, then one exact transcript entry for
each draft with a completed turn between them. The scratch provider entry
was deleted from both hosts after the runs.

The signed star test makes a change from a member, reads it from both
computers, then merges older local copies after an unstar. It verifies the
removed star stays absent. The paired delivery adapter fixture replaces the
grant after saving the draft, verifies the worker refuses the old pairing,
and reads the terminal receipt through the new pairing.

### Differential file verdicts

The latest model preference run reported 76 differing home paths, the
ten-step desktop run reported 88, and the nine-step queue run reported 82.
The queue run agreed on the engine transcript. Five reply fields differed in
the waiting entries' wall-clock timestamps. The table includes older baseline
paths as well.
A position such as "run/link.json/value/key" is a field difference inside
"run/link.json". Wildcards cover only the generated filenames at the stated
depth. The row remains responsible for its files and for proving compatibility
where another version reads them. Directory entries are listed beside the
records or runtime endpoint they hold.

| Differing home paths | Owner | What differs |
| --- | --- | --- |
| `agents`, `run/agents.json` | 4.1 | Agent runtime and roster |
| `agents/limits.json`, `agents/usage`, `agents/usage/claude-files`, `agents/usage/codex-files`, `agents/usage/grok-files`, `agents/usage/insights.sqlite`, `agents/usage/insights.sqlite-shm`, `agents/usage/insights.sqlite-wal`, `agents/usage/settings.json`, `agents/usage/usage.json`, `run/usage-attribution.json` | 5.4 | Usage, limits and the Insights database sidecars |
| `artifacts`, `artifacts/citations-v1.json`, `artifacts/copies`, `artifacts/index-v1.sqlite`, `artifacts/key`, `artifacts/log`, `box/marketplace`, `box/marketplace/mcp-relay-v1.json`, `library`, `library/backfill.json`, `run/docs.changed` | 5.5 | Artifact storage, Library, and Marketplace relay and document invalidation |
| `bots`, `run/bots-seen`, `run/notices`, `run/notices/teller`, `run/work`, `run/work/background.json`, `run/work/told.json`, `wakes` | 5.1 | Bots, wakes, work activity and notices |
| `box/approval.json` | 6.1 | TypeScript's `approvalsSince` reads the differing `since` field to distinguish old and new sessions during approval adoption. Rust session start reads the shared `last` and `fullAccess` values. |
| `chats` | 1.4 | TypeScript creates the folder on the first `kuvo/info` request. Rust creates it while preparing the resident's info response. |
| `box/found-folders.json` | 6.3 | First-seen project folders |
| `config`, `config/opencode`, `config/opencode/plugins`, `config/opencode/plugins/kuvo-defaults.js`, `run/fence`, `run/fence/agents.md` | 6.1 | Engine configuration and the sandbox instructions |
| `run/.queue-handoffs` | Unread outside the old lock | `rg -n -F .queue-handoffs box/src box/crates` finds only the TypeScript per-item lock in `engine/queue.ts`. Rust uses the session turn. |
| `run/glance`, `run/glance/asks.json` | Identical | Both writers publish the `asks` object that the TypeScript reader in `glance/asked.ts` consumes. |
| `run/glance-source.json` | Unread by released readers | `rg -n -F glance-source.json box/src box/crates gpui` finds only the Rust checkpoint reader and writer. The digest binds the checkpoint to its OpenCode process. |
| `run/lists/sessions.json` | Identical format | Both boxes and the Rust peer publisher read the list. The floor and row sequence differed after restarting the desktop case. Rust advances a complete inventory as one revision. The cursor scenario proves that the saved format supplies a cached first frame, that subsequent changes send only changed rows, and that a pre-restart cursor gets a full refresh. |
| `run/notes` | 7.3 | TypeScript creates this directory during the desktop replay. Rust creates it on the first guarded note write. Both claim the same per-session lock name and clear stale empty locks. The scenario also holds `run/work/.notes.lock` while a current request waits. Phase 7.3 owns the run alongside a released process. |
| `run/queues-scanned.json` | Identical | Both writers use a format-one port-to-process map. A same-process restart with a saved receipt resumes its inbox and preserves a stopped session in the conformance scenario. |
| `run/github-sign-in`, `run/work-queue`, `run/work-queue/forward`, `run/work-queue/model.json`, `run/work-queue/numbers.json` | 5.2 | Work Queue model, numbering, forwarding and GitHub credential exchange |
| `run/link.json`, `run/setup-card.json`, `run/setup-checkpoints/*.json` | 2.1 | Setup bootstrap identity, signed shared values and outcomes, and local checkpoint projection |
| `run/link.seen`, `run/link.version` | 7.3 | Legacy Mac activity and link version markers |
| `run/move-prompt-writes`, `run/move-prompt-writes/*`, `run/moves`, `run/owned-data`, `run/owned-data/mutations`, `run/owned-data/mutations/*.json` | 2.4 | Move writer fences and the shared owner-mutation journal |
| `run/outbox`, `run/outbox/sending` | 1.5 | Relay outbox directories and pending envelopes |
| `run/prompt-receipts`, `run/prompt-receipts/*.json` | Unread by released readers | `rg -n -F prompt-receipts box/src box/crates gpui` finds only the Rust send journal. Released clients read the route, queue and transcript. Scoped and listing receipts use separate records. |
| `run/queue-removals/spent/*` | Identical | Both boxes now write the same empty marker keyed by session, draft and edit receipt. An earlier queue differential compared empty bytes with Rust's JSON `null`. |
| `run/queues.json/value/*/resumed` | Identical format | Both writers record when Send Now resumes the queue. The two runs happened at different wall-clock times. |
| `run/route/ssh`, `run/socket.key` | 1.4 | Resident IPC endpoint and its random local credential |
| `run/vault.sock`, `run/ssh-for` | 6.2 | Vault IPC and session SSH-agent registration |
