# Rust updates coverage

Phase 7.2 is open. The keeper verifies signed manifests, prepares the local
target, and switches through the existing native selector or Unix shell keeper.
The resident uses the setup lease to decide when the reachable computers are
ready. A behind computer can take the main computer's committed release from
its signed announcement.
A prepared release keeps its full `box.json` beside `bin`. The shell keeper can
restore the launch identity from `bin/.kuvo-release.json` when a TypeScript
stager moved only `bin`. A Rust manifest uses the five Rust target triples
under `packs`, with the existing signed `pack` field available to older readers.

## Checks added in 7.2

| Check | Boundary |
| --- | --- |
| U1 | `update --phase prepare` rejects a foreign key, a forged signature, and edited manifest bytes. |
| U2 | The same command rejects a changed pack while preserving the selected release. |
| U3 | The same command declines an older release, a same-version pack with a different digest, and a release without `spreads: true`. Numeric minor and beta versions compare in release order. |
| U4 | The release signer emits one signed descriptor per target. The Rust updater validates the local pack and engine, probes its programs, and can retry preparation while the old resident keeps serving. An explicit native-keeper selection starts and confirms the prepared release. |
| U5 | The shell keeper answers an SSH request using Rust after the previous stager's bin-only move. It restores `box.json` on disk. |
| U6 | `GET kuvo/update` reports the installed version, running engine, latest known version, and the keeper's failed trial. |
| U7 | The prepared-pack scenario stops its keeper and uninstalls the standalone computer. It retains the home, clears the desktop identity, and leaves the scratch account and its temporary directory unchanged. |
| U8 | Two Rust computers prepare while a third is offline. Holding the second download leaves both on the old selection. A recorded stand-in turn answers across the switch. The returning computer retries a transient download error, then switches and serves. |
| U9 | The old main goes down while its member's pack download is held. Advancing the relay and computer test clocks expires the lease. The new main completes the release even while its manifest request hangs. A returning computer pulls the retained pack from the new main. |
| U10 | A returning TypeScript beta reads the Rust main's signed offer, stages its own target, and starts Rust through the shell keeper. The old home is copied before selection. |
| U11 | The published v0.7.2 and v0.8.0-beta.1 Mac golden homes take signed Rust installer media through `kuvo-install` into the actual move. The resulting home retains the original notes and the Rust launch manifest. The installer starts the Helper through a scratch launchctl stand-in and recovers a missing registration through `status` and `new`. |
| U12 | A TypeScript updater prepares the target-specific Rust pack while the old selection stays active. |
| U13 | A failed first Rust launch leaves the Rust selection and writes the keeper's refusal record. |
| U14 | A disposable Mac Helper hands over once for each replacement of the sealed bundle. The resident waits through an update, falls back to opening the Helper after launchd refuses, records a Helper that remains old, honors Stop before and during handover, and leaves a self-renewing Helper alone. |
| U15 | `helper install` writes the launch agent with its login item, keep-alive policy, and XML-escaped arguments. Loading waits for the previous job and retries a refused bootstrap. Status, existing-job start, disabled Login Items, and explicit restart after Stop use scratch launchctl and open stand-ins. |
| U16 | An ad-hoc signed Swift Helper uses a disposable Keychain and a Rust resident. When its signed bundle changes, the Helper exits 75 for launchd without a Rust handover. |
| U17 | The authenticated local Mac resident starts `kuvo-start pause` as a separate process with the scratch account as `HOME`. A machine returns the same refusal on both boxes and creates no pause marker. |

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

The second slice passed ten Rust and five TypeScript cases on macOS, with
stand-ins for the relay, engine, and running turn. The failed-trial status
matched in the differential run. The full directory comparison still found
differences in the residents' other state. The native Windows rerun passed
five cases. The update prepared and retried in 11.9 seconds and switched
automatically in 1.3 seconds. The fixture used a user-only ACL and registered
no scheduled task.

The macOS shell fleet first prepared in 21.1 seconds. The main and member
confirmed 20.6 and 24.9 seconds after the held download resumed. The returning
computer confirmed in 38.6 seconds, including a 503 retry. The native-keeper
failover fixture elected a new main in 22.9 seconds with the relay clock
advanced, confirmed 17.2 seconds after the held download resumed, and caught
up the returning computer in 24.1 seconds from the feed. With the feed
returning 404, the Rust computer confirmed in 68.1 seconds using the main's
retained pack, and the returning TypeScript beta confirmed in 52.3 seconds.
The TypeScript fixture uses the checkout's transition-capable bundle.
The v0.7.2 move took 44.0 seconds and the beta.1 move took 27.0 seconds.
Their Helper restarts took 0.8 and 1.1 seconds through the launchctl
stand-in. Those timings exclude generation of the golden homes.

The static Linux musl rerun on Ubuntu 24.04 under WSL2 passed eight cases.
Preparation and retry took 6.2 seconds, and selection and confirmation took
1.4 seconds. On the shell-keeper fleet, the first computer prepared in
2.9 seconds. The main and member confirmed 7.3 and 9.3 seconds after the
held download resumed, and the returning computer took 12.2 seconds,
including retrying a 503 response. The failover fixture elected a new main
in 4.1 seconds with the relay clock advanced, confirmed in 5.9 seconds after
the download resumed, and caught up the returning computer in 14.0 seconds
with the feed returning 404.

With peer release mail enabled, native Windows passed six cases. Preparation
with retry took 9.6 seconds and selection took 1.3 seconds. In the fleet case,
the first computer prepared in 2.8 seconds, lease failover took 4.2 seconds
from boot with the test clock advanced, and confirmation took 5.9 seconds
after the download resumed. The returning computer confirmed in 14.1 seconds
after the feed returned 404. Enrollment, selection and uninstall stayed in
scratch homes. This fixture registered no scheduled task.

One macOS fleet run hit its 45-second test deadline after failover while
another workspace was compiling. A focused rerun passed. Holding the next
main's manifest request open then reproduced a failure after the new main
held both the lease and the staged pack. With the request running separately
from activation, first preparation took 24.4 seconds, the waiting member
took over in 27.1 seconds from boot with the lease clock advanced, and it
confirmed 22.9 seconds after its download resumed. The returning computer
confirmed in 90.1 seconds via the new main. The check includes lease and
stage records in its failure output.

## TypeScript reference cases

`Covered` means the exposed behaviour is checked. `Partial` records which
underlying Rust path is proven and what the update or installer still owes.
Bridge-specific checks only for unpublished TypeScript betas and desktop app
update prompts follow the switch-over scope in [the design](rust-box.md#4-switch-over).
The reference table groups related TypeScript cases into 58 rows. Two rows
are covered, 31 have partial proof, three are specific to the TypeScript bridge
or desktop updater, and 22 are open. The inherited and live-proof table has
11 rows, nine with partial proof and two open.

| Source and case | Status | Remaining owner |
| --- | --- | --- |
| [updates](../../box/bridge-tests/updates.test.js), version order | Partial. U3 covers numeric ordering and refuses a different digest at the installed version. U4 covers beta-to-final ordering. An already-ahead fleet member remains open. | 7.2 |
| Same, pinned signatures and signed bytes | Covered by U1. | — |
| Same, stale Mac launcher marks | Open. | 7.2 |
| Same, partial rollback reported once | Partial. 7.1 writes the refusal record and U6 reads it. The interrupted in-place restore and its one-time settlement need the predecessor fixture. | 7.2 |
| Same, signed release with Helper kept for peers | Partial. U4 covers signing and selecting a target. U9–U10 pull the signed pack from a peer. Assembling published target packs with a validated Helper remains open. | 7.2, 6.2, 9.3 |
| Same, full packaged contents | Partial. U4 checks the shipped executable and manifest in isolation. Complete per-target artifacts remain open. | 7.2 |
| [update-follows-peers](../../box/bridge-tests/update-follows-peers.test.js), installed version and app version | Partial. U8 and U10 follow the main's signed installed version. Installer and app offers remain open. | 7.2 |
| Same, desktop looks for its own update | TypeScript reference and desktop updater. | — |
| Same, six-hour check | Partial. The resident schedules checks every six hours. The elapsed-round and manual check cases remain open. | 7.2 |
| [update-stops-nothing](../../box/bridge-tests/update-stops-nothing.test.js), compatible standby | TypeScript bridge-specific. Rust holder compatibility still needs fleet proof. | 4.1, 7.2 |
| Same, held turn survives a switch | Partial. 7.1 checked a shell selection and a surviving holder. U8 uses a stand-in with a recorded PID during automatic activation. Native holder adoption remains open. | 4.1, 7.2 |
| Same, holder keeps running across engine change | Open. | 3.2, 4.1, 7.2 |
| Same, failed new bridge resumes a held turn | Partial. The 7.1 shell test covers the Rust keeper's rollback with a stand-in holder. Fleet failure announcement and retry remain open. | 7.2 |
| Same, one-time pre-holder bridge restart | TypeScript bridge-specific. The one-time TypeScript-to-Rust interruption follows section 4. | 9.1 |
| [update-engine-turn](../../box/bridge-tests/update-engine-turn.test.js), same engine | Partial. 3.2 covers resident restart. Full release activation with a turn still running remains open. | 7.2 |
| Same, background shell delays engine restart | Open. | 3.2, 7.2 |
| Same, engine-model bridge turn ends once before switch | Open. | 4.1, 7.2 |
| [update-from-before](../../box/bridge-tests/update-from-before.test.js), earlier updater through keeper | Partial. U5 proves the bin-only layout, and M5 in 2.6 covers opening older data. A real predecessor performing the update and entering Rust remains open. | 7.2, 9.3 |
| [update-real-keeper](../../box/bridge-tests/update-real-keeper.test.js), saved baseline layout | Partial. 7.1 checks the shell's layout judge and the startup fence. | 7.2 |
| Same, pre-marker updater commits and migrates | Partial. U11 starts the Rust move through the signed installer with both released data sets. A full predecessor updater and real Helper start remain open. | 7.2, 9.3 |
| Same, interrupted pre-marker updater restores baseline | Partial. 7.1 checks Rust rollback and U13 checks that the first Rust launch does not revert to TypeScript. The old rollback fixture still needs the released-predecessor run. | 7.2, 9.3 |
| [resident-update-pilot](../../box/bridge-tests/resident-update-pilot.test.js), sandboxed live turn survives both outcomes | Open. | 7.2, 9.3 |
| [fleet-switch](../../box/bridge-tests/fleet-switch.test.js), T1 prepare, commit, and offline catch-up | Partial. U8–U10 pass through the relay stand-in and keeper. U9 falls back to authenticated release mail when the feed returns 404. Before-rejoin work admission and the fleet status routes remain open. | 7.2 |
| Same, T3 retry and five-minute limit | Partial. The keeper stores a five-minute barrier from first preparation. U8 retries a transient download and U9 pulls a retained pack from its new main. The elapsed deadline, mixed-target fallback and manual retry remain open. | 7.2 |
| Same, ahead member offers its version | Partial. The main accepts signed newer offers from reachable members. The ahead member scenario remains open. | 7.2 |
| Same, Update Now chooses the immediate candidate | Open. | 7.2 |
| Same, progress summary remains while switching and after settlement | Open. | 7.2 |
| Same, row progress and retry | Open. | 7.2 |
| Same, pull progress never moves back | Open. | 7.2 |
| Same, member asks main to check | Open. | 7.2 |
| [own-home-mac-serve-update](../../box/bridge-tests/own-home-mac-serve-update.test.js), separate judge and trial cleanup | Open. | 7.2 |
| Same, Helper starts the new code after the resident exits | Partial. 7.1 dispatches Helper `serve` to a Rust pack. Live Helper update remains open. | 6.2, 7.2, 9.3 |
| [update-switch](../../box/test/update-switch.test.ts), broken route probe and beta.35 switch | Partial. U4 probes the Rust binary with the engine pin, and U10 checks a live beta-to-Rust switch. The failed route probe remains open. | 7.2 |
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
| [install](../../box/bridge-tests/install.test.js) 124, signed pack, engine, and fresh verdict | Partial. U1–U4 cover update admission, 2.6 C1 covers the read-only verdict, and U11 checks the signed installer and engine during a move. Fresh making through the installer remains open. | 7.2 |
| Same 148, 204, 267, installer after last wipe, stop without Helper, and completion outside App Support | Open as installer scenarios. See 2.3 U7–U8 for wipe and receipt checks. | 2.3, 6.2, 7.2 |
| Same 361, two installers prepare together | Open. | 7.2 |
| Same 379 and 387, refuse local-file engine and damaged pack or engine | Partial. U2 and U4 cover update pack and engine integrity. The installer entry remains open. | 7.2 |
| Same 418, 442, 451, existing home, fleet update instead of reinstall, and offline status | Open. | 7.2 |
| Same 468, 505, Mac new and interrupted install, Helper launch | Partial. 2.6 covers staged making. U11 starts the Helper through the installer with a launchctl stand-in. Fresh installer admission and the real Helper remain open. | 6.2, 7.2 |
| Same 564, 594, 645, existing app's data moves, progress order, and legacy app kept | Partial. U11 covers the move, ordered progress, and both the retained source and published home. The live Helper readiness check remains open. | 6.2, 7.2 |
| Same 700, 725, launch retry after move and deferred key settlement | Partial. U11 checks installer restart after a missing Helper registration. 2.6 covers resumable settlement with stand-ins. Actual launchd and authenticated Keychain settlement remain open. | 6.2, 7.2 |
| Same 750, two installations preserved | Partial. 2.6 covers the verdict and fence. Installer invocation remains open. | 7.2 |
| Same 766, Linux move refusal | Partial. Existing Rust move command refuses. The installer entry remains open. | 7.2 |
| Same 774 and 793, Mac app copy and case spelling | Partial. 2.6 checks the Mac command's matching home. The installer entry remains open. | 7.2 |
| Same 810, 875, 901, Linux installation, live-process fence, and restart after failure | Open at installer boundary. | 2.3, 7.2 |
| [install-verdict](../../box/test/install-verdict.test.ts) 32, 45, 57, 74, 81, 89, 96, 111, 128, 147, 154, 169 | Partial. All 12 verdict cases are mapped in 2.6 C1, M2, M3, M5, and A10. U11 checks missing Helper registration and restart with a stand-in. Live readiness (45, 57, 128), restarted fresh making (74), and Linux starter admission (169) remain open. | 6.2, 7.2 |

## Inherited cases and system proof

| Source | Status | Remaining owner |
| --- | --- | --- |
| [Keeper](rust-keeper-coverage.md) route, future update record, rollback status, and former release | U5 covers bin-only manifest recovery and U6 covers the failed-trial status. Future pack file-list versions are refused by the Rust reader. Former-release integration remains open. | 7.2 |
| [Store](rust-store-coverage.md) authority-format cases | Installer stdin/prefix before making, making marker, repair records, and former-release rollback still need full installer and updater proofs. | 2.3, 7.2 |
| [Adding](rust-adding-coverage.md) `computers-add.test.js` 302, 360, 397, 445, 2038, 2069 | Open. Cover update discovery, missing version, fallback address, retired app, and interrupted Helper reinstall. | 2.3, 2.4, 2.6, 6.2, 7.2 |
| [Move](rust-move-coverage.md) `rehome`, `rehome-run`, `folder-version` 287, 301, 326, and own-home Mac | Partial. 2.6 has released data and cutover tests with stand-ins and U11 covers the installer entry for moves. The release trial and Helper launch remain open. | 4.1, 6.2, 7.2 |
| [Engine](rust-engine-coverage.md) inbox paging, incomplete inventory and ordered crash recovery | The existing inbox API is available. Its coordinated switch caller and release tests remain open. | 3.2, 7.2 |
| Same, engine swap holds, rollback, expiry, and downgrade fence | Open as release tests. | 3.2, 7.2 |
| [Backups](rust-backups-coverage.md) durable pre-switch backup and wipe generation check | Partial. U10 checks the saved TypeScript home, and U13 checks the one-time refusal to roll back. Interrupted handoff and wipe generation in this workflow remain open. | 7.2, 9.1 |
| Rust-to-Rust live three-computer switch, running turn, offline return, and failover | Partial. U8 uses the shell keeper and U9 uses the native keeper. A stand-in turn stays alive. Before-rejoin work admission and native holder adoption remain open. | 4.1, 7.2 |
| Golden 0.7.2 and beta.1 through `kuvo-install`, and a TS-beta computer catching up after the switch | Partial. U10–U11 pass in scratch homes. U10 uses the checkout's transition-capable TypeScript bundle. U11 uses released Mac app homes, a stand-in engine, and a scratch launchctl. The released Linux machine upgrade and real Helper job remain open. | 6.2, 7.2, 9.3 |
| Uninstall footprint and Mac, Linux, WSL and native-Windows proof | Partial. U7 covers stopped standalone scratch installations on Mac, native Windows, and Linux musl under WSL. The live shared, Helper-owned and installer flows remain open. | 2.3, 7.2 |
| [Setup](rust-setup-coverage.md) add and fleet authority, [Windows](rust-windows-coverage.md) case 18 | Partial. U9 checks a new main's lease and U4 checks an automatic Windows selection. Shared Windows activation, on-return admission, and cleanup of a busy previous release remain open. | 2.3, 7.2, 8.1 |

## Mac Helper supervision from 6.2

Twelve of the 18 groups below are covered, four have partial proof, and
two are open. U14's seven handover branches took 23.9 seconds together.
U15 took 5.8 seconds. U16 measured 0.3 seconds from replacement to exit.

| Source and case | Status | Remaining owner |
| --- | --- | --- |
| [helper-handover](../../box/bridge-tests/helper-handover.test.js#L182), one handover after update while work runs | U14 checks a fresh resident handing over its parent and a stand-in turn surviving. | — |
| [Same](../../box/bridge-tests/helper-handover.test.js#L215), wait for the update to finish | U14 holds the updater record for a live fixture PID and clears it before handover. | — |
| [Same](../../box/bridge-tests/helper-handover.test.js#L240), launchd refuses | U14 observes the scratch open program starting the replacement. | — |
| [Same](../../box/bridge-tests/helper-handover.test.js#L255), launchd starts a still-old Helper | U14 records the surviving old Helper once across restarts. Reinstalling the same signed bundle lets the keeper try once for the new copy. | — |
| [Same](../../box/bridge-tests/helper-handover.test.js#L296), Stop before or during handover | U14 checks both orders. | — |
| [Same](../../box/bridge-tests/helper-handover.test.js#L322), self-renewing Helper | U14 checks the renews bit from the Helper's startup message. U16 checks a real disposable Helper exiting 75 in 0.3 seconds. | — |
| [own-box-helper](../../box/bridge-tests/own-box-helper.test.js#L51), login agent and app association | U15 checks the launch agent as plutil parses it. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L85), wait for launchd and retry | U15 waits for the previous job and starts again after bootstrap refuses. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L103), reserved XML characters | U15 checks the decoded home and program arguments. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L115), whole start and leave an existing job | Partial. U11 restores a missing job and U15 checks kickstart, `--if-missing`, and disabled Login Items. A real launchd rehearsal and production-signed identity continuity remain open. | 9.3 |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L183), refuse another installation | U15 checks machine refusal, the missing Helper, and the disabled scratch adapter. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L202), unknown command and flags | U15 checks invalid app IDs, unknown commands, unknown flags, and trailing arguments. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L398), desktop's location for Open Kuvo | Open. | 7.1, 7.2 |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L416), at-work and Stop on this Mac | Partial. U17 checks the local Stop command and U14 checks the stop markers during handover. The at-work facts and events remain open. | 7.1, 7.2 |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L437), reject stopping a machine | Partial. U17 checks the refusal. The at-work applicability flag remains open. | 7.1, 7.2 |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L447), restart after Stop | U15 checks launchctl waiting for the old PID before kickstart. | — |
| [Same](../../box/bridge-tests/own-box-helper.test.js#L470), Darwin machine's Files permission | Open. | 6.1, 7.1, 7.2 |
| [helper-running](../../box/test/helper-running.test.ts), load or kickstart when necessary and leave a running Helper | Partial. U11 loads a missing registration and U15 starts an existing job. Actual process state under launchd remains open. | 9.3 |
