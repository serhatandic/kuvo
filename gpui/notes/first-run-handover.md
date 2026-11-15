# The first run: handover

## Done

The desktop app's first run (draft A, with C's skip and landing), `kuvo/src/first_run.rs`, shown by
the window (`workspace.rs`, `Welcome`) until the box says it's been had. What it does and why:
`notes/screens-first-run.md`.

- Welcome → Agents and plans (API keys apart) → Where to work (Choose a Folder… inside it) → the
  landing view, set to start where you picked. The box is told (`POST kuvo/setup/welcomed`).
- Where the box says a model runs on an account of yours and nothing needs you, Get Started goes
  straight to Where to work.
- Signing in, in place: Settings' own `SignIn`, drawn under the row (`in_place()`); nothing opens
  Settings or leaves the flow. Screen access isn't asked.
- Stand-in: `StandIn::first_run(replies, now, Scenario::{Nothing, Crowded, Running})`,
  `stand_in/first_run.rs` (`kuvo/setup`, `kuvo/setup/welcomed`, `kuvo/folders/suggested`, the home's
  folders, the free models). The sample box says `welcomed: true`, so every other screen is as it
  was. To walk it: `cargo run -p kuvo --features stand-in -- --stand-in --first-run nothing`
  (`crowded`, `running`).
- Tests: `kuvo/tests/first_run.rs` (9 scenarios through the window). Shots:
  `shots/screens-first-run-*` (`cargo run -p kuvo --features shots --bin shots -- first-run`).
- `kuvo_box`: `added` read on each account; a suggested folder's `seen` read whether whole or with
  a fraction.

## BOX-NEEDED

1. **One row per plan across the engine and the agents.** `GET kuvo/sign-in/accounts` lists the
   engine's Anthropic as a plan (`plan: true`) wherever its ways in have a sign-in page, beside
   Claude Code: two ways to the same plan in the first run's list (the stand-in shows it). The box
   hides ChatGPT's and SuperGrok's (`planHides`, `PLANS` in `box/src/link/work/model-rule.ts`) but
   has no such rule for Anthropic. Wanted: the box leaves it out of the plans (or says `plan:
   false`, or a `firstRun` fact) where a coding agent brings that plan. Not trivial (the rule, its
   vectors, PROTOCOL.md): not done here. The client keeps no such rule.
2. **Another computer's suggested folders, through this box.** `kuvo/folders/suggested` says only
   its own computer's. Picking a machine in Where to work says "Working on <name> isn't in the
   desktop app yet". It also needs the app to show another computer's box (the title bar's switcher,
   `BoxClient::machine`), which isn't built.
3. **Adding a machine from a box** (#636, already listed in `computers-handover.md`): the first run
   leaves "Add a Machine…" out until a box can do it.
4. **Whether a first run has anything to ask** (optional): the app works out "a model runs and
   nothing needs you" from `starting`, `free` and the accounts' `state` (`FirstRun::ready`). A fact
   on the box (say `ready` in `kuvo/setup`) would save the next client the same few lines.
5. **A name and mark for the free models' provider** (optional): the row says "Free models" with a
   plain icon and the models' names, since the app doesn't know whose they are.

## Not built (the app's side)

- Starting on another computer (above). A computer that can't be reached or is older is still
  shown as a tile; picking it says the same thing.
- The landing view's project chip lists only the box's projects: after the first run, a new folder
  can be picked only by starting there once. A folder picker for the landing view would reuse
  `Step::Browse`.
- A mark or illustration over the welcome card: left empty (a brand decision).
- The phone: its first run is pairing; nothing changed there. The same rows would fit it as a list.
- Docs (kuvo-web): `installation.md` and `accounts.md` describe the Mac app's first run, which is
  what ships today; they change when the desktop app does.

## Decisions for the user

- Wording: the line under "Agents and plans" ("Sign in to the ones you use. You can change them
  later in Settings."), "Free models · No account needed", "No project · Just chat, in no folder",
  the last button's "Continue".
- Whether computers that can't be worked on from the desktop app yet are shown in Where to work at
  all (shown, with the notice), or left out until switching is built.
- Whether the welcome, where everything's ready, should be passed over entirely (it's shown, with
  what's signed in, and one press goes to Where to work).
