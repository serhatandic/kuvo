> Historical, superseded by [How kuvo works](../architecture/how-kuvo-works.md), kept for its findings.

# Computers: handover

## Done

The Computers area (`kuvo/src/computers.rs`, the sidebar in `kuvo/src/areas.rs`), drawn from
`kuvo/computers` alone (PROTOCOL.md, "Computers"):

1. **The sidebar**: All Computers (the area's own page), Your Computers, each computer with its
   tile, a dot for how it stands (green connected, amber connecting or older, red away), its line and what's at work there; Add
   a Machine… under them; Shared Machines apart.
2. **All Computers**: each as a row with how it stands, its system, what's at work and its Kuvo Code
   version; Add a Machine…; a line for adding one where this computer is alone.
3. **A computer's page**: its tile, name and "state · system"; a notice for why it stands as it
   does (can't be reached, connecting, or
   older, in the window's own held words, `workspace::held_words`); At Work (the box's count; Show
   in Sessions for this window's own); About (kind, system, Kuvo Code, engine, how it's
   reached, plan limits it reads); Manage (Rename…, Connect for one that's away, Remove… for
   another), drawn and disabled, with a line saying they aren't in this app yet.
4. **Add a Machine** (#636): the box adds it (`kuvo/computers/add`, PROTOCOL.md "Adding one"); the
   page takes the address, offers the hosts the box suggests (filtered by what's typed: drawing),
   and draws the box's try: its steps, its question (a host key's fingerprint, in the code font,
   with the box's buttons), a failure with Try Again and Dismiss, and done with Show. While one is
   being added its steps are the page; the form comes back after. A computer's page draws the try
   about it (set up again, uninstall) and Manage's Set Up Again and Uninstall… as the box offers
   them. From Zed's remote dialog (read, not copied): one status per phase, the host's question
   shown as it is, Cancel always there, retry on failure; ours is a page, keeps the steps listed,
   and asks with buttons instead of "type yes".
5. Stand-in: `computers_alone()`, `more_computers(n)`, `manage_refused(words)`, and old-mini (a Mac on 0.9.180) in its list; it answers the manage routes as a box does.
6. Tests: `kuvo/tests/computers.rs` (6 scenarios, real clicks and a scroll wheel), unit tests in
   `computers.rs` and `kuvo_box/src/areas.rs`. Shots: `shots/screens-computers*`, `screens-computer-*`.

## From the box (#735)

1. **Done:** `features` on each computer (whole, or left out), `working` left out where never
   said, `atWork` (which sessions), and `actions` (`rename`, `connect`, `remove`, with the box's
   confirm), done by any box (PROTOCOL.md, "Computers", Managing them). The page draws Manage from
   `actions`: Connect and Remove… work (Remove asks with the box's words, as Settings' confirm);
   a refusal shows in the box's words on the page it was asked on.
2. **Waiting on a design choice (drafts for the user):** the Rename editor (Rename… is drawn,
   not done) and listing `atWork`'s sessions under At Work (the count is drawn as before).

## Not built (the app's side)

- Opening another computer's sessions: the window shows one box; switching to another
  (`BoxClient::machine`) isn't built, as the title bar's switcher says.
- The sidebar's list isn't virtualized (the page is): fine for tens of computers.

## Decisions for the user

- The layout (All Computers as the area's page, a page per computer, Add a Machine as a page like
  New Bot): built as recommended; shots in `shots/`. All Computers repeats the sidebar's list,
  adding each one's state with its system, what's at work and its Kuvo Code version (a table to
  compare them by). Alternatives: open the area on this computer's page, or Add a Machine as a
  sheet. Recommend keeping All Computers: it's where Add a Machine is plainest for a first run.
- Manage's disabled buttons versus leaving them out until a box offers them: drawn, so the
  feature's place is seen; recommend keeping them.
