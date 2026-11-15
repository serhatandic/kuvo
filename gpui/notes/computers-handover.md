# Computers: handover

## Done

The Computers area (`kuvo/src/computers.rs`, the sidebar in `kuvo/src/areas.rs`), drawn from
`kuvo/computers` alone (PROTOCOL.md, "Computers"):

1. **The sidebar**: All Computers (the area's own page), Your Computers (+ under the pointer to add
   one), each computer with its tile, a dot for how it stands (green connected, amber connecting
   or older, red away, faint known only by another's word), its line and what's at work there; Add
   a Machine… under them; Shared Machines apart.
2. **All Computers**: each as a row with how it stands, its system, what's at work and its Kuvo Code
   version; Add a Machine…; a line for adding one where this computer is alone.
3. **A computer's page**: its tile, name and "state · system"; a notice for why it stands as it
   does (can't be reached, connecting, known only through another, shared with another setup, or
   older, in the window's own held words, `workspace::held_words`); At Work (this window's own
   sessions, which open; another's counted); About (kind, system, Kuvo Code, engine, how it's
   reached, plan limits read there); Manage (Rename…, Connect for one that's away, Remove… for
   another), drawn and disabled, with a line saying they aren't in this app yet.
4. **Add a Machine**: what a machine is, the SSH host field, how it's set up, and a notice that
   adding isn't in this app yet; Add Machine waits (sends nothing).
5. Stand-in: `computers_alone()`, `more_computers(n)`, and old-mini (a Mac on 0.9.180) in its list.
6. Tests: `kuvo/tests/computers.rs` (6 scenarios, real clicks and a scroll wheel), unit tests in
   `computers.rs` and `kuvo_box/src/areas.rs`. Shots: `shots/screens-computers*`, `screens-computer-*`.

## BOX-NEEDED

1. **`features` on each computer in `kuvo/computers`** (what its box has, as it said them with
   its agents), so a client can tell which of your computers it would hold. `Computer::features` /
   `is_older()` read it; the stand-in says it for old-mini alone (marked STAND-IN). With no box
   saying it, no computer is called older.
2. **Rename, connect again, remove, on every box**, offered per computer (`can` on
   `kuvo/computers`, as the Work Queue's items say it). `kuvo/machines…` is a Mac's app's own and
   answers only while it's open, so the desktop app can't use it.
3. **Adding a machine from a box** (#636): the SSH setup steps (`POST kuvo/machines {destination}`
   answered by the box, with its `step`s as it goes) and the hosts to suggest (`hosts`, from that
   box's `~/.ssh/config`).

## Not built (the app's side)

- Opening another computer's sessions: the window shows one box; switching to another
  (`BoxClient::machine`) isn't built, as the title bar's switcher says.
- The sidebar's list isn't virtualized (the page is): fine for tens of computers.

## Decisions for the user

- The layout (All Computers as the area's page, a page per computer, Add a Machine as a page like
  New Bot): built as recommended; shots in `shots/`. Alternatives: open on this computer's page,
  or Add a Machine as a sheet.
- Manage's disabled buttons versus leaving them out until a box offers them: drawn, so the
  feature's place is seen; recommend keeping them.
