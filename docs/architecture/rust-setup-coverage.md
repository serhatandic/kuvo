# Rust setup coverage

Phase 2.1 owns the current relay-lease setup model. The resident owns the write
order, the lease observation and the live relay mail loop. It publishes a read
snapshot to the HTTP and addressed SSH servers. The machine startup, lease
journal, link state, checkpoint files, shared revisions, edit receipts, setup
card, personality library and time-zone records use their existing locations.

The two-computer scenarios start real Rust residents on loopback. The initial
membership fixture supplies the accepted roster and signed enrollment position
on stopped residents. Both residents then negotiate with the relay stand-in
through HTTP and exchange authenticated Noise mail over WebSockets. They
continue through an actual resident stop, lease claim, signed epoch change,
replica catch-up and versioned journal reads. Running the fixture does not
require a deployed relay, a helper sign-in or the install command.

## Rust scenarios

| ID | Scenario |
|---|---|
| B1 | A sole computer starts with a signed epoch, saves offline and restarts with its saved personality |
| B2 | A member's independent and conflicting edits commit at the main, replicate, survive promotion with the archived decisions and reach the former main |
| B3 | A member asks for handover and the main checks the successor before transferring the lease |
| B4 | The clock route rejects stale writes and keeps the accepted zone in wake-up files |
| B5 | A personality tombstone wins against a more recent copy |
| B6 | A cut-off main fences writes and follows the winning member when its relay path recovers |
| B7 | A restarted member catches up and a repeated edit returns its original decision |
| B8 | A corrupt checkpoint leaves the saved authority unchanged and holds startup |
| B9 | An unknown link format holds startup and preserves the bytes |
| B10 | A holder retry returns its original receipt and rejects a changed value at the same operation ID |
| B11 | A save over capacity leaves the draft recoverable with a terminal receipt |
| B12 | The Mac host proof precedes the authenticated key request, and restarting keeps the setup with its keys in the helper |
| B13 | Restart grace lets the main renew an expired lease at the same term |
| R1 | Reopening a machine restores its certified identity, public setup card and addressed SSH identity |
| R2 | The SSH reservation prevents an independent setup |
| R3 | Startup refuses a predecessor setup that has not converted to the current lease model |
| R4 | The main signs exact device grant and revocation generations, fences revoked callers and replays the original receipt |
| R5 | A member forwards signed device pairing and revocation to the leased main, verifies the bound receipt and applies the accepted replica |
| W1 | The existing Rust wire vectors verify signed documents, altered parts and canonical digests against TypeScript fixtures |

The B cases run in [live.test.mjs](../../box/crates/kuvo_setup/tests/live.test.mjs).
R1–R4 run in [runtime.rs](../../box/crates/kuvo_setup/tests/runtime.rs). R5 runs
through two setup runtimes and two peer residents in [peers.rs](../../box/crates/kuvo-box/tests/peers.rs).
The W cases are in [wire vectors](../../box/crates/kuvo_wire/tests/vectors.rs).

## TypeScript scenario map

Counts below are test declarations. Looped cases and nested subtests are included
under their parent declarations. A partial mapping names the Rust behavior
covered and the remaining integration or assertion. The reference inventory
contains 222 declarations across the listed files, including adjacent work.

| Reference | Count | Rust coverage or gap |
|---|---:|---|
| [setup-shared](../../box/test/setup-shared.test.ts) | 5 | SSH reservation → R2. Bootstrap → B1 and R1. Label restore preview belongs to the queue and restore ports. One-hop epoch catch-up → B2 for one missed epoch. Outcome pages → B2 for one page. More than one missed epoch and a multi-page archive still need tests. |
| [setup-one-model](../../box/test/setup-one-model.test.ts) | 3 | Both predecessor conversion tests map to intentional refusal in R3. Standby absence is implicit in the Rust setup routes. A standalone route assertion for it remains open. |
| [setup-merge](../../box/test/setup-merge.test.ts) | 7 | Capacity → B11 covers refusal and draft recovery. Reserved merge-denial capacity and source-bound receipts are separate. Merged setup admission, number aliases and Markdown changes belong to 2.3 and 5.2. |
| [setup-lists](../../box/test/setup-lists.test.ts) | 31 | Reach, list merging, and record routing are 2.2 or their area owners. |
| [offline-merge](../../box/test/offline-merge.test.ts) | 10 | Revision and same-field clash → B2. The explicit resolution and stale-choice paths still need route checks. Text passages, attachment records, queue deletion and handoff are owned by their respective record areas. |
| [time-zone unit scenarios](../../box/test/time-zone.test.ts) | 12 | Own clock, clock choice and wake-up files → B4. Separate legacy carried-zone sources, kit-change follow, count ties, rule mail and its fallback, and group/engine consumers still need coverage. |
| [wire-vectors](../../box/test/wire-vectors.test.ts) | 1 | W1 and the existing Rust peer-mail vectors cover the shared input and bytes. |
| [work-queue-numbering](../../box/test/work-queue-numbering.test.ts) | 6 | The lease client carries the saved high mark and reserves renewal headroom. The allocator, grant replays, queue mutations and Markdown import are 5.2. |
| [setup-two-boxes](../../box/bridge-tests/setup-two-boxes.test.js) | 3 | Common roster → B2. Handover → B3. Cards → R1. SSH enrollment, member device refusals, names saved through the computer management route, and queue numbering span 2.2, 2.3 and 5.2. |
| [documents](../../box/bridge-tests/documents.test.js) | 20 | Vector validation and part corruption → W1 and B8. Current bootstrap, following the main and stale authority → B1, B2, B6. Lost-mail catch-up → B7. Personality removal → B5. The old Mac-ranked, overlapping-setup, and credential re-enrollment cases require their compatibility or later setup paths. Third-computer forwarding, counter recovery and its cap, phone denials and equal-revision document ordering still need scenarios. |
| [devices-two-boxes](../../box/bridge-tests/devices-two-boxes.test.js) | 7 | Accepted membership and its replica → B2 and B7. Signed device pairing and removal → R4 and R5 at the managed adapter. The managed routes, Noise admission, signed reconnect exports and catch-up through another member are covered by [Managed](rust-peers-coverage.md#typescript-scenario-map). Removal with incoming devices switched off and uninstall are 2.3. |
| [personalities-box](../../box/bridge-tests/personalities-box.test.js) | 10 | Standalone save and removal-wins merge → B1 and B5. Route conformance covers missing fields and preset IDs. Mac app-folder libraries, oversized merge, 500 tombstones, ID filtering, retelling bots, stream changes from external writers and explicit future-format refusal still need corresponding scenarios. |
| [time-zone bridge scenarios](../../box/bridge-tests/time-zone.test.js) | 3 | Clock settings and wake-up projection → B4. Legacy kit/launch fallback, no-tzdata peer rules and actual wake scheduling remain open. |
| [setup-mac](../../box/bridge-tests/setup-mac.test.js) | 7 | Bootstrap and restart with Helper keys → B12. The other cases cover the retired rank rule, key rotation, SSH re-enrollment or following writes from an external process. |
| [setup-judge](../../box/bridge-tests/setup-judge.test.js) | 11 | Card output → R1 and R2. Multi-install discovery, probe judgments, attach action checks and future cards need their 2.3 cases. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) | 18 | Current main, offline exception, holder replay and capacity → B1, B6, B10, B11. Remaining cases concern queue, session, handoff and receipt index ownership or need live grant rotation across two peers. |
| Setup handover bridge files | 68 | Scenario links below. |

| Handover cases | Count | Rust coverage or gap |
|---|---:|---|
| [Move, away owner, local work, owner reads](../../box/bridge-tests/setup-handover.test.js#L61) | 5 | 2.2, 2.4, queue and engine. |
| [Mixed rollback, multiple handovers, SSH epoch](../../box/bridge-tests/setup-handover.test.js#L342) | 3 | B2 covers one missed epoch without unlink. Pinned SSH catch-up, two successive handovers and historical rollback combinations remain open. |
| [Successor and primary loss during handover](../../box/bridge-tests/setup-handover.test.js#L453) | 3 | B3 and B6 cover the healthy readiness exchange and lease fencing. The three exact transfer cuts are open. |
| [Credential cleanup, enroll/removal and access forwarding](../../box/bridge-tests/setup-handover-access.test.js) | 8 | 2.2 and 2.3. Scoped rename receipts and permission grants need their own route scenarios. |
| [SSH personalities, label restores](../../box/bridge-tests/setup-handover-shared.test.js#L17) | 4 | B5 covers the library merge. SSH command saves, the label restore command and its old or unreachable main checks remain open. |
| [Commit before publication and number blocks](../../box/bridge-tests/setup-handover-shared.test.js#L303) | 2 | B7 exercises durable save and catch-up. The exact post-commit loss cut, peer pairing and number-block allocation still need checks. |
| [Paired queued grants, drafts, notes and deletions](../../box/bridge-tests/setup-handover-shared.test.js#L383) | 4 | B2 covers the preference field decision. Queues, notes, attachment handling and live device changes need the respective area ports. |
| [Preference merge through takeover](../../box/bridge-tests/setup-handover-shared.test.js#L848) | 1 | B2. |
| [Bot ownership](../../box/bridge-tests/setup-handover-shared.test.js#L879), [unlink](../../box/bridge-tests/setup-handover-removal.test.js), [export](../../box/bridge-tests/setup-handover-export.test.js), [wipe](../../box/bridge-tests/setup-handover-wipe.test.js), [restore and mixed release](../../box/bridge-tests/setup-handover-recovery.test.js) | 29 | 2.3–2.5, engine/work, and rehearsal. |
| [Lease renewal, fencing and sole-main changes](../../box/bridge-tests/setup-handover-lease.test.js#L31) | 6 | Restart grace and a partitioned main → B13 and B6. The offline pairing, installer and unlink/number cases need 2.2, 2.3 and 5.2. |
| [Replica catch-up and mixed resident handover](../../box/bridge-tests/setup-handover-lease.test.js#L155) | 2 | Replica catch-up → B7. The mixed TypeScript resident case belongs to the historical rehearsal. |
| [Unlink after handover to an older release](../../box/bridge-tests/setup-handover-lease.test.js#L225) | 1 | Handover → B3. Unlink and the historical binary combination remain separate. |

### Bridge scenario details

| Reference scenario | Rust scenario or open assertion |
|---|---|
| [Two boxes, agreed roster and cards](../../box/bridge-tests/setup-two-boxes.test.js#L216) | B2, R1. SSH enrollment and number allocation remain open. |
| [Member restrictions and phone's list](../../box/bridge-tests/setup-two-boxes.test.js#L251) | B2 supplies accepted membership. Phone admission, adding and the phone's projection remain open. |
| [Mac hands over to a machine](../../box/bridge-tests/setup-two-boxes.test.js#L290) | B3 checks handover. The Mac/device combination remains open. |
| [Document vectors](../../box/bridge-tests/documents.test.js#L127) | W1. |
| [A third computer forwards peers](../../box/bridge-tests/documents.test.js#L142) | Third-computer document forwarding remains open. |
| [Forged, altered or replayed peers](../../box/bridge-tests/documents.test.js#L165) | W1 and B8 cover signature and checkpoint mutation. The peer-list replay sequence remains open. |
| [Different terms and two mains at one epoch](../../box/bridge-tests/documents.test.js#L194) | B6 covers stale authority. Equal-epoch injection and two Mac rosters remain open. |
| [A phone stays with its signed Mac](../../box/bridge-tests/documents.test.js#L241) | Legacy Mac associations and phone projection remain open. |
| [Main removes a machine and it starts separately](../../box/bridge-tests/documents.test.js#L273) | 2.3 unlink. |
| [A Mac enrolls again with a new signer](../../box/bridge-tests/documents.test.js#L285) | 2.3 re-enrollment. |
| [Two Mac rosters remain until one releases a machine](../../box/bridge-tests/documents.test.js#L300) | Legacy overlapping authority and release remain open. |
| [Older Mac coexists with a newer Mac's setup](../../box/bridge-tests/documents.test.js#L317) | Historical interop remains open. |
| [Two setups meet and choose by numbering source](../../box/bridge-tests/documents.test.js#L327) | Historical authority selection remains open. |
| [A setup that was set aside follows a main again](../../box/bridge-tests/documents.test.js#L339) | Legacy set-aside conversion remains open. |
| [Automatic main, joining by Mac and losing its setup](../../box/bridge-tests/documents.test.js#L361) | B1 covers automatic main. The join and detach sequence belongs to 2.3. |
| [Exchange with skewed clocks](../../box/bridge-tests/documents.test.js#L385) | Skew across signed document ordering remains open. |
| [Removal wins over a later add](../../box/bridge-tests/documents.test.js#L432) | B5 covers personalities. R4 covers a removed desktop's return; the competing peer phone document remains open. |
| [Lost counter learns newer versions](../../box/bridge-tests/documents.test.js#L472) | Counter recovery remains open. |
| [One missed mail with both computers connected](../../box/bridge-tests/documents.test.js#L497) | B7 exercises catch-up after reconnect. The same-connection drop remains open. |
| [Large document with an altered part](../../box/bridge-tests/documents.test.js#L516) | W1 and B8 cover corruption. A large library over the live relay remains open. |
| [Independent edits converge in either order](../../box/bridge-tests/documents.test.js#L548) | B2 covers independent and conflicting preference changes. Reverse arrival and other records remain open. |
| [Only signed `left` plus absence releases a machine](../../box/bridge-tests/documents.test.js#L570) | 2.3 accepted removal and unlink. |
| [Recovery rejects an implausibly high counter](../../box/bridge-tests/documents.test.js#L585) | The counter cap remains open. |
| [Desktop pairs through a member and is removed](../../box/bridge-tests/devices-two-boxes.test.js#L297) | R4 and R5 cover the signed writer and replica. Managed exercises the peer routes and Noise admission through a member. |
| [A relay refuses unsupported route versions](../../box/bridge-tests/devices-two-boxes.test.js#L416) | 1.4/2.2 paired transport. |
| [Removal with the main's incoming-device switch off](../../box/bridge-tests/devices-two-boxes.test.js#L439) | R5 covers revocation. The switch and route combination is 2.3. |
| [Reconnect returns membership and phones](../../box/bridge-tests/devices-two-boxes.test.js#L520) | Managed verifies both signed exports and the reconnect receipt. |
| [Reconnect catches up through another member](../../box/bridge-tests/devices-two-boxes.test.js#L543) | Managed verifies the returning member applies the original main-signed revocation with only another member online. |
| [Uninstall refuses while a member is active](../../box/bridge-tests/devices-two-boxes.test.js#L701) | 2.3 uninstall. |
| [Uninstall revokes its desktop before stopping](../../box/bridge-tests/devices-two-boxes.test.js#L739) | R4 covers durable signed revocation. Uninstall coordination belongs to 2.3. |

## Peer fixture changes

| Peer test | Previous fixture | Current fixture | Reason |
|---|---|---|---|
| `another_computers_session_is_in_the_first_state_after_restart_and_stays_visible_away` | Two links with the unsigned `set_test` roster | Two links with a signed checkpoint, leadership and both computers' signing keys | The resident takes its initial roster from accepted setup state. |
| `kept_activity_and_notices_keep_their_origin_and_dock_deduplicates_a_bot` | One link with an unsigned two-computer roster | One link with a signed two-computer roster and a configured local test relay | The kept activity still carries its accepted origin. |
| `a_restarting_release_keeps_its_session_actions_and_cannot_be_chosen_for_a_start` | Unsigned roster and an empty relay setting | Signed roster with the relay setting absent | The test needs accepted origins while keeping the other computer away. |
| `a_local_app_pairs_once_and_removal_survives_a_resident_restart` | A link in an unmarked temporary home | A link in a reserved standalone home | The reservation keeps the fixture on the standalone pairing path. |

## Structure and compatibility boundaries

- One Tokio task owns mutable setup state. Subscribers see a watch snapshot.
  Accepted changes publish through atomic file replacement, with a flushed temp
  file and a parent directory sync on Unix. The eventual shared store, the
  cross-process TypeScript link lock and Windows open-file replacement checks
  still need integration. Lease freshness uses the current process's monotonic
  clock as well as wall time. A restarted resident revalidates with the relay
  before using a saved observation for write authority.
- The new resident uses the existing signed envelope, lease request, Noise mail
  and shared replica formats. The handover readiness exchange uses typed
  request state in the resident and setup authority mail. It has no TypeScript
  forward-spool dependency. Old-model conversion stays with the TypeScript box
  and the migration boundary.
- `ResidentPeers` owns the relay and setup subscribes through its
  `RelayHandle`. `SetupHandle::change_devices` provides the signed managed
  device writer. Its primary and forwarded paths use the same durable
  membership transaction. Device routes and relay session grant integration
  are the 2.2 follow-up.
- The accepted edit history handles preference fields. Text passages, session
  notes, work inventory and operational moves remain the owning areas'
  responsibility. The local Mac helper adapter authenticates the host before
  proving an identity. A real signed Helper and installed Keychain rehearsal
  remains separate from the stand-in.
- The install-code conformance scenario still needs the 2.3 install route. The
  personalities route suite's complete lifecycle also calls the bot templates
  route, which belongs to the bots port. The two binaries can be checked
  against the same stopped, signed setup home before those cross-area scenarios
  are green.

## Conformance results

- TypeScript passes the two-computer scenario, both personality scenarios and
  the current layout-23 golden-home scenario. Rust passes the personality
  validation case. The other scenarios first stop at the installer-code, bot
  templates and project-list routes, respectively.
- TypeScript opens both tagged machine homes. The Rust run on the 0.7.2 home
  leaves its layout marker absent, and the 0.8.0-beta.1 run stops before the
  legacy peers document is projected. These checks still need migration and
  historical relay integration.
- The scoped Rust check of the current TypeScript golden home preserves its
  certified identity, leadership, accepted shared values and personality
  library bytes. The live Rust pair uses a signed enrollment fixture until the
  installer route is available. It confirms a member's save commits at the
  main, then survives the main's exit, lease expiry and member promotion.
- On the same stopped setup home the differential runner matches the setup
  view, personality list and a direct caller's personality-document refusal.
  The clock choice list differs because the bundled Jiff database
  lists IANA aliases and TypeScript uses the Intl zone catalog. The row
  revision hashes the offered IDs, so it differs too. The file inventory also
  differs in companion files and startup bookkeeping, including the relay
  setting and TypeScript's `run/link.seen` and `run/link.version` markers.
