# Rust adding and removal coverage

Phase 2.3 covers adding a computer, its enrollment records, and removing it.
Older-home migration belongs to 2.6. This map keeps its callers in view, including
the races that span removal, backup, moves, and an authority change.

## Process scenarios

| ID | Conformance scenario | What it verifies |
| --- | --- | --- |
| C1 | [Two computers agree on membership and a rename travels through the relay](../../protocol/conformance/two-boxes.test.mjs) | A fresh applicant enrolls using an installer code, the computers share the accepted signed roster, and a rename reaches the member. |
| C2 | [An installer code binds one computer, survives a retry, and keeps its identity](../../protocol/conformance/adding.test.mjs) | The issuer stores a token hash, commits membership before a reply lost by the relay, and resumes the same signed identity after a restart. Activation is durable and a different applicant cannot reuse the code. |
| C3 | [An unsupported installer code leaves the new computer’s home untouched](../../protocol/conformance/adding.test.mjs) | Both unsupported prefixes fail before any layout or identity write. |
| C4 | [A Rust applicant leaves an older home for the move](../../protocol/conformance/adding.test.mjs) | A valid code reaches the applicant's layout gate. The older marker stays byte for byte and the applicant does not start migration. |
| C5 | [The probe distinguishes older and current installations on the same computer](../../protocol/conformance/setup-judge.test.mjs) | Both binaries classify saved cards and legacy link lines, including the local computer, another setup, an old phone pairing, installations with incomplete metadata and a future card. |
| C6 | [The main retires only the provisional computer named by a cancellation receipt](../../protocol/conformance/adding.test.mjs) | A stopped installer prints its expired claim’s receipt. The main refuses a forged receipt, retires the exact entry once and allows a new enrollment with fresh keys. |
| C7 | [A seed keeps existing sign-ins and keys and takes portable credentials once](../../protocol/conformance/adding.test.mjs) | The local seed refuses future formats, preserves existing credentials, skips renewing tokens and chooses integration settings from the installed catalog. |
| C8 | [Local discovery chooses the existing machine](../../protocol/conformance/setup-judge.test.mjs) | The startup command chooses an existing member, ignores a stopped installation marked off, and waits for an unfinished move. |
| C9 | [SSH Add trusts the selected host and starts an enrolled resident](../../protocol/conformance/ssh-adding.test.mjs) | The TypeScript fake host waits for explicit trust, installs and signs in the resident. Add preserves one member across two verified addresses and refuses a copied machine ID with a different box key. A member follows attempts on the main, survives a lost start reply, answers the host key question, and deletes the failed WSL 1 attempt. |
| C10 | [Installer enrollment copies only the selected sign-ins and keys](../../protocol/conformance/adding.test.mjs) | Checklist IDs omit secrets and rotating sign-ins. The issuer seals the selected seed at rest. A multipart seed resumes after a failed engine write, preserves the selected key contents and is deleted after an acknowledgment. |
| C11 | [A member lists the main computer’s keys and issues an installer code through it](../../protocol/conformance/adding.test.mjs) | A member shows the main’s checklist, keeps the token on the requesting side and retries a lost reply with the same invitation hash. The selected copy is already sealed and stays usable after its source key changes. |
| U1 | [The main revokes an offline computer once and keeps the unlink receipt after restart](../../protocol/conformance/removal.test.mjs) | The accepted roster immediately excludes the peer and retains its tombstone. Replaying the operation returns the same signed receipt and leaves the membership revision alone. |
| U2 | [A released computer enrolls again when its retained work is unchanged](../../protocol/conformance/removal.test.mjs) | A member starts a standalone setup with a retained database. Rust refuses new work and accepts a checkpoint and rebuild with identical rows. The TypeScript run also observes the Usage scope rewrite. |
| U3 | [A departing member clears credentials and keeps its work before acknowledging unlink](../../protocol/conformance/removal.test.mjs) | A machine retains sessions, clears local sign-ins and keys, and can re-enroll. A Rust machine persists the unlink fence when a SQLite sidecar blocks cleanup and retries after restart. |
| U4 | [A main with incoming devices off still revokes a device and delivers a machine unlink](../../box/crates/kuvo_setup/tests/live.test.mjs) | A Rust main reads the persisted off setting through the Mac Helper path. It refuses a new pairing, publishes a signed desktop revocation and finishes a machine unlink while the setting stays off. |
| U5 | [Uninstalling a stopped machine clears its desktop identity and sign-ins but retains work](../../protocol/conformance/removal.test.mjs) | `uninstall --check` inspects a stopped machine. Uninstall retires the desktop and credential files, marks the machine off, preserves work and can be repeated. Live shutdown, startup entries, backup preparation and remote uninstall remain open. |
| U6 | [A member forwards an unlink and receives the main’s saved decision](../../protocol/conformance/removal.test.mjs) | The relay holds main-to-member traffic through the commit. The forwarded request and a direct replay return the same signed decision without advancing the membership revision. |
| U7 | [The only computer commits a wipe and erases only its old installation](../../protocol/conformance/removal.test.mjs) | A standalone Unix resident returns the signed reinstall offer, stops accepting clients and retires its directory. A sibling reached through a symlink stays intact. Rust enrolls again at the same path with fresh keys. The scenario uses an isolated account, no keeper, and no configured backup. |
| U8 | [A Rust wipe resumes against its detached directory and leaves a replacement alone](../../protocol/conformance/removal.test.mjs) | A fresh process can finish erasing the authorized directory after a crash. The receipt's stored device and inode must match, and the new installation at the original path stays intact. |

## Stored and inherited cases

- `run/link.json` retains the TypeScript invitation, claim, activation and
  signed membership formats. Layout 23 carries provisional recovery commitments
  in `ownershipRecovery`. The applicant adopts a signed roster and reads the
  main’s checkpoint and shared replica through the existing peer exchange.
- The Rust installer writes machine identity under the resident’s home lock.
  Rust reads the historical and Usage-scoped TypeScript release baselines. It
  uses committed SQLite rows when the physical database stamp has changed.
  Mac Helper-owned key rotation and relocated credential stores remain open.
- The existing card writer has process coverage under [setup R1 and R2](rust-setup-coverage.md).
  C5 covers remote probe verdicts. C8 covers the local startup decision before
  a home is written. The fresh SSH probe runs on the remote account. Discovery
  and attach actions for an installed or older remote home stay open.
- The [store map](rust-store-coverage.md) sends the first three authority-format
  cases and the seed, enrollment and install-offer families here. The
  [peers map](rust-peers-coverage.md) sends the incoming-device switch, removal
  and uninstall cases here. The setup map also assigns the cross-phase cases
  below. C7 covers receiving a local seed. C10 covers selecting and delivering
  machine-held credentials by installer code. Mac Helper-held keys, SSH
  credential delivery, the member’s incoming-device switch, member wipe,
  configured backup and move integration at wipe, and release packaging stay open.

## Verification

On macOS, TypeScript passes C1–C3, C5–C8, C10–C11, U1–U3 and U5–U7. Rust passes
C1–C11 and U1–U8. The Rust workspace compile check passes. The Mac-built static
x86-64 musl binary passes C1–C11, U1–U3 and U5–U8 in WSL 2 and in a single
lokum container capped at one CPU and 1 GB. The native Windows binary passes
C1–C8, C10–C11, U1–U3 and U5–U6. The Unix wipe cases are skipped there. The
[native wipe scenario](../../box/crates/kuvo-box/tests/windows_wipe.rs) covers U7 on Windows:
the resident runs from inside the home, hands the erase to a copy of itself in the receipts
folder and exits. The copy detaches and erases the home, and the reinstall at the same path
deletes the copy. The wipe is refused while the installation's sign-in task is registered.
The original TypeScript
fake SSH host, alias, WSL 1 refusal and device-revocation scenarios pass as
reference checks. The Windows PC’s WSL 2 account joined a Rust setup,
acknowledged plain unlink and re-enrolled with fresh keys. Native Windows also
joined, acknowledged unlink and re-enrolled after starting its resident in a
separate foreground SSH session. The native resident launched by Add ended
with the SSH command. Both live exercises used staged executables and isolated
homes. Engine and keeper installation and cross-platform release selection
still need the release path in 7.1 and the native startup owner in 8.1.

| Earlier mixed-only check | Status |
| --- | --- |
| Installer-code enrollment and cancellation in both TypeScript-beta and Rust directions | dropped: TS-beta interop (user 11-23) |
| TypeScript-beta and Rust unlink delivery and cross-binary re-enrollment | dropped: TS-beta interop (user 11-23) |
| Live SSH Add between TypeScript beta and Rust in both directions | dropped: TS-beta interop (user 11-23) |

Released Macs 0.5–0.7.2 and 0.8.0-beta.1 remain in scope. The TypeScript
conformance oracle uses its own residents. Rust live checks use Rust residents.
The tagged 0.7.2 machine retains its settings and identity on both binaries.
The beta.1 fixture retains that behavior on TypeScript. Rust's case stalls
when it waits for the updated beta.1 peers mail. Receiving that released
mail belongs to 7.3. Earlier released Macs still need adding and removal proof.

The differential runner compared stopped TypeScript homes without a file
allowlist. Installer validation refusals and unlink receipt replay, including
a conflicting request, gave equal answers. The positive code issuance differed
in its random bearer token and expiry. Both boxes returned the expected code
scope and saved the invitation under the token hash in C2. The complete home
inventories still differed. The refusal, issuance, and unlink replay runs had
63, 66, and 61 differing leaves respectively. These include startup service
files and compatibility projections. The issuance run also differed in the
invitation hash entries and the TypeScript `run/link.needed` marker.
The discovery differential agrees on both replies and has 60 differing leaves.
The missing-release SSH differential agrees on both replies and has 60 differing
leaves. The credential checklist and two selection refusals agree with 62 home
differences. The differing paths all have verdicts in the inventory table.

Re-enrollment after a TypeScript member's unlink originally failed against
either main. The released baseline hashed the inode and change time for
`agents/usage/settings.json`. The Usage worker rewrote that file when the
member became standalone. U2 now waits for the rewrite and re-enrolls the same
computer with fresh keys. The TypeScript fix retains the previous baseline
format and adds a companion that compares Usage separately from retained work.
Older TypeScript baselines without the companion still take the historical
path. The Rust applicant validates the usage digest and checks other retained
files with the store's native file stamps and a read-only SQLite snapshot.
The Linux test exposed a second TypeScript problem. Bun needed the SQLite URI
open flag to read a closed WAL database in immutable mode. Without it, unlink
discarded the re-enrollment baseline. U2 failed on unmodified TypeScript under
WSL and passed after enabling the flag. Saved work using relocated paths remains
open.

The Rust setup actor owns invitation saves, roster changes and unlink delivery.
The relay adapter authenticates installer packets on the existing connection,
and the applicant owns a short-lived socket plus the home writer lock. A
compare under the shared link lock protects against an intervening TypeScript
write. Layout 23 recovery commitments and the membership change share one
durable write. Unlink completion saves its acknowledgment and delivery status
in the same write before replying. The installer currently requires a stopped
resident to take over its home and conservatively refuses work it cannot
classify. Startup runs the move opener. Applicant enrollment uses the store's
layout gate after code validation so a bad code leaves the home untouched and an
older layout is kept for the move.

## Home inventory verdicts

The six differential runs compared the full stopped-home inventories
with the existing normalization. The runs differed on 59 distinct paths in
total. Every path is listed below. An owner row means a live reader or writer
exists. The directory entries are included with the respective files.

| Paths | Verdict and evidence |
| --- | --- |
| `run/link.json` | **2.3** owns the invitation entries. The code's random bearer token changes the hash used as the entry key. C2 checks that the hash, expiry and scope agree with the reply. **2.1** owns the remaining empty `left`, `macMoved`, `macPeers`, `removalRevisions`, `removed` and `removedHere` compatibility projections. `setup/membership.ts` and the Rust setup actor write these from the accepted roster. |
| `run/setup-card.json` | **2.3, 5.2.** The SSH probe in `add/scripts.ts` and the local reader in `setup/judge.ts` consume the card. The `used` and item-count fields depend on the work inventory and queue. |
| `run/wipe-preparation` | **2.3, 2.5.** `setup/before-wipe.ts` stores durable preparation and export state here. |
| `run/link.needed` | **7.1.** `box/src/keeper/kuvo-start` uses it to start the link. The agents' fence and browser also check the marker. |
| `run/link.version` | **Identical encoding.** Both write a plain build identifier. The identifier reflects the executable. Keeper version selection is **7.1**. |
| `run/route/ssh` | **1.4.** `server/ssh-pipe.ts` selects the TypeScript local SSH endpoint. |
| `run/agents.json` | **2.2.** The peer directory consumes the kept local agent announcement. |
| `run/moves`, `run/outbox`, `run/outbox/sending` | **2.4.** The move worker and `letters/outbox.ts` read the journals and pending letters. |
| `config`, `config/opencode`, `config/opencode/plugins`, `config/opencode/plugins/kuvo-defaults.js` | **3.1.** `engine/defaults.ts` supplies the startup layer and generated plugin. |
| `run/lists/sessions.json`, `run/queues-scanned.json` | **3.2.** Session-list writers and `engine/queue.ts` keep the projection and scan cursor. Port and timestamps can vary, while missing records and stable fields still count. |
| `agents` | **4.1.** `agents/main.ts` initializes the bridge's local home. |
| `bots`, `wakes`, `run/bots-seen`, `run/glance`, `run/glance/asks.json`, `run/notices`, `run/notices/teller`, `run/work`, `run/work/background.json`, `run/work/told.json` | **5.1.** Bot discovery, wake-up scheduling, the glance writer and notice replay use these. `bots/seen.ts`, `work/background.ts` and `notices/told.ts` read the cursors or kept state. |
| `run/github-sign-in`, `run/work-queue`, `run/work-queue/forward` | **5.2.** Queue forwarding and service startup use the saved queue state and sign-in staging. **2.3** uses the forward spool for installer and SSH requests. C11 verifies hash-only installer forwarding. C9 verifies a repeated SSH request within the live main's retry window. Recovery from a main restart still needs durable SSH action receipts. |
| `chats` | **5.3.** The shared chats store uses the home directory named in `link/home.ts`. |
| `agents/limits.json`, `agents/usage`, `agents/usage/claude-files`, `agents/usage/codex-files`, `agents/usage/grok-files`, `agents/usage/insights.sqlite`, `agents/usage/insights.sqlite-shm`, `agents/usage/insights.sqlite-wal`, `agents/usage/settings.json`, `agents/usage/usage.json`, `run/usage-attribution.json` | **5.4.** The usage worker writes the scans, settings, Insights database and attribution. `agents/box-services.ts` reads attribution. SQLite journal presence and stable settings still count. Enrollment's use of the settings scope is **2.3**. |
| `artifacts`, `artifacts/citations-v1.json`, `artifacts/copies`, `artifacts/index-v1.sqlite`, `artifacts/key`, `artifacts/log`, `library`, `library/backfill.json`, `box/marketplace`, `box/marketplace/mcp-relay-v1.json` | **5.5.** The artifact store, library backfill and Marketplace relay consume these. The index and kept records belong to their own services. |
| `box`, `box/approval.json`, `run/fence`, `run/fence/agents.md` | **6.1.** Startup materializes approval and sandbox defaults, and `fence/state.ts` projects the sandbox note. |
| `run/socket.key`, `run/ssh-for`, `run/vault.sock` | **1.4, 6.2.** `kuvo_server/src/ssh.rs` uses the rendezvous key. The TypeScript browser and shared setup client also read it. `link/home.ts` and the fence name the per-session SSH credentials and vault broker paths. |

The stopped-uninstall command also returned matching `--check`, uninstall and
replay JSON after replacing the scratch absolute paths. The six remaining
entries and file hashes matched. The last-computer wipe fixture removes the
home on both binaries and leaves the same receipt format and sibling file.

No path in these inventories was declared unread. The source search covered the
box, Rust crates and desktop. The [move's machine differential](rust-move-coverage.md#verdicts-for-the-machine-differentials)
covers the same startup consumers with older fixtures.

## TypeScript scenarios

This map lists 194 reference cases, including expanded parameterized cases. Four have coverage, 31 have partial coverage, and 159 are open. Of the open cases, 128 list 2.3 as an owner.

### computers-add

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [addressed SSH cannot start, answer, or cancel an Add in its transient process](../../box/bridge-tests/computers-add.test.js#L14) | Open | 2.3 |
| [a Darwin fixture keeper with an empty environment reaches only its fake launchctl](../../box/bridge-tests/computers-add.test.js#L30) | Open | 2.3, 6.2 |
| [a box says whether it adds computers, and suggests the hosts in ~/.ssh/config that aren't patterns](../../box/bridge-tests/computers-add.test.js#L47) | C9 checks availability, literal host suggestions and hiding a saved address. Availability with another release or owner and all configuration variants remain open. | 2.3 |
| [adding a computer: the box shows a host key it doesn't know, and on Trust copies itself there, starts it and signs it in](../../box/bridge-tests/computers-add.test.js#L60) | C9 checks first-use trust, binary copy, signed enrollment and the saved SSH address. Selected sign-ins and installed release, engine and keeper handling remain open. | 2.3, 7.1 |
| [adding a Windows PC with only WSL 1 tells you how to get WSL 2 before copying Kuvo Code](../../box/bridge-tests/computers-add.test.js#L173) | C9 proves the refusal verbatim and that the WSL 1 account is untouched. | — |
| [the same computer under another address is the one already here, not a second one](../../box/bridge-tests/computers-add.test.js#L211) | C9 checks the known reply, preserved membership and saved alias. | — |
| [a host that can't be reached, one that refuses your keys, and one whose key changed are each said in words, and nothing is trusted](../../box/bridge-tests/computers-add.test.js#L224) | Open | 2.3 |
| [a host key that changed from the one you trusted here is asked about again; Cancel leaves everything as it was](../../box/bridge-tests/computers-add.test.js#L261) | Open | 2.3 |
| [setting a computer up again finds its files in place and leaves its running engine alone](../../box/bridge-tests/computers-add.test.js#L289) | Open | 2.3 |
| [a machine whose Kuvo is older is brought up to date by this box whatever is at work there, its running engine left alone; Try Again only once it couldn't be reached for days](../../box/bridge-tests/computers-add.test.js#L302) | Open | 2.3, 7.2 |
| [a machine heard from that never says its version (a box from before they said it) updates in a moment, before any look over SSH, with nothing to press; a look that finds it as new clears it](../../box/bridge-tests/computers-add.test.js#L360) | Open | 2.3, 7.2 |
| [a machine whose address doesn't answer (a home network's while you're away) is reached at another it tells its Mac, its key checked as the one known; with none, it's said in words with Try Again](../../box/bridge-tests/computers-add.test.js#L397) | Open | 2.3, 7.2 |
| [a machine says where else its Macs may reach it: its tailnet's address first, then its name, never a loopback or link-local one](../../box/bridge-tests/computers-add.test.js#L439) | Open | 2.3, 2.2 |
| [once this Mac has moved from the old app, the old app opening again stops nothing: it's told it was replaced, and machines are still kept up to date](../../box/bridge-tests/computers-add.test.js#L445) | Open | 2.6, 7.2 |
| [a phone's Add a Machine (the old Mac app's `POST kuvo/machines`) is this box's own add, waited for; what it must ask you is asked on the Mac](../../box/bridge-tests/computers-add.test.js#L979) | Open | 7.3 |
| [uninstalling while it's being set up waits for the start under way, then nothing runs there and nothing starts it again (#603)](../../box/bridge-tests/computers-add.test.js#L472) | Open | 2.3 |
| [a Mac with earlier Kuvo Code data must update before being added and keeps its data](../../box/bridge-tests/computers-add.test.js#L544) | Open | 2.3, 2.6 |
| [a Mac that's a machine's box already, with earlier Kuvo Code data beside it, is added and set up again as before; that data is left as it is](../../box/bridge-tests/computers-add.test.js#L562) | Open | 2.3, 2.6 |
| [a Mac with an existing app folder is added without copying sign-ins or keys](../../box/bridge-tests/computers-add.test.js#L583) | Open | 2.3, 2.6 |
| [a computer set up again with keys made anew keeps its independently paired app](../../box/bridge-tests/computers-add.test.js#L597) | Open | 2.3 |
| [an add that fails, or is stopped, after Kuvo Code started on a computer that had nothing leaves nothing running there and no key behind](../../box/bridge-tests/computers-add.test.js#L629) | Open | 2.3 |
| [an add stopped while its accounts are being copied, on a computer that had nothing, is taken off there again](../../box/bridge-tests/computers-add.test.js#L655) | Open | 2.3 |
| [a computer that had Kuvo Code before (uninstalled there, its sign-ins kept) is never seeded, and a failed add leaves what it keeps](../../box/bridge-tests/computers-add.test.js#L669) | Open | 2.3 |
| [a phone's Remove, Uninstall and Forget (the old Mac app's `kuvo/machines/<id>` and `kuvo/hosts/<id>`) are this box's own](../../box/bridge-tests/computers-add.test.js#L697) | Open | 2.3, 7.3 |
| [a member of a setup, and a request that can't stay, are told where to add from](../../box/bridge-tests/computers-add.test.js#L751) | Open | 2.3 |
| [a missing or unreadable list of machines isn't overwritten when this box still keeps machines' links (#753)](../../box/bridge-tests/computers-add.test.js#L773) | Open | 2.3 |
| [an account enrolled after the probe keeps its data when the second setup is refused](../../box/bridge-tests/computers-add-recovery.test.js#L12) | Open | 2.3 |
| [a second SSH add cannot install or seed an account reserved by the first](../../box/bridge-tests/computers-add-recovery.test.js#L35) | Open | 2.3 |
| [a saved offline computer reconnects over SSH after its Mac rotates keys](../../box/bridge-tests/computers-add.test.js#L1407) | Open | 2.3 |
| [a credential write interrupted by SSH SIGTERM stops Add and recovery waits for the remote writer](../../box/bridge-tests/computers-add.test.js#L1449) | Open | 2.3 |
| [a credential write interrupted by SSH 255 stops Add and recovery waits for the remote writer](../../box/bridge-tests/computers-add.test.js#L1449) | Open | 2.3 |
| [a failed final release is reported and an abandoned reservation can be cleared explicitly](../../box/bridge-tests/computers-add-recovery.test.js#L127) | Open | 2.3 |
| [a recovery answer only clears the reservation shown to the user](../../box/bridge-tests/computers-add-recovery.test.js#L170) | Open | 2.3 |
| [an old reservation is kept and Add explains how to clear it after a restart](../../box/bridge-tests/computers-add-recovery.test.js#L189) | Open | 2.3 |
| [SSH uninstall cannot probe ownership while Add holds the account](../../box/bridge-tests/computers-add-recovery.test.js#L199) | Open | 2.3 |
| [SSH Add is blocked from the time uninstall probes ownership through deletion](../../box/bridge-tests/computers-add-recovery.test.js#L225) | Open | 2.3 |
| [a reservation creation error says the account couldn't be prepared and logs the SSH detail](../../box/bridge-tests/computers-add-recovery.test.js#L259) | Open | 2.3 |
| [an alias Add honours only its original pairing and cannot restore a key removed during the Add](../../box/bridge-tests/computers-add-recovery.test.js#L298) | Open | 2.3 |
| [uninstall revokes the computer even when its desktop identity folder cleanup fails](../../box/bridge-tests/computers-add-recovery.test.js#L335) | Open | 2.3 |
| [uninstall logs a link-write failure without putting the local path in its result](../../box/bridge-tests/computers-add-recovery.test.js#L369) | Open | 2.3 |
| [uninstall revokes access before SSH cleanup and reports a lost reply without its diagnostic](../../box/bridge-tests/computers-add-recovery.test.js#L392) | Open | 2.3 |
| [a revoked Add stops sending credentials after checking](../../box/bridge-tests/computers-add.test.js#L1799) | Open | 2.3 |
| [a revoked Add stops sending credentials after starting](../../box/bridge-tests/computers-add.test.js#L1799) | Open | 2.3 |
| [SSH uninstall leaves membership intact when the remote pre-wipe backup fails](../../box/bridge-tests/computers-add-recovery.test.js#L459) | Open | 2.3 |
| [SSH uninstall stops if the refreshed peer list drops the target during the probe](../../box/bridge-tests/computers-add-recovery.test.js#L476) | Open | 2.3 |
| [Stop after committed revocation retains the unconfirmed SSH cleanup and local instructions](../../box/bridge-tests/computers-add-recovery.test.js#L506) | Open | 2.3 |
| [a stopped Darwin machine starts through its existing launch job](../../box/bridge-tests/computers-add-darwin.test.js#L90) | Open | 2.3, 6.2 |
| [a stopped Darwin machine reports a refused legacy launch job](../../box/bridge-tests/computers-add-darwin.test.js#L112) | Open | 2.3, 6.2 |
| [an adopted Darwin reinstall reports failure while the bridge returns 503](../../box/bridge-tests/computers-add-darwin.test.js#L135) | Open | 2.3, 7.1 |
| [a Darwin keeper finishes an interrupted fresh install when both services answer](../../box/bridge-tests/computers-add-darwin.test.js#L166) | Open | 6.2, 7.1 |
| [a Linux primary starts fresh, enrolls over SSH and pairs devices](../../box/bridge-tests/computers-add-enrollment.test.js#L18) | C9 runs fresh enrollment and member forwarding from a Linux binary. This path still needs device pairing, follow-up reinstall and release startup coverage. | 2.3, 7.1 |

### install

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [status on a new computer: the pack checked and unpacked, the release's engine fetched once and checked, and the verdict `fresh`](../../box/bridge-tests/install.test.js#L124) | Open | 7.1 |
| [the installer offers reinstall only after the last setup wipe](../../box/bridge-tests/install.test.js#L148) | Open | 2.3, 7.1 |
| [a stopped Mac's authorized last wipe finishes without its helper](../../box/bridge-tests/install.test.js#L204) | Open | 2.3, 6.2 |
| [the last app-owned wipe verifies preparation and leaves a completion receipt outside App Support data](../../box/bridge-tests/install.test.js#L267) | Open | 2.3, 2.5 |
| [two installers at once on a new computer each get their answer: neither takes the other's unpacking or download away](../../box/bridge-tests/install.test.js#L361) | Open | 7.1 |
| [an engine named as a file is only ever a test's: without a test's say-so the release is refused as damaged, and no engine is taken](../../box/bridge-tests/install.test.js#L379) | Open | 7.1 |
| [a damaged copy is refused in words before anything of it runs: a pack that isn't what boxes.json says, an engine that isn't what the release says](../../box/bridge-tests/install.test.js#L387) | Open | 7.1 |
| [beside a box that's here already, nothing is made, and the engine the installer fetched goes once that box has it itself](../../box/bridge-tests/install.test.js#L418) | Open | 7.1 |
| [an existing box updates through its fleet instead of the install command](../../box/bridge-tests/install.test.js#L442) | Open | 7.2 |
| [a computer whose box has another engine than the release's is still told how it stands, offline: asking never waits on a download](../../box/bridge-tests/install.test.js#L451) | Open | 7.1 |
| [new on a Mac: this Mac's own box made in its home from the release and its engine, started by Kuvo Helper, the installer's staging gone](../../box/bridge-tests/install.test.js#L468) | Open | 6.2, 7.1 |
| [a Mac's install cut short is made again: what a making left, and a box made whose helper never got its launch agent](../../box/bridge-tests/install.test.js#L505) | Open | 6.2, 7.1 |
| [a Mac that holds an earlier Kuvo Code's sessions gets no new box beside them: they move instead](../../box/bridge-tests/install.test.js#L564) | Open | 2.6 |
| [install move progress stays in order when rehome is delayed between finishing and settling](../../box/bridge-tests/install.test.js#L594) | Open | 2.6 |
| [an earlier Kuvo Code's chats and settings move with `kuvo-install move`: each step said in the box's own words, its helper started, and the app's own folder as it was](../../box/bridge-tests/install.test.js#L645) | Open | 2.6 |
| [a move launchd wouldn't start: opening Kuvo again starts it with `new](../../box/bridge-tests/install.test.js#L700) | Open | 2.6, 6.2 |
| [a moved Mac whose saved sign-ins are still owed opens and updates without the move or the Keychain: they wait for you, and only you start them](../../box/bridge-tests/install.test.js#L725) | Open | 2.6, 6.2 |
| [a Mac that's one of your machines too is told so in words by the installer, and nothing is moved or stopped](../../box/bridge-tests/install.test.js#L750) | Open | 2.3, 2.6 |
| [on Linux there's nothing to move: said, and nothing made](../../box/bridge-tests/install.test.js#L766) | Open | 2.6 |
| [a copy of the app never judges or installs for the real one, and is refused before anything is written: Kuvo Code's own ID goes with ~/.kuvo-code alone](../../box/bridge-tests/install.test.js#L774) | Open | 6.2, 7.1 |
| [the real home is told however it's written: on a Mac's disk a name in other letters' case is the same folder, there or not yet](../../box/bridge-tests/install.test.js#L793) | Open | 6.2, 7.1 |
| [new on Linux: a machine's box in ~/.kuvo-code, its files and engine in place and started as a computer that's added is; one cut short is made again](../../box/bridge-tests/install.test.js#L810) | Open | 2.3, 7.1 |
| [on Linux nothing is made or set aside under something that still runs in the folder](../../box/bridge-tests/install.test.js#L875) | Open | 7.1 |
| [on Linux a start that fails after its keeper wrote keys and a link is still not a box: `new` is what to run, and it's made again whole](../../box/bridge-tests/install.test.js#L901) | Open | 7.1 |

### installer-enrollment

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a lost merge answer keeps one accepted roster and resumes with both old Work Queues](../../box/bridge-tests/installer-enrollment.test.js#L142) | Open | 2.3 |
| [ordinary enrollment leaves a used independent setup alone](../../box/bridge-tests/installer-enrollment.test.js#L304) | The admission check guards saved state and detectable local work. Exact inventory parity is open. | 2.3, 2.6 |
| [a whole setup comes over while one member is unreachable](../../box/bridge-tests/installer-enrollment.test.js#L330) | Open | 2.3 |
| [a prepared merge can resume with a new code before destination acceptance](../../box/bridge-tests/installer-enrollment.test.js#L415) | Open | 2.3 |
| [a sleeping computer follows both accepted mergers when it returns](../../box/bridge-tests/installer-enrollment.test.js#L452) | Open | 2.3 |
| [a merge reserves Work Queue numbers through the relay lease](../../box/bridge-tests/installer-enrollment.test.js#L494) | Open | 2.3, 5.2 |
| [installer retries a saved join after a lost answer and rejects code reuse by a different computer](../../box/bridge-tests/installer-enrollment.test.js#L538) | C2 covers plain enrollment retries and single-use binding. C10 covers selected credentials and C11 covers hash-only forwarding and a lost reply from the main. The remaining consent and paired-device races are open. | 2.3 |
| [a computer enrolled by installer code receives a verified backup over Relay](../../box/bridge-tests/installer-enrollment.test.js#L919) | Open | 2.3, 2.5 |
| [plain installer code re-enrolls an explicitly released computer with fresh keys](../../box/bridge-tests/installer-enrollment.test.js#L965) | U2 and U3 cover plain unlink, retained databases and key rotation on machine re-enrollment. Mac Helper and relocated data cases remain open. | 2.3 |
| [probe, SSH and installer code protect independent work and grants](../../box/bridge-tests/installer-enrollment.test.js#L1067) | Installer admission checks pairing and work. U2 checks both unchanged and changed retained SQLite work. SSH admission and full queue inventory remain open. | 2.3, 2.6 |
| [a post-claim edit cancels only the provisional installer roster entry](../../box/bridge-tests/installer-enrollment.test.js#L1134) | Open | 2.3 |
| [an expired installer claim can retire its exact entry after a member takes over the issuer’s relay lease](../../box/bridge-tests/installer-enrollment.test.js#L1208) | C6 covers receipt issue, scoped retirement and replay on the original main. Handover, grant and delayed-confirmation variants remain open. | 2.3 |
| [cancelling an unconfirmed installer claim fences its delayed confirmation](../../box/bridge-tests/installer-enrollment.test.js#L1385) | Open | 2.3 |
| [Mac code re-enrollment retries a lost host key rotation reply](../../box/bridge-tests/installer-enrollment.test.js#L1430) | Open | 2.3, 6.2 |

### setup-judge

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [each verdict, as a Mac adding a computer hears it: nothing there, one of ours, this very Mac, a member of another setup (its main computer unheard of for weeks: still not free), another setup's main computer, a machine with old setup records, a lone box](../../box/bridge-tests/setup-judge.test.js#L77) | C5 checks the verdicts through both residents. The remaining saved-roster, copied-machine and installed-app variants are open. | 2.3 |
| [an older box, with no card, is judged by its link.json: set up from iMac, it needs an update first; set up from this Mac, it's ours; one ~/.kuvo-code and an app's box of another setup on one computer are two](../../box/bridge-tests/setup-judge.test.js#L129) | C5 covers legacy probe lines beside another setup. The remaining probe fixtures and local discovery are open. | 2.3, 2.6 |
| [the probe's lines are what a box's link.json says: a box writes it as the probe reads it](../../box/bridge-tests/setup-judge.test.js#L154) | Existing setup tests R1–R2 prove the Rust card writer. SSH probe compatibility is open. | 2.3 |
| [Kuvo Code opening on your Mac mini, which MacBook set up: it attaches to the box that's there, in place, and makes none of its own; its main computer long unheard of changes nothing](../../box/bridge-tests/setup-judge.test.js#L180) | C8 covers attachment to a stopped member without creating an app home. Mac Helper handoff and older link fixtures remain open. | 2.3, 6.2 |
| [Kuvo Code opening where it has its own box: it goes on using it, with nothing else here, beside a ~/.kuvo-code of its own setup, or beside another's (two boxes, settled later), and an app from before cards keeps its own too](../../box/bridge-tests/setup-judge.test.js#L216) | Open | 2.3, 2.6 |
| [here is told where the app's own box is and which folder is the machines', never guesses: without either it says so and stops](../../box/bridge-tests/setup-judge.test.js#L256) | Open | 2.3, 6.2 |
| [here says whether the box there runs: its keeper running now, the box it attaches to](../../box/bridge-tests/setup-judge.test.js#L263) | Open | 2.3 |
| [Kuvo Code attached to a box in place says so to it (box route, as the app runs it there): its card says the app is there from then on; a phone can't say it](../../box/bridge-tests/setup-judge.test.js#L281) | Existing setup test R2 covers the authorized attach marker and refusal. | — |
| [a box its Mac removed is no box, whatever's left of it: its keeper still ending, or marked off before its way to start went; and beside the app's own, no second one](../../box/bridge-tests/setup-judge.test.js#L299) | C8 covers ignoring a member marked off. The simultaneous running-keeper and app-owned cases remain open. | 2.3 |
| [what in the app's own box folder says a box ran there is the vector's, which the app's tests hold the app's list to](../../box/bridge-tests/setup-judge.test.js#L331) | Open | 2.3, 2.6 |
| [an unsupported setup card cannot be mistaken for an empty computer](../../box/bridge-tests/setup-judge.test.js#L336) | C5 covers the refusal through the public probe route. Local startup discovery remains open. | 2.3 |

### mac-installing

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a Mac's note left by a connect cut off goes once it no longer counts; one under way stays](../../box/bridge-tests/mac-installing.test.js#L20) | Open | 2.3 |

### add-address

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [an address is a host's name, user@host or a Host from ~/.ssh/config; anything ssh would read otherwise is refused in words](../../box/test/add-address.test.ts#L13) | Open | 2.3 |
| [the hosts suggested are ~/.ssh/config's Hosts and those of the files it includes, never a pattern, nor one already added](../../box/test/add-address.test.ts#L24) | Open | 2.3 |
| [the files go as one tar the system's own tar unpacks, each whole and with its mode](../../box/test/add-address.test.ts#L55) | Open | 2.3 |

### add-seed

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a sign-in that renews itself is never one to give; a key and GitHub Copilot's are](../../box/test/add-seed.test.ts#L14) | C7 covers receiving the rotating and reusable sign-in types. C10 verifies that selection excludes a rotating sign-in. Sender selection of a reusable GitHub token stays open. | 2.3 |
| [of a box's sign-ins, a new computer gets each account's one in use that stays the same, never an integration's own](../../box/test/add-seed.test.ts#L22) | C10 exercises issuer-side active, inactive and MCP sign-ins. The equivalent SSH delivery remains open. | 2.3 |
| [settings go without what names this computer alone, with keys' files named as the other keeps them](../../box/test/add-seed.test.ts#L34) | Open | 2.3 |
| [the computer added takes them once: its own sign-in for an account stays, one that renews itself is refused, and it says how many, never what](../../box/test/add-seed.test.ts#L42) | C7 covers the deployed seed command end to end. | — |
| [no selection copies no sign-ins or keys and settings do not carry inline secrets](../../box/test/add-seed.test.ts#L80) | Open | 2.3 |
| [a selected catalog key uses the trusted integration configuration](../../box/test/add-seed.test.ts#L107) | C7 proves the seed uses the installed catalog and ignores supplied integration settings. Sender settings selection remains open. | 2.3 |
| [an SSH seed retry can configure its selected integration after adoption already saved settings](../../box/test/add-seed.test.ts#L122) | C7 covers fresh installation and seed replay. Recovery after the SSH adoption phase remains open. | 2.3 |

### add-uninstall

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [uninstall deletes the desktop identity even if the engine and link are already gone](../../box/test/add-uninstall.test.ts#L9) | U5 covers stopped local uninstall. The SSH wrapper and confirmation after a remote failure remain open. | 2.3, 7.1 |
| [uninstall reports incomplete cleanup if the desktop folder cannot be deleted without an engine](../../box/test/add-uninstall.test.ts#L30) | Open | 2.3 |
| [disconnecting a shared box reports a failed departure and keeps its identity](../../box/test/add-uninstall.test.ts#L52) | Open | 2.3 |

### file-addresses

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [each name comes back as the file it names](../../box/test/file-addresses.test.ts#L38) | Open | 2.3 |

### install-offer

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a saved wipe approval is tied to the old installation and only the erasing phase removes it](../../box/test/install-offer.test.ts#L8) | U8 verifies the identity of a detached directory and preserves a replacement. Resume through the installer and app-owned approval and stopping remain open. | 2.3, 6.2 |
| [an app member wipe resumes after detach and yields only a member receipt](../../box/test/install-offer.test.ts#L47) | Open | 2.3 |

### install-verdict

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [fresh: nothing of Kuvo Code's here, or only what's nobody's data (the engine's download, caches, the desktop app's own folder)](../../box/test/install-verdict.test.ts#L32) | Open | 2.3 |
| [ours: a Mac's own box in its home; `new` finishes it only while what starts it isn't in place](../../box/test/install-verdict.test.ts#L45) | Open | 6.2, 7.1 |
| [ours, moved by the box itself: a move that's done is never run again, whatever it still owes (opening or updating Kuvo never waits on the Keychain); its helper not in place, `new` starts it](../../box/test/install-verdict.test.ts#L57) | Open | 2.6, 6.2 |
| [a new box's making cut short isn't a box yet: fresh, to be made again](../../box/test/install-verdict.test.ts#L74) | Open | 7.1 |
| [app-07: an earlier app's data in its folder, no box of its own there](../../box/test/install-verdict.test.ts#L81) | Open | 2.6 |
| [app-box: a keeper build's box ran in the app's folder](../../box/test/install-verdict.test.ts#L89) | Open | 2.6 |
| [moving and stuck: a move's journal, before and past the point it goes back from; one that can't be read is stuck](../../box/test/install-verdict.test.ts#L96) | Open | 2.6 |
| [waits: another process holds the move's lock; one that's gone holds nothing](../../box/test/install-verdict.test.ts#L111) | Open | 2.6 |
| [what a move that's done left to finish holds the lock without a move: bringing saved sign-ins over, or settling what asks nothing, never keeps Kuvo from opening or updating](../../box/test/install-verdict.test.ts#L128) | Open | 2.6 |
| [back: moved back, it moves again only when asked](../../box/test/install-verdict.test.ts#L147) | Open | 2.6 |
| [machine and two: a machine's box in the home is never made over, with or without an earlier app's data](../../box/test/install-verdict.test.ts#L154) | Open | 2.3, 2.6 |
| [on Linux a box's start script alone says a machine's box is there, but one the installer was cut short making](../../box/test/install-verdict.test.ts#L169) | Open | 2.3, 7.1 |

### authority-formats

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [SSH setup and seed settings reject explicit unknown versions before writing](../../box/bridge-tests/authority-formats.test.js#L28) | Open | 2.3 |
| [the preceding and current settings seed and guarded setup token reach their deployed operations](../../box/bridge-tests/authority-formats.test.js#L54) | Open | 2.3 |
| [installer code rejects an older and future prefix before install makes the Box](../../box/bridge-tests/authority-formats.test.js#L69) | C3 proves bad prefixes leave the home untouched through the join entry point. Full installer preparation stays open. | 2.3, 7.1 |

### setup-merge

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a full setup still has space to save an accepted refusal](../../box/test/setup-merge.test.ts#L14) | Open | 2.3 |
| [the roster accepts a source-bound merge receipt and an unused replacement range](../../box/test/setup-merge.test.ts#L24) | Open | 2.3 |

### setup-two-boxes

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [this Mac leads a setup and its machine takes it: kuvo/setup says the same on both, each keeps its card, and both number on where they were, the machine from this Mac](../../box/bridge-tests/setup-two-boxes.test.js#L216) | C1 proves the common roster and propagated rename by installer code. SSH enrollment and numbering are open. | 2.3, 5.2 |
| [a member pairs no phone and adds no computer, naming the main computer, and lists the setup's machines for a phone](../../box/bridge-tests/setup-two-boxes.test.js#L251) | Open | 2.3 |

### setup-mac

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a Mac whose keys were made anew while it doesn't lead lets the setup it led with its old ones go: it numbers as piece 3 says, not as that setup's main computer](../../box/bridge-tests/setup-mac.test.js#L189) | Open | 2.3, 6.2 |
| [kuvo/setup/events says it now, and again as link.json changes, written here or by another process (setup over SSH)](../../box/bridge-tests/setup-mac.test.js#L209) | Open | 2.3, 9.1 |
| [this Mac's static key made anew: setting a machine up again lets the old key go, and the machine follows the new setup at once](../../box/bridge-tests/setup-mac.test.js#L255) | Open | 2.3, 6.2 |

### documents

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a machine its main computer's Mac removes leaves that setup with it, and leads one of its own](../../box/bridge-tests/documents.test.js#L273) | U2 and U3 prove a Rust member starts its own setup after plain unlink. Released-Mac signed roster compatibility stays with 7.3. | 2.3, 7.3 |
| [a machine's Mac sets it up again signing with a new key: it follows that Mac's setup as its new documents say](../../box/bridge-tests/documents.test.js#L285) | Open | 2.3 |
| [a machine no Mac set up leads a setup of its own; once a Mac sets it up, it follows the Mac's (an empty setup joins); its Mac stops naming a setup, and it follows none, keeping the epoch it had](../../box/bridge-tests/documents.test.js#L361) | Open | 2.3 |
| [a Mac lets a machine go only by a document that names it in `left` and no longer among its boxes](../../box/bridge-tests/documents.test.js#L570) | Open | 2.3 |

### devices

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a member its main computer set aside adds no device and vouches for none](../../box/bridge-tests/devices.test.js#L258) | Open | 2.3 |

### devices-two-boxes

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [the desktop app on a member's computer is paired through its own box and reaches every box of the setup, until another device removes it](../../box/bridge-tests/devices-two-boxes.test.js#L297) | Open | 2.3 |
| [a removal reaches a member while the primary's inbound device switch stays off](../../box/bridge-tests/devices-two-boxes.test.js#L439) | U4 covers the saved off setting, refused pairing, replicated revocation and machine unlink. Live switch changes and the connection-level checks remain open. | 2.3, 6.2 |
| [local uninstall refuses to clear a primary while a member holds an open session](../../box/bridge-tests/devices-two-boxes.test.js#L701) | Open | 2.3 |
| [local uninstall revokes its desktop before stopping the box, including a session open to its primary](../../box/bridge-tests/devices-two-boxes.test.js#L739) | Open | 2.3 |

### peers

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [a machine a Mac removed no longer counts that Mac: its peers and lists go, another Mac's stay](../../box/bridge-tests/peers.test.js#L851) | Open | 2.3 |
| [a Mac's peers document by mail, or passed on by a peer, counts once it's that Mac's signature; a list a peer passes on unsigned, never](../../box/bridge-tests/peers.test.js#L947) | Open | 2.3, 7.3 |

### setup-handover

| Reference scenario | Rust status | Owner of remaining work |
| --- | --- | --- |
| [an offline member follows two handovers and the former primary can be unlinked](../../box/bridge-tests/setup-handover.test.js#L392) | Open | 2.3, 9.3 |
| [an SSH-enrolled member advertises its accepted epoch and catches up](../../box/bridge-tests/setup-handover.test.js#L428) | Open | 2.3 |
| [unlink retries credential cleanup before acknowledging completion](../../box/bridge-tests/setup-handover-access.test.js#L18) | U3 covers a machine’s durable cleanup fence and restart. Helper and relocated stores remain open. | 2.3, 6.2 |
| [an unmatched pre-join pairing requires an explicit enrollment decision](../../box/bridge-tests/setup-handover-access.test.js#L96) | Open | 2.3 |
| [local uninstall refuses a primary before clearing identity or stopping it](../../box/bridge-tests/setup-handover-access.test.js#L124) | Open | 2.3 |
| [access routes return committed receipts from the main and through a member](../../box/bridge-tests/setup-handover-access.test.js#L143) | U1 proves the main’s durable unlink receipt. Access variants and member forwarding are open. | 2.3 |
| [forwarded computer changes carry receipts through the member route](../../box/bridge-tests/setup-handover-access.test.js#L163) | U6 covers a forwarded plain unlink and a lost reply. Paired-device replay after revocation, other computer changes, and a writer handover remain open. | 2.3, 5.2 |
| [a scoped rename cannot overwrite a later name after primary handover](../../box/bridge-tests/setup-handover-access.test.js#L173) | Open | 2.3 |
| [live permission replies from paired clients expire across pc5 reconnect](../../box/bridge-tests/setup-handover-access.test.js#L207) | Open | 2.2, 2.3 |
| [clients share machine changes and revocations across three boxes](../../box/bridge-tests/setup-handover-access.test.js#L303) | Open | 2.3 |
| [online unlink from a member keeps the target’s sessions and bots standalone](../../box/bridge-tests/setup-handover-removal.test.js#L14) | Open | 2.3, 2.4, 5.1 |
| [an offline member is unlinked at commit and wipes after returning](../../box/bridge-tests/setup-handover-removal.test.js#L33) | Open | 2.3, 2.5 |
| [revoked Relay-only headless member accepts a verified removal export after reconnect](../../box/bridge-tests/setup-handover.test.js#L2467) | Open | 2.3, 2.5, 6.2 |
| [revoked Relay-only app-owned Mac with a shutdown write accepts a verified removal export after reconnect](../../box/bridge-tests/setup-handover.test.js#L2467) | Open | 2.3, 2.5, 6.2 |
| [revoked Relay-only app-owned Mac with a lost export accepts a verified removal export after reconnect](../../box/bridge-tests/setup-handover.test.js#L2467) | Open | 2.3, 2.5, 6.2 |
| [unsupported erase layout stays pending and can cancel without losing work](../../box/bridge-tests/setup-handover-removal.test.js#L279) | Open | 2.3 |
| [a missing computer stays wipe-pending across primary restart](../../box/bridge-tests/setup-handover-removal.test.js#L296) | Open | 2.3 |
| [a lost disk closes a pending wipe and preserves the key revocations](../../box/bridge-tests/setup-handover-removal.test.js#L309) | Open | 2.3 |
| [unlinking the only computer wipes its local setup](../../box/bridge-tests/setup-handover-removal.test.js#L327) | U7 verifies the signed receipt and directory retirement on a sole Unix resident without a keeper or configured backup. The full three-computer prelude, action list, keeper, and reinstall handling remain open. | 2.3, 7.1, 8.1 |
| [a sole primary can supersede and cancel a prepared removal](../../box/bridge-tests/setup-handover-removal.test.js#L366) | Open | 2.3 |
| [a pending wipe follows the setup through handover](../../box/bridge-tests/setup-handover-removal.test.js#L395) | Open | 2.3 |
| [a failed pre-wipe backup leaves the removed computer’s data in place](../../box/bridge-tests/setup-handover-removal.test.js#L412) | Open | 2.3, 2.5 |
| [move all verifies accepted ownership before wiping the source](../../box/bridge-tests/setup-handover-export.test.js#L14) | Open | 2.3, 2.4 |
| [an offline Relay source completes its accepted moves and export on reconnect](../../box/bridge-tests/setup-handover-export.test.js#L56) | Open | 2.3, 2.4, 2.5 |
| [an encrypted export survives wipe and imports through restore](../../box/bridge-tests/setup-handover-export.test.js#L110) | Open | 2.3, 2.5 |
| [a missing listed attachment blocks a requested export and wipe](../../box/bridge-tests/setup-handover-export.test.js#L181) | Open | 2.3, 2.5 |
| [legacy export readiness cannot authorize a wipe](../../box/bridge-tests/setup-handover-export.test.js#L210) | Open | 2.3, 2.5 |
| [a failed requested export revokes access and keeps source data](../../box/bridge-tests/setup-handover-export.test.js#L281) | Open | 2.3, 2.5 |
| [an unreachable source keeps a requested export and wipe pending until it returns](../../box/bridge-tests/setup-handover-export.test.js#L308) | Open | 2.3, 2.5 |
| [an offline source’s accepted export preparation survives primary handover](../../box/bridge-tests/setup-handover-export.test.js#L337) | Open | 2.3, 2.5 |
| [move-only wipe waits for an explicitly revised, byte-verified queue plan](../../box/bridge-tests/setup-handover-wipe.test.js#L16) | Open | 2.3, 2.4, 5.2 |
| [removal preview discloses unselected payloads and missing retained artifacts](../../box/bridge-tests/setup-handover-wipe.test.js#L84) | Open | 2.3, 2.5 |
| [an unreadable bot remains unresolved and blocks move-all deletion](../../box/bridge-tests/setup-handover-wipe.test.js#L114) | Open | 2.3, 2.4, 5.1 |
| [a revised removal keeps completed moves and replaces a failed export](../../box/bridge-tests/setup-handover-wipe.test.js#L138) | Open | 2.3, 2.4, 2.5 |
| [a WAL checkpoint preserves an accepted wipe generation](../../box/bridge-tests/setup-handover-wipe.test.js#L165) | Open | 2.3, 2.5 |
| [source work added after readiness invalidates the wipe grant](../../box/bridge-tests/setup-handover-wipe.test.js#L197) | Open | 2.3, 2.5 |
| [a session attachment added after readiness invalidates the wipe grant](../../box/bridge-tests/setup-handover.test.js#L3435) | Open | 2.3, 2.5 |
| [a session attachment changed after readiness invalidates the wipe grant](../../box/bridge-tests/setup-handover.test.js#L3435) | Open | 2.3, 2.5 |
| [a deleted member retries a lost completion from outside its folder](../../box/bridge-tests/setup-handover-wipe.test.js#L266) | Open | 2.3, 2.5 |
| [a mixed-build update cut and lost move reply survive disk loss, restore, and replay](../../box/bridge-tests/setup-handover-recovery.test.js#L258) | Open | 2.3, 2.4, 2.5, 9.3 |
| [relay lease: a two-computer main pairs a phone and adds a computer with the other stopped](../../box/bridge-tests/setup-handover-lease.test.js#L31) | C2 covers ordinary enrollment under a valid lease. Device and independent offline member combinations are open. | 2.3 |
| [main saves alone: after unlinking one of two computers new items get numbers](../../box/bridge-tests/setup-handover-lease.test.js#L111) | U1 proves revocation with only the main online. Queue allocation following removal is open. | 5.2 |
| [main saves alone: a dead standby is unlinked in one request](../../box/bridge-tests/setup-handover-lease.test.js#L144) | U1 proves revocation and stable replay before the offline peer acknowledges. The dead-standby variant remains open. | 2.3 |
| [unlinking the main of two computers hands its role to the other, on an older version, and unlinks it](../../box/bridge-tests/setup-handover-lease.test.js#L225) | Open | 2.3, 9.3 |
