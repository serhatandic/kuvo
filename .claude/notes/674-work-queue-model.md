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
