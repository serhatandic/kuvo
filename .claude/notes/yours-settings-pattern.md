# Adding a page to Your settings (the desktop app)

Your settings are the desktop app's own, on one device (Settings' left side, "Your settings").
A computer's settings are its box's (`kuvo/settings/<id>`, drawn by `described.rs`): never put one of
those here. General, Appearance and Notifications are built this way (2026-11-16); Voice, Keyboard
Shortcuts, Side Panel and Privacy are to follow it (Updates: another session, with Sparkle).

## The pieces (all in `gpui/kuvo/src`)

1. **The list entry**: `settings/rows.rs`. Move the page's `listed(...)` line from `YOURS_LATER`
   to `YOURS` (keep its id, icon and search keywords; add words people would search for).
2. **The store keys**: `yours/mod.rs`. One typed struct per page, `#[serde(default)]`, with its
   defaults (a `Default` impl where a default isn't `false`/none). Add it as a field of `Yours`,
   read it in `Store::read` (`page(&value, "<id>")`) and write it in `Store::write` (the `pages`
   list): one JSON object per page, keyed by the page's id. Add a `Change` variant per setting and
   its line in `Change::apply`. The file is `~/.kuvo-code/desktop/settings.json`
   (`keeper::folder_of`); never 0.7's `~/Library/Application Support/run.kuvo.code`.
   A page that can't be read is its defaults; unknown keys are kept as they were.
3. **The rows**: `rows::yours`, a `"<id>" => ...` arm returning `Block`s. Use the row kinds
   (`Control::Switch`, `Menu`, `Buttons`, `Field`…), never hand-drawn elements, so the page restyle
   (#760) applies. A switch: `yours_switch(now.x, Change::X)`. A menu: `Act::Yours(Change::…)` per
   choice. Something only the system opens: `Act::Open(url)`. What the page needs beyond your
   settings (a list read from the system, like General's editors) goes in `YoursFacts`, filled by
   `SettingsView` (read in `new`/`shown` when `live`; a `set_…` for tests and shots).
   Words: few; a description only where the title isn't enough; never "box"; "Kuvo", "this Mac".
4. **The behaviour hook**: whatever the setting changes reads `yours::now(cx)` when it acts, and
   where it must react at once, `cx.observe_global::<yours::Store>(…)` (see
   `notifications.rs`'s Dock count, `panel/files.rs`'s editor). What only the OS does goes behind
   a function in `yours::desk`/`yours/mac.rs` (non-mac: a no-op or `None`), and through an
   injectable `Effects` where tests must see it without touching the real system (notifications
   are posted by `mac::post` with macOS's own sound, so Focus and System Settings silence it).
5. **The tests**: `kuvo/tests/yours.rs`. Each test calls `yours::init(Some(<temp folder>), cx)`;
   click the row through the window (`row-<x>-switch`, `row-<x>-menu` then `menu-item-<label>`,
   `row-<x>-<button>`), assert what the app then does, and `Store::read(path)` for the next run.
   Store unit tests (defaults, a bad file, unknown keys) are in `yours/mod.rs`.
6. **Shots**: a `settings-yours-<id>` screen in `kuvo/src/bin/shots.rs`
   (`../scripts/cargo-locked.sh run -p kuvo --features shots --bin shots -- settings-yours-<id>`), light and dark.
7. **Docs and changelog**: kuvo-web `src/content/docs/code/settings.md` (Your settings), and a
   CHANGELOG line.

## Drafts picked for General's editor (the user was away; the Foreman asked us to pick)

- A (built): "Open in <Editor>" as a small ghost text button with an external-link icon at the right
  of the file's header in Files (Codex's "Open in" sits top-right of its file view; Zed's "Open
  in…" is a header action too). With no editor picked, "Open", in the Mac's default text editor
  (`open -t`), never the app a file's type names (an agent's `.command` would run), with a tooltip
  saying where to pick another.
- B (not built): an icon-only button there opening a menu of the installed editors. Fewer words,
  but it hides which app opens and adds a menu to every open.
- Row: a menu, "Open code and text in", "Default text editor" first, then the editors found on
  this Mac (0.7's list of bundle ids, `yours/editors.rs`).
- Only a file on this Mac's computer has the button (`client.here()`); another computer's would
  need a copy fetched first (`// BOX-NEEDED` in `panel/files.rs`).

## Not ported from 0.7's General, and why

- Show the tab bar, Sessions side by side: the desktop app has no tab bar and no split.
- Swipe between computers: the desktop app has no swipe gesture.
- Keep this Mac awake: a computer's setting (the box's This Mac page, audit G1), not Yours.
- Show in the menu bar (G2), Send with ⌘Return (G7), Notifications' "which device" (G3): wait for
  the user's pick.
