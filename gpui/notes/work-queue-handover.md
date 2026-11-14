# The Work Queue: handover

## Done (#674: statuses, priorities and labels in place of categories)

Notes: `notes/screens-queue.md` (with the Zed study). Scenarios: `kuvo/tests/queue.rs` (34, real
clicks and keys against the stand-in), unit tests in `kuvo_box/src/work_queue.rs`,
`kuvo/src/queue/mod.rs` and `kuvo/src/time.rs`. Shots: `shots/screens-queue-*-{light,dark}.png`
(28 screens); `screens-area-queue` rendered again.

1. **The queue** (`kuvo/src/queue/`): the box's view across your computers
   (`GET kuvo/work-queue?owner=all&counts=1`), its items under the box's statuses in the box's
   order (a status with nothing leaves no header), Done, Canceled and Deleted folded (whatever the
   box says is `closed`), each row with its status mark, `#n`, title, what's being done on it,
   priority (none for Normal), labels (two, then "+N"; one in a narrow window), who it's for (a
   bot's still face from the view's `assignees`, else an initial), its due day, its computer and
   when it changed. Tabs: All, each open status with the box's count, and a tab for a closed
   status or a label while that's what shows. The sidebar: All Items (the box's `open`), Status
   (every status with its count), Labels (each with its count, + to make one, and its own menu, from a ••• under the pointer
   or a right-click, to edit or remove).
2. **The search is the box's** (`…&q=<words>`): each change to what's typed asks the box; the
   list, the tabs and the group headers show the narrowed view and its counts, the sidebar keeps
   the whole queue's. A newer search replaces one on its way, and an answer is only taken for what's
   typed now. Filters (computer, source, owner) stay view state: they choose which of the box's
   items show, and no count shows over a filtered list.
3. **New Item** (`screens-queue-new*`): written like a message (a title line, a description, no
   boxed form), chips under it (Status, Priority, Assignee, Due date, Labels), each opening a menu
   of what the box's view offers; a number picks a row (1 to 9), typing narrows, Up/Down/Return,
   Escape closes the menu alone; Labels picks several and makes one from what's typed; Due date has
   Today, Tomorrow, In a week, a day typed, None; Create more; ⌘↩ creates; Escape closes.
   `POST kuvo/work-queue/items` with no owner: whose queue is the box's to decide.
4. **An item's page**: the same chips change it (`PATCH kuvo/work-queue/items/<id>`, one field a
   pick; `null` clears an assignee or a due day); Edit… (title and description, the same sheet
   without chips), Mark Done, Mark Canceled, Delete, Restore.
5. **`can` decides what's offered**, on rows and pages: `edit` (Edit…), `set` (the chips' menus,
   the open statuses), `close` (Mark Done, Mark Canceled, the closed statuses in the menu),
   `delete`, `restore`. A chip that says nothing and can't be set isn't drawn; a row with nothing
   to do has no •••, and a mark there that says why. Why (`can.why`) is the box's sentence, drawn
   as it is: on that mark and on a chip that doesn't open (rested on), and in the page's notice.
   What the box still refuses is drawn in its words.
6. **Labels**: New Label (a name, one of the ten colours or the box's choice), Edit Label (rename,
   recolour), Remove (asked first, saying it comes off every item), each
   `POST kuvo/work-queue/labels`. No labels at all is said in the sidebar and in the Labels menu.
7. `kuvo_box::work_queue`: `Status`, `Priority`, `Label`, `Assignee`, `Can`, `Counts`, the item's
   new fields, `work_queue_view`, `work_queue_search`, `add_work_item`, `change_work_item`,
   `add_label`, `change_label`, `remove_label`. Gone: `WRITES`, `Category`, moves, categories.
   `work_counts` reads the box's `counts.open`.
8. The stand-in box (`kuvo/src/stand_in/queue.rs`) answers all of it in the box's shapes, with
   state: the fixed statuses, labels made/changed/removed, POST and PATCH, `can` (an item on a
   computer that's away: all false; issues: no edit, no close; one on its way to another computer:
   delete only; a deleted one: restore only; each with the box's `why`), `new`, a bot's `look` on
   an item's assignee, `q` with narrowed counts.
9. Every read-only path for a box that takes no writing is gone (the update gate holds such a
   box), and so is every `BOX-NEEDED` in the Work Queue's code.
10. `cargo build`, `cargo test --workspace` (304) and `cargo clippy --workspace --all-targets`
    clean, every run with `KUVO_*` unset and the stand-in box only.

## After the review

- **Late is the box's** (`overdue` on an item): a due day is red only where the box says so; the
  app's own rule is gone.
- **The Source filter is by kind**, as the box counts them: "GitHub issues" (`github`) and "GitHub
  pull requests" (`github-pr`) are two.
- **A change's answer keeps the sessions the app knew** for that item (the box answers a change
  with each "New Session", none at work) until the view is read again.
- **A pick while a change is on its way waits behind it** (the latest wanted, field by field) and
  is sent when it answers; the chips and the menu show what's wanted meanwhile, so two labels
  picked quickly are both put on. A refusal drops what waited.
- **A search's result is drawn only for the words it answered**; other words typed show "Searching
  for …" until the box answers.
- **A chip that can't be changed is drawn as a fact** (no edge, no fill, quiet words), with the
  box's why when rested on.
- **A coding agent wears its own mark** (the rail's, by its id), an initial only where Kuvo has
  none.
- **New Item's title is set as a title** (`TextField::title`: the page's title size, medium), over
  the description.
- **Edit Item sends only what changed**, and nothing where nothing did.
- After a chip's menu closes the keyboard is back in the field it was in (title or description).
- A due day's line shows a day a fortnight from today; the tabs' end fades where they're cut, and
  so does a long menu's; a dark sheet's chips and divider have the stronger edge; "As studio-mini
  last reported it."
- The stand-in answers as the box does: `overdue`, sources by kind (it has a pull request), a
  change's answer without session facts, an item of your own assigned to a bot handed to that
  bot's queue (`handedBy`), and `can` for a computer on an older Kuvo Code and for a shared
  machine, each with the box's sentence. A notice's words are asserted as the page draws them.

## BOX-NEEDED

None. The box now says what the last note asked for, and the app draws it: what a new item starts
as (`new: {status, priority}` on the view: New Item's preselected chips, shown and sent), why
something can't be done (`can.why`: the tooltip of a chip that doesn't open, the mark where a row's
••• would be, and the page's notice, in the box's words; nothing is worked out from
`computers[].away` any more), and a bot's `look` on an item's `assignee` (rows and the page draw
its face from it; the view's `assignees` is only the fallback).

Still awkward to draw from, for the box session to weigh (none blocks anything):
- **Filters as the box's query parameters**: `q` narrows the counts; the computer, source and owner
  filters are the app's, so no count shows over them. `computer=`, `source=`, `ownerIs=` (and
  `label=`, `priority=`, `assignee=`) on the view would make them the box's, with counts.
- **The colours a label may be** are listed in the app (`COLORS`, the protocol's ten): `colors` on
  the view would say them.
- **Which closed status is "done"**: Mark Done and Mark Canceled send the protocol's ids (`done`,
  `canceled`), shown only where the view lists them. A `kind` on a status would say it.
- **A status's or a priority's look** is drawn by its id (a newer box's status gets a plain ring).
- **Relative days** ("today") are this computer's day, sent as `YYYY-MM-DD`.

## Left

- **Not built:** filters by label, priority or assignee as chips (a label is a scope in the
  sidebar; see the box note above); reordering within a status and Move to Top (`before`, `after`,
  `top`); drag and drop; an item's activity feed (`events`, `handed`); opening or saving a file;
  Import; handing an item on; the per-conversation queue; `#243` chips in conversations; the note
  when another computer is away in the list (`computers[].away`); `wanted`; a calendar for the due
  day; a tooltip with a shortcut on each chip (Capy's "P"); sorting.
- **Not live:** the view is read as the area shows and after each change, not followed
  (`kuvo/work-queue/events`).
- **The search asks the box on every change to what's typed** (no pause: tests take no clocks). On
  a machine over SSH that's a request a keystroke; older ones are dropped, not stopped on the box.
- **Keyboard:** no walk through the rows, no ⌘F, no shortcut for New Item; Tab doesn't move between
  a sheet's fields. In a menu, a number picks only while nothing's typed in its line (a label's
  name may have numbers); a due day's numbers pick until its line is clicked into.
- **Left in the client, view words:** "At work", "Mark Done", "Mark Canceled", "Open"/"Closed" over
  the status menu, "No one", "Bot"/"Coding agent", Today/Tomorrow/In a week and "Nov 16", the
  empty lines, "From GitHub", "In your own queue".
- No VoiceOver labels (GPUI has none here yet); larger text not tried.
- The phone and the Mac app: not this piece. Docs and CHANGELOG: none needed for the GPUI app
  (not shipped); the box's change has its own.

## Edge cases left

- A row's trailing things (labels, face, due day, computer, when) aren't in slots of a fixed
  width, so they don't line up in columns from row to row.
- A label an item wears that the view doesn't list isn't drawn (a rename in flight).
- A menu taller than the room under its chip is moved up over the chip, not flipped above it.
- Leaving a page drops a sheet's typed text, unasked (Bots' too).
- A due day typed for a new item is only checked by the box at Create.
- "Create more" keeps the chips as picked for the next item (only the text clears).

## Decisions for the user

Built without drafts (the design was settled from Capy's shots); each is cheap to change. Shots
are in `gpui/shots/`.

- **Tabs are All and the six open statuses**; Done and Canceled are in the sidebar and folded in
  the list (`screens-queue`). Other way: Capy's "For you / Assigned / All". Recommended: keep
  (statuses are what the box counts; "for you" is Needs you).
- **A status with nothing has no header** (the reference keeps Done and Canceled as empty folded
  cards). Recommended: keep; its tab and sidebar row still show it at 0.
- **Status and priority marks are drawn, not icons** (rings, bars; no new icon files), in the
  theme's red, green and amber. A look question: recommended to keep until an icon pass.
- **New Item preselects what the box says a new item starts as** (`new`: Todo and Normal), as Capy
  does; from a status's list it starts there, from a label's with that label. Recommended: keep.
- **Return in the title goes to the description; ⌘↩ creates** (Linear's and Capy's way).
  Recommended: keep.
- **A label's menu is a ••• that shows under the pointer on its row** (and a right-click, and •••
  on its own list). Recommended: keep.
- **Removing a label is asked first; deleting an item isn't** (it's kept under Deleted, with
  Restore). Recommended: keep.
- **Mark Canceled is in •••**, Mark Done a button. Recommended: keep.
- **A row shows two labels, then "+N"** (one in a narrow window). Recommended: keep.
- **Overdue is red** on an open item's due day. Recommended: keep.
- **Wording** throughout ("Needs you" and the rest are the box's; the app's are listed above): the
  user's to settle.
