# Native Windows coverage for the Rust box

This maps native checks 13–26 in [Kuvo on Windows](windows.md#sessions-in-order) and the
cases handed to 8.1 by the other Rust maps. Native Windows has no TypeScript reference box.
The Rust keeper's existing cross-platform scenarios still exercise selector, rollback and
process ownership on all three systems. The live native trial used Windows 11 Pro x64 build
26300 and Git for Windows 2.56.1. SSH used the existing elevated token. Task Scheduler started
Kuvo with the user's limited interactive token.

## Windows trial checks

| Check | Status and evidence | Owner of remaining work |
| --- | --- | --- |
| 13 · OpenCode, Bun, SQLite, config and PTY | Open. Native resident startup exercises the box's own store. OpenCode's published runtime and its PTY need a full turn. | 8.2 |
| 14 · Starter and child windows | Passed. The starter PE has the Windows GUI subsystem and the trial keeper verified it had no console window. | 8.2 for engine, browser and agent child windows |
| 15 · Process identity, PID reuse, Job Objects, ConPTY and `.cmd` | Covered in part by [OS scenarios](../../box/crates/kuvo_os/tests/scenarios.rs) and [keeper scenarios](../../box/crates/kuvo-box/tests/keeper.rs). They check creation identity, job shutdown, native handover and explicit `.cmd` execution. | 8.2 for ConPTY |
| 16 · Same-SID pipe across sessions, rejecting other SID, remote peers and wrong keys, stale listener, holder handover | The OS tests inspect the actual pipe's SID-only protected ACL and consecutive connections. A resident started by Task Scheduler answered a same-SID named-pipe request from a fresh SSH connection with HTTP-equivalent status 200 and the same generation. A live second-account test and holder replay belong with the cross-session worker run. | 1.4 for native SSH denial and reconnect proof, 8.2 for agent holder handover |
| 17 · Chrome pipes, profile and credentials | Open. | 8.2 |
| 18 · Interrupted selector, open files and rollback with a holder | The plain selector, open-file publication and keeper rollback are in 0.4 and 7.1. The starter opens the stable keeper, which resolves that selector. The first SSH install seeds the same release directory layout. | 7.2 for signed update packs and busy old-release cleanup |
| 19 · Task restart, sign-out and reboot | Passed. On 24 November a scratch observer saw the production task start a new authenticated resident after a sign-out and console sign-in, and again after a restart and console sign-in, each with a new generation and task run. Task Scheduler recorded the starter's exact keeper exit code (42), then stayed idle under its normal restart rule. The production task's one-minute repeating trigger restarted the same starter automatically and got status 0 after status 42. After a forced exit in the real package, the same task started a new keeper and an authenticated resident answered with a new generation. The task ignores overlaps. Deliberate pauses and stops stay in effect. A PC at its sign-in screen stays offline under D8–D9. The test PC has auto sign-in disabled. | — |
| 20 · Native SSH probe, framing, forwarding and disconnect, relay replay and receipts | A Mac added the PC from a fresh account folder over SSH. Two new SSH connections received the same resident generation after the install connection ended. The live trial also stopped the keeper and removed the task and scratch install over SSH. The SSH transport and relay own their existing framing and replay rules. | 1.4 and 1.5 for the rest of the SSH and relay transport scenarios |
| 21 · Git Bash, CRLF, symlinks and credentials | [Native OS scenarios](../../box/crates/kuvo_os/tests/windows_native.rs) exercise Git Bash with actual Git, CRLF and spaces. Bash can write while its working directory exceeds MAX_PATH. Git tracks a file over that limit from a shorter repository root with `core.longpaths`. Native Git still fails when the repository's working directory itself exceeds MAX_PATH. Git Bash requests real symlinks, so it can report failure when the account lacks permission. Git's own credential behavior stays in its installation. Watch checks and vault commands run through this adapter. It finds Bash from `GIT_INSTALL_ROOT`, then the `git.exe` on PATH (its `cmd` or `mingw64\bin` folder), then the usual install folders, and never System32's WSL launcher. | 8.2 for wiring engine and agent turns to this command adapter and testing sign-ins |
| 22 · Five agents' native sign-ins, turns and stops | Open. | 8.2 |
| 23 · Drive, UNC, case and long paths | [Shared vectors](../../protocol/vectors/file-addresses/windows.json) exercise lexical round-trips and the Win32 long-path prefixes. The native shell test writes to a path over 260 characters. A case-sensitive directory kept two files whose names differ only by case. SSH was denied access to the WSL UNC share. Through the admin share `\\localhost\C$`, Git Bash worked in a UNC folder and the secret-folder ACL rule held. Git refuses a UNC repository until the user marks it `safe.directory`, which is Git's own rule. A home on a share is refused, because it must be on a local NTFS drive. | 8.2 for the engine and agent working directories |
| 24 · Defender, SmartScreen and firewall | Unsigned development builds only. | 9.1 for signed release packaging and SmartScreen/firewall rehearsal |
| 25 · ARM64 engine and Chrome emulation | Skipped. Release target is Windows x64. | 8.2 if ARM64 is added |
| 26 · Profile, NTFS identity, rename, delete and durability | The native home must be on local NTFS. Creation supplies the user's protected inheritable DACL in the same operation, and startup checks the owner and ACL. As TypeScript does on Unix, a folder for secrets is refused when another account owns it (the user, SYSTEM and Administrators may) and closed with a protected user-only ACL when others can open it. The locked-file and identity scenarios cover the store rules from 0.4 and 7.1. Windows flushes files through a write handle and uses write-through rename. The real resident wrote the selector, layout and `run/link.version` and answered an authenticated request. Roaming profiles are outside D1. | 8.1 for a fresh standard-account rehearsal: the test account is an administrator, so this needs a disposable standard account from the user |

## Cases assigned by the other maps

| Source | Status | Owner of remaining work |
| --- | --- | --- |
| [Adding coverage](rust-adding-coverage.md) · Native Windows Add and the resident's SSH lifetime | Passed on the real PC without a preseeded layout. Enrollment waits for the resident's authenticated answer while the account is signed in. | — |
| [Adding coverage](rust-adding-coverage.md) · U7 wipes the last computer | The native installer can stop the keeper and remove its task from the installing account. U7's signed unlink and wipe is still Unix-only. Windows needs an erase step after the installed executable and its starter have released their files. On Windows 11 a folder can be renamed while an executable inside it runs, so the home can be detached as on Unix. Only the erase of the running executable waits, and the next start of a box (such as the reinstall) can finish it from the Detached journal. | 8.1 for native wipe and reinstall, 2.3 for fleet removal |
| [Keeper coverage](rust-keeper-coverage.md) · Restart missing keeper and use saved launcher | Native Windows registers one hidden sign-in task with a stable starter and keeper. The live trial removed it from both the installing SSH token and the account's limited interactive token. Sign-out and restart with sign-in started it again (check 19). | 7.3 for released Mac repair |
| [Work coverage](rust-work-coverage.md) · Services a session left running | Windows has no process groups. A listening TCP socket (IPv4 or IPv6) counts for a command when its owner is in the command's process tree. Like a group, it still counts after the command's shell exits. A parent younger than its child has a reused PID and ends the chain. A native scenario starts a listening PowerShell from Git Bash and finds its port. | — |

## Targeted tests

In 8.1c Windows passed four native scenarios, 19 OS scenarios, the work watcher's services and
wakes tests, four keeper scenarios, and the join and uninstall scenarios. Linux on lokum passed
18 OS scenarios and six keeper scenarios using static musl builds. Two failures were already on
main. The `kuvo_store` declarations test lacks the stored-formats family "Retained work after
unlink". The `bots::kit` tests read a repository file by its build path, so they fail on another
computer. The vault conformance case "a machine vault keeps its secret through a restart" and
`vault::session_asks` each failed once on macOS, then passed on rerun.

Earlier, Windows passed 21 OS scenarios and four keeper scenarios. macOS passed 18 OS scenarios in
a serial run and six keeper scenarios. A macOS parallel run saw an intermittent process
identity lookup failure in the unchanged Unix adapter. Linux on lokum passed 17 OS scenarios
and six keeper scenarios using static musl builds. The TypeScript shared-vector reader passed
all 16 scenarios in its Windows primitives file.

## Packaging

The local package can enroll a computer with `kuvo-box install --join`. Enrollment works with
just the box and its runtime DLLs. Once enrollment succeeds, the package runs
`kuvo-box install --startup` with `kuvo-starter.exe` beside the box. That command installs the
programs in the private home's `bin` folder and registers the starter as `Kuvo.exe`. Native SSH
Add calls the same startup command after enrollment succeeds and before returning the Add
receipt. The account can run `kuvo-box uninstall --startup` to stop the keeper and remove its task.

D4 needs the user's approved name and icon and an Authenticode certificate with a release
signing key. Add the approved Windows resources before signing. Sign the final PE binaries with
SHA-256, add an RFC 3161 time stamp, verify Authenticode, then hash the signed artifacts when making
the signed release manifest. [Microsoft SignTool](https://learn.microsoft.com/en-us/windows/desktop/seccrypto/signtool)
