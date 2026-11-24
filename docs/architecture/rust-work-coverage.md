# Row 5.1 coverage: bots, wake-ups, notes, attention, approvals

Coverage of the TypeScript tests for [row 5.1](rust-box.md#6-phase-plan) in `kuvo_work`. The row is
split into parts, each mergeable once it has its proof:

- **5.1a** — wake-ups and watches (routes and the resident's own round) and the bot list
  (`kuvo/bots` and its events). This document's first part.
- **5.1b** — the rest of this box's own bot routes: templates, make, edit, delete, arrived,
  retell, seen, the gone key routes, and each bot told what it is as the engine starts.
- **5.1c** — work (activity, background, oversight, asks, settings, the work watcher that tells a
  conversation of its sessions), approval and Auto's decisions, attention, notices.
- **5.1d** — edits held for bots on your other computers, the bots elsewhere in `kuvo/bots`, and
  bots' old keys of their own taken away (with what a bot is told while one isn't connected).

Proof for each part: the area's route scenarios pass against both boxes through the binary, a
side-by-side run over the same home, Rust tests for what the scenarios can't reach, and the platform
runs.

## 5.1a — wake-ups and watches, the bot list

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_work::wakes::folder` | `areas/wakes.ts` (`wakeOf`, `repeatText`, `readWakes`, `wakeFolder`, `listWakes`, `allWakes`) |
| `kuvo_work::wakes::clock` | `kuvo-wake`'s `repeats`, `kept`, `next`, `daytime`, `zone` |
| `kuvo_work::wakes::fire` | `kuvo-wake fire` (wake-ups, retries, `settle`, `checks`, `delivered`) and `sendWakes`, `nextWake`, `wakeDemand` |
| `kuvo_work::wakes::check` | `kuvo-wake run`, `limited`, the curl/wget/gh notes, `cksum` |
| `kuvo_work::wakes::inbox` | `kuvo-wake take` |
| `kuvo_work::wakes::routes` | `answerWakes`, `wakesRoute`, `wakeEventsRoute`, `sessionWakesRoute`, `kuvo-wake add` and `watch` |
| `kuvo_engine::WakeEngine` | `kuvo-wake deliver`, `waiting`, `held`, `hold`, `archivedAt`; `waitForTools`, `keepWaitingLater`, `stopIfWaiting` |
| `kuvo_work::bots::{here, job, list, now}` | `bots/here.ts`, `bots/jobs.ts`, `bots/list.ts`, `bots/deletes.ts`, `bots/now.ts`, `lists/rule.ts` (`botsNow`, `botState`), `areas/waiting.ts` (`botsNowHere`) |
| `kuvo_work::bots::routes` | `botsRoute`, `botEventsRoute` |

### Route scenarios

`KUVO_CONFORMANCE_AREA=wakes`: all 9 scenarios pass against both boxes (two added in this part:
archived holds and watch refusals). `KUVO_CONFORMANCE_AREA=bots`: the three list scenarios pass
against both boxes on the machine and Mac variants; the other twelve are 5.1b's.

### Platforms

`KUVO_CONFORMANCE_AREA=bots` (both fixtures) with the Rust box: 15 pass on macOS, on native
Windows (the cross-built `kuvo-box.exe` run directly) and on Linux (the static musl build under WSL
Ubuntu 24.04), the same 15 on each; the other 25 are 5.1b's routes. The TypeScript box passes all 40
on macOS. The `kuvo_work` unit tests (18, including a watch's checks through `/bin/sh`) ran on
macOS. The shared route runner now makes each home private before seeding it, which the Rust box
requires on Windows. No live model run was made for this part: Wake Up Now and the round reach the
engine through the stand-in.

Clippy with warnings denied passes for `kuvo_work`. `kuvo_engine`, `kuvo_peers` and `kuvo-box` have
warnings on `main` in files this part doesn't touch.

### Side by side

A home with a bot (a repeating wake-up) and a conversation (two wake-ups, a summary and a watch),
run through 20 requests on both boxes: listing, refusals, pausing and letting go (a conversation's
and a bot's), `kuvo/bots`, cancelling wake-ups and watches, and Wake Up Now of one that isn't there.
Every wake-up answer and every file under `wakes/` and the bot's folder match. What differs is
owned elsewhere: `kuvo/bots` has no `held`, `refused` or `elsewhere` yet and the bot's `timezone`
file isn't written as the box starts (the start-up retell), both 5.1b; `run/work` and
`run/bots-seen` aren't made as the box starts, 5.1c; the rest are other rows' services (Insights,
artifacts, Marketplace, the Work Queue, approval). `now.since` is normalised by the runner: it's when
each box saw the state begin.

### TypeScript tests

| Test | Cases | Rust |
|---|---|---|
| [wakes](../../box/bridge-tests/wakes.test.js) | 1 first wake-up makes its folder, repeats | `wakes` "adding a wake-up…"; `clock::tests` |
| | 2 a bot's goes in its folder, refusals | `wakes` "a bot's wake-up…", "adding…" |
| | 3 an archived session's first wake-up waits | `wakes` "an archived conversation's first wake-up…" |
| [wakes-mac](../../box/bridge-tests/wakes-mac.test.js) | 1 the Mac app's folder | Dropped: the app-folder Mac box isn't ported (rust-box.md §5). |
| | 2 a client follows them | `routes::followed` (a line now, again on each change here and on a 2 s look). **Open:** a stream test, 5.1b with the bot events test. |
| | 3 a watch made as its agent makes one, refusals in its words | `wakes` "a watch is made…" |
| | 4 the box sends a due wake-up | `fire::tests::a_due_wake_up_goes…` |
| | 5–7 summaries reach the agent, Wake Up Now and held ones keep theirs | `fire::tests::a_due_wake_up_goes…` (summary given); `WakeEngine::deliver` marks it |
| | 8 a stopped conversation's wake-up waits on the box; a repeating one isn't kept twice | `fire::tests::a_stopped_conversations…`, `…still_waiting_from_last_time…` |
| | 9 two stopped conversations with the same words | `Queue::waits_from` is per conversation's queue |
| | 10–11 one written beside the box goes at its time, or within the minute | The resident looks every 2 s (`wakes::LOOK`) and rounds each minute. **Open:** a timed resident test, 5.1b. |
| | 12 archiving through the box holds them; your own pause outlasts it | `wakes` "an archived conversation's…" |
| | 13 archiving one that's moving waits | `routes::post_pause` refuses 409 while moving (unit **open**, 5.1b) |
| [time-zone bridge](../../box/bridge-tests/time-zone.test.js) | 1 a machine's bot wakes on your zone | `clock::tests::repeats_keep_their_time_of_day…` (Tokyo on a UTC computer); the zone files are 2.1's |
| | 2–3 carried zones and rules | 2.1 (B4, B20, B21); the round reads `timezone` and `zoneinfo` (`clock::zone_of`) |
| [time-zone unit](../../box/test/time-zone.test.ts) | wake-up consumers | `clock::tests::daylight_saving_keeps_the_hour`; the rest are 2.1, 5.3 (groups) and 7.3 |
| [wakes-events-shape](../../box/test/wakes-events-shape.test.ts) | new: event lines fit the route's shape | `routes::Row` always carries `watching` and `next` |
| `bots.test.js`, `bots-mac.test.js`: listing cases | list, paused, deletes, `machine` | `bots` list scenarios; `now::tests`, `job::tests` |

Everything else under row 5.1 is listed in the parts below, or open there with its part.

### Deliberate differences from TypeScript

- **The resident fires wake-ups itself.** TypeScript ran the `kuvo-wake fire` shell script each
  minute; Rust runs the same round in-process on the same files, so native Windows needs no shell
  for wake-ups and an agent's own `kuvo-wake add` keeps working beside it. Route adds and watches
  write the files `kuvo-wake` would, instead of running it.
- **Watch failure notes** go to a file named in `KUVO_WATCH_FAILED` rather than descriptor 3: the
  shared process launcher passes no extra descriptors. Readings and sums are unchanged (`cksum`
  ported), so a watch carried over doesn't fire once.
- **Looking for changes** beside the box is a 2-second look rather than file watches, for both the
  round and the event streams: no inotify descriptors, and the same on every platform.
- **Daylight saving** uses the time zone database's "compatible" rule directly (earlier of a repeated
  hour, later in a skipped one), which is what `daytime`'s arithmetic works out.
- **Wake Up Now** starts the engine on demand, as every Rust send does; TypeScript answered 503
  while it was stopped.
- **Not ported:** the engine swap wait (old update path, rust-box.md §5) and reading the bridge's
  record. Conversations the coding agents run (`via` bridge) are skipped by the round and refused by
  Wake Up Now until their adapters are in the resident (4.x).

### TypeScript bug fixed

`kuvo/wakes/events` lines left out `watching` (and `next`), which the route's shape requires.
Fixed in `allWakes`, with `box/test/wakes-events-shape.test.ts` (fails on the old code).

## 5.1b — making, changing, deleting and telling bots

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_work::bots::kit` | `bots/kit.ts` (`readWords`, `readKit`, `wordsHere`, `botTemplates`, `botInstructions`, `runsWithAgents`), `botPersonalities` |
| `kuvo_work::bots::make` | `bots/make.ts` (`makeBot`, the hour a request's answer is kept), `fitsLook`, `botLook`, `botModel`, `boxProjects` |
| `kuvo_work::bots::edit` | `bots/edit.ts` (`changeBot`, `effortOf`, `deleteBot`, `finishDeletes`, `bots-left.json`, `deleted.json`), `bots/edited.ts` |
| `kuvo_work::bots::tell` | `bots/tell.ts` (`told.json`, `toldNow`, `tellBot`, `freshenBot(s)`, `settleFolder`, `settleBot`, `repairRules`) |
| `kuvo_work::bots::seen` | `bots/seen.ts`, `botSeenRoute` |
| `kuvo_work::bots::reach` | `signInRefusal`, `startingModelHere`, `usableModels`, `modelNamed`, `toldThere` |
| `kuvo_work::bots::setup` | `zoneFilesFor`, `writeZoneFiles`, `ownClock`; the library and zone come from the setup service |
| `kuvo_work::approval` | `work/approval.ts` (`defaultApproval`, `approvalsSince`, `unavailable`, `effectiveApproval`, `approvalRules`, `approvalOf` for a bot), `refreshedRules`, `permissionsHere`, `work/former-asks.ts` |
| `kuvo_engine::BotEngine` | the engine calls above, `rewriteOn` |
| `Registry::retire` | the gone key routes (`own-keys.ts` router lines) |

### Proof

- `KUVO_CONFORMANCE_AREA=bots`: 40/40 against both boxes on macOS; the Rust box 40/40 on native
  Windows (three runs) and on Linux (the static musl build under WSL Ubuntu 24.04).
- Side by side, 22 requests over a home with a bot (templates, refusals, every Edit Bot change,
  effort, seen, arrived, retell, the gone key routes, delete twice): every answer matches but
  `held`, `refused` and `elsewhere` (5.1d). Both boxes made the same engine calls in the same
  order; what the bot was told differs only by each home's path. The trash folder is named by time
  and process, its contents the same. `box/approval.json`'s `since` is written by the TypeScript
  box's approval area (5.1c).
- What a bot is told was compared whole with TypeScript's for the same bot: the same text.
- Rust unit tests: `approval::tests` (6), `kit::tests` (4).
- Size: about 108,000 characters of Rust (tests left out) for about 94,000 of TypeScript (1.15×).

### TypeScript tests

| Test | Cases | Rust |
|---|---|---|
| [bots](../../box/bridge-tests/bots.test.js) | 410–453 New Bot's projects and starting model | `bots` "templates" scenario; `reach::starting_model` uses the engine's one rule |
| | 469, 548, 611 a bot made, what's missing said, one whose conversation can't be marked | `bots` "making a bot…", "made from a template", "keeps its look and model", "told who it is" |
| | 527, 572, 589 New Bot sent again, a request that leaves mid-way | `make::Bots::make`: one making per `request`, kept an hour, made whatever the request does |
| | 628, 683, 732, 765, 783, 859 Edit Bot changes, told, refusals | `bots` "changing a bot…", "what Edit Bot changes reaches…"; `edit::onto`, `keep_allowed` |
| | 805 effort | `bots` "a bot's effort…"; `edit::effort_of` |
| | 899, 945, 967, 999 Delete Bot, deletes finished later | `bots` "deleting a bot…"; `run/bots-left.json` and `finish_deletes` as the engine starts. Group chats: 5.3 |
| | 1032, 1090 a machine's own words and facts | `kit::tests`; the Mac kit carry (COMPAT 0.7) isn't ported |
| | 1129–1230 rules made again, a bot brought here | `approval::tests`; `tell::repair_rules`, `settle` (`arrived`) |
| | 305 a moving bot isn't changed | `MOVING` (409) in `change` and `delete` |
| | 286 a bot with the coding agents | 4.x: answered with the agents line until their bridge is in the resident |
| [bots-mac](../../box/bridge-tests/bots-mac.test.js) | 159, 192, 361, 374, 411 this Mac's bots and words | `bots` scenarios (mac variant), `kit::tests` |
| | 203, 212, 224, 237, 272, 287, 299, 429 told once, retold as the engine starts, rules put right | `tell::freshen` under one turn per bot; `follow_engine` |
| | 331 events | 5.1a `kuvo/bots/events`, told on each change here |
| [personalities-box](../../box/bridge-tests/personalities-box.test.js) | 199 a bot retold with a changed personality | `told_now` reads the library from the setup service; `POST kuvo/bots/retell` |
| `bot-keys-gone*`, `bots-held*`, `moves-elsewhere` (bots) | held edits, bots elsewhere, old keys | 5.1d |
| `bots-work*`, `bots-projects`, `bots-dead-models` | a bot's work | 5.1c |

### Deliberate differences from TypeScript

- **The engine starts on demand** to make, change, delete or settle a bot, as every Rust send
  does; TypeScript answered "The engine isn't running." The retell as the engine starts, and seen,
  use it only while it runs.
- **One turn per bot** for Edit Bot, Delete Bot, the retell and arrived: a retell as the engine
  starts never tells the old words after Edit Bot's new ones, and never writes in a folder being
  moved to the trash. On Windows a folder still open a moment (a listing reading it) is tried
  again for up to two seconds before Delete Bot says it couldn't.
- **Bots with the coding agents** (`via` bridge) are answered with the agents line, and a change to
  a coding agent's model isn't carried there, until their bridge is in the resident (4.x). A deleted
  bot's place in group chats stays noted in `run/bots-left.json` for 5.3.
- **A Mac's deleted bot** stays in Kuvo Code's trash folder: the retired app's Finder Trash hand-off
  isn't ported.
- **Not ported:** the older Mac kit carried into the box's facts (COMPAT 0.7 bot-kit), settling a
  new bot's approval again after it's made (5.1c), and what a bot is told while a key it had isn't
  connected (5.1d). Seen leaves the session list to take the engine's `session.viewed`.
