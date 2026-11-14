# The Work Queue: the queue, an item's page, and what you do to one

The Work Queue area's main view (⌃3). The idea is the Mac's (agents keep work items in queues, one
list across your computers; an item has a title, a description, progress, notes, files, links and
the sessions working on it; each stands in a status, with a priority, labels, someone it's for and a due day; you add them,
change them, close them, delete them and bring them back), drawn in this app's own language, not the Mac's views: where the Mac has a panel beside
a conversation with a split detail pane, here the queue is a page, and an item has a page of its
own. The look follows the reference the user gave (Capy's Issues: scope tabs, filters as chips to take
off, a line for an empty list, Done and Canceled folded away, a new issue written like a message
with chips under it, each opening a menu with a number beside every row). Code: `kuvo/src/queue/` (`mod.rs` the view,
what it reads and each change; `page.rs` the drawing; `picker.rs` the chips' menus; `marks.rs` the
status, priority, label and assignee marks), the sidebar in `areas.rs`;
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
  drawn from changes (the box's view, the scope, what a search found, a filter, a fold, an item
  busy) and spliced where it differs: a queue of thousands of items costs what's on screen, and the
  page keeps its scroll while an item changes. The list's head (tabs, search, chips) is outside the
  list, so it stays put.
- A status with nothing to list leaves no header; a folded one leaves only its header, drawn as a
  card of its own. What the box says is closed, and Deleted, start folded. While the search or a
  filter is on, nothing is folded, so a match in Done shows.
- The search leaves the thread: the box searches (`q`), since only it has what items say. A newer
  search replaces one on its way (`run` under one name), an answer is taken only for what's
  typed now, and a result is drawn only for the words it answered ("Searching for …" meanwhile). Escape clears it. Filters are chips, each taken off by its ×, or all by Clear.
- A chip's menu is the view's own (`picker.rs`), not the block's `Menu`: it has a line to type in,
  picks several (labels), and takes number keys. It's drawn under its chip, above everything, over
  a sheet that takes any press outside it (which closes the menu and does nothing more). Its rows
  are worked out as it draws, from the box's view and what's typed; a long list scrolls inside it.
- A read or a change under the same name replaces the one before it; an answer is its own item's,
  whatever shows by then.
- Sheets as Bots' are: the fields made once, the sheet open until the box answers, its refusal in
  its words, the keyboard back in the first field. An item's sheet sits near the top of the window,
  so its menus have room under them.
- **Where we differ:** no fuzzy matching or highlighted matches (the box's search; a plain
  "contains" in a menu's line); no keyboard walk through the rows yet; no window-wide modal layer.

## The app decides nothing

- The statuses, priorities, labels and who an item may be for are whatever the box lists, in its
  order; `closed` is the box's word for what's folded. Marks are drawn by id, a plain one for an
  id this doesn't know.
- Every count shown is the box's (`counts`): `open`, each status's, each label's, `removed`. A
  search's counts are the box's too, of what it found. Under a filter (the app's), no count shows.
- What may be done to an item is the box's (`can`): nothing it leaves out is offered, and the app
  works none of it out. What the box refuses anyway is drawn in its words, over the list or that
  item's own page.
- Late is the box's (`overdue`); the app compares no days. A change wanted while another is on
  its way waits behind it and goes when that one answers (one at a time for an item).
- After every change the view is read again (and searched again, where something is): an item's
  place, the counts and the labels are the box's.
- What a new item starts as is the box's (`new`): New Item shows it and sends it. Why something
  can't be done is the box's sentence (`can.why`), drawn as it is. Nothing is marked `BOX-NEEDED`.

## The views

- **The sidebar** (`areas.rs`): All Items, "Status" (the box's statuses and counts), "Labels" (+
  makes one; each with its count and, under the pointer, a ••• with Edit and Remove, as its
  right-click has; none says so).
- **The queue:** tabs (All, each open status; a closed status or a label while it shows); the
  search; Filter; ••• (New Label…, and for the label shown, Edit… and Remove…); + (New Item). Under
  them each status's items in a raised card, each row as above, with ••• (Mark Done, Mark Canceled,
  Move to an open status, Delete: what `can` allows). Deleted, folded, at the end, each with
  Restore.
- **An item's page:** the way back; what names it, whether a session is at work, Mark Done, Edit…
  and •••; its title; its chips (status, priority, assignee, due day, labels), each a menu where
  it may be set; notices (deleted, on its way to another computer, on another computer, and why
  something can't be done, in the box's words); progress; Description; Sessions; Links; Files; Notes; who added and
  changed it, and whose queue it's in.
- **Sheets:** New Item and Edit Item (a title, a description, a new item's chips, Create more,
  ⌘↩), New Label and Edit Label (a name, ten colours), Remove Label (asked).
