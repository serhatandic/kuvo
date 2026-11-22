# The composer (Zed's message editor)

Shots: `shots/screens-slash-*` ("/" offering what the box offers), `screens-conversation-*` (Stop
while a run is under way, and the queue over the composer), `screens-attached-*` (a picture and a
file attached), `screens-files-*` ("@"), `screens-agents-*` ("&").

## What Zed does

Read: `crates/agent_ui/src/message_editor.rs`, `crates/agent_ui/src/completion_provider.rs`,
`crates/agent_ui/src/agent_model_selector.rs`, `language_model_selector.rs`, `mode_selector.rs`,
`profile_selector.rs`, the composer in `thread_view.rs`, `assets/keymaps/default-macos.json`; for
phase 3, `crates/agent_ui/src/mention_set.rs`, `completion_provider.rs` (`completion_for_path`,
`completion_for_thread`), `conversation_view/message_queue.rs` and `thread_view.rs`'s
`render_message_queue_summary` and `render_message_queue_entries`. Behaviour only.

- A field that grows from one line, with a row of chips under it: the model (with its provider's
  icon) and the thinking effort as small borderless buttons with a chevron, each opening a menu.
- Send is a filled icon button: muted while empty, accent with text; while it generates with the
  field empty it's Stop. Esc stops.
- Return sends, Shift-Return adds a line.
- "/" opens a completion list of commands with their descriptions; a pick fills the field.
- Sending while it runs queues the message (or steers, by choice). The queue shows over the editor
  ("2 Queued Messages", folding open), each entry with its dot (accent for the next), Edit (moves
  it back into the editor), Steer, and Remove; Clear All.
- "@" opens a completion list of files (and threads, symbols, fetches) matched fuzzily; a pick
  inserts a mention that renders as a crease. Images paste or drop in as attachments.

## What Kuvo takes

- The text field block, bare in a card, growing to ten lines; Return sends, Shift-Return adds a
  line.
- Plain messages go to the box's send route. Slash and shell text go through
  the typed route, which returns what happened or what needs correction.
  Side questions and follow-ups open in side chats. Handoff can continue the
  conversation in another agent's session. The shared event line and any
  composer removals are follow-ups to [message delivery](../../docs/architecture/message-delivery.md).
- "/" lists what the box offers there (`GET …/typed`), names starting with what's typed first,
  then those containing it, each by name (as KuvoClient's `SlashCommands.matches`), scrolling
  past eight: ↑↓
  move, Tab or Return takes it ("/review "), Esc puts the list away. A keyword the text starts
  with shows as a chip above the field ("/plan"), as the sent message shows it.
- The model and effort menus draw the box's choices, with search and
  account selection for agents that have multiple sign-ins. A model on
  the same side is picked in place. Switching sides hands the conversation
  over to a new session. A refusal gives the box's reason.
- Each conversation keeps its draft and its own composer state. A send
  appears immediately in the conversation or queue, keyed by its ID.
  Reconnects retry that ID. A failed send stays visible with recovery
  actions where available.
- Send is an accent arrow; with nothing written while the session works, it's Stop (Esc too), which
  asks the box to stop (`kuvo/sessions/<id>/stop`).
- Sending while it runs steers, as the box's `send` does by default.

- **Attachments:** pick, drop or paste files. Pictures show as thumbnails
  and other files as chips. Local files can use a `file://` URI. Attachments
  to another computer and temporary pasted pictures go with their bytes.
  A file that cannot be read or sent is reported instead of being dropped.
- **"@" and "&"** (phase 3): a mention being typed at the end of what's written ("@" a word with no
  spaces; "&" a name, which runs to the line's end), each starting a word, opens the list over the
  field with what the box suggests for it (`kuvo/sessions/<id>/mentions`: the folder's files,
  ranked as the Mac ranks them; bots and sessions), each with what it is and what's beside it.
  ↑↓, Tab or Return, Escape, as "/". A pick puts in what the box says (`@web/src/Form.tsx `,
  `&Mira `); an agent picked is named when it's sent (`named`), while its name is still there.
- **The queue** (phase 3): what waits its turn shows over the composer, as the box orders it
  (`kuvo/sessions/<id>/queue`'s `items`, followed by its `…/events`): "2 waiting" (and "paused"),
  each message as you wrote it (a clock for yours, the accent for the next; another agent's with
  its name), its keyword and files. Under the pointer: Edit (yours: taken out of the queue, its
  words and files back in the composer) and Remove.

## Queue actions

When the agent works, Return steers and ⌘Return queues a message for after
the run. The send menu also offers Send Now. Waiting messages show the
actions the box supports, including edit, remove and steering. A paused
queue offers Resume. Mentions remain plain text for the box to interpret.
