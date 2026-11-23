# Rust backups coverage

Row 2.5 owns the computer archive, destinations and recovery. “Partial” means
the Rust scenario verifies only the parts named below.

**dropped: TS-beta interop (user 11-23)**. The TypeScript betas stay in the
pre-switch copy. Rust-to-Rust recovery is the acceptance test for disk, SSH
and Relay. The shared conformance and differential scenarios still exercise
the archive and restore contract against TypeScript.

## Computer backups

All 25 cases in [computer-backups.test.js](../../box/bridge-tests/computer-backups.test.js)
are mapped here. Rust evidence comes from [backup service scenarios](../../box/crates/kuvo_setup/tests/backups.rs)
and [resident conformance](../../protocol/conformance/backups.test.mjs). The
map has 12 covered cases and 13 partial cases with named owners.

| TS case | Rust status and remaining owner |
|---|---|
| Restore verified data after the old disk is erased | Covered. Disk and SSH compare each seeded file after source loss, including bot memory, group history, agent history and usage, Work Queue journals, move decisions, artifacts and binary attachments. Journal evidence recovers only within its authority. |
| Wakes stay paused and pending work never replays | Partial. Pause markers precede schedule placement. Pending spools and agent inboxes stay held and the private route lists waiting work. The live wake consumer and held-message resubmission need 5.1 and 3.2. |
| An update cannot publish a mixed-layout backup | Covered by an interrupted layout and update-trial test. The fleet-switch hook remains in 7.2 and 9.1. |
| Keep 30 daily points and expire old pre-operation points | Covered by the daily-retention scenario. The resident erases the resulting points after readback. |
| SSH deposits ciphertext on the chosen member and verifies readback | Partial. Rust-to-Rust fake SSH checks bytes and readback. The full enrollment and login-agent cases need 2.3, 1.3 and 6.2. |
| Relay survives source loss through accepted replacement membership | Partial. Real residents deposit, revoke, enroll, match and restore. An independent reader authenticates the envelope. Receiver scenarios cover both clock edges, quota, stale reservations, lost publication and late commit. The revoked export workflow and Mac identity reacquisition need 2.3 and 6.2. |
| Restore matches a replacement and keeps the old key revoked | Partial. Relay recovery checks fresh authority. Combined history and wipe replay need 2.3. |
| Separate vault key and no inherited grants | Partial. Local-only routes take the vault holder seam. A fixture vault checks that the computer key fails and that imports omit IDs, grants and usage. Real Helper and wrapped-key backends need 6.2. |
| Preview a destination before enabling and require the local app | Covered by the conformance route test. |
| A moved conversation stays fenced on its replacement | Partial. Local move fences and peer announcements are checked. Live owner writes and replay remain in 2.4 and 3.2. |
| An item with a newer departure is held with its files | Partial. Recovery reads old and local departure receipts and reports held bytes. Live peer item claims need 5.2. |
| Keep attachments named in a Work Queue item | Covered by the temporary-looking payload and soft-delete fixture. |
| Reject changed attachment bytes and a short Work Queue payload | Covered. The queue capture checks recorded sizes and hashes. Pre-wipe capture refuses added, missing or revised attachments, and only verified readback commits the receipt. The live removal workflow needs 2.3. |
| Report omissions and retain soft-deleted undo bytes | Covered. Both residents report missing and short legacy attachments, keep the soft-deleted item's bytes and list a held handoff with its attachment availability. |
| Keep or discard held Work Queue items | Partial. The private list shows reviewable records and their available bytes. The live writer and review receipts need 5.2. |
| Inventory the upgraded home and external roots | Partial. The service recovers external Mac attachment bytes into a new root and rewrites the transcript. Installed legacy layout upgrade remains in 2.6. |
| Paired backup cannot reactivate a handed-off item | Partial. Match, move fences and local handoff/departure are checked. Peer item claims and replay need 5.2 and 2.4. |
| Group runner holds meetings, resumed turns and waiting work | Partial. Restore marks the group held and drops the previous review receipt. The runner and review need 5.3. |
| Group memory points to the replacement transcript | Covered. The fixture compares private transcript bytes, the rewritten recap path and the forgotten recap marker. Group review remains in 5.3. |
| Legacy archive never installs Full Access | Covered by old-point conformance and the Rust disk test. |
| Retain a populated replacement DB and hold its snapshot | Covered. Both residents keep the populated replacement and hold the archived DB. The scenario reads the original row and the held row through SQLite. |
| A handoff cannot land between the two queue reads | Partial. Queue capture runs under the legacy queue lock. The handoff race needs the writer in 5.2. |
| Publish to a disk that rejects hard links without overwriting | Covered. Concurrent attempts keep one exact ciphertext on the native filesystem and on a scratch ExFAT volume. |
| Recover opted-in project modes, empty directories and safe links | Covered. Conformance checks modes on POSIX and tests safe links, empty directories and unsafe-link omissions through Relay. Safe Git preview belongs to 6.3. |
| Reopen managed bytes at changed paths and report pruned versions | Partial. A round trip checks redirected, occupied and expired attachment paths, retained artifact bytes and a pruned version report. The full native transcript and artifact fixture needs 3.2 and 5.5. |

## Cases assigned by other maps

| TS cases | Rust status and gap owner |
|---|---|
| [setup-shared](../../box/test/setup-shared.test.ts) label preview and acceptance | Open in 5.2. The Work Queue owns the label library. |
| [setup-handover-shared](../../box/bridge-tests/setup-handover-shared.test.js) restore 344 labels, old primary, unreachable primary | Open, all three in 5.2. |
| Paired backup-health route in [setup-handover-shared](../../box/bridge-tests/setup-handover-shared.test.js) | Partial. Both residents project only the health fields even with private, opaque identifiers. Established device grant and revocation over the device transport remain in 2.2. |
| [setup-handover-removal](../../box/bridge-tests/setup-handover-removal.test.js) offline wipe, three revoked Relay-only exports, failed pre-wipe backup | Open in 2.3. The backup hooks verify the exact point under lock. Mac secrets also need 6.2. |
| [setup-handover-export](../../box/bridge-tests/setup-handover-export.test.js) move-all wipe, offline moves/export, encrypted export restore, missing attachments, legacy readiness, failed and unreachable export, preparation across handover | Partial, all eight. Encrypted export, selected byte verification and retry are covered through the backup service. Signed preparation and delivery need 2.3, move admission and inventory need 2.4 and 3.2. |
| [setup-handover-wipe](../../box/bridge-tests/setup-handover-wipe.test.js) revised queue plan, removal preview, unreadable bot, revised export, WAL generation, newer work, two attachment revisions, lost completion | Partial, all nine. The backup side rejects revised bytes and verifies before committing. Removal revisions, generation and completion need 2.3, moves and queue state need 2.4 and 5.2. |
| Retired Insights origin into fresh and populated indexes in [setup-handover-recovery](../../box/bridge-tests/setup-handover-recovery.test.js) | Partial, both cases. Merge uses accepted origins and preserves local cursors. Setup history and the real fixture need 2.3 and 5.4. |
| Mixed-build update, lost move reply and restore in [setup-handover-recovery](../../box/bridge-tests/setup-handover-recovery.test.js) | Open in 9.3 with 2.4. The TS beta switch is one way. |
| Last app-owned wipe preparation in [install](../../box/bridge-tests/install.test.js) | Open in 2.3 and 6.2. The backup service exposes readback with a receipt callback. |
| [installer-enrollment](../../box/bridge-tests/installer-enrollment.test.js) verified Relay backup after joining | Covered by the Relay conformance scenario. |
| [Store map](rust-store-coverage.md) mixed layouts and replay | Partial. Interrupted layout and paused recovery are checked. Forwarding evidence and installed layout upgrades need 2.4 and 2.6. |
| [Setup map](rust-setup-coverage.md) approval and chats root interpretation | Covered. Approval carries the last mode and its timestamp. The inventory uses the same declared roots as TypeScript and has no separate chats root. Installed legacy root migration needs 2.6. |
| [branch-backups](../../box/test/branch-backups.test.ts) branches, stashes, repository loss, worktrees, pruning and Git safeguards | All 14 belong to 6.3. Computer backups carry the declared Git preferences file. |

## Stored formats and integration

- Version-one AES-256-GCM binds the source and point. RustCrypto CTR and GHASH stream one message, and the reader authenticates it before parsing.
- Relay backup upload reservations keep their version, source, point, size and digest in the receiving account. The receiver rechecks accepted membership, and replay checks publication against the size and digest. The focused golden replay reads the current generated archive envelope and resumes and commits the generated upload reservation.
- The setup actor owns replacement matches and approval. Backup also checks whether the accepted ownership view changed before placing files.
- The resident takes an optional VaultBackend. Row 6.2 supplies holder state, unlock and tamper status, generations, item metadata and value reads via Helper Keychain on Mac and the unlocked wrapped-key file store elsewhere. Import uses the ordinary new-item writer and fresh IDs.
- The pre-switch hook publishes a complete quiesced home at `backups/before-rust` and keeps it on retry. Phase 9.1 calls the hook after stopping the beta. Wipe export binds the selected attachment hashes and reopens the point before committing its proof under the backup lock. Phase 2.3 owns the signed preparation, source-generation scan, delivery and wipe itself. The fleet update caller remains in 7.2.

## Verification

| System | Rust backup service | Resident backup conformance |
|---|---:|---:|
| macOS | 14 passed, plus the clock and golden-envelope checks | Rust 7 passed, TypeScript 7 passed |
| Linux under WSL2 Ubuntu 24.04 | 14 passed | Rust 7 passed |
| Native Windows | 13 passed | Rust 6 passed, 1 POSIX device fixture skipped |

Rust-to-Rust disk, fake SSH and Relay scenarios compare seeded files after
source loss. The Relay scenario revokes the original source and restores through
fresh replacement membership. Node's AES-GCM reader verifies the archive
written by each resident. Native Windows verifies project bytes and safe
links over Relay. The fake SSH host uses a POSIX shell. The disk publication
test also passed on ExFAT after confirming that hard links were unavailable.
The Linux run used static musl binaries built on macOS and tested under WSL.
Lokum verification is part of the lead's batched Rust-area run.

The differential runner matched all seven backup responses and every restored
fixture path, including held queue and journal records. The raw comparison
reported 78 differences outside those paths, including independent setup
identities and startup state for Insights, artifacts, vault, the Work Queue
and the engine plugin. The box workspace compile check passed.

The focused golden checks use a scratch home from `generateCurrent`. The
all-family oracle currently stops on the missing `Retained work after unlink`
table entry, owned by 2.3.
