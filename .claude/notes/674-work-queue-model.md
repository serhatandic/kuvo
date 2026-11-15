# #674 Work Queue: statuses, priorities, labels (handover notes)

Branch `kuvo/674-work-queue-statuses-priority-labels` (the lead's brief calls it kuvo/674-work-queue-model),
worktree `~/.kuvo-code/data/opencode/worktree/c5e2b5/tidy-otter`. Never merged to main by the session: the lead merges.

## Where the logic is (all in the box)
- `box/src/link/work-queue/statuses.ts`: the eight statuses, four priorities, `fromCategory` (old → new).
- `labels.ts` (replaces `categories.ts`): `run/work-queue/labels.json`, CRUD, the primary's labels taken by followers.
- `migrate.ts`: once, from `before-statuses/` (the copy), `model.json` written last. Called from `readQueue`, `owners`, `changeQueues`.
- `assignees.ts`: bots here + coding agents; `usr_you` (store.ts `YOURS`) is your own queue.
- `acts.ts`: `addHere` (POST items), `actHere` PATCH, `withCan`; `view.ts` `canOf`.
- `areas/work-queue.ts`: view with `statuses`, `priorities`, `labels`, `assignees`, `q`, counts.
- `tools.ts`: `queue_labels` in place of `queue_categories`; status/priority/labels/assignee/due.
- Spec: PROTOCOL.md "The Work Queue" (Statuses … From categories), feature `work-queue.edit` (in the gate lists).

## Tests
- New: `box/bridge-tests/work-queue-statuses.test.js` (12 scenarios).
- Existing queue tests updated by a helper (see the final report).

## Other repos
- kuvo-web: worktree `~/.kuvo-code/tmp/opencode/kuvo-web-674`, branch `docs/674`.
- Phone: see the final report.

## State (2026-11-14, late)
- Box: done and committed (statuses, labels, migrate, POST/PATCH, can, q, tools, forward PATCH guarded by `work-queue.edit`).
- New tests: `box/bridge-tests/work-queue-statuses.test.js` (12, all fail on main's built box: proof taken),
  `box/test/work-queue-migrate.test.ts` (3; run on lokum with `scripts/lokum-box-check.sh <worktree> 674 test/work-queue-migrate.test.ts`: 2 passed there before the third was added).
- KuvoClient: decodes `status`→`category`, `statuses`→`categories` (In progress left out: its own section), `inProgress` = status in-progress;
  priority/labels/assignee/due/can added; names both apps build against unchanged (the retired Mac app wasn't compiled by this session).
- Phone: worktree `~/.kuvo-code/tmp/kuvo-674/phone` (branch `kuvo/674-work-queue-model`), `kuvo-ai` beside it is a symlink to the kuvo-ai worktree.
  Compile check (batch dir `~/.kuvo-code/tmp/batch-674`), WorkQueueListTests 11, WorkItemChipsTests 5, DemoMacTests 3, Release build: all pass.
- Helpers (in the kuvo-ai worktree, uncommitted until they report): existing bridge tests → new model; gpui desktop app switch.
- kuvo-web: `docs/674` in `~/.kuvo-code/tmp/opencode/kuvo-web-674`: model section drafted, UI paragraphs still to match the desktop app.

## Decisions waiting on the user (with recommendations)
1. Whose queue a client-added item goes in: `owner` given → that; a bot assignee → the bot's; else `usr_you` (new owner kind "you"). Recommended: keep.
2. Assigning an item from `usr_you` to a bot hands it to the bot's queue; assigning one already in a conversation's queue is only a fact. Recommended: keep.
3. `working: true` makes Todo/Backlog/Waiting → In progress; `working: false` leaves the status. Recommended: keep.
4. Removing a label takes it off every item (no "in use" refusal); clients ask first.
5. Phone: In progress isn't a Move To target (its own section); adding items and labels on the phone come later.
6. The retired Swift Mac app's New/Rename/Remove Category and its move call the removed routes: it can't be edited; say so in release notes or ship with 0.8 only.

## After the first review (same day)
- Fixed: an older box's change (it writes only `category`) is the later word (`standing`, statuses.ts); kept, handed and moved items carry a legacy `category` (`asKept`);
  one process migrates (`.moving-to-statuses`), copy fsynced with its folders; every custom category becomes a label; `fromLabel`/`toLabel` on moved events;
  told files kept with `categories: []`; an older box's forwarded move answered 409; `can.why`, view `new`, assignee `look`; KuvoClient Move To follows `can`.
- Checked on a copy of the real queue (622 items, temp home): 0 lost, 0 differing, events intact, backup byte-identical.
- Checks green at 23dc993c2: the four --check scripts, `bun run check`, bridge work-queue files + compat (121), bridge.test.js/seats/setup-* (95),
  Bun tests on lokum (43), KuvoClient package, phone compile + WorkQueueListTests 11 + DemoMacTests 3 + WorkItemChipsTests 5 + Release build.
- kuvo-web `docs/674` committed (48e475d), not pushed: the page's "Opening it" still describes the pre-0.8 Mac app's panel.
- Left: commit gpui once its helper's follow-up is in (run cargo test/clippy), second review's findings, final report.

## After the second and the desktop reviews
- Box: told mail carries `category`/`categories` for an older box; Parked label made where an item brings it; `overdue` is the box's; all checks green at 0d0503650 (216 bridge, 43 Bun here and on lokum).
- KuvoClient b3dfd2c7e: `overdue`; phone compile, 3 classes and Release green.
- Desktop: a20f68fc8 committed; the helper is applying the desktop review's fixes (overdue from the box, source kinds, sessions kept across a change, queued picks, stale search, flat chips, agent marks, title weight, stand-in faithfulness).
- Left from reviews, not done: clients don't read `fromLabel`/`toLabel` yet; events of items an older box hands in keep raw category ids; a queue file damaged by a power cut after the copy isn't restored from it; told-file shape for an older box has no test; queue_import's `status`/`label` undescribed (tool budget).

## Fix round after the separate review (no MUST, 8 SHOULD)
- Decided by the lead: no release shipped the Work Queue, so every shape kept for "a box from before statuses" is gone (standing, asKept, told/hand/move categories, the compat entry). migrate.ts and fromCategory stay (main builds on the user's computers).
- Migration no longer sets In progress: `started` ids in model.json, settled once by `started.ts` from the 3 s rounds when the engine says who's at work.
- Labels: a primary's removed label is stripped on followers; a follower refuses changing the primary's. `can.assign`, `today`, filters as route parameters (`computers`, `sources`, `owners`, `labels`), a closed issue takes no change, `usr_you` titled "Your own".
- Desktop: days from the box's today, filters sent to the box, Delete/Restore inert while busy, "N open", unknown status grouped, one search at a time.
- KuvoClient/phone: moves only where `can` says; sample Mac serves `new`, `today`, `assign`.
- Green: 216 bridge, 42 Bun here and on lokum, four --check, cargo 309 + clippy, KuvoClient, phone compile + 3 classes + Release; real-queue copy (623 items) 0 lost.
