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
| B14 | A stopped member opens with a retired standby field on disk and neither running route advertises it |
| B15 | A stale choice refreshes and a current resolution commits while retaining an independent edit |
| B16 | A member jumps directly over two missed epochs and restores 257 archived outcomes across two signed pages |
| B17 | A third member opens and receives the main's signed roster and device documents from the remaining member |
| B18 | A standalone library meets an oversized copy, filters malformed IDs and retains the newest 500 removals |
| B19 | An external file update reaches the library stream and a future format refuses writes across restart |
| B20 | A machine carries the kit, wake or launch zone in that order, follows kit updates, and keeps an accepted pick |
| B21 | A member projects transmitted zone rules and warns and removes the old rule file when a later signed save has no rules |
| B22 | A machine writes the version token and the empty, rate-limited activity marker in their legacy formats |
| D1 | A valid digest moves a lost signed-document counter forward and a digest past the TS cap is refused |
| D2 | Equal signed counts converge by digest in either arrival order, and a same-epoch document from an alternate main is rejected |
| C1 | Both residents offer the same Bun Intl clock choices and compute the reference revision |

The B cases run in [live.test.mjs](../../box/crates/kuvo_setup/tests/live.test.mjs).
R1–R4 run in [runtime.rs](../../box/crates/kuvo_setup/tests/runtime.rs). R5 runs
through two setup runtimes and two peer residents in [peers.rs](../../box/crates/kuvo-box/tests/peers.rs).
The W cases are in [wire vectors](../../box/crates/kuvo_wire/tests/vectors.rs). D1 and D2 exercise [the signed-document reader](../../box/crates/kuvo_setup/src/document.rs). C1 is in [clock.test.mjs](../../protocol/conformance/clock.test.mjs).

## TypeScript scenario map

Counts below are test declarations. Looped cases and nested subtests are included
under their parent declarations. A partial mapping names the Rust behavior
covered and the remaining integration or assertion. The reference inventory
contains 222 declarations across the listed files, including adjacent work.

| Reference | Count | Rust coverage or gap | Owner phase |
|---|---:|---|---|
| [setup-shared](../../box/test/setup-shared.test.ts) | 5 | SSH reservation → R2. Bootstrap → B1 and R1. Label restore preview belongs to the queue and restore ports. One-hop epoch catch-up → B2 for one missed epoch. Outcome pages → B2 for one page. B16 follows two missed epochs and checks both archive pages. | 2.5 and 5.2 label restore. |
| [setup-one-model](../../box/test/setup-one-model.test.ts) | 3 | Both predecessor conversion tests map to intentional refusal in R3. B14 checks that the route omits both the retired seat and the retired role. | 2.6 predecessor conversion. |
| [setup-merge](../../box/test/setup-merge.test.ts) | 7 | Capacity → B11 covers refusal and draft recovery. Reserved merge-denial capacity and source-bound receipts are separate. Merged setup admission, number aliases and Markdown changes belong to 2.3 and 5.2. | 2.3 merge admission, denials and source receipts. 5.2 numbering and import. |
| [setup-lists](../../box/test/setup-lists.test.ts) | 31 | Reach, list merging, and record routing are 2.2 or their area owners. | 2.2 reach and lists. 2.4 record routing. |
| [offline-merge](../../box/test/offline-merge.test.ts) | 10 | Revision and same-field clash → B2. B15 covers a stale resolution and accepting a proposal at the current revision. Text passages, attachment records, queue deletion and handoff are owned by their respective record areas. | 2.4 handoff. 3.2 session edits. 5.2 queue deletion. 5.5 attachments. |
| [time-zone unit scenarios](../../box/test/time-zone.test.ts) | 12 | Own clock, clock choice and wake-up files → B4. B20 covers carried inputs and kit updates. B21 covers rules and their absence in accepted saves. Legacy count ties and rule-mail fallback are pre-accepted-saves paths. | 7.3 legacy zone mail. 5.1 wake consumers. 5.3 group consumers. |
| [wire-vectors](../../box/test/wire-vectors.test.ts) | 1 | W1 and the existing Rust peer-mail vectors cover the shared input and bytes. | Covered. |
| [work-queue-numbering](../../box/test/work-queue-numbering.test.ts) | 6 | The lease client carries the saved high mark and reserves renewal headroom. The allocator, grant replays, queue mutations and Markdown import are 5.2. | 5.2 allocation, replay and import. |
| [setup-two-boxes](../../box/bridge-tests/setup-two-boxes.test.js) | 3 | Common roster → B2. Handover → B3. Cards → R1. SSH enrollment, member device refusals, names saved through the computer management route, and queue numbering span 2.2, 2.3 and 5.2. | 2.2 device policy. 2.3 enrollment and computer management. 5.2 numbering. |
| [documents](../../box/bridge-tests/documents.test.js) | 20 | Vector validation and part corruption → W1 and B8. Current bootstrap, following the main and stale authority → B1, B2, B6. Lost-mail catch-up → B7. Personality removal → B5. The old Mac-ranked, overlapping-setup, and credential re-enrollment cases require their compatibility or later setup paths. B17 covers third-computer forwarding. D1 and D2 cover count recovery and limits, equal-count ordering, and rejection of an alternate main at the same epoch. Phone denials and some live fault cuts remain. | 2.1 live fault cuts. 2.2 device denials. 2.3 re-enrollment and merged setups. 2.6 old authority. 7.3 old Mac mail. |
| [devices-two-boxes](../../box/bridge-tests/devices-two-boxes.test.js) | 7 | Accepted membership and its replica → B2 and B7. Signed device pairing and removal → R4 and R5 at the managed adapter. Device admission routes, device sessions and the reconnect phone/membership exchange are 2.2. Removal with incoming devices switched off and uninstall are 2.3. B17 checks signed roster and device document catch-up through another member. | 2.2 admission, device sessions, grants, paired reconnect and inbound switch. 2.3 uninstall. |
| [personalities-box](../../box/bridge-tests/personalities-box.test.js) | 10 | Standalone save and removal-wins merge → B1 and B5. Route conformance covers missing fields and preset IDs. B18 covers oversized merge, 500 removals and ID filtering. B19 covers stream changes from an external writer and future-format refusal. | 2.6 app-folder libraries. 5.1 retelling bots. |
| [time-zone bridge scenarios](../../box/bridge-tests/time-zone.test.js) | 3 | Clock settings and wake-up projection → B4. B20 covers kit and launch fallback. B21 checks that a member with a zone absent from its database projects received rules and warns when rules are unavailable. | 5.1 wake scheduling. |
| [setup-mac](../../box/bridge-tests/setup-mac.test.js) | 7 | Bootstrap and restart with Helper keys → B12. The other cases cover the retired rank rule, key rotation, SSH re-enrollment or following independent writers of link state under the same lock. | 1.2 cross-process link integration. 2.3 key rotation and re-enrollment. 2.6 retired rank and 9.3 released Mac rehearsal. |
| [setup-judge](../../box/bridge-tests/setup-judge.test.js) | 11 | Card output → R1 and R2. Multi-install discovery, probe judgments, attach action checks and future cards need their 2.3 cases. | 2.3 discovery, attach probing and future cards. |
| [owner-mutations](../../box/bridge-tests/owner-mutations.test.js) | 18 | Current main, offline exception, holder replay and capacity → B1, B6, B10, B11. Remaining cases concern queue, session, handoff and receipt index ownership or need live grant rotation across two peers. | 2.2 live grants. 2.4 handoff and receipt index. 3.2 sessions. 5.2 queue. |
| [setup-handover](../../box/bridge-tests/setup-handover.test.js) | 68 | Split out below. | See handover rows. |

| Handover cases | Count | Rust coverage or gap | Owner phase |
|---|---:|---|---|
| [Move, away owner, local work, owner reads](../../box/bridge-tests/setup-handover.test.js#L520-L837) | 5 | 2.2, 2.4, queue and engine. | 2.2 lists. 2.4 moves and owner reads. 3.2 session work. 5.2 queue work. |
| [Mixed rollback, multiple handovers, SSH epoch](../../box/bridge-tests/setup-handover.test.js#L838-L948) | 3 | B2 covers one missed epoch without unlink. B16 covers two successive handovers. Pinned SSH catch-up and historical rollback combinations remain open. | 2.3 SSH enrollment and unlink. 9.3 rollback. |
| [Successor and primary loss during handover](../../box/bridge-tests/setup-handover.test.js#L949-L1013) | 3 | B3 and B6 cover the healthy readiness exchange and lease fencing. The three exact transfer cuts are open. | 2.1 all three transfer cuts. |
| [Credential cleanup, enroll/removal and access forwarding](../../box/bridge-tests/setup-handover.test.js#L1014-L1428) | 8 | 2.2 and 2.3. Scoped rename receipts and permission grants need their own route scenarios. | 2.2 access and forwarded grants. 2.3 credential cleanup, enroll, removal and scoped rename. |
| [SSH personalities, label restores](../../box/bridge-tests/setup-handover.test.js#L1429-L1717) | 4 | B5 covers the library merge. SSH command saves, the label restore command and its old or unreachable main checks remain open. | 2.1 SSH personality writer. 2.5 and 5.2 label restore. |
| [Commit before publication and number blocks](../../box/bridge-tests/setup-handover.test.js#L1718-L1812) | 2 | B7 exercises durable save and catch-up. The exact post-commit loss cut, peer pairing and number-block allocation still need checks. | 2.1 commit before publication. 2.2 pairing. 5.2 number blocks. |
| [Paired queued grants, drafts, notes and deletions](../../box/bridge-tests/setup-handover.test.js#L1813-L2273) | 4 | B2 covers the preference field decision. Queues, notes, attachment handling and live device changes need the respective area ports. | 2.2 device grants. 3.2 drafts and notes. 5.2 queue deletion. 5.5 attachments. |
| [Preference merge through takeover](../../box/bridge-tests/setup-handover.test.js#L2274-L2304) | 1 | B2. | Covered. |
| [Bot ownership, unlink, wipe, restore and mixed release](../../box/bridge-tests/setup-handover.test.js#L2305-L4139) | 29 | 2.3–2.5, engine/work, and rehearsal. | 2.3 unlink/wipe. 2.4 ownership. 2.5 backup/restore. 3.2 sessions. 5.1 bots. 9.3 mixed release. |
| [Lease renewal, fencing and sole-main changes](../../box/bridge-tests/setup-handover.test.js#L4140-L4263) | 6 | Restart grace and a partitioned main → B13 and B6. The offline pairing, installer and unlink/number cases need 2.2, 2.3 and 5.2. | 2.2 offline pairing. 2.3 installer and unlink. 5.2 number allocation. |
| [Replica catch-up and mixed resident handover](../../box/bridge-tests/setup-handover.test.js#L4264-L4333) | 2 | Replica catch-up → B7. The mixed TypeScript resident case belongs to the historical rehearsal. | 9.3 mixed resident. |
| [Unlink after handover to an older release](../../box/bridge-tests/setup-handover.test.js#L4334) | 1 | Handover → B3. Unlink and the historical binary combination remain separate. | 2.3 unlink. 9.3 historical binary. |

### Bridge scenario details

| Reference scenario | Rust scenario or open assertion | Owner phase |
|---|---|---|
| [Two boxes, agreed roster and cards](../../box/bridge-tests/setup-two-boxes.test.js#L216) | B2, R1. SSH enrollment and number allocation remain open. | 2.3 SSH. 5.2 numbering. |
| [Member restrictions and phone's list](../../box/bridge-tests/setup-two-boxes.test.js#L251) | B2 supplies accepted membership. Phone admission, adding and the phone's projection remain open. | 2.2 devices. 2.3 adding. |
| [Mac hands over to a machine](../../box/bridge-tests/setup-two-boxes.test.js#L290) | B3 checks handover. The Mac/device combination remains open. | 2.2 device policy. 9.3 released Mac combination. |
| [Document vectors](../../box/bridge-tests/documents.test.js#L127) | W1. | Covered. |
| [A third computer forwards peers](../../box/bridge-tests/documents.test.js#L142) | B17. | Covered. |
| [Forged, altered or replayed peers](../../box/bridge-tests/documents.test.js#L165) | W1 and B8 cover signature and checkpoint mutation. The peer-list replay sequence remains open. | 2.1. |
| [Different terms and two mains at one epoch](../../box/bridge-tests/documents.test.js#L194) | B6 covers stale authority and D2 rejects a wrong signer at the current epoch. | 7.3 legacy Mac rosters. |
| [A phone stays with its signed Mac](../../box/bridge-tests/documents.test.js#L241) | Legacy Mac associations and phone projection remain open. | 7.3 Mac associations. 2.2 phone projection. |
| [Main removes a machine and it starts separately](../../box/bridge-tests/documents.test.js#L273) | 2.3 unlink. | 2.3. |
| [A Mac enrolls again with a new signer](../../box/bridge-tests/documents.test.js#L285) | 2.3 re-enrollment. | 2.3. |
| [Two Mac rosters remain until one releases a machine](../../box/bridge-tests/documents.test.js#L300) | Legacy overlapping authority and release remain open. | 2.6. |
| [Older Mac coexists with a newer Mac's setup](../../box/bridge-tests/documents.test.js#L317) | Historical interop remains open. | 7.3. |
| [Two setups meet and choose by numbering source](../../box/bridge-tests/documents.test.js#L327) | Historical authority selection remains open. | 2.6. |
| [A setup that was set aside follows a main again](../../box/bridge-tests/documents.test.js#L339) | Legacy set-aside conversion remains open. | 2.6. |
| [Automatic main, joining by Mac and losing its setup](../../box/bridge-tests/documents.test.js#L361) | B1 covers automatic main. The join and detach sequence belongs to 2.3. | 2.3. |
| [Exchange with skewed clocks](../../box/bridge-tests/documents.test.js#L385) | D2 covers ordering by signed counter and digest regardless of the timestamps in the documents. | Covered. |
| [Removal wins over a later add](../../box/bridge-tests/documents.test.js#L432) | B5 covers personalities. R4 covers a removed desktop's return; the competing peer phone document remains open. | 2.2 phone denial. |
| [Lost counter learns newer versions](../../box/bridge-tests/documents.test.js#L472) | D1. | Covered. |
| [One missed mail with both computers connected](../../box/bridge-tests/documents.test.js#L497) | B7 exercises catch-up after reconnect. The same-connection drop remains open. | 2.1. |
| [Large document with an altered part](../../box/bridge-tests/documents.test.js#L516) | W1 and B8 cover corruption. A large library over the live relay remains open. | 2.1. |
| [Independent edits converge in either order](../../box/bridge-tests/documents.test.js#L548) | B2 covers independent and conflicting preference changes. Reverse arrival and other records remain open. | 2.1 reversal. 3.2 session records. 5.2 queue records. |
| [Only signed `left` plus absence releases a machine](../../box/bridge-tests/documents.test.js#L570) | 2.3 accepted removal and unlink. | 2.3. |
| [Recovery rejects an implausibly high counter](../../box/bridge-tests/documents.test.js#L585) | D1. | Covered. |
| [Desktop pairs through a member and is removed](../../box/bridge-tests/devices-two-boxes.test.js#L297) | R4 and R5 cover the signed writer and replica. The managed routes and session access are the 2.2 follow-up. | 2.2. |
| [A relay refuses unsupported route versions](../../box/bridge-tests/devices-two-boxes.test.js#L416) | 1.4/2.2 paired transport. | 1.4 and 2.2. |
| [Removal with the main's incoming-device switch off](../../box/bridge-tests/devices-two-boxes.test.js#L439) | R5 covers revocation. The switch and route combination remains open. | 2.2. |
| [Reconnect returns membership and phones](../../box/bridge-tests/devices-two-boxes.test.js#L520) | B7 covers membership replica catch-up. Paired reconnect is the 2.2 follow-up. | 2.2. |
| [Reconnect catches up through another member](../../box/bridge-tests/devices-two-boxes.test.js#L543) | B17 checks the signed copies reaching the third computer. Paired session reattachment remains open. | 2.2. |
| [Uninstall refuses while a member is active](../../box/bridge-tests/devices-two-boxes.test.js#L701) | 2.3 uninstall. | 2.3. |
| [Uninstall revokes its desktop before stopping](../../box/bridge-tests/devices-two-boxes.test.js#L739) | R4 covers durable signed revocation. Uninstall coordination belongs to 2.3. | 2.3. |

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
  The clock uses the 445 IDs offered by the TypeScript box on Bun 1.4.2.
  To refresh the vector after a reference runtime change, run the command
  below with the replacement Bun and run the clock conformance case on
  both boxes. Jiff supplies offsets and rules for the same IDs.

  ```sh
  bun -e 'console.log([...new Set([...Intl.supportedValuesOf("timeZone"), "UTC"])].sort().join("\n"))' > box/crates/kuvo_setup/data/time-zones.txt
  ```

## Startup file audit

The comparison uses stopped copies of a signed TypeScript home. The search
covered the desktop, released and current link sources, scripts, and the Rust
resident. Reproduce the reader search with
`git grep -n -E 'link[.](seen|version)|SEEN_FILE|VERSION_FILE|PROCESS_RECORD|setup-card[.]json|socket[.]key|keyNow' -- gpui scripts box/src box/crates`
and `git grep -n -F 'run/link.seen' v0.7.2 -- '*.swift'`.
The file paths in the table also serve as search terms. Some writers split
paths into components, including `RUN`, `BOX`, `BOTS` and `WORK_FOLDER`.

| Differing paths or fields | Readers and result | Owner phase |
|---|---|---|
| `run/link.seen` | Released `RemoteEngine.swift` uses its mtime to leave an engine running on quit. Rust writes an empty marker on authenticated non-app traffic with the same five-minute throttle. | Covered in B22 for the resident. 7.3 and 9.3 for the released reader drill. |
| `run/link.version` | `link/areas/info.ts` reports it and the legacy setup command compares it with the running link's own build. The move's `PROCESS_RECORD` excludes it. Rust writes its resident build as plain UTF-8 without a newline. Build identifiers differ. | Covered in B22 for the writer. 7.1 for selector and rollback. |
| `run/link.json` relay, `keyNow`, `own` and `sharedOutcomes` | `setup/lead.ts`, `main.ts` and `documents/work.ts` read these fields. Both preserve an existing relay. The startup fallback accepts ws/wss and strips a terminal slash. Rust carries the current static key and keeps the prior eight, publishes signed counters on connection, and treats absent outcomes as empty. Installed link identity changes and caller ordering remain covered by the respective integration phases. | 2.3 rotation and installer. 9.3 mixed startup order. |
| `run/setup-card.json` | `add/scripts.ts` prints it and `setup/judge.ts` reads it. Rust writes the versioned card at startup. The TS bench can finish before its periodic write. Active queue item and live usage inputs depend on their owner. | 2.3 discovery. 5.2 queue and card input. |
| `run/socket.key`, `run/route/ssh`, `run/ssh-for`, `run/vault.sock` | `server/ssh-pipe.ts` and the Rust server use the same local secret encoding and addressed socket. Independently created secrets vary. Rust closes its socket on shutdown. Vault and SSH session directory state are read by `vault/ssh.ts` and `vault/holder.ts`. | 1.4 local transport. 6.2 vault and session directory. |
| `run/lists/sessions.json`, `run/notices/teller`, `run/outbox/sending`, `run/moves` | Reader in `notices/told.ts` checks the live writer PID; peer list and move code read the session and move records. A move preserves durable records. Empty runtime directories have no desktop, installer or updater reader in the search. | 2.2 lists and notices. 2.4 moves. 1.5 letter interoperability. |
| `agents/limits.json`, `agents/usage/*`, `run/usage-attribution.json`, `run/agents.json` | Agents and limits/Insights read these records, including `agents/box-services.ts` for attribution. The move excludes live attribution. | 4.1 roster. 5.4 usage. |
| `run/glance/asks.json`, `run/bots-seen`, `run/work/background.json`, `run/work/told.json` | `glance/asked.ts`, `bots/seen.ts`, `at-work.ts` and `work/wakes.ts` read them. | 3.2 asks. 5.1 bots and work. |
| `run/github-sign-in`, `run/queues-scanned.json`, `run/work-queue/forward` | `services.ts`, `engine/queue.ts` and `work-queue/forward.ts` own them. Empty scratch directories have no desktop or installer reader. | 5.2 queue and GitHub. |
| `artifacts/*`, `library/backfill.json`, `box/marketplace/mcp-relay-v1.json` | `artifacts/prepare.ts`, `library/service.ts` and `marketplace/connections.ts` read or create them. | 5.5 services. |
| `config/opencode/plugins/kuvo-defaults.js`, `run/fence/agents.md`, `box/approval.json`, `chats` | Engine boot, fence and approval code read their own inputs. Backup and move also interpret approval and the chats root. GPUI discovers the resident through `run/serve.json`; it gets the chats root from `kuvo/info`. | 3.1 plugin. 5.1 approval. 6.1 fence. 2.5 and 2.6 backup and migration. |
