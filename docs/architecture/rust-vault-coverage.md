# Rust vault coverage

Row 6.2 follows the TypeScript vault. The shared `vault` conformance area
starts each implementation in an isolated home. It exercises the encrypted
store, unlock and client proof, signed data, the holder socket, the ask book,
command-output masking, completed tool replay, legacy browser sign-in,
SSH signing, and backup export/import. An optional second binary
opens and edits the first binary's wrapped store. The Mac scenario uses an
ad hoc debug Helper and a disposable Keychain specified by its filename.
`mac_stage` adds a released-move scenario using the system Keychain tool
with an explicit disposable file.

## TypeScript cases

A partial case has a corresponding Rust path or shared scenario, with an
uncovered condition in the original test. Other owners are dependencies of
this row. Installed Helper cases still need the released supervision path.

| TypeScript case | Status | Coverage or owner |
|---|---|---|
| [your Mac sets up a machine's vault over SSH, proving it's the Mac that set the machine up, and keeps its key](../../box/bridge-tests/vault.test.js#L205) | Covered | 6.2 conformance |
| [whether a password is the one kept is told without giving it or noting it as shown, and only to a client](../../box/bridge-tests/vault.test.js#L234) | Covered | 6.2 conformance |
| [nothing on the machine shows a value: not its files, not the box's answers to anything without a client's key](../../box/bridge-tests/vault.test.js#L249) | Covered | 6.2 conformance |
| [an agent uses only what it's granted: vault_items lists it, without names of the rest](../../box/bridge-tests/vault.test.js#L260) | Partial | 6.2 and 4.1 agent entry |
| [a bot signs in on its own site; on another site the fill is refused and the bot paused until you look](../../box/bridge-tests/vault.test.js#L273) | Partial | 6.2 |
| [vault_run hands values to a command, never to the agent: its output comes back with them hidden](../../box/bridge-tests/vault.test.js#L291) | Covered | 6.2 conformance |
| [an agent that needs something asks you for it, and gets its name, never its value](../../box/bridge-tests/vault.test.js#L325) | Covered | 6.2 conformance |
| [an item given once is used once without asking again; the next use asks](../../box/bridge-tests/vault.test.js#L339) | Covered | 6.2 conformance |
| [a value stays hidden whatever its item's name, and a key file's strings each hide on their own](../../box/bridge-tests/vault.test.js#L352) | Partial | 6.2 |
| [every use, ask and answer is noted, without a value](../../box/bridge-tests/vault.test.js#L363) | Partial | 6.2 |
| [Pause stops every agent's use at once](../../box/bridge-tests/vault.test.js#L375) | Partial | 6.2 |
| [only a call its conversation is making uses the vault: another command naming the session, or other arguments, gets nothing](../../box/bridge-tests/vault.test.js#L382) | Covered | 6.2 conformance |
| [the vault's files changed outside Kuvo Code count for nothing: a grant written in, or a site added, gives nothing](../../box/bridge-tests/vault.test.js#L397) | Partial | 6.2 |
| [one fill for each call: what races the browser for it gets nothing more](../../box/bridge-tests/vault.test.js#L422) | Covered | 6.2 conformance |
| [Pause isn't undone by deleting its file, nor a grant taken away given back by an older copy](../../box/bridge-tests/vault.test.js#L431) | Covered | 6.2 conformance |
| [a value printed through echo and base64, or as hex, still shows as its item](../../box/bridge-tests/vault.test.js#L453) | Covered | 6.2 conformance |
| [a sign-in only for signing in is never handed to a command or another field, and a value too short to hide isn't kept](../../box/bridge-tests/vault.test.js#L460) | Partial | 6.2 |
| [a file's bytes stay hidden however a command prints them: whole, or wrapped as base64 and PEM wrap it](../../box/bridge-tests/vault.test.js#L475) | Partial | 6.2 |
| [the Mac's proof works once, and what answers on the vault's socket gets the Mac's key only by proving it holds it](../../box/bridge-tests/vault.test.js#L488) | Covered | 6.2 conformance |
| [the Mac reaches the vault with a proof for each request, never its key: one made for another request, or used again, gets nothing](../../box/bridge-tests/vault.test.js#L566) | Partial | 6.2 |
| [a conversation that isn't a bot's is paused too when it's steered to another site, and a sign-in on a name without dots works](../../box/bridge-tests/vault.test.js#L583) | Partial | 6.2 |
| [after the box starts again the vault is locked, until a client's key unlocks it](../../box/bridge-tests/vault.test.js#L606) | Covered | 6.2 conformance |
| [an SSH key you keep for a project signs through its conversations' own agent, as git over SSH uses it, and another project's agent doesn't offer it (on Linux, only a key you keep for everyone)](../../box/bridge-tests/vault.test.js#L674) | Covered | 6.2 conformance |
| [a Mac whose home is short keeps its SSH agents in the home, where the tests look for them](../../box/bridge-tests/vault.test.js#L808) | Covered | 6.2 conformance |
| [the variable that opens the vault to agents opens it only in a test's own folder, never in a real box's, where a line in your shell's profile could set it](../../box/bridge-tests/vault.test.js#L829) | Partial | 6.2 |
| [without the box's own process, a request says so plainly](../../box/bridge-tests/vault.test.js#L847) | Partial | 6.2 |
| [on a box whose agents' commands aren't fenced, agents get none of the vault's tools, but a sign-in you let them make on their own still fills, as before the vault](../../box/bridge-tests/vault.test.js#L853) | Covered | 6.2 conformance |
| [a conversation can connect to its SSH agent from a long multibyte macOS home](../../box/bridge-tests/vault.test.js#L883) | Partial | 6.2 and 1.4 long home and socket startup |
| [while the vault asks you about a sign-in, other conversations keep browsing, and the sign-in goes in once you allow it](../../box/bridge-tests/vault-fill-wait.test.js#L211) | Partial | 6.2 |
| [on Autopilot nothing asks you while it browses: a sign-in, a code, a step or an item from the vault come back at once, left undone, a subagent's too](../../box/bridge-tests/vault-fill-wait.test.js#L229) | Partial | 6.2 |
| [Auto's run on an engine model is its conversation's: on Autopilot, nothing it does waits either](../../box/bridge-tests/vault-fill-wait.test.js#L244) | Partial | 6.2 |
| [an item it's only allowed by asking is left undone on Autopilot, with nothing filled, and its line in the vault's activity says so](../../box/bridge-tests/vault-fill-wait.test.js#L254) | Partial | 6.2 |
| [turned on Autopilot while it waits on you, an ask stops waiting, its card goes, and the agent hears it's left undone](../../box/bridge-tests/vault-fill-wait.test.js#L264) | Partial | 6.2 |
| [a page that left its site while the vault asked you gets nothing](../../box/bridge-tests/vault-fill-wait.test.js#L285) | Open | 6.4 browser recheck |
| [a process without the box's socket key gets nothing from the vault's socket, even while a sign-in calls](../../box/bridge-tests/vault-fill-wait.test.js#L300) | Partial | 6.2 |
| [an item added on this Mac keeps its values in the Keychain, through its app, and nothing of them in the box's folder](../../box/bridge-tests/vault-host.test.js#L75) | Covered | 6.2 conformance |
| [its values come back from the Keychain for a client, and for a bot's fill on the item's site](../../box/bridge-tests/vault-host.test.js#L91) | Partial | 6.2 |
| [something else on this Mac, without the app's word, can't list or read it](../../box/bridge-tests/vault-host.test.js#L101) | Partial | 6.2 |
| [a host.json rewritten by something on this Mac sends nothing of the vault elsewhere: the app proves itself with a secret no file holds](../../box/bridge-tests/vault-host.test.js#L106) | Covered | 6.2 conformance |
| [an older copy of the vault's files put back while the box was stopped counts for nothing: the app keeps their newest generations](../../box/bridge-tests/vault-host.test.js#L135) | Covered | 6.2 conformance |
| [an app from before the generations were has its files' own taken once; a Keychain that can't be read keeps agents waiting, and says why](../../box/bridge-tests/vault-host.test.js#L151) | Partial | 6.2 |
| [with the app not answering, what needs the Keychain says so; removing an item takes its values from the Keychain](../../box/bridge-tests/vault-host.test.js#L174) | Partial | 6.2 delete and Helper refusal covered; reconnect cases open |
| [an agent on a machine lists the vault's tools, in its box's words](../../box/bridge-tests/vault-tools.test.js#L136) | Partial | 6.2 |
| [its bot uses what it's granted, through its box, and never sees a value](../../box/bridge-tests/vault-tools.test.js#L142) | Partial | 6.2 |
| [a session its bot started for its work is the bot's: it uses what the bot may, Always gives it to the bot, and the bot's pause holds it](../../box/bridge-tests/vault-tools.test.js#L159) | Partial | 6.2 |
| [a bot deleted (or gone from this computer) isn't shown as able to use what it could, nor as paused](../../box/bridge-tests/vault-tools.test.js#L183) | Partial | 6.2 |
| [its seat in a group chat asks you each time, whatever it may use on its own, and never for good](../../box/bridge-tests/vault-tools.test.js#L197) | Partial | 6.2 |
| [what its seat asks you for is given once, even given for good](../../box/bridge-tests/vault-tools.test.js#L225) | Partial | 6.2 |
| [its seat signs in with a sign-in it could use on its own only once you say so, in the chat](../../box/bridge-tests/vault-tools.test.js#L244) | Partial | 6.2 |
| [on Autopilot a command needing an item it can't use on its own isn't a failed step: it runs nothing, asks nothing and says what's left](../../box/bridge-tests/vault-tools.test.js#L258) | Partial | 6.2 |
| [a conversation whose root the engine can't say just then asks you, whatever everyone may use](../../box/bridge-tests/vault-tools.test.js#L268) | Partial | 6.2 |
| [a vault wait finds an arrival even without a filesystem notification](../../box/bridge-tests/vault-waits.test.js#L11) | Open | 6.4 browser waiting |
| [a machine's vault is described, changed and followed as a settings page, and no value is ever on it](../../box/bridge-tests/settings-vault-live.test.js#L92) | Open | 6.1 settings |
| [an old helper still running after an update is handed over once, at once, whatever is at work: the new build runs, and is left as it is](../../box/bridge-tests/helper-handover.test.js#L182) | Covered | 7.2 U14 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [not while an update is under way: the old helper is handed over once it's done](../../box/bridge-tests/helper-handover.test.js#L215) | Covered | 7.2 U14 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [one launchd refuses to start is opened instead, so this Mac is never left without its helper](../../box/bridge-tests/helper-handover.test.js#L240) | Covered | 7.2 U14 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [a helper still old after its one try is noted, never tried again](../../box/bridge-tests/helper-handover.test.js#L255) | Covered | 7.2 U14 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [Kuvo stopped on this Mac: its helper is never handed over, nor started, before or as it goes](../../box/bridge-tests/helper-handover.test.js#L296) | Covered | 7.2 U14 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [a helper that renews itself is never handed over by the box](../../box/bridge-tests/helper-handover.test.js#L322) | Covered | 7.2 U14 and U16 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [helper install: the launch agent starts Kuvo Helper at login, with this box's folder, under Kuvo Code in Login Items](../../box/bridge-tests/own-box-helper.test.js#L51) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [helper install --load waits for launchd to let go of the one before, and starts it again once it has](../../box/bridge-tests/own-box-helper.test.js#L85) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [helper install: a path with characters XML reserves is written as launchd reads it](../../box/bridge-tests/own-box-helper.test.js#L103) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [helper start: the whole start through the helper, its launch agent put in place, loaded and started as each needs, one running left as it is](../../box/bridge-tests/own-box-helper.test.js#L115) | Partial | 7.2 U11 and U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62). Signed launchd rehearsal is in 9.3. |
| [helper install refuses what isn't a Mac's own box in a home of its own, and writes nothing](../../box/bridge-tests/own-box-helper.test.js#L183) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [helper says what it doesn't know](../../box/bridge-tests/own-box-helper.test.js#L202) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [a Mac's box in a home of its own says where its server answers, as its helper starts it, and takes it away as it goes](../../box/bridge-tests/own-box-helper.test.js#L211) | Partial | 6.2 |
| [kuvo/box/permissions: what macOS allows Kuvo Helper, as the helper running this box noted it](../../box/bridge-tests/own-box-helper.test.js#L270) | Open | 6.1 settings |
| [kuvo/box/permissions: a permission switched on in System Settings is said as a client asks, the helper asked to look again first](../../box/bridge-tests/own-box-helper.test.js#L340) | Open | 6.1 settings |
| [kuvo/box/permissions: a machine's box runs under no such program, and says so](../../box/bridge-tests/own-box-helper.test.js#L387) | Open | 6.1 settings |
| [Kuvo's desktop app says where it is as it opens: kept for Open Kuvo, never taken for the app the browser opens](../../box/bridge-tests/own-box-helper.test.js#L398) | Open | 7.1 and 7.2 Helper supervision |
| [kuvo/box/at-work and kuvo/box/stop: what Kuvo Helper draws in the menu bar, and Stop Kuvo on this Mac, which the box's keeper does](../../box/bridge-tests/own-box-helper.test.js#L416) | Partial | 7.2 U14 covers the stop markers. The Helper routes belong to 7.1 and 7.2. |
| [kuvo/box/stop: a machine's box isn't stopped so (nothing would start it again at login)](../../box/bridge-tests/own-box-helper.test.js#L437) | Open | 7.1 and 7.2 Helper supervision |
| [helper start after Stop Kuvo on this Mac waits for the stopped helper to go before starting it again](../../box/bridge-tests/own-box-helper.test.js#L447) | Covered | 7.2 U15 in [Rust updates coverage](rust-updates-coverage.md#mac-helper-supervision-from-62) |
| [a Darwin machine under Kuvo Helper reports Files from the Helper's note](../../box/bridge-tests/own-box-helper.test.js#L470) | Open | 7.1 and 7.2 Helper supervision |
| [it's one of this computer's sections, and before there's a vault it says where its first item is added](../../box/test/settings-vault.test.ts#L77) | Open | 6.1 settings |
| [the items are listed by name with what each is and how it's used, and never a value](../../box/test/settings-vault.test.ts#L92) | Open | 6.1 settings |
| [Add Item is checked as it's typed without the values themselves, refused in the same words, and adds an item](../../box/test/settings-vault.test.ts#L111) | Open | 6.1 settings |
| [Edit… opens empty for each value, shows one only through the vault's values route, and saves only what changed](../../box/test/settings-vault.test.ts#L160) | Open | 6.1 settings |
| [Who Can Use It… takes access away from any client, and lets it be used more only from one that makes sure it's you](../../box/test/settings-vault.test.ts#L199) | Open | 6.1 settings |
| [a key's other field is taken away from Edit…, and a new file is added only with its bytes](../../box/test/settings-vault.test.ts#L241) | Open | 6.1 settings |
| [Delete… takes the item away with its values and grants](../../box/test/settings-vault.test.ts#L257) | Open | 6.1 settings |
| [what agents wait on you for is answered from the page: once from any client, always and from now on only from one that makes sure it's you](../../box/test/settings-vault.test.ts#L267) | Open | 6.1 settings |
| [Pause stops every agent's use; a paused bot is let go on](../../box/test/settings-vault.test.ts#L313) | Open | 6.1 settings |
| [the activity says what happened in a sentence, yours plainly, and never a value](../../box/test/settings-vault.test.ts#L329) | Open | 6.1 settings |
| [a locked vault says so and shows nothing else; a request that isn't a client's reads nothing](../../box/test/settings-vault.test.ts#L337) | Open | 6.1 settings |
| [the page followed is sent again as the vault changes, without a value](../../box/test/settings-vault.test.ts#L349) | Open | 6.1 settings |
| [no section's form can carry what's secret, nor read a value but through the vault's own route](../../box/test/settings-vault.test.ts#L365) | Open | 6.1 settings |
| [vault pause accepts the original signed envelope and refuses to replace a future one](../../box/test/settings-vault.test.ts#L405) | Partial | 6.2 |
| [vault activity upgrades mixed lines and keeps a future entry through an append and prune](../../box/test/settings-vault.test.ts#L422) | Partial | 6.2 |
| [the vault socket accepts a legacy hello and refuses a future request before dispatch](../../box/test/settings-vault.test.ts#L442) | Covered | 6.2 conformance |
| [ssh-add lists the conversation's keys, by the same fingerprints ssh-keygen gives them](../../box/test/ssh-agent.test.ts#L80) | Partial | 6.2 OpenSSH listing covered; exact fingerprints open |
| [each kind of key ssh-keygen makes signs through it, and the signature checks out](../../box/test/ssh-agent.test.ts#L96) | Covered | 6.2 conformance |
| [a key the vault doesn't let it use now signs nothing, and one it doesn't offer isn't there](../../box/test/ssh-agent.test.ts#L109) | Partial | 6.2 separate project keys covered; refusal cases open |
| [a key with a passphrase, or that isn't a key, says what to do](../../box/test/ssh-agent.test.ts#L122) | Partial | 6.2 parser exists; encrypted and invalid inputs need coverage |
| [a command that sends a flood of sign requests is cut off, rather than asking the user for each](../../box/test/ssh-agent.test.ts#L129) | Partial | 6.2 bounded queue exists; flood and cancellation need coverage |
| [its socket is yours alone from the moment it's there, never for a moment anyone's](../../box/test/ssh-agent.test.ts#L157) | Covered | 6.2 private socket checked at publication |
| [a watch on its folder hears of its socket, and a closed agent leaves none behind](../../box/test/ssh-agent.test.ts#L167) | Partial | 6.2 registration watch covered; teardown assertions open |
| [a socket the agent can't open is an error on it, never a crash of the box](../../box/test/ssh-agent.test.ts#L212) | Partial | 6.2 bind errors handled; specific refusal case open |
| [a conversation the launcher notes gets its agent at once, though the folder's watch says nothing](../../box/test/ssh-agents-kept.test.ts#L38) | Partial | 6.2 filesystem registration covered; dropped-notification fallback open |
| [a group chat's seat steered to another site pauses its bot, and the notice opens the chat](../../box/test/vault-tripwire-seat.test.ts#L12) | Partial | 6.2 |
| [A loaded Mac Helper that has ended is started again](../../box/test/helper-running.test.ts#L35) | Partial | 7.2 U15 checks kickstart. Actual launchd rehearsal is in 9.3. |
| [An already running Mac Helper is left running](../../box/test/helper-running.test.ts#L40) | Partial | 7.2 U15 checks the existing job. Actual launchd rehearsal is in 9.3. |
| [A Helper launch job that was never loaded is installed](../../box/test/helper-running.test.ts#L45) | Partial | 7.2 U11 checks load and restart. Actual launchd rehearsal is in 9.3. |
| [vault-phones.test.js](../../box/bridge-tests/vault-phones.test.js) | dropped: phone app (retired) | — |
| [vault-waits.js](../../box/bridge-tests/vault-waits.js) | Open | 6.4 browser waiting helper |
| [make-vault-fixture.ts](../../box/test/make-vault-fixture.ts) | Open | 6.1 settings fixture |

## Cases assigned here by other coverage maps

| Prior coverage | Assigned case | Status and owner |
|---|---|---|
| [rust-store-coverage.md](rust-store-coverage.md) | Helper rendezvous decoding and Helper launch-stdin format/recovery. | Partial. Helper scratch conformance exercises current rendezvous and stdin. Legacy and future versions need focused assertions (6.2). Recovery also needs 7.1. |
| [rust-engine-coverage.md](rust-engine-coverage.md) | Tool wording (case 3). Session asks (cases 8–13 and 19). Vault IPC and SSH registration. | Partial. The resident exposes typed session cards, owning conversation and caller, scoped replies, cancellation and waiting updates. The binary adapter to `kuvo_engine::ExternalAsks` depends on 3.2c. |
| [rust-setup-coverage.md](rust-setup-coverage.md) | Signed installed Helper rehearsal, socket key, addressed SSH, vault socket and SSH session directory. | Partial. The released starter and resident recovery run with a debug Helper and disposable Keychain. The signed launchd rehearsal remains in 9.3. Per-session registration runs in scratch. Production enablement needs 6.1 and 4.1. |
| [rust-move-coverage.md](rust-move-coverage.md) | Custom Claude credentials, released move Keychain copy/rollback, interactive saved-site transfer, vault and SSH session paths. | Partial. The Claude credential move and rollback fixtures cover the named credential seam. The app can start the saved-site handover through the box. A scratch key-tool process and loopback vault fixture check verification before removal, conflicts, duplicates, and short passwords. The released desktop flow remains with 6.1 and the signed rehearsal. |
| [rust-backups-coverage.md](rust-backups-coverage.md) | Separate key and no inherited grants; SSH deposits, replacement membership, revoked export, last-app wipe. | Partial. Real wrapped storage and a replacement Mac with a fresh Helper Keychain both restore vault points with a separate key and no inherited grants. Mac enrollment identity, revoked export and wipe orchestration remain with 2.3 and 6.2. |
| [rust-adding-coverage.md](rust-adding-coverage.md) | Vault socket and SSH registration. Darwin launchctl cases at 560, 1993, 2015 and 2069; Helper install and recovery cases at 204, 468, 505, 700, 725, 774 and 793. | Partial. The Unix holder, SSH registration and disposable Helper protocol work here. Installed launch, recovery and wipe remain in 6.2 with 7.1 and 7.2. |
| [rust-adding-coverage.md](rust-adding-coverage.md) | Mac re-enrollment after a lost rotation reply, judges for existing/unknown homes, new-home verdicts, both Mac key replacement cases and all three revoked relay exports. | Open. 6.2 owns the Keychain-dependent proof with 2.3, 2.5, 2.6 and 7.1. |
| [rust-box.md](rust-box.md) | Resident vault survives restart. | Covered by the shared wrapped-vault restart scenario. |


## Row gates

- Keep the production agent gate closed until row 6.1 supplies a verified
  complete fence and row 4.1 supplies active process status. The scratch-only
  switch mirrors the TypeScript conformance entry.
- Row 6.2 owns command-worker survival and recovery during a resident
  restart, setting changes that affect pending asks, Mac Helper recovery
  and signed-file failure cases. Completed command replay is covered. Browser launch and site recheck
  belong to 6.4.
- The app-owned Mac wipe has a vault operation for the 2.3 callback. The
  disposable Keychain test checks removal before detach, removal and retry
  after detach, refusal outside App Support and preservation of a neighboring
  app's service. The prepared-wipe and shutdown path calls it through 2.3c.
- The stock differential runner also compares unrelated resident-owned
  files. Read the vault route and storage path results alongside the
  complete comparison.

## Verification

- On macOS both binaries passed the nine `vault` scenarios, including
  wrapped-store edits in both directions, an ad hoc debug Helper with a
  disposable Keychain, and OpenSSH signing and project scope. The SSH check
  includes an ECDSA P-521 key with a short private scalar, as OpenSSH encodes it.
  The socket scenario checks that an orderly exit removes the listener path.
  It caught the stale socket left by TypeScript and a shutdown race in Rust.
  The Rust Helper scenario uses the released starter. It catches a starter
  that loses the Helper's credential pipe and checks the supervised restart.
  Four filtered Mac move tests passed, including the real disposable
  credential move and injected rollback and resume cases.
- Both boxes also passed a separate Mac recovery scenario. It exports under
  one Helper and imports under a second Helper with its own empty Keychain.
  The replacement reads back the saved sign-in after a restart with a new
  item ID and no inherited grants.
- Native Windows passed all four applicable Rust scenarios. The Unix socket,
  command and SSH checks, the Mac Helper, and the second-binary run were skipped.
- The static x86-64 musl binary passed all seven applicable Rust scenarios
  in Ubuntu 24.04 under WSL. This run also covered Linux's everyone-only
  SSH grants. The Mac Helper and second-binary run were skipped.
- Six differential vault replies and the saved vault files matched. The full
  home comparison retains differences for other areas, including generated
  setup identities and runtime records.
- The shared tests scanned the fixture homes, backup archives, relevant
  replies and captured logs for generated vault values. Agent tool output,
  unauthorized replies, transcripts and files checked had zero matches.
  Approved browser fills and authenticated key delivery intentionally carry
  the corresponding values in memory.
