# Rust Work Queue coverage

Row 5.2 is open. The current slice covers local migration and reads, the watch
stream, saved attachments and cached GitHub state. It creates items with a durable
receipt, deletes and restores local items, spends and exchanges ordinary number
blocks on the relay lease, and saves signed labels. It also edits local text,
status, order and fields, reads and writes manual relations, previews Markdown
references, and stages client attachments with stable anchors and collision-safe
names. Inline picture markers can be removed separately from their files. The
peer snapshot capability exchanges combined views, retained deletions and
reference chips. Rust snapshots also carry optional per-status counts of
unlisted items, so setup-wide status and computer totals include older
closed work. The two-resident conformance case checks all 150 items across
the 100-row limit, reopening an older record and closing it again. It is
skipped on the frozen TypeScript box, which still caps remote counts.
Text, source, label and individual-owner filtering still counts matching
listed rows; the aggregate does not describe the omitted rows' contents.
Whole items and attachments can be read quietly from their
keeper through the same transport as setup search. A computer can send an item
edit, delete or restore to its keeper through the tracked request and receipt
exchange. Markdown imports read the backlog files and claim each number on the
main's lease. Imports on a member wait for the main's reply. A member can save
an edit against a revision and replay it to the keeper when it returns. The
keeper merges independent passages and records competing values as choices.
Expired deletions keep a fence that prevents an offline draft from recreating
the item. Compaction prunes stored relations and retires its attachments.
Visible GitHub issues are read through the configured `gh` account. The cache
and lock use the TypeScript formats. An agent can add an issue through a
verified tool call, and a second computer can read its full item. Device sign-in
keeps the located attempt and code tied to the requesting client. The full
client feature is still gated on the remaining routes.

Verified queue tools add and number ordinary items, list and read work, and
import structured backlogs. An agent's attachments are staged from its allowed
folders after checking saved paths and file identity. The tool read encloses a
GitHub description in a fence that cannot be closed by the description itself.
An agent can link an existing numbered item to a GitHub issue. Its old number
continues to find it, and its previous description is kept as a note.
On the keeper, agent updates and deletions record the calling conversation.
An agent can undo an agent deletion, and user deletions remain the user's to
restore. A verified tool on another computer can send an edit, attachment,
deletion or undo to the keeper and preserve the conversation in the history.
Agents can also manage the main computer's label library. Removing a label
records the agent on the items it changed. Working links record starts and stops and are cleared when work closes. Starting
work on a fourth item drops the oldest link and records its stop. Agents can
remove a saved file and link, and attachments linked to a GitHub item use the
same file staging as ordinary agent edits.
An agent can hand an item to a conversation on the same computer. The new
queue is written before the old copy is retired, and the item stays in both
conversations' views. Across computers, the keeper sends the item and its
files over the existing Work Queue relay. A checkpointed destination stages the
item through a prepare and release exchange. The source retires it only after
the destination saves the item and its receipt. The original number and the
source conversation's view survive. An attachment that needs multiple relay
frames travels with the moved item's saved decision. Other items' decisions
stay on the source. A full conversation move also carries its queue items,
original numbers, deletion fences, files and exactly the forwarding decisions
addressed to those records. The two-resident scenario carries a file that
had no saved digest, and verifies the destination's durable proof. If the
source attachment changes before the switch, the original stays there and
the destination rolls back its file, item and deletion fence. The ordinary
conversation move passes with either Rust or TypeScript at each end.
The private label restore command previews the main computer's accepted
library over the quiet read transport. A member sends a restore to the main,
which signs the save and publishes it to the setup. Migrated items settle after
the engine can report which sessions are active. That one-time check leaves
their existing update timestamps in place. Queue writes, numbering
and backup capture use the existing PID-marker directory locks. A held writer
does not stop other resident routes, and a dead writer's lock is reclaimed.

Case numbers follow declaration order in each TypeScript file. Partial means a
conformance scenario exercises part of the TypeScript case. Open cases are
assigned to the indicated row for the rest of the port.

| TypeScript suite | Cases | Status and owner |
|---|---:|---|
| [work-queue](../../box/bridge-tests/work-queue.test.js) | 1–5, 7–8, 10–11, 13, 16–19, 23, 25, 28, 31–32, 42–44, 49, 51, 57 | Partial, 5.2. Creation retry, summary counts and order, local delete and restore, label writes, saved attachment download and a view change written by another process. A verified agent adds, lists and reads numbered work, including progress, history and attachments. Agent updates, deletion, undo, starts and stops record the agent's identity. A quick restart folds an intervening stop. A local handoff stays visible in both queues' views with one saved item. Changed invocation arguments fail admission. Expired attachments are retired. Both client Markdown and agent structured imports preserve status and number, and a member receives its claimed numbers from the main. History keeps a stop when a note intervenes, and edits retain an older external note author's words. Multiple offline imports reserve numbers before ordinary waiting items can take them. A newer unknown queue owner remains untouched even with an unreadable format or a newer copy of a local item. Move packing/forgetting of unknown owners, subagent, one-shot, working links across computers, project folder, late grants and import failure variants still need scenarios. |
| | 6, 14–15, 20–22, 26–27, 29, 33–35, 37–38, 41, 46–48, 50, 54, 60 | Covered. The fourth link replaces the oldest one; consecutive reorders by the same author fold, but another author, status change, intervening note or a clock ahead does not. Work starts again after reopening. Unknown history entries survive within the last 50; legacy items gain no invented history. Interrupted local handoffs retire the older duplicate on the next write, unreadable owner files are set aside, and unknown fields survive edits. Offline imports skip repeats, reserve available claims, and renumber collisions with a note; a returned number waits for the item holding it and never displaces a deletion. The lease grants and replays blocks, a lost ledger recovers above kept numbers and settles pending work, and far or seven-digit import claims are renumbered without skipping the ordinary range. Unknown chips return null. A deleted item with a future status returns to Todo with an attributed move; old deleted labels stay removed. Seventy converging reorders preserve position. Agent reads describe both directions of manual links and mention backlinks. |
| | 12, 24, 36, 39–40, 45, 52–53, 55–56, 58–59 | Open, 5.2. Agent operations, live session variations, edits, remaining import cases, staging, recovery and handoff. |
| | 9 | Dependency on 5.1e's agent `start_session` route. After the real invocation is verified and a child address exists, but before sending its first task, call the queue linking hook with the explicit item reference or first task mention, preserving the three-link bound and attribution. Unverified calls may not link work. Prove the child can read and update the linked item; do not implement a second start_session route in 5.2. |
| | 30 | Owned by 5.3. Prove the group chat seat's queue owner and local handoff, and reject edits by a seat on a computer that does not keep the group. The queue tools already check group ownership; integrate the real group and seat route when ported. |
| [work-queue-across](../../box/bridge-tests/work-queue-across.test.js) | 1, 3–4, 9–10, 13, 16, 20, 23–26, 30–31 | Partial, 5.2. Authenticated snapshots and acknowledgements carry remote rows and chips through the relay. The shared memory transport reads the keeper's full item and file. Client and attributed agent edits, attachments, deletions and restores reach the keeper from another computer. Shared label changes reach both residents. A two-resident handoff preserves the number and file, and leaves a single live item. The ordinary handoff also passes with either resident running TypeScript. A multipart snapshot stays invisible until complete, then appears once despite a duplicate part. A disconnected keeper's rows retain their status but stop showing active work and reject edits; reconnecting restores live projection and edits. An older unlisted item can be read with its full attachment and edited at the actual keeper, even with another peer away; absence is distinguished from uncertainty. Mac and Windows initially hit an announcement-before-view publication race in that fixture; waiting for the advertised capabilities and in-memory publication passed the targeted reruns and the complete WSL area. The later storage slice also waited for the third peer's capability publication before asserting a definitive absence (a found item can already answer during that interval). Native Windows file reads in two slices returned a transient 404 during startup; each passed its single targeted rerun. No attachment-read code changed in either slice. Ordinary concurrent bookkeeping writes in the same directory reproduce the false refusal in both boxes; the directory revision can change without the directory being replaced. The Rust-only regression scenario reads an unchanged attachment while sibling bookkeeping files are created and removed. Directory handles remain nofollow and compare native identity, while the opened leaf retains its complete identity/revision check. The frozen TypeScript box still refuses some reads under this load; that case is skipped there. A dropped full handoff letter leaves the original unchanged until a retry saves its number, note and history on the destination. Handoffs return after 30 days or three days after a computer leaves; deletion cancels a pending handoff. A deleted GitHub item cannot be restored by either a client or a verified agent beside a live peer twin, and handoff compares the stable issue ID even across a repository move. A replaced attachment is refused in peer reads and cannot complete a checkpointed handoff. Further quiet periods, acknowledgement retry, error replies, incoming duplicate handoffs and receipt replay remain open; scoped forwarding depends on 2.3. The unsigned legacy primary-label branch of case 18 requires an uncheckpointed setup, a state Rust refuses before opening. Neither supported tagged release (0.7.2 or beta.1) has `run/work-queue` or `fromPrimary`; their tagged golden homes do not need label conversion. The current-beta golden preserves `labels.json` but does not seed `fromPrimary`, `primary` or `primaryAt`. |
| | 18 | Covered for accepted saves. The main signs a rename/recolor and removal while a member is offline. On reconnection the old ID disappears from the signed library and both live and deleted saved rows; another label remains, restoration does not resurrect the removed label, and recreating its name receives a new ID. Legacy unsigned rules remain in TS (see switch requirement below). |
| | 11–12, 14, 19, 27–29 | Covered. Hand-off expiry and cancellation preserve the original, and restoring a canceled copy fails if the recipient has already accepted it. Notes and prior history reach the recipient with the retried item. A numberless handoff receives its number from the new keeper. Repeated identical GitHub readings do not retell an acknowledged snapshot; a changed reading does. A fresh peer GitHub reading avoids a second query, future times are clamped, a stale reading can be refreshed locally, and the newer reading wins. Recent GitHub closures and gone issues stay in the latest hundred. A gone issue is queried by stable ID before its old repository and its new address is used. |
| | 2, 5–8, 15, 17, 21–22, 32 | Open, 5.2 and 2.4. Further projections, forwarding, retell cadence, numbering on receipt and mention bindings. Retain retired ranked numbering checks in TS. |
| [work-queue-two-boxes](../../box/bridge-tests/work-queue-two-boxes.test.js) | 1 | Partial, 5.2. A member spends an ordinary block from the main after it goes away and the main's combined view shows the member's rows. A handoff moves the item and its attachment to a conversation on the other computer. Bot and retry variants remain open. |
| | 2 | Partial, 5.2. Both computers see deletion and restoration. Either computer can initiate the changes, and the keeper preserves their history. The other computer can read the full item and its file and change its priority. Partition and mixed-peer cases remain open. |
| [work-queue-statuses](../../box/bridge-tests/work-queue-statuses.test.js) | 1, 3, 9–10, 15–16 | Partial, 5.2. Local migration, retry, creation defaults and validation, priority, assignee and due date edits, overdue projection, closing/reopening and ordering; invalid edits are atomic. Labels are renamed and removed from live and deleted work without reusing old IDs. Existing agent scenarios verify status changes and label commands; some exact agent wording, migration and view variants still need scenarios. |
| | 2 | Covered. An unavailable engine leaves the migration marker intact. Its first complete answer moves only work that is still active and retires the marker. |
| | 4–7, 11–14 | Covered. Reopening a migrated queue, even when its model marker is lost, retains both edits and removed labels without modifying the original copy. Assigning personal work to a bot moves it into the bot's own queue, while work in a conversation stays there; revision-aware edits do the same and replay their decision. Search matches title, description, note, number and label, including deleted rows, and counts use the combined filters. A busy migration remains readable and refuses writes with the correct message, then recovers and copies unreadable owner bytes without overwriting them. Capability words for ordinary, deleted, GitHub and in-flight items agree with the permitted mutations. |
| | 8 | Open, 5.2. First queue without migration copy. |
| [work-queue-github](../../box/bridge-tests/work-queue-github.test.js) | 1–3, 5–8 | Partial, 5.2. The stand-in verifies `gh` arguments, adding an issue without a number, replay, remote projection and a full remote item. Fresh reads preserve item bytes and a closed issue's stored status. A current lock holds off reads, abandoned locks are reclaimed, and a signed-out host backs off. Tool reads fence GitHub text. Converting a numbered item preserves its reference and earlier description. A one-shot SSH read returns cached state while a held GitHub refresh completes and is saved in the resident. The TypeScript standalone process path and Rust resident routing are intentionally different; the answer is independent of the held refresh. Remaining conversion cases, missing-command and detailed peer freshness variants are open. |
| | 4, 9–15 | Covered. Draft pull requests project as open work, merged or closed as Done, and reopen without changing stored work. A glance uses cached issue state and makes no `gh` call. Null or timed-out answers and inaccessible issues do not become gone; an issue moved to another repository is found by its stable id in the same read. A failed node lookup holds off further requests without marking the issue gone; a definitive missing answer marks it gone and a later read reopens it. A forbidden agent add reports the provider's words, inaccessible issues are retried only when due, and a live item wins a shared reference over a deleted item, which cannot be restored beside it. |
| [work-queue-github-envelope](../../box/bridge-tests/work-queue-github-envelope.test.js) | 1 | Partial, 5.2. The resident returns the located reply with the code and original attempt. It rejects reuse from another client and supports status and cancellation. The bridge-to-link path needs its own scenario. |
| [work-queue-migrate](../../box/test/work-queue-migrate.test.ts) | 1–5 | Partial, 5.2. The original bytes and unknown fields survive, an interrupted migration resumes and a future owner file stops startup. A signed label write preserves order and leaves legacy ownership metadata intact; the pre-shared local label write remains to be proven. Busy migration preserves reads and refuses changes; a simulated disk-copy failure, owner-file refusal during an edit and complete snapshots need scenarios. |
| | 6 | Covered. A future label file refuses an edit and retains its bytes, and a future status model refuses reads without being overwritten. |
| | 7 | Open, 5.2. Peer summaries. |
| [work-queue-numbering](../../box/test/work-queue-numbering.test.ts) | 1–6 | Covered. A restarted member receives the same persisted grant after losing a reply despite changed request fields; another member cannot spend that range. The main honors its accepted inherited counter and relay high mark, refuses a new grant without its current lease, and resumes granting when it returns. The last granted number is usable offline, and a pending item remains editable until a new block arrives. Markdown imports claim numbers and replay without duplicates. |
| [work-queue-relations](../../box/test/work-queue-relations.test.ts) | 1, 4–5 | Covered. The preview route uses the shared exclusions and UTF-16 offsets for non-BMP text, fences, code spans, links, and bidirectional text. An agent writes a typed note with stable mention and UTF-16 offsets, and its target has the corresponding backlink. |
| | 2–3, 6–10 | Partial, 5.2. Saved note mentions preserve their ranges and backlinks. Local relation edits preserve mention bindings and show the inverse. Downloads refuse symlink replacements. Client staging keeps distinct names, bytes and anchors when both the metadata and disk have collisions. Inline marker removal preserves current text and attachment bytes. Missing indexes are reconstructed on reads. A write purges an expired row and its stored edges. More staging cases and durable recovery still need coverage. |
| | 11 | Covered. The live writer keeps its lock and an unrelated route responds during the wait. The queue and ledger both recover from a dead writer. |
| [queue-live](../../box/bridge-tests/queue-live.test.js), [queue-stop](../../box/test/queue-stop.test.ts), [queue-items](../../box/test/queue-items.test.ts) | 1, 1–6, 1–25 respectively | Conversation message queues. See the case-by-case [engine map](rust-engine-coverage.md) for status and rows 3.2, 4.1 and 5.1. |

Of the 182 directly named cases, 57 are covered, 67 are partial, 26 are open in
Work Queue and 32 are tracked in the engine map. The Work Queue conformance area also
checks the initial migration and future-format refusal through a real resident.

| Cross-area cases | Work Queue status and owner |
|---|---|
| [setup-shared](../../box/test/setup-shared.test.ts) 3 | Partial, 5.2 and 2.5. The holder command previews without saving, keeps existing definitions, requires force for a retired ID, and restores on the main with both residents seeing the accepted result. A Rust member can use a TypeScript main. A TypeScript member requires the broader `data.ownership` feature on a Rust main. The provisional preview, further recovery and protected backup cases need scenarios. |
| Switch from the TypeScript beta (9.1) | Before switching each computer, confirm its setup is checkpointed (accepted saves). No uncheckpointed primary-label state (`fromPrimary`, `primary`, `primaryAt`) may reach a Rust resident, which refuses unconverted setups. |
| [setup-merge](../../box/test/setup-merge.test.ts) 2–6 | The setup-merge number reservation and source-bound approval are a dependency on 2.3: reserve on the current relay lease and persist the exact grant before replying, including replay. The import and Markdown rewrite remain in 5.2 after that seam is plugged in. Partial in case 6 for local alias lookup. |
| [offline-merge](../../box/test/offline-merge.test.ts) 2, 7 | Partial, 5.2. A member saves a draft against an owner revision and the two residents merge independent passages when the keeper returns. Local conformance covers the deletion fence before and after restoration and recovery of refused text. Competing passages, retired revisions and legacy receipts still need scenarios. |
| [offline-merge](../../box/test/offline-merge.test.ts) 3–6, 8–10 as applied to queue records | Partial for compact tombstones. Expired deletions retain their fence and a draft receives a refresh on attempted creation. Large and legacy passage layouts, transferred choices, former-owner replay and attachment records remain open in 5.2, 2.4 and 5.5. |
| [setup-two-boxes](../../box/bridge-tests/setup-two-boxes.test.js) joined roster, cards and numbering | Open for 5.2 numbering and card input. Existing roster and card coverage belongs to 2.3. |
| [documents](../../box/bridge-tests/documents.test.js) numbering precedent and independent edits in both orders | Open, 5.2. Existing preference merge belongs to 2.3. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) queue slices for creation, claim, revision edits, scopes, handoff and receipts | Partial, 5.2 and 2.4. Ordinary revision-aware creation and PATCH replay the same decision through the shared edit outbox. During the backup-review proof the TypeScript area once returned a pending (202) answer instead of the immediate deleted (409) receipt in this existing fixture; the single targeted rerun passed, and no TypeScript or offline edit code changed. Claim failure, handoff and the authenticated forwarding boundaries still need coverage. |
| [setup-handover](../../box/bridge-tests/setup-handover.test.js) 1–5 | Open for 5.2 queue handoff, owner reads and independent queue work while the main computer is offline. Move evidence and transfers belong to 2.4. |
| [setup-lists](../../box/test/setup-lists.test.ts) 16, 20 and 28 | Open, 5.2 and 2.4. Move placement, away deletions and the first copy in the combined view. |
| [setup-handover-shared](../../box/bridge-tests/setup-handover-shared.test.js) 2–4, 6–7, 9–10 | Partial for member preview and restore. Large label restores, old and unreachable main, number blocks across takeover, paired grants, offline edits and deletion recoveries remain open in 5.2. |
| [setup-handover-lease](../../box/bridge-tests/setup-handover-lease.test.js) 4; 3 | Both numbering portions covered: after unlinking its last member, the sole remaining main allocates above its last number; after takeover, the new holder allocates strictly above the old holder's relay high mark. |
| [setup-handover-access](../../box/bridge-tests/setup-handover-access.test.js) forwarded receipts across changes and revocation | Blocked on 2.3's shared `data.scoped-forward` and `data.ownership` grants, admission and durable decision seam. Before 2.3 closes, wire the queue scope into that seam: bind origin operation to submitter, setup, owner and intent; authenticate paired/addressed SSH writers; determine current owner and transfer fences; preserve terminal decisions after revocation and across moves. Do not advertise either broad feature for queue-only support. Legacy authenticated computer forwards remain available in 5.2. |
| [setup-handover-wipe](../../box/bridge-tests/setup-handover-wipe.test.js) queue slices of revised plan, retained payloads, revised export, generation changes, attachment revisions and completion | Open, 5.2 with 2.3, 2.4 and 2.5. The backup map records existing snapshot checks. |
| [installer-enrollment](../../box/bridge-tests/installer-enrollment.test.js) reserve merge numbers | Dependency on 2.3 before it closes: reserve merge numbers on the current relay lease and persist the source-bound grant before replying. |
| [setup-mac](../../box/bridge-tests/setup-mac.test.js) legacy discovery and setup numbering | Open for 5.2. Keep the number floor when setup authority changes. |
| [format-versions](../../box/bridge-tests/format-versions.test.js) future ledger | Partial, 5.2. Startup refuses the ledger and preserves its bytes. The version gate during allocation and the link-state portion still require the referenced scenarios. |
| [resident-delivery](../../box/test/resident-delivery.test.ts) case 2 | Open, 5.2. Keep attachment disk work off the dispatch thread. |
| [accounts-check](../../box/test/agents/accounts-check.test.ts) case 18 | Partial, 5.2. A stand-in `gh` verifies that the located device-code attempt stays pending for its original client and permits cancellation. Browser completion and account catalog refresh need scenarios. |
| [message-words](../../box/test/message-words.test.ts) cases 3 and 7 | The published tool titles and the queue-add history words are covered by the Work Queue tools case and [engine conformance](../../protocol/conformance/engine-sessions.test.mjs). The remaining queue tool history variants stay with 5.2 and 3.2. |
| [forward-evidence](../../box/test/forward-evidence.test.ts) cases 2 and 3 | Partial in 5.2 and 2.4. The handoff and full conversation move carry exactly the scoped decisions for their item and carried deletion fence, leaving unrelated evidence behind. Replaying at the new keeper and retaining future or ambiguous records still need scenarios. |
| [moves-safety](../../box/test/moves-safety.test.ts) queue handoff lock, attachment and index sync, and missing indexed payload | Partial, 5.2 and 2.4. A changed source attachment prevents switch and rolls back the destination's prepared item, bytes and deletion fence. A retry must leave a live owner's lock intact; remaining interrupted placement cases need scenarios. |
| [Backup coverage](rust-backups-coverage.md) held queue records, replay fences and capture during handoff | Partial, 5.2 and 2.5. The local-app review route lists held items using the live queue owner and deletion/departure fences, keeps or discards with a durable review receipt, and replays a lost receipt from the saved row marker. Missing attachment bytes stay absent, conflicting live file names refuse the keep, and a deletion cannot be revived. Further cross-computer owner claims and handoff serialization still need scenarios. Backup bytes and capture are tracked in 2.5. |
| [Move coverage](rust-moves-coverage.md) queue placement, handoff and attachment revisions | Partial, 5.2 with 2.4. Carry an original item's number, file proof, deletion fence and scoped decisions with a conversation, also across mixed versions. Refuse switch and clear the destination's staging if a source attachment changed. |

Open work in 5.2 includes the remaining peer-view and owner-read cases,
forwarded writes and receipts, offline merge edge cases and handoff, import collision and
recovery cases and merge reservations, agent and forwarded attachments, mention binding after number grants and owner
changes, bot reassignment and remaining status and order cases, GitHub read and
sign-in variants, and the remaining agents' tools.
Label restore recovery and setup seeding remain open. Signed ordinary label changes use the Rust
setup authority request and reply. A Rust member can also save an ordinary label through the
TypeScript main's authenticated ownership route and both receive the signed library. The reverse
direction needs the complete Rust ownership capability; it is not advertised just for labels.
