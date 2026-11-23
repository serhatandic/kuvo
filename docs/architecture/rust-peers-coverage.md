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
- **Computer facts** is `computers_show_only_valid_facts_from_the_latest_announcement`.
- **Claims** is `a_session_has_a_remote_owner_only_while_one_computer_claims_it`.
- **Forwarding** is `a_member_forwards_device_pairing_and_revocation_to_the_main`.
- **Announcements** is `peers_announce_changes_answer_refreshes_and_repeat_after_ten_minutes`.
- **Managed** is `a_reconnecting_computer_gets_signed_device_removal_from_a_member_while_main_is_away`. The setup authority is also tested in [R4 and R5](rust-setup-coverage.md).
- **Paging** is `an_away_worktree_pages_under_its_own_computers_project`.
- **Work** is `a_lead_can_list_summarize_and_stop_work_elsewhere_and_only_answer_within_its_rules`.
- **Work return** is `an_away_computer_keeps_its_last_work_rows_and_the_stop_reaches_it_on_return`.
- **Work replay** is `a_resent_call_keeps_its_answer_after_restart_and_an_expired_allow_has_no_effect`.
- **Work snapshots** is `work_rows_go_only_to_the_leads_computer_retry_until_acknowledged_and_wait_while_away`.
- **Work retry** is `an_away_peer_gets_its_oldest_call_at_each_retry_until_it_returns`.
- **Wire and outbox** refer to phase 1.5's [wire and relay coverage](rust-wire-coverage.md).

| Source | Covered | Partial | Gaps | Owner phase |
| --- | --- | --- | --- | --- |
| `box/test/setup-lists.test.ts` (31) | 2–3, 6, 8, 18–19 → First state. 21 and 31 → History. 4 and 11 → Updating. 7 → Paging, including the archived project and both pages. | 9 → Updating proves saved catalogs. 5 → First state proves waiting and pending membership. 10 → First state proves live rows, actions and reach on return. 28 → First state proves pending and away session, waiting and notice views. 15 → Updating proves the kept page. 22 → History proves the activity projection. | 1, 12–14, 16–17, 20, 23–27, 29–30. | 2.4 — 16, 28 (moves); 3.2 — 1, 5, 9, 12, 14–15, 24–27, 29–30 (live session routes, including archived sessions); 5.1 — 10, 13, 22, 28 (bot views); 5.2 — 20, 28 (Work Queue); 5.5 — 17, 23, 28 (library). |
| `box/test/notices-rule.test.ts` (16) | — | 11 → History reads kept notices through the setup route. | 1–10, 12–16. Notice creation, phone delivery and agent rules. | 5.1 — 1–16. |
| `box/test/notices-plain.test.ts` (7) and `notices-usage.test.ts` (13) | — | — | All 20. Text and usage notice producers. | 5.1 — all 7 in notices-plain; 5.4 — all 13 in notices-usage. |
| `box/bridge-tests/peers.test.js` (34) | 30 → Wire and outbox. | 5 → Claims proves validation, newer-only retention and ambiguous ownership. 16 → Announcements proves refreshes, list changes and ten-minute repeat. 17 → First state and Computer facts prove address, enrollment, reach, working count, compatibility and sanitization. 7, 9, 11, 13, 22, 24, 26–27, 29 and 34 → Wire and outbox. | 1–4, 6, 8, 10, 12, 14–15, 18–21, 23, 25, 28, 31–33. Setup, legacy roster support and letters’ consuming paths. | 2.1 — current equivalents in B2, B5, B17, B20–B21 and D2; 2.3 — 21; 2.4 — 6–14, 26–29, 31–34; 2.6 — 1, 5, 15–17 (legacy roster and `run/agents.json` export); 3.2 — 17, 20 (live local engine facts and list production); 4.1 — 17 (actual status of this computer’s agents); 7.3 — 2–4, 18–19, 22–25 (legacy mail, roster and libraries). |
| `box/bridge-tests/owner-mutations.test.js` (18) | 7–8 → Updating locates a remote session reported only in its owner’s announcement and prefers the local session when that peer also claims it. | — | 1–6, 9–18. Local and move authority, mutation admission, receipts and forwarded operations. | 2.1 — 4–6 covered by B1, B6 and B10; 2.4 — 1–3, 9–18. |
| `box/bridge-tests/devices.test.js` (7) | 1–2 → Device. | 4 → Forwarding and Managed prove the signed main’s pairing reply, receipt replay, removal refusal and wait response while the main is gone. 6 → Managed proves the signed grant projection and authenticated desktop admission. 7 → Device proves dispatch over a grant. | 3, 5. Former issuer guidance and a computer set aside. | 2.3 — 5; 3.2 — 7 (prompt dispatch); 5.1 — 6 (phone notification filtering); 7.3 — 3, 4 (legacy issuer and forwarding). |
| `box/bridge-tests/devices-two-boxes.test.js` (7) | 4 → Managed verifies both signed exports after a sync request. | 1 → Managed proves pairing through the member, the same desktop grant and device routes at both computers, and revocation by a second device that stays connected. 2 → Device proves mixed-case route-version admission in Noise. 5 → Managed proves three-resident catch-up while the main is away. | 3, 6–7. Removal with incoming devices off and uninstall. | 2.3 — 1, 3, 6–7 (incoming-device switch and removal); 3.2 — 1–2 (prompt dispatch); 7.3 — 1, 5 (legacy phone invites and previous-build catch-up). |
| `box/bridge-tests/reach-elsewhere.test.js` (3) | — | 1 → Work exercises authenticated calls, arguments, permission and replies across two computers. 2 → Work return proves a live call gets the relay’s refusal promptly, kept rows retain their last-seen time and a queued stop settles on return. 3 → Work exercises calls in both directions between Mac and machine roles. | — | 3.2 — 1–3 (real engine effects, ask context and conversation reads); 2.4 — 1, 3 (real move fixtures). |
| `box/bridge-tests/reach-elsewhere-rules.test.js` (17) | 1 → Work replay and Work return. 5 → Work and Work replay. 6, 11 → Work replay. 9 → Work retry and Work return. 12, 15–17 → Work. | 2 → Work, Work replay and authenticated relay admission. 7, 14 → Work snapshots, including spacing, acknowledgments and reconnect. 8 → Work return, including retained owner, empty snapshot, stale replay and malformed rows. | 3–4, 10, 13. Switched moves, ordinary letters and ancestry, and reporting to a moved lead. | 2.4 — 3–4, 10, 13 (moves and letters); 3.2 — 2, 7–8, 14 (real engine and local session/ancestor feed). |
| `box/bridge-tests/start-elsewhere.test.js` (3) and `start-elsewhere-rules.test.js` (45) | — | — | All 48. Addressed starts, admission, engine recovery and receipts. | 3.2 — all 3 in start-elsewhere and all 45 in start-elsewhere-rules. |
| `box/bridge-tests/mac-peers.test.js` (14) | — | 2–3, 6 and 8 → Wire and outbox. | 1, 4–5, 7, 9–14. Mac migration, peer policy, letters, notices and seats. | 2.4 — 2–3, 6, 8, 10–12; 2.6 — 5; 5.1 — 7, 13–14; 5.3 — 4, 9; 7.3 — 1 (legacy Mac roster and agent projection). |

Phase 2.2’s relay and route assertions are exercised by the named scenarios above. Other owners
are listed where the TypeScript scenario also depends on their components. The shared attention
routes cover lease renewal, the one-minute cap and malformed requests.

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

- The managed device route uses `devices::ManagedDevices` on `SetupHandle`. The setup task
  owns the signed save and relay trust updates. The device route, Noise dispatcher and
  addressed SSH use the same accepted device view. Reconnecting members can forward the
  main's retained signature from `setupReplica` in `run/link.json`. Incoming-device-off
  removal and uninstall belong to 2.3. The Mac identity still needs the Keychain/bootstrap seam.
- Engine and work feed the resident's local list, catalog, queues, bots and notice history as they
  change. The peer activity projection includes `lastSeenSessions`, while the Work route belongs
  to its consumer. The kept-conversation reader can serve an offline newest page already on disk.
  Live peer follow, renewal and transcript population still need the owning session read routes.
- The journal lookup covers switching and committed session, bot and group moves and suppresses
  local ownership while a move holds the copy. Activation transactions, queue assignment, forwarded submissions, and compatibility roster migration have their
  own consuming phases.

## Shared checks

Both binaries pass the peers cases for a kept session while its computer is away and for two
enrolled residents hearing each other over the relay. Both pass the three attention cases.
The mixed TypeScript–Rust check exchanges a paired request and an outbox receipt through the
production relay on loopback. The 17 TypeScript reach rules and seven device cases also pass. TypeScript peers case 17
expects a zero working count immediately when the relay marks a peer away. The implementation
preserves the count during its two-minute quiet grace. The Rust test checks zero after that grace.

All 15 Rust peer scenarios pass on macOS, native Windows and the static Linux musl build
running under WSL. The macOS workspace compile check also passes. Lokum's SSH connection
became unavailable before the container check completed.

The TypeScript enrollment scenario passes. Rust’s conformance fixture starts with an unmarked
home, which the layout gate rejects. With a current marker it claims the lease and returns 404 at
`POST kuvo/computers/add/code`, owned by 2.3. The enrollment scenario still gates the Done marker.

### Side-by-side home inventory

The same signed, offline layout-23 seed gives equal replies for the kept session’s location,
waiting counts, device roster and unauthorized pairing. The kept announcement, list and both
reach files compare equal. A live probe writes each process’s own timestamp to the same reach
paths. An interrupted atomic replace can leave a `run/.kuvo-write-*` scratch file.

- `run/outbox/` and `run/outbox/sending/` are empty in the TypeScript run. Rust creates the storage
  when a writer needs it. The TypeScript scanner in `box/src/link/letters/outbox.ts` treats a
  missing directory as an empty list. Searches for these paths and `.kuvo-write-` under
  `gpui/`, `box/src/keeper/`, `box/src/link/{add,updates}/` and `scripts/` found no runtime readers.
- The TypeScript run writes the legacy `run/agents.json` export. It has real readers in
  `box/src/link/peers/known.ts` and `box/src/agents/main.ts`. Its remaining export and older-roster
  behaviour are listed under 2.6 in the map.
- Setup startup owns the differences in `run/link.json`, `run/setup-card.json` and its startup
  markers. The keeper and browser check `run/link.needed`. The legacy Mac reads `run/link.seen`
  and the link version marker serves the previous launcher. The browser reads the independently
  generated `run/socket.key`. The local engine list, IPC and vault endpoints, and service
  startup for notices, usage, Library and Work Queue also differ.
- The wider comparison of
  `GET kuvo/computers` still shows differences in full feature lists, local engine and agents facts,
  and setup and SSH actions. The peer conformance checks assert the same reach and owner fields
  through the live route on both builds.

The Rust resident validates computer facts once when it reads or accepts an announcement, then
projects typed values from its watch view. It shares the same working-session calculation between
the route and announcements. The persisted empty work snapshot deliberately keeps its timestamp
so a delayed older snapshot cannot restore rows after a clear. TypeScript deletes the cleared file
in `box/src/link/work/elsewhere.ts` and can accept that older snapshot afterward.
