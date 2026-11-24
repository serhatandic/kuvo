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

Intel Macs: machine only (box, no app).
Intel Mac: checked without running; a real run waits for an Intel Mac.

## Checks added in 7.2

| Check | Boundary |
| --- | --- |
| U1 | `update --phase prepare` rejects a foreign key, a forged signature, and edited manifest bytes. |
| U2 | The same command rejects a changed pack while preserving the selected release. |
| U3 | The same command declines an older release, a same-version pack with a different digest, and a release without `spreads: true`. Numeric minor and beta versions compare in release order. |
| U4 | The release signer emits one signed descriptor per target. The Rust updater refuses an invalid engine archive and a signed pack whose program fails its probe. The old resident keeps serving through both refusals and preparation retries. An explicit native-keeper selection starts and confirms the prepared release. |
| U5 | The shell keeper answers an SSH request using Rust after the previous stager's bin-only move. It restores `box.json` on disk. |
| U6 | `GET kuvo/update` reports the installed version, running engine, latest known version, and the keeper's failed trial. `POST` refuses to update a borrowed home. |
| U7 | The prepared-pack scenario stops its keeper and uninstalls the standalone computer. It retains the home, clears the desktop identity, and leaves the scratch account and its temporary directory unchanged. |
| U8 | Two Rust computers prepare while a third is offline. Holding the second download leaves both on the old selection. A recorded stand-in holder answers across the switch, and the replacement resident still sees the same active engine session. The returning computer retries a transient download error, then switches and serves. |
| U9 | The old main goes down while its member's pack download is held. Advancing the relay and computer test clocks expires the lease. The new main completes the release even while its manifest request hangs. A returning computer pulls the retained pack from the new main. |
| U10 | A returning TypeScript beta reads the Rust main's signed offer, stages its own target, and starts Rust through the shell keeper. The old home is copied before selection. |
| U11 | Signed Rust installer media makes a fresh Mac home in 46.1 seconds and moves the published v0.7.2 and v0.8.0-beta.1 Mac golden homes in 55.3 and 51.1 seconds. Both released apps are stopped. The moves retain the original notes, select the signed release atomically and keep its full signed manifest and pack. The installer starts the Helper through a scratch launchctl stand-in and recovers a missing registration through `status` and `new`. Both installed shell keepers answer `kuvo/info` within 0.3 seconds. |
| U12 | The bridge beta prepares the target-specific Rust pack while the old selection stays active. The deployed beta.48 chooses the unusable legacy descriptor in the same signed envelope and fails before staging. |
| U13 | A failed first Rust launch leaves the Rust selection and writes the keeper's refusal record. |
| U14 | A disposable Mac Helper hands over once for each replacement of the sealed bundle. The resident waits through an update, falls back to opening the Helper after launchd refuses, records a Helper that remains old, honors Stop before and during handover, and leaves a self-renewing Helper alone. |
| U15 | `helper install` writes the launch agent with its login item, keep-alive policy, and XML-escaped arguments. Loading waits for the previous job and retries a refused bootstrap. Status, existing-job start, disabled Login Items, and explicit restart after Stop use scratch launchctl and open stand-ins. |
| U16 | An ad-hoc signed Swift Helper uses a disposable Keychain and a Rust resident. When its signed bundle changes, the Helper exits 75 for launchd without a Rust handover. |
| U17 | The authenticated local Mac resident starts `kuvo-start pause` as a separate process with the scratch account as `HOME`. A machine returns the same refusal on both boxes and creates no pause marker. |
| U18 | The deployed beta.48 reaches the Rust main and takes its signed pack via relay. Its fixed JavaScript link probe refuses the Rust-only pack and beta.48 remains selected. |
| U19 | The deployed beta.48 returns after two computers confirm Rust. The Rust main offers the separately signed TypeScript bridge beta and serves both packs over authenticated relay mail while their URLs return 404. The returning computer confirms the bridge with its keeper, engine stand-in and agents process, then confirms Rust with a private copy of its old home. |
| U20 | A member's manual check makes its main check the signed feed. If the main is away, the member keeps the request across its own restart and asks the main when it reconnects. The check respects a persisted away record even while the live peer view still says reachable, as the TypeScript reference does. The focused Mac run took 6.4 seconds, including enrollment. |
| U21 | An untrusted signature in the retained release does not prevent a check against the signed feed. The previous resident stayed on the installed version and made no feed request. The fixed resident checked in 2.2 seconds. |
| U22 | The stopped Mac uninstall reports its Helper login job, refuses a job for another folder and a failed launchctl removal, then unregisters and removes its own job on retry. It retains work and other login jobs in the scratch account. A second uninstall succeeds with no login job left. |
| U23 | A stopped Unix home with an installed keeper removes only Kuvo's tagged reboot entry. An unreadable crontab leaves the original table and the installation intact. The retry keeps the other jobs and saved work and writes nothing to the scratch account's temporary folder. The Mac run with a disposable crontab took 2.5 seconds. |
| U24 | The signed installer admits the stopped released-machine homes by their saved machine identity. It rejects the wrong identity and waits if a recorded old process is still alive. The v0.7.2 and beta.1 fixtures moved, kept their keys and saved work, and answered through the selected Rust keeper in 17.2 and 21.1 seconds on Mac and 4.7 and 4.7 seconds on Linux musl under WSL2. |
| U25 | A Rust main tries the saved SSH address while its v0.7.2 machine is offline. When the address answers, it checks the saved machine identity, sends the retained signed target pack, and starts `kuvo-install`. If the first keeper launch fails after selection, the next round resumes it. The Rust keeper answers and the SSH reservation and installer staging clear. The old key, secret and work remain in place. |
| U26 | The signed Linux installer judges a fresh home and refuses a move without creating the home. It installs its target pack, preserves earlier work and the desktop identity, adds one tagged reboot job, and waits for the new resident to answer. A failed startup leaves the fresh verdict available. A retry preserves the partial identity and link alongside the earlier work and makes a new setup. If an earlier keeper is still running, the installer leaves the home and crontab untouched. |
| U27 | A Linux installer stopped after publication and before keeper startup resumes with the same selected release, saved SSH identity and work archive. A successful install retires earlier incomplete installer staging and preserves its partial launcher in the work archive. Both cases passed on native Linux and WSL2. |
| U28 | The main takes a signed release from a reachable member already running it. Both the manifest feed and the pack URL return 404. The main gets the retained pack over peer mail, switches and reports the installed version. The offline computer retains its old selection until it returns. Passed on Mac, WSL2, and native Windows. |
| U29 | A slow member stays on its release while the main passes the five-minute preparation deadline. The main switches from its saved preparation clock while the member's download is held. The member confirms once its download resumes, and the offline computer catches up when it returns. Passed through the shell keeper on Mac. |
| U30 | A failed fleet download records the signed version and full pack digest. Restarting the resident keeps the failure available for retry. The local retry route rejects a stale digest and requests the matching pack again while the previous release keeps serving. |
| U31 | The main receives a member's fixable failure and rejects a retry for a stale digest. After the main has switched, it can still request a retry for the member's signed target over authenticated mail. The member retries and the main's own failure record stays untouched. Passed on Mac, native Linux, WSL2, and native Windows. |
| U32 | The computers list reports the local failure with its signed target version and digest, the current release, and whether retry is available. The list and retry replies use the same row shape and wording. Passed on Mac, native Linux, WSL2, and native Windows. |
| U33 | The main's computers list reports the reachable member's failure with the version and digest of the signed pack for that member's platform. The row shows the member's current release and offers a retry for the matching target. Passed on Mac, native Linux, WSL2, and native Windows. |
| U34 | The computers list shows getting ready and ready from the live preparation, then up to date for a confirmed computer and off for one awaiting return. The selected signed release on trial shows switching while the previous release remains current. Once the keeper clears the trial, the row shows the installed version and digest. Passed on Mac, native Linux, WSL2, and native Windows. |
| U35 | The computers list counts the setup's preparing, ready, confirmed, and offline computers. It shows the failed computer's warning, keeps the summary visible through switching, and retains the settled result for ten minutes across a restart. A computer that first sees the release after confirmation has no visible completion. Passed on Mac, native Linux, WSL2, and native Windows. |
| U36 | A signed Linux install starts an answering keeper with earlier staging that cannot be deleted. The installer moves the staging out of the active path, reports success, and a later status check identifies the installed machine. The Rust cases passed on native Linux and WSL2, and the matching TypeScript case passed on WSL2. |
| U37 | The Linux installer reopens its saved journal at each publication boundary. Recovery keeps earlier work and conversations in one archive, keeps the already staged engine, publishes the selected signed release, and starts the keeper. Native Linux and WSL2 passed after archival began, after the engine moved, and after publication before startup. |
| U38 | With the feed unavailable, the main pulls a known signed release from an ahead member. The relay holds parts while both computers report the same accepted fraction and wording. Replaying already accepted parts leaves the count unchanged. The main confirms the release after the remaining parts arrive. Passed on Mac, native Linux, WSL2, and native Windows. |
| U39 | A member reports the signed release it is trying while its previous release remains current. Neither computer accepts a retry for that trial. A three-minute relay gap keeps the switching row, and the saved last-heard time appears when the ten-minute restart window expires. Passed on Mac, native Linux, WSL2, and native Windows. |
| U40 | A verified pack that cannot unpack leaves a blocked row tied to its signed digest. The current release keeps serving, the row survives restart, and manual retry is refused. Passed on Mac, native Linux, WSL2, and native Windows. |
| U41 | A brief relay gap preserves a member's preparing row. If the member last reported the signed pack ready, its row shows switching during the restart grace before a trial is reported. Passed on Mac, native Linux, WSL2, and native Windows. |
| U42 | The first holder of a verified pack stops sending after five accepted parts. Its incomplete transfer times out. A second holder starts, the fraction stays at five parts until the second transfer exceeds that count, and the recipient confirms the release. Passed on Mac, native Linux, WSL2, and native Windows. |
| U43 | A saved keeper trial refusal blocks only its matching signed pack and reports the same reason in both update reads. It refuses retry and skips the download. A refusal for a different digest leaves the current pack eligible. Passed on Mac, native Linux, WSL2, and native Windows. |
| U44 | A successful prepare retires unused older releases and finishes an interrupted retirement. A running holder keeps its own older release until it exits. The selected and newly staged releases and unrelated folders remain available. Passed on Mac, native Linux, WSL2, and native Windows. |
| U45 | The computers list keeps the recorded last-heard time and shows minutes, hours, yesterday, or days as the gap grows. A recently heard but unreachable computer with no recorded update facts reads off immediately. Passed on Mac, native Linux, WSL2, and native Windows. |
| U46 | The native shell installer refuses a damaged signed envelope or selected pack during status without creating the home. The restored media reports the expected verdict and installs. Passed on Mac, native Linux and WSL2. Native Windows passed the keeper status control. |
| U47 | The native keeper replaces the resident while its real Rust holder is running a stand-in agent turn. The new resident takes the next controller generation with the same holder and agent PIDs. The turn completes once in the saved conversation and the keeper confirms the release. Passed on Mac, native Linux, WSL2 and native Windows. |
| U48 | Four simultaneous installer status calls check the same verified media, return identical fresh verdicts and leave the home absent. The ordinary installation then succeeds. Passed on Mac, native Linux and WSL2. |
| U49 | The native shell installer rejects a signed engine descriptor that names a local file. Status leaves the home absent and the restored network descriptor proceeds to installation. Passed on Mac, native Linux and WSL2. Native Windows passed the keeper status control. |
| U50 | The release builder assembles Rust target packs with the native executable and common catalogs, browser, and configuration. Mac arm64, native Linux x64, WSL2, and native Windows x64 verify and prepare their pack from the same signed manifest. The Mac pack retains the ad-hoc Helper signature, and the Windows pack ships the runtime library and probes the pinned engine. |
| U51 | A trial executable exits just after the previous resident stops, leaving its Rust holder running. The keeper resumes the recorded rollback, and the previous release adopts the same holder and agent at the next generation. The conversation completes once and the keeper settles the refusal for the failed digest. Passed on Mac, native Linux, WSL2, and native Windows. |
| U52 | The release signer reads the five named target archives beside the compatibility pack. It checks their release and engine metadata, signs every archive's digest and URL, and keeps the exact signed TypeScript bridge. An inconsistent target fails before an envelope is written. |
| U53 | The release dry run builds the five target programs and assembles and signs them separately from the desktop release. A local replay packaged all five binaries, checked every signed archive digest and file list, and prepared the matching release on Mac arm64, native Linux x64, WSL2, and native Windows x64. Static checks verified the Intel-only box executable, its universal ad-hoc signed Helper, and the static Linux arm64 executable. |
| U54 | During the shell-keeper fleet switch, the main Mac retains the anonymous and named launcher markers from older clients. It confirms the signed release and reports an unblocked updater with the same marker bytes. The TypeScript launcher-marker reference also passed. |
| U55 | Fresh installer status checks the pinned engine download without writing files. Corrupt bytes and an unavailable download give the TypeScript failures. With an earlier executable already installed, status answers offline without a download. Restored media passes and installs on Mac, native Linux, and WSL2. Native Windows passes the signed installer status cases. |
| U56 | A computer announces the verified candidate and its prepared digest while it still reports its current release. The peer receives the original signed envelope, and the retained archive matches that digest. Passed on Mac, native Linux, WSL2, and native Windows. The TypeScript reference also passed. |
| U57 | A verified release with an unavailable engine download offers retry and keeps the current release. Once the pinned engine is available, retry prepares the same signed target. The invalid pack control stays blocked. Passed on Mac, native Linux, WSL2, and native Windows. |
| U58 | An installed machine answers installer status while the engine feed is offline. A new install is refused and update is left to the fleet updater. No engine is fetched or installer staging left, and the resident, machine identity and selected release stay in place. Passed on native Linux and WSL2. |
| U59 | The Mac installer reports that a machine installation and an earlier app home coexist. It returns the TypeScript refusal for both move and new, and keeps both sets of work in place. Passed on Mac with the TypeScript reference. |
| U60 | The native Mac installer accepts an alternate case spelling of the selected home and returns the same status. It refuses a different home and the production app identifier in a scratch home, without creating either path. The TypeScript installer references cover app ownership and alternate spelling. |
| U61 | With a live Mac move holding its lock, both installers report waiting for status, move, and new. The actions return the waiting result and exit code, keep the lock and files, and status offers a fresh install once the holder exits. |
| U62 | The Mac installer resumes an interrupted setup from signed media. It reports the archive with the earlier launcher, selects the verified new release, clears incomplete staging, and starts the Helper through the launchctl fixture. The TypeScript interrupted-install reference also passed. |
| U63 | With work in an earlier app home or app-owned box, both Mac installers offer a move and give the same refusal for new setup. They keep the earlier files and leave the destination home absent. |
| U64 | With an existing machine installation on a Mac, both installers return the machine verdict and the same explanations for move and new. The original launcher and work stay in place and no installer staging is created. |
| U65 | The Mac installer agrees with TypeScript when asked to move a fresh home, move or install after sessions moved back, or install while an unfinished move is recorded. It reports the reason and status without changing the home, receipt, or journal. |
| U66 | The Mac installer gives the same refusal as TypeScript for an unreadable move journal on status, move, and new. It keeps the journal and refuses before writing installer staging. |
| U67 | Both Mac installers offer to start a loaded Helper that launchd reports as stopped, and a paused home. A running Helper or a registered job whose state is unavailable needs no action. The fixture uses a scratch launchctl stand-in and preserves the registration. |

U26 passed on Linux and Ubuntu 24.04 under WSL2. The four corresponding
TypeScript installer scenarios passed under the same WSL2 distribution.
The Rust updates area passed 17 applicable cases on WSL2 and seven on
native Windows, including the signed update and stopped uninstall.
On Mac, the signed fresh install and released-home moves, stopped Helper
uninstall, and tagged reboot cleanup passed against the release binary.

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
The deployed 0.8.0-beta.48 bundle and keeper from `44d71568d` took the same
signed pack from the Rust main over the relay. The updater refused it 41.6
seconds after boot with “The new link doesn't work with it here.” The beta.48
selection stayed active. Its updater runs fixed JavaScript probes and reads
the legacy `pack` descriptor. The fixture used a Bun preload to substitute
the public verification key for both the original resident and its detached
updater. The copied bundle and keeper came unchanged from the revision.
In the two-step run, the online Rust computers first prepared in 44.8 seconds
and confirmed 21.3 and 25.3 seconds after their held download resumed. The
returning beta.48 confirmed the bridge in 21.2 seconds and Rust 50.3 seconds
later, taking 71.5 seconds in total. Both packs came from the Rust main after
their HTTP requests returned 404. `sign-box.mjs --bridge` signed the outer
Rust manifest with the test signing key.

To check U18–U19, run `scripts/built-at.sh 44d71568d` with `BUILT_AT_CACHE`
set to scratch. Set `KUVO_CONFORMANCE_DEPLOYED_TS_BIN` to its `kuvo-link.js`
and `KUVO_CONFORMANCE_DEPLOYED_TS_KEEPER` to that revision's `kuvo-start`.
Set `KUVO_CONFORMANCE_TS_BIN` and `KUVO_CONFORMANCE_TS_AGENTS` to the bridge
beta build's link and agents bundles. The deployed build's trust anchor is
substituted only in its disposable Bun runtime.
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

A native Linux run capped at one CPU reached the returning computer's
120-second fixture deadline during the relay transfer. A trace counted 936
of 1,005 full-size relay frames at that point. The remaining frames arrived
by 140 seconds, and the keeper selected and confirmed the release after
160.7 seconds. The same Linux musl binary returned in 18.1 seconds under
WSL2. The fixture now budgets both the box's peer-transfer limit and its
keeper trial limit.

The following macOS run passed 22 checks and skipped the TypeScript-only
prepare check. First preparation took 27.8 seconds in the shell-keeper
scenario. Its main and member confirmed 39.1 and 44.0 seconds after the
download resumed, and the returning Rust computer took 28.9 seconds. The
failover fixture took 36.7 seconds to prepare, 39.0 seconds from boot to take
the lease with the test clock advanced, and 29.1 seconds from the resumed
download to confirm. The returning computer confirmed in 68.1 seconds.
The transition-capable TypeScript computer confirmed in 43.1 seconds.
The v0.7.2 and beta.1 moves took 42.7 and 38.4 seconds through `kuvo-install`,
with Helper restarts taking 1.1 and 0.9 seconds. The TypeScript conformance
area passed its seven cases. In the differential run, `kuvo/update` and the
machine's Stop refusal matched. The other runtime inventory had 80 differences.

The following native Windows run passed seven checks. Preparing and retrying
took 9.5 seconds, and selection took 2.5 seconds. The failover fixture prepared
in 4.8 seconds, changed main in 6.2 seconds from boot with the test clock
advanced, confirmed in 5.8 seconds after the download resumed, and caught up
the returning computer in 14.1 seconds. The fixture resolved the llvm-mingw
runtime DLL from the scratch folder for every executable copy.

The static Linux musl run including the bridge-offer code passed nine checks
under WSL2. The shell fleet first prepared in 4.4 seconds and confirmed on the
main and member 7.8 and 10.8 seconds after the download resumed. The returning
computer confirmed in 12.2 seconds. With lease failover, the new main took
over 6.1 seconds after boot with the test clock advanced, confirmed 5.9 seconds
after the held download resumed, and served the returning computer in 14.0
seconds. The deployed-beta and Mac fixtures ran on macOS.

With the bridge and manual-check cases enabled, macOS passed 25 Rust checks
and all seven applicable TypeScript checks. The exact deployed beta confirmed
the bridge in 22.0 seconds and Rust 33.8 seconds later. Its direct attempt
refused the Rust pack after 62.0 seconds with the old release still selected.
The released Mac installer moves took 37.7 seconds for v0.7.2 and 59.2 seconds
for beta.1. Their Helper restarts took 1.1 and 1.0 seconds. A focused rerun
of U8 checked an active engine turn across confirmation. Preparation took
48.1 seconds, the main and member confirmed in 23.7 and 25.9 seconds after
the held download resumed, and the returning computer took 33.2 seconds.
The `GET kuvo/update`, borrowed-home `POST kuvo/update` and machine Stop
responses matched side by side. The inventory comparison still found 84
other paths with differences. The refreshed native Windows updates-only run
passed all seven checks in scratch. It prepared its first computer in 16.1
seconds, changed main in 18.2 seconds from boot, confirmed 17.8 seconds after
resuming the download, and brought back the offline computer in 44.1 seconds.
The latest macOS workspace build passed. In its focused beta.48 rerun, the
returning computer took 11.9 seconds to confirm the bridge and 42.7 seconds
more to confirm Rust through authenticated relay transfers.

The refreshed Linux run passed 11 checks with a static x86_64 musl binary on
Ubuntu 24.04 under WSL2. The release-pack fixture removed debug symbols to
fit the signed pack's size limit. The shell fleet prepared in 6.4 seconds,
confirmed on the main and member in 8.9 and 13.0 seconds after the held
download resumed, and confirmed the returning computer in 13.3 seconds.
The failover fixture prepared in 6.4 seconds, changed main in 8.2 seconds
from boot with the test clock advanced, confirmed 7.9 seconds after the
download resumed, and brought the returning computer back in 20.0 seconds.

The next static Linux run passed 13 checks in 113.6 seconds under WSL2
Ubuntu 24.04. Both released machine homes took the signed installer and
answered in 4.6 seconds each. The tagged reboot entry was removed in
0.16 seconds using a disposable crontab. Set
`KUVO_CONFORMANCE_RELEASED_MACHINES` to a scratch directory with
`v0.7.2/machine` and `v0.8.0-beta.1/machine` generated by
`generateRelease(tag, folder, { app: false })`.

The later macOS updates run passed 28 checks. The main and online member
confirmed in 27.9 and 32.6 seconds after the held download resumed. The
offline computer confirmed in 30.1 seconds. In the failover fixture the new
main took the lease in 27.1 seconds from boot, confirmed 40.9 seconds after
the held download resumed, and brought the returner back in 68.1 seconds.
The exact deployed beta.48 took the bridge in 17.1 seconds and then Rust
in 40.7 seconds. On the same run signed media made a fresh Mac in 38.5
seconds, moved the released Mac apps in 68.1 and 55.0 seconds, and moved
the released machine homes in 17.2 and 21.1 seconds. The pinned-engine
test also verified the TypeScript exit-status rule against the actual
version output observed from the SHA-512-pinned Linux download.

The static Linux branch with the saved-machine transport passed 14 checks
under WSL2 in 125.5 seconds. In U25 the returning v0.7.2 computer answered
in 8.6 seconds and installer and reservation cleanup took 61 milliseconds
more. The scenario uses a scratch SSH stand-in that runs each guarded command
in the returning computer's account. U8 confirmed the main and member in
9.4 and 12.5 seconds after the download resumed and caught up the offline
computer in 14.4 seconds. In U9 a replacement main took over in 8.2 seconds
from startup, confirmed 7.9 seconds after resuming the download, and brought
the offline member back in 20.0 seconds.
After merging the standalone SSH guard from main, the same static WSL2
test returned the computer in 11.9 seconds, including a failed first keeper
start and the automatic retry. Reservation and staging cleanup took another
45 milliseconds. With the first launch refused and the retry missing, the
old build never started the selected Rust keeper.
The full run on that Linux build passed 14 checks in 124.2 seconds. Both
released machine homes moved in 4.3 seconds each. The Mac caller with the
same signed test release sent a distinct Linux target pack over the scratch
SSH transport to Ubuntu 24.04 under WSL2. After the forced first launch
failure, the computer answered in 33.2 seconds. The reservation and staging
cleared 304 milliseconds later, and its identity and saved work stayed.

The optimized macOS run passed 28 checks with three platform skips. It
prepared the first computer in 4.9 seconds. The main and member confirmed
7.2 and 13.6 seconds after the download resumed. The offline computer
confirmed in 13.3 seconds. The failover run took the lease in 4.5 seconds,
confirmed 3.6 seconds after resuming the download, and brought back the
offline computer in 8.1 seconds. The deployed beta.48 took the bridge in
11.2 seconds and confirmed Rust 8.8 seconds later. The fresh Mac install
took 3.9 seconds and the released Mac moves took 6.7 and 5.0 seconds. The
stopped released machine fixtures answered in 1.5 and 1.7 seconds. The
previous optimized run failed when a short engine probe exited before the
keeper could record its process identity. The installer now uses the exit
status from the child it owns.

## TypeScript reference cases

`Covered` means the exposed behaviour is checked. `Partial` records which
underlying Rust path is proven and what the update or installer still owes.
Bridge-specific checks only for unpublished TypeScript betas and desktop app
update prompts follow the switch-over scope in [the design](rust-box.md#4-switch-over).
The reference table groups related TypeScript cases and names the owner of
each remaining check. The inherited and live-proof table tracks cross-row
dependencies.

| Source and case | Status | Remaining owner |
| --- | --- | --- |
| [updates](../../box/bridge-tests/updates.test.js), version order | Partial. U3 covers numeric ordering and refuses a different digest at the installed version. U4 covers beta-to-final ordering. An already-ahead fleet member remains open. | 7.2 |
| Same, pinned signatures and signed bytes | Covered by U1 and U21. | — |
| Same, stale Mac launcher marks | U54 confirms the signed switch and unblocked updater with both markers present. | — |
| Same, partial rollback reported once | Partial. U51 resumes the native rollback journal and records the matching refusal while the held turn finishes. The interrupted in-place restore and its one-time settlement still need the predecessor fixture. | 7.2 |
| Same, signed release with Helper kept for peers | Partial. U9–U10 pull the signed pack from a peer. U50 prepares locally assembled target packs, including a Mac Helper with a verified ad-hoc signature. U52 signs the complete target set with the bridge envelope. Release-signed Helper media and publication remain open. | 7.2, 6.2, 9.3 |
| Same, full packaged contents | Partial. U50 prepares the complete local Mac arm64, Linux x64, and Windows x64 packs. U53 checks the signed Intel Mac and Linux arm64 archives as well. Published media remain open. | 9.3 |
| [update-follows-peers](../../box/bridge-tests/update-follows-peers.test.js), installed version and app version | Partial. U8 and U10 follow the main's signed installed version. Installer and app offers remain open. | 7.2 |
| Same, desktop looks for its own update | TypeScript reference and desktop updater. | — |
| Same, six-hour check | Partial. The resident schedules checks every six hours. U20 covers manual checks forwarded by a member. The elapsed-round cases remain open. | 7.2 |
| [update-stops-nothing](../../box/bridge-tests/update-stops-nothing.test.js), compatible standby | TypeScript bridge-specific. Rust holder compatibility still needs fleet proof. | 4.1, 7.2 |
| Same, held turn survives a switch | U8 checks fleet activation with a recorded worker PID. U47 checks adoption of the actual Rust holder after a successful keeper switch. U51 checks the same adoption and single completion after rollback. | — |
| Same, holder keeps running across engine change | Open. | 3.2, 4.1, 7.2 |
| Same, failed new bridge resumes a held turn | Partial. The 7.1 shell test covers the Rust keeper's rollback with a stand-in holder. Fleet failure announcement and retry remain open. | 7.2 |
| Same, one-time pre-holder bridge restart | TypeScript bridge-specific. The one-time TypeScript-to-Rust interruption follows section 4. | 9.1 |
| [update-engine-turn](../../box/bridge-tests/update-engine-turn.test.js), same engine | Partial. 3.2 covers resident restart. U8 checks the replacement resident can still see an active turn in the same engine stand-in. The actual native engine proof remains open. | 7.2 |
| Same, background shell delays engine restart | Open. | 3.2, 7.2 |
| Same, engine-model bridge turn ends once before switch | Open. | 4.1, 7.2 |
| [update-from-before](../../box/bridge-tests/update-from-before.test.js), earlier updater through keeper | Partial. U5 proves the bin-only layout, and M5 in 2.6 covers opening older data. A real predecessor performing the update and entering Rust remains open. | 7.2, 9.3 |
| [update-real-keeper](../../box/bridge-tests/update-real-keeper.test.js), saved baseline layout | Partial. 7.1 checks the shell's layout judge and the startup fence. | 7.2 |
| Same, pre-marker updater commits and migrates | Partial. U11 starts the Rust move through the signed installer with both released data sets. A full predecessor updater and real Helper start remain open. | 7.2, 9.3 |
| Same, interrupted pre-marker updater restores baseline | Partial. 7.1 checks Rust rollback and U13 checks that the first Rust launch does not revert to TypeScript. The old rollback fixture still needs the released-predecessor run. | 7.2, 9.3 |
| [resident-update-pilot](../../box/bridge-tests/resident-update-pilot.test.js), sandboxed live turn survives both outcomes | Open. | 7.2, 9.3 |
| [fleet-switch](../../box/bridge-tests/fleet-switch.test.js), T1 prepare, commit, and offline catch-up | Partial. U8–U10 pass through the relay stand-in and keeper. U9 falls back to authenticated release mail when the feed returns 404. Before-rejoin work admission and the fleet status routes remain open. | 7.2 |
| Same, T3 retry and five-minute limit | Partial. U29 covers the elapsed deadline and the waiting member's catch-up. U30 covers local retry across a restart and U31 covers authenticated peer retry after the main has switched. U32–U33 cover retry availability in the computer rows. U57 retries a failed engine download for the same signed release. U9 pulls a retained pack from its new main. Mixed-target fallback remains open. | 7.2 |
| Same, ahead member offers its version | U28 covers receiving the signed offer, pulling the member's retained pack and switching. The member's up-to-date row is part of the row-progress work below. | 7.2 |
| Same, Update Now chooses the immediate candidate | Partial. An explicit check bypasses the automatic release age rule. U20 verifies that an authenticated member request starts the check. A newly published release still needs a full switch scenario. | 7.2 |
| Same, progress summary remains while switching and after settlement | Covered by U35. | — |
| Same, row progress and retry | U30–U31 cover local and peer retry. U32–U33 cover retryable failure rows. U34 covers getting ready, ready, switching, off and up to date. U40 covers a preparation failure that must wait for the next release. | — |
| Same, pull progress never moves back | U38 checks local and peer progress and duplicate delivery. U42 checks replacing a stalled holder with a second one while retaining progress on both computers. | — |
| Same, member asks main to check | Covered by U20. | — |
| [fleet-progress](../../box/test/fleet-progress.test.ts), ready computer goes quiet, rows progress through trial, and a brief gap keeps the last row | U34 checks preparation through confirmation. U39 checks a signed trial reported over relay and expiry past the restart window. U41 checks a brief gap during preparation and a ready computer that goes quiet before reporting a trial. | — |
| Same, local box says it is switching and ready reads separately from done | Covered by U34. | — |
| Same, preparation fraction is visible in local and peer rows | Covered by U38. | — |
| Same, off row reports when it was last heard from | U34 checks one never heard from, U39 checks a quiet trial's expiry, and U45 checks last-heard wording and a peer without update facts. | — |
| Same, notifier stays while restarting, phases and words, ten-minute settled display | U35 covers the live summary and saved settlement across restart. The exact peer restart grace remains part of the reach cases above. | 7.2 |
| Same, verified kept pack is announced | U56 compares the remote announcement with the retained verified pack before activation. U8–U9 cover authenticated delivery after selection. | — |
| [own-home-mac-serve-update](../../box/bridge-tests/own-home-mac-serve-update.test.js), separate judge and trial cleanup | Open. | 7.2 |
| Same, Helper starts the new code after the resident exits | Partial. 7.1 dispatches Helper `serve` to a Rust pack. Live Helper update remains open. | 6.2, 7.2, 9.3 |
| [update-switch](../../box/test/update-switch.test.ts), broken route probe and beta.35 switch | Covered in scope. U4 rejects a signed pack whose Rust program fails the self-test. U10 checks a live switch with the transition-capable beta. The historical beta.35 switch is outside the scope of section 4. | — |
| Same, continue only interrupted engine turn | Open. | 3.2, 4.1, 7.2 |
| Same, guard commits after selected resident answers | 7.1 keeper scenario covers this. | — |
| Same, resident switch preserves the engine and sandbox files | Partial. 7.1 keeps workers during the shell switch. The prepared-release engine and plugins need an end-to-end test. | 7.2 |
| [release-catalogs](../../box/test/release-catalogs.test.ts), integrations and popular servers in fixture | U50 checks the catalogs in assembled Rust releases against the TypeScript catalogs. | — |
| Same, Mac integrations | U50 checks the Mac catalog in the signed target packs. | — |
| Same, legacy files byte-for-byte where preserved | The legacy writer checks still pass with the new Rust packing option. U50 compares the bytes and modes of shared release files with the TypeScript writer. | — |
| Same, files writer stays below destination | The common destination-bound writer check still passes. U50 prepares full packs in private homes, and the Unix installer status leaves a fresh home absent. The live uninstall footprint remains open. | 7.2 |
| [add-uninstall](../../box/test/add-uninstall.test.ts), stopped uninstall deletes desktop identity | 2.3 U5 covers stopped standalone. U22 also covers a stopped Mac with a Helper job. SSH wrapper and live keeper path remain open. | 2.3, 7.2 |
| Same, incomplete desktop cleanup reported | Open. | 2.3, 7.2 |
| Same, failed departure keeps identity | Open. | 2.3 |
| [install-offer](../../box/test/install-offer.test.ts), last wipe approval follows old identity | Partial. 2.3 U8 covers detachment identity. Installer re-entry remains open. | 2.3, 6.2, 7.2 |
| Same, app member wipe and receipt | Open. | 2.3, 7.2 |
| [install](../../box/bridge-tests/install.test.js) 124, signed pack, engine, and fresh verdict | Covered by U11 on Mac and U26 on Linux. | — |
| Same 148, 204, 267, installer after last wipe, stop without Helper, and completion outside App Support | Open as installer scenarios. See 2.3 U7–U8 for wipe and receipt checks. | 2.3, 6.2, 7.2 |
| Same 361, two installers prepare together | U48 checks the four concurrent status replies in the native read-only status path. All four return the same fresh verdict as the reference. | — |
| Same 379 and 387, refuse local-file engine and damaged pack or engine | U46 checks damaged release media, U49 checks local-file engine refusal, and U55 checks failure to download or verify the signed engine. U2 and U4 cover update pack and engine integrity. | — |
| Same 418, 442, 451, existing home, fleet update instead of reinstall, and offline status | U55 checks offline status with an earlier installed engine. U58 preserves an already running installation, refuses reinstall and leaves release changes to the fleet updater. | — |
| Same 468, 505, Mac new and interrupted install, Helper launch | Partial. 2.6 covers staged making. U11 checks a fresh Mac install and U62 checks an interrupted install through signed media. Both start the Helper through a launchctl stand-in. A real Helper job remains open. | 6.2, 9.3 |
| Same, earlier app's sessions prevent a new home beside them | U63 checks both earlier layouts through the native and TypeScript Mac installers. | — |
| Same 564, 594, 645, existing app's data moves, progress order, and legacy app kept | Partial. U11 covers the move, ordered progress, and both the retained source and published home. The live Helper readiness check remains open. | 6.2, 7.2 |
| Same 700, 725, launch retry after move and deferred key settlement | Partial. U11 checks installer restart after a missing Helper registration. 2.6 covers resumable settlement with stand-ins. Actual launchd and authenticated Keychain settlement remain open. | 6.2, 7.2 |
| Same 750, machine with and without an earlier app home | U59 checks status, move, and new with both homes present. U64 checks the existing machine by itself. | — |
| Same 766, Linux move refusal | Covered by U26. | — |
| Same 774 and 793, Mac app copy and case spelling | U60 checks the installer entry for an alternate spelling and keeps production and scratch homes separate. 2.6 checks the Mac command's matching home. | — |
| Same 810, 875, 901, Linux installation, live-process fence, and restart after failure | U24 covers signed installation on stopped released homes. U26 covers fresh Linux making, refusing a live writer, and retrying a failed first launch. U27 covers interruption after publication and retiring earlier incomplete staging. U36 covers a cleanup failure after startup. U37 replays interruption during archival and between the engine and release moves. | — |
| [install-verdict](../../box/test/install-verdict.test.ts) 32, 45, 57, 74, 81, 89, 96, 111, 128, 147, 154, 169 | Partial. All 12 verdict cases are mapped in 2.6 C1, M2, M3, M5, and A10. U11 and U67 check Helper registration, readiness and restart with a stand-in. U61 checks a live move lock through both installer commands. U62 checks restarted fresh making (74). U65 and U66 check fresh refusal, moved-back receipt, and unfinished or unreadable journals. Real Helper readiness and Linux starter admission (169) remain open. | 6.2, 7.2, 9.3 |

## Inherited cases and system proof

| Source | Status | Remaining owner |
| --- | --- | --- |
| [Keeper](rust-keeper-coverage.md) route, future update record, rollback status, and former release | U5 covers bin-only manifest recovery. U6 and U43 cover a keeper's trial refusal in update status and the fleet row. Future pack file-list versions are refused by the Rust reader. Former-release integration remains open. | 7.2 |
| [Store](rust-store-coverage.md) authority-format cases | Installer stdin/prefix before making, making marker, repair records, and former-release rollback still need full installer and updater proofs. | 2.3, 7.2 |
| [Adding](rust-adding-coverage.md) `computers-add.test.js` 302, 360, 397, 445, 2038, 2069 | Open. Cover update discovery, missing version, fallback address, retired app, and interrupted Helper reinstall. | 2.3, 2.4, 2.6, 6.2, 7.2 |
| Fresh SSH Add across different targets and headless Mac code join | U53 supplies signed packs by target. TypeScript sends its own common release files and fetches the target's pinned engine on the sender. Fresh Rust Add and headless code join still need to select and stage the target's signed pack and engine before enrollment, then start its machine keeper. | 2.3 |
| [Move](rust-move-coverage.md) `rehome`, `rehome-run`, `folder-version` 287, 301, 326, and own-home Mac | Partial. 2.6 has released data and cutover tests with stand-ins and U11 covers the installer entry for moves. The release trial and Helper launch remain open. | 4.1, 6.2, 7.2 |
| [Engine](rust-engine-coverage.md) inbox paging, incomplete inventory and ordered crash recovery | The existing inbox API is available. Its coordinated switch caller and release tests remain open. | 3.2, 7.2 |
| Same, engine swap holds, rollback, expiry, and downgrade fence | Open as release tests. | 3.2, 7.2 |
| [Backups](rust-backups-coverage.md) durable pre-switch backup and wipe generation check | Partial. U10 checks the saved TypeScript home, and U13 checks the one-time refusal to roll back. Interrupted handoff and wipe generation in this workflow remain open. | 7.2, 9.1 |
| Rust-to-Rust live three-computer switch, running turn, offline return, and failover | Partial. U8 uses the shell keeper and U9 uses the native keeper, with a stand-in turn kept across the fleet switch. U47 separately checks actual native holder adoption during a keeper switch. Before-rejoin work admission remains open. | 4.1, 7.2 |
| Golden 0.7.2 and beta.1 through `kuvo-install`, and a TS-beta computer catching up after the switch | Partial. U10 checks the bridge beta taking Rust directly. U18–U19 check the exact deployed beta.48's refusal and automatic two-step catch-up. U11 moves released Mac app homes with a stand-in engine and launchctl. U24 moves the stopped released machine homes through signed media. U25 checks automatic delivery to a returning v0.7.2 home with a guarded SSH stand-in. A released machine whose old keeper is still running, real SSH delivery, shipping the bridge beta, and a real Helper job remain open. | 2.3, 7.2, 9.1, 9.3 |
| Uninstall footprint and Mac, Linux, WSL and native-Windows proof | Partial. U7 covers stopped standalone scratch installations on Mac, native Windows, and Linux musl under WSL. U22 verifies stopped Helper-owned uninstall. U23 exercises reboot entry cleanup on Mac and Linux under WSL2 with a disposable crontab. Live shared uninstall and installer flows after wipe remain open. | 2.3, 7.2 |
| [Setup](rust-setup-coverage.md) add and fleet authority, [Windows](rust-windows-coverage.md) case 18 | Partial. U9 checks a new main's lease, U4 checks an automatic Windows selection, and U44 checks retirement once a holder using a superseded release exits. Shared Windows activation and on-return admission remain open. | 2.3, 7.2, 8.1 |

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

## Update and Stop home inventory

After `GET kuvo/update` and `POST kuvo/box/stop` on copies of the same
offline seed, both replies matched. The seed contains the installed version,
the failed update record and the folder layout marker. It has no setup
identity. The two residents create their own identities, sign their own first
checkpoints and write their startup records independently. The inventory also
includes files each resident creates while idle. Counts below are comparison
entries, including directory entries and individual JSON fields.

| Owning row | Entries | Difference and follow-up |
| --- | ---: | --- |
| 1.4 | 1 | `run/route/ssh`: local SSH IPC endpoint inventory. The server row owns the endpoint lifecycle. |
| 2.1 | 23 | `run/docs.changed`; `run/link.json` fields `key`, `keyNow`, `leadership/1/{d,s}`, `name`, `secret`, `setup/{id,primary,sign}`, `shared/membershipAndAccess/value/boxes/0/{address,key,name,sign}`, `sharedSetup`, and `sign`; `run/setup-card.json` fields `primary/{key,name}`, `self/{key,name}`, and `setup`; two files under `run/setup-checkpoints/`. Fresh key and setup generation accounts for the different signatures and checkpoint names. Setup owns comparison with a common saved identity. |
| 2.2 | 2 | `run/notices` and `run/notices/teller`: peer notice startup directories. |
| 2.3 | 4 | `run/link.json` fields `left`, `removalRevisions`, `removed`, and `removedHere`: removal defaults and receipts. This row also owns the existing `kuvo_setup` Clippy findings. |
| 3.1 | 4 | `config`, `config/opencode`, `config/opencode/plugins`, and `config/opencode/plugins/kuvo-defaults.js`: generated OpenCode defaults. |
| 3.2 | 2 | `chats` and `run/glance-source.json`: session storage and the ask checkpoint source. |
| 4.1b | 1 | `run/agents.json`: bridge startup record. Resident agent adoption owns any readers that still depend on it. |
| 4.1c | 1 | `run/socket.key`: independently generated local tool authentication key. |
| 5.1 | 7 | `bots`, `box/approval.json`, `run/bots-seen`, `run/work`, `run/work/{background,told}.json`, and `wakes`: bot and wake folders, approval defaults, and background work records. |
| 5.2 | 2 | `run/work-queue/{model,numbers}.json`: queue startup model and number allocation records. |
| 5.4 | 12 | `agents`, `agents/limits.json`, `agents/usage`, its `{claude,grok,codex}-files` directories, `insights.sqlite` and its `-shm` and `-wal` files, `settings.json`, and `usage.json`; also `run/usage-attribution.json`. These are usage scanners, limits, settings and SQLite startup state. |
| 5.5 | 10 | `artifacts` and its `citations-v1.json`, `copies`, `index-v1.sqlite`, `key`, and `log`; `box/marketplace` and its `mcp-relay-v1.json`; `library` and its `backfill.json`. |
| 6.1 | 2 | `run/fence` and `run/fence/agents.md`: generated sandbox instructions. |
| 7.2 | 1 | `run/link.version`: build marker differs because the two binaries have different build IDs. U24 verifies that the released-machine move answers from the selected Rust release. |
| 7.3 | 2 | `run/link.json` fields `macMoved` and `macPeers`: legacy Mac coordination defaults. |
| **Total** | **74** | |
