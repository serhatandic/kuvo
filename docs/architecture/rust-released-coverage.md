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
