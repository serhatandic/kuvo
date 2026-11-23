# Rust keeper coverage

Row 7.1 covers the process that supervises the resident, the release selection and trial,
and the shell launch path that remains in use on Mac and Linux. The process tests use the
real `kuvo-box keep`, `serve`, and `kuvo-start` in disposable homes. They live in
[keeper.rs](../../box/crates/kuvo-box/tests/keeper.rs) because they drive binaries and
OS process lifetimes. Route conformance runs the resident's protocol separately.

The native [holder probe](../../box/crates/kuvo-box/tests/fixtures/keeper-holder.ps1)
uses the real Rust keeper for both handovers and a stand-in resident to launch
[a separate holder](../../box/crates/kuvo-box/tests/fixtures/keeper-holder.c).
Build the fixture with llvm-mingw and pass its executable, the Rust executable,
and a scratch folder to the probe. The real Rust holder consumer remains 4.1.

The six keeper process tests passed on Mac and as static Linux binaries on WSL2.
Native Windows passed the four applicable keeper process tests and both holder
transitions in the probe. The selector test keeps its old Windows file handle open
through both selection and rollback. The 21 targeted TypeScript scenarios and
the Rust workspace compile check also passed.

## Selection and process ownership

- A staged target is `versions/<version>-<12-digit-pack-digest>/bin`. A Rust release
  carries a `box/1` manifest in `box.json` beside `bin` with `launch: "rust"`.
  The shell reads that field through its selected `bin`. Packaging, signatures and
  preserving the manifest while staging via the preceding TypeScript release are 7.2.
- `kuvo-box keep select <target>` publishes `active-release` as a plain file, using
  the store's durable replace operation. Before replacing it, the keeper writes
  `run/previous` using the same target spelling as the shell. Windows replacement
  falls back to `FileRenameInfoEx` with POSIX replace flags when `MoveFileExW` cannot
  replace an open selector. A reader sharing delete keeps its existing view.
- Confirmation requires the selected resident's authenticated `kuvo/info` response
  with its own generation and `run/link.version` build. At 120 seconds a failed trial
  records the same `update-cannot` refusal as the shell and switches the code back.
  A private rollback marker makes that decision replayable across an interruption.
- Unix uses an owned process group while the keeper is alive, and the next keeper
  validates the executable, PID and start identity before adopting the resident.
  Windows uses a named kill-on-close Job Object. The resident's independent workers
  break away from that job. `keep handover` is started by the successor. It opens the
  existing job before asking the predecessor to leave, and then holds the same
  resident. An abrupt keeper exit closes the last job handle so the resident
  restarts under the same selector and independent holders can be taken up again.
- `keep pause` and `keep off` publish the same markers as the shell. A new start
  clears pause. Off removes legacy demand marks and remains in force. Unix retains
  the Mac-only rule for pause. Native Windows offers the local pause and resume
  lifecycle to its starter. Registering that starter is 8.1.

## Source scenarios

| TypeScript source and case | Status and evidence | Remaining owner |
| --- | --- | --- |
| `keeper-pause.test.js`: pause until next Mac start | Keeper process case tests pause, actual resident exit, and restart. The existing shell case covers engine and bridge stop on its launch path. | 4.1 for real Rust holders |
| Same: refuse pause on a Unix machine box | Keeper process case checks refusal and absence of the marker. Existing shell case stays. | — |
| `keeper-route.test.js`: chosen engine and engine rollback pin, even with a newer one installed | Existing shell test. Manifest case adds Rust `route`, `route-to`, and foreground helper `serve`, plus nested `launch` ignored. | 7.2 for published packs |
| `keeper-rollback.test.js`: saved rollback restores code and keeps writes | Keeper trial case proves the same outcome using selector replacement. The historical in-place restore stays in the shell. | — |
| Same: three missing old trial judges, watchdog rollback | Shell case continues. The Rust keeper owns the 120-second version trial and durably replays its rollback decision. | — |
| Same: stop old Mac server for the Helper to restart, without rollback | Existing shell case. The Rust shell case stops the old Rust resident and confirms a different process and generation. | 6.2 and 9.3 for the signed Helper path |
| Same: stopped engine can no longer be adopted from its start records | Shell case continues. Engine lifecycle and its records belong to `kuvo_engine`. | 3.1 / 3.2 |
| Same: stale Mac visit is preserved while its watchdog mark is removed | Existing shell case for 0.7. | 7.3 for released Mac compatibility of the Rust keeper |
| Same: three failed attempts to restore files, report partial restore | Existing shell case. A selector replaces one file atomically, so it cannot leave files from both releases. | — |
| Same: concurrent rollback leaves the other attempt alone | Shell case and selector process case covering a competing request during trial. | — |
| Same: future update record is held before starting old code | Existing shell case. Future selector or release records fail parsing. | 7.2 for its update-state reader |
| `resident-supervision.test.js`: one resident, readiness generation, and off | Keeper process cases cover the owned resident, generation readiness, pause, off, Unix re-adoption, and Windows planned handover and crash recovery. Rust shell case covers Rust-to-Rust selection without starting split workers. | — |
| `bridge.test.js`: `run/link.needed` when paired or pairing, cleared when expired | Existing bridge case tests producer and removal. Legacy shell process case proves the marker starts the split worker. The Rust resident is started even on an unpaired computer. | 4.1 and 6.4 for other legacy marker consumers |
| Same: restart missing keeper and use saved launcher when a released Mac overwrites it | Existing TypeScript compatibility case; shell keeps its saved launcher behavior. The Rust keeper takes local ownership via its lock and adopts a matching resident when it can still be alive. | 7.3 for released Mac repair, 8.1 for native startup registration |
| `setup-mac.test.js`: old signed setup cannot be overwritten via SSH while the keeper is paused | Existing old shell/SSH case. The write authority and cross-process mixed-version writer handoff require the released Mac rehearsal. | 2.3, 7.3, 9.1 and 9.3 |
| `updates.test.js`: a partial in-place restore is reported once | Existing TypeScript record case. Rust records timestamp, failed version and pack digest in `update-cannot`, and leaves the earlier code selected. | 7.2 for the update status consumer |
| Same: version order, signature requirement, stale Mac launcher marks, packaging, full release contents | These run at update/staging time; the Rust keeper accepts an already staged target. | 7.2 |
| `folder-version.test.js`: startup refuses newer layout, and a failed legacy trial can restore the compatible old code | Rust process case checks no server record and unchanged future marker. Old rollback remains covered in the shell. | 4.1 for the old bridge gate, 7.2 for pack-level integration |
| `authority-formats.test.js`: interrupted machine-helper recovery, retired Mac served marker, former-release rollback | Existing shell recovery path if no Rust manifest. The Rust process test replays a durable selector rollback and confirms no data rollback. | 6.2 for the Helper's records, 7.2 for the installer and former-release fixture |
| `own-box-engine.test.js`: Helper records the launch environment, keeps the recorded note, normalizes trailing HOME slash, or refuses unconfigured legacy launch | Existing shell cases. Rust uses the operating system process start identity and existing executable lookup. | 6.2 / 7.3 for legacy Helper and installation integration |
| `rust-move-coverage.md`, `rust-setup-coverage.md`, `rust-adding-coverage.md`: `run/link.version` and `run/link.needed` | Keeper checks the live build for confirmation; process tests assert the plain build and the shell observes legacy demand. | 4.1 for `kuvo/info` availability of actual agents |

## Installation cases assigned jointly with the keeper

The full installer and pack workflow is 7.2. These cases in
[rust-adding-coverage.md](rust-adding-coverage.md) were provisionally listed against
7.1 before separating selection after staging from the installer:

| Case | Status and remaining owner |
| --- | --- |
| `computers-add.test.js` lines 2038, 2069: 503 during Darwin reinstall, interrupted fresh install finishes after readiness | Open in 2.3 and 7.2, with the signed Helper in 6.2. Kernel/HTTP-level readiness is covered here. |
| `install.test.js` lines 124, 148, 361: fresh verdict, last wipe, simultaneous prep | Open in 7.2, with wipe eligibility in 2.3. |
| Same lines 379, 387: reject a local-file engine and a damaged pack before execution | Open in 7.2 for digest/signature admission. The keeper refuses invalid selection targets and rolls back valid staged targets that fail at runtime. |
| Same lines 418, 451: existing home and engine cleanup, offline status | Open in 7.2. |
| Same lines 468, 505, 774, 793: new or interrupted Mac install, reject a copy, canonical Mac HOME | Open in 6.2 and 7.2. |
| Same lines 810, 875, 901: new/interrupted Linux install and live-process refusal | Open in 2.3 and 7.2. |
| `install-verdict.test.ts` lines 45, 74, 169: Mac Helper admission, fresh install cut short, Linux starter | Open in 6.2 and 7.2. |
| `authority-formats.test.js` line 69: older and future installer-code prefixes before making the home | Join rejects the prefixes in conformance C3. Full installer prep remains 2.3 and 7.2. |
