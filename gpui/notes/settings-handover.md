# Settings split: handover

The desktop app's Settings as the user picked it (draft A of #727, 2026-11-15): your settings and a
computer's, a sidebar and a search each. What it is and how it's drawn: `notes/screens-settings.md`.

## Done

- Two sides, the foot row between them, the computer picker as the computer sidebar's title
  (changes only Settings), search on its side with the other side's matches listed.
- A computer's sections from its box (`kuvo/settings`), each page drawn by one renderer for every
  row kind (`kuvo/src/settings/described.rs`); changes with the row's revision, a stale one shown
  with the page as it is now; the open page followed where the box streams.
- States: reading, couldn't be read, away, connecting, known only through another, older Kuvo, not
  reachable from the desktop app yet, a section not in the desktop app yet, nothing found.
- `kuvo_box`: `Transport::peer` and `BoxClient::computer` (the box of one of `kuvo/computers`'
  computers, where this client reaches it: today only the window's own).
- Stand-in: `kuvo/settings` as a box answers it (General, Permissions, Browser; revisions; 403,
  404, 409 with the page), a machine's box behind it (`devbox`, `peer_box`), `forget_peers`,
  `add_settings_section` with `settings::every_kind()` (a row of each kind), `set_browser_setting`.
- `kuvo_ui`: a menu scrolls past 420 px; `MenuEntry::muted`.
- Tests: `kuvo/tests/settings.rs`. Shots: `shots/screens-settings-a-*`.

## BOX-NEEDED

1. **Accounts, Models, Usage onto `kuvo/settings`** (#741): still drawn from `kuvo/sign-in/accounts`,
   `kuvo/config`, the offered models, the limits and the usage settings, with their words in
   `rows.rs` (`OWN`, `accounts`, `models`, `usage`). Models' three rows and Usage's three notices
   are worded in the app; Accounts' layout rules are the app's.
2. **The rest of a computer's sections** (`rows::LATER`): Custom Auto, Agents, Commands, Skills,
   Integrations, Vault, MCP Servers, Git, Worktrees, Archived Sessions, Engine, Storage, Phone. Each
   is listed and says it isn't in the desktop app yet; one the box describes under the same name
   takes its place by itself.
3. **`steps`** (signing in, pairing, a vault item led by the box): no box answers it yet; a
   `steps` row says it isn't in the desktop app yet.
4. **`secret`'s change isn't in PROTOCOL.md**: the app sends the typed text as `value`, and
   `null` for Remove. No box section has a secret yet; settle it in the protocol with the first.
5. **`folder`**: typed for now; the box's folder picker (`kuvo/folders`) isn't wired to it.
6. **A list's `add`**: drawn as a button that asks the box (`POST …/<row>/<add id>`); what an add
   that needs words (a host, a folder) asks isn't said in the protocol yet.
7. **A last reading of an away computer's settings**, to draw read-only (the drafts' question 5):
   not kept; it says "<name> is away".
8. **When a computer was last seen** (the drafts' "seen 2 h ago"): `kuvo/computers` doesn't say.
9. **Icons**: a box names `settings`, `shield`, `globe`…; one the app hasn't draws as sliders. The
   set every client has isn't listed in PROTOCOL.md.

## Not built (the app's side)

- **Reaching another computer's box.** The desktop app reaches the window's box alone (as the
  title bar's switcher says), so against a real box every other computer's settings page says
  "<name>'s settings aren't in the desktop app yet". `Transport::peer` is where a machine over SSH
  or a box through the relay comes in; the stand-in answers it for `devbox`.
- Your own sections beyond Appearance (General, Notifications, Voice, Keyboard Shortcuts, Side
  Panel, Updates, Privacy): listed, each saying it isn't in the desktop app yet.
- `custom` rows: none is drawn (background work, the login item); they show the box's why.
- A long menu (every time zone) scrolls but has no type-to-filter; the keyboard's highlight doesn't
  scroll into view.
- The phone: the same split there is its own piece.
