# Row 5.1 coverage: bots, wake-ups, notes, attention, approvals

Coverage of the TypeScript tests for [row 5.1](rust-box.md#6-phase-plan) in `kuvo_work`. The row is
split into parts, each mergeable once it has its proof:

- **5.1a** — wake-ups and watches (routes and the resident's own round) and the bot list
  (`kuvo/bots` and its events). This document's first part.
- **5.1b** — the rest of the bot routes: templates, make, edit, delete, arrived, retell, seen,
  held edits and bots elsewhere, the gone key routes.
- **5.1c** — work (activity, background, oversight, asks, settings, the work watcher that tells a
  conversation of its sessions), approval and Auto's decisions, attention, notices.

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

Everything else under row 5.1 (`bots*`, `bot-keys-gone*`, `box-work`, `work-*`, `approval*`,
`attention*`, `at-work*`, `sandbox-asks`, `outside-asks-live`, `personalities-box` bot retell,
`notices-plain`, `notices-rule`, `conversation-activity`, `work-answers`, `setup-lists`, `mac-peers`
7 and 13–14, `devices`, and the cases other coverage maps assign to 5.1) is open, owned by 5.1b
and 5.1c as listed above.

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
