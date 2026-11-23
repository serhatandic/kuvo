# Rust move coverage

Phase 2.6 is in progress. The resident passes the store's `MoveRequired` and
`ResumeMove` handoffs to `kuvo_move`. The machine path recognizes a predecessor
by its saved files, takes the store's layout lock, checks again under the lock,
and commits layout 23 after the last step. A current layout opens without
rerunning the imports. The machine path refuses an active old writer and an
unfinished TypeScript Mac move. Mac app discovery recognizes the released app's folder and holds startup until
cutover. The private Mac staging entry prepares and validates cloned data
without touching the published layout.

The TypeScript machine reference takes data in through several entry points.
The Rust machine move runs those on-disk imports before starting the resident.
Settings copy the earlier global and private configuration and shared key files,
then take the saved branch prefix. The kit supplies the carried clock, projects
and custom personalities. The link projects the stored beta.1 peers list,
preserving the identity and other saved fields. Outbox mail moves by rename,
address-only browser pages acquire their session field, and obsolete agent
programs, copied sign-ins and Mac presence records are retired. The phases
replay from disk after a process exits. No intermediate layout version is
published. Mac staging also replays from a fresh copy after an interruption. The
publication phase still needs its journal for placement, worktree repairs
outside the home and Helper handover.

## Rust scenarios

[Machine scenarios](../../box/crates/kuvo_move/tests/machine.rs) exercise the
real filesystem and the store handoff. [Mac scenarios](../../box/crates/kuvo_move/tests/mac_stage.rs)
exercise the private stage and the tagged engine fixtures.

| ID | Scenario |
|---|---|
| M1 | An older machine adopts its settings, projects its kit and peers, carries queued mail and browser pages, and retires only the old runtime and presence files. A second open preserves an edited setting. |
| M2 | A newer layout, unsupported interrupted layout and unfinished TypeScript app move preserve the files and hold startup. |
| M3 | A fresh home opens at 23 and the released Mac app's data holds startup before any destination is written. |
| M4 | A child stopped by PID after each of eight durable machine boundaries restarts. The complete file and directory inventory compares with an uninterrupted move at the same path. |
| M5 | A machine that never had a bridge opens through the store's interrupted 0→1 handoff and keeps its instructions. |
| M6 | A live predecessor bridge keeps the move from retiring its files. |
| A1 | An older Mac's attachments, chats, links, and saved run records are cloned into private staging. Process records stay with the old app. |
| A2 | A binary plist projects projects, worktrees and Git and approval settings. The project and worktree records name the final home. |
| A3 | A write in the source after cloning stops staging. A retry opens the changed input. |
| A4 | The staged database for each tagged Mac passes the isolated pinned engine's migration and session list checks. A child killed at each staging boundary rebuilds the same private tree. The source and published layout remain intact. |
| S1 | The stored-format conformance scenarios refuse newer, malformed and invalid layout records and reopen a fresh home at 23. |

## TypeScript scenario map

The primary inventory contains 135 test declarations. Each loop is counted at
its declaration. The two forward-move loops expand to eleven cuts, the rollback
loop to three, and the saved-sign-in unit loop to four outcomes. At the primary
declaration level, two are covered by S1, and the remaining 133 are partial,
open, or owned by another phase. The associated compatibility checks below are
tracked separately.

| Reference | Declarations | Rust coverage and remaining owner |
|---|---:|---|
| [rehome](../../box/test/rehome.test.ts) | 36 | M2 covers a recognizable unfinished journal's startup fence and M4 covers the machine crash boundaries. A1–A4 cover private clones, a source write during cloning, and database and path rewrites within the staged engine and selected records. Final placement, kept copies, move progress, broader writer detection, rollback and forward recovery are open in 2.6. App-facing readiness, new install and keeper scheduling also need 7.2. |
| [rehome-run](../../box/test/rehome-run.test.ts) | 35 | M2, M3 and M6 exercise the destination, detection and writer fences for the machine entry point. A4 checks the tagged database migration and engine validation in staging. Process quiescence, handoff and cleanup, copied sign-ins, and the standalone run command remain 2.6. |
| [rehome-upgrades](../../box/test/rehome-upgrades.test.ts) | 9 | M1 covers the separate machine kit projections. A2 checks app plist decoding, projects, worktrees and Git. A4 checks the beta.1 pending letter. The remaining settings and peer projection, bots, group chats, broader letter cases, artifact rewrites and future app data remain 2.6. |
| [rehome-passwords](../../box/test/rehome-passwords.test.ts) and [bridge scenario](../../box/bridge-tests/rehome-passwords.test.js) | 2 | Mac handoff records and settling are open in 2.6. The actual saved sign-in transfer also needs the vault and Helper adapter in 6.2. |
| [install-verdict](../../box/test/install-verdict.test.ts) | 12 | M2 and M3 cover blocking an unfinished journal and recognizing data beside the app's own cache and program folders. M5 covers in-place startup without a bridge. The full verdict matrix, new-install collision handling, explicit move and restart need 2.6 and 7.2. |
| [install](../../box/bridge-tests/install.test.js) | 24 | The older app, move progress, failed launch settlement and two-install cases need the 2.6 staged move and 7.2 installer. Fresh install, release checks, staging races and keeper launch are 7.2. Wipe and reinstall admission are 2.3 and 7.2. |
| [folder-version](../../box/bridge-tests/folder-version.test.js) | 16 | S1 covers future and unreadable marker refusals. M2 and M5 cover the move's reader and valid 0→1 retry. Mac import sealing remains 2.6. Steps 11 and 17 and the layout transitions only used by unpublished TypeScript betas are excluded by section 4. Bridge and keeper entry points and trial rollback need 4.1 and 7.1–7.2. |
| [update-from-before](../../box/bridge-tests/update-from-before.test.js) | 1 | An updater handing over to the new keeper and updating its engine and bridge belongs to 7.2 and 9.3. M5 only covers opening data once the new executable runs. |

### Associated compatibility scenarios

| Reference | Rust coverage and remaining owner |
|---|---|
| [Settings adoption](../../box/bridge-tests/bridge.test.js#L2303), [Git settings](../../box/bridge-tests/git-words.test.js#L93) and [Git record version](../../box/bridge-tests/git-words.test.js#L138) | M1 covers saved-settings precedence, branch-prefix import and reopening. Legacy SSH/route handoff and externally linked settings need their 2.6 or 7.3 path. |
| [Worktree handoff](../../box/bridge-tests/worktrees.test.js#L392) and [saved prefix](../../box/bridge-tests/worktrees.test.js#L548) | M1 covers the prefix on disk. Keeping the legacy worktree offered by another computer needs the 2.6 handoff and worktree consumer. |
| [Carried time zone](../../box/test/time-zone.test.ts#L76), [following a changing kit](../../box/test/time-zone.test.ts#L124) and [time-zone bridge scenarios](../../box/bridge-tests/time-zone.test.js) | M1 covers initial zone selection from the kit. Propagation, an explicit setup choice superseding the carried setting, and the wake-up consumers need 2.1 and their area scenarios. Repeated old-Mac input needs 7.3. |
| [Unversioned peers](../../box/bridge-tests/peers.test.js#L44), [later mail](../../box/bridge-tests/peers.test.js#L57) and [old outbox](../../box/bridge-tests/peers.test.js#L586) | M1 covers fixed on-disk peer projection and relocation of unclaimed mail. Receiving newer legacy peers mail needs 7.3. Delivery after relocation also needs the 2.4 letter consumer. |
| [Mac's home and Helper](../../box/bridge-tests/own-home-mac.test.js) | Migration settlement is open in 2.6, Helper and setup startup in 2.1 and 7.2, and awake/engine behaviour with their respective runtime phases. |
| [Tagged compat cases](../../protocol/conformance/compat.test.mjs) and [hold/1](../../protocol/conformance/holder.test.mjs) | The old Mac app cases need the staged transaction, database validation, and Helper steps in 2.6. Historical verbs and arriving beta.1 mail need 7.3. Holder controller exchange is covered in conformance. Full resident adoption belongs to 4.1. |

## Boundaries and gaps

- **2.6** still owns both Mac app imports, including the remainder of staged layout-1 work,
  source writer handling, placement, remaining path and history repairs, app
  preferences and records, group chats, Keychain handover and setup conversion
  through `kuvo_setup`. It also owns
  full parity of the machine imports, including the legacy worktree and Git
  handoff, exact handling of malformed legacy files, and verification against
  the golden homes. The existing home differential CLI starts the resident
  from a home seed. It needs a staged-app mode to compare the old app directory,
  its destination and the post-move answers together.
- **7.2 and 9.3** own installer and keeper integration and the fleet rehearsal,
  including the pre-Rust backup and rollback drill. The move code currently
  starts from the box executable’s home gate or its library entry.
- **7.3** owns `stays` and `next` from the compatibility inventory, including
  the live response to unsigned beta.1 peers mail, old SSH settings verbs and
  repair after an old Mac has overwritten installation files.
- The beta.30 items, layout.4 setup, and migrations 11 and 17 are excluded.
  The beta.1 tag packages a Mac app and Unix machine scripts. It has no
  native Windows installer or native release asset in its release workflow.

## Verification

The TypeScript reference passes all five tagged/current compat cases, the
holder transcript, and all four stored-format cases on macOS. The Rust
workspace checked with tests after `scripts/build-built.sh` generated the
wire vectors. All six machine scenarios passed on macOS, native Windows and
WSL2 Ubuntu 24.04, including the eight process cuts. The seventh harness entry
is the child used by the crash scenario. LLVM MinGW built the native tests and
resident. The Windows resident answered `kuvo/info` from a fresh home at layout
23. A static x86-64 musl build supplied the WSL test executable. The capped
Linux run on lokum stopped returning output and both SSH addresses timed out,
so it has no recorded test result.

The Rust stored-format area passes 4/4. The holder area fails because the
Rust executable does not yet implement the holder mode (4.1). Rust compat
passes the 0.7.2 machine case. The beta.1 machine fails on the next live,
unsigned peer update (7.3). Its original saved peers are already projected
at startup. The current-beta case reaches the resident and first fails on
`kuvo/projects` (3.2). A direct check preserves all 18 durable files listed
by that fixture byte for byte and gets the expected setup and personalities.

| Fixture | Post-startup differential and separate assertions |
|---|---|
| 0.7.2 machine | Opens at 23 with the original key and relay secret. `kuvo/info`, `kuvo/setup` and 62 paths differ in the full comparison. Differences include newly minted setup identity and signing data, link fields and resident companion files. |
| 0.8.0-beta.1 machine | Opens at 23 with original identity and the saved `peersFrom` projected under `legacy`. The two answers and 54 paths differ, including the startup signing field and companion files. |
| 0.7.2 Mac app folder | The reference move passes. Rust stops at the unimplemented `rehome --run` command, before a paired destination can be compared. |
| 0.8.0-beta.1 Mac app folder | Same command gap. The reference moves the app data and its pending letter. |
| Current TypeScript beta | Already at 23. The setup answer matches, while `kuvo/info` and 48 paths differ, including companion files and startup bookkeeping. |

The differential counts include directory entries. The sequence asked for
`GET kuvo/info` and `GET kuvo/setup` on each stopped machine seed. The entire
post-stop inventories were compared. These are recorded differences, with
the companion files still visible in the comparison.

## Verdicts for the machine differentials

The three machine runs against main through `b417a12e8` reported 62, 57 and 51
differing paths. The earlier runs reported 62, 54 and 48. The groups below
cover the union. Each prefix includes the directory entry and the children
listed with it. The same startup and request sequence ran in each binary.

| Paths | Verdict and reason |
|---|---|
| `folder-version.json`, retained machine identity and beta.1 stored peer records | **Made identical.** Both machines reach layout 23 with the same key and relay secret. Beta.1 retains the same stable peer fields and last-heard value in `legacy`. The extra startup fields in the same link file are accounted for below. |
| `run/link.json` startup fields, `run/link.seen`, `run/link.version`, `run/link.needed`, `run/docs.changed`, `run/setup-card.json`, `run/setup-checkpoints/*.json` | **Owned by 2.1b.** Setup bootstraps the sign seed, links roster and local authority, marks use, and publishes the card and checkpoint. `setup/lead.ts`, `state.ts` and `documents/exchange.ts` are the TypeScript references. The relay setting, signatures and setup IDs are minted at startup. File names under `setup-checkpoints` embed a signature-derived hash. Current differences include stable field presence, so the files as a group are still open even after accounting for random values and the card clock. The installed 0.7 Mac also reads `link.seen` in the served 7.3 `leave-running` path. |
| `run/peer-reach.json`, `run/peer-reach-checked.json`, `run/agents.json` | **Owned by 2.2.** These are the answered peer reach and directory projections from `peers/reach.ts` and `peers/agents.ts`. Clock values are observations at startup. The existence and contents of the directory and reach files still need to match. |
| `run/moves`, `run/outbox`, `run/outbox/sending`, `run/outbox/sending/ltr_golden.json` and generated `ltr_*.json` under that directory | **Owned by 2.4.** The resident's first send and retry round in `letters/outbox.ts` creates or retires entries and initializes move bookkeeping. A generated letter ID may differ between runs. Stable recipient, payload, disposition and file presence still count. The separate 2.6 import from a flat beta.1 outbox passes M1. |
| `config`, `config/opencode`, `config/opencode/plugins`, `config/opencode/plugins/kuvo-defaults.js` | **Owned by 3.1.** Engine launch materializes defaults and its plugin in `engine/defaults.ts`. These are the new engine configuration layer, separate from the imported user's settings. |
| `run/queues-scanned.json`, `run/lists`, `run/lists/sessions.json` | **Owned by 3.2.** Startup scans queues and saves the local session projection in `engine/queue.ts` and the lists writers. The scan keys can embed the engine port. The list epoch embeds process and clock data. The differential already normalizes the documented volatile fields. Stable payload and presence remain open. |
| `agents`, `agents/sessions/ses_golden/messages-v1.sqlite-shm` | **Owned by 4.1.** The bridge initializes its home and opens the transcript; the extra shared-memory index is SQLite runtime state. `agents/main.ts` and the transcript store define the startup behavior. The seeded transcript itself is preserved. |
| `agents/limits.json`, `agents/usage`, `agents/usage/{claude-files,codex-files,grok-files,settings.json,usage.json,insights.sqlite,insights.sqlite-shm,insights.sqlite-wal}`, `run/usage-attribution.json` | **Owned by 5.4.** Usage background work creates scans, the SQLite index and its journals, and marks attribution in `usage/service.ts`. Scan times, the allocated ports and SQLite's journal bytes can vary. The backing data families and their stable results still need to agree. |
| `bots`, `wakes`, `run/bots-seen`, `run/glance`, `run/glance/asks.json`, `run/notices`, `run/notices/teller`, `run/work`, `run/work/background.json`, `run/work/told.json` | **Owned by 5.1.** The bot, ask, wake and notice services create these roots and records as they start and catch up. `bots/seen.ts` consumes the bot cursor. The differential normalizes notice scan times and teller PID, and still reports missing records and other fields. |
| `run/work-queue`, `run/work-queue/forward`, `run/work-queue/numbers.json`, `run/github-sign-in` | **Owned by 5.2.** Queue replay, its number allocations and GitHub sign-in staging are created by the resident services, including `work-queue/numbers.ts` and `services.ts`. |
| `artifacts`, `artifacts/citations-v1.json`, `artifacts/copies/**`, `artifacts/index-v1.sqlite`, `artifacts/key`, `artifacts/log/**`, `library`, `library/backfill.json`, `box/marketplace`, `box/marketplace/mcp-relay-v1.json` | **Owned by 5.5.** Artifact indexing creates content-addressed copies and the monthly log at startup. `library/service.ts` reads and writes its backfill receipt, and `marketplace/connections.ts` manages its relay address. Paths may contain content digests or month names. The differential compares committed SQLite rows and still requires the stable records and file presence. |
| `box`, `box/approval.json`, `run/fence`, `run/fence/agents.md` | **Owned by 6.1.** Startup creates approval and sandbox defaults. `fence/state.ts` projects the engine's sandbox note. No historical value in these fixture inputs was dropped by the move. |
| `run/socket.key`, `run/ssh-for`, `run/vault.sock` | **Owned by 6.2.** The vault broker and per-session SSH credentials use these. `home.ts` creates and reads the random socket key, and `fence/files.ts` records how it is provided to the engine. Its bytes may vary. The stable shape and the broker endpoints remain open. |
| `run/route/ssh` | **Owned by 1.4.** TypeScript's local SSH route rendezvous is selected by `server/ssh-pipe.ts`. The transport owner has to account for the differing endpoint lifetime and path. |
| `GET kuvo/info` fields `bridge`, `serverBuild`, `pieces.link` | **Owned by 2.1b.** These contain the running build and bridge readiness. A build identifier varies by binary. The structure, compatibility fields and readiness still have differing fields. |
| `GET kuvo/info` fields `name`, `macs`, `phones` | **Owned by 2.1/2.2.** Setup identity and device/peer association supply these facts. Legacy live client projections also need 7.3. The imported link's saved name and original keys are preserved on disk. |
| `GET kuvo/info` fields `engineState`, `running`, `pieces.engine`, `pieces.agents` | **Owned by 3.1 and 4.1.** These describe the currently adopted processes, which is separate from moving the saved home. |
| `GET kuvo/info` fields `features`, `compatibility` | **Owned by 1.1 and the advertised route owners.** Rust and TypeScript advertise their implemented surfaces. Missing route features remain visible. |
| `GET kuvo/setup` field `setup` on 0.7.2 | **Made identical after accounting for the generated ID.** Both answers have the same structure and all other fields match. The current-beta answer also matches. |
| `GET kuvo/setup` field `you.key` on beta.1 | **Owned by 2.1b.** The stable self projection differs with a legacy Mac attachment. |
| Both Mac app destinations and `rehome --run` reply | **Open in 2.6.** Rust has an unpublished staging entry. Remaining app projections, external path repair, Helper key handover, placement and forward recovery still have to work before there is a Rust destination inventory to classify. |

No differing path has an unread verdict. Existing consumers and ownership gaps
are tracked by the phase that owns the corresponding startup work. The table
does not count runtime paths as identical merely because their contents vary.

### Verdicts for the two Mac staging inventories

The Mac comparison ran the TypeScript `rehome --run` on the same scratch
source after taking the Rust staging inventory. It compared every relative
entry in private Rust staging and the published TypeScript home, excluding
the installer's input at `updates/install` and the Rust staging directory
itself. This is a stage-to-destination comparison. A published Rust app
destination and its post-move route answers are still open in 2.6.

| Paths in both tagged cases | Verdict and reason |
|---|---|
| All other compared entries, including `attachments/**`, `chats/**`, `config/opencode/opencode.json`, `config/opencode/plugins/kuvo-code.js`, rekeyed `data/opencode/snapshot/**`, and beta.1's `run/outbox/sending/ltr_fixture.json` | **Made identical in staging.** The 0.7.2 comparison matches bytes and modes on 66 paths. The beta.1 comparison matches bytes and modes on 69 paths. Relative symlink targets also match. |
| `data/opencode/opencode.db` | **Made identical at the SQLite boundary.** Both cases have identical schema and committed rows in every table, including remapped projects, session paths and attachment URLs. Page bytes differ. The Rust stage also starts the pinned engine on a private copy and validates its active and archived session lists and sampled directories. |
| `box`, `box/kind`, `folder-version.json`, `updates`, `updates/rehome` | **Open in 2.6.** These publication entries are created by the TypeScript move after staging. Rust leaves the live destination untouched until cutover. |
| `updates/rehome/journal.acts`, `updates/rehome/journal.json`, `updates/rehome/state.json` | **Open in 2.6.** The TypeScript journal records placement, outside writes and what the move owes. Some fields embed clock values. The stable actions, results and state still require a Rust publication journal. |
| `updates/rehome/keychain.json` | **Open in 2.6.** The Keychain tools journal their copies and settlement when invoked by the mover. The 6.2 Helper supplies the signed tool and broker. Rust staging leaves the source Keychain alone. |
| Worktree backlinks outside the destination | **Open in 2.6.** The TypeScript cutover repairs the fixture's external Git records. Rust staging rekeys the engine's snapshots. The update to the external repository needs its journal and recovery. |

The staged sequence consists of clone, import, database migration, path
rewrite and engine validation. The tagged scenario kills the Rust process
by PID after each of those five boundaries, then verifies that every staged
byte and link target matches the uninterrupted result. Both source trees
stay identical in the Rust runs. The eight machine-move cuts still pass.

The macOS workspace compile check and four regular Mac staging scenarios
pass. The tagged scenario passes against both generated app folders.
Six machine scenarios pass again on macOS, native Windows and WSL2.
Linux testing on lokum is deferred at the lead's request.
