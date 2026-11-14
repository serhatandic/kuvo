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

## BOX-NEEDED (for the box lane's queue)

Each is marked in the code at the spot it's drawn; a neutral stand-in is drawn meanwhile.

| # | What the box should say | Where (route) | Marked at | Drawn meanwhile |
|---|---|---|---|---|
| 1 | What deleting a bot takes with it, in its words (conversation, wake-ups, keys, folder, and where the folder goes: it differs by computer) | e.g. `deletes` on each bot in `GET kuvo/bots` | `bots/mod.rs` (`bot_items`, the Delete Bot row), `bots/page.rs` (`sheet_content`, Delete) | "Removes it from This Mac." and "<name> is removed from This Mac."; nothing is claimed about what goes with it |
| 2 | The bots of your other computers as a list a client can draw and change (name, look, job, where each lives) | e.g. `GET kuvo/bots?across=1`, as `kuvo/computers` lists computers | `bots/mod.rs` (`elsewhere_items`) | A bot elsewhere shows only if the box holds an edit for it or heard a refusal, with a line saying its page is on that computer |
| 3 | Each `held` and `refused` entry's bot by name and look, as the box last knew them | `held[]`, `refused[]` in `GET kuvo/bots` | `bots/mod.rs` (`elsewhere_name`) | The name being given in the held change, else "A bot on <computer>"; a blank face unless the held change gives a look |

Not marked, but the box's to consider: `PATCH` with `model` takes a `variant` (effort); the page
sends none (it has no effort picker), so the box picks.

## Left

- **Not built (scope):** wake-ups (the list, Pause; `paused`, `wakes`, `watching` are read by the
  box but not drawn), goals, Work limits and what's allowed without asking, a bot's own keys, the
  vault, its memory, Export and Import, Move to Computer, a bot made on another computer (the app
  reaches this Mac's box only), group chats' pages, the work bar over a bot's conversation and Stop.
- **Faces:** the app draws one still face (calm eyes); the box offers six. The Look sheet picks
  colour and shape and keeps the bot's face; a new bot's face is the template's or the box's first.
- **A model's effort** isn't picked on the page (see above).
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
- **Delete sits at the foot of the page** with a red button, asked first (`screens-bot-delete`).
  Its wording waits on BOX-NEEDED 1.
- **New Bot is a page**, templates as tiles with Blank first (`screens-bot-new`); Create is always
  enabled and the box says what's missing (`screens-bot-new-refused`). Recommended: keep (no rule
  of the box's is copied to disable it).
- **After Create, the new bot's conversation opens**, its job in the composer.
- **"On Other Computers"** in the sidebar for bots with a held edit or a refusal
  (`screens-bot-held`, `screens-bot-waiting`). Recommended: keep until BOX-NEEDED 2 lists them all.
- **Six recent sessions, then "Show all".**
- **Wording** throughout (row lines such as "What it thinks with.", "The model’s own way", "It
  hasn’t started a session yet.", the empty page's sentence): the user's to settle.
- **The face picker is left out** until faces are drawn (still).
