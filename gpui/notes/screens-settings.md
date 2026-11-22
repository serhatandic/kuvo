# Settings

The Settings area (⌃5), in two parts with a sidebar each (the user's pick of the #727 drafts, A,
2026-11-15; the note and drafts are on `kuvo/727-settings-split-design-and-drafts`,
`gpui/notes/settings-split.md`):

- **Your settings** keeps device controls for General, Appearance, Notifications,
  Keyboard Shortcuts, Side Panel, Privacy and Updates. Updates shows Sparkle
  controls and a separate check for releases for your computers. Voice has a
  placeholder page.
- **A computer's settings** belong to its box. The Settings picker changes
  only which computer's settings are shown. Connected boxes supply described
  pages from `kuvo/settings`. Accounts, Models and Usage still draw from
  their dedicated box routes. The approved delivery design proposes moving
  per-computer settings under Computers.
- The **foot row** goes to the other side ("Computer settings · This Mac ›" / "Your settings ›"),
  at the section last open there. The **search** finds on its own side, and lists what it finds on
  the other under "Computer · This Mac" / "Your settings", with an arrow; picking one goes there
  and leaves the search behind.
- Each page says once, under its title, whose it is. The open section is under the theme's selected
  wash, not the raised card the other sidebars use (#723).
- A computer that is connecting, away, or unable to serve a page shows its
  reach or compatibility state. Reachable computers open their available pages.

Places have keys (`yours/<id>`, `box/<computer key>/<id>`; a section's name alone is the computer
shown's), which is what back and forward keep.

The page layout is the user's OpenBot reference: a big page title, quiet group labels, one rounded
card of rows a group with hairlines between rows, each row's name (medium) and grey line on the
left and its control on the right, generous spacing, on the sidebar's soft grey.
Code: `kuvo/src/settings/` (`mod.rs` the view, where it is and its changes; `described.rs` a page
the box describes as rows; `rows.rs` the section lists and this app's own pages; `page.rs` the
drawing; `sign_in.rs` the sign-in sheet); `kuvo_box/src/settings_pages.rs`, `accounts.rs`,
`settings.rs`. Scenarios: `kuvo/tests/settings.rs`. Shots: `shots/screens-settings-a-*` and
`shots/screens-sign-in-*`.

## A page the box describes

One renderer for every kind of row (`described.rs`), in the box's words; nothing in the app knows
what a setting is. `switch` a switch; `choice` a menu (its choices' lines beside them; a long one
scrolls); `text`, `number`, `folder` a field saved with Return or Save (a folder is typed for now);
`secret` a masked field, or Replace and Remove once one is set; `status` words; `actions` buttons
(a menu past two), one with `opens` opened here; `list` its items as rows (naming the card where it's
alone in a group with no heading), or its `empty` words; `meter` usage windows; `open` a row with ›;
`steps` and `custom` say they aren't in the desktop app yet (or the box's why); a kind this app
doesn't know says a newer Kuvo changes it. A row or an action with `can: {why}` is drawn, can't be
pressed, and says why under the row. A `confirm` is asked first in the box's words. Every change
carries the row's `revision`; a stale one shows the box's words over the page and the page as the
box has it now. The page open is followed (`…/events`) where the box streams.

## What Zed does (studied for behaviour and speed, nothing copied)

Read (GPL-3.0, behaviour only): `crates/settings_ui/src/settings_ui.rs` (`SettingsWindow`,
`render_nav`, `render_page`, `reset_list_state`, `update_matches`, `build_filter_table`,
`filter_matches_to_file`), `crates/settings_ui/src/pages/llm_providers_page.rs`
(`render_provider_section`, `render_api_key_providers_item`, `get_or_create_configuration_view`).

- **The nav is a `uniform_list`, the page a `list` with a `ListState`.** The page's items (its
  title, each section header, each setting, a spacer at the end) are a flat index, and only those
  on screen are laid out. The list is `reset` only when the number of visible items changes (a
  page switch, a search), and `remeasure`d when the font size changes; nothing is rebuilt in
  render.
- **Search is a filter table.** One bool per item per page, worked out off the main thread
  (fuzzy, on a background task), then the nav shows pages with any match and the page only the
  matching items; a header goes with its items.
- **Views made once.** A provider's configuration view (its key field and its state) is created
  the first time it's drawn and kept, so typing in it never rebuilds it.
- **A provider's key.** "API Key Configured" with Reset Key once there's one; one from an
  environment variable can't be reset here and says which variable; otherwise a key field, where
  to get one, and "Or set the … env var".

## What Kuvo takes

- The page is a `gpui::list` over a flat `Vec<Item>` (title, group labels, rows, notes), worked out
  in `rebuild` only when what it's drawn from changes (a reading, the search, a change on its way,
  the appearance) and compared with the last: only the run of items that differ is `splice`d, so
  the rest keep their measured heights and the page keeps its scroll (`ListState::reset` would take
  it to the top on every busy row). Accounts' 20-odd rows, or a long list of providers, cost only
  those on screen.
- Search filters sections and rows by name and line, case-insensitively (the list is short, so
  no background task), and a section by the box's keywords too; a section's name keeps its whole
  page; a page with nothing found shows the first on its side that has something, again as each
  page's reading comes in. The first search reads every page of the computer shown, so it can find
  on all of them, from either side.
- A sheet (a sign-in, a confirm) dims the page and the Settings sidebar, takes the keyboard, and
  Escape closes it.
- Each page reads the box only as it opens (and again each time), each change goes to the box's
  route with the row busy meanwhile, and the box's answer is what's drawn after; a refusal shows
  over the page in the box's words. The sign-in sheet is made once per sign-in and keeps its fields.
- A key's field is masked (dots, never copied or cut): a new `TextField::masked()`.
- Where we differ: no "Reset Key" in place (Disconnect forgets a plan's keys and sign-ins, as the
  Mac does); a provider with only an environment variable says to set it, as the box's `environment`
  lists it.

## Accounts

The box's facts, drawn: the coding agents' accounts (`kuvo/agents/api/integration`), then the
engine's plans and providers signed in (or ended), then the rest to add, each in its side's order,
with the box's ways in (`kuvo/sign-in/accounts`). Only those the box says Accounts lists
(`listed`) show, each with the box's mark (`service`; its first letter where it names none).
What a row says and offers is the box's too (`state`, `line`, `actions`, #668): its line under the
name, a warning where its state needs you (`ended`, `problem`), its buttons with the box's words,
and a confirm in the box's words where an action has one. This app keeps no rule for when Sign In,
Sign In Again, Install, Disconnect, Sign Out or Remove Account show. Only the layout is its own:
signed in, the actions are in a menu (a lone one that asks first stays a button, Disconnect);
otherwise buttons, in the box's order; an action that only works on the box's computer (`here`)
is drawn only there. In a menu, a row that opens something first ends in an ellipsis, drawn here.
Not drawn yet (no control for them): a program's `update` and `rename`. Coding agents’ `addAnother` creates a separate account, then opens its sign-in.

The sheet (the Mac's `ConnectView` steps): the box's ways in order, legacy ones under "Other ways
to sign in", "Or set … in your shell profile"; one way alone starts at once. A key: masked field,
the way's form, Connect, and the engine's refusal under the field. Credentials on the computer: the
form, then connect. In the browser (started through the box, `kuvo/sign-in/start`): the page
opens, the device code the box says apart (`userCode`) is shown and copied, "Paste a
Code" where it may end on one (`acceptsCode`), the status asked about once a second
(`kuvo/sign-in/status`) until it's done or the box's `failure` words; Cancel gives it up on the
engine; ten unanswered asks give up.
