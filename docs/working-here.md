# Working here

How coding sessions work on Kuvo, in full. CLAUDE.md keeps one line of each.

## Reporting and handovers

- **Report to the lead** (Kuvo Foreman) or the user. Never wait on a question box: it holds the
  whole session.
- **When the user isn't around:** keep a list of every decision that needs them (design, brand,
  wording, anything needing a prompt or the user at the Mac), with what you recommend and why.
  Build what doesn't depend on the answers, so their choices come last. Never build the
  "recommended side" of a waiting decision when it costs the user anything (an ask, a refusal, an
  extra step, a limit, a change to what they already use): build around it and say it isn't built.
  Hand the whole list over at once, with paths to the mocks.
- **Fresh session at each phase:** write a handover note (done, open, decisions) and ask the lead
  or the user for a fresh session.
- **Check against the user's goal, not only the spec.** Anything that gets in its way goes to the
  top of your report.
- **Subagents: two at a time** per session, workers and their own subagents counted. Start another
  only when one finishes. No reviewer subagents or review passes unless the user asks. Check your
  own client changes against "Where new code goes" (`architecture/how-kuvo-works.md`) before
  committing: logic in the wrong place blocks the merge.

## The bar

- **Small fixes land fast**, with the check that proves them (for a bug, the test that fails on the
  old code) and no ceremony beyond that.
- **Bigger work keeps the same care to the end:** read before changing, test, look at the UI, check
  the changelog. If you're skimming, guessing, or about to report what you haven't checked, stop
  and do it properly; if the context is too full, say so and suggest a fresh session.
- **Routes are documented in their definition** (`box/src/shared/routes`), and listed by
  `scripts/build-built.sh` in `protocol/ROUTES.md`. Add to `protocol/PROTOCOL.md` only a
  rule a client must know that the route can't say, in a few lines.

## Designing what people see

- Design the UI with as much care as the internals, in the same task; no sketch to polish later.
- **Drafts first:** before any new screen, control or visible change, draft two or three options
  (light and dark) with your recommendation, and let the user pick, even when one seems obvious.
  Meanwhile build only what doesn't depend on the choice. A bug fix that restores an existing
  design needs none.
- **Look at how the best do it** (Codex, Claude Code, Cursor, GitHub, Linear, Apple's apps): say
  what you borrow, where we differ, and why. Where they've settled a behavior (what a mode does,
  what asks and what's refused, how a common control works), match it, showing ours, theirs and
  what we'll do side by side; doing it differently is the user's call.
- Match how the app already draws the same kind of thing (`gpui/kuvo_ui` and the related views in
  `gpui/kuvo`): materials, spacing, wording, states, motion.
- Work through every state: empty, loading, failed, long text, many items, a narrow window, light
  and dark, larger text, VoiceOver. See it yourself in a demo copy where you can; if you couldn't,
  say so.
- When the machinery makes the right UI hard, change the machinery.
- **The right panel is only for common tools** (browser, terminals, files, Changes). A feature's own
  view goes in a popover or sheet on the conversation, or a page in Settings.
- **Brand:** ask before icons, logos and marks, brand or accent colors, taglines and headlines.
  Render options and let the user pick; never apply one "for now".

## GitHub

- Sessions never push, tag or open a PR. Commit finished, tested work to your branch and tell the
  lead it's ready; the lead merges into main and pushes, on the user's word.
- No polling `gh run`, `gh api`, `gh pr` or `git fetch`. No GitHub writes beyond the task (issues,
  labels, releases, tags, settings). One account: never another account or token.
