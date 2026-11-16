# As the app opens, and what macOS allows (Zed's remote server setup, Zed's file drag)

Code: `kuvo/src/launch.rs` (the window's root), `kuvo_box/src/install.rs` (running the installer
the app carries), `kuvo/src/permissions/` (the heads-up, the tile, its icon, the two macOS calls).
Shots: `shots/screens-launch-*`, `shots/screens-permission-*`. Tests: `kuvo_box/tests/install.rs`,
`kuvo/tests/launch.rs`, `kuvo/tests/permissions.rs`. The drafts they were picked from:
`.claude/notes/drafts/713-p7/README.md` on the drafts' branch (first run A for a new Mac, B for
a move, which piece 7 builds; permissions C).

## What Zed does

Read for behaviour only (GPL, nothing copied):

- `crates/recent_projects/src/remote_connections.rs`: setting up its server on a machine is one
  small prompt with a status line and a spinner, which turns into the error with a way to try
  again. Nothing else of the app shows meanwhile.
- `crates/auto_update`: an update is offered quietly, never in the way.
- `crates/gpui_macos/src/window.rs` (`start_external_drag`) and `crates/gpui/src/window.rs`
  (`promote_external_drag_to_platform`), both Apache-2.0, the GPUI we build on: a drag begun on
  an element (`on_drag`) that leaves its window is handed to AppKit as a file drag, if the
  element named files for it (`external_drag_payload`).

## What Kuvo takes, and where we differ

- **One status line while it's set up, the error in its place with Try Again**, as Zed's. Ours
  sits in the first run's own Welcome card, on this Mac's row (the user's pick), so a new Mac's
  setup runs straight into the welcome on the same card. Someone whose Kuvo is here already
  never sees the card: the window's quiet "Starting…", then their sessions.
- **The box decides everything.** The app runs `kuvo-install status`, then the verb it names,
  and draws each line: for a move the box's own words and count for the step (`title`, `index`,
  `of`), as a thin bar under the row. A move the box still runs from an earlier open (`waits`
  with `status.progress`) is drawn where it is and asked about again every two seconds. The
  installer's own first steps (`checking`, `engine`) have no words yet (BOX-NEEDED), so a new
  Mac's row says only "Setting up Kuvo…".
- **Only the app in its bundle installs.** `cargo run` carries no release: it attaches, and says
  so where there's nothing to attach to. The installer checks the same again itself.
- **The drag is GPUI's own.** The tile is a GPUI window (`WindowKind::PopUp`: a panel over
  System Settings that takes no keyboard) whose icon is an `on_drag` with
  `external_drag_payload` naming the helper's bundle. Where we add to GPUI
  (`permissions/mac.rs`, macOS only): finding System Settings' window
  (`CGWindowListCopyWindowInfo`, frames only) and moving the tile beside it
  (`setFrameTopLeftPoint:`), which GPUI has no call for.
- **The icon** is read from the helper's own bundle (the PNG inside its `.icns`,
  `permissions/icns.rs`); the image macOS shows under the pointer once the drag has left the
  tile is its plain one for an app.
- **Every name is the box's**: which permission to bring up, what System Settings calls it, the
  address of its list, the program's name and bundle. Only the box on this computer is asked:
  System Settings can't be opened on another.

## States

Asking (quiet); setting up; moving, with its step and bar; starting (waited for up to a minute);
refused or failed in the box's words, with Try Again; a move that stopped (where, and why, with
Try Again); another move at work; an installer that can't be read; a build that
installs nothing; a narrow window. Permissions: nothing to ask; one to ask; System Settings open
with the tile; allowed (the tile goes, the next one is asked); Not Now; the box can't take Not
Now (said, the heads-up stays); a box on another computer (nothing).

## Not seen in a running window

Rendered offscreen and driven in GPUI's test window only. The tile over the real System
Settings, the drag into its list, the tile following its window, VoiceOver and larger text need
the rehearsal account.
