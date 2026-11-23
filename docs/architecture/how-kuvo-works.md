# How Kuvo works

Current on 22 November 2026. Paths are under `box/src/link/`
unless otherwise stated. The [November simplicity check](../archive/simplicity-check-1122.md)
records the earlier baseline. The remaining work is listed below.

Every computer runs the same **box**: the engine, the coding agents, their conversations, bots and
wake-ups. Each box works on its own, with no client connected. One of them is the **main
computer**, which alone decides what the whole setup shares. **Clients** (the desktop app, the menu
bar) show what a box says and ask it to change things. Computers reach each other through
**Kuvo's relay**, end to end encrypted, so the relay can't read what they send.

**Marked** rows have one current mechanism, plus an older one still beside it.

| Concern | The one mechanism | Where |
|---|---|---|
| Who's the main computer | A 60-second lease at Kuvo's relay. The main renews it every 10 s. When it lapses, another computer claims it: always-on computers at once, the rest a minute later. The new main signs a certificate for the next epoch, and the rest follow. A main without a fresh lease pauses setup-wide changes and says so, unless it's alone in its setup, where nothing could take over. A new setup starts in this model, epoch 1's certificate and checkpoint signed as it's made. **Marked:** setups that haven't converted keep the old model's branches (check 9). | `relay/src/leases.ts`; `setup/relay-lease.ts`, `lease.ts`, `takeover.ts`, `leadership.ts` |
| Shared data | The main saves setup-wide values alone: membership, devices, settings, personalities, labels. It publishes them as one signed copy. Members keep that copy, so whoever takes over starts from it. | `setup/shared.ts`; `box/src/shared/data-declarations.ts` |
| Updates | The main holds the newest signed release: from the feed, from Update Now, or from what a member offers. Every computer that's on prepares it beside the version it runs, then says it's staged or why it can't be. The main switches when every computer that's on is staged, or 5 minutes after the first one is. The others switch when they see it running. Staging leaves ordinary work available. The box supplies the release status and available actions. A switch is one rename of the `bin` link, and the keeper switches back if the new version doesn't answer. It keeps a compatible engine running and waits for engine turns that a restart or bridge handover would cut. An attempt that waits five minutes defers the switch. A computer that's off catches up when it's back. The desktop app updates itself through Sparkle, as an installed app does. **Marked:** the old trial, engine swap and quiet-moment wait still run for computers whose `bin` isn't a link yet (check 8). | `updates/fleet.ts`, `update.ts`, `keep.ts`; `box/src/keeper/kuvo-start` |
| Where logic lives | The box decides, and clients draw. One set of routes serves every client, and a feature list says what each box has. | `server/router.ts`, `areas/*`; `box/src/shared/features.ts`; `box/src/shared/routes`; `protocol/PROTOCOL.md` |
| Who owns a conversation, bot or group, and how others find it | The computer it lives on. A move hands it from one computer to another, and only those two take part. "Where does this live?" is one rule, in `ownerOf`: here, if this computer has it (its session list, bots and group chats); else where this computer's move journal sent it; else the one computer that says it has it. Each computer announces every record it has (`owns`, digests of the IDs). A request that reaches the wrong computer goes on to the owner, or the client is told where the record is. **Marked:** letters and the agents list still find a peer's conversation their own way (`peerOf`, with `moved` notices). | `ownership/resolve.ts` (`ownerOf`); `moves/move.ts`, `moves/records.ts`; `peers/agents.ts`; `ownership/messages.ts` |
| Running agents | The **engine** (opencode, unmodified, one folder per version) runs provider-model sessions. The resident starts it on demand and parks it after an idle period. The **agents bridge** runs Claude Code, Codex, Cursor, Grok and Antigravity. Each agent's process runs under a **holder** of its own. A bridge on its way out (an update, a release switch) lets go of every holder, at work or between turns, and never stops one; only the new bridge attaches, and takes each up where it is. The tools relay keeps its port and keys, so a held agent's tools keep working. The box keeps each session's queue. | `box/src/agents/` (`kuvo-agents.js`), `box/src/agents/hold/`; `engine/queue.ts` |
| How clients reach Kuvo | One process per computer (`kuvo-link.js serve --resident`) answers three ways: the local app over loopback HTTP (password in `run/serve.json`), SSH (`bin/kuvo-start route-to`, which runs `box route-to` on the keeper's engine and passes it to that process over a private socket), and the relay (Noise sessions). All three go through one dispatcher. **Marked:** the split launch (`run` plus `serve --kept`) and the plain `box route` over SSH are still kept (check 10), with an engine-picking fallback only for older installations without the keeper launcher (check 6). | `server/resident.ts`, `serve.ts`, `ssh-pipe.ts`; `relay/host.ts`; `main.ts`; `gpui/kuvo_box/src/transport.rs`, `local.rs`, `ssh.rs` |
| Getting between computers | Peer mail: messages encrypted with Noise between computers' keys, which the relay passes on without reading. A computer is added over SSH or with an installer code. Big things go as mail in parts: moves, and release packs when no download works. | `relay/src/relay.ts`; `peers/mail.ts`, `known.ts`, `reach.ts`; `add/` |
| Keeping Kuvo running | The keeper, `bin/kuvo-start`, on every computer. It starts and watches the resident box and the bridge. The resident schedules wake-ups and requests engine starts. Browser clients and tools start the browser when needed. On Linux an `@reboot` line starts it. On a Mac, Kuvo Helper starts it: a signed launch agent, which also holds Keychain access. | `box/src/keeper/kuvo-start`; `add/scripts.ts`; `helper.ts`; `mac/helper/Sources/KuvoHelperKit/BoxHelper.swift` |
| Stored data and formats | Everything is in `~/.kuvo-code`. Each kind of data is declared once, with its owner, backup and restore. Each file carries a format version. A change older builds can't read goes to a new path, so going back a version always works. **Marked:** a folder layout number, with 20 empty steps and 12 readiness checks, still sits beside this (check 11). | `box/src/shared/data-declarations.ts`, `format-version.ts`, `versioned-file.ts`, `folder-version.ts` |
| Backups and wipe | A daily backup, and one before a wipe, each kept 30 days, to a disk or another computer, with restore. **Unlink + wipe** is one accepted removal: access ends at once, and the computer erases only after its backup and planned moves are verified. | `backup/engine.ts`, `destination.ts`, `relay-store.ts`, `restore.ts`; `setup/removal*.ts`, `wipe.ts`, `before-wipe.ts`; `uninstall.ts` |
| Secrets | The vault. A Mac keeps values in the Keychain, through Kuvo Helper. Linux keeps them in encrypted files, whose key is wrapped by an unlock key that only your clients hold. Coding agents keep their own sign-ins where they always do. | `vault/store.ts`; `mac/helper/Sources/KuvoHelperKit/HostKeychain.swift` |
| The browser | Each box runs its own Chrome for Testing, kept by `kuvo-browser.js host`. Agents browse with its tools, and clients watch and use it live in the side panel. An idle tool connection lets Chrome close, and the next browser operation reopens it. | `box/src/browser/kuvo-browser.js`; `browser/install.ts`, `view.ts`; `gpui/kuvo_box/src/browser_view.rs` |

## Features on top

- **Bots:** a bot lives on one computer, its home, and moves the way a conversation does (`bots/`).
- **Wake-ups:** each box fires its own sessions' and bots' wake-ups, with no client connected (`box/src/keeper/kuvo-wake`).
- **Work Queue:** each item belongs to its queue's owner. Numbers come in blocks from the main, and the relay lease keeps the highest one given out (`work-queue/`, `work-queue/numbers.ts`).
- **Coding-agent accounts:** an account is one agent pointed at one folder on one box. The first is the agent's own sign-in, and the agent renews it and reports its usage itself (`agent-accounts.md`; `box/src/agents/accounts.ts`).
- **Usage:** each box reads its own accounts and tells its peers, so every box lists them all. The menu bar shows the limits (`box/src/usage/`; `mac/helper/.../MenuBarItem.swift`).
- **Insights:** each box keeps a SQLite index and swaps pages of usage metadata with its peers (`box/src/usage/insights*.ts`).
- **Library:** each box keeps what was handed to you there and mails the metadata to its peers. The nearest box filters and combines its kept copies (`library/`, `peers/lists.ts`).
- **Delivery:** the desktop shows sends immediately and retries the same ID. The box settles interrupted sends and the agents bridge keeps queued messages across restarts. The shared event line carries merged session, waiting and queue lists for clients that subscribe to them. See [Message delivery](message-delivery.md).
- **Side panel:** files, terminals, Changes and the browser. Each is a box route that the app draws (`files.ts`, `git.ts`, `areas/*`; `gpui/kuvo`).

## Where new code goes

CLAUDE.md's first two rules, in full.

- **A box decides, a client draws.** Sessions, agents, accounts, models, settings, files, git,
  processes, the network and what's allowed belong in `box/`. A client reads the box's state,
  changes it through a route, and draws the result. Keep clients thin enough that a new client
  (Windows, Linux) is almost trivial; between two working designs, pick the one a new client would
  rewrite less of.
- **The box states facts** for clients to present, documented where each route is defined
  (`box/src/shared/routes`) and, for the rules a route can't say, in `protocol/PROTOCOL.md`, so
  another client can use them.
- **Only this stays in a client:** drawing and layout, animation, window and selection state, the
  side panel's browser view, and what only an installed app can do (updating itself, login items,
  notifications). The keeper starts and watches the box and takes on no features.
- **Never add** a route only the app answers (`toApp` in `box/src/link/mac/relay.ts`), a copy of
  box logic in a client (or in `kuvo-browser.js` and the keeper side by side), or client logic
  "for now", "until the box can", or because the app is quicker to change.
- **Logic already in a client:** don't extend it. Move it to the box if that's small; otherwise
  make the smallest fix and list it in your report as left to move.
- **If something truly can't be on the box,** say why before building it; the user decides.
  Plans and briefs name where each new piece of logic lives.
- **Never only for this Mac**: not only in the app, only for the Mac's box, or only through the
  Mac's SSH connection. Where a box can't do something yet, say so in the UI and your report;
  never quietly make the Mac a hub.
- **Every operating system in one place.** `IS_MAC` means the box beside the app, not the
  system. Prefer code that's the same everywhere; where systems differ (file watching, processes,
  paths, the keeper, the shell, Keychain versus files), handle each in one place, test each, and
  say plainly what can't apply on one and why.

## Remaining work

Findings 1–7 in the archived check have landed on main: box-authored removal
choices, offline saves for a sole box, wider owner lookup, box-authored update
and compatibility display, the keeper's SSH launcher, and current-model setup
creation. Letters still have their own peer locator.

- **8–12 and 14:** retire the old update path, unconverted setup authority,
  split launch, folder layout steps, pending predecessor save formats and
  0.7 import/compatibility paths when their supported readers and rollback
  window allow it. Split launch has its own [step 7](one-client-process.md#remaining).
- **13:** the phone app is retired. APNs and legacy wire routes are still
  present. Decide separately when installed-client support can be removed.
- **15–19:** snapshot and lease polling, the separate epoch and relay term,
  cross-computer proofs, wipe generations and merge admission remain follow-ups.
  The archived check records their rationale and suggested changes.
- **20:** the protocol now starts with the current desktop and peer paths.
  The build generates the link bundle in `box/dist/`.

Sparkle updates the installed desktop client. The fleet update runs on the boxes.

See [Kuvo on Windows](windows.md) for the WSL, native box and desktop plan.
