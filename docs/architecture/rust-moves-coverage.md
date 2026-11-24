# Rust moves and ownership coverage

Phase 2.4 is done. The Rust resident moves ordinary OpenCode chats and their
descendants, wake-up folders, managed attachments, held prompt queues, ordinary note histories
and session-scoped forwarded decisions. A destination reply proves the generation and hash it
committed. The source keeps the retired original until it saves a backup. The peer
resident routes ordinary letters and notes through the same owner lookup used
for addressed work and receipt lookup.

## Proof

| Scenario | What it exercises |
|---|---|
| Stored outcomes | A resident reads a held incoming copy, v0 journal and departure records, a successful v1 outcome, a legacy refusal, and a missing result through the move routes. Both residents run the same case. |
| Old departure | The location route still leads to the owner after the v0 departure stops being advertised. Both residents run the same case. |
| Other computers | A third computer distinguishes a connected computer whose first move snapshot is pending from a known away computer. It sees both parties' authenticated projections and receives their successive removals on the move event stream. Both residents run the same case. |
| Newer departure | The location route and a letter addressed to the previous computer follow a newer peer announcement after a move. The location falls back to the durable departure after expiry and keeps a local conversation at home. |
| Live transfer | Two Rust residents move a chat, its child, a paused queue, an ordinary note and a replay fence in each direction between the main computer and a member. The former computer's outbox still names its old owner. The letter reaches the new owner once. The TypeScript reference runs the same transfer and checks the records it leaves. |
| Pinned OpenCode | A pair of Rust residents and a separate pair of TypeScript residents each run one OpenCode Go DeepSeek v4.1 Flash prompt with OpenCode 2.2.6. Each move retires the original and the destination opens the transcript with the model's reply. |
| Lost part, arrival and switch replies | The relay holds the live Rust transfer's first packets for `check`, `checked`, `offer`, `ready`, `part`, `got`, `working`, `arrive`, `arrived`, `switch` and `switched`. The source resends the part whose acknowledgement was lost, and retries arrival after a minute. The destination restarts after committing the switch. The source reconnects and recovers the acknowledgement through its next scheduled switch retry. |
| Lost part window | Both residents lose the first 16 parts, then retry the first eight and complete the transfer with the whole transcript. |
| Retry clocks | The Rust resident resumes a switch whose saved timestamp is ahead of its clock. A steady wall clock still allows the next minute's retry, and a backward jump prompts another retry of the same authenticated switch. The part window uses monotonic time separately. A brief relay drop during a move neither marks the peer away nor sends a duplicate offer, and a step whose send fails while offline stays unsent until reconnect in both residents. Both residents run the brief-drop conformance case. |
| Quiet destination | After the first part goes unanswered, the source shows that it is waiting. Authenticated progress clears the notice. Ten more minutes without progress release the original and save a failure receipt. |
| Interrupted import | The engine accepts the import but its reply is cut short by a destination restart. The destination deletes just that copy using the import receipt. The source keeps its original and reports failure. |
| Drop during import | Both residents hold a copy while setup is in flight. They submit the drop decision after the engine replies, remove the copy and replay that decision to a later switch. |
| Partial copy | A restarted destination finishes the live transfer using the part it saved earlier. After a week an incomplete copy can be removed with a durable drop decision. A complete two-computer copy still waits for its switch decision. |
| Replacement offer | Both residents let the same authenticated sender replace its incomplete copy under a new move ID, retain a durable drop decision for the old ID, and require an explicit drop of a complete copy before accepting its replacement. |
| Restart after a failed offer | Both residents resume requests to drop a failed older offer without sending it again. A journal that requires acknowledgement continues to ask past the normal expiration date, until the destination sends its authenticated receipt. |
| Import contention | Both residents preserve an unrelated local child after a failed lookup, a second conversation claiming the child's ID after preflight, or a lost import reply. They roll back their own staged imports and the wake-up folder already installed by that move. |
| Cleanup | A switched source that cannot export its backup retains the original while its receipt reports cleanup pending and location points at the destination. The next round respects the retry interval and the same resident cleans up once the interval elapses. Restart recovery was also exercised. A destination that cannot delete a dropped copy keeps it held, replays the same drop proof on a late switch, and cleans up once its engine accepts deletion. |
| Cleanup after partial deletion | Both residents keep a complete backup when the engine removes the child but refuses to remove the original. After restart they finish cleaning up using that backup, while the engine refuses a fresh export. |
| Dropped-copy retry | Failed deletion keeps each copy held. The Rust resident tries again each minute for a recent drop and each hour after a week. Both copies are released when deletion succeeds. The TypeScript reference covers the failed deletion and the late warning. |
| Waiting and recent requests | On the same live switch the source shows it is waiting for the destination after its reply is lost. A resident also drives a recent request recorded before its first check when it starts, while a second simultaneous request is refused. The live transfer imports children from both engine pages. |
| Changing conversations | A live move rebuilds a bundle if a replay fence is saved after its first offer. Another move sees a new message on each offer, gives up after three offers, and leaves all four messages on the source. |
| Receipt indexing | A Rust move with unrelated receipt history yields after one bounded scan. The source keeps the original and resumes the scan after restarting. The retry carries the replay fence. TypeScript's indexed preparation is covered by its owner-mutation scenario. |
| Receipt issuer | Both residents submit a real draft before moving it. The original computer can still read its committed effect with the new owner shown. The destination cannot look up someone else’s computer receipt or replay the earlier draft. |
| Receipt pages | An accepted replacement changes while a page is in flight. The caller gets an incomplete page without a next cursor. Retrying the original cursor returns every receipt once. Rust selects up to 41 candidates per indexed receipt family with issuer and author filters before reading the records. |
| Owner observations | An authenticated absence invalidates older pages and survives a restart. An older answer cannot replace a newer observation. A conversation, its file inventory and its paused queue answer from the destination after a live move, including with native Windows attachment paths. |
| Session actions and live asks | Moving conversations report `held` on their actions menu and refuse pin, archive, rename and delete. Moved conversations redirect those actions with the new computer's key even when a stale local copy remains, and reject late permission replies. Move attachment protection covers active moves, recent departures and recent drops. Both residents run the same conformance case. |
| Removal fence | Unlinking an activated destination whose acknowledgement was lost leaves the source held across a restart. It reports a pending switch and refuses to start a second copy. |
| Replacement restore | The first issuer commits a draft through another computer, then backs it up before moving the conversation. After disk loss and unlink, the replacement restores the issuer’s receipt. The live owner keeps the replay fence, transcript and verified attachment and refuses to run the draft again. |
| Retired backup | Only the destination starts the retention clock. The backup stays while a move back is in progress. Once that move is dropped, an expired backup stays recorded through a failed deletion and the next round retries it. A destination distinguishes the first local reply from a carried reply with a future timestamp. |
| Unproven import | An authenticated drop sends proof that the source can stay active. Both residents keep an older incoming journal held when it has no evidence that its local session came from the move. |
| Arrival query and size limit | Both residents answer an authenticated repeat with the held copy's generation and hash, report failure for an unknown copy and replay an eight-day-old switch receipt. They send no ownership proof for an unknown switch, drop or decision request. An offer one byte over 1 GB receives the size-limit refusal without opening a copy. |
| Archive links | Both residents refuse a linked manifest, a linked wake-up directory, a link outside that directory and a chain of links that would escape it. The destination keeps its own file private and unchanged and creates no conversation or wake-up directory. |
| Destination checks | Both residents show the destination by name and mention a running turn in the move confirmation. They leave the original active after an unanswered check, an unavailable engine or a missing sign-in. A retry after sign-in moves the original and preserves the destination's account note in the receipt. |
| Background work | Both residents refuse a move while the source has a running background command. The source also checks children as it discovers them, and ignores background work past the six-hour reporting limit. |
| Wake-up folder | Both residents hold a wake-up folder on each computer until the switch. The source uses configured roots outside its home. The destination rebases the directory and restores the saved pause. The source retains its backup until the returning copy activates. Both move the unpaused folder back and read the receipt after the journal retires. Letters and notes reach the returned conversation while the full engine list is unavailable. A source wake-up folder over 1 GB is refused and its hold is released. |
| Legacy wake-up hold | Both residents restore the saved pause in a failed v0 journal, remove an orphaned bare `moving` marker and retain a pause that names another move. |
| Waited letter | Both residents identify the sender by its own announcement. The original tool hears the authenticated answer after restarting with its awaited record. A user-stopped recipient saves the letter, tells the sender why it is waiting and uses the separate reply instructions after Resume. |
| Sender introduction | Both residents hold the engine's refreshed list before releasing a new sender's first letter. The recipient sees the sender's name and address on cold start and after a restart with an older list and a temporarily unavailable sender lookup. |
| Letter retry | The Rust relay scenario observes the same bounded letter bytes after each minute, ignores a receipt from another computer, and accepts the reply from the intended computer both before and after its acknowledgement. Shared scenarios check expiry, order and late replies. |
| Letter ordering and delivery recovery | Both residents send pending letters oldest first and serialize concurrent incoming deliveries per recipient. A temporary lookup failure leaves the letter pending. Replaying the same letter after the destination restarts without receiving its engine's acceptance returns the receipt with one prompt ID and one saved message. |
| Letter allowance | Both residents retry letters to a held conversation and an unavailable engine without charging the peer. Its next twenty messages can be delivered. The following message waits for the rate limit. |
| Note addresses | Both residents learn session addresses from accepted notes and move announcements, keep the newer address across a restart, and send a pending note there when the original announcement expires. A redirect from the authenticated recipient is durable. A different computer's redirect is ignored. |
| Older letter destinations | Both residents allow a lead to write to an unlisted conversation it started and refuse another conversation's letter. Listed conversations and recent arrivals still receive letters. |
| Tool that stopped waiting | Both residents deliver an ordinary letter without the wait, cancel an unsent transcript request, clear `.alive` alongside `.gaveup`, and deliver a reply that was left behind to the local conversation. Replaying a delivered letter starts no second turn, and an unlisted sender hears that it cannot receive a reply. On Windows, outbox deletion retries sharing violations, `give_up` defers on an incomplete letter write, and a failed relay send clears the retry cooldown and flushes on reconnect. |
| Transcript read | Both residents retry a temporarily unavailable page, return excerpts to the waiting tool and skip an undone passage. Only the lead can read its archived work. The read submits no prompt. Rust uses the shared 3.2 reader and returns authenticated replies while its agent list is unavailable. |
| Late answer and notes | A saved answer, a local conversation's reply and two queued notes wait while the receiver is held without arming a local retry cooldown. After the switch, the new owner accepts them once with their original attribution. A transcript left by an abandoned read is discarded. A separate transfer accepts a note on the new owner while the original awaits cleanup. |
| Refused letter | Both residents ignore a refusal from another computer. They save the owner’s refusal while the sender is held, then deliver it once as a queued note after a resident restart and release of the hold. The archived recipient is untouched. |
| Managed files | Both residents refuse missing and export-only attachments before switching. The destination keeps the exact bytes, verifies the file receipt, rebases typed root and child references and a held prompt while preserving a quotation, and opens the file after its own restart and loss of the original file. |
| Prompt fence | A journal survives a resident restart and refuses direct prompts, commands, sends and queue resumes. Both residents fence queue removal, undo and resume while sending a live move. After the destination proves it dropped the copy, the source still has the original paused queue in order and allows removal. The Rust composer keeps a saved draft pending during the hold, then delivers it once after release. |
| Queued payload integrity | An absent waiting payload prevents the first offer and gives the same refusal in both residents. A payload corrupted after the offer prevents the switch. The source keeps the paused queue and its entry. |
| Stop during a move | Both residents allow Stop while the source waits for its active turn. Packing waits for the durable queue hold, then the destination opens the paused queue in its original order. |
| Queued work on a child | An unpaused child inbox keeps either source from offering the conversation. The source keeps the root, child and original queued work when the inbox cannot be read. |
| Paused queue after a move | Both residents carry a queued letter followed by a message with a file. The old queue is emptied, the destination shows the original words and file name, and resuming sends only the first message while its turn is active. |
| Owner requests | A relay test binds both directions of the exchange to the accepted setup, peer, request nonce and operation. A new durable journal takes effect without a new list snapshot. A replacement can answer for the original issuer. A restart test checks allowed, revoked and checking device states. |
| Owner request limits | Two Rust residents exchange an addressed 5 MiB draft and a 128 KiB scoped submission, each with a multipart reply. Both reject one byte over the draft limit. The TypeScript Work Queue sender and reassembler match the decrypted part fields in all four exchanges. Accepted membership exposes cached peer features while a computer is offline. |
| Golden receipt | Both residents open a current TypeScript golden home and keep its switch receipt byte for byte. |
| OpenCode export and rollback | `Engine::session_move` holds one engine lease. The scenario reads a paginated child tree, imports with the destination directory and ownership receipt, and recovers an import whose reply was lost after OpenCode accepted it. A receipt for another move leaves the copy intact. |
| Bound decision | A real Rust resident receives authenticated queries for an unknown move, a different generation and a different hash before it answers the valid retry. The reply proves the committed generation and inventory only. |
| Stored formats | The manifest reader accepts unmarked and stamped v0, v1, v2 and v3. Writers use v1 for plain bundles, v2 when they carry session notes, and v3 when they carry forwarding evidence, as the TypeScript packer does. Rust storage scenarios also check immutable receipts, independent hints, and the unmarked legacy decision map after reopening. |
| Differential | Both residents answer the same three listing and outcome requests and preserve the same seeded move files. The move and outbox folder entries and both peer reach files agree. The full inventory still differs in bootstrap, usage, library, engine-list and setup files. |

Run the shared routes against each binary with `KUVO_CONFORMANCE_AREA=moves`.
The Rust relay interruption scenario uses the crypto exported by the built
TypeScript bundle to identify packets in the stand-in relay. Both residents in
that scenario run Rust. `KUVO_CONFORMANCE_MOVE_PEER` selects the other binary
for the peer reach differential. The crate's `move_storage`, `move_transfer`,
`session_move` and focused relay tests cover the storage, transport and engine
boundaries.

Rust keeps the transfer and letter state in resident tasks. The engine move
handle holds an OpenCode lease and reuses its prompt and queue operations. A
prompt write owns a scoped marker until it has committed or returned. The
move reads the durable fence and waits for existing writers before comparing
its final snapshot. `kuvo_store` seals bytes before their part acknowledgement
and publishes activation receipts under a scoped lock.

The TypeScript sender now range-checks authenticated `got` indices before
counting them. The scenario holds the only part, sends an acknowledgement for
index 1, then releases part 0 and proves that the move finishes.

A fresh TypeScript engine creation event clears the deletion left in the
resident list by the outbound move. The reverse wake-up scenario disables
the full engine list so that this event has to restore the returned chat.
It failed on the old bundle and passed after the change. Rust also checks
the live engine for an authenticated local reply's sender if its saved list
still lacks the chat.

The TypeScript outbox also refreshes from the engine before introducing a
sender missing from the cached list. The old process sent the letter ahead of
its announcement. In Rust, the outbox waits for the publication to the right
peer, and the receiving resident applies that announcement before attributing
the letter. The sender's previous computer can identify it from the switch.

The TypeScript router now follows the queue's Stop and Hold rules while a
source waits for an active turn to finish. The old router refused Stop after
the destination accepted the move check. Both residents let Stop settle the
turn and open the queue paused on the destination, in its original order.

The move area passed 29 Rust and 27 applicable TypeScript scenarios on
macOS, including the two-binary reach comparison. The backup retry interval
was checked separately after those runs. Fifteen Rust storage, transfer and
outbox tests also passed on macOS. WSL2 with GNU libc and native Windows each
passed 27 in their full move runs, followed by focused checks of the later
changes. Static Linux musl ran focused scenarios under WSL2. The large
wake-up test uses a sparse file on Unix. The optional two-binary reach
comparison ran on macOS. The tests use disposable residents, a loopback
relay and an OpenCode stand-in.

The next pass ran 37 move scenarios on Rust and 34 applicable TypeScript
scenarios on macOS, including the two-binary reach comparison. Focused checks
also ran on native Windows, WSL2 GNU and static Linux musl. The delivery and
move records matched in the separate side-by-side run. Startup and unrelated
home files still differ.

The later macOS move-area runs passed 58 Rust and 56 applicable TypeScript scenarios,
including the two-binary reach comparison. Focused checks added afterward passed on
both residents. That production build passed five focused checks on native Windows and
six each on WSL2 GNU and static Linux musl. These covered wake-up holds and the return move,
queue admission, letter replay and attachment placement.

After integration with 3.2c, macOS TypeScript passed all 59 applicable move scenarios and
WSL2 GNU passed all 61 applicable Rust scenarios. The TypeScript owner-mutation reference
passed six focused cases, and the Rust relay checked minute-based retries. Native Windows and
static Linux musl each passed the three focused scenarios for receipt indexing after restart,
preserving the issuing computer, and reading a transcript. The pinned OpenCode proof passed
once on each implementation with disposable residents.

The macOS Rust move area passed 61 of 62 before the final transcript fix. Holding the agent
list reproduced the remaining failure. The corrected transcript and sender-order scenarios
passed on macOS and WSL2 GNU. The side-by-side run matched the move requests and the persisted
move, outbox and peer reach files.

For the owner read, receipt page and queue integration, macOS Rust passed all 67 move scenarios,
including the two-binary peer reach comparison. The TypeScript reference passed all 63 applicable
scenarios. WSL2 GNU and static Linux musl each passed all 66 applicable scenarios. The focused
replacement restore passed on TypeScript and on Rust for macOS, WSL2 GNU, static Linux musl and
native Windows. After the native attachment-path fix and merging `main`, native Windows passed
65/65 applicable moves (64/65 on the full run plus an immediate rerun of the one timed-out letter
case) and the focused replacement restore. After adding session-action and live-ask move fences,
move attachment protection (`protects_attachments`), and the mutation-lock responsiveness check,
scoped Clippy (`kuvo_engine`, `kuvo_peers`), `move_storage` (4/4), `resident`, the two-binary peer
reach check, and the 6 focused move scenarios plus focused replacement restore passed on macOS Rust,
macOS TypeScript, WSL2 GNU, static Linux musl and native Windows. A side-by-side run matched all
five move and owner-read responses and the persisted move, outbox and peer reach records. The full
home comparison still reports startup and unrelated file differences.

After aligning the conformance engine stand-in with real OpenCode 2.2.6 asynchronous interrupt
behaviour (`engine-sessions` passed 81/81 on both TypeScript and Rust), adding TypeScript offline
move-step retry parity (`box/src/link/moves/move.ts`, `moves-safety.test.ts` 42/42), serializing
concurrent incoming letter deliveries per recipient (`box/src/link/letters/take.ts`), fixing the
local-only frozen-move outbox skip and `.alive` cleanup on `.gaveup` (`box/src/link/letters/outbox.ts`),
and hardening Rust Windows outbox deletion retries, partial-JSON `give_up` deferral, failed-send
retry clearing, and reconnect flushes (`kuvo_store`, `kuvo_peers`), the full move area and focused
replacement restore passed with zero failures across every platform: macOS Rust 69/69 moves
(including the two-binary peer reach comparison) + 1/1 restore, macOS TypeScript 65/65 applicable
moves + 1/1 restore, WSL2 GNU 68/68 applicable moves + 1/1 restore, static Linux musl 68/68
applicable moves + 1/1 restore, and native Windows 67/67 applicable moves + 1/1 restore.

Live TS↔Rust beta transfer and seeded mixed switch replay are **dropped: TS-beta
interop (user 11-23)**. The released Mac and beta.1 manifest readers remain in
scope. The side-by-side reach and persisted records checks continue to use
both binaries.

## Peer reach files

The differential runner checks both `run/peer-reach.json` and
`run/peer-reach-checked.json`. A fresh home with no observations leaves both files absent.
An unreachable peer creates both records. When that peer answers, both files remain
and its keys are removed. Another peer's saved numeric observations survive both
writes, including fractional timestamps accepted by the TypeScript reader. The
pre-port Rust build dropped those entries.

The side-by-side reach scenario compares those states as well as the final home files.
The runner normalizes numeric observation timestamps under computer keys and still
compares file existence, keys and value types. Stream parts and in-memory queue mail
leave the files alone, as checked over the Rust relay. The `run/agents.json` export
belongs to 4.1.

For a separate differential run, supply the returning computer's offline seed as
the third CLI argument. A request can include
`"peerReach": {"key": "<computer key>", "state": "away"}` followed by a request
with `"state": "answered"`. The runner starts the peer after observing its absence
and compares the two saved maps before dispatching each request.

## TypeScript scenario map

Numbers are one-based, in declaration order in each reference file. The scoped inventory
below counts 212 declarations. One hundred six have full scenario coverage, 13 have partial proof,
and 93 remain open or assigned to their other owners. Mixed-only portions are recorded
separately where they appear.

| Reference | Cases | Status and remaining owner |
|---|---|---|
| [moves](../../box/bridge-tests/moves.test.js) | 3 | Partial, an ordinary letter follows a live move. The calling tool belongs to 4.1. |
| | 1 | Covered by move choices for a running turn, an away destination and a missing chat. |
| | 2 | Partial, both residents move a wake-up folder round trip under the journal's hold with separate chats and wake-up roots. The installed Mac and its Trash are 9.3. |
| | 7–8 | Covered by the live transfer of a tree with more children than one engine page and by two simultaneous requests competing for the same chat. |
| | 11 | Covered by the live move of a paused letter and an attached-file prompt, checking their order, shown words, and first delivery after resume. |
| | 12 | Covered, engine-held steer and queue items move in delivery order ahead of locally queued work. Resume restores each prompt and its bytes. Unreadable queued content gives the same refusal on both residents. |
| | 14 | Covered by two different files with the same basename plus a child file in the Mac attachment root. The destination verifies each receipt, keeps the order and quoted paths, and opens every file after restart and source loss. |
| | 15–16 | Covered by a live source refusing an expired attachment and a file path outside its managed roots. |
| | 4, 9 | Bot moves and wake-up state owned by 5.1. |
| | 6 | Group placement owned by 5.3. |
| | 5 | Covered by a connected destination refusing while its engine is unavailable. The source stays active. |
| | 10 | Autopilot state, its instruction and its decision history transfer belong to 5.3. |
| | 13 | Covered by Stop while the source waits for its active turn and by reading the paused queue on the new owner. |
| [moves-elsewhere](../../box/bridge-tests/moves-elsewhere.test.js) | 1 | Covered by a third computer observing both authenticated projections and their successive clears over the event stream. |
| | 2 | Covered by receiver-clock expiry, sender clock rewind, and filtering malformed, local and unauthenticated notices. |
| | 3 | Covered by a following client that loses a quiet peer’s notice when the receiver clock reaches expiry. |
| | 4 | Covered by the first snapshot, changes of stage, minute refresh with current bytes, and a single clear. |
| | 5 | Covered by retrying an active snapshot and its clear after the relay rejects the sends, and by an authenticated peer refreshing the next snapshot. |
| [moves-lanes](../../box/bridge-tests/moves-lanes.test.js) | 1–9 | Native transfer and attachment carry owned by 4.6, with bot placement in 5.1. |
| [moves-lanes-races](../../box/bridge-tests/moves-lanes-races.test.js) | 1–5 | Native runtime proof belongs to 4.6 and 5.1. The ordinary conversation comparisons and retry behavior are covered under moves-races. |
| | 6–16 | Native carry and retained copies owned by 4.6, with bot placement in 5.1. |
| [moves-races](../../box/bridge-tests/moves-races.test.js) | 24 | Covered by a resident retrying an unanswered switch for eleven minutes without dropping, ignoring mismatched proof and releasing the original only on the bound drop receipt. |
| | 1 | Covered by a setup failure arriving as the source switches, followed by the authenticated drop receipt that releases the original. |
| | 2 | Covered by refusing an unreadable payload before the first offer and keeping the original after corruption prevents switching. |
| | 3 | Covered by delaying an admitted queue write, refusing the next write once the source holds the conversation, and checking the new owner's queue before and after resuming it. |
| | 4 | Covered by an idle child's pending inbox blocking the offer and a failed inbox read preserving the source. |
| | 5 | Covered by the source ignoring a drop for another generation and releasing its original after the destination’s matching drop receipt. |
| | 6 | Covered by a duplicate bundle part before activation and a delayed repeat afterward. There is one import and the same durable switch receipt. |
| | 7 | Covered by the real relay disconnecting before the source can handle an authenticated ready receipt. The rejected part goes on the next round after reconnecting, before the lost-part interval. |
| | 8 | Covered by dropping the entire first window, retrying eight parts, and verifying the completed move contains its original transcript. |
| | 9 | Covered by an unknown switch, drop and decision query yielding no ownership proof. |
| | 10 | Covered by rolling back an earlier child or root import and its installed wake-up folder after another import fails. |
| | 11 | Covered by an unanswered part reaching its deadline and leaving the original with a failed receipt. |
| | 12 | Covered by a live destination refusing an offer over an existing chat and leaving its original unchanged. |
| | 13–14, 28, 33 | Covered by refusing four invalid archive links while preserving the destination's unrelated file and conversations. |
| | 15 | Covered by a second authenticated computer trying the held move’s ID and a different ID for the same conversation. |
| | 16 | Covered by a live restart of the destination after it acknowledges the first part of a two-part bundle. |
| | 17 | Covered by three live offers while the transcript changes on each try. The original stays active with every message. |
| | 18 | Covered by fencing queue removal, undo and resume during transfer and preserving the paused queue after the destination drops its copy. |
| | 19 | Covered by a conversation location and queued letter following a newer move announcement, then a delayed note following its retained address after restart. Expired announcements and a conversation still owned here are checked separately. |
| | 20 | Covered by removing the destination after the switch acknowledgement was lost and retaining the source’s durable hold through a restart. |
| | 21 | Covered by a live drop waiting for an outstanding engine import, then removing it and replaying the durable drop decision. |
| | 22 | Covered by a transient lookup error, a live conflict after preflight, and a lost import reply. Each rolls back only the move's own imports. |
| | 23, 32 | Covered by a live source surviving an engine deletion error after its confirmed switch, completing cleanup, and retaining the full original backup with its child. |
| | 25 | Covered by resuming an old failed offer and a journal that requires acknowledgement. The destination's drop receipt retires each journal. |
| | 26 | Covered by replay of a legacy switch receipt eight days old, after the announcement would have expired. |
| | 27 | Covered by refusing a bundle that includes a local conversation, claims a different root, has an unrelated child, or contains a cycle. |
| | 29 | Covered by refusing an older computer with known capabilities and by trying an unannounced computer on an older setup. |
| | 30 | Covered by replacing an incomplete copy from the same sender, requiring a drop of a complete managed copy, and waiting before replacing a copy still importing on an older setup. |
| | 34 | Covered by a failed export keeping the original retired and reporting cleanup pending, followed by a successful retry on the same resident after its interval. |
| | 31 | Covered by the live lost switch acknowledgement and by a restarted destination replying only with its durable decision. |
| | 35 | Covered by a receiving resident whose cumulative import time reaches thirty minutes. It sends the matching failure and removes all the imports and the installed wake-up folder, including the import that accepted but never replied. |
| | 36 | Covered by the authenticated oversized offer, exact refusal and empty destination. |
| | 37 | Covered by the source and recipient listing the named computers while a two-part transfer is held between acknowledgements. |
| | 38 | Covered by waiting progress, clearing it on authenticated contact and releasing the original when the destination goes quiet again. |
| | 39 | Covered by a live source whose durable switch is unanswered for 90 seconds and whose repeated switch later recovers the destination receipt. |
| | 40 | Covered by authenticated switch retries with a future saved time, a steady wall clock and a backward jump. The part-window test checks monotonic retry deadlines. |
| | 41–42 | Covered by an incomplete copy waiting after 90 seconds and a complete copy waiting after 150 seconds, and by clearing a waiting announcement when the peer answers again. |
| | 45 | Covered by preserving the earlier backup until the returning copy activates, then discarding it while retaining the returned conversation and folder. |
| | 50, 52 | Partial, the receiver refuses an unavailable OpenCode provider before the source holds its work and permits a retry after sign-in. Native coding-agent preflight remains 4.6. |
| | 51 | Covered by retaining the authenticated destination's replacement-account note in the move receipt. Selecting a replacement for a native agent belongs to 4.6. |
| | 43–44, 46–47, 49 | Covered by retained backups and retry after failed deletion, first-run replies tied to the carried transcript, and grace-period cleanup of orphaned move bundles. |
| | 48 | Covered by move choices that count a chat’s delegated work and unanswered questions, including a second level of work. |
| | 53–54 | Native account fallback is owned by 4.6. Bot key handling is owned by 5.1 with native carry in 4.6. |
| | 55 | Covered by refusing a running command before admission and when discovering an active child. |
| [moves-recover](../../box/bridge-tests/moves-recover.test.js) | 5–7, 21 | Covered by the fresh arrival reply after a lost reply, the known and unknown arrival queries, an interrupted engine import and its rollback, and an older held copy without import ownership. |
| | 10 | Covered by driving a request saved before its first check and retiring one with no hold time after thirty minutes waiting for the relay. |
| | 1–2, 4, 12 | Group hold, release and rollback owned by 5.3. |
| | 3 | Covered by a failed deletion keeping the copy held and repeating its proof, followed by minute-based retry and removal once the engine allows it. |
| | 11 | Covered by a sparse source wake-up file over 1 GB. The source releases its hold and keeps the original without offering the copy. |
| | 15 | Covered by dropping an unfinished bundle while the engine cannot answer its readiness check. |
| | 18 | Covered by an abandoned tool reply that stays held until the switch and follows the conversation once. |
| | 16, 22 | Covered by an offer retried while a dropped copy is held and again after cleanup succeeds, and by dating the week-long warning from the drop. |
| | 17 | Covered by the overdue drop warning once and staying held, followed by hourly retry and cleanup. |
| | 19–20 | Covered by a reply from a local conversation following its held caller to the new owner, and by a resident delivering a saved reply to its local conversation. |
| | 23–24 | Covered by queued notices retaining their text, author and presentation on the new computer and after a return home. |
| | 8 | Covered by a changed transcript causing a second offer, retrying that same offer every minute, and releasing the original and its wake-up marker when the destination stays quiet. |
| | 9 | Covered by a resident with successive slow engine exports, releasing the original and its wake-up marker at the thirty-minute deadline before the first offer. |
| | 13–14 | Partial. Session news and a user-answer note wait during the switch and reach the new owner once. Creating the notices from the failed session event or the late ask answer needs 5.1 and 3.2 respectively. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) | 1–3 | Work Queue creation, forwarded claims and edits owned by 5.2. |
| | 15–16 | Partial, the final move snapshot is rebuilt when a replay fence is saved after an offer. The destination preserves a settled decision and rolls back a conflicting handoff. A live submission keeps its committed effect at the issuing computer after a move and can't be replayed on the destination. Restoring an archive separates issuer history from the replay fence after a real move and replacement. A distinct explicit non-admission scenario belongs to the submission adapter in 3.2. |
| | 17 | Covered by bounded move receipt scanning across restart and by indexed receipt pages with a stable cursor. |
| | 9–10, 18 | Covered by owner observations, ordered snapshot updates, and receipt pages held across issuer replacement. |
| | 11 | Setup edit capacity and recovery belong to 2.1. |
| | 12–13 | Attachment-only Work Queue edits and replay of scoped Work Queue mutations belong to 5.2. Pairing verification uses 2.2. |
| | 14 | Interrupted native draft recovery belongs to 3.2 and 4.6. |
| | 4–6 | Setup-owned, 2.1. See [setup coverage](rust-setup-coverage.md). |
| | 7–8 | Covered by 2.2's Updating scenario for an announcement-only owner and a local session also claimed by a peer. |
| [peers](../../box/bridge-tests/peers.test.js) | 34 | Covered by ignoring another computer’s refusal and delivering the authenticated owner’s refusal once after restarting with a pending note. |
| | 7 | Covered by named and unlisted senders, an authenticated awaited reply and replay of the original letter without a duplicate prompt. |
| | 9 | Covered by a failed lookup followed by a restart after engine acceptance, replaying the same prompt ID. |
| | 12 | Covered by retrying delivery to both a held conversation and an unavailable engine, then delivering twenty messages before the peer reaches its rate limit. |
| | 26 | Covered by pending letters arriving oldest first and a letter following a newer move announcement in spite of its older saved address. |
| | 27 | Covered by a note with a stale address reaching the live new owner, with its prompt, display text and attribution shortened at whole characters. |
| | 28 | Covered by a letter with a stale remote address reaching the conversation after it moves home and its awaited reply returning without relay mail. |
| | 29 | Covered by a live move followed by a letter whose engine acceptance went unheard. Replaying it after restart acknowledges the original prompt without a second submission. |
| | 31, 33 | Covered by late replies recovered through the resident and by abandoned transcript replies being discarded during the same hold. |
| | 13 | Covered by a live outbox retrying at one minute, authenticated acknowledgement, Unicode-safe wire limits, answers before or after acknowledgement, late reply recovery, expiry while disconnected and the longer lifetime of notes. |
| | 14 | Covered on cold start and with an older saved list. The peer sees the fresh sender's authenticated list before the letter and receives its verified name and reply address. |
| | 6, 10–11, 32 | Open. 4.6 owns the disappearing or restarting coding-agent bridge and the resulting letters and abandoned answers. Ordinary OpenCode restarts and late answers are covered. |
| | 8 | Covered by a waited letter to a user-stopped conversation and the saved reply instructions after Resume. |
| [mac-peers](../../box/bridge-tests/mac-peers.test.js) | 11 | Partial, transcript mail has a live two-computer scenario. The calling tool’s route and name lookup still need 3.2 and 4.1. |
| | 6 | Covered by retryable mail refused during a move, the held recipient seeing no prompt, and saved outbox mail following the acknowledged owner. |
| | 8 | Partial, disconnect and reconnect cover durable delivery. The offline answer from the calling tool still needs 3.2 and 4.1. |
| | 2–3, 10, 12 | Open. 3.2 and 4.1 supply the calling tools, name lookup and capability refusal against an older peer. The letter receiver and reply transport are exercised separately. |
| [setup-lists](../../box/test/setup-lists.test.ts) | 16 | Typed drafts, remote queue ordering and lost-effect receipts belong to 3.2, with authenticated transport from 2.2. |
| | 28 | The 2.4 move projection is covered by the first snapshot pending and away states on the third computer. The other merged views belong to 2.2, 5.1 and 5.2. |
| [reach-elsewhere](../../box/bridge-tests/reach-elsewhere.test.js) | 1, 3 | The live cases move a bot. 5.1 owns its runtime, 4.1 owns the caller, and 3.2 owns the remote engine effects. [2.2](rust-peers-coverage.md) proves the authenticated call transport. |
| [reach-elsewhere-rules](../../box/bridge-tests/reach-elsewhere-rules.test.js) | 11 | Covered by lead-only letters to an unlisted older conversation and the exception for a conversation that arrived recently. |
| | 4 | Covered for ordinary chats while the source is switching and retiring. Only the authenticated new owner speaks for the chat before its announcement. The reply reaches that owner. Bot moves remain 5.1. |
| | 3, 14 | Open. The 3.2 remote work actions and 5.1 bot lead runtime must run against the new owner for case 3. In case 14, the 3.2 sender must stop a pending call whose lead moved, and the 5.1 notice follows the new owner. |
| [offline-merge](../../box/test/offline-merge.test.ts) | 4 | Partial. The new owner keeps the original revision and its older coarse passage. The editor’s refinement and merge belong to 3.2. |
| | 8–9 | Work Queue deletion and former-owner replay owned by 5.2. |
| [setup-handover](../../box/bridge-tests/setup-handover.test.js) | 1 | Partial, v0–v3 storage and the compatibility writer are covered. The Rust writer uses the same format selection as the TypeScript archive reader. 5.2 owns the older reader’s rejection through the Work Queue handoff. |
| | 2 | The replacement restore for an ordinary conversation is covered separately. Group placement belongs to 5.3. |
| | 3 | Remote send and its client receipt belong to 3.2. The group’s saved reads and messages belong to 5.3. The ordinary conversation snapshot is covered under owner-mutations. |
| | 4 | Work Queue operation while the primary is away belongs to 5.2. |
| | 5 | Partial, authoritative conversation reads preserve the latest observation and report missing files. The pending asks view belongs to 3.2. |

The following 29 declarations come from the bot ownership, unlink, export, wipe, and restore
scenarios listed together in the [setup map](rust-setup-coverage.md). The numbers follow that
order and count a parameterized declaration once.

| Cases | Status and remaining owner |
|---|---|
| 1 | Bot owner writes owned by 5.1. |
| 2–10, 27–28 | Owned by 2.3, unlink, wipe and its retry or cancellation. |
| 11, 14–19 | Owned by 2.3 and 2.5, accepted backup and export through removal and restore. |
| 12–13, 20–23, 25–26 | The ordinary move receipt and payload verification are covered. Accepted removal plans and revisions belong to 2.3, export and restore to 2.5, and bot, Work Queue and group payloads to 5.1–5.3. |
| 24 | Owned by 2.3 and 2.5, snapshot and wipe generation. |
| 29 | Live TS-beta transfer portion dropped: TS-beta interop (user 11-23). The Rust restore after issuer disk loss and unlink is covered. 9.3 owns the release and rollback rehearsal. |

## Remaining transfer and delivery work

- Late asks and remote work producers need their 3.2 runtime paths. The live calling paths for
  `send_message` and `read_conversation` belong to the 4.1 tools relay.
- 4.6 owns native carry and bridge transitions. 5.1 owns bot runtime and session notices.
  5.3 owns group runtime and Autopilot transfer. They use the shared journals and decisions.
- 5.2 owns Work Queue item and file placement, including the old-reader rejection across a queue
  handoff. 2.3 owns managed wipe preparation and revisions. 2.5 owns the export and recovery
  workflow. The ordinary move, unlink and replacement restore exercise the shared receipt fences.
