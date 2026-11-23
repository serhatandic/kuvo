# Rust moves and ownership coverage

Phase 2.4 is open. The Rust resident moves ordinary OpenCode chats and their
descendants, wake-up folders, managed attachments, held prompt queues, ordinary note histories
and session-scoped forwarded decisions. A destination reply proves the generation and hash it
committed. The source saves a backup before retiring its original. The peer
resident routes ordinary letters and notes through the same owner lookup used
for addressed work and receipt lookup.

## Proof

| Scenario | What it exercises |
|---|---|
| Stored outcomes | A resident reads a held incoming copy, v0 journal and departure records, a successful v1 outcome, a legacy refusal, and a missing result through the move routes. Both residents run the same case. |
| Old departure | The location route still leads to the owner after the v0 departure stops being advertised. Both residents run the same case. |
| Other computers | A third computer sees an authenticated in-progress snapshot and receives its removal on the move event stream. Both residents run the same case. |
| Live transfer | Two Rust residents move a chat, its child, a paused queue, an ordinary note and a replay fence in each direction between the main computer and a member. The former computer's outbox still names its old owner. The letter reaches the new owner once. The TypeScript reference runs the same transfer and checks the records it leaves. |
| Lost part, arrival and switch replies | The relay holds the live Rust transfer's first packets for `check`, `checked`, `offer`, `ready`, `part`, `got`, `working`, `arrive`, `arrived`, `switch` and `switched`. The source resends the part whose acknowledgement was lost, and retries arrival after a minute. The destination restarts after committing the switch. The source reconnects and recovers the acknowledgement through its next scheduled switch retry. |
| Interrupted import | The engine accepts the import but its reply is cut short by a destination restart. The destination deletes just that copy using the import receipt. The source keeps its original and reports failure. |
| Waiting and recent requests | On the same live switch the source shows it is waiting for the destination after its reply is lost. A resident also drives a recent request recorded before its first check when it starts, while a second simultaneous request is refused. The live transfer imports children from both engine pages. |
| Unproven import | An authenticated drop sends proof that the source can stay active. Both residents keep an older incoming journal held when it has no evidence that its local session came from the move. |
| Arrival query and size limit | Both residents answer an authenticated repeat with the held copy's generation and hash and report failure for an unknown copy. An offer one byte over 1 GB receives the size-limit refusal without opening a copy. |
| Destination checks | Both residents show the destination by name and mention a running turn in the move confirmation. Before pausing a chat, an unanswered check returns HTTP 503 and a missing engine provider returns a sign-in refusal. A retry after sign-in moves the original. |
| Wake-up folder | Both residents hold a wake-up folder on each computer until the switch. The source uses configured roots outside its home. The destination rebases the directory and restores the saved pause. The source retains a backup with its conversation. Rust also moves the unpaused folder back. |
| Legacy wake-up hold | Both residents restore the saved pause in a failed v0 journal, remove an orphaned bare `moving` marker and retain a pause that names another move. |
| Waited letter | Both residents identify the sender by its own announcement. The destination sends an answer after the engine reply. The original tool gets it once through its saved letter ID and authenticated destination. |
| Refused letter | Both residents give the original sender the authenticated refusal as one queued note and leave the archived recipient untouched. Rust finishes a durable refusal if it restarts before writing the notice. |
| Managed files | Both residents refuse missing and export-only attachments before switching. The destination keeps the exact bytes, verifies the file receipt, rebases typed root and child references and a held prompt while preserving a quotation, and opens the file after its own restart and loss of the original file. |
| Prompt fence | A journal survives a resident restart and refuses direct prompts, commands, sends and queue resumes. The Rust composer keeps a saved draft pending during the hold, then delivers it once after release. |
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

The TypeScript sender counts an authenticated `got` index before checking that
it identifies a part in the bundle. For a one-part bundle, `got` with index 1
can make it ask for arrival before part 0 gets there. Rust only counts indices
in the bundle. The TypeScript source is unchanged.

The final full move area passed 21 Rust scenarios and all 19 applicable
TypeScript scenarios on macOS, including the two-binary reach comparison.
WSL2 with GNU libc, static Linux musl under WSL2 and native Windows each
passed 20 Rust scenarios across full runs and focused reruns. The optional
two-binary reach case ran on macOS. Focused transfer, relay, receipt and
engine tests passed on all four platforms. Native Windows also passed both
file-sharing scenarios after an earlier live run hit a transient journal
replacement error. The tests use disposable residents, a loopback relay and
an OpenCode stand-in.

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
below counts 212 declarations. Fourteen have full scenario coverage, 23 have partial proof,
and 175 remain open or assigned to their existing owner. Mixed-only portions are recorded
separately where they appear.

| Reference | Cases | Status and remaining owner |
|---|---|---|
| [moves](../../box/bridge-tests/moves.test.js) | 3, 11 | Partial, an ordinary letter follows a live move and a paused prompt queue still delivers after resume. The exact message tool path and multiple-item queue order are 2.4 and 3.2. |
| | 1 | Covered by move choices for a running turn, an away destination and a missing chat. |
| | 2 | Partial, both residents move a wake-up folder under the journal's hold with separate chats and wake-up roots. The actual installed Mac and its Trash still need 2.4 and 9.3 proof. |
| | 7–8 | Covered by the live transfer of a tree with more children than one engine page and by two simultaneous requests competing for the same chat. |
| | 12, 14 | Partial, typed managed files travel with the root, its child and a held prompt. Engine-held order with multiple attachments, duplicate basenames, and the Mac app's separate attachment root are still 2.4 and 3.2. |
| | 15–16 | Covered by a live source refusing an expired attachment and a file path outside its managed roots. |
| | 4–6, 9–10, 13 | Open, 2.4 Autopilot and other transfer shapes. Engine queue effects are 3.2. Bot and group runtime parts are 4.6, 5.1 and 5.3. |
| [moves-elsewhere](../../box/bridge-tests/moves-elsewhere.test.js) | 1 | Partial, authenticated snapshots and a stream removal are exercised. Add both parties' successive clear and independent projections in 2.4. |
| | 2 | Partial, filtering one's own moves is exercised. Add malformed inputs, non-members, sender clock changes and injected expiry in 2.4. |
| | 3 | Open, prove expiry on a quiet peer with an injected clock in 2.4. |
| | 4–5 | Partial, first snapshot and explicit clear cross the relay. Prove minute refresh, rejected sends, reconnect timing and the waiting bit in 2.4. |
| [moves-lanes](../../box/bridge-tests/moves-lanes.test.js) | 1–9 | Open, native transfer 2.4 and 4.6. |
| [moves-lanes-races](../../box/bridge-tests/moves-lanes-races.test.js) | 1–16 | Open, transfer checks and recovery 2.4, native carry and retention 4.6. |
| [moves-races](../../box/bridge-tests/moves-races.test.js) | 9, 24, 26 | Partial, live activation and replay after a lost switch reply, unknown or mismatched proof, and durable drop are exercised. Automatic retry across the complete set of peer and wall-clock interruptions remains 2.4. |
| | 19 | Partial, Rust loads durable departures and source switch journals into peer lookup. Add the reference's multi-hop and advertisement expiry scenarios in 2.4. |
| | 20, 34 | Partial, switch recovery or terminal client receipt is exercised with seeded state. Add removal fencing (2.3 and 2.4) and source cleanup against a failing engine (2.4 and 3.2). |
| | 31 | Covered by the live lost switch acknowledgement and by a restarted destination replying only with its durable decision. |
| | 36 | Covered by the authenticated oversized offer, exact refusal and empty destination. |
| | 37 | Partial, live progress names come from accepted membership in the third-computer scenario. Add source and recipient progress during an initiated transfer in 2.4. |
| | 39 | Covered by a live source whose durable switch is unanswered for 90 seconds and whose repeated switch later recovers the destination receipt. |
| | 45 | Partial, a live chat returns to the same computer and discards the earlier move's backup. The pre-activation retention and timed cleanup checks remain 2.4. |
| | 50, 52 | Partial, the receiver refuses an unavailable OpenCode provider before the source holds its work and permits a retry after sign-in. Native coding-agent preflight and replacement account notes remain 2.4 and 4.6. |
| | 1–8, 10–18, 21–23, 25, 27–30, 32–33, 35, 38, 40–44, 46–49, 51, 53–55 | Open, 2.4 transfer, retry, contention and cleanup. OpenCode import and queue effects depend on 3.2. Carry and agent retention depend on 4.6. |
| [moves-recover](../../box/bridge-tests/moves-recover.test.js) | 5–7, 21 | Covered by the fresh arrival reply after a lost reply, the known and unknown arrival queries, an interrupted engine import and its rollback, and an older held copy without import ownership. |
| | 10 | Partial, the resident drives a request saved just before its first check. Timing out the same class of request still stuck when no hold time was recorded remains 2.4. |
| | 1–4, 8–9, 11–12, 15–17, 22 | Open, 2.4 replay and rollback with engine and group effects from 3.2 and 5.3. |
| | 13–14, 18–20, 23–24 | Open, 2.4 ordinary letters, notes and delayed delivery, including forwarding after a move. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) | 1–3, 9–18 | Open, 2.4 owner mutation claims, observations, replay and handoff evidence. Queue and session effects depend on 5.2 and 3.2. |
| | 4–6 | Setup-owned, 2.1. See [setup coverage](rust-setup-coverage.md). |
| | 7–8 | Covered by 2.2's Updating scenario for an announcement-only owner and a local session also claimed by a peer. |
| [peers](../../box/bridge-tests/peers.test.js) | 34 | Partial, an archived chat's refusal becomes one queued note for the conversation that sent it. The forged refusal and restart while the failure notice is pending still need a 2.4 resident test. |
| | 7 | Partial, a named verified sender and waited reply have a live two-computer test. Unknown sender and duplicate delivery in the same scenario still need 2.4 proof. |
| | 6, 8–14, 26–29, 31–33 | Open, 2.4 letter handling through the resident. [2.2](rust-peers-coverage.md) proves parts of mail and outbox storage. |
| [mac-peers](../../box/bridge-tests/mac-peers.test.js) | 2–3, 6, 8, 10–12 | Open, 2.4 letter handling and Mac peer policy. The linked peer map also covers wire pieces. |
| [setup-lists](../../box/test/setup-lists.test.ts) | 16, 28 | Open, 2.4 real move projections alongside the relevant merged views in 2.2, 5.1 and 5.2. |
| [reach-elsewhere](../../box/bridge-tests/reach-elsewhere.test.js) | 1, 3 | Open, 2.4 real move fixtures, with engine effects in 3.2. [2.2](rust-peers-coverage.md) proves the authenticated call transport. |
| [reach-elsewhere-rules](../../box/bridge-tests/reach-elsewhere-rules.test.js) | 3–4, 11, 14 | Open, 2.4 switched sessions, letters, ancestry and returning results to a moved lead. The last two were numbered 10 and 13 before the stale snapshot scenario was added. |
| [offline-merge](../../box/test/offline-merge.test.ts) | 4, 8–9 | Open, refinement of an older note passage on the new owner in 2.4 and 3.2, and Work Queue deletion and former-owner replay in 2.4 and 5.2. |
| [setup-handover, move and owner reads](../../box/bridge-tests/setup-handover.test.js#L15) | v3 compatibility | Partial, v0–v3 storage and the compatibility writer are covered. Old-reader rejection through the archive and queue handoffs remain 2.4 and 5.2. |
| | Group survives source loss, away owner, standalone work with primary away, authoritative remote reads | Open, 2.4 transfer and owner reads with 2.2, 2.5, 3.2 and 5.2. |

The following 29 declarations come from the bot ownership, unlink, export, wipe, and restore
scenarios listed together in the [setup map](rust-setup-coverage.md). The numbers follow that
order and count a parameterized declaration once.

| Cases | Status and remaining owner |
|---|---|
| 1 | Open here, bot owner writes in 2.4 and 5.1. |
| 2–10, 27–28 | Owned by 2.3, unlink, wipe and its retry or cancellation. |
| 11, 14–19 | Owned by 2.3 and 2.5, accepted backup and export through removal and restore. |
| 12–13, 20–23, 25–26 | Open, real move and byte verification in 2.4, removal admission and revisions in 2.3, export and persisted data in 2.5, 3.2 and 5.2. |
| 24 | Owned by 2.3 and 2.5, snapshot and wipe generation. |
| 29 | Live TS-beta transfer portion dropped: TS-beta interop (user 11-23). Rust receipt replay with disk loss and restore remains 2.4 and 2.5, with release rehearsal in 9.3. |

## Remaining transfer and delivery work

- 2.4 owns retry and waiting progress parity, wake-up and attachment edge cases,
  source and destination contention, handoff recovery for forwarded edits and ordinary
  letters with a tool that gives up and transcript reads.
- 3.2 owns the effect and receipt adapter behind the new ownership handle, typed consent
  and restored queue effects. 2.4's peer forwarding carries both draft limits through multipart mail.
- 4.6, 5.1 and 5.3 own native coding-agent carry and bot and group runtime placement. 2.4
  owns their integration with the same relay steps, journals and immutable decisions.
- 5.2 owns the Work Queue item and file placement, with 2.4 responsible for preserving
  its handoff proof during a move. 2.3 and 2.5 own removal admission and restoration
  of copies after source disk loss, using the 2.4 receipt.
