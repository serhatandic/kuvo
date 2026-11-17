# Updates that never cut work: a design (handover)

Status: design only, 2026-11-16. Nothing here is built. Line numbers are at HEAD `5d9b7b8db`
(another agent is editing `update.ts` and `main.ts` right now: its stopgap, `updates/serve-current.ts`,
has `serve` confirm the trial and leave when its files change). [V] = I read it in the code.
[A] = an assumption, to check before building on it.

The user's direction: an update never kills the processes that run agents. Parts that serve
clients restart and clients reconnect. The part that owns agents hands them over live (herdr's
model). Restoring is only a fallback. A rollback never puts back data written after the update, and
every box (a Mac too) judges an update by the same rule, from real signals, not from a timer that
fires when no confirmation arrives.

## 1. What owns what today

| Process | Started by | Restarted by | What an update does to it today |
|---|---|---|---|
| Keeper `kuvo-start keep` | Linux: crontab `@reboot` → `kuvo-start` → `setsid sh kuvo-start keep` (kuvo-start:130-137). Mac: Kuvo Helper, in a session of its own (BoxHelper.swift `startKeeper`). | Mac: the helper (`ended`). Linux: `kuvo-start box` from the link and the launch (kuvo-launch:221). | It re-execs itself when `bin/.kuvo-boot` changes (kuvo-start:196) [V]. It holds no work. |
| Engine (opencode, unmodified, `opencode serve`) | `kuvo-start engine` → `kuvo-launch`, `setsid`, a new port and password on every start (kuvo-start:292-311, kuvo-launch:189) [V] | The keeper loop, with backoff, when `engine.json`'s pid is gone and `run/stopped` isn't there (kuvo-start:221-231) | **Always stopped**, through `kuvo-start stop` → `halt` (update.ts:542). This happens even when the engine version didn't change. Every turn running in it is cut. |
| Bridge `kuvo-agents.js` | `kuvo-launch` after the engine, inside `setsid sh -c 'while kill -0 $enginepid …'` (kuvo-launch:207). The loop restarts it 5 s after it exits, up to 5 times, and only while *that engine pid* lives. | That same loop. Nothing else. | Killed with its whole process group (`kill -- -$a`, kuvo-start:118). It also exits on its own whenever its engine's pid goes (agents/main.ts:565-571), and `stop()` → `closeAll()` finishes every turn as `"interrupted"` (sessions.ts:3133-3153). |
| Agents' processes | The bridge. Claude Code: Agent SDK `query()` with `pathToClaudeCodeExecutable` (agents/claude.ts:1478). The SDK spawns the CLI on **stdio pipes**, and permission and hook callbacks (`canUseTool`, `PreToolUse`) run inside the bridge. Codex: `codex app-server`, JSON-RPC over pipes (rpc.ts:44). Grok and Antigravity: ACP over the same `RpcProcess`. Cursor: a worker over RPC. `!` commands: `detached`, a group of their own (sessions.ts:248). Sign-in: a PTY (signin.ts:166), short-lived. | Nobody. The next turn spawns a fresh process, or resumes by ID (`resume: id`, claude.ts:1496). | They die with the bridge. They share its launch's process group (not `detached`), so `halt` signals them directly. Their stdin closes when the bridge exits [V spawn mode; A that each CLI exits on stdin EOF]. **They don't survive their parent.** |
| Link `run` | The keeper, only while `run/link.needed` (phones, a pairing, peers or a vault: keeper.ts:22; kuvo-start:263) | The keeper, within 15 s | Exits after an update *it* ran (update.ts:784). The `update` command SIGTERMs it (main.ts:1246). An update run by another process (`serve`) leaves it on old code. |
| `serve --kept` (Linux, clients) | The keeper while `run/serve.needed` (kuvo-start:269) | The keeper | Stopped only by the `update` CLI (`stopKeptServe`, main.ts:1248). **A nightly update run inside `run` leaves it serving old code.** The other agent's `serve-current` covers this. |
| `serve` (Mac, clients) | Kuvo Helper: `kuvo-start serve`, its own child, with secrets on stdin (BoxHelper.swift `startServe`) | The helper, when it ends | At HEAD it is never restarted by an update and never confirms one. The stopgap fixes both. |
| `kuvo-browser.js host` (Chromium over a pipe, clients on a unix socket) | The first `mcp` or `connect` client, in its own session | Any client that finds it gone, or older (`KEEPER` hash) | Untouched by `stop`. A newer client that finds an older `KEEPER` **stops it** (`ensure` → `stopKeeper`), which closes its pages. The `mcp` clients are the engine's MCP children, so they die with the engine. |
| Kuvo Helper (Mac) | launchd login agent `run.kuvo.code.machine` | launchd | Its bundle is swapped whole (`swapBundle`, update.ts). I found nothing that restarts it onto the new bundle [A: grep only], so it runs old code until the next login or Stop/Start. When it exits it ends `serve` and the keeper, but the engine and bridge go on, detached (`finish`, BoxHelper.swift:538-559) [V]. |
| Desktop app (gpui) | The user | Sparkle | A client. It offers its release (`kuvo/box/update {from}`) and reconnects. |

Two facts set the design. **Engine, bridge and agents are chained**: anything that restarts the
engine (an update, a crash, the bridge's `HangWatch` killing a hung engine) ends every coding agent's
turn, through main.ts:565 and kuvo-launch:201. And **the bridge already has a no-cut renewal**
(`renewal.ts`): it waits for a quiet moment, holds new turns, then leaves, never cutting a turn. That
is a drain, not a handoff. On a Mac's own box in 1.0 it applies too: the bridge is started by
`kuvo-launch` with `KUVO_AGENTS_REMOTE=1` (kuvo-launch:207). PROTOCOL.md's line "This Mac's own
bridge comes in its app" is stale.

## 2. What must change, per process

- **Client-facing (restart freely; clients reconnect): `serve`, `serve --kept`, link `run`, the
  keeper, Kuvo Helper.** None of them owns a turn. After the files are swapped, each one exits and
  is restarted onto the new code by its own parent (the stopgap's `serveCheck` "leave" for `serve`;
  the same for `run`; the keeper's `.kuvo-boot` re-exec; the helper exiting when its bundle's hash
  changes, a small Swift change in the "installed program" category). Requests in flight fail, and
  clients retry. The link's proxy to the bridge and engine retries connection-refused for a few
  seconds, so a restart looks like a pause, not an error.
- **Never killed by an update: the engine's turns and the agents' processes.** `update()` stops
  calling `kuvo-start stop`. Swapping files does not stop anything. Each owner of work takes the new
  version by its own rule (below).
- **The bridge** must stop dying with the engine. It follows `engine.json` (port, password, pid) and
  re-reads it on connection failure, instead of exiting at main.ts:565. The keeper keeps the bridge
  running (as it keeps `run` and `serve`), not a loop tied to one engine pid. Then a bridge version
  change is: **live handoff** (§3) where the agents allow it; otherwise the existing `Renewal` drain.
  Both never cut.
- **Agents' processes** move under a small long-lived **holder** (§3), so they outlive any bridge,
  including a crashed one.
- **The engine: being honest.** opencode is unmodified and its turns live in its own process (the
  model's stream, the tool loop, permission asks, its MCP children). A turn can't be handed to
  another process. Two opencode servers on one data dir are **not safe**:
  1. A newer engine runs its database migrations as it starts [A: opencode's drizzle migrations; the
     repo only shows `opencode.db` plus WAL, update.ts:282-292], so the old one runs on a schema it
     doesn't know.
  2. "This session is busy", pending permission asks and the event bus are per process, so a prompt
     reaching the new engine for a session the old one is running starts a second loop on that
     conversation.
  3. Both would work on the same snapshot repository and watched files.

  A copy of the data dir would fork the conversations. So the least-bad option is **quiesce and
  swap, never cut**:
  - Only releases that change `manifest.engine` touch the engine at all. Most don't.
  - The new engine waits in `engine/<v>/` (the keeper's `V`); `kuvo/info` `pieces.engine.behind`
    says so (this exists).
  - At the first quiet moment (no busy or retrying engine session, settled 30 s), with new prompts
    **held** at the box edge meanwhile, the old engine is stopped with nothing running, and the new
    one starts. Held prompts go to it in order.
  - Holding is bounded as `Renewal` bounds it: hold after an hour inside the night window, or after a
    day; let go after 30 minutes; try again the next night.
  - Retrying sessions count as busy (never cut). Update Now with the client's confirmation ("what
    your sessions are doing stops", `installerSteps.waiting`) stays the explicit override.
  - The visible outage is the migration time, during which prompts are queued, not refused.

## 3. The handoff protocol (the bridge and its agents)

**The holder (`kuvo-hold`, new, box code).**
- One small process per agent process, spawned by the bridge with `setsid` (its own session, so no
  group kill reaches it).
- It spawns the agent program as *its own* child on pipes, so the agent's parent never changes. No
  reparenting, and no fd passing. (Bun has no SCM_RIGHTS API that I know of [A]. `Bun.spawn` exposes
  no pipe fds to hand to a successor [A]. herdr passes PTY fds; we don't need to.)
- It numbers every stdout line (`seq`) and keeps a bounded journal: `run/agents/hold/<id>.journal`,
  a few MB, oldest dropped.
- It listens on `run/agents/hold/<id>.sock` (the folder is 0700). Watch macOS's 104-byte socket path
  limit, which home.ts:73 already handles for the browser.
- It writes stdin for exactly one controller at a time. A controller attaches with a generation
  number, and a higher generation fences off the older controller (only the newest may write).
- It exits when its agent exits, after a controller has read the exit.
- Record `run/agents/hold/<id>.json`: `{proto:"hold/1", holder:<HOLDER hash>, pid, child, program,
  provider, session, thread, sdk:<agent SDK version>, started, acked}`.
- It is protocol-agnostic and rarely changes. A new `HOLDER` build is only used for *new* agent
  processes, and old holders live until their agent ends. So holders never need replacing: the
  generations simply coexist.

**Bridge transports through the holder.**
- `RpcProcess` (Codex, ACP, Cursor's worker) gets a "holder" transport.
- Claude Code uses the SDK's `spawnClaudeCodeProcess` option, which returns a `SpawnedProcess` whose
  stdin and stdout are the holder socket (present in the installed SDK 0.3.303, sdk.d.ts:2489 and
  9626) [V]. The SDK also has a `Transport` with sequence numbers and multi-client "worker"
  semantics (sdk.d.ts:9937, `bridge.d.ts`); it's alpha, so worth reading but not depending on.
- Holders are worth having even before any handoff: a bridge crash no longer kills agents.

**Handoff, old bridge → new bridge** (replacing `Renewal`'s "go when quiet" when every live process
can be adopted).
1. The files are swapped. The old bridge sees a newer build (`newerInPlace`, as today) and runs
   `kuvo-agents.js selftest` (as today), plus a new `adopt-check`. That check gets the list of live
   holders (`proto`, `sdk`, provider) and answers which ones the new build can adopt. **All or
   nothing:** if any can't be adopted, it falls back to today's drain for the whole bridge.
2. The old bridge stops taking requests (the link retries), flushes its store (`store.flush`), and
   writes `run/agents/handover.json`: `{from:<build>, generation, sessions:[{id, holders, turn:{turnID,
   messageID, open part}, asks:[ids], acked:{holder:seq}}]}`. It detaches from each holder without
   `closeAll` (a new `stop({handover:true})`: no abort, no `finish("interrupted")`, no lost-work
   note), closes its port, and exits 0.
3. The keeper starts the new bridge at once. The new bridge reads `handover.json` and attaches to
   each holder with `generation+1`. It replays the journal from `acked`, rebuilds each live turn into
   the same message in `messages.json`, re-shows pending asks by their ids, then deletes
   `handover.json`.
   - Codex and ACP: replies to request ids it didn't send are matched from `handover.json`'s
     in-flight list.
   - Claude: see the spike below.
4. Clients: the bridge's `api/event` feed drops, and clients reconnect and refetch the session.
   Nothing is lost: the store has everything up to the handoff, and the journal has everything
   after.
5. If the new bridge fails to come up within a minute, the keeper starts the *previous* bundle
   (kept in `updates/previous/`). It attaches the same way, because holders don't care which bridge
   attaches.

**`!` commands** get holders too. Until then they count as turns that block a handoff, as they block
`Renewal` today.

**Spike S1 (must come before Claude's live handoff).** Attach a *fresh* SDK `query()` through
`spawnClaudeCodeProcess` to a Claude Code CLI already mid-turn. Check four things:
- whether a second `initialize` control request is accepted;
- whether hook and `canUseTool` callback ids registered by the first `query()` still route;
- whether a replayed `can_use_tool` request can be answered;
- whether the stream simply continues.

If S1 fails, Claude Code sessions use the drain (the turn runs to its end under the old bridge; new
turns are held), and that is said plainly in `kuvo/info`'s `pieces.agents.waits`. **Spike S2:** the
same for `codex app-server`, where only request ids and notifications matter, so it should be the
easy case. A different agent-SDK or app-server version between the bridges counts as "not
adoptable" (the `sdk` field).

## 4. Generations and compatibility

- **Client ↔ box.**
  - `kuvo/info` `features` (`FEATURES`, only ever appended) and the client gates (`gate.rs`,
    KuvoClient `BoxGate`). The lead says the app now treats a box lacking non-essential features as
    "behind" and uses it as it is [A: not in HEAD's `gate.rs`].
  - New: `kuvo/info` states the update as facts, `update: {inPlace, running per piece, waits:
    "engine-quiet"|"agents-quiet"|"handover"|null}`. The `pieces` field exists for machines; give a
    Mac's own box `pieces` too, so a client draws "updates when sessions finish" instead of holding
    the box.
- **Link and serve ↔ engine.** The link speaks the unmodified engine's HTTP API
  (`engine/compatibility.ts`). Rule: a release's link and bridge must work with **its own engine and
  the previous release's engine (N-1)**, because during an engine drain the new link runs against
  the old engine. `box.json` gains `engines: [<v>, <v-1>]`. The release check runs the link and
  bridge `selftest` against both. `update()` refuses a release whose `engines` doesn't include the
  running engine.
- **Bridge ↔ holder.** `hold/1` is additive only. A bridge must speak every `proto` still listed
  under `run/agents/hold/`, because holders outlive bridges.
- **Bridge ↔ agents.** The agents' own protocols, pinned by the SDK version recorded per holder (as
  above).
- **New serve, old engine (during a drain): allowed** by the N-1 rule. **Old serve, new engine:**
  that only happens in a code rollback that keeps the new engine. A rollback is only allowed when
  the previous release lists the new engine in its `engines`, otherwise only the engine is kept new
  and the rollback is refused (§5).
- **Data generations.**
  - The engine's database: `box.json` records `engineData` (the newest migration the engine applies
    [A: readable from its migrations table]) and `downgradeSafe` (the release check opens a copy
    migrated by the new engine with the previous engine and lists and reads sessions).
  - The bridge store (agents/store.ts has no format field [V]) gains `format` in `session.json`.
    The link's state files get the same. A reader that sees a newer `format` than it knows refuses
    to *write* that record, so it never truncates fields it doesn't know.

## 5. Health and rollback: one rule for every box

- **Who judges.** The updater process itself: always a detached `kuvo-link.js update …` process,
  never a client-facing one (today `startUpdate` can run inside `serve` or `run`, whose restarts kill
  it). It stays alive until it has judged. If it dies (the machine goes down), the keeper starts
  `update --settle` on its next round, and the judgment resumes. The same code runs on a Mac and on
  Linux. No relay welcome, no `serve` and no `run` is needed.
- **Signals** (all from processes that took the new code; pieces still draining are "waiting", not
  unwell):
  1. A fresh `kuvo-link.js box route GET kuvo/info` from the new files answers, and its `features`
     include the old box's.
  2. Every client-facing process that runs (`serve.json`, `run/link.pid`) answers `kuvo/info` with
     the new `BRIDGE_VERSION`.
  3. The engine answers `api/session?limit=1` (whatever version runs).
  4. The bridge answers `api/info` and either says the new build, or says `renewal.waits` ∈
     {quiet, holding, handover} (healthy and waiting).
  5. No crash loop: the keeper's engine `fails` counter, the bridge's restarts, and `serve`'s
     restarts (the helper's backoff) don't rise during the window.
- **When.** After the swap, the box is judged well once all signals hold through **5 minutes of the
  judging process's own monotonic uptime** (a Mac asleep doesn't count down). It is judged unwell
  when a piece that took the new code fails a signal three looks in a row, or crash-loops. A slow
  engine migration is "starting", not failed, while its process lives and its log grows. This
  replaces both the keeper's 20-minute wall-clock rollback (kuvo-start:238-254) and the 120 s
  `cameUp` cutoff (update.ts:320, 571).
- **Rollback = code only.**
  - It puts `bin/` (and the helper bundle) back from `updates/previous/` and lets each piece take
    that code by the same no-cut rules (restart for client-facing processes; handoff or drain for
    the bridge; quiesce for the engine).
  - The engine goes back only if the previous release's `engines` includes the running engine's data
    generation and `downgradeSafe` is true. Otherwise the engine stays new, only the link and bridge
    go back, and `kuvo/update` `last` says so in plain words. If even that can't run (the previous
    link doesn't support the new engine), the rollback **refuses**: it stays on the new release and
    reports "unwell, couldn't go back".
  - Data is **never** put back.
- **Backup.** Before an engine change, the database copy (`keepData`) is still taken, into
  `updates/backups/<date>-<from>-<to>/`, never under `rollback/`, and never restored by code. It is
  kept until the next engine change has been well for 7 days, and documented as a manual last resort
  (kuvo-web troubleshooting; `uninstall.md` and `privacy.md` list the folder).

## 6. Audit: paths that lose or overwrite data, or cut work

Data:
1. **restore.sh puts the old database back** (update.ts:282-292, generated by `keepForRollback`). It
   deletes the live WAL and SHM files (`rm -f "$D/opencode.db-wal" "$D/opencode.db-shm"`, :289) and
   moves the pre-update copy over `opencode.db` (:290). **Everything the engine wrote since the
   update is lost**: conversations, sessions, messages.
2. **The keeper runs that restore after 20 minutes of wall clock** if nothing confirmed the trial
   (kuvo-start:241-249). The trial is confirmed by `confirmUpdate` (update.ts:649), which is called
   only by `run`'s 3-minute timer or relay welcome (main.ts:470, 323), or by the `update` command when
   `run` isn't needed (main.ts:1250).
   - The un-confirmed case: Update Now goes through the `kuvo/update` route, so `startUpdate` runs
     inside `serve`. That `serve` exits (update.ts:784). The `run` process, if one runs, is never
     restarted and its one 3-minute timer fired long ago. If none runs, nothing exists to confirm.
     Either way: after 20 minutes, restore, data lost.
   - Then the release rests a day (`offerRests`) and is taken again: **a loop that loses up to 20
     minutes of conversations each time** on a Mac. This most likely matches "the 10th time"
     [A: read in code, not reproduced on the user's Mac].
   - A Mac asleep for 20 minutes right after a good update also rolls back as it wakes (the timer is
     the file's mtime against `date`).
3. **A second update while a trial is open deletes the first rollback** (`fs.rmSync(ROLLBACK)`,
   update.ts:252). `update()` doesn't check `updateOnTrial()`; only `offerDue` and `offerHeld` do. The
   night path and GitHub's Update Now don't. The known-good code and its database backup are lost,
   and the "previous" kept becomes the unconfirmed version.
4. **A half-done restore is started anyway.** After 6 failed tries the keeper drops the trial and
   starts whatever is there (kuvo-start:250-251). restore.sh exits on the first failing `cp` (`||
   exit 1`), so `bin/` can be a mix of two releases.
5. The bridge store and the link's state are never rolled back (good), but nothing checks that older
   code reads what newer code wrote (store.ts has no format) [A: unknown fields are kept on rewrite
   because records are parsed and saved whole; check per release].
6. `confirmUpdate` deletes every other engine version (update.ts:656). That is harmless today. Once
   engines drain, it must keep the one still running and the previous release's engine (the code
   rollback needs it).

Work cut:

7. **Every update stops the engine, bridge and agents** (update.ts:542 → kuvo-start:116-128,
   `kill -- -$a` on the bridge's group, then the engine with `kill -9` after 10 s), even with
   `otherEngine` false (:533).
8. **Every rollback does the same, at a moment the keeper chooses**, with no busy check (restore.sh
   :259, `kuvo-start stop`).
9. **Any engine restart ends every coding agent's turn.** The bridge exits when its engine's pid
   goes (agents/main.ts:565-571); `closeAll` marks turns interrupted (sessions.ts:3133); every engine
   launch kills a running bridge first (kuvo-launch:201).
10. **The updater runs inside client-facing processes** (`startUpdate` in `serve` or `run`,
    update.ts:774). If it is killed between `stop` and `start`, `run/stopped` stays, and the engine
    stays down until the keeper's 20-minute restore (which then loses nothing, but cuts 20 minutes of
    service).
11. `cameUp` gives the engine 120 s (update.ts:320, 571). A slow migration on a big database is
    judged failed, killed mid-migration by restore.sh's stop, and rolled back.
12. The browser keeper is replaced by stopping it (kuvo-browser.js `ensure` → `stopKeeper`), which
    closes its pages and open asks.
13. `boxBusy` waits on sessions that never end (retrying, subagent turns whose end was lost), so
    night updates may never run. This isn't data loss, but it is why updates don't arrive.
14. Seen in passing: the keeper still runs `pkill -f "watchdog …"` (kuvo-start:187, 279, COMPAT
    0.7). That breaks the "never kill by pattern" rule, and should go with that COMPAT.

Checked, no loss found: `clearUpdateStaging` (update.ts:642) keeps `rollback`, `installed.json`,
`rehome` and `offered` (I didn't read `rehome`'s own code); `checkDrift` (:705) only blocks updates;
`kuvo-install update` only stages files under `updates/install` and execs the *new* release's
`update --installer` (kuvo-install:231-232), so it inherits items 1-11.

## 7. Fallbacks, and the order to build it in

Each step can ship on its own. Tests are box tests (`scripts/test-box.sh box|bridge <filter>`) with
real components and only the engine's HTTP stubbed where a turn must be held open. Each runs on
macOS and on Linux (a devbox folder of its own), with injected clocks, no sleeps.

1. **Stop losing data (urgent, small).** Drop the `withData` lines from `keepForRollback`. Move
   `keepData` to `updates/backups/`. If the engine changed and its downgrade isn't known safe, the
   rollback keeps the engine and puts back only `bin/`. Test: an update with an engine change, a
   conversation written after it, a forced rollback; the conversation is still there and the engine
   runs.
2. **No update during an open trial; the updater always runs detached.** `update()` refuses while
   `updateOnTrial()`. `startUpdate` and the route spawn `update --now|--offered` as `update-now`
   does. Test: a second Update Now during a trial is refused, and the rollback folder is unchanged;
   killing `serve` mid-update doesn't kill the update.
3. **One health rule** (§5), replacing the keeper's 20-minute timer, `cameUp`'s 120 s, and every
   confirm path (`machineWelcomed`, `run`'s 3-minute timer, the stopgap's `serve` confirm). Tests:
   a well update with no `run` and no relay gets confirmed on both systems; a box whose new `serve`
   crash-loops gets a code-only rollback; a judging process restarted midway resumes; time asleep
   (clock injected) doesn't count.
4. **Updates stop nothing.** `update()` swaps files and never calls `kuvo-start stop`. `run`,
   `serve` and `serve --kept` leave on a file change (generalising the stopgap). The helper exits
   when its bundle's hash changes. The bridge takes the new build through today's `Renewal`. Tests: an
   update during a running coding-agent turn and a running engine turn; both finish; the new code is
   live afterwards in `serve`, `run` and the bridge.
5. **Unchain the bridge from the engine.** The keeper keeps the bridge running; the bridge follows
   `engine.json`; `kuvo-launch` no longer kills it. Test: restart the engine during a coding-agent
   turn, and the turn completes.
6. **Engine quiesce and swap** for engine bumps (§2): held prompts shown as queued, bounded holding,
   N-1 engines in `box.json`, the release check running `selftest` against both engines and the
   downgrade check. Test: a prompt sent during the swap arrives once, in order, on the new engine.
7. **Spikes S1 (Claude SDK attach) and S2 (Codex app-server)**, notes only, before step 9.
8. **Holders** for agents' processes and `!` commands. Renewal still drains. Test: SIGKILL the bridge
   mid-turn; the agent process lives; a new bridge reattaches and the turn ends normally.
9. **Live handoff** (§3): Codex and ACP first, Claude if S1 holds. The drain remains the fallback,
   all-or-nothing per handoff. Test: a handoff mid-turn with a pending permission ask; the ask shows
   again and is answered once, and the turn's message is one message.
10. **Data formats** (`format` fields; writers refuse newer records), and the browser keeper
    replaced only when no client is attached and no ask is open.

**Fallbacks, in order of preference:** live handoff, then the drain (`Renewal`, or quiesce for the
engine: waits, never cuts), then Update Now with the user's confirmation (cuts, said first), then a
code-only rollback, then a refusing rollback ("stays on X: unwell"), then a manual restore from
`updates/backups/` (docs only).

**For the user (UI drafts needed before building, CLAUDE.md "Drafts first"):** how a queued prompt
looks during an engine swap; the words for "updates when sessions finish" and for a refused
rollback; whether Update Now keeps its "stops what's running" confirm once steps 4-9 make it rare.
**Docs:** kuvo-web machines and troubleshooting (updates no longer restart sessions; the backups
folder), `uninstall.md` and `privacy.md` (`updates/backups`, `run/agents/hold`), and PROTOCOL.md
"Updates" and "A new version while it works" at each step.

## Status, 2026-11-17: steps 3, 4 and 5 built (branch kuvo/updates-p1-one-health-rule-updates-stop)

- **Step 3:** `box/src/link/updates/health.ts` (`settleUpdate`, `lookAtPieces`). The judge is the
  updater, or `update --settle`, which the keeper starts and which resumes `trial.judged`. Rollback
  is `kuvo-start rollback`: a lock, three tries, then it gives up and starts the box as it is.
- **Step 4:** `update()` stops nothing unless the engine changes (then `engine-stop` only).
  `updates/code-in-place.ts`: run / serve / serve --kept leave on new code. The helper exits 75 on
  a new bundle; a clean exit means a stop on purpose.
- **Step 5:** `kuvo-start bridge` (kuvo-launch `KUVO_BRIDGE=1`). The bridge follows
  `run/engine.json`. A kept bridge makes way for the keeper's new engine (`engineBehind`), and
  `confirmUpdate` keeps the engine the bridge runs on until then.
- **Still differs from §5:**
  - Signal 2 checks that new processes answer, not their `BRIDGE_VERSION`; old ones are ended by
    the keeper (`COMPAT(beta.30 old-code)`).
  - An engine change still restarts the engine, with no hold (step 6).
  - The engine folder kept for a bridge is pruned only at the next confirm.
  - BusyBox `ps` has no `-p`, so old processes aren't ended there.

## Status, 2026-11-17: step 6 built (branch kuvo/updates-p2-an-engine-change-swaps-the)

- **The swapper:** `box/src/link/updates/engine-swap.ts` (`kuvo-link.js engine-swap`), started by the
  keeper while an older engine than its `V` runs and no update runs or is on trial. State in
  `run/engine-swap.json` (`waiting` → `holding` → `swapping` → done, or `back`). Quiet 30 s, else an
  hour on at night or a day on; Update Now writes `run/engine-swap.now` (holds at once, still never
  cuts). Hold bounded at 30 min (let go, held prompts to the old engine, again an hour on), and
  `swapHolds` ends any hold whose swapper is gone or that's over an hour old.
- **The hold:** `shared/engine-swap.ts` `swapHolds`; `link/engine/queue.ts` `swapHoldsFor(target)`
  (engine only, never the bridge) → `keepWaiting(..., target)` keeps the message on the box. Sites:
  send route, queue/add, sendInto (letters, notes), wake-ups, `start_session`'s first prompt, goals'
  /goal, the raw forward (`api/session/<id>/prompt` → `{waits:true}`; command → 503 with the line),
  `drain`/Send Now/Steer (no start; Send Now/Steer put it first), the bridge's group seats (wait in
  `untilSwapLets`). Before the stop, `moveInboxToBox` moves every engine inbox (latest 100 sessions,
  a week) onto the box. After: `releaseWaiting` sends each non-paused session's waiting items, once,
  in order (first as steer).
- **Engines:** keeper `engine-halt` (no `stopped` mark), no engine start while `run/.engine-swapping`
  names a live swapper, `pick` each round honouring `run/engine-back` for a day. `update()`:
  `engineApart` (box.json `engines` lists the running engine) → code only, link selftest on both
  engines, `installed.json` keeps `engine/engines/downgradeSafe`; confirm keeps those engines.
  Release: `scripts/engine-check.mjs` (release.yml step) → `ENGINE_CHECK` → box.json `engines`,
  `downgradeSafe`; `engine-pin.json` gains `previous` at the next engine bump (docs/upstreams.md).
- **Words:** `kuvo/update` `engineChange {stage, from, to, line}` and `engineLast`; queue answers'
  `line`; this computer's update line (`keepingSelfUp`). gpui draws the queue's `line`.
- **Test:** `box/bridge-tests/engine-swap.test.js` (swap with held prompts and a running coding
  agent turn; a new engine that fails goes back with held prompts; a bounded hold lets go).
- **After two reviews:** the keeper never starts a newer engine by itself (`run/engine-ran`, noted
  each round and before the swapper's start; `W`/`B` in `pick`); the busy rule is `models.ts`'s
  (shells, ptys, asks), an open terminal or ask blocks the timed hold and says so; the bridge's own
  engine turns wait before they begin (`EngineLink.beforeTurn`), and `kuvo-wake` hands wake-ups to
  the box during a hold.
- **Not done:** the phone's queue doesn't draw `line` yet; a group chat seat's or a review's turn on
  the engine waits inside its request, after it began listening (the bridge's own engine turns wait
  before); tests don't kill the swapper midway, nor cover the raw forward and seats, and they run in
  order on one box; commands are refused (503 with the line) rather than queued during a hold;
  child sessions' inboxes aren't moved; the "together" path (a release whose
  `engines` don't list the running engine) still stops and starts the engine with no hold;
  `untilFree` (COMPAT installer) still waits for quiet for any engine change.

## Spikes S1, S2 and ACP: results (step 7, 2026-11-17)

Question: can a NEW controller re-attach to an agent's turn that an OLD (killed) controller started,
without the turn noticing? All runs: the agents' real binaries, a stub model API on 127.0.0.1, a
temporary HOME and the agent's own home under it, a made-up API key. No sign-in was read or needed.
Each controller is its own process and is killed with SIGKILL (Claude) or has its socket closed
(Codex, Grok) mid-turn. The scripts are in `.claude/notes/spikes/` (`hold-spike.mjs` is the
throwaway holder: the agent as its child on pipes, a unix socket, the newest controller fences the
older one, a journal of every line). Run them with `bun <script> <args>`, as each one's header says.

**S1, Claude Code 2.1.325 with Agent SDK 0.3.303: YES, and the CLI is built for it.**
- Setup (`s1-claude.mjs`): controller A is a `query()` whose `spawnClaudeCodeProcess` starts the
  holder (detached) and returns the holder's socket as stdin/stdout. B and C are fresh `query()`s
  whose `spawnClaudeCodeProcess` only connects. The stub makes the model run `echo one`, then
  `echo two`, then stream ten words 400 ms apart.
- A killed while its `canUseTool` was open: holder and CLI live on. B's `query()` sends a **second
  `initialize`, which succeeds**, and the CLI puts the open ask in the reply's
  `pending_permission_requests` (documented in sdk.d.ts: "a client joining an already-initialized
  session learns about in-flight prompts", from Claude Code 2.1.268). B's `canUseTool` ran, the
  command ran once (`one.txt`; the model saw exactly one tool result).
- **The holder must not replay control requests for Claude.** With replay on, B's `canUseTool` ran
  twice for one request_id (once from the replay, once from `pending_permission_requests`); the
  second answer was ignored by the CLI. The SDK says receivers must tolerate a request_id arriving
  both ways and "render it once": the bridge dedupes asks by request_id.
- **A hook callback open at the kill is cancelled by the CLI, not executed**: on B's `initialize`
  the CLI sends `control_cancel_request` for it and gives the model the tool result
  `PreToolUse:Bash hook error: The SDK host reconnected before its PreToolUse hook answered, so this
  tool call was not executed. No one denied it; retry the same tool call.` (`is_error`). The turn
  goes on (a real model retries); nothing runs that the bridge didn't check. Replaying the hook
  callback doesn't help: the CLI cancels it before reading the answer.
- Hooks after the attach route to the new controller: its `initialize` registers its own
  `hook_0` (ids are `hook_<n>` in registration order, per `query()`), and B's `PreToolUse` ran for
  `echo two`.
- The stream simply continues: B saw `STREAM1-2`, was killed, C attached mid-message and saw
  `STREAM3-10`, the whole `assistant` message and `result` (`success`, `num_turns: 3`). What came
  while no one was attached is in the holder's journal; the final `assistant` message carries the
  whole text anyway.
- Limits: the new `query()`'s command-line options don't apply (the process is already running:
  model, permission mode, `resume`, plugins, settings, env stay the first bridge's). Only what
  `initialize` carries is renewed (hooks, system prompt fields, agents, SDK MCP servers). So an
  adopting bridge takes the recorded `opened` key from the holder's record, not its own options.
- The CLI exits on stdin EOF (the holder closing its stdin ended it, exit 0): §1's [A] holds for
  Claude Code.

**S2, Codex 0.169.0 `app-server`: YES over stdio under a holder; NO through Codex's own socket.**
- Holder (`MODE=holder s2-codex.mjs`): A starts a thread (`approvalPolicy: untrusted`) and a
  turn; the model calls `exec_command`; Codex sends `item/commandExecution/requestApproval` (id 0).
  A's socket closes. Codex sees nothing (its pipes are the holder's). B attaches: the holder replays
  the open request, B answers `{decision: "accept"}`, the command runs once (`one.txt`), the
  stream goes on; C attaches mid-stream and sees `STREAM3-10`, `item/completed` with the whole
  text, and `turn/completed` (`completed`).
- **A second `initialize` is refused** (`-32600 Already initialized`), harmlessly: an adopting
  bridge skips `initialize`/`initialized`.
- Ids: Codex's own request ids are its own (0, 1, …): answer them by id. Answers to requests the
  old bridge sent go to whoever is attached: the new bridge matches them from the handover's
  in-flight list, and starts its own ids in a range of its generation (RPC `firstID`) so they never
  collide with an old bridge's.
- Codex exits on stdin EOF (exit 0 when the holder closed it).
- Codex's own multi-client transport (`app-server --listen unix://PATH`, `app-server proxy`):
  **not usable.** It makes its real socket and a lock under `/private/tmp/codex-daemon-<uid>/` (a
  symlink at PATH), against the footprint rule, and in 0.169.0 neither a line-JSON client nor
  `app-server proxy --sock` got an answer to `initialize` within 90 s. Not pursued further.

**ACP, Grok 1.0.77 (`grok agent stdio`): YES under a holder.** (`s3-acp.mjs`; Grok takes
`GROK_XAI_API_BASE_URL`, `XAI_API_KEY` and an OpenAI-style `chat/completions` stub.)
- A's turn asks `session/request_permission` (id 0) for `run_terminal_command`; A's socket closes;
  B attaches, **sends `initialize` again, which Grok accepts**, answers the replayed ask with
  `allow-once`; the command runs once; C attaches mid-stream, sees `STREAM3-10` and then the answer
  to A's `session/prompt` (id 4, `stopReason: end_turn`), a request C never sent: as for Codex,
  matched from the in-flight list.
- **Antigravity: not run** (same ACP code path in the bridge, but I found no stub route for its
  model in the time). Counted as not adoptable until a spike shows it. **Cursor**: its worker is
  our own (`cursor-worker.ts`), so holding it is ours to make safe; not adopted in step 8.

**What this means for steps 8-9.**
- The holder is protocol-agnostic for transport, but replay is per kind: `jsonrpc` (Codex, ACP)
  replays requests the agent sent that no controller answered; `claude` replays none (the CLI
  re-sends asks on `initialize` and cancels hook callbacks itself).
- Every adopter must tolerate an ask seen twice (same id) and answers to ids it didn't send.
- Adoption needs, per holder: the SDK/agent version it started with (the `sdk` field), the
  provider, the agent's own ids (Claude session, Codex thread, ACP session), and what the bridge
  had open (`opened`, in-flight request ids, the turn's message): S1/S2 show the agent side holds;
  the bridge side is what steps 8-9 build.

## Status, 2026-11-17: step 8, holders built (8a), not yet what agents start with (8b)

**Built (8a), tested on macOS and Linux:**
- `box/src/agents/hold/program.js`: the holder (`hold/1`), plain JavaScript on Node's modules. The
  bridge imports it as text, writes it to `run/agents/hold/kuvo-hold-<HOLDER>.js` and runs it on its
  own Bun (about 13 MB per holder, against 35 MB for the bridge's bundle). It runs the agent as its
  child on pipes, in a session of its own, numbers each stdout line and keeps the latest 4 MB in
  memory. It listens on `<id>.sock` in a 0700 folder and records `<id>.json` (0600, with a
  per-holder key). It takes one bridge at a time, by key and generation: a higher generation fences
  off the older bridge, and a stale or keyless one is refused. On attach it replays from the last
  `ack`.
  - `jsonrpc` holders also send again what the agent asked that no bridge answered.
  - `claude` holders never replay control requests, from the journal or otherwise: Claude Code
    re-sends its asks itself (S1), so a replayed one is asked twice. The test caught this.
  - The holder ends once its agent ended and a bridge heard it (or after 15 minutes with none). Its
    files go with it. An agent with no bridge attached for 15 minutes is stopped as a bridge stops
    one (stdin closed, then SIGTERM, then SIGKILL), so nothing held runs for good.
  - Its key and spec come on its stdin, never in an environment or on a command line. Open asks
    keep their own line (at most 1,000), and go when answered or settled (`serverRequest/resolved`,
    `$/cancelRequest`). The agent's end is said after its last output. A bridge that stops reading
    is dropped, to attach again from its ack.
- `box/src/agents/hold/client.ts`: `hold()`, `attach()` (generation+1), `heldIn()`, `holdFolder()`
  (`run/agents/hold`; on macOS, past the 104-byte socket limit, Kuvo Code's short folder in `/tmp`
  as `SSH_AGENTS` does; elsewhere none: unheld). It also gives `Held` (lines, stderr, exit, write,
  end, kill, `note` for record metadata, `detach`) and `heldProcess()` (a `SpawnedProcess` for
  Claude's SDK `spawnClaudeCodeProcess`).
- `RPC` takes a `Held` in place of a command (Codex, ACP, Cursor's worker): the same lines once
  `start()` is called (so the handlers are set before a replayed ask arrives), and `firstID` for a
  generation's own ids.
- On macOS, the `/tmp` fallback folder is used only if it is this user's alone (as `safeFolder`).
  The program file is checked against the bridge's own copy before it runs.
- Tests (`box/test/agents/hold.test.ts`, `hold-claude.test.ts`): the first bridge, a process of its
  own, is SIGKILLed with an approval open. The agent lives; a new bridge attaches (generation 2), is
  asked again once, answers, and the turn ends (`turn/completed`, or the SDK's `result`), the command
  run once. Also covered: fencing, a stale or wrong key, an agent that ended before anyone attached,
  and what each kind sends again.

**8b, built 2026-11-17: Codex, Claude Code and Grok run under holders, and a crash doesn't end their turns.**
- Which agents: `HELD_KINDS` in `box/src/agents/hold/adopt.ts` (codex, claude, grok). Antigravity
  and Cursor are left out, in code and here: no spike showed a new bridge can take theirs over, so
  a crash ends their turns as before and Renewal waits for them. The held list is the one switch:
  empty, everything runs as before step 8, and a starting bridge stops any held agent it finds.
- Starting: `codex.ts` `start()`, `claude.ts` `holdSpawn()` (also through the sandbox's launcher),
  and `acp.ts` `open()` (Grok's flavor has `held`) start the agent under `hold()`. If the holder
  can't start, the agent runs as the bridge's own process. Each turn notes in the holder's record
  where its lines begin (`from`), and Codex's turn id or ACP's prompt request id
  (`noteTurn`); the note goes when the turn ends.
- Adopting (`adoptHeld`, run in `main.ts` before sessions load):
  - every live holder of a held kind with turns noted is attached at generation+1, from its
    oldest turn's `from`;
  - any other holder (idle, an unheld kind, a gap in its journal, a failed attach) is stopped by
    its pid.
  `Sessions` takes up a turn only if its session's last turn is still open, on that agent (not
  Auto), and began with a message (not a summary or a subagent's task). For those, `repair` drops
  the stored steps of the open turn and `resumeHeld` reruns it from the journal, before the bridge
  listens. A turn not taken up is let go (`HeldGate.drop`): Codex is told `turn/interrupt` for it,
  and an agent with nothing taken up is stopped.
- Per agent:
  - **Codex**: the adopted app-server becomes its key's server, without `initialize`. Its lines wait
    until every turn taken up in it is ready (the gate: the shared server runs several sessions).
  - **Claude Code**: a new SDK `query()` joins the held process; its open ask returns through
    `pending_permission_requests`. Asks are de-duplicated by tool-use id, Codex's by request id. The
    sandbox launcher isn't made again, since `fenceAgent` would clear what the live process runs with.
  - **Grok**: the answer to the crashed bridge's `session/prompt` ends the turn (`RPC.onStray`).
- Stopping: the keeper's `halt` (stop, restart, pause, off, so also uninstall and Stop Background
  Work) runs `halt_held`. It stops each holder by the pid its record names, only if that pid runs
  `kuvo-hold-`, and waits up to 20 s. The holder closes the agent's stdin, sends SIGTERM after 5 s and
  SIGKILL after 15 s. `uninstall.ts` lists and stops holders too. A bridge that stops on purpose
  (Renewal, SIGTERM) stops its held agents through `RPC.kill` / `heldProcess.kill`, which signal the
  holder's own process.
- Review fixes (2026-11-17):
  - a bridge is dropped only once output already queued for it passes the journal size, so one long
    line no longer cuts it off;
  - Codex asks are keyed by app-server and id;
  - records count only when their pid runs that holder (`isHolder`, which uses the command line),
    and stale ones are removed, never signalled;
  - an agent runs in its own process group, and a stop signals the group, so what it started goes
    too;
  - a bridge stopping on purpose stops every agent it holds (`stopAllHeld`);
  - starting and attaching time out after 10 s;
  - a resumed turn's stored steps are dropped only once its agent takes it (otherwise its tools are
    closed as after a crash);
  - Codex ignores replayed lines from before a thread's own turn, and interrupts a dropped thread's
    turn once its id shows;
  - `Held.stop()` does nothing once its holder is gone;
  - old holder programs are removed;
  - the recorded `opened` leaves out Kuvo Code's tools' key.
- Second review's fixes (2026-11-17):
  - **Never dropped for size.** A bridge whose socket is full gets backpressure: the holder stops
    reading the agent's stdout until it drains, and the agent waits as on any pipe. Only a bridge
    that reads nothing for a minute is let go. A bridge that loses its holder without hearing the
    agent's end stops it by pid (`isHolder`), so nothing runs unwatched.
  - **The journal is on disk:** `<id>.journal` beside the record (0600, in the 0700 folder),
    64 MB at most, pruned to its newer half, removed as the holder goes (and by `heldIn` with a
    crash's leftovers). Why disk and not a bigger in-memory size: a long Claude Code turn
    (streamed words, partial messages, tools' output) runs to tens of MB, and memory would cost that
    per holder, held or not. (Not measured on real turns: estimated from S1's lines, about 300 bytes
    per streamed delta, plus tool results.)
  - **A take-up that fails keeps what was shown:** the turn's stored steps go only once the agent's
    first line comes again (`Held.firstLine`). Before that, a failure (an exception from the agent,
    the attach lost) closes their tools as a crash does, and the agent is stopped.
  - **Claude Code's version:** the record gets `sdk` as the holder starts, and `version` from
    `system/init` (`claude_code_version`). Adopted only from 2.1.268 (`CLAUDE_REJOINS`) and with the
    same SDK; otherwise stopped, and its turn ends as a crash ends it.
  - **Two bridges at once:** a "generation" refusal leaves the holder to the bridge that took it.
  - **PID checks:** the keeper's `held_pid` and uninstall's `holders()` match the holder's program
    and its own id.
  - **Re-review NITs, fixed:**
    - A take-up that throws lets go of its own turn only (`HeldGate.release`). A shared Codex
      app-server stops only once no turn taken up in it is left.
    - On a shared app-server, a session's kept steps go at its own thread's first line
      (`HeldTurn.began`), not at any thread's.
    - A bridge fenced off while its socket was full no longer holds back the agent's output
      (`unwait`). The test for it passes on the old holder too, because the newer bridge's own drain
      resumed reading anyway, so that stall was mostly theoretical.
  - **Not handled:** a bridge that crashes while another runs isn't adopted by the running one; its
    held agents stop after 15 minutes. There's one bridge at a time (the keeper's, and Renewal's
    makes way before the next starts), so it takes two bridges up at once, which the generation
    fence already sorts out.
- Tests:
  - `box/bridge-tests/agents-held.test.js`: the real bridge, holders and keeper, with stand-ins that
    speak Codex's, Claude Code's (as S1 recorded it) and ACP's protocols. For each agent, the bridge
    is SIGKILLed with a permission ask open. The agent lives; the next bridge asks once; the answer
    goes once; the turn ends `succeeded` as one turn, with what was said before and after the crash.
    Then `kuvo-start stop` leaves no agent or holder running.
  - `box/test/agents/hold*.test.ts`: the holder protocol, and an unattached agent stopped (the
    15-minute wait is the holder's `orphan` setting, 100 ms in the test, not an injected clock: the
    holder is a separate program on real timers).
  - `agents-renewal.test.js`: Renewal still drains, now with Codex held.
  - `claude.test.ts` checks the SDK's own and the launcher's spawning, so it runs unheld
    (`holding.on = false`).

**Not built, and why:**
- **`!` commands stay unheld.** They already run in a process group of their own (`detached`), so a
  bridge crash doesn't signal them. But their output pipe goes with the bridge, and taking one up
  means rebuilding its shell message and output in a new bridge: a different path from agents'
  turns. Until then a crash marks a running `!` command failed (`repair`), as before, and Renewal
  waits for them.
- Turns not taken up after a crash end as before (interrupted): Auto sessions, summaries, turns
  with a message you sent partway (steered: the replay would place it wrongly), and
  subagents' sessions. A Codex or Claude subagent's own session is repaired as interrupted while
  its parent's turn goes on.
- A held agent's stderr after it exits is heard for 2 s (the holder's wait), not `heardWithin`.
- Claude Code that crashed before its first `system/init` (so no version recorded) isn't adopted.
- Answers to requests the crashed bridge sent are ignored, except ACP's prompt. A Codex turn that
  crashed before Codex answered `turn/start` with an error would wait (no `turn/started` ever comes).
- Not covered by a test: the held path with the box's sandbox on (the launcher under a holder,
  fenced Codex). Also unchecked: whether Kuvo Code's tools reach a resumed turn's agent in the
  sandbox after the bridge restarts (the relay's port can change; Codex reloads its thread at its
  next turn, and Claude Code restarts at its next turn because its tools' key isn't recorded).
- Two bridges at once: a bridge doesn't adopt while `run/bridge.json` names another live
  `kuvo-agents` process, the one with the port.
- Step 9 (live handoff instead of Renewal's drain) can now build on this. Adoption is crash
  recovery; a handoff adds `handover.json` and detaching without stopping.
