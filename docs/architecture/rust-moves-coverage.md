# Rust moves and ownership coverage

Phase 2.4 is open. The Rust resident reads existing move journals and client outcomes,
publishes and receives move snapshots, and replies to an authenticated retry for a
committed ownership decision. It has codecs for bundle manifests and ownership receipts.
It does not yet drive a transfer or change ownership over the relay.

## Proof

| Scenario | What it exercises |
|---|---|
| Stored outcomes | A resident reads a held incoming copy, v0 journal and departure records, a successful v1 outcome, a legacy refusal, and a missing result through the move routes. Both residents run the same case. |
| Old departure | The location route still leads to the owner after the v0 departure stops being advertised. Both residents run the same case. |
| Other computers | A third computer sees an authenticated in-progress snapshot and receives its removal on the move event stream. Both residents run the same case. |
| Lost switch reply | A TypeScript source restarts with a pending switch. The Rust destination restarts with its verified, generation-bound receipt. Over the local relay, the source sees completion and records the departure. The test seeds the committed copy and does not transfer a conversation. |
| Golden receipt | Both residents open a current TypeScript golden home and keep its switch receipt byte for byte. |
| Bound decision | A real Rust resident receives authenticated queries for an unknown move, a different generation and a different hash before it answers the valid retry. The reply proves the committed generation and inventory only. |
| Stored formats | The manifest reader accepts unmarked and stamped v0, v1, v2 and v3. Writers use v1 for plain bundles, v2 when they carry session notes, and v3 when they carry forwarding evidence, as the TypeScript packer does. Rust storage scenarios also check immutable receipts, independent hints, and the unmarked legacy decision map after reopening. |
| Differential | Both residents answer the same three listing and outcome requests and preserve the same seeded move files. The full inventory still differs in unrelated bootstrap, usage, library, engine-list and setup files. |

Run the shared routes against each binary with `KUVO_CONFORMANCE_AREA=moves`.
With the TypeScript binary selected, set `KUVO_CONFORMANCE_MOVE_PEER` to the Rust
binary to include the mixed restart scenario. The crate's `move_storage` test and the
`a_destination_replays_only_the_committed_bundles_receipt` relay test cover the other
local checks.

Rust uses typed journal views and receipts, the resident's peer actor for authenticated mail
and timed announcements, and `kuvo_store` for atomic writes and durable sealing.
A receipt writer holds a scoped lock across the read and write. The replay path only answers
committed decisions that can be sealed. A live activation journal belongs to the move worker.

The three shared move scenarios and five Rust storage and relay cases passed on
macOS, WSL2 with GNU libc, static Linux musl under WSL2, and native Windows.
The macOS workspace compile check, the mixed switch-reply scenario, and the
TypeScript–Rust reach differential also passed.

## Peer reach files

The differential runner checks both `run/peer-reach.json` and
`run/peer-reach-checked.json`. A fresh home with no observations leaves both files absent.
An unreachable peer creates both records. When that peer answers, both files remain
and its keys are removed. Another peer's saved numeric observations survive both
writes, including fractional timestamps accepted by the TypeScript reader. The
pre-port Rust build dropped those entries.

The mixed reach scenario compares those states as well as the final home files.
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
below counts 209 declarations. Two have matching peer coverage from 2.2, 13 have partial
proof, and 194 are open here or assigned to their existing owner. A storage or mail proof
alone never marks a full transfer scenario covered.

| Reference | Cases | Status and remaining owner |
|---|---|---|
| [moves](../../box/bridge-tests/moves.test.js) | 1–16 | Open, 2.4 transfer and lookup. OpenCode inventory, queue and import semantics need 3.2, bot/group and queue effects need 5.1–5.3. |
| [moves-elsewhere](../../box/bridge-tests/moves-elsewhere.test.js) | 1 | Partial, authenticated snapshots and a stream removal are exercised. Add both parties' successive clear and independent projections in 2.4. |
| | 2 | Partial, filtering one's own moves is exercised. Add malformed inputs, non-members, sender clock changes and injected expiry in 2.4. |
| | 3 | Open, prove expiry on a quiet peer with an injected clock in 2.4. |
| | 4–5 | Partial, first snapshot and explicit clear cross the relay. Prove minute refresh, rejected sends, reconnect timing and the waiting bit in 2.4. |
| [moves-lanes](../../box/bridge-tests/moves-lanes.test.js) | 1–9 | Open, native transfer 2.4 and 4.6. |
| [moves-lanes-races](../../box/bridge-tests/moves-lanes-races.test.js) | 1–16 | Open, transfer checks and recovery 2.4, native carry and retention 4.6. |
| [moves-races](../../box/bridge-tests/moves-races.test.js) | 9, 24, 26, 31 | Partial, committed replay and unknown or mismatched proof cases are exercised. Safe source rollback, replay at each cut, and live activation remain 2.4. |
| | 19 | Partial, Rust loads durable departures and source switch journals into peer lookup. Add the reference's multi-hop and advertisement expiry scenarios in 2.4. |
| | 20, 34 | Partial, switch recovery or terminal client receipt is exercised with seeded state. Add removal fencing (2.3 and 2.4) and source cleanup against a failing engine (2.4 and 3.2). |
| | 37 | Partial, live progress names come from accepted membership in the third-computer scenario. Add source and recipient progress during an initiated transfer in 2.4. |
| | 1–8, 10–18, 21–23, 25, 27–30, 32–33, 35–36, 38–55 | Open, 2.4 transfer, retry, contention and cleanup. OpenCode import and queue effects depend on 3.2. Carry and agent retention depend on 4.6. |
| [moves-recover](../../box/bridge-tests/moves-recover.test.js) | 1–12, 15–17, 21–22 | Open, 2.4 replay and rollback with engine and group effects from 3.2 and 5.3. |
| | 13–14, 18–20, 23–24 | Open, 2.4 ordinary letters, notes and delayed delivery, including forwarding after a move. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) | 1–3, 9–18 | Open, 2.4 owner mutation claims, observations, replay and handoff evidence. Queue and session effects depend on 5.2 and 3.2. |
| | 4–6 | Setup-owned, 2.1. See [setup coverage](rust-setup-coverage.md). |
| | 7–8 | Covered by 2.2's Updating scenario for an announcement-only owner and a local session also claimed by a peer. |
| [peers](../../box/bridge-tests/peers.test.js) | 6–14, 26–29, 31–34 | Open, 2.4 letter handling through the resident. [2.2](rust-peers-coverage.md) proves parts of mail and outbox storage. |
| [mac-peers](../../box/bridge-tests/mac-peers.test.js) | 2–3, 6, 8, 10–12 | Open, 2.4 letter handling and Mac peer policy. The linked peer map also covers wire pieces. |
| [setup-lists](../../box/test/setup-lists.test.ts) | 16, 28 | Open, 2.4 real move projections alongside the relevant merged views in 2.2, 5.1 and 5.2. |
| [reach-elsewhere](../../box/bridge-tests/reach-elsewhere.test.js) | 1, 3 | Open, 2.4 real move fixtures, with engine effects in 3.2. [2.2](rust-peers-coverage.md) proves the authenticated call transport. |
| [reach-elsewhere-rules](../../box/bridge-tests/reach-elsewhere-rules.test.js) | 3–4, 10, 13 | Open, 2.4 switched sessions, letters, ancestry and returning results to a moved lead. |
| [setup-handover, move and owner reads](../../box/bridge-tests/setup-handover.test.js#L520-L837) | v3 compatibility | Partial, v0–v3 storage and the compatibility writer are covered. Old-reader rejection through the archive and queue handoffs remain 2.4 and 5.2. |
| | Group survives source loss, away owner, standalone work with primary away, authoritative remote reads | Open, 2.4 transfer and owner reads with 2.2, 2.5, 3.2 and 5.2. |

The following 29 declarations are the broader [setup handover and removal group](../../box/bridge-tests/setup-handover.test.js#L2305-L4139)
assigned jointly to phases 2.3–2.5 by the [setup map](rust-setup-coverage.md). Numbers restart at the
start of that range and include a parameterized declaration once.

| Cases | Status and remaining owner |
|---|---|
| 1 | Open here, bot owner writes in 2.4 and 5.1. |
| 2–10, 27–28 | Owned by 2.3, unlink, wipe and its retry or cancellation. |
| 11, 14–19 | Owned by 2.3 and 2.5, accepted backup and export through removal and restore. |
| 12–13, 20–23, 25–26 | Open, real move and byte verification in 2.4, removal admission and revisions in 2.3, export and persisted data in 2.5, 3.2 and 5.2. |
| 24 | Owned by 2.3 and 2.5, snapshot and wipe generation. |
| 29 | Open, live mixed move and receipt replay in 2.4, disk loss and restore in 2.5, release rehearsal in 9.3. |

## Next transfer seam

The OpenCode port exposes session listing and held inbox reconciliation. A live move needs a
named engine boundary for inspecting, fencing, exporting, importing and retiring a session tree
and its paused queue. It must provide import ownership proof for rollback and serialize new
prompt admissions with the source's final comparison. Native lanes, group copies, Work Queue
files, and forwarded-edit evidence then use the same transfer protocol with their own data
owners. The proof still needs a real session going each way over a mixed relay and deliberate
interruption of every transfer step.
