# How Kuvo works

At `5ae7178f3` (22 November 2026). Paths are under `box/src/link/` unless they say otherwise.
What's left to remove, numbered as findings, is in `simplicity-check-1122.md`.

Every computer runs the same **box**: the engine, the coding agents, their conversations, bots and
wake-ups. Each box works on its own, with no client connected. One of them is the **main
computer**, which alone decides what the whole setup shares. **Clients** (the desktop app, the menu
bar) show what a box says and ask it to change things. Computers reach each other through
**Kuvo's relay**, end to end encrypted, so the relay can't read what they send.

**Marked** rows have one current mechanism, plus an older one still beside it.

| Concern | The one mechanism | Where |
|---|---|---|
| Who's the main computer | A 60-second lease at Kuvo's relay. The main renews it every 10 s. When it lapses, another computer claims it: always-on computers at once, the rest a minute later. The new main signs a certificate for the next epoch, and the rest follow. A main without a fresh lease pauses setup-wide changes and says so. **Marked:** new setups are still created in the old model, and setups that haven't converted keep its branches (check 7, 9). | `relay/src/leases.ts`; `setup/relay-lease.ts`, `lease.ts`, `takeover.ts`, `leadership.ts` |
| Shared data | The main saves setup-wide values alone: membership, devices, settings, personalities, labels. It publishes them as one signed copy. Members keep that copy, so whoever takes over starts from it. | `setup/shared.ts`; `box/src/shared/data-declarations.ts` |
| Updates | The main holds the newest signed release: from the feed, from Update Now, or from what a member offers. Every computer that's on prepares it beside the version it runs, then says it's staged or why it can't be. The main switches when every computer that's on is staged, or 5 minutes after the first one is. The others switch when they see it running. A switch is one rename of the `bin` link, and the keeper switches back if the new version doesn't answer. A computer that's off catches up when it's back. The desktop app updates itself through Sparkle, as an installed app does. **Marked:** the old trial, engine swap and quiet-moment wait still run for computers whose `bin` isn't a link yet (check 8). | `updates/fleet.ts`, `update.ts`, `keep.ts`; `box/src/keeper/kuvo-start` |
| Where logic lives | The box decides, and clients draw. One set of routes serves every client, and a feature list says what each box has. | `server/router.ts`, `areas/*`; `box/src/shared/features.ts`; `protocol/PROTOCOL.md` |
| Who owns a conversation, bot or group, and how others find it | The computer it lives on. A move hands it from one computer to another, and only those two take part. Other computers find it in what each computer announces. A request that reaches the wrong computer goes on to the owner, or the client is told where the record is. **Marked:** the lookup also reads records from removed mechanisms, and announcements list only 40 sessions (check, ownership section). | `moves/move.ts`, `moves/records.ts`; `ownership/resolve.ts` (`ownerOf`); `peers/agents.ts`; `ownership/messages.ts` |
| Running agents | The **engine** (opencode, unmodified, one folder per version) runs model sessions. The **agents bridge** runs Claude Code, Codex, Cursor, Grok and Antigravity. Each agent's process runs under a **holder** of its own, so restarting or updating the bridge doesn't cut a turn, and the new bridge takes the holders over. The box keeps each session's queue. | `box/src/agents/` (`kuvo-agents.js`), `box/src/agents/hold/`; `engine/queue.ts` |
| How clients reach Kuvo | One process per computer (`kuvo-link.js serve --resident`) answers three ways: the local app over loopback HTTP (password in `run/serve.json`), SSH (`box route-to`, passed to that process over a private socket), and the relay (Noise sessions). All three go through one dispatcher. **Marked:** the split launch (`run` plus `serve --kept`) and the plain `box route` over SSH are still kept (check 10). | `server/resident.ts`, `serve.ts`, `ssh-pipe.ts`; `relay/host.ts`; `main.ts`; `gpui/kuvo_box/src/transport.rs`, `local.rs`, `ssh.rs` |
| Getting between computers | Peer mail: messages encrypted with Noise between computers' keys, which the relay passes on without reading. A computer is added over SSH or with an installer code. Big things go as mail in parts: moves, and release packs when no download works. | `relay/src/relay.ts`; `peers/mail.ts`, `known.ts`, `reach.ts`; `add/` |
| Keeping Kuvo running | The keeper, `bin/kuvo-start`, on every computer. It starts and watches the box, the engine, the bridge, the browser and wake-ups. On Linux an `@reboot` line starts it. On a Mac, Kuvo Helper starts it: a signed launch agent, which also holds Keychain access. | `box/src/keeper/kuvo-start`; `add/scripts.ts`; `helper.ts`; `mac/helper/Sources/KuvoHelperKit/BoxHelper.swift` |
| Stored data and formats | Everything is in `~/.kuvo-code`. Each kind of data is declared once, with its owner, backup and restore. Each file carries a format version. A change older builds can't read goes to a new path, so going back a version always works. **Marked:** a folder layout number, with 20 empty steps and 12 readiness checks, still sits beside this (check 11). | `box/src/shared/data-declarations.ts`, `format-version.ts`, `versioned-file.ts`, `folder-version.ts` |
| Backups and wipe | A daily backup, and one before a wipe, each kept 30 days, to a disk or another computer, with restore. **Unlink + wipe** is one accepted removal: access ends at once, and the computer erases only after its backup and planned moves are verified. | `backup/engine.ts`, `destination.ts`, `relay-store.ts`, `restore.ts`; `setup/removal*.ts`, `wipe.ts`, `before-wipe.ts`; `uninstall.ts` |
| Secrets | The vault. A Mac keeps values in the Keychain, through Kuvo Helper. Linux keeps them in encrypted files, whose key is wrapped by an unlock key that only your clients hold. Coding agents keep their own sign-ins where they always do. | `vault/store.ts`; `mac/helper/Sources/KuvoHelperKit/HostKeychain.swift` |
| The browser | Each box runs its own Chrome for Testing, kept by `kuvo-browser.js host`. Agents browse with its tools, and clients watch and use it live in the side panel. | `box/src/browser/kuvo-browser.js`; `browser/install.ts`, `view.ts`; `gpui/kuvo_box/src/browser_view.rs` |

## Features on top

- **Bots:** a bot lives on one computer, its home, and moves the way a conversation does (`bots/`).
- **Wake-ups:** each box fires its own sessions' and bots' wake-ups, with no client connected (`box/src/keeper/kuvo-wake`).
- **Work Queue:** each item belongs to its queue's owner. Numbers come in blocks from the main, and the relay lease keeps the highest one given out (`work-queue/`, `work-queue/numbers.ts`).
- **Usage:** each box reads its own accounts and tells its peers, so every box lists them all. The menu bar shows the limits (`box/src/usage/`; `mac/helper/.../MenuBarItem.swift`).
- **Insights:** each box keeps a SQLite index and swaps pages of usage metadata with its peers (`box/src/usage/insights*.ts`).
- **Library:** each box keeps what was handed to you there, and the box you ask gathers the other computers' parts (`library/`).
- **Side panel:** files, terminals, Changes and the browser. Each is a box route that the app draws (`files.ts`, `git.ts`, `areas/*`; `gpui/kuvo`).

## Verdict

**Not yet everywhere: 8 of 13 concerns have exactly one mechanism.** The other five each have one
current mechanism, plus an older one left beside it by a change that has already landed:
- the main's old setup model (check 7, 9);
- the old update path, for computers not yet switched (8);
- the ownership lookup's old sources (ownership section);
- the split client launch (10);
- the folder layout steps (11).

None needs a new design. Each goes by deletion: most after 0.8.0's forced update (trim step 7),
the launch with #924 step 7. Sparkle beside the fleet update isn't counted as a second mechanism:
the app is a client, and it updates itself the way an installed app does.
