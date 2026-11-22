> Historical, superseded by [How kuvo works](../architecture/how-kuvo-works.md), kept for its findings.

# Simple updates: prepare everywhere, then switch together

Status: design, trimmed to the smallest version that meets the user's rules (2026-11-21).
Decisions by the lead, on the user's behalf: D1-D5, D7, D8 and D10 yes; D6 (the app joins the
vote) no, so Sparkle stays exactly as it is; D9 is cut below. No code has changed. Line numbers
are at main `1b44ea971`.

The user's rules, which everything below is measured against:
1. One version per computer, with every part switching in one step.
2. Prepare everywhere, then switch together.
3. Never locked, nothing manual.
4. Computers that are off don't block anyone.
5. Updates from older versions, including 0.7.2, are covered.

---

## Part 1, in plain words

### What went wrong on 11-21

You updated four computers: main-linux (main, Linux), mac-mini (Mac), macbook (Mac) and
spare-linux (Linux, off since 05:43).
- **One computer that was off held up the other three.** Each computer waited for every
  computer to confirm it could read the new folder layout, and spare-linux couldn't answer.
- **Waiting was reported as failing**, with retries an hour, then a day, later.
- **Kuvo shut you out of computers that were behind** ("This computer needs to update to
  0.8.0-beta.38 before it can be used"), including during their 5-minute health check. The
  app also kept a refusal list of its own.
- **Update Now hit the same walls.**
- **Each computer was half updated**, because Kuvo's parts switched at different times.
- **A one-character bug** broke Kuvo's internal calls, and nothing stopped that release from
  being installed.

### The new way

1. **Get ready.** When your main computer has a new version, every computer that's on downloads
   it, checks it, and sets it up next to the old one. You keep working on the old version.
2. **Switch.** Once every computer that's on is ready, the main computer switches. The others
   see that and switch too. Each computer switches **everything at once**: one moment it's
   entirely the old version, the next entirely the new one.
3. **Or nobody switches.** If a computer can't get ready, nobody switches, and everyone keeps
   working on the old version. If that computer still can't after a day, the others go ahead
   without it, and it shows why.

A computer that's **off** isn't waited for. When it's back, it updates to what the main computer
runs.

**Nothing ever locks you out.** Every computer works with your app and phone at every moment, on
whatever version it runs. A switch takes a few seconds, the same as Kuvo restarting. Coding
agents' turns carry on through it, and any other turn it interrupts picks up again by itself.

**From 0.7.2 and older:** Kuvo is updated to the current version without asking.

### What you'll see

- While computers get ready: "Updating to beta.39 · 2 of 4 ready". Nothing to do.
- When everyone's up to date, the notice goes away by itself.
- A computer that's off: "beta.38 · Off: updates when it's back". Nothing to do.
- A computer that can't update for a reason you can fix: the reason and one button, **Try
  Again**. For a reason you can't fix: the reason and "Updates with the next release".

---

## Part 2, the technical design

### The switch: one link per computer

Every file a release installs is under `bin/` (`BOX_FILE`, `update.ts:89`: `bin/*`,
`bin/runtime/*`, the Mac helper bundle `bin/Kuvo Helper.app`) or `config/opencode/plugins/`.
The keeper and every launch path already go through `bin/` (the `@reboot` line runs
`sh "$HOME/.kuvo-code/bin/kuvo-start"`, `add/scripts.ts:270`). The engine binary is already
kept per version (`engine/<version>/opencode`), and which one runs is `V=` in `bin/kuvo-start`.

So **`bin` becomes a symlink** to `versions/<version>-<sha12>/bin`, and
`config/opencode/plugins` becomes a symlink to `bin/opencode-plugins`. With that, one
`rename()` of a new `bin` link over the old one switches every part (link, bridge, browser
keeper, keeper, wake, plugin, which engine, the Mac helper) in one atomic step. That's rule 1.
No launch entry changes.

**Switching** (a detached `kuvo-link.js update --switch`, as updates run today, `update.ts:1281`):
1. Write `run/previous` = the old `bin` target (the only record of a switch).
2. `kuvo-start stop`: every Kuvo process stops. Coding agents under holders keep running
   (`agents/hold`).
3. `rename(bin.new → bin)`. **This is the switch.**
4. `bin/kuvo-start start`, now the new keeper. The new link upgrades the folder layout as it
   starts (`upgradeKuvoFolder`, already checkpointed), and the new bridge adopts held turns.
5. **Start guard** (in the keeper, so it runs even if the updater died): while `run/previous`
   exists, the keeper asks the new link `box route --internal kuvo/info` after it starts. If it
   answers, `run/previous` goes. If it doesn't within 2 minutes, the keeper renames `bin` back to
   `run/previous`'s target, starts that, and records `cannot: "<version> didn't start here"`.
   Going back is safe for data by the 848 rule (incompatible formats use separate paths, and
   downgrades keep working) and by the engine's `downgradeSafe` check (`update.ts:400-405`).
6. The version before `previous` is deleted. Two versions are kept at most.

**Crash or power loss:** if it happens before step 3, `bin` is the old version: the old version
starts, still has the new one staged, sees the main runs it, and switches again. After step 3,
`bin` is the new version and the guard applies. Either way the computer is whole on one version.

**The one-time conversion** of today's `bin/` folder into the link happens when R (the first
release with this design) first starts: `ln -s versions/R/bin bin.new`, `mv bin versions/R/bin`,
`mv bin.new bin`. Between the two `mv`s (microseconds, once per computer) there's no `bin`.
Power loss in exactly that window leaves a computer that doesn't start at boot. The next
`kuvo-install` (the app on that Mac, or Set Up Again) restores `bin` from `versions/R/bin`. We
accept that risk instead of building around it.

**Interrupted turns** (rule 3, nothing manual): a restart today pauses an engine run it cut off
"until Resume" (`engine/queue.ts:5-8`). After an update's switch, it continues instead: the box
sends that session "continue" once. That also covers an agent whose holder couldn't be adopted
(`adoptable`, `hold/adopt.ts:26`).

### Prepare

A computer prepares the release its main holds (see "The fleet rule"), entirely beside what runs:
1. Fetches the signed manifest and pack from the main or the standby (as today, `fleet.ts:273`),
   and the engine from its URL if it isn't there yet.
2. Verifies the signature, the pack's SHA-256 and every file's SHA-256 (`update.ts:709-774`,
   unchanged).
3. Unpacks into `versions/<version>-<sha12>/bin`, the plugin into `bin/opencode-plugins`, and the
   engine into `engine/<engine>/` (unused until the switch).
4. Runs the checks that exist today, from the staged files: `opencode --version`, the link's
   `selftest` on the new engine and on the running one (`update.ts:811-820`), the bridge's
   `selftest` (`agents/main.ts:80`), and `sh -n` on the keeper (`:821`). It adds one:
   **`box route --internal kuvo/info` from the staged link** (11-21's bug), the same call the
   start guard uses.
5. Checks there's room (`roomForData`, `update.ts:453`).
6. Announces `staged: {version, sha256}`, or `cannot: {version, reason, fixable}`.

### The fleet rule

There are no new messages and no round records. Each computer already announces its facts to its
peers (`announce`, `keep.ts:30`; `fleetFacts`, `fleet.ts:158`: `kuvo`, `installedSha256`,
`release`). The fleet rule reads them:

- **The main** holds a signed release newer than what it runs (from its feed check, or one a
  Mac's app handed its box; see "Where releases come from"). It announces it as `release`, and
  prepares it too.
- **Every member** that sees the main announce a `release` newer than its own version prepares it
  and announces `staged` (or `cannot`).
- **The main switches** when every member that's on (not away: `awaySince`, `peers/reach.ts:79`)
  has announced `staged` for that release. It also switches 24 hours after it first held the
  release, with whoever is staged by then (D3). Members that are off don't count (rule 4).
- **Every member switches** as soon as it sees the main *running* a version newer than its own:
  at once if it has that version staged, otherwise after preparing it. That is the commit, and
  the same rule is how a computer that was off catches up (rule 4).
- **A computer never goes back** (signed releases only, never older; unchanged). After a
  takeover, a new main that's behind its members prepares their version first, the same way.

Why this is enough for rule 2: nothing switches until the main does, and the main switches only
when all that are on are ready. If the main can't prepare, or its own start guard sends it back,
nobody switches: the main is the first to try the new version. If the main dies before
switching, nothing has changed anywhere. If it dies after switching, its members follow when it
answers again; until then they're whole on the old version and fully usable.

**Try Again** (only for a fixable `cannot`): re-runs that computer's prepare. It reuses
`POST kuvo/update/retry` (`progress.ts:175`) with this meaning.

### Where releases come from

- The main's feed check, unchanged: GitHub `boxes.json` every 6 hours, taken 3 days after
  release (`update.ts:58`, `:87`).
- A Mac's app (Sparkle, unchanged, D6 no) offers its bundled release to its own box at launch
  (`launch.rs:494`, `POST kuvo/box/update`). The box now **only keeps** it (`keepRelease`) and
  announces it as `release`. It never installs it by itself. The main takes a newer signed release
  that any member announces: the one rule widened in `releaseAuthority` (`fleet.ts:273`), and
  the main only. That's how betas from the private feed reach the fleet, and why D9 (a setup-wide
  beta channel) isn't needed.
- `POST kuvo/update {now}` on the main looks at the feed now. It exists already and stays as it
  is. No new button.

### States

| State | Meaning | Shown |
| --- | --- | --- |
| Up to date | Runs what the main runs | Nothing needed |
| Getting ready | Preparing the main's release | Nothing needed |
| Ready | Staged; waiting for the main | Nothing needed |
| Off | Away | Nothing needed: "updates when it's back" |
| Can't update | Its last `cannot` | Fixable: the reason + Try Again. Not fixable: the reason + "Updates with the next release" |

There's no "switching" state: it lasts seconds, and a computer that's switching can't announce
anything anyway. A computer that was off and is catching up shows as getting ready.

### Mixed versions (D7)

Mixed versions remain in four cases: between computers for the seconds after the main switches,
on a computer that's off or can't update, in an app that hasn't relaunched into its Sparkle
update, and in a phone. None of them refuses anything. Step 0 (being built now) removes the
version refusals in the box and the app. Features follow what each computer says it has (the
existing capability gate), and data follows the existing explicit format versions: an unknown
format isn't acknowledged, and is delivered again after the reader catches up.

### Older versions

| Coming from | How it gets R |
| --- | --- |
| Mac app 0.5 to 0.7.2 | Sparkle installs the desktop app; R's appcast entry is `sparkle:criticalUpdate` below 0.8.0 (the forced update). The app's first-launch move (#713) installs R with R's installer. |
| A machine set up by 0.5 to 0.7.2 | Already forced today: "it gets this box when the first Mac with 0.8 connects" (PROTOCOL.md, "For released Macs"). Any box with R that adds or sets it up again installs R. Nothing new to build, only the test. |
| 0.8.0-beta.1 to beta.38 | Through step 0's release first, which their old updaters take by the old path (step 0's session covers the bootstrap for computers stuck before it). Then step 0's updater takes R through the old choice record, which R's main keeps writing after it switches (removed once no member is below R). |

### What gets removed

Removed in step 0 (in progress elsewhere): the version refusals in `client.ts` and their callers,
the app's fleet gate and saved holds, the "blocked counts as failed" bookkeeping, and the wait on
members that are off.

Removed by this design:
- `layoutHold` (`choice.ts:38-51`) and its uses (`update.ts:719-721`, `:854-855`, `:1510-1515`,
  `areas/setup.ts:213`, `setup/shared.ts:213-225`), and the "every member supports F" checks
  for what the main runs (`keep.ts:69`, `setup/handover.ts:135-139`, `setup/takeover.ts:213-216`).
  Under the fleet rule a member reads setup data only after it runs the main's version.
- The health trial: `health.ts` (`settleUpdate`), the trial marker and `ON_TRIAL`
  (`update.ts:60`, `:301`, `:592`, `:1416`), `confirmUpdate` (`:1074`), and `keepForRollback`
  with its `restore.sh` (`:337-394`). The start guard and `previous` replace them.
- File-by-file swapping (`update.ts:886-910`) and `swapBundle` (`:972`): replaced by the `bin`
  link.
- The engine change at a later quiet moment: `engine-swap.ts` and `engineApart` (`update.ts:511`).
- The bridge's quiet-moment wait: `agents/renewal.ts`.
- Failure backoffs and the offer's rest: `update.ts:99-100`, `:1435-1461`, `:1582-1596`,
  `attempt.ts:35`, `fleet.ts:45`.
- Installing an offered release: `offerDue`, `takeOffer`, `startOfferedUpdate` and installing from
  `updates/offered` (`update.ts:1555-1671`). Offering only keeps the release now.
- `progress.ts` states `failed`, `behind`, `nextRetryAt`, and the dismissal store (`:72-90`,
  `:118-159`).
- The release choice as a setup save that waits on the standby, as what R members follow
  (`setup/shared.ts:296`). R members follow the main's announced facts.
- App (`gpui/kuvo/src/fleet.rs`): `needs_update_first` (`:49`, `:224-227`), `retry_words`
  (`:115-118`), `Line::Failed`'s retry, and the per-row Update now; and in `title_bar.rs:661`,
  the Dismiss footer.

### The view (D10, no shots)

Computers A as locked, with these changes:
1. Chip: "Updating to beta.39 · 2 of 4 ready". It hides itself when nothing is needed, and comes
   back amber only for a fixable "can't update".
2. Progress bar: green (up to date or ready), blue (getting ready), grey (off), amber (can't
   update). No red.
3. Rows: `✓ beta.39 · Up to date`, `◌ Getting beta.39 ready…`, `✓ Ready for beta.39`,
   `● beta.38 · Off: updates when it's back`, `▲ beta.38 · <reason>` with **Try Again** (only if
   fixable), `● beta.38 · <reason> · Updates with the next release`.
4. Removed: per-row Update now, Show…, "needs an update first", "Next retry in", Dismiss and its
   footer line. Open Computers stays.
5. The popover headline says the reason only for a fixable cause.
6. Sidebar short words: "Can't update" (amber) only when fixable. Nothing else.

The box states the facts (`state`, `reason`, `fixable`, the ready count). The app only puts
them into words.

### Tests (each fails on today's code)

Disposable homes in `box/bridge-tests`, on macOS locally and on Linux on devbox, with test clocks
and no sleeps.

| # | Scenario | Asserts |
| --- | --- | --- |
| T1 | Four homes, one stopped; release N → N+1 with a layout bump. | Three switch, and the main first. Nothing is recorded as failed. No request is refused at any step. The stopped one, started, switches to N+1. |
| T2 | Crash before and after the `bin` rename; a new version whose `kuvo/info` doesn't answer. | Every part runs one version. The broken version is switched back by the keeper and shown as can't update. |
| T3 | One member's prepare fails (small disk; a release whose `box route --internal` fails). | Nobody switches. Try Again after the fix completes it. Past 24 h on the test clock, the others switch without it. |
| T4 | An engine turn and a held agent turn are running at the switch. | The held turn carries on. The engine turn continues by itself. |
| T5 | From 0.7.2 (the #713 fixture) and a machine home set up by 0.7.2. | Both end on R with no question. |
| T6 | From beta.35, with the real signed pack, one beta.35 member off. | Everyone ends on R. The off member catches up when it's started. |

From now on, every update test includes coming from 0.7.2 and from beta.35.

### Build plan (session wall-clock)

| Step | What | Tests | Wall-clock |
| --- | --- | --- | --- |
| 0 | Unlock (in progress, another session) | (its own) | n/a |
| 1 | The `bin` link: conversion, staging into `versions/`, the switch, the start guard in the keeper (macOS and Linux), `box route --internal` added to prepare, continue for cut turns | T2, T4 | 45-75 min |
| 2 | The fleet rule: `staged`, `cannot` and `release` facts; the main switches when all that are on are staged (or after 24 h); members follow the main's version; the main takes a release any member holds; the removals above | T1, T3 | 60-90 min |
| 3 | The view: box summary states, app words and buttons | gpui tests of what's drawn | 20-40 min |
| 4 | Older versions: the critical appcast entry, the old choice record kept, the 0.7.2 and beta.35 tests, a devbox rehearsal; PROTOCOL.md, `docs/footprint.md` (`versions/`), CHANGELOG | T5, T6 | 45-75 min |

Total: about 3-5 hours of session wall-clock over 4 sessions. Step 3 can run beside step 2 (the
facts are fixed here). Step 4 comes last.

### Cuts from the first draft, and why each is safe

| Cut | Why it's safe |
| --- | --- |
| **The side-by-side trial start** (the new engine on a database copy, the bridge on a scratch home, a layout dry run) | Signature, hashes, `selftest` for link and bridge, and `box route --internal` catch what can be caught without switching, and they're the checks today's updater runs plus the 11-21 call. Whatever only fails for real is caught by the start guard, which switches back by itself. Never locked, nothing manual. |
| **The switch journal** (`run/switch.json`, its steps, finishing at next start) | The `bin` rename is the only step that changes what runs, so there's nothing in between to finish or undo. Layout steps already checkpoint themselves. `run/previous` is the only record, and only the start guard needs it. |
| **Coordinator states and messages** (prepare, ready, cannot, commit, abort, round files, the signed commit record, gossip, deadlines) | The facts every computer already announces carry `release`, `staged` and `cannot`, and "commit" is the main running the new version. Abort isn't needed because prepared computers hold nothing. Gossip isn't needed because no member is ever stuck waiting for the commit: until it sees the main's version, it's whole and usable on the old one. |
| **The drain** (holding new turns, waiting up to 5 minutes) | Holders carry agents' turns already. Other cut turns continue by themselves. The switch takes seconds, like any restart, and the app already re-sends a message while a box comes back. |
| **The compatibility lane** (old clients against every new release) | Step 0 removes every version refusal. The capability gate and the format versions are existing, tested mechanisms. A gap would show up as a feature not shown, not as a lockout. |
| **The app in the vote** (D6 no) and the beta channel in the box (D9) | Without refusals, the app's timing doesn't matter. Betas reach the fleet through the app's bundled release, which the main can now take from any member. |
| **Any member as a source for members** | Members keep pulling from the main or the standby. A member that can't reach either just stays whole on its version until it can (rule 4 is about not blocking others). |
| **A new forced install for 0.7.2 machines** | It already exists ("For released Macs"). Only the test is new. |
| **The engine on a database copy, `previous` kept longer, the "switching" state, the conversation's waiting words, Settings' Check for Updates** | None of the five rules needs them. Each was either a convenience or something already covered above. |
| **Phone, kuvo-web pages, reviews** | Dropped by the lead. In-repo docs (PROTOCOL.md, footprint, changelog) stay in step 4, as CLAUDE.md requires. |

What we accept by cutting: the microsecond conversion window (above), and that a computer whose
main is off doesn't update until the main is back. That's by design: the main is the one that
decides.
