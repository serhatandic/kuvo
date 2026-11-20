# Work Queue 939 architecture decisions

Base commit `b592c91126003d2786721902d86df84119c09705`. This covers the four Box gaps from the November 20 audit. The separate screens and mobile work remain parked.

## Last wipe for an app-owned Mac Box

The Box accepts the last wipe through the existing membership operation. The wipe worker verifies the requested move and export preparation and rechecks the source generation after stopping writers. For an app-owned Box, the destructive boundary is its canonical Application Support directory, including Box data kept beside the Box. The worker refuses symlinked layouts and configured data roots that resolve outside this directory. It stops the existing keeper, fences its specific launchd service, and clears the legacy app’s three scoped identity entries. Disposable fixtures stand in for OS operations.

The existing erase journal binds the setup and operation to the directory device and inode. An app-scoped journal path keeps different app copies independent. The detached worker renames the approved root, durably records the detach, erases the detached tree, and then records completion. The installer resumes authorized interrupted work using the original data-root settings and exposes the verified receipt on status. Reinstall clears the receipt after a successful start. The installed app bundle, user projects outside the root, and encrypted deposits under `~/.kuvo-backups` are outside the erase boundary.

## Relay-only backup deposit and recovery

The Box advertises `box.backups.relay`. A backup destination with a computer key and no SSH address uses authenticated enrolled peer mail. Selecting an explicit SSH address or separate disk uses the existing transport. Destination availability requires a supported transport. Backup configuration and the recovery key remain in the source Box's private store.

The source seals the same authenticated archive used for disk and SSH. The receiver reserves space, receives ordered ciphertext chunks, verifies the SHA-256 checksum and publishes the complete archive durably. The source reads the deposited archive back and authenticates it with the local key before reporting a verified restore point. No recovery key is sent to the receiver.

A detached Mac worker obtains its Box identity from the existing authenticated host. The live Mac relay process checks a proof over the exact request and destination before sending the worker's spool.

Archives use `~/.kuvo-backups/relay/<setup>/<owner>/<source>/<point>.kuvo` on the receiving disk, alongside existing SSH deposits. The source binds the capture and transport to the same accepted setup. The receiver checks that scope, accepted membership, and revocation for each operation. A current owner can access its own archive namespace. The current replacement can access the exact source named by an accepted replacement record. The restore path still verifies the manifest and current authority before activating records. A re-enrolled recipient on the same disk can serve the archives within the same setup. If the receiving disk is physically lost, its restore points are unavailable.

The existing 30-daily and pre-operation retention policy applies when the source runs retention. Unlink and ordinary wipe leave deposited archives in place on either side. Explicit archive erase is supported within the authorized namespace. Transfers use 256 KiB chunks. The receiver reserves up to 2 GiB for each archive, up to 4 GiB of staging across four active uploads, and up to 32 GiB of kept ciphertext and reservations in the Relay store, with a maximum of 4,096 points and reservations combined. The same checks account for available disk space. One active upload per owner bounds contention and permits resuming an interrupted point by offset. The next allocation clears partials idle for an hour. Exhausted capacity refuses new work without evicting verified points. The limits bound disk impact and memory use without putting recovery keys or new bearer credentials on the receiver. Larger archives can use existing SSH or disk destinations. Make a Relay backup before revoking the source, since the later removal-only export accepts disk or SSH.

### Relay backup ownership and compatibility

Audit finding 5 gives the three external archive writers (disk, SSH and Relay) and the Relay receiver journal independent entries in the [data declarations](../../box/src/shared/data-declarations.ts). The stores use the declared locations and the archive codec uses the declared magic version. The archive format stays at `KUVO backup 1`. Disk and SSH use destination filesystem permissions and stage ciphertext next to its destination before publishing. Their temporary files are disposable if a process stops before publication.

The source owns the ciphertext. The Relay receiver stores it and controls its upload reservations. Every request checks the currently accepted setup and membership. An accepted replacement can read and erase the precise old source in that setup, and opens or resumes uploads under its own enrolled key. It cannot resume an old owner's reservation. The recovery key stays with the source or the person restoring the archive.

The Relay journal has its own `formatVersion: 1`, independent of Box upgrade or reinstall. Under the receiver's operation lock, a matching resume or append validates the unmarked v0 fields, writes v1 atomically and keeps the staged offset. A matching commit or abort can consume v0 directly. A matching begin can recreate an empty partial after a stop between writing the reservation and opening the partial. The next allocation reaps reservations idle for an hour, including stale corrupt journals small enough to inspect. An explicit unsupported version is preserved and blocks allocations that cannot account for its size. A rollback to a receiver that compared journal bytes can leave a v1 upload pending until expiry. It can still serve published archives. This journal change needs no wire negotiation or Box folder migration.

Peer mail keeps its request ID and the detached worker proof over the recipient and full request. The store handles replay using the source, point, byte offset, size and digest. A receiver that stops after linking an archive can confirm the same point by checksum on retry. Erase retires matching staging before returning, even if a crash already lost the link to the archive. A delayed commit cannot recreate the erased point. The source records a verified point only after it reads back and authenticates the archive. If all commit replies are lost, the caller can still report failure while a complete encrypted point remains on the receiver.

## Removal after Relay revocation

The follow-up to audit priority 3 starts at `bc46a35c6`. A daily point can predate the source's latest work. The primary still revokes access before delivering a removal instruction, and Relay's existing membership checks still cover list, read, deposit and erase. A source with automatic Relay backups persists its refusal of an ordinary wipe before requesting a separate export. The primary records the refusal against the same setup, source and removal operation. Changing the backup configuration or replaying the plain instruction leaves this fence in place.

The primary can add an explicit disk or SSH export to that held operation. The authenticated export, accepted move receipts when requested, attachment inventory and checked source generation form the wipe grant. The worker reads back the selected archive and rechecks source work after writers stop. A configured disk or SSH backup still runs its usual pre-wipe hook. A Relay-only move plan asks for an explicit export before work moves. A revised move-and-export plan preserves the user's chosen move, and the archive captures what remains at the source. Older daily points remain on the Relay receiver.

A canonical app-owned member erases its validated Application Support tree with a separate member journal. The journal binds the old directory's identity, allows installer recovery after detach, and yields a member acknowledgment without a reinstall offer. If stopping the keeper changes the source generation, the worker keeps its in-memory identity available for removal mail and waits for a fresh grant. That listener accepts no ordinary requests and writes no new identity outside the data root. If it is lost before a new grant, the old app's source Box must run again to continue review. Keychain cleanup and physical deletion finish before the worker releases an erase receipt. The SSH Add probe can also present the canonical app's signed proof to a later primary.

The Box operation and Computers listing expose the pending action. The SSH Uninstall try reports success after confirmed erasure. Rendering the destination chooser and operation state in the desktop and mobile screens remains part of audit priority 4.

## Work Queue attachments

Capture and restore staging walk the saved item's attachment list, including valid leaf names ending in `.new`, `.lock`, or `.tmp`. Unlisted leaves and generic temporary files stay excluded. Older records without a file list retain the ordinary durable-file selection. Upgrade staging and restored ownership fences still run as before.

## Update actions

The listing Box attaches its offered action directly to update facts in the existing Computers response. The action supplies the route choice, confirmation, refusal and SSH address where applicable. The desktop renders the response and dispatches the supplied action. The feature gate continues to use each Box's announced features. The desktop offers its bundled release to its local Box at launch and retains installer bootstrap and data migration. The previous client-inferred action and installer update fallback are gone.

## Verification

Implementation commit `feb2243eb5cfa64fe81d24b874583372220db068`.

- The backup bridge suite passed all 22 tests. The Relay fixture covers encrypted readback, the authenticated detached Mac worker, rejection of unsigned spools, source loss and revocation, unrelated access, accepted replacement restore, interrupted uploads, capacity refusal, and retention after erasure. The attachment fixture restores the listed names and their bytes after erasing the source.
- The focused installer, update offer, and changed-source removal checks passed. The app-owned fixture uses a separate verified export, refuses an export inside the erase root, preserves unrelated files, resumes a stopped erase, and checks the exact Keychain service and accounts with a stand-in.
- The desktop Home and Launch suites passed all 51 tests. TypeScript, the Box build check, and the generated route contract check passed.
- The existing setup-handover tests for unlinking the only computer and restoring an encrypted export after wipe did not complete on this branch. Both also timed out on a disposable copy of the verified base. Linux verification was unavailable because the devbox hostname did not resolve.
- Independent review found the app-root export overlap, the unauthenticated detached spool boundary, preservation of verified SSH aliases, and Relay durability and scope issues. The implementation addresses those findings. All erase checks used disposable layouts.
