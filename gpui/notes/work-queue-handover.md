# The Work Queue: handover

## Done

Notes: `notes/screens-queue.md` (with the Zed study). Scenarios: `kuvo/tests/queue.rs` (19, real
clicks and keys against the stand-in), unit tests in `kuvo_box/src/work_queue.rs` and
`kuvo/src/queue/mod.rs`. Shots: `shots/screens-queue-*-{light,dark}.png` (17 screens);
`screens-area-queue` rendered again.

1. **The queue** (`kuvo/src/queue/`): the box's view across your computers
   (`GET kuvo/work-queue?owner=all&counts=1`), its items under the box's categories in the box's
   order, each wearing its category as a chip (#674); scope tabs (All, In Progress, each category,
   with the box's counts) that pick the same scope as the sidebar's rows; a search; filters as
   chips to take off (computer, outside source, owner); a line where nothing's listed; Done and
   Deleted folded.
2. **An item's page** (`GET kuvo/work-queue/items/<id>`): what names it and how it stands, its
   title, its category as a menu, progress, Description as Markdown, the sessions working on it
   (each opens in Sessions), links (opened in the browser), files, notes, who added and changed it
   and whose queue it's in; one on another computer says whether it was read there or is as last
   told (`told`); a deleted one, with Restore; one on its way to another computer (`handing`).
3. **What you do**, each through the box's route, its answer drawn, its refusal in its words:
   moving an item between categories and Mark Done (`POST …/items/<id>/move {category}`), Delete
   and Restore (`DELETE …/items/<id>`, `POST …/restore`), New Category, Rename and Remove
   (`POST kuvo/work-queue/categories {add | rename, to | remove}`).
4. **Adding an item and editing its title and description** are drawn and tested against two
   routes no box has yet (below): New Item and Edit… show only where the box's `features` name
   `work-queue.edit`. Against today's box neither shows: the + is there, off, saying why when rested
   on, and the queue's foot says who adds items.
5. `kuvo_box::work_queue`: typed reads and sends for all of the above.
6. The stand-in box answers these in the box's shapes (`kuvo/src/stand_in/queue.rs`): its queue is
   state (moved, done, deleted, restored, added, rewritten, categories), with an item on a
   computer that answers, one on a computer that's away, and two GitHub issues.
7. `cargo build`, `cargo test --workspace` and `cargo clippy --workspace --all-targets` clean, every
   run with `KUVO_*` unset and the stand-in box only.

## BOX-NEEDED

Each is marked `// BOX-NEEDED:` in the code. The first is what scope 3 asked for and the box
doesn't do; the rest are #674's direction (category chips, dynamic categories, no defaults).

| What | Proposed (route) | Where it's marked; what's drawn meanwhile |
|---|---|---|
| A client adds an item, and changes an item's title and description. Today the protocol says clients "never change what they say": only agents' tools do (`queue_add`, `queue_update`). | `POST kuvo/work-queue/items {title, body, category?}` → `{item}`; `PATCH kuvo/work-queue/items/<item> {title?, body?}` → `{item}`; feature `work-queue.edit`; 409/503 `{message, computer}` as a move has. The box decides whose queue a user's item goes in (the app sends no owner), what a title may be, and refuses an issue's (GitHub's). | `kuvo_box/src/work_queue.rs` (`WRITES`). The forms are built; shown only where the feature is named. |
| Done as a state, not a category (and "In progress", Parked: decisions below). | `state` on each item (`open`, `done`, more later) and `POST …/items/<item>/state`; or keep the move. | `kuvo/src/queue/mod.rs` (`DONE`). Done is the protocol's `done` category: folded, checked, and Mark Done is a move there. |
| No built-in categories: every one renamed and removed like any other, none made by default. | `builtIn` gone (or false for all) in `kuvo/work-queue/categories`; `{rename}` and `{remove}` taking any. | `kuvo/src/queue/page.rs` (`list_head`). Rename and Remove are offered for those the box says aren't `builtIn`. |
| A category's colour. | `color` on each of `categories` (one of the looks' names). | `kuvo/src/areas.rs` (`category_color`): the built-in four by id (KuvoClient's rule, there before), one colour for every custom one. |
| What may be done to each item, so nothing is offered that the box will refuse. | `can: {finish, move, edit, delete}` on each item of `kuvo/work-queue` and `…/items/<id>`. | `kuvo/src/queue/page.rs` (`move_targets`). Two things are drawn from the box's facts: `closesOn` hides Mark Done and Done as a target (as the protocol says); a deleted item is offered only Restore. Everything else is offered and the box's refusal drawn: Edit… on a GitHub issue, anything on an item whose computer is away or that's on its way to another. |
| A search of what items say and their notes. | `GET kuvo/work-queue?owner=all&q=<words>` → the same view, narrowed (with its counts). | `kuvo/src/queue/mod.rs` (`takes`). The search finds titles and numbers only (the list carries no descriptions). |

Not marked in code, for the box session to weigh with these:
- **More than one chip on an item** (decision below): `categories: [ids]` beside `category`. The row
  and the page draw one chip from `category` (or `doneAs`).
- **Counts for a narrowed view**: with the search or a filter on, the list shows no counts (the
  box counted the whole view; the app counts nothing).

## Review

One reviewer pass; the MUST and every SHOULD fixed, each with a scenario (written after the fix,
not run against the code before it):
- **MUST:** rules about what may be done to an item sat in the app unmarked (no move for one on
  its way elsewhere, none out of Done for a GitHub issue, no new item into Done). The two the
  protocol doesn't state are gone (the box refuses, in its words); what's left is in one place
  (`move_targets`, `can_finish`), marked `BOX-NEEDED` and listed above.
- A refusal that came after you'd gone to another item showed over that item's page: it shows
  over the list or its own item's page only
  (`live::a_refusal_that_comes_after_youve_gone_shows_over_no_other_items_page`).
- A change the box took, whose re-read then failed, drew the item back where it was: the sidebar's
  copy takes what the box answered first (`a_change_whose_reread_fails_keeps_what_the_box_answered`).
- Nothing ran the page the app's way: three `live::` scenarios do (an item read after you've gone
  on, the refusal above, a sheet's answer).
- Two assertions read the stand-in's counts, not what's drawn: the count drawn is asserted now, and
  with a box whose count differs from its list (`every_count_drawn_is_the_boxs_never_the_apps`).
- A sheet's save could be dropped by a later sheet's, and its answer taken by a sheet reopened:
  each opening has its own number, and its answer is its own (`live::a_sheets_answer_is_its_own_openings`).
- NITs fixed: a move to where an item already is asks nothing; Remove Category waits for its
  answer; a filter is the same filter whatever its computer is called now; the bar shows wherever
  the box gives a share; the old glance-only reader (`BoxClient::work_queue`) is gone; a test's
  name; the sentence for a box that takes no writing is also on the + itself.
- NITs left: leaving a page drops a sheet's typed text, unasked (Bots' too); an item whose
  category the box doesn't list isn't drawn under All (the protocol says the box prevents it); the
  GitHub filter takes `github-pr` too (the box counts sources by `github`); on an item's page no
  sidebar row is picked; in a narrow window the tabs are cut at the search with nothing saying
  they scroll.

## Left

- **Not built:** an item's activity feed (`events`, `handed`; the feed's order is KuvoClient's
  rule, not ported); opening or saving a file (`…/files/<name>`: rows show name and size only);
  reordering within a category and Move to Top (`before`, `after`, `top`); drag and drop; Import;
  handing an item on; the per-conversation queue (a session's own view, "Working on"); `#243`
  chips in conversations (`refs`); the note when another computer is away (`computers[].away`);
  `wanted` (an item waiting for its number shows none).
- **Not live:** the view is read as the area shows and after each change, not followed
  (`kuvo/work-queue/events`): an item at work elsewhere doesn't update on a page left open.
- **The rail's count** for the queue is the box's `kuvo/waiting` as before.
- **Keyboard:** no walk through the rows, no ⌘F for the search, no shortcut for New Item; Tab
  doesn't move between a sheet's fields (the text field has no tab stops yet).
- **Left in the client, view words:** the tabs' and rows' names, "In progress", "Done", "At work",
  "Added by … · Changed by …", "On <computer>" and its lines, "Nothing in … yet.", the foot's
  sentence on a box that takes no writing, "From GitHub" (a source's kind named).
- The search and the filters are view state in the app: they pick which of the box's items show.
- No VoiceOver labels (GPUI has none here yet); larger text not tried.
- The phone: no change (the GPUI app isn't shipped). Docs and CHANGELOG: none needed (nothing
  users run changes).

## Decisions for the user

Built without drafts (the lead's brief: build, with shots); each is cheap to change. Shots are in
`gpui/shots/`.

**#674: the open design points**
- **Can an item carry more than one chip?** Options: (a) one category, as now; (b) several, as
  labels (GitHub, Linear). Recommended: (b), with states apart (next point): then a chip only ever
  says what kind of work it is, and "where it stands" is never a chip. The row has room for two or
  three before they fold to "+2". Needs the box (`categories: [ids]`).
- **What Done, "In progress" and Parked become.** Recommended: states, not chips. Open → Done is a
  state you set (Mark Done); In progress stays what the box works out (a session at work), never
  set by hand; Parked becomes a state too ("Parked", folded like Done, as the reference folds
  Canceled). Then tabs are states (Open, In Progress, Parked, Done) and chips are categories, and
  the page already reads that way. The other way (all chips) loses "folded Done" unless one chip
  is special again.
- **How items in an old Next or Later move over.** Recommended: Next and Later stay as two
  ordinary categories with their items, renamed or removed like any other once empty; no item
  moves by itself. A box with no categories at all then shows one flat list (the page handles
  that: no headers). The other way (drop them, items left with no chip) loses the order people
  already keep.

**This piece**
- **Adding and editing from the app** (`screens-queue-new`, `-edit`) changes the protocol's rule
  that only agents write items. Recommended: yes, for title and description, with the box
  refusing an issue's. Until the box has it, the app shows neither (`screens-queue-read-only`).
- **The tabs repeat the sidebar's rows** (All, In Progress, each category): one scope, two places,
  so a narrow window (sidebar folded) keeps them (`screens-queue-narrow`). Recommended: keep until
  the states decision, then tabs become states and the sidebar keeps categories.
- **An item has a page**, opened from its row, with the way back; the Mac splits the panel.
  Recommended: keep.
- **Every group folds**; folded, it's a card (`screens-queue-done`). Done and Deleted start
  folded; searching or filtering unfolds everything.
- **The chip on a row repeats its group's header** while the list is grouped by category. Kept
  because #674 asks for a chip on each item, and it's what remains once grouping goes.
- **Delete isn't asked first** (it's kept under Deleted, with Restore). Recommended: keep.
- **Filters:** computer, source, owner; one of a kind widens, another kind narrows. Category isn't
  a filter chip (the tabs do it).
- **New Item is a + with a tooltip**, to leave the tabs room. Recommended: keep.
- **The foot's sentence on a box that takes no writing**, and all wording here: the user's to
  settle.
- **Category colours** are the four built-in ones' and one for all custom ones: a look question
  once categories are all the user's own.
