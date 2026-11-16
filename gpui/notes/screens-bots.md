# Bots: a bot's page, New Bot, changing and deleting one

The Bots area's main view (⌃1). The idea is the Mac's (a bot has a name, a look, a job, a
personality and a model, its own conversation and the sessions it starts), drawn in this app's own
language, not the Mac's views: where the Mac opens a bot's conversation and keeps everything else in
one Edit Bot form with Save, here a bot has a page. Code: `kuvo/src/bots/` (`mod.rs` the view, what
it reads and each change; `page.rs` the drawing), the sidebar in `areas.rs`, a bot's conversation in
`workspace.rs`; `kuvo_box/src/bots.rs`. Scenarios: `kuvo/tests/bots.rs`. Shots:
`shots/screens-bot-*`.

## What Zed does (studied for behaviour and speed, nothing copied)

Read (GPL-3.0, behaviour only): `crates/workspace/src/modal_layer.rs` (`ModalLayer`,
`toggle_modal`, `hide_modal`, `on_before_dismiss`), `crates/ui/src/components/modal.rs`,
`crates/settings_ui/src/components/input_field.rs` (`SettingsInputField`) and `dropdown.rs`,
`crates/settings_ui/src/pages/llm_providers_page.rs` (`LlmProviderForm`,
`save_llm_provider_form`), `crates/agent_ui/src/agent_configuration/
configure_context_server_modal.rs` and `manage_profiles_modal.rs`,
`crates/agent_ui/src/threads_archive_view.rs` (`update_items`), `crates/project_panel` (`remove`)
and `crates/gpui/src/platform.rs` (`PromptButton`).

- **Modals.** What had the keyboard is remembered and gets it back; a layer under the modal takes
  every click; Escape is the modal's own to handle; a modal with work under way refuses to close.
- **Forms.** One setting in a row saves by itself (Enter, or leaving the field); a form of several
  fields has one Save, reads its fields then, and shows the first error above its buttons. A modal
  that waits on something disables its button and ignores a second press; the settings forms don't,
  and can save twice.
- **Fields are made once** and kept in the view; render only shows them. A menu's entries are
  built in a closure that runs as it opens.
- **Destructive things** ask first (the confirm button named for what it does, then Cancel;
  Escape cancels).
- **A list of past items** is a flat vector rebuilt when its source changes, drawn through a
  virtual `list`; a short "recent" section is a plain column.

## What Kuvo takes

- A bot's page is a `gpui::list` over a flat `Vec<Item>` (its head, notices, its sessions, its
  settings rows), worked out in `rebuild` only when what it's drawn from changes and spliced where
  it differs, as Settings' pages are: a bot with hundreds of sessions costs what's on screen, and
  the page keeps its scroll while a row saves.
- One setting, one change: Personality and Model are menus that send as they're picked, the row
  busy meanwhile; Name, Job and Look open a sheet with Save, which stays open with the box's words
  if it's turned down. A sheet with a change on its way ignores a second Save and doesn't close.
  Escape closes a sheet; the page and the sidebar wait dimmed under it; the keyboard goes to the
  sheet's first field and back to the page.
- The sheets' fields are made once with the view. Menus are built as they open.
- Delete asks first, its button named "Delete Bot"; Return doesn't delete.
- The page reads only as it's shown (`kuvo/work?bot=`, `kuvo/bots/templates`, the models
  offered); nothing is asked for a bot that isn't on screen.
- **Where we differ:** no window-wide modal layer yet (the sheet is the page's own, as Settings');
  a refusal of a menu's change shows over the page, not in the row.

## The views

- **The sidebar** (`areas.rs`): "Bots" with + (New Bot); the box's bots; "On Other Computers", the
  bots the box holds an edit for (`held`) or heard a refusal about (`refused`); Group Chats.
- **A bot's page:** its still face (64 px), name, job and what it's doing now (`now`, in the
  sidebar's words) with Chat; what's held for it and what was refused; Recent sessions
  (`kuvo/work?bot=`, the box's order, six then "Show all"), each with its state as the box says it
  and opening in Sessions; Settings (Name, Look, Job, Personality, Model, Lives on, Works in);
  Delete Bot.
- **Its conversation:** the same conversation view and composer as a session's, in the page's
  place, under a strip with the way back; its row stays picked. Back and forward know both.
- **New Bot:** the box's templates as tiles, Blank first; Name, Look, Personality, Model and Works
  in, each from what the box offers; only what was picked is sent (the rest is the box's to
  choose), with one `request` id for the form so a retry makes the bot once. Then its conversation,
  the template's job in the composer to edit and send.
- **A bot on another computer:** what's known of it here (the name being given, that computer's
  name), "Waiting for <computer>" with the fields that wait, or the computer's refusal in the box's
  words.

No logic of the box's is here: what a bot may be called, which looks, personalities and models
there are, whether an edit is done or held, and every refusal's words are the box's. The app's own
words are the row names and lines, "N at work…" (the sidebar's, from `now`), a session's state word
from the box's `state`, and "Waiting for <computer>".
