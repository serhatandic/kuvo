# #636 Add a Machine over SSH, in the box

Written 2026-11-15, from reading `swift/KuvoCode/Views/MachineForm.swift`,
`Stores/Machines.swift`, `Stores/MachineBox.swift`, `Stores/PhoneLink.swift` (`setUp`),
`App/AppModel+Machines.swift` and `OpenCodeKit/…/RemoteEngine.swift` (read only), and the box's
`setup`, `peers`, `mac`, `updates`, `uninstall` and keeper code. Status at the end ("Where it is").

## 0. In short

- A box adds computers: `box/src/link/add/`. A client sends an address, draws the box's steps,
  its question and its words, and nothing else. No SSH, no setup logic and no wording in a client.
- **New routes, not `kuvo/machines`.** `kuvo/machines…` is one of `MAC_APP_ROUTES`: the box hands
  it to the old Swift app, which keeps its own list. The box's own flow is `kuvo/computers/add…`
  (feature `computers.add`), beside `kuvo/computers`, which is already every box's list.
- **SSH is used to set a computer up, and never after.** The Swift app held a tunnel to each
  machine's engine and started that engine itself. A box doesn't: the new computer's box keeps
  itself (`kuvo-start`), and boxes talk through the relay ("Peers"). Clients reach each box
  directly (#742). So there is no connection state, no reconnect loop and no tunnel to port.
- **The host's key is checked by the box**, which the Swift app never did (it failed with "connect
  once in Terminal"). Unknown host: the box reads its key without signing in, shows the
  fingerprint, and keeps it in its own `~/.kuvo-code/ssh/known_hosts` on Trust. Your
  `~/.ssh/known_hosts` is read, never written.
- **Who can add today: a Mac's box that isn't a member of another's setup** (`writerOf().here`).
  Everything under `add/` runs on Linux and is tested there, but only a Mac's box names the
  computers of a setup (`peersOf`, `macPeersBody`, `setMacPeers` are `IS_MAC`): a Linux box as
  main computer is #183's, and the route says so in words until then (§6).

## 1. The step map (Swift → box)

| # | What the Swift app does | Where | In the box |
|---|---|---|---|
| 1 | Address: trimmed; refused if it starts with `-`, has `/`, `:` or white space, or is this Mac | `MachineForm.problem` | `add/address.ts` `addressProblem`, same rules and words |
| 2 | Suggests `Host`s from `~/.ssh/config` (with `Include`, no patterns), not added yet, at most 4, filtered by what's typed | `SSHConfig.hosts`, `MachineForm` | `add/address.ts` `configHosts`: the box says them all (up to 50, not added yet); a client filters as you type (drawing) |
| 3 | `ssh` options: `BatchMode`, `RemoteCommand=none`, `RequestTTY=no`, `ConnectTimeout=15`, `ServerAlive…`, a shared connection in `/tmp/kuvo-ssh-<uid>/%C` | `SSHConnection` | `add/ssh.ts`, the same, plus `StrictHostKeyChecking=yes` and the box's own known hosts file beside yours. The shared connection's folder: `/tmp/kuvo-ssh-<uid>` on a Mac (the footprint's listed exception), `~/.kuvo-code/run/ssh` on Linux |
| 4 | Scripts sent as base64 through `exec sh -c '… eval'`, files and secrets on stdin, never in arguments | `SSHConnection.command` | `add/ssh.ts` `remote()`, the same |
| 5 | Host key: none; ssh's own errors worded afterwards | `RemoteEngineError` | **New**: `add/host-key.ts`. Verdict `known` / `unknown` / `changed`; the question; Trust keeps it. Changed: refused in words (yours: `ssh-keygen -R`; the box's own file: asked again with both fingerprints) |
| 6 | Keys: your `~/.ssh`, config and agent | system ssh | The same (the box's computer's), with `SSH_AUTH_SOCK` from your login shell where the box wasn't started with it. Refused with a key, and the vault unlocked with SSH keys in it: tried once more through an agent of the box's own that offers the vault's keys (`vault/ssh-agent.ts`) |
| 7 | Probe: system and kind, glibc, macOS 13+, developer tools, "is it this Mac", engine there | `RemoteEngineInstaller.check` | `add/scripts.ts` `PROBE`, `add/probe.ts` reads it; same refusals and words |
| 8 | What's there already (`foundProbe`), judged by the box | `foundProbe`, `kuvo/setup/judge` | The same text, judged in process (`setup/judge.ts`); `ours`/`free`/`joins` go on, anything else is the box's question before anything is written there |
| 9 | The same computer under another address (`machine-id`), folded | `Machine.identity`, `fold` | `machine-id` read and made as before; one already here by that id: its address is kept as another of its addresses, answered `known`. No "Same Computer As" by hand (left out) |
| 10 | Never over a newer box (`box-version` newer than this app) | `BoxVersion` | The same: a newer box keeps its files, and is only signed in |
| 11 | Keeper files, by their hash (`.kuvo-boot`) | `installBoot` | One step, "files": the box's own release files (`bin/…`, `bin/runtime/…`, the engine's plugin), as this box has them in its own folder, sent as one tar on stdin; each put in place by rename; skipped when `box-version` and `.kuvo-boot` match |
| 12 | `kuvo-link.js`, catalogs, bots' words, agents' bridge, browser helper, `kuvo-wake`, `kuvo-npx` | `MachineBox.install`, `installAgents`, `MachineBrowserInstaller`, `Wakes`, `Launcher` | Same step: they're all files of the release now (`release/files.ts`) |
| 13 | Engine: downloaded on the Mac, checked, streamed to the machine | `RemoteEngineInstaller.install` | "engine": this box's own binary when the system and kind match; else the pinned package (`shared/engine-pin.json`), fetched and checked here (sha512), kept in `~/.kuvo-code/cache/engine-packs`, streamed. The machine downloads no code |
| 14 | Settings made its own, seeded from the Mac's settings and keys | `MachineBox.adopt` | `box adopt` with no seed: each box keeps its own settings and keys, so nothing of this computer's is copied. Sign-ins are made there (its own first run) |
| 15 | Starts the engine from the Mac (`launchScript`: port, passwords, settings, keys on stdin), holds a tunnel | `RemoteServer.launch`, `attach` | "starting": writes `run/launch.env` (the session's PATH, shell and language, your time zone, its name) and `run/launch.notes`, then `kuvo-start start`: the machine's own keeper starts its engine with a port and passwords of its own, which never leave it. No tunnel |
| 16 | Start at boot: the `@reboot` line, and on a Mac the launch agent | `startAgainScript` | Same text and marks, in the same script as the start (one script, so a removal never lands between them) |
| 17 | Marks the version (`box-version`), keeps `bin/.own` for 0.7 Macs | `BoxVersion.mark` | `box-version` is a release file. `bin/.own`, `mac-installing`, `older-mac`: COMPAT for 0.7 Macs, not ported |
| 18 | Signs it into the setup: `kuvo/link/setup`, `kuvo-link.js setup` on stdin, its answer kept (`phone-link.json`), `PUT kuvo/link/peers`, `POST kuvo/link/docs` | `PhoneLink.setUp` | "joining": `add/join.ts`, the same exchange with no app between: this box's keys and documents, `setup` there, the peer written with `setMacPeers`, its documents taken, `machines.json` and `phone-link.json` written by the box |
| 19 | Reads `kuvo/info` there; older than this app needs: "couldn't be updated" | `bringUpToDate` | The join's answer is checked against the features this box's clients need (`kuvo/info` through `box route`); words if it's older |
| 20 | Connection states, reconnect, back-off, the feed, sign-in port forward, sessions' tunnel | `MachineEngine` | **Not ported**: no connection is held (§0). Signing in on a machine from another computer needs its own way (noted in "Left out") |
| 21 | Updates: every connect puts newer files there | `MachineEngine.start` | Boxes update themselves (signed releases). Over SSH: "Set Up Again" (`POST kuvo/computers/<key>/setup`) runs the same flow at its address, which brings an older box up to this one's files |
| 22 | Remove (off this Mac, runs on) | `Machines.remove` | Already the box's: `DELETE kuvo/computers/<key>` (#735) |
| 23 | Uninstall: forget sign-ins, `kuvo-start off`, `leave`, keys, boot files, `@reboot`, launch agent; kept to run later when it can't be reached | `shutDown`, `removalScript`, `runLater` | `POST kuvo/computers/<key>/uninstall`: one script there (`leave`, the box's own `uninstall` command, sign-ins and keys), then removed here. Not reachable: refused in words, nothing queued (left out: the 30-day queue) |
| 24 | #603: uninstall during a connect could leave an engine and the `@reboot` line | `sendsWhile`, `began`/`off` | One thing at a time per computer (`add/queue.ts`): an uninstall asked while it's being set up stops that try at its next step and runs after the script under way has ended; the try says "Stopping…", the computer "Restarting…" while its box starts again |

## 2. Routes and events (PROTOCOL.md, "Computers", Adding one)

Feature `computers.add`. Every box answers; one that can't add says why.

- `GET kuvo/computers/add` → `{can, why?, hosts: [string], tries: [try]}`; `GET
  kuvo/computers/add/events` follows it (NDJSON).
- `POST kuvo/computers/add {destination}` → `202 {try}`. `400 {message}` for an address that
  can't name one; `409 {message, primary?}` where this box doesn't add computers.
- `POST kuvo/computers/add/<id>/answer {answer}` → `200 {try}`; `409` when it asks nothing now.
- `DELETE kuvo/computers/add/<id>` → stops one under way, forgets one that's over.
- `POST kuvo/computers/<key>/setup` and `/uninstall` → `202 {try}`; offered in `actions`
  (`setup`, `uninstall`, with the box's confirm) on machines this box has an SSH address for.
- A try: `{id, kind: add|setup|uninstall, destination, name, state: working|asking|stopping|failed|done,
  title, steps: [{id, title, state: waiting|now|done|failed, detail?, sent?, total?}], question?:
  {id, title, message, facts: [{label, value, code?}], answers: [{id, label, kind}]}, failure?:
  {title, message, again}, computer?: {key, name}, known?}`. Words are the box's.
- Kept in `run/adding.json` so a box that started again says a try was cut short (never resumed
  by itself: you try again).

## 3. Desktop (gpui)

Computers › Add a Machine, the page that's there: the field takes typing, hosts from the box
under it, Add Machine. Then the same page draws the try: the steps as a card of rows (done,
now with a spinner and its detail, waiting), the question as a card with the fingerprint in the
code font and the box's buttons, a failure as a notice with Try Again, done with "Show <name>".
From Zed's remote dialog (read, not copied): one replaceable status line per phase, the host's
question shown as it is, Escape/Cancel always there, Retry on failure. Where we differ: a page,
not a modal (the area's pages are how this app does New Bot and Add a Machine); the steps stay
listed so a slow copy is seen in place; the fingerprint is a question with buttons, not "type yes".

## 4. Files on this computer and there (footprint)

- Here: `~/.kuvo-code/ssh/known_hosts`, `~/.kuvo-code/run/adding.json`, `run/ssh/` (Linux: shared
  connections), `cache/engine-packs/`. `/tmp/kuvo-ssh-<uid>` on a Mac, as before.
- There: `~/.kuvo-code`, the `@reboot` line, and on a Mac `~/Library/LaunchAgents/
  run.kuvo.code.machine.plist`: all as the Swift app left them (docs/footprint.md's exceptions).

## 5. Tests

- `box/bridge-tests/computers-add.test.js`: the built link as a Mac's box, a stand-in `ssh` (a
  script that runs what it's given in a scratch home, with a host key of its own), the real keeper
  and link there, a stand-in engine. Unknown host → question → Trust → files, engine, start,
  join → listed as a computer; a changed key refused; a refused key; set up again; uninstall;
  uninstall during a try.
- `box/test/add-address.test.ts`: addresses and `~/.ssh/config` hosts, in a scratch HOME.

## 6. Left out, and why

- **A Linux box adding computers**: only a Mac's box writes the peers document (#183). Said by
  the route: "Add computers from a Mac for now…".
- **Adding or making an SSH key in the vault** (#280): the flow uses what's there.
- **Passwords and passphrases typed into the app**: `BatchMode`, as before. A key that needs
  unlocking is said in words.
- **"Same Computer As" by hand, removing one address**: folded by `machine-id` only.
- **Uninstall kept for later** when the computer can't be reached.
- **Signing in to a provider on a machine from another computer** (the Swift app forwarded the
  callback's port over SSH): needs a relay-borne way.
- **The phone's Add a Machine**: no UI in this task.
- **0.7-Mac compatibility on the machine** (`bin/.own`, `mac-installing`).

## Where it is (2026-11-15, second sitting)

**Built and committed on `kuvo/636-add-a-machine-over-ssh-moves-into`** (main merged in, with #742
and #745):
- Box: `box/src/link/add/`, `areas/computers-add.ts`, the routes of §2, `computers.add`, `setup`
  and `uninstall` in `actions`; PROTOCOL.md ("Computers", Adding one); the gate lists; footprint;
  changelog.
- Desktop: `gpui/kuvo/src/computers.rs` (the form, suggested hosts, the try card: steps,
  question, failure, done), `kuvo_box/src/areas.rs`, the stand-in, shots
  `gpui/shots/screens-computer-add*-{light,dark}.png`.
- Tests: `box/bridge-tests/computers-add.test.js` (8), `box/test/add-address.test.ts` (4),
  `gpui/kuvo/tests/computers.rs` (3 new). On lokum: the bridge file and the address tests pass.
- Docs: kuvo-web branch `docs/636` (worktree `~/.kuvo-code/tmp/opencode/kuvo-web-636`):
  machines.md, privacy.md. Phone: branch `kuvo/636-computers-add` (worktree
  `~/.kuvo-code/tmp/opencode/ios-636`): the sample Mac's feature list.

**Decided while building:**
- A try stopped as it starts, on a computer that had no Kuvo Code before, is uninstalled again there.
- Uninstall on a machine another Mac still uses only lets this computer go and leaves it running.
- #603's "Restarting…": the step's words for Set Up Again ("Restarting Kuvo Code there") and the
  stopped try's "Stopping…", not a new `reach` value (clients decode that as a closed set).
- `launch.env` gets no `KUVO_TZ` (the zone is the box's own setting now).
- On the page, one machine is added at a time: while one is under way its steps are the page.

**Not tested (written):** the vault-key fallback (`ssh.ts` `vaultAgent`), the engine fetched for
another kind of computer (`files.ts` `engineFor`'s download), a real `ssh`/`sshd` (the stand-in
speaks ssh's own error lines), a Mac as the computer added on Linux and the reverse.

**Open for the user:** see the final report (the relay and keys of a Mac that never ran the old
app; a machine of another setup; the page's layout alternatives).

**After the reviewer's pass (fixed):** a try being stopped no longer answers for its computer
(what's asked next waits behind it; adding or setting up while it's uninstalled is a 409 in
words; a second Uninstall is the first); answers name their ask (`at`); Trust New Key drops the
old key's line and every other key under that host's name, and the scan writes plain names; Cancel
throws rather than leaning on a stopper; the probe changes nothing (`machine-id` is made as the
computer starts); a copied disk's `machine-id` doesn't fold two computers into one where the card
says its key; a failed clean-up after a cancelled start is said, not swallowed; the copy's
progress and the "already one of yours" words are the box's; a finished uninstall is drawn with
your computers; Try Again never falls through to adding.

**Left from the review:** files copied by an add cancelled before its start stay in `~/.kuvo-code`
there (nothing runs); the vault offers its first four SSH keys only; no box test for the vault
path, cancel mid-copy, `inAnotherSetup` or a restart with `run/adding.json`; no desktop test for
Set Up Again's row or a try card on a machine's page; the phone has no Add a Machine screen.
