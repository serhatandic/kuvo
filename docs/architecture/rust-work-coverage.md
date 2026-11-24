# Row 5.1 coverage: bots, wake-ups, notes, attention, approvals

Coverage of the TypeScript tests for [row 5.1](rust-box.md#6-phase-plan) in `kuvo_work`. The row is
split into parts, each mergeable once it has its proof:

- **5.1a** — wake-ups and watches (routes and the resident's own round) and the bot list
  (`kuvo/bots` and its events). This document's first part.
- **5.1b** — the rest of this box's own bot routes: templates, make, edit, delete, arrived,
  retell, seen, the gone key routes, and each bot told what it is as the engine starts.
- **5.1c** — approval (5.1c-1, done), then work (activity, background, oversight, asks, settings,
  the agents' work tools, the work watcher that tells a conversation of its sessions) and notices
  with their holds while you're at a client. Auto's reviewer (`kuvo/sessions/:id/decisions`,
  `work/reviewer.ts`, `work/reviewing.ts`) is 6.1's, with the sandbox it acts in.
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

## 5.1c-1 — approval

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_work::approval::ApprovalService` | `areas/approval.ts` without the reviewer: `approvalHere`, `shown`, `changeFor`, `write`, `toSettle`, `settle`, `remake`, `setApproval`, `reapplyApprovals`, `adoptConversation`, `adoptPending`, `keepApprovals`, `approvalRoute` |
| `kuvo_work::approval::Approvals` | `work/approval.ts`: `approvalOf` (whole), `madeBefore`, `rememberApproval`, `allowFullAccess`, `otherComputersAllowed`, `allowOtherComputers`, `APPROVAL_WORDS` |
| `kuvo_work::bots` (`change`, `freshen`) | `adoptBots`: a bot's conversation from before there were approvals, adopted in its own turn |

### Proof

- New route scenarios `box/test/fixtures/routes/approval.json` (`KUVO_CONFORMANCE_AREA=approval`):
  this computer's switch, default and the other computers' switch; a conversation's approval read
  and set, Full Access refused until confirmed, the rules sent to the engine. Both boxes pass on
  macOS; the Rust box on native Windows and Linux (WSL, static musl). Auto's own fields are left
  out of the scenarios: it's held here and not offered on Windows. `bots` stays 40/40 on all three
  (Windows three times).
- Side by side, 15 requests (switches, every refusal, a conversation set and read again, Full
  Access confirmed and taken back): every answer matches; `box/approval.json` differs only by
  `since`, when each box first kept approvals.
- Size: about 27,000 characters of Rust for about 25,000 of TypeScript (the reviewer's routes
  left out): 1.08×.

### TypeScript tests

| Test | Cases | Rust |
|---|---|---|
| [approval](../../box/bridge-tests/approval.test.js) | 108, 132, 199, 265, 274, 313, 341, 369 a conversation read and set, Accept Edits, Full Access only once confirmed, a new conversation's default, what's never Full Access | `approval` scenarios; `Approvals::of`, `rules`; `approval::tests` |
| | 150, 172, 185, 212, 227, 248, 385, 402 Auto with the sandbox, `.env`, Settings changes reaching conversations in Auto | Auto is held as Ask First until the sandbox (6.1); the rules made for Auto are `approval::tests`, `rules` |
| | 419, 444, 470 Auto's reviewer | 6.1 |
| [approval-round-memory](../../box/test/approval-round-memory.test.ts), [approval-settings-round](../../box/test/approval-settings-round.test.ts) | the Full Access launcher markers; Settings changed while the box was down | 6.1 (the launcher's markers); `keep` makes them again as the engine starts |
| [approval-words](../../box/test/approval-words.test.ts) | each mode's name and line | `options` in the `approval` scenarios |
| [windows-approval](../../box/test/windows-approval.test.ts) | Auto as coming soon, Ask First by default on Windows | `Approvals::unavailable` (`windows`, "Coming soon on Windows.") and `default_approval`; the scenarios leave Auto out so they pass on Windows too |
| [attention](../../box/bridge-tests/attention.test.js), [attention-mac](../../box/bridge-tests/attention-mac.test.js) | the lease (`kuvo/attention`) | Peers (the `attention` area passes on both boxes) |
| | holding your phones' notices while a client's in use, answered or seen | 5.1c, with notices: open |

### Deliberate differences from TypeScript

- **The round looks when something changes** (the engine starts, the Full Access switch or
  Settings › Permissions change, the session list changes) rather than every two seconds, and only
  while the engine runs: a lease each round would keep it from stopping when idle.
- **A bot's conversation from before there were approvals** is adopted by the bots, in the bot's
  turn (as it's told what it is as the engine starts, and before Edit Bot changes it), not by the
  round, so what Edit Bot says to it is the last it hears. A bot's work kept with no approval is
  left as its rules say.
- **Not ported here:** the sandbox's Full Access markers and Auto's places (6.1), and the coding
  agents' subagents' approval (their bridge, 4.x).

## 5.1c-2 — work settings, oversight, work changes, the asks list

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_work::work::WorkSettings` | `work/settings.ts` (`projectsFolder`, `suggestedFolder`, `folderGiven`, `setProjectsFolder`, `keepsAwakeSetting`, `patchWorkSettings`, `workSettings`), `work/awake.ts` (`keepsMacAwakeItself`, `awakeForWork`, `holdForWork`, `lookAtWork`, `keepAwakeWhileWorking`) |
| `kuvo_work::work::Records` | `work/changes.ts`, `work/oversight.ts` |
| `kuvo_work::work::asks` | `workAsks`, `projectPlace`, `folderName`, `resetAskWords` |
| `kuvo_work::work::Work` | `workRoute` (`kuvo/work/events`, `kuvo/work/oversight` and its events), `workAsksRoute` (`kuvo/work/settings`, `kuvo/work/asks` and its events) |

### Proof

- Route scenarios `box/test/fixtures/routes/work.json` (`KUVO_CONFORMANCE_AREA=work`): where new
  projects go (none, refusals, a folder picked, a change on what was read), oversight and asks on a
  new box. Both boxes on macOS; the Rust box on native Windows and Linux (WSL, static musl), with
  `approval` and `bots` again on all three.
- Side by side, 12 requests (every settings refusal, a folder picked, a stale `was`, a keep-awake
  that isn't true or false, asks, oversight): every answer and `box/work.json` match.
- `asks::tests` (a project's folder name).
- Size: about 27,500 characters of Rust for about 24,000 of TypeScript (settings, keep-awake,
  changes, oversight, the asks list and their routes): 1.15×.

### TypeScript tests

| Test | Cases | Rust |
|---|---|---|
| [bots-projects](../../box/bridge-tests/bots-projects.test.js) | 222, 236 a projects folder picked, only a real one | `work` scenarios; `WorkSettings::change` |
| | 100–208, 250–338 asking to make or clone a project, answering, Always, `kuvo/projects/new` | answering and `kuvo/projects/new`: 5.1c-4; asking (the make_project tool): 5.1e |
| `own-mac` (Keep This Mac Awake) | the setting and whether it holds now | `WorkSettings::keep_awake`: `caffeinate -i -w` while agents work, `run/work-awake.json`; only on a Mac box on macOS |

### Deliberate differences from TypeScript

- **Keep This Mac Awake** asks the session list whether agents work (any conversation at work or
  asking) rather than the engine's and bridge's active lists each look; a session list that can't
  say holds as nothing at work.
- **Followed lists** (`kuvo/work/events`, oversight and asks events) look again as the session list
  changes and every two seconds, rather than watching files.

## 5.1c-3 — a bot's work from the session list

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_engine` session list, `work` beside each row (`session_list::work::listed_work`) | `work/listed-work.ts` (`listedWork`), `answersOn`, `depthOf`, `isHiddenRun` |
| `kuvo_work::work::bot` (`Work::bot_work`) | `areas/work.ts` `botWorkSummary` and `kuvo/work`'s route; `workRow`, `finishedState`, `readAsks`, `readStopped`, `botLimits`, `startOfDay` |
| `ApprovalService::keep` pacing | `keepApprovals`' two-second interval |

### Proof

- `work` scenarios add a bot's work for a new bot and the refusals (no bot named, none here): both
  boxes on macOS; the Rust box on native Windows and Linux (WSL, static musl).
- The session list's `work` field is additive: `engine-sessions` passes 81/81 against both boxes
  on macOS and against the Rust box on Linux; on Windows three runs of four passed, one failed
  "session list cursors carry changes and tombstones" with the other session listed as changed
  (not seen in twelve macOS runs of that case alone).
- Seen once under load, open: that failure didn't come back in 10 Windows runs of the case alone
  and 6 of the area against `main`, nor 10 of each against this part; the case now writes both
  answers and `sessions.json` to its output when it fails.
- `session_list::work::tests` (2): a bot's work keeps what it ran on, how it went and its asks
  answered; a subagent keeps where it ran; your own conversation and a hidden run keep none.
- Side by side, 16 requests (with `kuvo/work?bot=` and `kuvo/sessions`): every answer matches.
- Size: about 12,400 characters of Rust for about 10,500 of TypeScript: 1.18×.
- The approval round runs at most every two seconds, the first two seconds after the engine starts,
  as TypeScript's interval does: run the moment the engine was up, it took writes and list reads
  that `engine-sessions` holds for its own cases.

### Deliberate differences from TypeScript

- **A phone's `kuvo/work?bot=`** is answered from the session list, as every other client's is;
  TypeScript asked the engine again for a phone's (each session's ask and last reply).

## 5.1c-4 — making a project, and answering what's asked

### Where it lives

| Rust | TypeScript |
|---|---|
| `kuvo_work::work::projects` (`make_project`, `made_words`, `cloneable`, `shown_path`, `first_commit`) | `work/projects.ts` (`makeProject`, `madeWords`, `cloneable`, `namedPlace`, `shownPath`, `cloneInto`); `git.ts` `firstCommit`, `gitLater` |
| `kuvo_work::work::answers` (`Work::answer_work`, `change_asks`, `allow_always`, `do_work`, `tell_late`) | `work/asks.ts` (`answerWork`, `changeKept`, `allowAlways`, `doWork`, `tellLate`) |
| `kuvo_services::git::change_projects` (beside `read_projects`) | `areas/projects.ts` `changeProjects` |
| `WakeEngine::tell` (a wake-up's `deliver` goes through it) | `engine/queue.ts` `sendInto` to the engine, with `boxNote` |
| `POST kuvo/projects/new`, `POST kuvo/work/asks/:id` in `Work::register` | `areas/work.ts` `newProjectRoute`, `workAsksRoute` |

### Proof

- `work` scenarios (3 new, 8 in all): a client makes a project (refusals: no name, no projects
  folder with `needs`, a folder not given in full, a name that makes no folder, addresses git won't
  clone, a folder with files in it, a project there already; a plain folder and a git repository
  made); what's asked answered (not asked, a wrong answer, the projects folder picked with the
  first allow, answered once, deny, deny overnight for a clone); and an answer given after its
  tool stopped waiting told to its conversation (`[The user answered] …`, marked "Your answer").
  8/8 against both boxes on macOS; against the Rust box on native Windows and on Linux (WSL, static
  musl). `bots` stays 40/40 on all three (wake-ups now deliver through `WakeEngine::tell`).
- `work::projects::tests` (3): which addresses git clones without asking; `~` for paths under your
  home only; a new repository's empty first commit, with nothing left over, made once.
- Side by side, 21 requests (`kuvo/projects/new`, `kuvo/work/asks/:id`, `kuvo/work/asks`,
  `kuvo/projects`, `kuvo/work/settings`): every answer matches. The projects, asks and settings
  files match too, except as below.
- Size: about 30,700 characters of Rust for about 23,500 of TypeScript: 1.30× as the files stand;
  1.14× without each line's indentation (Rust indents by four, TypeScript by two, and rustfmt
  breaks a chain over 60 characters into lines of its own).

### TypeScript tests

| Test | Cases | Rust |
|---|---|---|
| [bots-projects](../../box/bridge-tests/bots-projects.test.js) | 236 a client makes a project in the projects folder | `work` scenarios |
| | 250 a new project starts with a commit, by Kuvo Code where no one's set | `work::projects::tests`; the scenarios' runs set who commits, so Kuvo Code's own name is the unit test's fallback: open for 5.1e's tool scenarios |
| | 162, 290 what would fail anyway (a folder with files in it, a project there already, an address that isn't a repository's) | `work` scenarios, as refusals of `kuvo/projects/new`; not asking first is the tool's: 5.1e |
| | 208 an answer given after its tool stopped waiting reaches its conversation; a second answer finds it gone | `work` scenarios |
| | 222 allowing needs a projects folder, and only a real one; a no doesn't | `work` scenarios |
| | 132 Always kept with the bot | `allow_always`; its scenario needs the tool asking first: 5.1e |
| | 299, 338 a project that can't be made is the answer ("Couldn't do it") | `do_work`; read-only folders aren't set up in the shared scenarios: 5.1e's tool cases |

### Gaps and owners

- **A plan's reset answered** (`resetLimits`): the coding agents' `spendReset` isn't in this box;
  Rust answers that the reset wasn't used. 5.4 (usage and limits).
- **`box/found-folders.json`**: TypeScript notes a made project's folder as first found there
  (`noteFolderFound`, for Auto's reviewer). 6.3, with the folders it lists.
- **A late answer to a conversation elsewhere**: one the coding agents run, or one that moved to
  another computer since (a letter, `tellMovedAway`), stays untold here. The agents' sessions: 4.4;
  letters: 5.1e with the work tools. Telling those left untold as the box starts and every few
  minutes (`tellLateAnswers`): the work watcher, next in 5.1c.

### Deliberate differences from TypeScript

- **The session list** lists a made project at once; TypeScript's saved list caught up later (side
  by side, at shutdown its `run/lists/sessions.json` didn't have them yet).

