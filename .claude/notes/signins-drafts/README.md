# Saved sign-ins from 0.7: where the app asks (drafts, 2026-11-17)

`options-light-dark.png` (from `signins-options.html`, light left, dark right). The box side is built:
`kuvo/box/rehome` says `owed: ["passwords"]` and `signIns` (`asking` / `done` with `added`, `kept` /
`failed` with `message`), and `POST kuvo/box/rehome {signIns: true}` starts the hand-over (Kuvo on
this Mac only). What isn't built is where the app draws it.

- **A, a row in Settings › This Mac (recommended).** Beside Running and the update: it's this Mac's
  Keychain and this Mac's prompt, and that page already holds the other "macOS asks/allows" rows.
  The button opens a short confirm that says what macOS will ask (the rule: prompts only when you
  can see why), then the row shows waiting / done (Open Vault) / failed (Try Again…). Gone once
  nothing's owed. The phone shows nothing (the prompt is on the Mac).
- **B, at the top of the Vault page.** Where people look for passwords; but the Vault page lists
  any computer's vault, and this is only this Mac's.
- **C, a one-time line atop the window** with Not Now. Most visible; but a banner for a one-off,
  low-stakes step is more than it needs, and it competes with Update Ready.

How others do it: Chrome and Safari import passwords only from Settings, with a confirm that says
the system will ask for your password; 1Password and Bitwarden the same (an Import page, never a
banner). A matches that. No count is shown before reading: the box can't know it without asking
macOS.
