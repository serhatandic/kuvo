# Computers: handover

## Done

The Computers area (`kuvo/src/computers.rs`, the sidebar in `kuvo/src/areas.rs`), drawn from
`kuvo/computers` alone (PROTOCOL.md, "Computers"):

1. **The sidebar**: All Computers (the area's own page), Your Computers, each computer with its
   tile, a dot for how it stands (green connected, amber connecting or older, red away, faint known only by another's word), its line and what's at work there; Add
   a Machine… under them; Shared Machines apart.
2. **All Computers**: each as a row with how it stands, its system, what's at work and its Kuvo Code
   version; Add a Machine…; a line for adding one where this computer is alone.
3. **A computer's page**: its tile, name and "state · system"; a notice for why it stands as it
   does (can't be reached, connecting, known only through another, shared with another setup, or
   older, in the window's own held words, `workspace::held_words`); At Work (the box's count; Show
   in Sessions for this window's own); About (kind, system, Kuvo Code, engine, how it's
   reached, plan limits it reads); Manage (Rename…, Connect for one that's away, Remove… for
   another), drawn and disabled, with a line saying they aren't in this app yet.
4. **Add a Machine**: what a machine is, the SSH host field (taking no typing yet), and a notice
   that adding isn't in this app yet; Add Machine waits (sends nothing).
5. Stand-in: `computers_alone()`, `more_computers(n)`, and old-mini (a Mac on 0.9.180) in its list.
6. Tests: `kuvo/tests/computers.rs` (6 scenarios, real clicks and a scroll wheel), unit tests in
   `computers.rs` and `kuvo_box/src/areas.rs`. Shots: `shots/screens-computers*`, `screens-computer-*`.

## BOX-NEEDED

1. **`features` on each computer in `kuvo/computers`**, proposed for PROTOCOL.md "Computers":
   "`features?`: the names its box's `kuvo/info` lists, as it said them with its agents
   (`agents.computer`, "Peers"); left out for one that hasn't said". `Computer::features` /
   `is_older()` read it and apply the update gate's own list (`gate::missing`, as for the window's
   box); the stand-in says it for old-mini alone (marked STAND-IN). With no box saying it, no
   computer is called older. Two things for the box session: peers keep only the first 300
   feature names ("Peers"; 171 today), so past that a box would wrongly look older, and the field
   needs the whole list or a word saying it's cut; and the phone's Computers view needs the same
   `BoxGate` call once the box sends it.
2. **Which sessions are at work**: a per-session `atWork` fact on `kuvo/sessions/list`, or their
   ids on the `here` computer of `kuvo/computers`, so a computer's page can list them. Until then
   the page draws the box's count (`working`), with Show in Sessions for this window's own; no
   copy of the box's rule is kept in the app.
3. **`working` left out for a peer that never said it** (an older box is listed with `0`), so
   "Nothing is at work" isn't claimed of it. Meanwhile an older computer's page draws no count.
4. **Rename, connect again, remove, on every box**, offered per computer (`can` on
   `kuvo/computers`, as the Work Queue's items say it). `kuvo/machines…` is a Mac's app's own and
   answers only while it's open, so the desktop app can't use it.
5. **Adding a machine from a box** (#636): the SSH setup steps (`POST kuvo/machines {destination}`
   answered by the box, with its `step`s as it goes) and the hosts to suggest (`hosts`, from that
   box's `~/.ssh/config`). The form's field takes no typing until then.

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
