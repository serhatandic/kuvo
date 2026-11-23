# Adding a page to Your settings

Your settings are the desktop app's own, on one device (Settings' left side, "Your settings"). A
computer's settings belong to its box (`kuvo/settings/<id>`, drawn by `described.rs`): never put one
of those here. All paths are in `gpui/kuvo/src`.

1. **The list entry**: `settings/rows.rs`. Move the page's `listed(...)` line from `YOURS_LATER` to
   `YOURS` (keep its id, icon and search keywords; add words people would search for).
2. **The store keys**: `yours/mod.rs`. One typed struct per page, `#[serde(default)]`, with its
   defaults. Add it as a field of `Yours`, read it in `Store::read` (`page(&value, "<id>")`) and
   write it in `Store::write` (the `pages` list): one JSON object per page, keyed by its id. Add a
   `Change` variant per setting and its line in `Change::apply`. The file is
   `~/.kuvo-code/desktop/settings.json`. A page that can't be read is its defaults; unknown keys are
   kept as they were.
3. **The rows**: `rows::yours`, a `"<id>" => ...` arm returning `Block`s. Use the row kinds
   (`Control::Switch`, `Menu`, `Buttons`, `Field`…), never hand-drawn elements. A switch:
   `yours_switch(now.x, Change::X)`. A menu: `Act::Yours(Change::…)` per choice. Something only the
   system opens: `Act::Open(url)`. What the page needs beyond your settings (a list read from the
   system) goes in `YoursFacts`, filled by `SettingsView`, with a `set_…` for tests and shots.
   Few words; a description only where the title isn't enough; never "box".
4. **The behaviour**: whatever the setting changes reads `yours::now(cx)` when it acts, and where it
   must react at once, `cx.observe_global::<yours::Store>(…)` (see `notifications.rs`,
   `panel/files.rs`). What only the operating system does goes behind a function in `yours::desk` /
   `yours/mac.rs` (elsewhere a no-op or `None`), through an injectable `Effects` where tests must see
   it without touching the real system.
5. **The tests**: `kuvo/tests/yours.rs`. Each test calls `yours::init(Some(<temp folder>), cx)`,
   clicks the row through the window (`row-<x>-switch`, `row-<x>-menu` then `menu-item-<label>`,
   `row-<x>-<button>`), asserts what the app then does, and reads `Store::read(path)` for the next
   run. Store unit tests are in `yours/mod.rs`.
6. **Shots**: a `settings-yours-<id>` screen in `kuvo/src/bin/shots.rs`
   (`../scripts/cargo-locked.sh run -p kuvo --features shots --bin shots -- settings-yours-<id>`),
   light and dark.
7. **Changelog**: a line under Unreleased.
