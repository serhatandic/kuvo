# The first run (Zed's onboarding, Capy's cards)

Code: `kuvo/src/first_run.rs`; the window shows it from `workspace.rs` (`Welcome`). Shots:
`shots/screens-first-run-*`. Tests: `kuvo/tests/first_run.rs`. The drafts it was picked from:
`notes/first-run-drafts.md` on the drafts' branch (option A, with C's skip and landing).

## What Zed does

Read for behaviour only (`crates/onboarding/src/onboarding.rs`, `basics_page.rs`; GPL, nothing
copied): one welcome page in the window's place, with Finish Setup always there; sections you can
pass over (theme, keymap, agents with Install each); what's set applies at once, and the page is
shown once, remembered in its own store.

## What Kuvo takes, and where we differ

- **Shown once on this device.** The desktop's saved `welcomed` flag comes first.
  A compatibility read from the initial box can carry forward an older welcome.
  Switching computers skips first run. When the initial box is still being
  read, the window shows its startup or reach status.
- **One card at a time.** Welcome, Agents and plans, then Where to work.
  Back returns to the preceding card. Return takes the primary action and
  Escape goes back. Welcome can skip the account card when the box is ready.
- **Each row is the box's** (`kuvo/sign-in/accounts`): its mark, name, line, and its first action
  that signs in or installs, in the box's words. `plan` keeps plans in the list and keys behind
  "Use an API Key Instead…"; `added` leaves an account added beside an agent's own to Settings, so
  each agent has one row. A signed-in row ends in a check and offers nothing.
- **Signing in happens under the row**, with Settings' own sign-in (`settings/sign_in.rs`, drawn
  in place): the box's ways in where there's more than one, a device code, a code to paste, a key.
  One at a time; Cancel, going on or back gives one up on its engine too. Install runs in the row,
  then opens the sign-in.
- **The free models** are a row of their own (`free` in `kuvo/models/offered`), so nothing has to
  be signed in to go on.
- **Skipping ahead**: where the box says there's nothing to ask (`ready` in `GET
  kuvo/setup/ready`: a model on one of your accounts runs, and no plan's sign-in ended or reports
  a problem), Get Started goes to Where to work, and the welcome lists what's signed in. The box
  decides; the app only goes by it. Agents and plans stays a step Back.
- **Where to work**: your computers as tiles where there's more than this one (selected is the
  theme's `selected` wash, not a raised card); under them the folders the box suggests
  (`kuvo/folders/suggested`), the latest worked in picked to begin with, "No project" as a row,
  and Choose a Folder…, which looks through the computer's folders as the box lists them
  (`kuvo/folders`), never a system picker: the same on a machine.
- **It ends on the real landing view**, its project chip set from the pick, the keyboard in the
  composer.

## States

Box starting, not answering, or older than the app needs (the window's own held words, mid-way
too); accounts loading, unreadable (Try Again), the agents' bridge down; installing, an install
that failed, a sign-in that failed; many rows (the card's middle scrolls); no folders suggested
(nothing picked: you choose, or No project); folders unreadable; a folder the computer won't let
be read; a narrow window (the card takes the width).
