# #761 drafts: Kuvo Helper's menu bar item, and the first-Quit note

Rendered with SwiftUI's ImageRenderer (`swiftc -O -o render render.swift && ./render .`), light and
dark, made-up content, the desktop app's colours (`gpui/kuvo_ui/src/theme.rs`).

## The item and its panel

- `A-card.png`: **A, a card.** The item is a mark, the count of sessions at work beside it, an amber dot
  while something needs you. Its panel is a card in the desktop app's look: Needs you, At work
  (with the bot it works for), the rings (as the rail draws them, short outside, long inside), Open
  Kuvo, Stop Kuvo on this Mac….
- `B-menu.png`: **B, a plain macOS menu** (as Tailscale and Docker Desktop do): the same facts as menu items.
- `C-glance.png`: **C, a glance.** The rings themselves are the item. The panel shows counts and big rings, and no list.
- `item-states.png`: the item in each state: working with something waiting, working, idle, and C's rings.
- `stop-confirm.png`: Stop asks first only while something is at work.

## The first Quit

- `quit-1-in-window.png`: **1, in the window, as ⌘Q is pressed the first time**: a card that shows the
  item, then Cancel and Quit. After you choose, it never shows again.
- `quit-2-from-item.png`: **2, after quitting**: the item's panel opens once by itself, with a note on top
  (0.7 did this).

## What others do

- **Docker Desktop**: closing the window keeps it in the menu bar. Its item is an NSMenu with the
  status, Dashboard and Quit, and the whale moves while it starts.
- **Tailscale**: it lives only in the menu bar, as an NSMenu: status, devices, Exit.
- **Dropbox**: a menu bar item with a popover panel of recent activity. The icon shows syncing.
- **Claude desktop**: an optional menu bar item with a short NSMenu.
- **Slack, Discord and Teams** (on Windows): the first time you close the window, a note says the app
  is still running in the tray. That's the closest to our first-Quit note.
- **Where we differ**: Quit keeps the work going (Kuvo Helper is a separate program), so the item can't
  go away while Kuvo runs here. A panel lists sessions and asks, which a plain menu does poorly.

## Recommendation

**A, with note 1.** A matches the desktop app's cards. It shows what's at work and what needs you
without opening the app, and the count and dot give the item meaning at a glance (Dropbox's
lesson). Note 1 explains before anything surprising happens, and lets you cancel. Note 2 depends on
the app telling the helper it just quit, and it opens a panel by itself.

**Brand decision:** the item's mark in A and B is a stand-in SF Symbol. Kuvo's menu bar mark is for
the user to pick.

## Picked (11-16)

A (the card) and Quit note 1. The item's mark is still the user's: `logo-template.png` puts four
options side by side on light and dark menu bars, each with and without a count: the stand-in, the
app icon's own artwork made one colour (with thin gaps where its shapes' colours meet, or solid,
eyes clear either way), and the website favicon's hexagon. `logo-template-gaps.png` and
`logo-template-solid.png` are the templates themselves (`logo.swift` makes them from
`swift/KuvoCode/AppIcon.icon/Assets/artwork-light.png`). The helper draws its mark from
`MenuBarMark` alone, so whichever is picked goes in there.
