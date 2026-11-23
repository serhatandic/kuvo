# Rust storage coverage

Row 1.2 supplies the home layout and storage primitives. The row that owns a file supplies its
semantic reader, signature and body checks, retention policy, and mutations. Its reader must
apply the checks in [stored formats](stored-formats.md) before changing any records.

## Shared scenarios

| TypeScript scenario | Rust scenario or disposition |
| --- | --- |
| `format-version.test.ts` (3) | `storage`: ordered upgrades, legacy/additive decoding with unknown fields kept, version-specific dispatch and rejection of malformed or future versions. |
| `adopt-launch-lock.test.ts` (5 lock scenarios) | `storage`: live launch writer and older ownerless launcher block, dead holder is replaced with the current PID, an active breaker blocks, and exactly one claimant can take over a dead breaker. Windows checks process liveness because the old command-line roles are Unix scripts. |
| `adopt-launch-lock.test.ts` (1 CLI scenario) | `box adopt` through the legacy setting verbs, row 7.3. Uses the same guard tested above. |
| `work-answers.test.ts` (local lock case) | `storage` covers same-process async writer ordering even with zero cross-process wait. Session note behavior belongs to 5.1. |
| `data-declarations.test.ts` (4) | `storage`: independent required-kind and family inventories, external archive and upload policies, and Work Queue owner resolution. 104 data kinds, 55 families and 9 mutation lanes. |
| `folder-version.test.js` (newer and unreadable marker) | `stored-formats` conformance refuses them before startup writes. It also checks an inconsistent checkpoint and a fresh home reopened at 23. |
| `folder-version.test.js` (older, interrupted, layout 11 and 17 records, and sealing) | `storage` verifies older and interrupted checkpoints route to the move while invalid transitions leave their bytes intact. The retired numbered steps stay TypeScript-only. Row 2.6 owns the staged import, restart and persistence-before-final-checkpoint cases. Rows 2.1 and 9.1 own the already-converted setup check and rehearsal. |
| `folder-version.test.js` (bridge, keeper, engine launch and trial recovery) | Rust resident rejects before starting the engine. Separate bridge startup and TypeScript trial recovery stay TypeScript-only. Row 7.1 owns native keeper refusal and rollback. |
| `forward-version.test.ts` (4) | Direct and replacement bridge request/reply negotiation is retired with the standalone bridge. Revalidation of draft permissions and a turn's engine identity belong to 3.2 and 6.1. |
| `agents/store-format.test.ts` (2) | `stored-formats` covers a future checkpoint target. Legacy session promotion and refusal before overwrite/delete belong to 4.6. |
| `authority-readers.test.ts` (3) | Bridge HTTP negotiation stays TypeScript-only. Media framing belongs to 6.4, importer list gates to 2.6, and Helper rendezvous to 6.2. `storage` exercises their shared list/object checks. |
| `windows-primitives.test.ts` (9) | Paths, process identity and private IPC use the row 1.3 `kuvo_os` scenarios. Rust layout paths reuse its account/profile resolution. |
| `format-versions.test.js` (4) | Mail and HTTP/SSH version gates are in 1.4 and 1.5. Row 2.1 owns the link file refusal and row 5.2 owns the ledger refusal. |
| `authority-formats.test.js` (12) | Seed, SSH setup and installer code (first three) belong to 2.3. Installer stdin, repair, making markers and former-release rollback belong to 7.1 and 7.2. Helper recovery/stdin belong to 6.2 and 7.1. The retired Mac serve path stays TypeScript-only. Supervision readers belong to 3.2, 4.1 and 7.1. Rehome action/state belongs to 2.6. |
| `rehome-upgrades.test.ts` (3) | Plist inputs, stamped folder lists, and preservation of unsupported artifact logs during import belong to 2.6. |

The shared Rust scenarios also exercise no-clobber key publication, Unix private modes and
symlink replacement, file identity for indexing, SQLite WAL/busy settings and transactional
schema adoption, and two-process Windows sharing behavior. Search and Insights still own
their respective schemas (3.2 and 5.4).

## File owner scenarios

Tests that use the shared helpers as setup still belong to the operation they exercise.
The following phase rows own the family-specific version refusals and transitions.

| Owner row | TypeScript inputs |
| --- | --- |
| 2.1, 2.2 | `setup-shared`, `setup-lists`, `setup-handover`, `setup-judge`, `devices-two-boxes`, `peers`, `personalities-box` — setup, personalities, peer directory and trust. |
| 2.3 | `add-seed`, `installer-enrollment`, `install-offer` — seed receipts and source or credential selection. |
| 2.4, 5.2 | `moves-safety`, `forward-evidence`, `offline-merge`, `queue-items`, `work-queue-migrate` — handoffs, future move journals/manifests and decisions, deletion and departure fences, ledger/schema gating. |
| 2.5 | `computer-backups` — refuse mixed layouts, preserve and replay the right records when backing up and restoring. |
| 3.2, 5.1 | `queue-items`, `draft-orphan`, `session-actions`, `ordinary-note-routes` — pending requests, portable notes and waiting queues. |
| 4.1–4.6 | `accounts`, `added-account-route`, `agents/{acp,codex,store-format}` — account metadata, runtime records and session stores. |
| 5.4 | Insights and usage preferences; keep the live schema upgrade in its owner, using the shared SQLite opener. |
| 5.5 | `artifacts`, `artifacts-route`, `integrations-upgrade`, `marketplace`, `skill-removal` — replay/compaction and refusal before rewriting log/settings bytes. |
| 6.1–6.4 | `models-starred`, `settings-vault`, `git-settings`, `git-words`, `projects-worktrees`, `browser-keeper`, `browser-websocket-review` — settings, vault envelopes and activity, Git and project lists, browser records and exchanges. |
| 7.1–7.3 | `engine-swap`, `fleet-switch`, `install`, `keeper-rollback`, `own-box-helper`, `own-home-mac-serve-update`, `ssh-pipe`, `ssh-route` — keeper and release changes, installation and older SSH clients. |
| Retired | Standalone agents bridge forwarding and legacy typed commands have no Rust storage caller. |

## Golden and binary checks

Run the binary gate with `KUVO_CONFORMANCE_AREA=stored-formats` and each selected binary.
Generate a scratch current-beta oracle with
`node protocol/conformance/compat/storage-golden.mjs`. Pass the printed path as
`STORE_GOLDEN` to `scripts/cargo-box.sh test -p kuvo_store --test golden -- --ignored`.
The test covers 55 families and 61 representative files, compares 38 JSON envelopes against
the TypeScript primitives, and reads the two SQLite samples. The signed documents remain
encoded on disk. The storage test compares the inner stamp and decoded fields and checks the
outer bytes separately.

The tagged-home and holder conformance cases also exercise work outside the store.
The released home move is 2.6, the released SSH behaviors are 7.3, and holder adoption
is 4.1. The current-beta end-to-end case expects projects (6.3), personalities and setup
(2.1), Work Queue (5.2), bots (5.1), and expired delivery processing (2.4).

| Check | Result |
| --- | --- |
| Shared Rust storage scenarios | 17 passed on macOS, Linux (lokum with 2 CPUs and 2 GiB), WSL2 (Ubuntu 24.04), and native Windows. On Windows, the two-process probe covered a delete-sharing reader and a non-delete-sharing reader. |
| Current-beta oracle | Passed on all four platforms. Independent TypeScript fixture generations matched across 229 entries with the fixture's prescribed normalization. |
| Stored formats through the binary | 4 passed on TypeScript and 4 on Rust. |
| Compat and holder through the binary | TypeScript passed 5 compat and 1 holder case. Rust reached the current layout and then received 404 from Projects. The four released-home cases need 2.6 and 7.3. The holder command needs 4.1. |
| Offline golden-home differential | The layout checkpoint matched. The three requests (`kuvo/info`, `kuvo/setup`, `kuvo/projects`) and the post-run inventory still differ. The report includes release/feature negotiation (9.1), setup/link state (2.1), projects (6.3), expired letters (2.4), other service runtime files, and independently generated socket keys. |

There are no remaining shared-storage cases assigned to 1.2. End-to-end parity of the
feature-owned cases above is part of their implementation and the switch rehearsal (9.3).
