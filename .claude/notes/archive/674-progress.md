> Historical, superseded by [Screens queue](../../../gpui/notes/screens-queue.md), kept for its findings.

# #674 progress (stopped at a safe point, 2026-11-15, for the usage window)

Everything is committed; nothing is running. Branches: kuvo-ai `kuvo/674-work-queue-statuses-priority-labels`
(worktree `~/.kuvo-code/data/opencode/worktree/c5e2b5/tidy-otter`), phone `kuvo/674-work-queue-model`
(`~/.kuvo-code/tmp/kuvo-674/phone`), web `docs/674` (`~/.kuvo-code/tmp/opencode/kuvo-web-674`).

## Fix round (separate review: no MUST, 8 SHOULD): done and committed
All of the lead's list: compat for "boxes from before statuses" deleted (migrate.ts + fromCategory kept); labels across
computers; `can.assign`; a closed issue takes no change; `today`; filters as route parameters; desktop Delete/Restore
while busy, "N open", unknown status group; phone/KuvoClient items; NITs; migration In progress settled once by
`started.ts`. Changelog and docs folded.

## Reviewer pass on the fix commits: 2 MUST, 5 SHOULD, 5 NIT
Fixed and committed: M1 (assign false on a far item's page and answers), M2 (strict `atWorkForSure`, tested through the
engine stand-in), S1/S2 (labels.json keeps the primary's key; strip only by the same primary's non-empty word; a box
that's its own primary changes its labels), S3 (said in PROTOCOL), S5 (KuvoClient scenarios), N1, N2 (the
`work-queue.edit` peer gate and `older` are gone), N4 (comment), N5 (changelog words).
S4 (desktop: a narrowed list re-asked when the queue changes) is fixed in `set_said` and committed; cargo test 309 and
clippy clean; **its scenario is NOT written yet**.

## Left to do when resumed
1. Desktop scenario for S4 in `gpui/kuvo/tests/queue.rs` (filter on; change an item through a second `BoxClient` on the
   same stand-in; `page.set_said(...)`; assert one more narrowed GET and the list/counts follow).
2. N3 (desktop: under a label's list, clicking a status tab leaves the label): fix or list as an edge case.
3. Re-run on the final tips: the four `--check` scripts, `bun run check`, bridge files (work-queue*, compat, bridge,
   seats, setup-*), Bun tests here and `lokum-box-check.sh`, cargo test + clippy, KuvoClient `swift test`, phone
   compile + WorkQueueListTests/DemoMacTests/WorkItemChipsTests + Release build. Last full green run was before the
   reviewer fixes for bridge (only work-queue-across + statuses re-run since: 46 pass); KuvoClient passes; phone not
   re-run since KuvoClient's last two commits.
4. Re-review after the two MUSTs (one `reviewer` pass on commits after `8e3377cf2`), then the final report.
5. Remove `~/.kuvo-code/tmp/dd-phone-release-674` and `~/.kuvo-code/tmp/batch-674` at the end.

## Edge cases to list in the report (not fixing)
add-for-bot without a hand-off note; `before-statuses/` never removed; `fromLabel`/`toLabel` not drawn; a primary that
removes its LAST label doesn't take it off followers (an empty list never strips); the settle promotes an item still
Todo/Backlog/Waiting even if you moved it between those since; phone picture test `feedShots` times out on main too;
the retired Swift Mac app not compiled by this session.

## Resumed after the reset (2026-11-15)
- S4's scenario written (`a_narrowed_list_follows_the_queue_as_it_changes`; fails without the fix: 1 ask instead of 2).
- N3 left as an edge case (a scope is one of status or label; combining them is a design change).
- Green on the tips: four --check, `bun run check`, bridge 216, Bun 42 here and on lokum, compat, cargo 310 + clippy 0,
  KuvoClient, phone compile + 3 classes + Release; real-queue copy (626 items) 0 lost, backup byte-identical.
- Waiting: the re-review of `8e3377cf2..HEAD`; then the final report and removing the two build folders.
