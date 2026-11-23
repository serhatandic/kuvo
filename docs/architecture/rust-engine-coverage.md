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
`engine-live` also uses an installed pinned engine supplied in
`KUVO_CONFORMANCE_OPENCODE`, with a local model response held open to put
subsequent sends in its real inbox.
`kuvo_engine/tests/resident.rs` additionally interrupts an inbox transfer
after the first message, restarts the resident, and checks delivery order.
### TypeScript scenario map

Numbers are one-based test declarations in each file at `a21b64fc5`.
The original 294 cases include 11 covered, 87 partial and 196 gaps.
Covered cases have direct Rust evidence. Partial cases exercise only part of
the original result or use the local OpenCode boundary where the original
also uses a bridge, peer, settings route, or failure injection. Multiple
owners on the same case each own the part described by their phase.

- Message parts covers all 13 read and all 25 compose vectors. The two retained-file
  vectors use real inline payloads and check the recovered file bytes, the prompt
  files, and both kinds of attachment notes. Validation and error recovery are separate.
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
| `box/test/message-parts.test.ts` (18) | 2, 5, 11, 17 | 1, 3–4, 8–10, 12–16, 18 | 6–7 | 3.2 · 1, 3–4, 6, 8–10, 12–16, 18<br>4.1 · 7 |
| `box/test/engine-context.test.ts` (1) | 1 | — | — | — |
| `box/test/engine-swap-inboxes.test.ts` (2) | 1–2 | — | — | — |
| `box/test/queue-stop.test.ts` (6) | — | 1, 4 | 2–3, 5–6 | 3.2 · 1–6 |
| `box/test/queue-items.test.ts` (25) | 7 | 1–6, 9–10, 13, 21–23 | 8, 11–12, 14–20, 24–25 | 3.2 · 1–6, 8–24<br>4.1 · 25<br>5.1 · 21 |
| `box/test/resident-delivery.test.ts` (2) | — | 1 | 2 | 4.1 · 1<br>5.2 · 2 |
| `box/test/session-actions.test.ts` (26) | — | 1, 3–5, 14, 20 | 2, 6–13, 15–19, 21–26 | 3.2 · 1, 3–10, 14–16, 20, 23–26<br>4.1 · 2, 10<br>2.4 · 11–13, 19<br>5.1 · 3, 5<br>6.1 · 17–19, 21–22<br>6.3 · 7–8, 15–16, 24 |
| `box/test/session-pages.test.ts` (3) | — | 1–2 | 3 | 3.2 · 1–3 |
| `box/test/session-lists.test.ts` (47) | — | 3–5, 10, 15–16, 20, 23–24, 46 | 1–2, 6–9, 11–14, 17–19, 21–22, 25–45, 47 | 3.2 · 1–17, 19–24, 26–29, 46–47<br>4.1 · 14<br>5.1 · 18, 23, 25–28, 30–45<br>6.3 · 11, 16–17, 46<br>2.4 · 45<br>7.3 · 16 |
| `box/test/session-notes.test.ts` (8) | — | 6 | 1–5, 7–8 | 3.2 · 1–8 |
| `box/test/session-notes-writers.test.ts` (7) | — | 1 | 2–7 | 3.2 · 1–7<br>7.3 · 6 |
| `box/bridge-tests/queue.test.js` (56) | — | 2, 9–10, 13–14, 22, 27, 29–33, 43, 54 | 1, 3–8, 11–12, 15–21, 23–26, 28, 34–42, 44–53, 55–56 | 3.2 · 1–39, 43, 51–56<br>4.1 · 44–50<br>4.6 · 40–42<br>5.1 · 5, 25<br>5.4 · 17–21 |
| `box/bridge-tests/queue-live.test.js` (1) | — | 1 | — | 3.2 · 1 |
| `box/bridge-tests/start-session.test.js` (5) | — | 1, 3–4 | 2, 5 | 3.2 · 1–5<br>5.1 · 2<br>6.3 · 3–5 |
| `box/test/typed.test.ts` (9) | 1, 3 | 2, 4, 6–9 | 5 | 3.2 · 2, 4, 6–9<br>4.1 · 5 |
| `box/test/draft-orphan.test.ts` (5) | 2 | 1, 3–4 | 5 | 3.2 · 1, 3–5 |
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
They include 4 covered, 15 partial and 98 gaps.
Each row uses the same one-based numbering and ownership rule.

| Source | Covered | Partial | Gap | Owners for partial and gap cases |
| --- | --- | --- | --- | --- |
| `box/test/session-asks.test.ts` (19) | — | 1, 3–4, 18 | 2, 5–17, 19 | 3.2 · 1–5, 13–19<br>6.1 · 6–7, 12–13, 19<br>6.2 · 8–13, 19<br>2.4 · 19 |
| `box/test/model-readiness.test.ts` (1) | — | 1 | — | 3.2 · 1 |
| `box/test/models-offered.test.ts` (15) | — | — | 1–15 | 3.2 · 1–7, 9–13<br>4.1 · 1–2, 5, 8–9, 14–15<br>5.1 · 8<br>5.4 · 14–15 |
| `box/test/models-starred.test.ts` (7) | — | — | 1–7 | 3.2 · 1–7<br>2.1 · 4–7<br>7.3 · 3 |
| `box/test/models-speed.test.ts` (3) | — | — | 1–3 | 3.2 · 1–3<br>4.2 · 1 |
| `box/test/model-aliases.test.ts` (3) | — | — | 1–3 | 4.1 · 1–3<br>5.4 · 3 |
| `box/test/model-names.test.ts` (3) | — | — | 1–3 | 3.2 · 1–3 |
| `box/test/search-snippet.test.ts` (4) | — | 3 | 1–2, 4 | 3.2 · 1–4 |
| `box/test/attachment-paths.test.ts` (2) | — | — | 1–2 | 3.2 · 1–2 |
| `box/test/add-address.test.ts` (3) | — | — | 1–3 | 2.4 · 1–3<br>3.2 · 1–3 |
| `box/bridge-tests/archive-waiting.test.js` (3) | 1 | — | 2–3 | 5.1 · 2<br>6.3 · 3 |
| `box/bridge-tests/attachments-go.test.js` (6) | — | — | 1–6 | 3.2 · 1–6<br>4.1 · 2–4<br>2.4 · 5<br>7.3 · 1, 3 |
| `box/bridge-tests/search.test.js` (11) | — | 3, 5–7, 10 | 1–2, 4, 8–9, 11 | 3.2 · 1–11<br>4.1 · 3 |
| `box/bridge-tests/session-models.test.js` (33) | 1, 3, 20 | 5, 18–19, 32 | 2, 4, 6–17, 21–31, 33 | 3.2 · 2, 4–5, 9–10, 12–14, 16–19, 22–30, 32–33<br>4.1 · 6–9, 11, 15–16, 21, 23, 26–27, 31 |
| `box/bridge-tests/models-refresh.test.js` (4) | — | — | 1–4 | 3.2 · 1–4 |

### Implementation and remaining parity work

The Rust queue uses the TypeScript index and waiting payload paths and the
shared Rust atomic file writer. A
per-session turn orders writes and an owned permit serializes durable index
replacement off the resident executor. Removal and Undo leave a spent token
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
engine inbox.

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
through the receipt topic. The scope is local desktop delivery in a standalone
setup. Scoped submissions, paired and addressed delivery, editing a saved
outbox draft, move fences, and mixed-version receipt indexing remain open.
Phase 2.4 owns the shared owner journal and peer forwarding. Phase 3.2 owns
the engine's submission and recovery routes and the rest of the local delivery contract.

The engine's event reader owns the local ask cache. Its session list and
waiting count use the same snapshot. Normal questions and permissions are
answered in their original session, and archiving holds the inbox and stops
a run waiting on a question or permission. Global MCP forms, vault and
sandbox asks, revocable consent, and the full bot and Work Queue counters
still need their own adapters. The wake pause route and project removal
remain in 5.1 and 6.3. A worktree whose session creation gets a failure
response is removed only after a complete inventory shows no conversation
using it. Ambiguous transport failures leave the worktree kept.

Model readiness reads the conversation folder's OpenCode catalog and
remembers its names. The picker, stars, speed, provider refresh, and
coding-agent catalogs remain. Search caches local engine transcripts by
session revision. Projection of a coding-agent transcript waits for 4.1.
Attachment ingest covers the kept bytes and notes. Retention across branch,
move and removal and the raw-prompt attachment path remain open.
Engine-swap inbox transfer is independent of the release keeper in 7.2.

### Differential file verdicts

The table assigns all 76 paths reported by the final ten-step desktop comparison.
It also records the legacy presence marker seen in the earlier run.
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
| `bots`, `run/bots-seen`, `run/notes`, `run/notices`, `run/notices/teller`, `run/work`, `run/work/background.json`, `run/work/told.json`, `wakes` | 5.1 | Bots, wakes, work activity, notes and notices |
| `box/approval.json`, `chats` | 3.2 | Session approval timestamp at creation and the Chats folder lifecycle |
| `box/found-folders.json` | 6.3 | First-seen project folders |
| `config`, `config/opencode`, `config/opencode/plugins`, `config/opencode/plugins/kuvo-defaults.js`, `run/fence`, `run/fence/agents.md` | 6.1 | Engine configuration and the sandbox instructions |
| `run/.queue-handoffs`, `run/glance`, `run/glance/asks.json`, `run/lists/sessions.json`, `run/queues-scanned.json` | 3.2 | Queue recovery markers, local list sequence and asks cache |
| `run/github-sign-in`, `run/work-queue`, `run/work-queue/forward`, `run/work-queue/numbers.json` | 5.2 | Work Queue numbering, forwarding and GitHub credential exchange |
| `run/link.json`, `run/setup-card.json`, `run/setup-checkpoints/*.json` | 2.1 | Setup bootstrap identity, signing documents, and local checkpoint projection |
| `run/link.seen`, `run/link.version` | 7.3 | Legacy Mac activity and link version markers |
| `run/move-prompt-writes`, `run/move-prompt-writes/*`, `run/moves`, `run/owned-data`, `run/owned-data/mutations`, `run/owned-data/mutations/*.json` | 2.4 | Move writer fences and the shared owner-mutation journal |
| `run/outbox`, `run/outbox/sending` | 1.5 | Relay outbox directories and pending envelopes |
| `run/prompt-receipts`, `run/prompt-receipts/*.json` | 3.2 | Rust prompt claim and answer journal, including its mixed-version recovery seam |
| `run/route/ssh`, `run/socket.key` | 1.4 | Resident IPC endpoint and its random local credential |
| `run/vault.sock`, `run/ssh-for` | 6.2 | Vault IPC and session SSH-agent registration |
