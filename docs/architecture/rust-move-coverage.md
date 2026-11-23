# Rust move coverage

Phase 2.6 is in progress. The resident passes the store's `MoveRequired` and
`ResumeMove` handoffs to `kuvo_move`. The machine path recognizes a predecessor
by its saved files, takes the store's layout lock, checks again under the lock,
and commits layout 23 after the last step. A current layout opens without
rerunning the imports. The machine path refuses an active old writer and an
unfinished TypeScript Mac move. Mac app discovery recognizes the released app's
folder and holds startup until staged import can bring it over.

The TypeScript machine reference takes data in through several entry points.
The Rust machine move runs those on-disk imports before starting the resident.
Settings copy the earlier global and private configuration and shared key files,
then take the saved branch prefix. The kit supplies the carried clock, projects
and custom personalities. The link projects the stored beta.1 peers list,
preserving the identity and other saved fields. Outbox mail moves by rename,
address-only browser pages acquire their session field, and obsolete agent
programs, copied sign-ins and Mac presence records are retired. The phases
replay from disk after a process exits. No intermediate layout version is
published. The staged Mac import will need a separate journal for placement,
repair and handover outside the destination.

## Rust scenarios

[Machine scenarios](../../box/crates/kuvo_move/tests/machine.rs) exercise the
real filesystem and the store handoff.

| ID | Scenario |
|---|---|
| M1 | An older machine adopts its settings, projects its kit and peers, carries queued mail and browser pages, and retires only the old runtime and presence files. A second open preserves an edited setting. |
| M2 | A newer layout, unsupported interrupted layout and unfinished TypeScript app move preserve the files and hold startup. |
| M3 | A fresh home opens at 23 and the released Mac app's data holds startup before any destination is written. |
| M4 | A child stopped by PID after each of eight durable machine boundaries restarts. The complete file and directory inventory compares with an uninterrupted move at the same path. |
| M5 | A machine that never had a bridge opens through the store's interrupted 0→1 handoff and keeps its instructions. |
| M6 | A live predecessor bridge keeps the move from retiring its files. |
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
| [rehome](../../box/test/rehome.test.ts) | 36 | M2 covers a recognizable unfinished journal's startup fence and M4 covers the machine crash boundaries. All Mac staged placement, kept copies, database and path rewrites, move progress, writer detection, rollback and forward recovery are open in 2.6. App-facing readiness, new install and keeper scheduling also need 7.2. |
| [rehome-run](../../box/test/rehome-run.test.ts) | 35 | M2, M3 and M6 exercise the destination, detection and writer fences for the machine entry point. The app runner scenarios are open in 2.6, including engine data, process quiescence, handoff and cleanup, copied sign-ins, and the standalone run command. |
| [rehome-upgrades](../../box/test/rehome-upgrades.test.ts) | 9 | M1 covers the separate machine kit projections. App preferences and list decoding, settings and projects, worktrees and Git, bots, group chats, letters and seen IDs, artifact rewrites and future app data remain 2.6. |
| [rehome-passwords](../../box/test/rehome-passwords.test.ts) and [bridge scenario](../../box/bridge-tests/rehome-passwords.test.js) | 2 | Mac handoff records and settling are open in 2.6. The actual saved sign-in transfer also needs the vault and Helper adapter in 5.5. |
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

- **2.6** still owns both Mac app imports, including the staged layout-1 work,
  source writer checks, copy and placement, database migration and validation,
  path and history repair, legacy app preferences, group chats, Mac letters,
  Keychain handover and setup conversion through `kuvo_setup`. It also owns
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
