# Rust updates coverage

Phase 7.2 is open. The first slice implements signed manifest admission, local
target selection and preparation, and preservation of the launch manifest.
A prepared release keeps its full `box.json` beside `bin`. The shell keeper can
restore the launch identity from `bin/.kuvo-release.json` when a TypeScript
stager moved only `bin`. A Rust manifest uses the five Rust target triples
under `packs`, with the existing signed `pack` field available to older readers.

## Checks added in 7.2

| Check | Boundary |
| --- | --- |
| U1 | `update --phase prepare` rejects a foreign key, a forged signature, and edited manifest bytes. |
| U2 | The same command rejects a changed pack while preserving the selected release. |
| U3 | The same command declines an older release and a release without `spreads: true`. |
| U4 | The release signer emits one signed descriptor per target. The Rust updater validates the local pack and engine, probes its programs, and can retry preparation while the old resident keeps serving. An explicit native-keeper selection starts and confirms the prepared release. |
| U5 | The shell keeper answers an SSH request using Rust after the previous stager's bin-only move. It restores `box.json` on disk. |
| U6 | `GET kuvo/update` reports the installed version, running engine, latest known version, and the keeper's failed trial. |
| U7 | The prepared-pack scenario stops its keeper and uninstalls the standalone computer. It retains the home, clears the desktop identity, and leaves the scratch account and its temporary directory unchanged. |

On macOS the update area passed five Rust cases and four shared TypeScript
cases. U4 rejected a damaged engine from a loopback feed, then prepared and
retried in 39.0 seconds. Explicit selection and confirmation took 1.2 seconds.
Both release directories used the same tested Rust executable with distinct
version and pack identities. The workspace compile check, the three shell
keeper route cases, and the two existing TypeScript signing and packaging
cases passed. The differential runner matched the failed-trial status response
and reported differences in the rest of the home inventory.

On native Windows the same five Rust cases passed. Preparation and retry took
12.3 seconds, and the keeper confirmed the selection in 1.5 seconds. U7 left
the account and its temporary directory unchanged. The scratch cross-build
used the llvm-mingw runtime DLL alongside the test executable.

On Ubuntu 24.04 under WSL2, the static x86_64 musl build passed all five Rust
cases. Preparation and retry took 8.2 seconds, and selection and confirmation
took 1.5 seconds. U7 passed there too. Lokum's Docker socket was unavailable
to the test account, so this run covers Linux and WSL.

## TypeScript reference cases

`Covered` means the exposed behaviour is checked. `Partial` records which
underlying Rust path is proven and what the update or installer still owes.
Bridge-specific checks only for unpublished TypeScript betas and desktop app
update prompts follow the switch-over scope in [the design](rust-box.md#4-switch-over).
The reference table maps 81 cases. Two are covered, 42 have partial proof,
three are specific to the TypeScript bridge or desktop updater, and 34 are open.

| Source and case | Status | Remaining owner |
| --- | --- | --- |
| [updates](../../box/bridge-tests/updates.test.js), version order | Partial. U3 checks an older release and the beta-to-final ordering in U4. Numeric components, same version with a new digest, and already-ahead peers need coverage. | 7.2 |
| Same, pinned signatures and signed bytes | Covered by U1. | — |
| Same, stale Mac launcher marks | Open. | 7.2 |
| Same, partial rollback reported once | Partial. 7.1 writes the refusal record and U6 reads it. The interrupted in-place restore and its one-time settlement need the predecessor fixture. | 7.2 |
| Same, signed release with Helper kept for peers | Partial. U4 covers signing and selecting a target. Target pack assembly, Helper validation, and retaining the signed feed and pack for peers are open. | 7.2, 6.2 |
| Same, full packaged contents | Partial. U4 checks the shipped executable and manifest in isolation. Complete per-target artifacts remain open. | 7.2 |
| [update-follows-peers](../../box/bridge-tests/update-follows-peers.test.js), installed version and app version | Open. | 7.2 |
| Same, desktop looks for its own update | TypeScript reference and desktop updater. | — |
| Same, six-hour check | Open. | 7.2 |
| [update-stops-nothing](../../box/bridge-tests/update-stops-nothing.test.js), compatible standby | TypeScript bridge-specific. Rust holder compatibility still needs fleet proof. | 4.1, 7.2 |
| Same, held turn survives a switch | Partial. 7.1 checked a shell selection and a surviving holder. Automatic fleet activation remains open. | 7.2 |
| Same, holder keeps running across engine change | Open. | 3.2, 4.1, 7.2 |
| Same, failed new bridge resumes a held turn | Partial. The 7.1 shell test covers the Rust keeper's rollback with a stand-in holder. Fleet failure announcement and retry remain open. | 7.2 |
| Same, one-time pre-holder bridge restart | TypeScript bridge-specific. The one-time TypeScript-to-Rust interruption follows section 4. | 9.1 |
| [update-engine-turn](../../box/bridge-tests/update-engine-turn.test.js), same engine | Partial. 3.2 covers resident restart. Full release activation with a turn still running remains open. | 7.2 |
| Same, background shell delays engine restart | Open. | 3.2, 7.2 |
| Same, engine-model bridge turn ends once before switch | Open. | 4.1, 7.2 |
| [update-from-before](../../box/bridge-tests/update-from-before.test.js), earlier updater through keeper | Partial. U5 proves the bin-only layout, and M5 in 2.6 covers opening older data. A real predecessor performing the update and entering Rust remains open. | 7.2, 9.3 |
| [update-real-keeper](../../box/bridge-tests/update-real-keeper.test.js), saved baseline layout | Partial. 7.1 checks the shell's layout judge and the startup fence. | 7.2 |
| Same, pre-marker updater commits and migrates | Open. | 7.2, 9.3 |
| Same, interrupted pre-marker updater restores baseline | Partial. 7.1 checks Rust rollback. The TypeScript-to-Rust transition follows the no-automatic-rollback decision in section 4, and the old rollback fixture still needs the released-predecessor run. | 7.2, 9.3 |
| [resident-update-pilot](../../box/bridge-tests/resident-update-pilot.test.js), sandboxed live turn survives both outcomes | Open. | 7.2, 9.3 |
| [fleet-switch](../../box/bridge-tests/fleet-switch.test.js), T1 prepare, commit, and offline catch-up | Open. | 7.2 |
| Same, T3 retry and five-minute limit | Open. | 7.2 |
| Same, ahead member offers its version | Open. | 7.2 |
| Same, Update Now chooses the immediate candidate | Open. | 7.2 |
| Same, progress summary remains while switching and after settlement | Open. | 7.2 |
| Same, row progress and retry | Open. | 7.2 |
| Same, pull progress never moves back | Open. | 7.2 |
| Same, member asks main to check | Open. | 7.2 |
| [own-home-mac-serve-update](../../box/bridge-tests/own-home-mac-serve-update.test.js), separate judge and trial cleanup | Open. | 7.2 |
| Same, Helper starts the new code after the resident exits | Partial. 7.1 dispatches Helper `serve` to a Rust pack. Live Helper update remains open. | 6.2, 7.2, 9.3 |
| [update-switch](../../box/test/update-switch.test.ts), broken route probe and beta.35 switch | Partial. U4 probes that a Rust binary starts with the engine pin. The internal route probe and TS beta switch remain open. | 7.2 |
| Same, continue only interrupted engine turn | Open. | 3.2, 4.1, 7.2 |
| Same, guard commits after selected resident answers | 7.1 keeper scenario covers this. | — |
| Same, resident switch preserves the engine and sandbox files | Partial. 7.1 keeps workers during the shell switch. The prepared-release engine and plugins need an end-to-end test. | 7.2 |
| [release-catalogs](../../box/test/release-catalogs.test.ts), integrations and popular servers in fixture | Open for Rust release builds. | 7.2 |
| Same, Mac integrations | Open for Rust release builds. | 7.2 |
| Same, legacy files byte-for-byte where preserved | Open for Rust release builds. | 7.2 |
| Same, files writer stays below destination | Partial. U4 prepares in a private home. Full release assembly and uninstall footprint need proof. | 7.2 |
| [add-uninstall](../../box/test/add-uninstall.test.ts), stopped uninstall deletes desktop identity | 2.3 U5 covers stopped standalone. SSH wrapper and live keeper path remain open. | 2.3, 7.2 |
| Same, incomplete desktop cleanup reported | Open. | 2.3, 7.2 |
| Same, failed departure keeps identity | Open. | 2.3 |
| [install-offer](../../box/test/install-offer.test.ts), last wipe approval follows old identity | Partial. 2.3 U8 covers detachment identity. Installer re-entry remains open. | 2.3, 6.2, 7.2 |
| Same, app member wipe and receipt | Open. | 2.3, 7.2 |
| [install](../../box/bridge-tests/install.test.js) 124, signed pack, engine, and fresh verdict | Partial. U1–U4 cover update admission and 2.6 C1 covers the read-only verdict. Installer admission and engine download remain open. | 7.2 |
| Same 148, 204, 267, installer after last wipe, stop without Helper, and completion outside App Support | Open as installer scenarios. See 2.3 U7–U8 for wipe and receipt checks. | 2.3, 6.2, 7.2 |
| Same 361, two installers prepare together | Open. | 7.2 |
| Same 379 and 387, refuse local-file engine and damaged pack or engine | Partial. U2 and U4 cover update pack and engine integrity. The installer entry remains open. | 7.2 |
| Same 418, 442, 451, existing home, fleet update instead of reinstall, and offline status | Open. | 7.2 |
| Same 468, 505, Mac new and interrupted install, Helper launch | Partial. 2.6 covers staged making. The installer handoff and real Helper remain open. | 6.2, 7.2 |
| Same 564, 594, 645, existing app's data moves, progress order, and legacy app kept | Partial. 2.6 A7 and C1 cover move and publication. The installer entry and Helper remain open. | 7.2 |
| Same 700, 725, launch retry after move and deferred key settlement | Partial. 2.6 covers resumable settlement with stand-ins. | 6.2, 7.2 |
| Same 750, two installations preserved | Partial. 2.6 covers the verdict and fence. Installer invocation remains open. | 7.2 |
| Same 766, Linux move refusal | Partial. Existing Rust move command refuses. The installer entry remains open. | 7.2 |
| Same 774 and 793, Mac app copy and case spelling | Partial. 2.6 checks the Mac command's matching home. The installer entry remains open. | 7.2 |
| Same 810, 875, 901, Linux installation, live-process fence, and restart after failure | Open at installer boundary. | 2.3, 7.2 |
| [install-verdict](../../box/test/install-verdict.test.ts) 32, 45, 57, 74, 81, 89, 96, 111, 128, 147, 154, 169 | Partial. All 12 verdict cases are mapped in 2.6 C1, M2, M3, M5, and A10. Helper readiness and restart (45, 57, 128), restarted fresh making (74), and Linux starter admission (169) remain open with the installer. | 6.2, 7.2 |

## Inherited cases and system proof

| Source | Status | Remaining owner |
| --- | --- | --- |
| [Keeper](rust-keeper-coverage.md) route, future update record, rollback status, and former release | U5 covers bin-only manifest recovery and U6 covers the failed-trial status. Future pack file-list versions are refused by the Rust reader. Former-release integration remains open. | 7.2 |
| [Store](rust-store-coverage.md) authority-format cases | Installer stdin/prefix before making, making marker, repair records, and former-release rollback still need full installer and updater proofs. | 2.3, 7.2 |
| [Adding](rust-adding-coverage.md) `computers-add.test.js` 302, 360, 397, 445, 2038, 2069 | Open. Cover update discovery, missing version, fallback address, retired app, and interrupted Helper reinstall. | 2.3, 2.4, 2.6, 6.2, 7.2 |
| [Move](rust-move-coverage.md) `rehome`, `rehome-run`, `folder-version` 287, 301, 326, and own-home Mac | Partial. 2.6 has released data and cutover tests with stand-ins. The installer entry, release trial and Helper launch remain open. | 4.1, 6.2, 7.2 |
| [Engine](rust-engine-coverage.md) inbox paging, incomplete inventory and ordered crash recovery | The existing inbox API is available. Its coordinated switch caller and release tests remain open. | 3.2, 7.2 |
| Same, engine swap holds, rollback, expiry, and downgrade fence | Open as release tests. | 3.2, 7.2 |
| [Backups](rust-backups-coverage.md) durable pre-switch backup and wipe generation check | Existing backup boundary. Call it from the TypeScript-to-Rust switch and prove interrupted retries. | 7.2, 9.1 |
| Rust-to-Rust live three-computer switch, running turn, offline return, and failover | Open. | 7.2 |
| Golden 0.7.2 and beta.1 through `kuvo-install`, and a TS-beta computer catching up after the switch | Open. | 7.2 |
| Uninstall footprint and Mac, Linux, WSL and native-Windows proof | Partial. U7 covers stopped standalone scratch installations on Mac, native Windows, and Linux musl under WSL. The live shared, Helper-owned and installer flows remain open. | 2.3, 7.2 |
