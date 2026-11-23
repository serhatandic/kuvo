# Rust peer coverage

Phase 2.2 reads the layout-23 peer files before serving the first state. Relay mail can replace
those copies only for an enrolled computer, and only with a newer snapshot. Each connection
authenticates its device grant before entering the same dispatcher used by local HTTP and SSH.
The dispatcher rechecks the grant on each request.

The TypeScript boxes keep process-global caches and re-read files for one-shot SSH commands. The
Rust resident owns updates in a Tokio task, publishes immutable views, and does disk work on the
blocking pool. The publisher and paired request streams run as separate tasks so an outbound
stream does not hold the resident's receiver. Generated protocol types describe the route data,
while reach and record ownership have Rust types of their own. Phase 1.5's Noise and mail framing
remain the transport.

`kuvo_peers::relay::RelayHandle` supplies the same relay sender and a typed, bounded event
subscription to other crates. `ResidentPeers` owns the connection and continues to consume
session events directly. Setup can subscribe through the handle and refresh peer documents
after a lagged event stream.

## TypeScript scenario map

Numbers are one-based top-level test declarations at `85255a916`. Covered rows exercise the
scenario through the Rust process or peer boundary. Partial rows name the evidence they share.
Everything else in each row is listed in the last column.

- **First state** is `another_computers_session_is_in_the_first_state_after_restart_and_stays_visible_away`.
- **History** is `kept_activity_and_notices_keep_their_origin_and_dock_deduplicates_a_bot`.
- **Device** is `a_local_app_pairs_once_and_removal_survives_a_resident_restart`.
- **Updating** is `a_restarting_release_keeps_its_session_actions_and_cannot_be_chosen_for_a_start`.
- **Wire and outbox** refer to phase 1.5's [wire and relay coverage](rust-wire-coverage.md).

| Source | Covered | Partial | Gaps |
| --- | --- | --- | --- |
| `box/test/setup-lists.test.ts` (31) | 2–3, 6, 8, 18–19 → First state. 21 and 31 → History. 4 and 11 → Updating. | 9 → Updating checks the saved catalog and agents. 5 and 28 → First state, with a kept ask and pending-member transition. 15 → Updating reads the kept page, with live follow and catch-up still needed. 22 → History checks the activity projection, which the Work route still has to consume. | 1, 7, 10, 12–14, 16–17, 20, 23–27, 29–30. Engine integration, catalog and project cases, cross-computer starts and follows, library, archive deletion and search. |
| `box/test/notices-rule.test.ts` (16) | — | 11 → History reads kept notices through the setup route. | 1–10, 12–16. Notice creation, phone delivery and agent rules. |
| `box/test/notices-plain.test.ts` (7) and `notices-usage.test.ts` (13) | — | — | All 20. Text and usage notice producers. |
| `box/bridge-tests/peers.test.js` (34) | 30 → Wire and outbox. | 5, 16–17 → First state checks authenticated advertisements, copying and computer identity. 7, 9, 11, 13, 22, 24, 26–27, 29 and 34 → Wire and outbox. | 1–4, 6, 8, 10, 12, 14–15, 18–21, 23, 25, 28, 31–33. Setup, legacy roster support and letters' consuming paths. |
| `box/bridge-tests/owner-mutations.test.js` (18) | 7–8 → Updating locates a remote session reported only in its owner’s announcement and prefers the local session when that peer also claims it. | — | 1–6, 9–18. Local and move authority, mutation admission, receipts and forwarded operations. |
| `box/bridge-tests/devices.test.js` (7) | 1–2 → Device. | 7 → Device reaches the dispatcher with a desktop grant. The engine write and note cases remain. | 3–6. Signed device projection, main forwarding, and former issuer guidance. |
| `box/bridge-tests/devices-two-boxes.test.js` (7) | — | 1 → Device proves standalone pairing, live revocation and restart. 2 → Device rejects conflicting mixed-case route-version headers over Noise before dispatch. | 3–7. Revocation propagation while the main is away or its switch is off, signed membership catch-up and uninstall. |
| `box/bridge-tests/reach-elsewhere.test.js` (3) and `reach-elsewhere-rules.test.js` (17) | — | — | All 20. Calls, answers, queue delivery and permission policy for work left on another computer. |
| `box/bridge-tests/start-elsewhere.test.js` (3) and `start-elsewhere-rules.test.js` (45) | — | — | All 48. Addressed starts, admission, engine recovery and receipts. |
| `box/bridge-tests/mac-peers.test.js` (14) | — | 2–3, 6 and 8 → Wire and outbox. | 1, 4–5, 7, 9–14. Mac migration, peer policy, letters, notices and seats. |

Total across these files is 215 cases, with 15 covered, 26 partial and 174 gaps. The shared
attention route scenarios cover lease renewal, the one-minute cap and malformed requests.
The latest kept-file reader preserves the original notification ID and computer. Sending and
holding phone notifications remains with the notice producers.

## Integration still needed

- The setup port supplies the live accepted membership and signed device mutations. The current
  resident loads the checkpointed membership projection at startup and writes desktop grants
  on a configured standalone machine. A managed member can read its accepted grants, while
  managed mutations require the signed setup save and forwarding path. The Mac identity still
  needs the Keychain/bootstrap seam.
- Engine and work feed the resident's local list, catalog, queues, bots and notice history as they
  change. The peer activity projection includes `lastSeenSessions`, while the Work route belongs
  to its consumer. The kept-conversation reader can serve an offline newest page already on disk.
  Live peer follow, renewal and transcript population still need the owning session read routes.
- The journal lookup covers switching and committed session, bot and group moves and suppresses
  local ownership while a move holds the copy. Activation transactions, queue assignment, forwarded submissions, and compatibility roster migration have their
  own consuming phases.

## Shared checks

The conformance suite's three attention scenarios pass on both binaries. Its membership-and-rename
scenario passes on TypeScript and stops at `GET kuvo/setup` on Rust, which needs the setup route
registration. The differential runner agrees on the kept conversation location, waiting count,
known device roster, and refusal to pair from an untrusted local origin. Its home comparison still
reports startup files from other TypeScript areas that the Rust resident has yet to initialize.
