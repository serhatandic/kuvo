# Kuvo's GPUI client

Kuvo Code's current macOS desktop app. The box owns behavior. GPUI reads its
facts and sends actions through its routes. See [How Kuvo works](../docs/architecture/how-kuvo-works.md).

- `kuvo/`: the app (binary `kuvo`, named Kuvo): the sidebar, a conversation, the composer and the
  landing view ("The app", below), and `shots`, its screens rendered offscreen.
- `kuvo_box/`: a client of a Kuvo box, with no GPUI ("Talking to a box", below).
- `kuvo_ui/`: the blocks, one module each: `theme`, `icon`, `button`, `text_field`,
  `chat_list`, `markdown`, `code_block`, `menu`, `row`, `small`, plus the dotted `backdrop`.
- `gallery/`: the gallery app (`kuvo_gallery`) and the `shots` binary.
- `shots/<block>-{light,dark}.png`: each gallery page; `shots/screens-*.png`: the app's screens
  against the stand-in box. Both rendered offscreen.
- `LICENSES-DEPS.md`: every dependency and its licence.

## Setting up

Rust lives in Kuvo's own folders, never in `~/.cargo` or your shell profile:

```sh
export RUSTUP_HOME=$HOME/.kuvo-code/tools/rustup CARGO_HOME=$HOME/.kuvo-code/tools/cargo
export PATH=$CARGO_HOME/bin:$PATH
# Once: curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
```

`rust-toolchain.toml` pins the toolchain (1.98.1, the one GPUI's pinned revision is built with);
rustup fetches it on first use. Xcode is needed. GPUI compiles its Metal shaders when the app
starts (`runtime_shaders`), so the separate Metal toolchain isn't needed.

From `gpui/`, use `../scripts/cargo-locked.sh` for local Cargo builds, tests, checks and shots.
For a long-lived app or gallery window, build first and launch the built executable separately
so the Rust slot frees once the build finishes.

## The app

```sh
cd gpui
../scripts/cargo-locked.sh build -p kuvo --features stand-in
./target/debug/kuvo --stand-in   # against a made-up box
./target/debug/kuvo              # this Mac's own box
```

- **Launch and packaging.** The bundled desktop runs the included box
  installer, which reports setup, import or repair actions, then attaches to
  the box. An unbundled binary only attaches. From the repository root,
  `scripts/build-desktop-app.sh` builds an ad hoc `build/Kuvo.app` with the
  signed box release and Kuvo Helper. The default bundle ID uses the real
  `~/.kuvo-code`. Use `scripts/check-desktop-app.sh --run build/Kuvo.app` for
  a dry run with a separate bundle ID and box home. See [release verification](../docs/releases.md)
  for signed and notarized builds.
- **First run.** Welcome is remembered on this desktop device, with a
  compatibility read from the initial box. Get Started goes through Welcome,
  Agents and plans, and Where to work. A ready box can skip the sign-in step.
  When macOS permissions need attention, the desktop draws the box's request
  and offers to open System Settings. The permission drag tile is a GPUI popup
  with an external drag payload for the helper bundle; only this computer can open
  its own System Settings.
- **Areas.** Bots (⌃1) includes conversations and group chats. Sessions (⌃2)
  shows projects and chats. Work Queue (⌃3) has filters, labels and item actions.
  Computers (⌃4) lists each box's reach, work and update state with the actions
  that box offers. Settings (⌃5) has separate device and computer pages.
  Insights (⌃6) shows usage and cost. Library (⌃7) gathers received files and
  images across your setup when the box supports it. The rail also shows
  usage rings and waiting counts. Each area keeps its view while hidden; the hidden
  area does not draw. In narrow windows the area sidebar folds away.
- **Computer picker.** The title bar switches the workspace to another
  computer. Settings has its own picker for a computer's settings. Computers
  supports adding, renaming, changing the main and removing computers.
- **Conversations.** The sidebar shows pinned sessions, projects, chats and
  work waiting on you. New Session (⌘N) opens the landing composer. The
  composer draws model, account and effort choices from the box, supports
  attachments and mentions, and still asks the box what slash and shell
  commands mean. Sends show immediately and retry with the same ID.
- **Side panel.** ⌥⌘0 toggles it. ⌥⌘I and ⌥⌘F open Changes and Files. Each
  conversation keeps its tabs, including file tabs, terminals, browser, side
  chats and other outputs where supported. Changes shows changed lines
  under their files. In a narrow window the panel overlays the conversation.
  The divider resets on a double click; its width survives hiding the panel.
  Tools use the conversation’s computer and worktree.
- ⌘⇧L switches light and dark.

`kuvo::stand_in` (the `stand-in` feature, left out of the app as shipped) is the made-up box: a Mac's box with sessions in a few projects and chats, one at
work, one waiting on you, one long enough to page through. It answers in this process, as a box
answers the routes the app uses; with `--stand-in`, a message sent gets a reply streamed back.

Shots (`gpui/shots/screens-*-{light,dark}.png`), from the stand-in at a fixed time:

```sh
../scripts/cargo-locked.sh run -p kuvo --features shots --bin shots [-- conversation landing asking narrow empty]
```

## Drawing contracts

- Keep a single selection holder per conversation. Selectable text reports layout and paints
  the selection; ordered message keys, content paths and byte offsets let it survive streaming
  and paging. Copy rebuilds units from conversation data, not the visible rows, so it spans
  offscreen messages without relayout. Chrome and controls are not selectable text.
- The theme owns semantic tokens and text roles. The system UI font is used on macOS; GPUI’s
  font matching needs weight 520 to select SF Medium. The proposed negative letter spacing is
  recorded but not applied because text styles have no tracking control. Brand marks use the
  approved maker colors (Claude orange, the other marks monochrome); changes are a brand decision.
- Virtualized lists preserve scroll by splicing changed rows rather than resetting the list.
  The conversation follows its tail only while the reader is at the bottom. Menus return focus
  only if they still own it, not when their selected action intentionally moved focus.
- Text fields handle graphemes and IME composition, retain one undo history bounded by bytes,
  and turn pasted line breaks into spaces for single-line fields. Markdown links open only
  supported schemes; code highlighting and line wrapping are presentation, not box rules.

## Running the gallery

```sh
cd gpui
../scripts/cargo-locked.sh build -p kuvo_gallery
./target/debug/kuvo_gallery
```

Pick a block in the sidebar. ⌘⇧L switches light and dark (or the sun and moon button); ⌘[ and
⌘] step through the blocks. Everything is live: type in the fields, open the menus, right-click
the sessions, press "Stream a reply into both" on the chat list and scroll one side back.

## Shots

```sh
../scripts/cargo-locked.sh run -p kuvo_gallery --features shots --bin shots            # every block
../scripts/cargo-locked.sh run -p kuvo_gallery --features shots --bin shots -- menus   # just one
```

GPUI renders each page with Metal into a texture that's read back, so no window opens and no
screen-recording permission is needed. Output goes to `gpui/shots/`.

## Tests

From `gpui/`, run focused scenarios through the shared Rust lane:

```sh
../scripts/cargo-locked.sh test -p kuvo --features stand-in --test screens <filter>
../scripts/cargo-locked.sh test -p kuvo_box --test vectors <filter>
```

The app scenarios use test windows and a stand-in box. Use the matching
target for areas, composer, settings, queue, bots or the panel. `kuvo_box`
tests wire vectors, a loopback stand-in and SSH with a test home.
See [the testing guide](../docs/testing.md).

## Talking to a box

`kuvo_box` discovers the local resident through `run/serve.json`. Remote
computers can use paired Noise over the relay or addressed SSH, with the
keeper launching `route-to`. Auto can choose between those transports.
Addressed SSH streams supported responses. Legacy `route` can poll and carries
OS-account authority. Browser and terminal sockets use local HTTP or paired
relay where supported. See [the protocol](../protocol/PROTOCOL.md) and
[resident launch and rollback](../docs/architecture/one-client-process.md).

The client draws the box's compatibility status from `kuvo/info`. A box
that's behind can still do ordinary work. If a request cannot be supported
or authorized, the box supplies the refusal.

```rust
use kuvo_box::{BoxClient, Draft, Timing};

let client = BoxClient::this_mac(&home)?;
let info = client.connect()?;
let sessions = client.follow_sessions(Timing::default());
let draft = Draft::new(BoxClient::new_id(), "/plan sketch it");
client.send_draft("ses_…", &draft, None)?;
```

The current composer sends slash commands through the typed route. The box
also describes stored message parts. The desktop retains a send's ID while
retrying. Its session lists, conversations, queues and areas still use
separate follows. The shared event line and merged session lists are pending.
See [message delivery](../docs/architecture/message-delivery.md).

To list sessions against the stand-in:

```sh
../scripts/cargo-locked.sh run -p kuvo_box --example list_sessions -- --stand-in
```
