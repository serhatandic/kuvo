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
The first inventory recorded 294 cases. Across both inventories, 124 of
411 declarations are covered, 90 are partial and 197 are gaps.
Row 3.2 retains 160 partial or gap declarations.
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
| `box/test/queue-stop.test.ts` (6) | 1–2, 4–6 | 3 | — | 3.2 · 3 |
| `box/test/queue-items.test.ts` (25) | 7, 9–10, 22 | 1–6, 13, 21, 23 | 8, 11–12, 14–20, 24–25 | 3.2 · 1–6, 8, 11–21, 23–24<br>4.1 · 25<br>5.1 · 21 |
| `box/test/resident-delivery.test.ts` (2) | — | 1 | 2 | 4.1 · 1<br>5.2 · 2 |
| `box/test/session-actions.test.ts` (26) | 1, 4, 9, 14, 25 | 3, 5, 20 | 2, 6–8, 10–13, 15–19, 21–24, 26 | 3.2 · 6–8, 10, 15–16, 20, 23–24, 26<br>4.1 · 2, 10<br>2.4 · 11–13, 19<br>5.1 · 3, 5<br>6.1 · 17–19, 21–22<br>6.3 · 7–8, 15–16, 24 |
| `box/test/session-pages.test.ts` (3) | 1–2 | 3 | — | 4.1 · 3 |
| `box/test/session-lists.test.ts` (47) | 4–5, 10, 24, 27 | 3, 15–16, 19–20, 23, 46 | 1–2, 6–9, 11–14, 17–18, 21–22, 25–26, 28–45, 47 | 3.2 · 1–3, 6–9, 11–17, 19–23, 26, 28–29, 46–47<br>4.1 · 14<br>5.1 · 18, 23, 25–28, 30–45<br>6.3 · 11, 16–17, 46<br>2.4 · 45<br>7.3 · 16 |
| `box/test/session-notes.test.ts` (8) | 2 | 6 | 1, 3–5, 7–8 | 3.2 · 1, 3–8 |
| `box/test/session-notes-writers.test.ts` (7) | 3 | 1 | 2, 4–7 | 3.2 · 1–2, 4–7<br>7.3 · 6 |
| `box/bridge-tests/queue.test.js` (56) | 6–7, 15–21, 33, 39 | 2, 9–10, 13–14, 22, 27, 29–32, 43, 54 | 1, 3–5, 8, 11–12, 23–26, 28, 34–38, 40–42, 44–53, 55–56 | 3.2 · 1–5, 8–14, 22–32, 34–38, 43, 51–56<br>4.1 · 44–50<br>4.6 · 40–42<br>5.1 · 5, 25 |
| `box/bridge-tests/queue-live.test.js` (1) | — | 1 | — | 3.2 · 1 |
| `box/bridge-tests/start-session.test.js` (5) | — | 1, 3–4 | 2, 5 | 3.2 · 1–5<br>5.1 · 2<br>6.3 · 3–5 |
| `box/test/typed.test.ts` (9) | 1–4, 6–9 | — | 5 | 4.1 · 5 |
| `box/test/draft-orphan.test.ts` (5) | 1–4 | 5 | — | 3.2 · 5 |
| `box/test/conversation.test.ts` (3) | — | 3 | 1–2 | 3.2 · 1–3 |
| `box/test/conversation-activity.test.ts` (2) | — | — | 1–2 | 3.2 · 1<br>4.1 · 2<br>5.1 · 2 |
| `box/test/message-words.test.ts` (11) | — | 8–9 | 1–7, 10–11 | 3.2 · 1–2, 4–7<br>5.1 · 8–10<br>5.2 · 3<br>5.3 · 11<br>6.1 · 3<br>6.2 · 3 |
| `box/test/session-news-kinds.test.ts` (5) | — | — | 1–5 | 5.1 · 1–5 |
| `box/test/projects-worktrees.test.ts` (6) | — | 1–4 | 5–6 | 3.2 · 1–4<br>6.3 · 1–6 |
| `box/test/start-cancelled-worktree.test.ts` (2) | — | 1–2 | — | 3.2 · 1–2<br>6.3 · 1–2 |
| `box/bridge-tests/worktrees.test.js` (32) | — | 1, 8–9, 14 | 2–7, 10–13, 15–32 | 3.2 · 1, 8–9, 13–14, 17–18, 24–30<br>4.1 · 12, 30<br>6.3 · 1–32<br>7.3 · 15–16 |
| `box/bridge-tests/worktrees-where.test.js` (12) | — | — | 1–12 | 6.3 · 1–11<br>5.5 · 12 |

### Additional 3.2b reference cases

The following 117 declarations extend the original 294-case inventory.
Each row uses the same one-based numbering and ownership rule.

| Source | Covered | Partial | Gap | Owners for partial and gap cases |
| --- | --- | --- | --- | --- |
| `box/test/session-asks.test.ts` (19) | 1, 3–5, 14–18 | 2, 6–11, 19 | 12–13 | 3.2 · 13, 19<br>4.1 · 2<br>6.1 · 6–7, 12–13, 19<br>6.2 · 8–13, 19<br>2.4 · 19 |
| `box/test/model-readiness.test.ts` (1) | — | 1 | — | 3.2 · 1 |
| `box/test/models-offered.test.ts` (15) | 3–4, 6–7, 13 | 1–2, 5, 8–12, 14–15 | — | 3.2 · 10–11<br>4.1 · 1–2, 5, 8–9, 11–12, 14–15<br>5.1 · 8<br>5.4 · 14–15 |
| `box/test/models-starred.test.ts` (7) | 1–2, 4–7 | — | 3 | 7.3 · 3 |
| `box/test/models-speed.test.ts` (3) | 2–3 | 1 | — | 4.2 · 1 |
| `box/test/model-aliases.test.ts` (3) | — | — | 1–3 | 4.1 · 1–3<br>5.4 · 3 |
| `box/test/model-names.test.ts` (3) | 1–3 | — | — | — |
| `box/test/search-snippet.test.ts` (4) | 1–4 | — | — | — |
| `box/test/attachment-paths.test.ts` (2) | — | 1–2 | — | 2.4 · 1–2<br>2.5 · 1–2<br>4.1 · 1 |
| `box/test/add-address.test.ts` (3) | — | — | 1–3 | 2.3 · 1–3 |
| `box/bridge-tests/archive-waiting.test.js` (3) | 1 | — | 2–3 | 5.1 · 2<br>6.3 · 3 |
| `box/bridge-tests/attachments-go.test.js` (6) | 6 | 1–5 | — | 3.2 · 1–4<br>4.1 · 1–4<br>2.4 · 5<br>7.3 · 1, 3 |
| `box/bridge-tests/search.test.js` (11) | 1–2, 4–11 | 3 | — | 3.2 · 3<br>4.1 · 3 |
| `box/bridge-tests/session-models.test.js` (33) | 1–4, 10, 13–14, 17, 20, 22, 28–30, 33 | 5, 16, 18–19, 26, 32 | 6–9, 11–12, 15, 21, 23–25, 27, 31 | 3.2 · 5, 9, 12, 18–19, 23–25, 27, 32<br>4.1 · 6–9, 11, 15–16, 21, 23, 26–27, 31 |
| `box/bridge-tests/models-refresh.test.js` (4) | 1–2, 4 | 3 | — | 6.1 · 3 |

### Implementation and remaining parity work

The Rust queue uses the TypeScript index and waiting payload paths and the
shared Rust atomic file writer. A Tokio task orders writers by session or
receipt key. A dropped caller releases its turn, and an unrelated session
can write while one is waiting on OpenCode. Forwarded session PATCH requests
use the same turn as resident writers. An owned permit serializes durable
queue index replacement off the resident executor. Removal and Undo leave a spent token
so a retry cannot take a new occurrence. Direct sends journal the original
request before posting. On retry the resident checks the saved answer, the
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
and sandbox owners (6.1 and 6.2). Local bot activity includes delegation
and the Work Queue counts newer copies once; `PeerWaiting` takes the live
peer-activity snapshot from 5.1. The wake pause route and project removal
remain in 5.1 and 6.3. A worktree whose session creation gets a failure
response is removed only after a complete inventory shows no conversation
using it. Ambiguous transport failures leave the worktree kept.

Model readiness reads the conversation folder's OpenCode catalog and
remembers its names. Concurrent clients share reads by engine and folder.
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
Raw prompts and commands use the same attachment ingest as the composer,
with the raw prompt's own send-once response cache. Expiry preserves moved
payloads and transcript pictures. `AttachmentSource` carries the other
runtime's complete inventory and move protections. Removal waits until both
runtimes can account for any branch references.
Path remapping is exercised through the backup and move consumers in 2.4
and 2.5, with coding-agent carry in 4.1. Installer address parsing belongs
to enrollment in 2.3.
Engine-swap inbox transfer is independent of the release keeper in 7.2.
Queue limit stops keep their reason even if OpenCode's inbox is unavailable.
The resident recovers missed failures from the transcript and looks up the
current engine before it releases a reset. Its next-due task checks the
persisted state again under the session turn, so a manual Resume or Stop
wins. The same release can retry a refused send or an in-progress removal.

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
| macOS | 59 deterministic engine cases against each binary at the catalog and session-writer checkpoint. Both also passed the beta fixture and the OpenCode Go live scenario. The Rust resident scenario verifies the owner adapters and revoked delivery. |
| WSL GNU | 75 deterministic Rust cases after merging the move fences, plus the earlier OpenCode Go live scenario, with Mac-built executables. |
| Native Windows | 57 deterministic Rust cases at the setup search checkpoint, including Windows project keys. The staged OpenCode pin reports version 2.2.6. |
| Linux musl | 57 deterministic Rust cases at the setup search checkpoint with the Mac-built static executable under WSL. An earlier 35 cases passed on Lokum. |

The signed star test makes a change from a member, reads it from both
computers, then merges older local copies after an unstar. It verifies the
removed star stays absent. The paired delivery adapter fixture replaces the
grant after saving the draft, verifies the worker refuses the old pairing,
and reads the terminal receipt through the new pairing.

### Differential file verdicts

The latest model preference run reported 81 differing paths, and the
ten-step desktop run reported 93. The table includes the older baseline paths as well.
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
| `run/lists/sessions.json` | 3.2 | Both boxes and the Rust peer publisher read the list. Rust writes the same owner, start identity and `workByBot` fields and advances each inventory as one revision. The latest desktop restart differed in the saved floor and row sequence. The preceding run agreed on both fields. The cursor scenario now proves a cached first frame, a later inventory, and full refresh for an old cursor across resident restart. |
| `run/notes` | 3.2, 7.3 | TypeScript keeps per-session writer locks. Rust orders these writes through the resident. Compatibility with the released lock and stale lock cleanup still need proof. |
| `run/queues-scanned.json` | Identical | Both writers use a format-one port-to-process map. A same-process restart with a saved receipt resumes its inbox and preserves a stopped session in the conformance scenario. |
| `run/github-sign-in`, `run/work-queue`, `run/work-queue/forward`, `run/work-queue/model.json`, `run/work-queue/numbers.json` | 5.2 | Work Queue model, numbering, forwarding and GitHub credential exchange |
| `run/link.json`, `run/setup-card.json`, `run/setup-checkpoints/*.json` | 2.1 | Setup bootstrap identity, signed shared values and outcomes, and local checkpoint projection |
| `run/link.seen`, `run/link.version` | 7.3 | Legacy Mac activity and link version markers |
| `run/move-prompt-writes`, `run/move-prompt-writes/*`, `run/moves`, `run/owned-data`, `run/owned-data/mutations`, `run/owned-data/mutations/*.json` | 2.4 | Move writer fences and the shared owner-mutation journal |
| `run/outbox`, `run/outbox/sending` | 1.5 | Relay outbox directories and pending envelopes |
| `run/prompt-receipts`, `run/prompt-receipts/*.json` | Unread by released readers | `rg -n -F prompt-receipts box/src box/crates gpui` finds only the Rust send journal. Released clients read the route, queue and transcript. Scoped and listing receipts use separate records. |
| `run/route/ssh`, `run/socket.key` | 1.4 | Resident IPC endpoint and its random local credential |
| `run/vault.sock`, `run/ssh-for` | 6.2 | Vault IPC and session SSH-agent registration |
