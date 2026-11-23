# Rust move coverage

Phase 2.6 is complete. The resident passes the store's `MoveRequired` and
`ResumeMove` handoffs to `kuvo_move`. The machine path recognizes a predecessor
by its saved files, takes the store's layout lock, checks again under the lock,
and commits layout 23 after the last step. A current layout opens without
rerunning the imports. The machine path refuses an active old writer and an
unfinished TypeScript Mac move. Mac app discovery recognizes the released
app's folder and holds startup until cutover. `rehome --run` validates an
installer-staged release, prepares the old app's data, and publishes it under
the layout lock and the lock the signed key tools recognize.

The TypeScript machine reference takes data in through several entry points.
The Rust machine move runs those on-disk imports before starting the resident.
Settings copy the earlier global and private configuration and shared key files,
then take the saved branch prefix. The kit supplies the carried clock, projects
and custom personalities. The link projects the stored beta.1 peers list,
preserving the identity and other saved fields. Outbox mail moves by rename,
address-only browser pages acquire their session field, and obsolete agent
programs, copied sign-ins and Mac presence records are retired. The phases
replay from disk after a process exits. No intermediate layout version is
published. Mac staging rebuilds a private copy after an interruption. The
publication journal names the prepared files and reciprocal Git edits. Rust
checks file identities to recognize completed renames and compares the old
and new Git values when it replays. Both the layout and completed journal
must be durable before startup. Helper reads back the copied keys. On the
account's real home, originals remain until the new resident authenticates to
Helper. Rust uses typed steps, the store's RAII locks and atomic writes, a
private SQLite transaction for rewrites, and a scoped engine for validation.
If the source changes during the copy or placement, the mover rolls back any
publication and retries under the same lock, for up to three rounds. A saved
process lifetime guards each signal to a predecessor process. An unrelated
writer is named in the refusal and remains running.

## Rust scenarios

[Machine scenarios](../../box/crates/kuvo_move/tests/machine.rs) exercise the
real filesystem and the store handoff. [Mac scenarios](../../box/crates/kuvo_move/tests/mac_stage.rs)
exercise private staging and publication using the tagged engine and Keychain
fixtures. [Setup scenarios](../../box/crates/kuvo_setup/tests/live.test.mjs)
use authenticated Helper and relay stand-ins.

| ID | Scenario |
|---|---|
| M1 | An older machine adopts its settings, projects its kit and peers, carries queued mail and browser pages, and retires only the old runtime and presence files. A second open preserves an edited setting. |
| M2 | A newer layout, unsupported interrupted layout and unfinished TypeScript app move preserve the files and hold startup. |
| M3 | A fresh home opens at 23 and the released Mac app's data holds startup before any destination is written. |
| M4 | A child stopped by PID after each of eight durable machine boundaries restarts. The complete file and directory inventory compares with an uninterrupted move at the same path. |
| M5 | A machine that never had a bridge opens through the store's interrupted 0→1 handoff and keeps its instructions. |
| M6 | A live predecessor bridge keeps the move from retiring its files. |
| A1 | An older Mac's attachments, chats, links, and saved run records are cloned into private staging. Process records stay with the old app. Absolute links into the old app are rewritten. Relative links and external targets are preserved. |
| A2 | A binary plist projects projects, worktrees, Git and approval settings, and starred models. The project and worktree records name the final home. Integration key references in engine and bot config name the destination key directory. The relay and device preference, and personality additions and removals, also carry over. |
| A3 | A write in the source after cloning stops staging. A retry opens the changed input. |
| A4 | The staged database for each tagged Mac passes migration and session list checks against the fixture's pinned-engine stand-in. A child killed at each staging boundary rebuilds the same private tree. The source and published layout remain intact. |
| A5 | Staged artifact logs remap paths and path-derived IDs. A future-format line keeps the source and staged file byte for byte and refuses the move. |
| A6 | A future legacy Mac record refuses the import before any clone. |
| A7 | Each tagged app move survives 16 PID cuts. The final inventory, source, Git backlink, and Keychain stand-in match the uninterrupted run. A locked Keychain and failed read-back preserve the originals and prevent publication. A source write after the first placement rolls the move back and is carried on retry. |
| A8 | The command retries one source write in the same invocation and stops after three rounds of continued writes. A late placement collision rolls back and keeps the new file belonging to the other writer. A live status query reads the current staging step. An explicit resume finishes a published plan after a process cut. Imported group files reach the private import boundary before publication, and an external-history rename resumes by file identity. A shared-memory change and empty WAL appearing at the same checkpoint leave the attempt running. A cut during the displacement of a removed installation preserves its archive. |
| A9 | A future engine schema is detected on a private database copy before staging. A browser-only app gets its own home, and a staged Claude Code account keeps history under the moved chat's name. |
| A10 | A cut-short Mac installation holds startup until a new attempt finishes. The fresh release reaches layout 23 and the resident can then open it. |
| A11 | A linked global settings file is imported into a machine's own settings, with its original bytes and link intact. |
| A12 | A custom Claude Code account keeps its credential under its published configuration path. A read-back failure restores the previous credential and rolls back placement. A process cut after saving the previous credential, writing the new one, or retiring the original resumes through a disk-backed stand-in. Credentials never enter the move journal. |
| C2 | The six released-move command fixtures from `rehome-upgrades.test.ts` run against either executable. Projects, worktrees, the empty and populated machine list, a 0.6 bot, 0.7.2 settings and notices, group seats and messages, devices and relay settings, and beta.1 letters and personality removals are checked directly. The three TypeScript reader and private-staging tests in that file also pass. |
| C1 | The rehome conformance area runs the same released app fixture against both commands. It covers installer verdicts, unplaced and torn journals, file-only legacy placement recovery and retry receipts, missing and incompatible engines, a failed database migration, malformed preferences and newer app records, an existing machine home, archived leftovers from a removed installation, live writers and a read-only reader, older app discovery, the old keeper's control endpoint, a capped APFS volume, CLI status and validation, legacy bots and group chats, overlapping attachments, Usage and letters, staged and default-account history, and 255 active and archived sessions through the pinned engine. It checks the older settings omissions and their notice order, deferred copied-key settlement, sign-ins already recorded as imported, saved sign-in receipts and installer access during settlement, and the standalone move's Move Back reply. |
| S2 | The authenticated older Mac main converts its checkpoint after receiving the relay lease. A PID kill at lease claim and a restart advance the same setup by one epoch. |
| S1 | The stored-format conformance scenarios refuse newer, malformed and invalid layout records and reopen a fresh home at 23. |

## TypeScript scenario map

The primary inventory contains 135 test declarations. Each loop is counted at
its declaration. The two forward-move loops expand to eleven cuts, the rollback
loop to three, and the saved-sign-in unit loop to four outcomes. Nine original
declarations are out of scope under section 4. The switch decision drops
17 more declarations specific to the unpublished in-place move. The tables record the
remaining ownership and the evidence mapped to each declaration.
The associated compatibility checks below are tracked separately.

| Reference | Declarations | Rust coverage and remaining owner |
|---|---:|---|
| [rehome](../../box/test/rehome.test.ts) | 36 | M2 covers the startup fence and M4 the machine crash boundaries. A1–A4 cover private clones, source writes, and database and path rewrites. A7–A10 and C1 cover standalone publication, recovery, status and fresh making. The in-place move, its clearing, Move Back, kept-copy expiry and its scheduling are dropped: TS-beta only (user 11-23). The standalone installer and Helper handoff belong to 7.2. |
| [rehome-run](../../box/test/rehome-run.test.ts) | 35 | M2, M3 and M6 exercise machine admission. A4 checks the tagged database and pinned engine; A7–A9 and C1 cover the standalone move, retries, writer discovery, notices, CLI status, disk space, browser-only admission and history. C2 covers the complete released preferences fixtures. A12 covers the custom account credential step. The real Keychain interface belongs to 6.2 and Helper launch belongs to 7.2. |
| [rehome-upgrades](../../box/test/rehome-upgrades.test.ts) | 9 | M1 and A2–A6 cover the machine kit, preferences and format gates. C1 and C2 cover the released app projections and notices, including the earlier pending-letter format and a Unicode boundary case. |
| [rehome-passwords](../../box/test/rehome-passwords.test.ts) and [bridge scenario](../../box/bridge-tests/rehome-passwords.test.js) | 2 | A7 and C1 check the move's sign-in obligations, prior imports and settlement replies. The interactive saved-site transfer and vault verification belong to 6.2. |
| [install-verdict](../../box/test/install-verdict.test.ts) | 12 | M2 and M3 cover blocking an unfinished journal and recognizing app data beside cache and program folders. M5 covers in-place startup without a bridge. A10 and C1 check the read-only verdicts, the CLI's earlier refusal of an unreadable record, and a fresh making retry with a preserved archive. Helper readiness and installer restart come from 7.2. Reinstall and wipe are 2.3 and 7.2. |
| [install](../../box/bridge-tests/install.test.js) | 24 | A7–A10 and C1 cover the staged move, progress, standalone making and two-install admission. Installer release staging, failed launch settlement and keeper launch are 7.2. Wipe and reinstall admission are 2.3 and 7.2. |
| [folder-version](../../box/bridge-tests/folder-version.test.js) | 16 | S1 covers future and unreadable marker refusals. M2 and M5 cover the move's reader and valid 0→1 retry. A7 covers layout sealing for the tagged apps. Group-file sealing still needs its fixture. Steps 11 and 17 and the layout transitions only used by unpublished TypeScript betas are excluded by section 4. Bridge and keeper entry points and trial rollback need 4.1 and 7.1–7.2. |
| [update-from-before](../../box/bridge-tests/update-from-before.test.js) | 1 | An updater handing over to the new keeper and updating its engine and bridge belongs to 7.2 and 9.3. M5 only covers opening data once the new executable runs. |

The declaration lines below identify each scenario in the linked files above.
A scenario listed with a Rust ID still has the remaining work in its owner
column. The loop declarations cover each cut or outcome described by that loop.

| Reference | Declaration lines | Evidence and remaining owner |
|---|---|---|
| `rehome.test.ts` | 305, 422, 446, 463, 674, 698, 715, 742, 884, 910, 921, 972, 1112 | In-place move, clearing, rollback, Move Back and kept-copy expiry dropped: TS-beta only (user 11-23). A7–A8 and C1 cover equivalent released-path publication, interruption, isolated recovery and archived destination files. |
| `rehome.test.ts` | 625, 705, 835, 953 | Old phone move status and scheduled in-place move dropped: TS-beta only (user 11-23). A8 and C1 cover the released command's synchronous progress. |
| `rehome.test.ts` | 512, 543, 566, 582, 601, 644, 659 | A10 and C1 cover fresh making, collision admission, a copy failure, retry, and a repeated command. The installer and Helper handoff belong to 7.2. |
| `rehome.test.ts` | 800, 867, 942, 983, 999 | M2 and M6 fence a recognizable unfinished move and a running machine writer. C1 covers the torn journal, disk-space refusal, malformed root and released-app account isolation. Machines open in place and standalone Mac movement is refused on other platforms. |
| `rehome.test.ts` | 1007, 1032, 1045, 1060, 1098, 1124 | A3 and A4 cover concurrent changes and database migration and validation. A8 covers retry rounds and rollback on a new destination collision. A9 and C1 cover an unsupported schema at command preflight and the pinned engine's large paginated list. |
| `rehome.test.ts` | 1144 | The in-place invocation is dropped: TS-beta only (user 11-23). A12 covers the released move's equivalent credential handover and rollback. The real Helper interface belongs to 6.2. |
| `rehome-run.test.ts` | 20, 87, 110, 384, 461, 537, 573, 595, 608 | A1, A4 and A7 cover tagged migration, publication, path changes and interruption recovery. C1 checks the old keeper's loopback stop, empty folders, attachment collisions and the rewritten message URI. The released-path fixtures check the retired engine queue notice. The differential passes with the pinned engine. Helper startup belongs to 7.2. |
| `rehome-run.test.ts` | 128, 250, 262, 274, 282, 290, 325, 351 | A3 and M6 cover a changed source file and known live writer. A8 covers bounded retry and rollback and shared-memory and empty-WAL activity. C1 verifies the old engine and older app stop. The unrelated writer, external reader, chat program and another copy of the current app stay running. |
| `rehome-run.test.ts` | 159, 174, 198, 209, 395, 420, 435, 506 | A7 and the released command fixtures cover Keychain refusal, support log details, verification, old copies and rollback through stand-ins. A9 and C1 cover path-derived histories and recognizing prior website imports. A12 covers added Claude Code credential copy, rollback and resume. Real Keychain primitives and website vault operations belong to 6.2. Old launch agent cleanup and starting the real Helper belong to 7.2. |
| `rehome-run.test.ts` | 216, 224, 237, 371, 494, 553, 583, 623, 632, 643 | A2–A4 and M2–M3 cover input readers, the engine gate and startup fences. A8–A9 and C1 cover live status, the future engine schema, browser-only admission, missing engine, two existing installations, malformed preferences and the torn journal. |
| `rehome-upgrades.test.ts` | 77, 165, 177, 184, 193, 209 | C2 runs the full released-command fixture set through both executables. A2 covers the typed Rust projection of binary preferences. |
| `rehome-upgrades.test.ts` | 214 | The TypeScript staging folders list is private to its rewriter. Rust plans the corresponding history moves before publication and saves typed relocations, exercised in A8. M2 fences old journals at startup. |
| `rehome-upgrades.test.ts` | 251 | Covered by A6. |
| `rehome-upgrades.test.ts` | 235 | Covered by A5. |
| `rehome-passwords.test.ts`, `rehome-passwords.test.js` | 9 and 54, respectively | A7 and C1 check move obligations. Vault and Keychain operations belong to 6.2. |
| `install-verdict.test.ts` | 32, 74, 81, 89, 96, 111, 147, 154, 169 | M2, M3 and M5 exercise startup gates. C1 covers the fresh, moving, stuck, waits, app-07, app-box, back, machine and two verdicts, plus an interrupted new Mac home. A10 checks its restart. Installer launch and update handoff belong to 7.2. |
| `install-verdict.test.ts` | 45, 57, 128 | C1 covers the ours verdict, archived attempts and a concurrent sign-in or settlement worker. The live Helper readiness oracle, startup and updates belong to 7.2. |
| `install.test.js` | 124, 361, 379, 387, 418, 442, 451, 468, 505, 766, 774, 793, 810, 875, 901 | Installer and keeper admission, release checks, retries and starting remain 7.2. M3 and M6 exercise the underlying fresh-home and writer checks. |
| `install.test.js` | 148, 204, 267 | 2.3 and 7.2 own wipe, its completion receipt and reinstall admission. |
| `install.test.js` | 564, 594, 645, 700, 725, 750 | A1–A4 and A7 cover staging, publication and stand-in settlement. M3 covers the detection fence. 7.2 owns installer progress and Helper launch. |
| `folder-version.test.js` | 33, 52, 76, 98, 123, 142, 161, 184, 203 | Section 4 excludes replaying the intermediate TypeScript-only layout steps. The Rust store's format rules are 1.2. |
| `folder-version.test.js` | 226, 381 | M5 covers the interrupted 0→1 handoff. A7 covers app publication and layout sealing. A8 and C1 cover the imported group record and messages at the private import boundary and in the published home. |
| `folder-version.test.js` | 252, 273 | Covered by S1. |
| `folder-version.test.js` | 287, 301, 326 | 4.1 owns the bridge gate, 7.1 and 7.2 own keeper/startup gate and release trial rollback. |
| `update-from-before.test.js` | 54 | M5 covers opening the home after starting the new executable. 7.2 and 9.3 own switching to it and keeper judgment. |

### Associated compatibility scenarios

| Reference | Rust coverage and remaining owner |
|---|---|
| [Settings adoption](../../box/bridge-tests/bridge.test.js#L2316), [Git settings](../../box/bridge-tests/git-words.test.js#L93) and [Git record version](../../box/bridge-tests/git-words.test.js#L138) | M1, A2 and A11 cover saved-settings precedence, a linked settings file, branch-prefix import and reopening. The cited Git tests submit a live `POST kuvo/git/settings/move` request, owned by 7.3. |
| [Worktree handoff](../../box/bridge-tests/worktrees.test.js#L392) and [saved prefix](../../box/bridge-tests/worktrees.test.js#L548) | M1 and A2 cover the saved prefix and records on disk. These tests submit `POST kuvo/worktrees/record` and start a worktree through the live worktree API. The worktree consumer is 3.2, and the released Mac request is 7.3. |
| [Carried time zone](../../box/test/time-zone.test.ts#L76), [following a changing kit](../../box/test/time-zone.test.ts#L124) and [time-zone bridge scenarios](../../box/bridge-tests/time-zone.test.js) | M1 covers initial zone selection from the kit. Propagation, an explicit setup choice superseding the carried setting, and the wake-up consumers need 2.1 and their area scenarios. Repeated old-Mac input needs 7.3. |
| [Unversioned peers](../../box/bridge-tests/peers.test.js#L44), [later mail](../../box/bridge-tests/peers.test.js#L57) and [old outbox](../../box/bridge-tests/peers.test.js#L586) | M1 covers fixed on-disk peer projection and relocation of unclaimed mail. Receiving newer legacy peers mail needs 7.3. Delivery after relocation also needs the 2.4 letter consumer. |
| [Mac's home and Helper](../../box/bridge-tests/own-home-mac.test.js) | A7 covers migration settlement with a Keychain stand-in. Real Helper startup needs 7.2. Awake and engine behaviour remain with their runtime phases. |
| [Predecessor setup](../../box/test/setup-one-model.test.ts), [Mac main](../../box/bridge-tests/setup-mac.test.js#L137) and [following a newer Mac](../../box/bridge-tests/documents.test.js#L317) | S2 covers conversion of a checkpointed Mac primary through `kuvo_setup`. Layout.4 conversion is excluded. Membership interoperability, old numbering, and the remote legacy Mac follow path depend on 2.1, 5.2, and 7.3. |
| [Legacy personality library](../../box/bridge-tests/personalities-box.test.js#L88) | M1 carries custom records from the machine kit. A2 imports the Mac's local library and removals. 2.1 owns setup-wide replication. |
| [Tagged compat cases](../../protocol/conformance/compat.test.mjs) and [hold/1](../../protocol/conformance/holder.test.mjs) | Both Mac app cases and the 0.7.2 machine pass on Rust. Historical verbs and arriving beta.1 mail need 7.3. Holder mode belongs to 4.1. |

## Boundaries and gaps

- The switch accepts completed TypeScript homes. Unfinished TypeScript
  journals fence startup, with file-only pre-clear placement recovery in the
  standalone command. The in-place move, destructive clearing, Move Back and
  kept-copy expiry are dropped: TS-beta only (user 11-23). The command came
  after both releases in `810abb362`. The released upgrade uses
  `planMove(..., { whole: true })`, which has an empty clearing list. `finish`
  writes `app` for that move, and only the in-place branch writes `keptUntil`.
  `backRefusal` refuses Move Back when `app` exists. C1 checks that reply.
- For a standalone move an attachment name collision is
  resolved in the private copy and the combined directory is placed whole.
  Actions from an older file-only journal are validated against its private
  staging and archive paths and unwound before retry. An explicit resume
  writes the failure and retry receipt first. Files from a removed destination
  move to a kept folder through a separate identity-checked plan before cutover.
  Fresh making prepares the release privately before placing it and commits
  layout 23 under the startup lock before dropping the attempt marker.
  External history renames
  use a separate journaled list with file identities. Grok's index is retired
  after the move reaches its forward-only step. The signed Keychain program for
  website sign-ins belongs to 6.2. The vault seam is the login create and
  values-match operations used by `rehome/passwords.ts`; the original is
  retired only after the exact value verifies. For added Claude Code accounts,
  the mover calls `KeychainItems` through `Run::execute_with_keychain` and
  `resume_with_keychain`. Helper supplies its generic-password operations in
  6.2. The actual signed tools and live
  Helper launch still need a dedicated integration check. Both tagged
  published destinations match with the pinned OpenCode executable and the
  fixture's Keychain stand-ins.
- **7.2 and 9.3** own installer and keeper integration and the fleet rehearsal.
  Section 4 specifies the pre-Rust backup and manual recovery. The move code
  starts from the executable’s home gate or `rehome --run`. The installer and
  keeper own launching the new Helper.
- **7.3** owns `stays` and `next` from the compatibility inventory, including
  the live response to unsigned beta.1 peers mail, old SSH settings verbs and
  repair after an old Mac has overwritten installation files.
- The beta.30 items, layout.4 setup, and migrations 11 and 17 are excluded.
  The beta.1 tag packages a Mac app and Unix machine scripts. Its
  `.github/workflows/release.yml` has no native Windows installer or release
  asset. The Windows beta.1 coverage uses synthetic storage inputs.

## Verification

The TypeScript reference passes all five compat cases, the holder
transcript, and all four stored-format cases on macOS. The current-beta
fixture includes the retained-work stamps made by the 2.3 unlink writer.
The Rust workspace checked with
tests after `scripts/build-built.sh` generated the wire vectors. In the
machine-move session all six machine scenarios passed on macOS, native Windows,
WSL2 Ubuntu 24.04, and lokum, including all eight process cuts. The seventh
harness entry is the child used by the crash scenario. LLVM MinGW built the
native Windows tests and resident. A Mac-built static x86-64 musl executable
passed on WSL2 and on lokum in a disposable container capped at one CPU and
1 GB. The one-thread libtest progress prefix originally masked the crash
checkpoint. The final run passed with the checkpoint on its own line.
The six machine scenarios were rerun on macOS, on lokum and WSL2 using a
Mac-built static executable, and on native Windows using LLVM MinGW.
The lokum container runs an init so the harness is an ordinary process when
the live-PID scenario checks it.
After refreshing from main, the machine suite passed again on macOS and
native Windows. The refreshed static musl build passed under WSL2. The
earlier lokum pass covers the separate Linux host.

With the verified OpenCode 2.2.6 package, C1 passes 30 checks on each
executable, including its enclosing scenario. The focused Rust run passes
20 Mac scenarios, both tagged Mac scenarios, and six machine scenarios
plus their child harness. Nine preferences scenarios pass in each run of
C2. Eleven further released-move reference scenarios pass on Rust.
The TypeScript pending-letter regression failed before the fix because
truncation split a surrogate pair. Both executables now preserve a valid
string at the wire limit. The older malformed-journal assertion now uses
the specific receipt the TypeScript command emits, also checked in C1.
The peer moves area passed three checks and skipped two mixed-binary
cases on each executable.

The Rust stored-format area passes 4/4. The holder area fails because the
Rust executable does not yet implement the holder mode (4.1). Rust compat
passes both tagged Mac app cases and the 0.7.2 machine case (3/5). The beta.1 machine fails on the next live,
unsigned peer update (7.3). Its original saved peers are already projected
at startup. The current-beta case reaches the resident and first fails on
`kuvo/projects` (6.3). A direct
check after opening the resident preserved all 18 durable files byte for
byte and kept the four retained-work fields and the computer identity.

| Fixture | Post-startup differential and separate assertions |
|---|---|
| 0.7.2 machine | Opens at 23 with the original key and relay secret. The rerun with `8e3f4262b` differs on 57 paths. `kuvo/info` differs and `kuvo/setup` differs only on the generated setup ID. |
| 0.8.0-beta.1 machine | Opens at 23 with original identity and the saved `peersFrom` projected under `legacy`. The rerun with `8e3f4262b` differs on 53 paths, `kuvo/info` and the stable `you.key` in `kuvo/setup`. |
| 0.7.2 Mac app folder | All 76 published paths with the engine stand-in and all 81 with OpenCode 2.2.6 match. The move reply, Git backlink, and Keychain stand-in match. After startup, the setup answer matches and 40 runtime paths differ in the recorded run. |
| 0.8.0-beta.1 Mac app folder | All 79 published paths with the engine stand-in and all 84 with OpenCode 2.2.6 match, including the pending letter. The move reply, Git backlink, and Keychain stand-in match. After startup, the setup answer matches and 44 runtime paths differ in the recorded run. |
| Current TypeScript beta | Already at 23. The setup answer matches. After preparing an offline copy of the fixture, `kuvo/info` and 46 paths differ with `8e3f4262b`. |

The differential counts include directory entries. The sequence asked for
`GET kuvo/info` and `GET kuvo/setup` on each stopped machine seed. The entire
post-stop inventories were compared. These are recorded differences, with
the companion files still visible in the comparison.

## Verdicts for the machine differentials

The original machine runs reported 62, 54 and 48 differing paths. Main through
`b417a12e8` reported 62, 57 and 51. The rerun after merging 2.1b at
`ac1c56076` reported 60, 56 and 50. The next run on the publication branch
reported 57, 51 and 42. The rerun with 2.1, 2.2 and 3.2b on main reported
60, 54 and 45. The rerun after merging `8e3f4262b` reported 57, 53 and 46.
The additional current-beta difference is the startup document counters in
`run/link.json`. The prior owner groups still cover the union. The 0.7.2
phone present only in Rust's startup roster has its original timestamp;
expiry and the membership projection belong to 2.2 and 2.1. The session
list remains a 3.2 producer. The latest `run/lists` directory is present on
both sides. Each prefix
includes the directory entry and the children listed with it. The same
startup and request sequence ran in each binary.

| Paths | Verdict and reason |
|---|---|
| `folder-version.json`, retained machine identity and beta.1 stored peer records | **Made identical.** Both machines reach layout 23 with the same key and relay secret. Beta.1 retains the same stable peer fields and last-heard value in `legacy`. The extra startup fields in the same link file are accounted for below. |
| `run/link.seen` | **Made identical.** The 2.1b startup writer matches the empty marker. The installed 0.7 Mac uses the mtime in its leave-running path. That released-client drill belongs to 7.3 and 9.3. |
| `run/link.version` | **Made identical at the writer boundary.** Both write the build identifier as plain UTF-8. The identifier differs with the executable. The keeper's version selection and rollback belong to 7.1. |
| `run/link.needed` | **Owned by 7.1.** The keeper's `kuvo-start` reads it to decide when to start the link. The agents' fence and browser also read it. |
| `run/link.json` startup fields, `run/docs.changed`, `run/setup-checkpoints/*.json` | **Owned by 2.1.** The current-beta link differs on startup peer and phone document counters (`kuvo_setup/src/document.rs`). Older cases differ in the startup roster's phone projection, empty membership fields and beta.1 signing and ownership fields. `setup/lead.ts`, `setup/membership.ts`, `state.ts` and `documents/changed.ts` write these fields and the document notification. Checkpoint file names embed a signature-derived hash. New setups create different IDs and keys. The stable roster fields still differ, so the older checkpoints do too. Link adoption with an attached legacy Mac also needs 7.3. |
| `run/setup-card.json` | **Owned by 2.3 and 5.2.** Installation discovery reads the startup card. The release cases differ in first-write timing or the stable `used` field. The new setup ID and card clock may vary. See the [2.1 startup file audit](rust-setup-coverage.md#startup-file-audit) for the queue and usage inputs. |
| `run/peer-reach.json`, `run/peer-reach-checked.json` | **Owned by 2.4.** These are the peer reach projections from `peers/reach.ts`. Clock values are observations at startup. The stable records and file presence still need to match. |
| `run/agents.json` | **Owned by 4.1.** The legacy roster export is read by `agents/main.ts` and `peers/known.ts`. |
| `run/moves`, `run/outbox`, `run/outbox/sending`, `run/outbox/sending/ltr_golden.json` and generated `ltr_*.json` under that directory | **Owned by 2.4.** The resident's first send and retry round in `letters/outbox.ts` creates or retires entries and initializes move bookkeeping. A generated letter ID may differ between runs. Stable recipient, payload, disposition and file presence still count. The separate 2.6 import from a flat beta.1 outbox passes M1. |
| `config`, `config/opencode`, `config/opencode/plugins`, `config/opencode/plugins/kuvo-defaults.js` | **Owned by 3.1.** Engine launch materializes defaults and its plugin in `engine/defaults.ts`. These are the new engine configuration layer, separate from the imported user's settings. |
| `run/lists` | **Owned by 3.2.** Both sides created the directory in the 2.1b run. The latest TypeScript current-beta run stopped before it appeared. |
| `run/queues-scanned.json`, `run/lists/sessions.json` | **Owned by 3.2.** Startup scans queues and saves the local session projection in `engine/queue.ts` and the lists writers. The scan keys can embed the engine port. The list epoch embeds process and clock data. The differential already normalizes documented volatile fields. Stable fields such as the owner and bot work projection remain open. |
| `agents`, `agents/sessions/ses_golden/messages-v1.sqlite-shm` | **Owned by 4.1.** The bridge initializes its home and opens the transcript; the extra shared-memory index is SQLite runtime state. `agents/main.ts` and the transcript store define the startup behavior. The seeded transcript itself is preserved. |
| `agents/limits.json`, `agents/usage`, `agents/usage/{claude-files,codex-files,grok-files,settings.json,usage.json,insights.sqlite,insights.sqlite-shm,insights.sqlite-wal,insights-logs,insights-logs/codex-files}`, `run/usage-attribution.json` | **Owned by 5.4.** Usage background work creates scans, the SQLite index and its journals, and marks attribution in `usage/service.ts`. Scan times, the allocated ports and SQLite's journal bytes can vary. The backing data families and their stable results still need to agree. |
| `bots`, `wakes`, `run/bots-seen`, `run/glance`, `run/glance/asks.json`, `run/notices`, `run/notices/teller`, `run/work`, `run/work/background.json`, `run/work/told.json` | **Owned by 5.1.** The bot, ask, wake and notice services create these roots and records as they start and catch up. `bots/seen.ts` consumes the bot cursor. The differential normalizes notice scan times and teller PID, and still reports missing records and other fields. |
| `run/work-queue`, `run/work-queue/forward`, `run/work-queue/numbers.json`, `run/github-sign-in` | **Owned by 5.2.** Queue replay, its number allocations and GitHub sign-in staging are created by the resident services, including `work-queue/numbers.ts` and `services.ts`. |
| `artifacts`, `artifacts/citations-v1.json`, `artifacts/copies/**`, `artifacts/index-v1.sqlite`, `artifacts/key`, `artifacts/log/**`, `library`, `library/backfill.json`, `box/marketplace`, `box/marketplace/mcp-relay-v1.json` | **Owned by 5.5.** Artifact indexing creates content-addressed copies and the monthly log at startup. `library/service.ts` reads and writes its backfill receipt, and `marketplace/connections.ts` manages its relay address. Paths may contain content digests or month names. The differential compares committed SQLite rows and still requires the stable records and file presence. |
| `box`, `box/approval.json`, `run/fence`, `run/fence/agents.md` | **Owned by 6.1.** Startup creates approval and sandbox defaults. `fence/state.ts` projects the engine's sandbox note. No historical value in these fixture inputs was dropped by the move. |
| `run/socket.key`, `run/ssh-for`, `run/vault.sock` | **Owned by 6.2.** The vault broker and per-session SSH credentials use these. `home.ts` creates and reads the random socket key, and `fence/files.ts` records how it is provided to the engine. Its bytes may vary. The stable shape and the broker endpoints remain open. |
| `run/route/ssh` | **Owned by 1.4.** TypeScript's local SSH route rendezvous is selected by `server/ssh-pipe.ts`. The transport owner has to account for the differing endpoint lifetime and path. |
| `GET kuvo/info` field `serverBuild` | **Made identical for the field shape.** The value identifies which executable answered. |
| `GET kuvo/info` fields `bridge`, `pieces.link` | **Owned by 7.1 and 4.1.** These report installed and running versions and process readiness through the keeper and bridge. Build IDs can vary. Stable readiness and the installed/running fields still differ. |
| `GET kuvo/info` fields `name`, `macs`, `phones` | **Owned by 2.1/2.2.** Setup identity and device/peer association supply these facts. Legacy live client projections also need 7.3. The imported link's saved name and original keys are preserved on disk. |
| `GET kuvo/info` fields `engineState`, `running`, `pieces.engine`, `pieces.agents` | **Owned by 3.1 and 4.1.** These describe the currently adopted processes, which is separate from moving the saved home. |
| `GET kuvo/info` fields `features`, `compatibility` | **Owned by 1.1 and the advertised route owners.** Rust and TypeScript advertise their implemented surfaces. Missing route features remain visible. |
| `GET kuvo/setup` field `setup` on 0.7.2 | **Made identical after accounting for the generated ID.** Both answers have the same structure and all other fields match. The current-beta answer also matches. |
| `GET kuvo/setup` field `you.key` on beta.1 | **Owned by 2.1 and 7.3.** The resident's stable self projection with a legacy Mac attachment still differs after the 2.1b merge. The original identity is preserved in the move. |
| Both Mac app destinations and `rehome --run` reply | **Made identical** for the tagged cases. The published comparison and startup verdicts follow. |

No differing path has an unread verdict. Existing consumers and ownership gaps
are tracked by the phase that owns the corresponding startup work. The table
does not count runtime paths as identical merely because their contents vary.

### Verdicts for the two published Mac destinations

[The Mac differential runner](../../protocol/conformance/differential/mac-app.mjs)
generates the two tagged fixtures, runs each command at the same paths with
the same input, and checks the stopped destinations before starting either
resident. The published comparison passes with both the fixture engine and
the real OpenCode 2.2.6 binary verified against the SHA-512 pin. Both use the
fixture's Keychain stand-ins.

| Paths in both tagged cases | Verdict and reason |
|---|---|
| `box`, `box/kind`, `folder-version.json`, `updates`, `updates/rehome` and the other published files | **Made identical.** Modes, bytes and symlink targets match, including the attached files, conversations and beta.1's pending letter. |
| `data/opencode/opencode.db` | **Made identical at the SQLite boundary.** Mode, schema, version and committed typed rows match. For the real engine, nine newly applied migrations have independent `time_completed` values. The earlier migration times and all other stable fields match. Physical pages differ. |
| `data/opencode/log/opencode.log` | **Made identical** in the real-engine run after accounting for timestamps, run IDs, migration duration, and the optional final line recording the verifier's SIGTERM. Every stable message and field matches. |
| `updates/rehome/journal.acts`, `updates/rehome/keychain.json` | **Made identical.** Action log and Keychain receipt match byte for byte. |
| `updates/rehome/journal.json` | **Made identical** after accounting for wall-clock values in `began`, `clonedAt`, and the final component of `kept`. Every stable field, the placement list, the set of names at each database path, and the mode match. |
| `updates/rehome/state.json` and the command's JSON reply | **Made identical** after accounting for `movedAt`. Inventory counts, audit notes, setup obligations and other stable fields match. |
| Git backlinks and original worktree link outside the home | **Made identical.** The Git metadata points to the published worktree and the original link is preserved. |
| Keychain stand-in outside the home | **Made identical.** Copies verify and originals are retired after publication. The signed tool and actual Helper launch still need a real integration fixture. |
| `GET kuvo/setup` after starting each resident | **Made identical.** Both return the same values for each of the moved app copies. |
| `GET kuvo/info` after starting each resident | **Owned by 1.1, 2.1 and 3.1.** The remaining fields are advertised features and compatibility, the startup computer name, and engine readiness. The executable build can vary. |
| Other runtime files after starting each resident | The owner groups in the machine table apply to the shared services. The two Mac runs also differ on `box/defaults-added` and `config/opencode/opencode.json` after engine startup (**3.1**), on `run/work-awake.json` (**5.1**), and on the Mac activity marker `run/link.seen` (**2.1/7.3**). The stopped destinations already match. |

The tagged interruption scenario kills the process by PID at five private
staging boundaries and eleven publication or settlement boundaries per app
fixture. The full published inventory after restart matches the uninterrupted
move after projecting the same clock fields. Both source trees stay identical.
An extra cut after displacing the first file from a removed installation
checks that its saved files all reach the archive after restart.
A disk-backed credential stand-in also cuts after the account's saved item
is copied aside, after the new item is written, and after the old item is
retired. The named Helper primitive is the 6.2 integration boundary.
The setup conversion scenario also kills the resident after the relay accepts
the claim and checks that a restart advances the same setup only once.

To reproduce the published comparison, use both built executables.

```sh
KUVO_BOX_BIN_A=box/dist/kuvo-link.js KUVO_BOX_BIN_B=box/target/debug/kuvo-box \
  node protocol/conformance/differential/mac-app.mjs
```

Set `KUVO_TEST_ENGINE` to the pinned executable to use the real engine.
Pass a requests file to compare answers and runtime files after startup as well.
