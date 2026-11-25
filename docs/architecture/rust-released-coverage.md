# Released Mac coverage (7.3)

The public predecessors are 0.5–0.7.2 and 0.8.0-beta.1. The latter's peer lists
are unsigned and authoritative only for their direct SSH enroller. The
current signed setup path continues to use the accepted checkpoint. The
TypeScript beta beyond beta.1 is not a live compatibility target.

## Part A: historical setting verbs and unsigned roster mail

| Case | Proof |
| --- | --- |
| R1 | The two tagged machine homes in the `compat` conformance area keep their keys, secret and name at layout 23. The beta.1 machine receives a packet written by the tagged bundle, and keeps its newer roster at the original path and shape. |
| R2 | A beta.1 Mac with no saved address gives its first roster. The live computer list includes the named computers other than the local key. A malformed peer is omitted. An older list and a list sent by another peer change nothing; a further direct Mac list removes a computer. The agent announcements provide deterministic delivery barriers. |
| R3 | The beta.1 Mac's tagged SSH transcript reads unowned settings, refuses a change before adoption, seeds them, changes them, saves a key, reads them, and reads bots through a running resident. Repeated adoption does not overwrite the updated settings; clearing the key removes its file. The historic reader ignores additive settings and route fields. |
| R4 | The two released app homes still move their conversations, messages, attachments, and settings without changing the original app's link file. |

Part A results: Mac TypeScript and Rust all six **released** cases pass;
native Windows, WSL Ubuntu 24.04 (musl), and Linux (static musl on lokum in a
one-CPU/1-GB container) each pass four machine cases, with the two Mac app
moves inapplicable. The Rust engine-sessions area passes 81/81; TypeScript
compile check passes. The Rust baseline failed exactly on the missing
beta.1 roster packet and the missing `box read` command.

The full both-box `compat` run additionally includes the current TypeScript
beta durable fixture: TypeScript 7/7, Rust 6/7. The current-beta case fails on
a repeated run because `agents/sessions/ses_golden/session.json` has added
idle/outcome fields. The Foreman assigned this regression to 5.1, not to
released compatibility. The released tests remain unchanged and green.

The differential runner opened the same tagged beta.1 machine seed in both
binaries and compared `GET kuvo/config` and `GET kuvo/bots` and the home
files. Configuration, the historical bots array, identity and the legacy
roster projection match. The full comparison reports additional modern bot
fields (elsewhere, held, pendingComputers and refused), runtime-file
projections, signing seed and document counters. These are outside Part A;
the Foreman assigned the modern fields/runtime discrepancies to their
respective resident owners. The historical reader ignores those additive
fields. The exact released settings transcript and roster updates are
also compared against the same tagged oracle by R1–R3.

| TypeScript case | Port / remaining owner |
| --- | --- |
| `peers.test.js` 44, 57 | R1–R2: on-disk legacy projection, live newer, older and non-enroller lists, peer omission. |
| `peers.test.js` 77, 100 | 7.3: legacy enroller online-time expiry and clock changes still open. |
| `peers.test.js` 878 | R2: an address-less Mac's roster mail is taken. Explicit no-answer check for an address-less signed enroller remains 7.3. |
| `peers.test.js` 950, 980 | R2: a peer cannot supply an unsigned roster; the unsigned list isn't a signed document. Signed legacy authority and replacement/re-enrollment remain 7.3 / 9.3. |
| Tagged `compat.test.mjs` | R1, R3, R4. Current-beta durable case owned by 5.1. |
| `box read/adopt/change/key/route/personalities` | R3 exercises the binary; R6 runs beta.1's own MachineBox shell lines through the engine runtime and the pack's delegate; R7 the personalities save. |

## Part B: overwritten tools and the released command path

| Case | Proof |
| --- | --- |
| R5 | `compat-repair`: an earlier Mac's launcher, keeper and browser files are put back from the selected signed pack (not during `run/mac-installing`); a sound visit writes nothing; the stale `bin/.own` goes; `run/repair.json` records it. A damaged saved pack is removed and only the running release's signed pack is fetched again; a bad download changes nothing. The live saved keeper (`bin/.kuvo-start-keeper`) repairs an overwritten `kuvo-start` and `kuvo-launch`, moves a day-old `run/attached` to `run/older-mac`, registers its own `@reboot` line on Linux, repairs a later visit, and keeps the same resident answering. Both boxes give the same busy, repaired and sound transcript for the same overwrite. The 0.7 leave-running guard (its tagged shell) holds exactly while `run/link.seen` is fresh. |
| R6 | `compat-shim`: the pack builder emits `bin/kuvo-link.js`, a delegate that only forwards `box adopt/read/change/key/route/personalities` to `bin/kuvo-box` (stdin, stdout and exit code through). beta.1's tagged `RemoteEngineInstaller.script` and `MachineBox.lastLine` lines find the Rust-installed pinned engine (beta.1's own 2.0.32 is absent, so its newest-engine fallback) and run its SSH transcript, every verb through the delegate. The pack test (`release-catalogs`) checks the delegate and the saved keeper in all five target packs. |
| R7 | `compat-shim`: `box personalities` merges beta.1's library at a main computer, answered as before (`personalities`, `removed`, `operation`, `save`); a member refuses with the main-computer message. The Rust command hands it to the resident's internal `kuvo/personalities/exchange`. |

| R8 | `compat-repair` on WSL 2 (real Windows Task Scheduler, disposable tasks): `box wsl-startup <distro>` (the installer's same registration) registers this installation's hidden sign-in task for the saved keeper, starts it so Windows runs the keeper inside the distribution, keeps only Linux folders in the saved PATH, refuses another distribution, and `uninstall --check` / `uninstall` report and remove the task (one already gone counts as removed). A task an earlier release registered for `bin/kuvo-start` is registered again for the saved keeper with its account and trigger, and a later overwrite can't stop it starting. The Linux installer registers the task instead of a crontab line on WSL 2; a released WSL machine's upgrade registers it again. |

Part B results (Rust): macOS `compat-repair` 5/5 (2 WSL-only skips), `compat-shim` 2/2,
`compat` 7/7; TypeScript passes every case that applies to it (the saved keeper, the shim and
WSL are Rust-pack only). Linux (static musl, lokum, one CPU / 1 GB): repair 4/4, shim 2/2,
released compat 4/4. WSL 2 Ubuntu 24.04 (musl): repair 6/6, shim 2/2, released compat 4/4.
Native Windows: repair 3/3, shim 2/2 (the delegate under the pinned Windows engine), released
compat 4/4. Off the Mac the current-beta compat case needs Bun to generate its home and the app
moves need APFS, so they don't apply there.

**Row 9.3: the Helper's keeper path.** Kuvo Helper starts `bin/kuvo-start` itself
(`mac/helper/Sources/KuvoHelperKit/BoxHelper.swift`, `start`, its folder check and its version
read). On a Rust pack it must start `bin/.kuvo-start-keeper` when `bin/kuvo-box` and that file
are there, and `bin/kuvo-start` otherwise, as `kuvo_keeper::shell_keeper` picks. The proving
case: a Helper test where the Rust pack's `bin/kuvo-start` has been replaced by a 0.7 Mac's
(`exit 1`) still starts `keep` from the saved keeper, which then repairs `bin/kuvo-start`; and
the 9.3 rehearsal's released-Mac visit to the Mac's own box.

## Part C: `stays` and `next` items

| Item | Proof or owner |
| --- | --- |
| 0.7 files, keeper, leave-running | R5. The Helper's own keeper path is row 9.3 (below). |
| 0.7 first-launch | `compat-stays`: `kuvo/accounts/found` lists an opencode's keys and sign-ins (never a key), the engine's shell keys and connected providers, and the coding agents, saying which parts weren't read; `/use` copies a key into the engine once, refuses a sign-in and one that's gone. The Rust box advertises `accounts.found`. New in Rust (`serve/found.rs`). |
| 0.7 engine-stop | `compat-stays`: with `run/stopped`, `kuvo/info` says stopped, the engine's routes answer "The engine isn't running.", there's no Stop, and Start takes the older Mac's Stop away. |
| 0.7 visits | `updates`' "the shell keepers wait for a preparing member, then an offline computer catches up" seeds `run/older-mac` and an older Mac's launcher and still updates the fleet (7.2); R5 records the visit. |
| 0.7 keeper-number | `bin/kuvo-browser.js`, which carries `keeper: 51`, ships unchanged in every Rust pack (`release-catalogs`); its own scenarios are `browser-keeper-version.test.js`. |
| 0.7 app-agents-stopped | Only for a box still in the retired app's folder. A Rust Mac box always lives in its own home: the move takes it there and leaves `run/agents-stopped` behind (`kuvo_move/src/mac.rs`, move cases C1/C2). |
| 0.7 bot-json-job | `agents-bots`: a changed job's line is in the bot's `bot.json` beside its `job.json`. |
| 0.7 own-envelope | `compat-stays`: a message a client wrote the blocks of itself (`text`, no `words`) reaches the engine as written, with `metadata.kuvo.parts`; a plain one gets none. The phone's `<from-phone>` marking goes with the phone app. |
| 0.7 limits-own-numbers | Row 5.4: it sits on peer plan limits (`limits-peers.test.ts`, all open there). |
| beta.1 box-read-change, peers-list | R1–R3, R6. |
| beta.1 deleted-bots | `agents-bots`: a deleted bot is first in `bots/deleted.json`. |
| beta.1 mac-moves | Open: Rust keeps a beta.1 Mac's legacy `moved` list but doesn't use it for where a conversation lives. |

**Not served by design** (the lead, 2026-11-25): beta.1's `kuvo-link.js setup` (its
phone link sync; the phone app is dropped), `kuvo-link.js leave <key>` (removing a machine
from a beta.1 Mac before it updates) and `box bot-keys-refresh` (the TypeScript box refused it
too). The delegate refuses each with exit 2: "Update Kuvo Code on this Mac to use this computer."

**The delegate goes with the TypeScript box**, 4 weeks after the last switch, with the
`box read/change/key` markers.

Rust differs from the TypeScript structure here on purpose. Repair verifies the signed
envelope and the pack digest, then hashes each installed file against the packed manifest and
extracts only the wrong files into a draft beside the release, renaming each into place; the
TypeScript keeper copied back `bin/.own` by modification time. A beta.1 home opened by Rust
leads its own setup, so `box personalities` saves through the setup rather than the legacy
file, and needs the resident (the keeper keeps it running).

Remaining cases and owners are also indexed in the move, moves, keeper, peers
and setup coverage maps. In particular the repair of overwritten tools,
legacy enroller expiry/unlink, signed-roster coexistence, leave-running
integration, legacy zone/personality and device mail, and the remaining
`stays` / `next` items are not yet complete.
