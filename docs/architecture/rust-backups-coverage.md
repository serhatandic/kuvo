# Rust backups coverage

Row 2.5 owns the computer archive, destinations and recovery. “Partial” means
the Rust scenario verifies only the parts named below.

**Dropped: TS-beta interop (user 11-23).** The TypeScript betas stay in the
pre-switch copy. Rust-to-Rust recovery is the acceptance test for disk, SSH
and Relay. The shared conformance and differential scenarios still exercise
the archive and restore contract against TypeScript.

## Computer backups

All 25 cases in [computer-backups.test.js](../../box/bridge-tests/computer-backups.test.js)
are mapped here. Rust evidence comes from [backup service scenarios](../../box/crates/kuvo_setup/tests/backups.rs)
and [resident conformance](../../protocol/conformance/backups.test.mjs).

| TS case | Rust status and remaining owner |
|---|---|
| Restore verified data after the old disk is erased | Partial. Rust-to-Rust disk recovery compares each seeded file, including binary data and pending work. The larger fixture and journal families remain in 2.5. |
| Wakes stay paused and pending work never replays | Partial. Pause markers precede schedule placement. Pending spools and agent inboxes stay held. The live wake consumer and held-message review need 5.1 and 3.2 with 2.5. |
| An update cannot publish a mixed-layout backup | Covered by an interrupted layout and update-trial test. The fleet-switch hook remains in 7.2 and 9.1. |
| Keep 30 daily points and expire old pre-operation points | Covered by the daily-retention scenario. The resident erases the resulting points after readback. |
| SSH deposits ciphertext on the chosen member and verifies readback | Partial. Rust-to-Rust fake SSH checks bytes and readback. The full enrollment and login-agent cases need 2.3, 1.3 and 6.2. |
| Relay survives source loss through accepted replacement membership | Partial. Real residents deposit, revoke, enroll, match and restore. The receiver scenario resumes a legacy reservation and replays append and commit. Bad clocks, quota edges, and crash-cut fixtures remain in 2.5. |
| Restore matches a replacement and keeps the old key revoked | Partial. Relay recovery checks fresh authority. History conflicts and the combined wipe replay need 2.3 and 2.5. |
| Separate vault key and no inherited grants | Backend seam in 2.5. Real backends and route wiring remain in 6.2 and 2.5. |
| Preview a destination before enabling and require the local app | Covered by the conformance route test. |
| A moved conversation stays fenced on its replacement | Partial. Local move fences and peer announcements are checked. Live owner writes and replay remain in 2.4 and 3.2. |
| An item with a newer departure is held with its files | Partial. Recovery reads old and local departure receipts. Live peer item claims and the paired fixture need 5.2 and 2.5. |
| Keep attachments named in a Work Queue item | Covered by the temporary-looking payload and soft-delete fixture. |
| Reject changed attachment bytes and a short Work Queue payload | Partial. Size and saved digest are checked. Pre-wipe selected hashes and receipt commit need 2.3 and 2.5. |
| Report omissions and retain soft-deleted undo bytes | Partial. The queue test verifies undo bytes. Legacy omission and visible review remain in 2.5 and 5.2. |
| Keep or discard held Work Queue items | Open in 5.2 with 2.5 for live queue writes and review receipts. |
| Inventory the upgraded home and external roots | Partial. Declared roots are inventoried. Isolated upgrade and external-root fixture need 2.5 and 2.6. |
| Paired backup cannot reactivate a handed-off item | Partial. Match, move fences and local handoff/departure are checked. Peer item claims and replay need 5.2 and 2.4. |
| Group runner holds meetings, resumed turns and waiting work | Partial. Restore marks the group held and drops the previous review receipt. The runner and review need 5.3. |
| Group memory points to the replacement transcript | Partial. Verified recap paths are rewritten. Full group history and review fixture need 5.3 and 2.5. |
| Legacy archive never installs Full Access | Covered by old-point conformance and the Rust disk test. |
| Retain a populated replacement DB and hold its snapshot | Partial. Bootstrap and conflict handling are implemented. The WAL scenario covers scrub. Populated replacement fixture remains in 2.5. |
| A handoff cannot land between the two queue reads | Partial. Queue capture runs under the legacy queue lock. The handoff race needs the writer in 5.2. |
| Publish to a disk that rejects hard links without overwriting | Partial. Publication uses rename under the publishing lock. A no-hard-link filesystem and concurrent retry fixture remain in 2.5. |
| Recover opted-in project modes, empty directories and safe links | Partial. Conformance verifies executable mode, empty directories, safe links and an unsafe-link omission. Safe Git preview and native Windows link verification need 6.3 and 2.5. |
| Reopen managed bytes at changed paths and report pruned versions | Partial. Restore allocates attachment paths and rewrites known transcript fields and the archive inventory reports pruned versions. The full fixture needs 3.2, 5.5 and 2.5. |

## Cases assigned by other maps

| TS cases | Rust status and gap owner |
|---|---|
| [setup-shared](../../box/test/setup-shared.test.ts) label preview and acceptance | Open in 5.2. The Work Queue owns the label library. |
| [setup-handover-shared](../../box/bridge-tests/setup-handover-shared.test.js) restore 344 labels, old primary, unreachable primary | Open, all three in 5.2. |
| Paired backup-health route in [setup-handover-shared](../../box/bridge-tests/setup-handover-shared.test.js) | Partial. Local admission and the limited response are tested. Established device grant and revocation fixture remains in 2.2 and 2.5. |
| [setup-handover-removal](../../box/bridge-tests/setup-handover-removal.test.js) offline wipe, three revoked Relay-only exports, failed pre-wipe backup | Open, all five in 2.3 and 2.5. Mac secrets also need 6.2. |
| [setup-handover-export](../../box/bridge-tests/setup-handover-export.test.js) move-all wipe, offline moves/export, encrypted export restore, missing attachments, legacy readiness, failed and unreachable export, preparation across handover | Open, all eight in 2.3 and 2.5. Move admission and selected bytes need 2.4 and 3.2. |
| [setup-handover-wipe](../../box/bridge-tests/setup-handover-wipe.test.js) revised queue plan, removal preview, unreadable bot, revised export, WAL generation, newer work, two attachment revisions, lost completion | Open, all nine in 2.3 and 2.5. Moves and queue state need 2.4 and 5.2. |
| Retired Insights origin into fresh and populated indexes in [setup-handover-recovery](../../box/bridge-tests/setup-handover-recovery.test.js) | Partial, both cases. Merge uses accepted origins and preserves local cursors. Setup history and the real fixture need 2.3, 5.4 and 2.5. |
| Mixed-build update, lost move reply and restore in [setup-handover-recovery](../../box/bridge-tests/setup-handover-recovery.test.js) | Open in 9.3 with 2.4 and 2.5. The TS beta switch is one way. |
| Last app-owned wipe preparation in [install](../../box/bridge-tests/install.test.js) | Open in 2.3 and 2.5. |
| [installer-enrollment](../../box/bridge-tests/installer-enrollment.test.js) verified Relay backup after joining | Covered by the Relay conformance scenario. |
| [Store map](rust-store-coverage.md) mixed layouts and replay | Partial. Interrupted layout and paused recovery are checked. Forwarding evidence and installed layout upgrades need 2.4 and 2.6. |
| [Setup map](rust-setup-coverage.md) approval and chats root interpretation | Partial. Approval carries only the last mode and its timestamp. The inventory has no separate chats root. External Mac roots and project boundaries need 2.5 and 2.6. |
| [branch-backups](../../box/test/branch-backups.test.ts) branches, stashes, repository loss, worktrees, pruning and Git safeguards | All 14 belong to 6.3. Computer backups carry the declared Git preferences file. |

## Stored formats and integration

- Version-one AES-256-GCM binds the source and point. RustCrypto CTR and GHASH stream one message, and the reader authenticates it before parsing.
- Relay backup upload reservations keep their version, source, point, size and digest in the receiving account. The receiver rechecks accepted membership, and replay checks publication against the size and digest. The receiver test replays a legacy reservation and rejects a late commit after erase. Replay with the generated golden home remains in 2.5.
- The setup actor owns replacement matches and approval. Backup also checks whether the accepted ownership view changed before placing files.
- Row 6.2 must implement VaultBackend with unlock and tamper status, generation checks, item metadata and value reads via Helper Keychain on Mac and the unlocked wrapped-key file store elsewhere. Import uses the ordinary new-item writer and fresh IDs without inherited grants or usage. The local vault routes then connect to these operations with 2.5.
- Pre-switch export and update/wipe hooks remain with 2.5, 2.3, 7.2 and 9.1. Wipe can authenticate a point and commit its proof under the backup lock. The pre-switch copy remains the manual recovery path. The pending list reads the held records. Resubmission and queue review receipts need the queue and engine owners in 5.2 and 3.2.

## Verification

| System | Rust backup service | Resident backup conformance |
|---|---:|---:|
| macOS | 7 passed | Rust 4 passed, TypeScript 4 passed |
| Linux on Lokum | 7 passed | Rust 4 passed |
| WSL2 Ubuntu 24.04 | 7 passed | Rust 4 passed |
| Native Windows | 6 passed | Rust 3 passed, 1 POSIX project case skipped |

Rust-to-Rust disk and fake SSH scenarios compare the seeded bytes after the
source home is moved out of reach. The Relay scenario sends through the real
residents and test relay, revokes the original source and restores through
fresh replacement membership. The fake SSH service test uses a POSIX shell.
Linux and WSL used static musl binaries built on macOS.

The differential runner matched all five backup requests and every restored
file in its disk fixture. The whole-home comparison still returned 80 startup
differences from independent identities and services outside the backup
requests. These included Insights, artifacts, vault, setup startup and the
engine plugin. The box workspace compile check passed.
