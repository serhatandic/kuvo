# Bots: handover

## Done

Notes: `notes/screens-bots.md` (with the Zed study). Scenarios: `kuvo/tests/bots.rs` (13, real
clicks and keys against the stand-in), unit tests in `kuvo_box/src/bots.rs` (3) and
`kuvo/src/bots/mod.rs`. Shots: `shots/screens-bot-*-{light,dark}.png` (14 screens), each looked at;
`screens-area-bots` rendered again.

1. **A bot's page** (`kuvo/src/bots/`): face, name, job, what it's doing now (`now`), Chat; its
   recent sessions (`GET kuvo/work?bot=`), each opening in Sessions; its settings in Settings' row
   style.
2. **Its conversation** (`workspace.rs`, `open_bot_chat`): the app's conversation view and
   composer, in Bots, in the page's place; back and forward go between the two.
3. **Editing** through `PATCH kuvo/bots/<id>`: Name, Job (line and details), Look (colour and
   shape), Personality, Model. The box's answer is drawn: the bot as it is now; `202 {held}` as
   "Waiting for <computer>"; a refusal in its words. `held` and `refused` in `GET kuvo/bots` are
   drawn in the sidebar ("On Other Computers") and on a page.
4. **New Bot** (the + beside Bots; `GET kuvo/bots/templates`, `POST kuvo/bots` with `request`),
   then its conversation with `firstMessage` in the composer.
5. **Delete** (`DELETE kuvo/bots/<id>`), asked first.
6. `kuvo_box::bots`: typed reads and sends for all of the above; `areas::Bot` reads
   `jobDetails`, `personality`, `model`, `project`, `agents` too.
7. The stand-in box answers these in the box's shapes (`kuvo/src/stand_in/bots.rs`): its bots are
   state now (made, changed, deleted, moved to another computer, a refusal), each with its own
   conversation among the box's sessions.
8. `cargo build`, `cargo test --workspace` and `cargo clippy --workspace --all-targets` clean, every
   run with `KUVO_*` unset and the stand-in box only.

## What the box says now (the BOX-NEEDED items, built)

None is left marked in the code; each stand-in is gone and the box's answer is drawn (PROTOCOL.md,
`kuvo/bots`; features `bots.deletes`, `bots.effort`, `bots.elsewhere`, all three in the update gate).

| What | Where (route) | Drawn |
|---|---|---|
| What deleting a bot takes with it, a line each in the box's words, then what stays | `deletes` on each bot in `GET kuvo/bots`, and on each in `elsewhere` | Delete's confirm, as the lines are (`sheet_content`); the Delete Bot row claims nothing itself |
| The bots of your other computers, as each last told the box asked | `elsewhere` in `GET kuvo/bots` | "On Other Computers" lists them all; a page with Name, Look and Delete, each held by the box for that computer |
| Who a held edit or a refusal is for | `name`, `look` on each of `held`, `refused`; `elsewhere` lists each bot as you left it, with `waiting`, `refused`, and `unlisted` for one no computer lists now | The box's list as it is: the app merges nothing (`There::of`) |
| Why a held edit waits though its computer answers | `waits` on a `held` entry | The notice's words |
| The effort a bot's model runs at | `effort` in `PATCH kuvo/bots/<id>` | An Effort row under Model, from the model's own efforts as the box lists them |

## Review

One reviewer pass; every MUST and SHOULD fixed, with a scenario for each where one can be written
(they weren't run against the code before the fixes):
- **MUST:** `held` and `refused` left out by the box (while it can't check its seals) were read as
  none, so "Waiting for <computer>" vanished: what was last heard is kept now (PROTOCOL.md), and
  the stand-in sends both lists always, as a box does, with a switch for leaving them out
  (`an_edit_for_a_bot_on_another_computer…`, `kuvo_box`'s `whats_held_is_kept_while_the_box_cant_say`).
- **MUST:** a sheet's answer landed on whatever sheet was open by then (another bot's Delete could
  be closed, or taken for done): each answer is its own bot's and its own sheet's now
  (`live::an_answer_that_comes_after_youve_gone_to_another_bot_is_its_own_bots`, run the app's way).
- A row's "Saving…" and a refusal showed on whichever bot was open: both are per bot, and a second
  pick waits for the first's answer (`live::a_refusal_that_comes_after_youve_gone…`).
- A bot's conversation was remembered by its session, which the box may replace (a coding agent's
  model): back, forward and returning to Bots ask the box's list for it, and a conversation open
  goes to the new session as the list is read.
- Create answered after you'd left New Bot pulled the window to the new bot, and coming back made a
  second request: the form stays while one's being made, and the window moves only from New Bot
  (`live::a_bot_made_after_youve_left_new_bot_moves_nothing`).
- Delete's understated confirm is a decision for the user now (below).
- Tests added: Delete of a bot that moved away, held (`deleting_a_bot_that_moved_away…`); Back after
  a delete asserts where it lands.
- NITs fixed: a `now` in a change's answer is kept; a computer the box gives no name is "another
  computer"; a template made for a project says so on Works in (`wantsProject`, a fact drawn); Back
  passes over a deleted bot's page; a sheet closes when its bot leaves the list; the Name row's line
  no longer repeats the name.
- NITs left: every change and each showing of the area reads the bot's sessions, the templates and
  the models again (three small reads); the template's name filling the Name field while it's
  still a template's name is the app's own form filling.
- Not tested: a `PATCH` that answers `bot: null`, and one that answers with a new `session` (the
  stand-in does neither).

## Left

- **Not built (scope):** wake-ups (the list, Pause; `paused`, `wakes`, `watching` are read by the
  box but not drawn), goals, Work limits and what's allowed without asking, the
  vault, its memory, Export and Import, Move to Computer, a bot made on another computer (the app
  reaches this Mac's box only), group chats' pages, the work bar over a bot's conversation and Stop.
- **Faces:** the app draws one still face (calm eyes); the box offers six. The Look sheet picks
  colour and shape and keeps the bot's face; a new bot's face is the template's or the box's first.
- **A bot on another computer:** its job, personality, model and sessions aren't shown or changed
  from here (the box asked has only what that computer told it: name, look, job's line); the page
  says so. Its job isn't edited from here, as a `PATCH` replaces the whole job and its details
  aren't known here.
- **Not live:** the bots list and a bot's sessions are read as the area and the page open and
  after each change, not followed (`kuvo/bots/events`, `kuvo/work/events`): `now` on a page left
  open goes stale. The sidebar had this already.
- **Left in the client, view words:** "N at work, N waiting on you" (the sidebar's), a session's
  state word from the box's `state` (`state_words`), "Waiting for <computer>" and which fields wait
  (named by the box's own field names), "<computer> didn’t take a change" over the box's message,
  the rows' names and lines.
- Tab doesn't move between the Job sheet's two fields (the text field has no tab stops yet).
- The side panel isn't offered beside a bot's conversation.
- Running live (reads off the window's thread) has no test of its own here; the same `run` path is
  Settings', which has one.
- No VoiceOver labels (GPUI has none here yet); larger text not tried.
- The phone: no change (the GPUI app isn't shipped). Docs and CHANGELOG: none needed (nothing
  users run changes).

## Decisions for the user

Built without drafts (the lead's brief: build, with shots); each is cheap to change. Shots are in
`gpui/shots/`.

- **A bot has a page, and Chat opens its conversation** (`screens-bot-page`, `screens-bot-chat`).
  The Mac opens the conversation straight from the row. Recommended: keep the page first (it's
  where "what's it doing" is answered); the other way is one line (`area_picked`).
- **A bot's conversation shows in Bots**, not in Sessions, under a strip with the way back.
  Recommended: keep.
- **No Edit Bot form with Save:** each setting changes by itself, in Settings' row style
  (`screens-bot-rename`, `-job`, `-look`). Recommended: keep; an edit held for another computer
  then waits a field at a time, as the box holds them.
- **Delete's confirm shows the box's lines** (`screens-bot-delete`, `screens-bot-elsewhere-delete`),
  each with a trash mark, what stays with a check; the Delete Bot row says only "You’ll see what
  goes with it before anything is deleted." The lines' wording is the box's (`bots/deletes.ts`).
- **A bot on another computer has a page** (`screens-bot-elsewhere`): Name, Look, Lives on, Delete,
  and a line saying what's on its own computer. Its row in the sidebar says "On <computer>".
- **Effort is a row of its own** under Model ("The model’s own" first, then the model's efforts);
  picking a model leaves its effort to the model.
- **New Bot is a page**, templates as tiles with Blank first (`screens-bot-new`); Create is always
  enabled and the box says what's missing (`screens-bot-new-refused`). Recommended: keep (no rule
  of the box's is copied to disable it).
- **After Create, the new bot's conversation opens**, its job in the composer.
- **"On Other Computers"** in the sidebar lists every bot your other computers told this box of,
  then any it only holds an edit for (`screens-bot-held`, `screens-bot-waiting`).
- **Six recent sessions, then "Show all".**
- **Wording** throughout (row lines such as "What it thinks with.", "The model’s own way", "It
  hasn’t started a session yet.", the empty page's sentence): the user's to settle.
- **The face picker is left out** until faces are drawn (still).
