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
scenario through the Rust process or peer boundary. Partial cases name the Rust evidence and
their remaining integration. The owner column assigns every partial and gap from the original
26 partials and 174 gaps. It retains the assignment of cases closed in this pass.

- **First state** is `another_computers_session_is_in_the_first_state_after_restart_and_stays_visible_away`.
- **History** is `kept_activity_and_notices_keep_their_origin_and_dock_deduplicates_a_bot`.
- **Device** is `a_local_app_pairs_once_and_removal_survives_a_resident_restart`.
- **Updating** is `a_restarting_release_keeps_its_session_actions_and_cannot_be_chosen_for_a_start`.
- **Paging** is `an_away_worktree_pages_under_its_own_computers_project`.
- **Work** is `a_lead_can_list_summarize_and_stop_work_elsewhere_and_only_answer_within_its_rules`.
- **Work return** is `an_away_computer_keeps_its_last_work_rows_and_the_stop_reaches_it_on_return`.
- **Work replay** is `a_resent_call_keeps_its_answer_after_restart_and_an_expired_allow_has_no_effect`.
- **Work retry** is `an_away_peer_gets_its_oldest_call_at_each_retry_until_it_returns`.
- **Wire and outbox** refer to phase 1.5's [wire and relay coverage](rust-wire-coverage.md).

| Source | Covered | Partial | Gaps | Owner phase |
| --- | --- | --- | --- | --- |
| `box/test/setup-lists.test.ts` (31) | 2–3, 6, 8, 18–19 → First state. 21 and 31 → History. 4 and 11 → Updating. 7 → Paging. | 9 → Updating proves saved catalogs; 5 and 28 → First state proves waiting and pending membership; 10 → First state proves the session and computer return; 15 → Updating proves the kept page; 22 → History proves the activity projection. | 1, 12–14, 16–17, 20, 23–27, 29–30. | 2.2 — 7, 10, 28 (session and computer projection); 2.4 — 16, 28 (moves); 3.2 — 1, 5, 9, 12, 14–15, 24–27, 29–30; 5.1 — 10, 13, 22, 28 (bots); 5.2 — 20, 28 (Work Queue); 5.5 — 17, 23, 28 (library). |
| `box/test/notices-rule.test.ts` (16) | — | 11 → History reads kept notices through the setup route. | 1–10, 12–16. Notice creation, phone delivery and agent rules. | 5.1 — 1–16. |
| `box/test/notices-plain.test.ts` (7) and `notices-usage.test.ts` (13) | — | — | All 20. Text and usage notice producers. | 5.1 — all 7 in notices-plain; 5.4 — all 13 in notices-usage. |
| `box/bridge-tests/peers.test.js` (34) | 30 → Wire and outbox. | 5, 16–17 → First state checks authenticated announcements, copying, address, reach and working count. The ten-minute repeat and legacy agent projection remain. 7, 9, 11, 13, 22, 24, 26–27, 29 and 34 → Wire and outbox. | 1–4, 6, 8, 10, 12, 14–15, 18–21, 23, 25, 28, 31–33. Setup, legacy roster support and letters' consuming paths. | 2.1 — 18–19, 22–24; 2.2 — 5, 16–17; 2.3 — 21; 2.4 — 6–14, 26–29, 31–34; 2.6 — 1, 5, 15–17 (legacy projection); 3.2 — 20; 7.3 — 2–4, 25. |
| `box/bridge-tests/owner-mutations.test.js` (18) | 7–8 → Updating locates a remote session reported only in its owner’s announcement and prefers the local session when that peer also claims it. | — | 1–6, 9–18. Local and move authority, mutation admission, receipts and forwarded operations. | 2.1 — 4–6; 2.4 — 1–3, 9–18. |
| `box/bridge-tests/devices.test.js` (7) | 1–2 → Device. | 7 → Device reaches the dispatcher with a desktop grant. The engine write and note cases remain. | 3–6. Signed device projection, main forwarding, and former issuer guidance. | 2.2 — 3–6 (held for the 2.1 managed device adapter); 3.2 — 7. |
| `box/bridge-tests/devices-two-boxes.test.js` (7) | — | 1 → Device proves standalone pairing, live revocation and restart. 2 → Device rejects conflicting mixed-case route-version headers over Noise before dispatch. | 3–7. Revocation propagation while the main is away or its switch is off, signed membership catch-up and uninstall. | 2.2 — 1, 3–5 (held for the 2.1 managed device adapter); 3.2 — 2 (prompt dispatch after version admission); 7.2 — 6–7 (uninstall, held for the same adapter). |
| `box/bridge-tests/reach-elsewhere.test.js` (3) | — | 1 → Work exercises calls across two computers. 2 → Work return exercises kept rows, queued stop and return. 3 → Work return uses the same wire protocol across machine and Mac roles. | — | 2.2 — 1–3 (relay, queue and permission); 3.2 — 1–3 (real engine effects, ask context and conversation reads); 2.4 — 1, 3 (real move fixtures). |
| `box/bridge-tests/reach-elsewhere-rules.test.js` (17) | 1 → Work replay and Work return; 5 → Work and Work replay; 6, 11 → Work replay; 8 → Work return; 12 → Work; 15, 17 → Work. | 2 → Work proves ancestry refusal through the adapter; 7, 14 → Work return proves the addressed snapshot, kept rows and an empty clear; 9 → Work retry and Work replay prove oldest-first, retry and expired allow; 16 → Work exercises both rule sets in isolation. | 3–4, 10, 13. Switched moves, ordinary letters and ancestry, and reporting to a moved lead. | 2.2 — 1–2, 5–9, 11–12, 14–17; 2.4 — 3–4, 10, 13; 3.2 — 2, 7–8, 14 (real engine and local session/ancestor feed). |
| `box/bridge-tests/start-elsewhere.test.js` (3) and `start-elsewhere-rules.test.js` (45) | — | — | All 48. Addressed starts, admission, engine recovery and receipts. | 3.2 — all 3 in start-elsewhere and all 45 in start-elsewhere-rules. |
| `box/bridge-tests/mac-peers.test.js` (14) | — | 2–3, 6 and 8 → Wire and outbox. | 1, 4–5, 7, 9–14. Mac migration, peer policy, letters, notices and seats. | 2.4 — 2–3, 6, 8, 10–12; 2.6 — 5; 5.1 — 7, 13–14; 5.3 — 4, 9; 7.3 — 1 (legacy Mac roster and agent projection). |

Total across these files is 215 cases, with 24 covered, 35 partial and 156 gaps. The shared
attention route scenarios cover lease renewal, the one-minute cap and malformed requests.
The latest kept-file reader preserves the original notification ID and computer. Sending and
holding phone notifications remains with the notice producers.

## Integration still needed

- The engine registers `ResidentPeers::attach_work(Arc<dyn calls::RemoteWork>)` once. Its
  `remote_work::WorkHandle` sends authenticated calls and publishes complete work snapshots on
  the resident relay. The adapter supplies ancestry, the current ask and derived permission
  checks, and applies effects and queue delivery idempotently by ID. It must recheck authority
  during an effect. Phase 3.2 registers the local engine/bridge adapter, conversation reads
  and the open-work/start state machine. Phase 2.4 supplies switched-session and moved lead
  delivery. The binary advertises `work.elsewhere` only after attachment.
- The peer task retains the TypeScript outbox call and kept work rows formats. It journals call
  results under `run/work/call-results` and accepted answer origins under `run/work/call-answers`.
  Engine effects and answer queue deliveries carry a stable ID across restart. An empty kept
  rows map carries its timestamp to reject a delayed snapshot.

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
