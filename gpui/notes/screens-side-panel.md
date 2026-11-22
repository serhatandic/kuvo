# The side panel (Zed's docks)

Examples: `shots/screens-changes-*`, `screens-file-*` and `screens-panel-narrow-*`. Older shots may show the earlier panel. Code: `kuvo/src/panel/mod.rs`,
the workspace's `panel_card`, `toggle_panel`, `resize_to`.

## What Zed does

Read: `crates/workspace/src/dock.rs` (its resize handle, `min_size`, how a dock's size is kept and
restored with the workspace). Behaviour only; nothing copied (GPL).

- A dock on either side holds panels; a toggle shows or hides it, and its size survives hiding.
- A thin handle (6 px) on its inner edge drags it; a double click puts it back to its default size.
- Each panel says its narrowest; the dock's size is saved with the workspace.

## What Kuvo takes

- Each conversation keeps its own right-panel tabs, including bot conversations.
  **+** opens New Tab, with file search, Files, a side chat and a terminal.
  Files open in their own tabs. Changes, Branches, Browser and conversation
  outputs open where their routes are supported. Terminal and browser sockets
  are available over local or paired relay connections.
- ⌥⌘0 toggles the panel. ⌥⌘I opens Changes and ⌥⌘F opens Files.
  The title-bar button also toggles it.
- Drag the divider to resize or double-click it to reset. The width stays
  while the panel is closed. In a narrow window the panel slides over the
  conversation; it can be closed to return to the conversation.
- Tools take their folder from the conversation's computer and worktree.
  File tools show a reason when no folder is available.

## Where we differ, and why

- One side, not docks on both: the sidebar is the left's.
- The panel width is held in workspace state for the lifetime of the window.
