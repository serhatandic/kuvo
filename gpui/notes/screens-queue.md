# The Work Queue: the queue, an item's page, and what you do to one

The Work Queue area's main view (⌃3). The idea is the Mac's (agents keep work items in queues, one
list across your computers; an item has a title, a description, progress, notes, files, links and
the sessions working on it; you move items between categories, finish them, delete them and bring
them back), drawn in this app's own language, not the Mac's views: where the Mac has a panel beside
a conversation with a split detail pane, here the queue is a page, and an item has a page of its
own. The look follows the reference the user gave (an issues page: scope tabs, filters as chips to
take off, a line for an empty list, Done folded away). Code: `kuvo/src/queue/` (`mod.rs` the view,
what it reads and each change; `page.rs` the drawing), the sidebar in `areas.rs`;
`kuvo_box/src/work_queue.rs`. Scenarios: `kuvo/tests/queue.rs`. Shots: `shots/screens-queue-*`.

## What Zed does (studied for behaviour and speed, nothing copied)

Read (GPL-3.0, behaviour only): `crates/agent_ui/src/threads_archive_view.rs` (`update_items`: the
filter and the buckets), `crates/git_ui/src/git_panel.rs` (`update_visible_entries`,
`collapsed_sections`), `crates/picker/src/picker.rs` (`update_matches`, `pending_update_matches`),
and what the Bots note read of modals and forms (`notes/screens-bots.md`).

- **A filtered list is rebuilt, not hidden.** When the query or its source changes, one pass makes
  a flat vector of headers and entries (a header pushed as its group's first match comes by), and a
  virtual list draws only what's on screen. Groups with nothing in them leave no header.
- **Folding is the view's own state**, a set of folded sections consulted in that same pass; rows
  of a folded section aren't in the vector at all.
- **Typing filters at once** on what's already in memory (titles), lower-cased; work that leaves
  the thread (a picker's delegate) replaces the one before it, so an older answer never lands on a
  newer query.
- **A filter that leaves nothing** falls back to something useful rather than stranding you on an
  empty list with no way out.

## What Kuvo takes

- The page is a `gpui::list` over a flat `Vec<Row>` worked out in `rebuild` only when what it's
  drawn from changes (the box's view, the scope, the search, a filter, a fold, an item busy) and
  spliced where it differs: a queue of thousands of items costs what's on screen, and the page
  keeps its scroll while an item changes. The list's head (tabs, search, chips) is outside the
  list, so it stays put.
- A category with nothing to list leaves no header; a folded one leaves only its header, drawn as
  a card of its own. Done and Deleted start folded. While the search or a filter is on, nothing is
  folded, so a match in Done shows.
- The search filters as it's typed, on the titles and numbers the box's view already holds; Escape
  clears it. Filters are chips, each taken off by its ×, or all by Clear, so a list narrowed to
  nothing always shows the way out beside "Nothing matches."
- A read or a change under the same name replaces the one before it (`run`); an answer is its own
  item's, whatever shows by then.
- Sheets as Bots' are: the fields made once, the sheet open until the box answers, its refusal in
  its words above the buttons, the keyboard back in the first field.
- **Where we differ:** no fuzzy matching or highlighted matches (a plain "contains", lower-cased);
  no keyboard walk through the rows yet; no window-wide modal layer.

## The app decides nothing

- The categories are whatever the box lists, in its order; nothing is assumed of them but the one
  the protocol keeps finished items in (`DONE`, marked `BOX-NEEDED`).
- Every count shown is the box's (`counts`, #711): tabs, the sidebar, a group's header. Narrowed by
  the search or a filter, the list shows no counts, since the box counted the whole view.
- Search and filters only choose which of the box's items show (as the sidebar's search of loaded
  rows does); they change nothing and ask nothing.
- What may be done to an item is the box's to say: `closesOn` hides Mark Done and Done as a place
  to move to; everything else is offered and the box's refusal is drawn in its words.
- After every change the view is read again: an item's place, the counts and the categories are
  the box's.

## The views

- **The sidebar** (`areas.rs`): All Items, In Progress, then "Categories" (+ makes one) with the
  box's categories and counts.
- **The queue:** tabs (All, In Progress, each category) that pick the same scope as the sidebar's
  rows; the search; Filter (the computers that told of their queues, the outside sources and the
  owners the box's view names); ••• (New Category…, and for one you made, Rename… and Remove); +
  (New Item) where the box takes it. Under them each category's items in a raised card: how it
  stands (a ring, a spinner at work, a check done), `#n` or an issue's own name, the title (struck
  where the box says `struck`), what's being done on it now, a bar where the box gave a share, the
  computer it's on, its category's chip, when it last changed, and ••• (Mark Done, Move to…,
  Delete). Deleted, folded, at the end, each with Restore.
- **An item's page:** the way back to the list it came from; what names it and how it stands, Mark
  Done, Edit… and •••; its title; its category as a menu that moves it; notices (deleted, on its way
  to another computer, on another computer and whether it was read there or is as last told);
  progress; Description (Markdown); Sessions (each opens where sessions are); Links (opened in the
  browser); Files; Notes, the newest first; who added and changed it, and whose queue it's in.
- **Sheets:** New Item (title, description, category), Edit Item (title, description), New
  Category and Rename (a name).
