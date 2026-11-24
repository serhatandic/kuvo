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
| [work-queue](../../box/bridge-tests/work-queue.test.js) | 1–5, 7–8, 10–11, 13, 16–17, 23, 25, 28, 31–32, 48, 57 | Partial, 5.2. Creation retry, summary counts and order, local delete and restore, label writes, saved attachment download and a view change written by another process. A verified agent adds, lists and reads numbered work, including progress, history and attachments. Agent updates, deletion, undo, starts and stops record the agent's identity. A quick restart folds an intervening stop. A local handoff stays visible in both queues' views with one saved item. Changed invocation arguments fail admission. Expired attachments are retired. Both client Markdown and agent structured imports preserve status and number, and a member receives its claimed numbers from the main. Subagent, one-shot, working links across computers, project folder, mixed import and failure variants need scenarios. |
| | 6 | Covered. The fourth link replaces the oldest one and records an attributed stop without changing the earlier item's status. |
| | 9, 12, 14–15, 18–22, 24, 26–27, 29–30, 33–47, 49–56, 58–60 | Open, 5.2. Agent operations, live session variations, edits, remaining import cases, staging, recovery and handoff. |
| [work-queue-across](../../box/bridge-tests/work-queue-across.test.js) | 1, 4, 9–10, 13, 16, 18, 20, 24 | Partial, 5.2. Authenticated snapshots and acknowledgements carry remote rows and chips through the relay. The shared memory transport reads the keeper's full item and file. Client and attributed agent edits, attachments, deletions and restores reach the keeper from another computer. Shared label changes reach both residents. A two-resident handoff preserves the number and file, and leaves a single live item. The ordinary handoff also passes with either resident running TypeScript. Blocked and retried handoffs, partition and receipt replay, scoped forwarding and retained local label cases still need scenarios. |
| | 2–3, 5–8, 11–12, 14–15, 17, 19, 21–23, 25–32 | Open, 5.2 and 2.4. Numbering and leases, further projections, forwarding, handoff and deduplication. Retain retired ranked numbering checks in TS. |
| [work-queue-two-boxes](../../box/bridge-tests/work-queue-two-boxes.test.js) | 1 | Partial, 5.2. A member spends an ordinary block from the main after it goes away and the main's combined view shows the member's rows. A handoff moves the item and its attachment to a conversation on the other computer. Bot and retry variants remain open. |
| | 2 | Partial, 5.2. Both computers see deletion and restoration. Either computer can initiate the changes, and the keeper preserves their history. The other computer can read the full item and its file and change its priority. Partition and mixed-peer cases remain open. |
| [work-queue-statuses](../../box/bridge-tests/work-queue-statuses.test.js) | 1, 3, 9–10, 15–16 | Partial, 5.2. Local migration, retry, creation defaults and validation, priority, assignee and due date edits, overdue projection, closing/reopening and ordering; invalid edits are atomic. Labels are renamed and removed from live and deleted work without reusing old IDs. Existing agent scenarios verify status changes and label commands; some exact agent wording, migration and view variants still need scenarios. |
| | 2 | Covered. An unavailable engine leaves the migration marker intact. Its first complete answer moves only work that is still active and retires the marker. |
| | 11, 13–14 | Covered. Assigning personal work to a bot moves it into the bot's own queue, while work in a conversation stays there; revision-aware edits do the same and replay their decision. Search matches title, description, note, number and label, including deleted rows, and counts use the combined filters. |
| | 4–8, 12 | Open, 5.2. Migration writer contention and recovery, action limits. |
| [work-queue-github](../../box/bridge-tests/work-queue-github.test.js) | 1–3, 5–7 | Partial, 5.2. The stand-in verifies `gh` arguments, adding an issue without a number, replay, remote projection and a full remote item. Fresh reads preserve item bytes and a closed issue's stored status. A current lock holds off reads, abandoned locks are reclaimed, and a signed-out host backs off. Tool reads fence GitHub text. Converting a numbered item preserves its reference and earlier description. Reopening, remaining conversion cases, missing-command and detailed peer freshness variants are open. |
| | 9 | Covered. A glance uses cached issue state and makes no `gh` call. |
| | 4, 8, 10–15 | Open, 5.2. Pull request cases, one-shot reads, held and moved records, and competing live and deleted references. |
| [work-queue-github-envelope](../../box/bridge-tests/work-queue-github-envelope.test.js) | 1 | Partial, 5.2. The resident returns the located reply with the code and original attempt. It rejects reuse from another client and supports status and cancellation. The bridge-to-link path needs its own scenario. |
| [work-queue-migrate](../../box/test/work-queue-migrate.test.ts) | 1–2, 4 | Partial, 5.2. The original bytes and unknown fields survive, an interrupted migration resumes and a future owner file stops startup. Future-file refusal during an edit and the complete snapshots need scenarios. |
| | 3, 5–7 | Open, 5.2. Read when copying fails, legacy label writes, future model and labels during edits, and peer summaries. |
| [work-queue-numbering](../../box/test/work-queue-numbering.test.ts) | 1–6 | Covered. A restarted member receives the same persisted grant after losing a reply despite changed request fields; another member cannot spend that range. The main honors its accepted inherited counter and relay high mark, refuses a new grant without its current lease, and resumes granting when it returns. The last granted number is usable offline, and a pending item remains editable until a new block arrives. Markdown imports claim numbers and replay without duplicates. |
| [work-queue-relations](../../box/test/work-queue-relations.test.ts) | 1, 4 | Covered. The preview route uses the shared exclusions and UTF-16 offsets for non-BMP text, fences, code spans, links, and bidirectional text. |
| | 2–3, 5–10 | Partial, 5.2. Saved note mentions preserve their ranges and backlinks. Local relation edits preserve mention bindings and show the inverse. Downloads refuse symlink replacements. Client staging keeps distinct names, bytes and anchors when both the metadata and disk have collisions. Inline marker removal preserves current text and attachment bytes. Missing indexes are reconstructed on reads. A write purges an expired row and its stored edges. More staging cases, durable recovery and typed note writes still need coverage. |
| | 11 | Covered. The live writer keeps its lock and an unrelated route responds during the wait. The queue and ledger both recover from a dead writer. |
| [queue-live](../../box/bridge-tests/queue-live.test.js), [queue-stop](../../box/test/queue-stop.test.ts), [queue-items](../../box/test/queue-items.test.ts) | 1, 1–6, 1–25 respectively | Conversation message queues. See the case-by-case [engine map](rust-engine-coverage.md) for status and rows 3.2, 4.1 and 5.1. |

Of the 182 directly named cases, 15 are covered, 54 are partial, 81 are open in
Work Queue and 32 are tracked in the engine map. The Work Queue conformance area also
checks the initial migration and future-format refusal through a real resident.

| Cross-area cases | Work Queue status and owner |
|---|---|
| [setup-shared](../../box/test/setup-shared.test.ts) 3 | Partial, 5.2 and 2.5. The holder command previews without saving, keeps existing definitions, requires force for a retired ID, and restores on the main with both residents seeing the accepted result. A Rust member can use a TypeScript main. A TypeScript member requires the broader `data.ownership` feature on a Rust main. The provisional preview, further recovery and protected backup cases need scenarios. |
| [setup-merge](../../box/test/setup-merge.test.ts) 2–6 | The setup-merge number reservation and source-bound approval are a dependency on 2.3: reserve on the current relay lease and persist the exact grant before replying, including replay. The import and Markdown rewrite remain in 5.2 after that seam is plugged in. Partial in case 6 for local alias lookup. |
| [offline-merge](../../box/test/offline-merge.test.ts) 2, 7 | Partial, 5.2. A member saves a draft against an owner revision and the two residents merge independent passages when the keeper returns. Local conformance covers the deletion fence before and after restoration and recovery of refused text. Competing passages, retired revisions and legacy receipts still need scenarios. |
| [offline-merge](../../box/test/offline-merge.test.ts) 3–6, 8–10 as applied to queue records | Partial for compact tombstones. Expired deletions retain their fence and a draft receives a refresh on attempted creation. Large and legacy passage layouts, transferred choices, former-owner replay and attachment records remain open in 5.2, 2.4 and 5.5. |
| [setup-two-boxes](../../box/bridge-tests/setup-two-boxes.test.js) joined roster, cards and numbering | Open for 5.2 numbering and card input. Existing roster and card coverage belongs to 2.3. |
| [documents](../../box/bridge-tests/documents.test.js) numbering precedent and independent edits in both orders | Open, 5.2. Existing preference merge belongs to 2.3. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) queue slices for creation, claim, revision edits, scopes, handoff and receipts | Partial, 5.2 and 2.4. Ordinary revision-aware creation and PATCH replay the same decision through the shared edit outbox. Claim failure, handoff and the authenticated forwarding boundaries still need coverage. |
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
| [Backup coverage](rust-backups-coverage.md) held queue records, replay fences and capture during handoff | Open for 5.2 live item claims, review writes, review receipts and handoff serialization. Backup bytes and capture are tracked in 2.5. |
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
