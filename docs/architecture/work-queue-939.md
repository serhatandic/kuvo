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
