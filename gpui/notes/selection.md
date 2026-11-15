# Selecting and copying text in a conversation

**Ours:** `kuvo_ui/src/selection.rs` (the model, the text element, the area that takes the
mouse), used by `markdown.rs`, `code_block.rs`, `output.rs`, and owned per conversation by
`kuvo/src/conversation.rs` (the rows' text for copying: `kuvo/src/entries.rs`, `units`).
Gallery: the Selection page. Shots: `shots/selection-*`, `shots/screens-select-*`.

## How Zed does it (studied, not copied: `crates/markdown/src/{markdown.rs,selection.rs}`, GPL)

- **One selection per rendered markdown block** (`Markdown` entity: `start`, `end`,
  `reversed`, `pending`, and a mode: character, word, line, all). Positions are byte offsets
  in the message's markdown *source*, so text streaming in at the end never moves them.
- **Hit-testing laid-out lines**: each run of inline text is a `StyledText` whose `TextLayout`
  is kept after layout (`RenderedLine`, with a map from drawn to source offsets). A point goes
  to the line that contains it, else the line on its row, else the end of the line above a gap.
- **Painting**: the highlight is painted per wrapped segment *between* the runs' backgrounds
  and the glyphs (`paint_background`, quads, `paint_foreground`), so inline code chips neither
  hide it nor hide under it. Nothing is laid out again when the selection changes.
- **Mouse**: press sets the anchor (shift extends from the old one; 2 clicks a word, 3 a line,
  4 all), move extends by the same unit while pressed, release ends it; a press on a link only
  opens it if released on the same link. A press outside clears it. The press focuses the block,
  so ⌘C (`markdown::Copy`) and Copy as Markdown go to it. While dragging, it asks its scroll
  parent to keep the head in view (`request_autoscroll`).
- **Reach**: within one message. The agent panel's right-click menu has Copy, Copy as Markdown,
  Copy Link, and "Copy This Agent Response" for the whole message.
- **Copy**: plain text joins the selected lines with newlines; Copy as Markdown re-balances the
  source so a half-selected `**bold**` comes out closed.

## Why the Mac version got messy

`swift/KuvoCode/Views/Session/RowStack.swift` and `ReplyView.swift`: every chunk of a reply is
its own `NSTextView` (prose, each code block, your message), and each of those natively *owns a
selection*. To select across rows, `RowStackView` keeps a second, conversation-wide selection
(`(row, unit, offset)` marks) and pushes ranges back into each text view, tints rows that have
no text view, intercepts their mouse and keyboard (`trackSelection`, "two holders of one
selection would each draw and clear it their own way"), recolours TextKit's highlight
(`SelectionLayoutManager`, `SelectionColor`) and redraws on focus changes. `SelectionMenu` adds
a third holder for widgets. The cause: **the selection lives in many native views that each
think it's theirs**, and a coordinator fights them; every new kind of chunk needs new wiring.

What we avoid: any element holding selection state; any per-chunk selection; deciding what's
selectable from the views on screen (rows scrolled away still copy).

## Our design

GPUI elements hold no state, so there's only ever **one holder**: a `SelectionState` per
conversation. Text is drawn by a `SelectableText` element that only *reports* where it was laid
out and *paints* its part of the selection.

- **Units.** Each piece of selectable text is a unit addressed by its row's key (the message id)
  and a path in reading order (`[content part, block, item, …]`, the same path the markdown
  already used for element ids). Paths compare lexicographically, so reading order needs no
  layout. A spot is a unit plus a byte offset. Text streaming in only appends units or text, so
  a selection survives streaming; rows keep their keys, so it survives earlier pages loading
  above and scrolling away.
- **What's selectable**: your words, the agent's markdown (headings, lists, quotes, tables,
  inline code, links), code blocks (not their line numbers), thinking text, and opened tool
  output (the command and what it printed, a diff's lines without numbers or signs). Chrome isn't:
  step rows, chips, times, buttons. A selection across them just skips them.
- **Layout is never redone for selection.** Each frame, units in view register their
  `TextLayout` and bounds (cheap: GPUI's list only draws rows in view). Hit-testing walks that
  list; painting a highlight is a few quads per visible line. Copy doesn't need layout at all:
  it rebuilds each row's units from the conversation's data (`entries::units`), so a selection
  reaching rows scrolled far away still copies whole.
- **Hit-testing** (Zed's rules): the unit under the point; else the unit on that row, nearest
  side; else, in a gap, the end of the unit above; above everything, the first unit's start.
- **Mouse**: press, drag, double-click word (Unicode word bounds, as the composer), triple-click
  line (a paragraph is one line until it has a hard break; in code and output it's the code
  line), shift-click extends, dragging after a double or triple click extends by words or
  lines. A press in the transcript's empty space starts a selection too (as a page does). Links
  open on a click, never at the end of a drag or a double-click.
- **Across messages: yes.** Zed stops at a message, but a conversation reads as one document
  and every chat a Mac user knows that's built on a page (ChatGPT, Claude, Codex, Cursor's chat)
  selects across messages; the Mac app did too. With one holder it costs nothing extra. Copy
  joins messages with a blank line.
- **Auto-scroll**: while the button is held past the list's top or bottom edge, the list
  scrolls each frame, faster the further out the pointer is, and the selection follows.
- **⌘A** selects the message you're in (where the selection or the last click is); pressing it
  again selects the whole conversation as loaded. (Zed: the message; a page: everything. This
  gives both. A decision for the user.)
- **⌘C, right-click Copy**: plain text. Blocks are joined with a blank line, list items get
  `- `/`1. `/`- [x] ` and two spaces per level, table cells tabs, code as it is (no fences).
  The right-click menu: Copy Link (on a link), Copy, Select All, as the app's other menus.
- **Look**: the theme's `selection` (the accent, faint) while the transcript has the keyboard,
  `pressed` (grey) when it doesn't, as macOS and our text field do. Inline code's fill is now
  painted by the same element as a rounded chip under the highlight.
- **Keyboard**: a press on text gives the transcript the keyboard (so ⌘A/⌘C are the
  transcript's); a press on empty space only does once the drag selects something, so clicking a
  fold button leaves the composer typing.

## Not yet

- Copy as Markdown (needs a map from drawn text back to the source, as Zed has).
- Keyboard selection (shift-arrows) in the transcript; Find.
- Text in widgets and other non-text rows.
