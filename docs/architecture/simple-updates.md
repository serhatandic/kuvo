# Simple updates: prepare everywhere, then switch together

Status: design, for the user's decisions (2026-11-21). No code has changed. Written after the
beta.35 → beta.38 update of 11-21 locked the user out of their own app. The traced flow is in
the lead's notes (`update-flow-1121.md`); the facts it relies on are cited below with file:line
at main `1b44ea971`.

---

## Part 1, in plain words

### What went wrong on 11-21

You updated four computers: main-linux (main, Linux), mac-mini (Mac), macbook (Mac)
and spare-linux (Linux, offline since 05:43).

- **One computer that was off held up the other three.** Before updating, each computer waited
  for *every* computer to say it could read the new folder layout, and spare-linux couldn't answer.
- **Waiting was reported as failing.** "Waiting for spare-linux" was saved as "Update failed", with a
  retry in an hour, then in a day.
- **Kuvo shut you out of your own computers.** As soon as a computer was behind the release you
  chose, it refused your app ("This computer needs to update to 0.8.0-beta.38 before it can be
  used", "Couldn't load bots"). It did this during its 5-minute health check too. The app had a
  second refusal list of its own, and remembered it after a restart.
- **Update Now hit the same walls**, so it couldn't help.
- **Each computer was half updated.** Kuvo's parts (the link, the server for your devices, the
  coding agents' bridge, the engine, the browser) each switched at their own time. The bridge
  waited for a quiet moment, so mac-mini ran beta.35's bridge beside beta.38's link.
- **A one-character bug** in beta.38 broke Kuvo's internal calls (tools, health checks,
  wake-ups). It's fixed on main (`1b44ea971`). Nothing stopped that release from reaching your
  computers.
- **Two computers showed "needs an update first" with nothing to press but "Show…".**

### The new way

An update works like a vote that must be unanimous.

1. **Get ready.** Your main computer asks every computer that's on to get the new version ready.
   Each one downloads it, checks its signature, sets the whole new version up next to the old
   one, and tries it out on the side (it starts it and runs its checks) without switching. You
   keep working the whole time, on the version you have.
2. **Switch, all together.** Only when every computer says "ready" does the main computer say
   "switch". Each computer then switches **everything in one step**: it is entirely the old
   version, then entirely the new one, never part of each.
3. **Or don't switch at all.** If any computer can't get ready, nobody switches. Everyone stays
   on the old version, working fully, and you see why in plain words. Nothing is "failed" and
   nothing locks you out.

That's all there is. There's no health-check period that locks you out, no retry timers, no
"Update Now" to rescue a stuck update, and no rollback to manage: a version that can't work is
caught while getting ready, before anything changes.

**A computer that's off doesn't hold anyone up.** It's left out of the vote. When it comes back,
the first thing it does is fetch the version everyone else switched to and switch, before it
does anything else with your other computers.

**Your app never refuses you.** At every moment (getting ready, switching, a computer behind,
a computer off) the app works with every computer. A feature a computer doesn't have yet simply
isn't shown there until it switches.

**Your running sessions keep running.** Coding agents' turns carry across the switch, as they
already do across restarts. Messages you send during the few seconds of a switch wait and then go.

**The app joins the same vote.** On a Mac, the app downloads its new version during "get ready"
and restarts into it at "switch", together with that Mac's Kuvo.

**From 0.7.2 and older:** a computer with Kuvo older than 0.8.0 is updated to the current
version automatically when it's found. You aren't asked first.

### What you'll see

- While computers get ready: "Updating to beta.39 · 2 of 4 ready". Nothing for you to do.
- For the few seconds of switching: "Switching to beta.39…". Then the notice goes away.
- A computer that's off: "beta.38 · Off: updates when it's back". Nothing for you to do.
- A computer that can't update for a reason you can fix: the reason and **one** button that
  works, e.g. "Not enough space on spare-linux (needs 1.2 GB) · Try Again".
- Never: "Update failed · Next retry in 1 day", "needs an update first", or a computer you
  can't use.

---

## Part 2, the technical design

### (a) The model

#### Roles

- **Coordinator: the main computer** (`setup.primary`). It starts each round, collects the
  answers, and decides. It's already the only computer that chooses a release
  (`update.ts:659`, `update.ts:1495`).
- **Participants:** every member that's reachable when the round starts (relay presence, or
  mail heard in the last 2 minutes), the main included. On a Mac where the app is running, the
  app is part of that Mac's participant (see (c2)).
- **Left out:** members that aren't reachable when the round starts, or that stop answering
  during it. They catch up later (see (c1)).

#### What starts a round

The main starts a round when it learns of a release newer than the fleet's committed one:
- its own feed check (`boxes.json` every 6 h, 3 days after release; `update.ts:58`, `:87`);
- a Mac's app that Sparkle found a newer version for (the box on that Mac tells the main;
  the release itself comes from the same feed, which publishes `boxes.json` beside the appcast);
- **Check for Updates** on any client, which asks the main to look now, without the 3-day wait.

Only a signed release (`verifyBoxes`, `update.ts:144`) newer than the committed one, with
`spreads: true`, is ever chosen. Downgrades stay impossible.

#### Prepare (on each participant)

Prepare changes nothing that's running. A participant:

1. **Fetches** the signed manifest and pack from any member that holds it, the main first.
   Trust comes from the signature, not from who sends it, so the main-or-standby-only rule
   (`fleet.ts:273`) goes. The engine comes from its manifest URL as today.
2. **Verifies** the signature, the pack's SHA-256, every file's SHA-256 (`update.ts:709-774`),
   and the engine's integrity.
3. **Stages the whole version** at `~/.kuvo-code/versions/<version>-<sha12>/`: every part Kuvo
   runs on that computer (`bin/*` (link, bridge, browser, keeper scripts, wake, npx,
   `runtime/node`), the engine's plugin, the engine binary, the catalogs, and on a Mac the
   Kuvo Helper bundle). `installedLinkBuild` and `box.json` describe it as today.
4. **Tries it out on the side**, in a scratch copy of the home's small state (`run/link.json`
   and the other versioned records) and a copy of the engine's database:
   - the new engine starts on the copy, migrates it, and answers `api/session?limit=1`;
   - the new link passes `selftest` on that engine and on the running engine (`update.ts:813-820`);
   - **the new link answers `box route --internal kuvo/info`** with every feature the current
     one has (the call broken on 11-21; `health.ts:100` already uses it);
   - the new bridge starts against the scratch engine and answers `api/session/active`;
   - the folder-layout steps up to the new layout run on the scratch copy
     (`folder-version.ts`, `upgradeKuvoFolder`);
   - `sh -n` on the new keeper; on a Mac, `codesign --verify` on the helper bundle;
   - there's room for the switch (the database copy the switch keeps, plus a margin;
     `roomForData`, `update.ts:453`).
5. **Answers** `ready {round, version, sha256}`, or `cannot {round, reason, fixable}`, where
   `reason` is plain words and `fixable` says whether the user can fix it (space, permissions)
   or not (the release doesn't run here).

A participant that's ready promises it can switch without any further download or check.

#### Decide (on the main)

- **All participants ready → commit.** The main writes a signed **commit record** to its own
  disk first (fsync), then sends it out: `{setup, epoch, round, version, sha256, signed manifest,
  participants, at}`, signed with the setup's primary key.
- **Any participant answers `cannot`, or is reachable but not ready at the deadline (30 min) →
  abort.** The main sends `abort {round}`. Nobody switches. Staged files stay for the next
  round. The main starts a new round once the computer that couldn't says its cause is gone
  (it re-checks on its own; see Try Again in (f)), or a newer release appears. (Decision D3
  covers a computer that keeps failing.)
- **A participant stops answering mid-round** → it's dropped from the participants (treated as
  off), and the round goes on without it.

#### Commit (on each computer): one switch

Every Kuvo part is started through one pointer, `~/.kuvo-code/current`, a symlink to a version
folder. The launch entry (`kuvo-launch`, the `@reboot` line or launchd agent) runs
`current/bin/kuvo-start`, and the keeper starts everything from `current`. The switch:

1. Write `run/switch.json {from, to, round, step: "draining"}` (durable).
2. **Drain:** new turns are held (queued and shown as waiting, never refused). Turns that
   survive a restart (coding agents under holders, `agents/hold/adopt.ts:16`) aren't waited
   for. Turns that don't (the engine's own turns, and agents whose holder can't be adopted,
   `adoptable()` at `adopt.ts:26`) get up to 5 minutes to end (decision D4).
3. `step: "stopping"`: stop every Kuvo process from the old version (link `run`, `serve`,
   box services, bridge (its holders stay), engine, browser keeper, wake). The keeper loop
   stays.
4. `step: "migrating"`: run the new version's folder-layout steps, with every writer stopped.
   These are checkpointed and replayable already (`folder-version.ts:106-135`).
5. `step: "flipping"`: create `current.new → versions/<to>` and `rename()` it over `current`.
   **This rename is the commit point.** It's atomic on macOS and Linux.
6. `step: "starting"`: start everything from the new version; held turns go to it.
7. `step: "done"`: the old folder becomes `previous` (kept for the start guard, deleted at the
   next switch); `switch.json` is removed. The computer reports `switched` to the main.

On a Mac, the Kuvo Helper bundle keeps its fixed path (its login item and macOS permissions are
tied to it), so it's swapped by rename in steps 5-6 under the same journal (as
`swapBundle`, `update.ts:972`, does today). The app restarts at step 6 (see (c2)).

**Crash or power loss mid-switch.** The keeper reads `run/switch.json` before starting anything:
- before the flip (`draining`, `stopping`, `migrating`): `current` is still the old version, and
  the staged one is intact. The commit is a fact of the fleet, so the keeper **finishes** the
  switch (steps 3-7 are idempotent). If the staged folder is damaged, it starts the old version
  and catches up (prepare again, then switch), the way a computer that was off does;
- after the flip: it starts the new version and finishes step 7.

So a computer is never left in between. It's whole on one version, and it's on the new one as
soon as it can be.

**Start guard (the one automatic safety net).** If the new version doesn't come up after the flip
(the link or engine doesn't answer within 2 minutes of running time, or a part restarts more than
3 times), the keeper flips `current` back to `previous` with the same rename, starts it, and
reports `cannot {reason: "beta.39 didn't start here", fixable: false}`. Going back is safe for
the code. For data, the 11-21 format rule says incompatible formats go to separate paths and
downgrades keep working (the lead's 848 decisions; `authority-first-readers.md`). For the engine
database, the existing `downgradeSafe` and the "the new engine wrote nothing" check apply
(`update.ts:400-405`). Prepare runs the same start on scratch copies, so this should almost
never happen. It replaces the 5-minute health trial and its lock.

#### The coordinator dying

- **Before it decides:** participants hold nothing. They keep running the old version, and the
  staged folder just waits. Nothing times out into a bad state. When the main comes back, it
  re-reads its round (`run/fleet-round.json`) and asks again; staged participants answer
  `ready` at once.
- **After it wrote the commit record:** the record is durable, and the main re-sends it at start.
  In the meantime, any computer that got it passes it on (below).
- **The commit spreads by gossip.** Every computer includes the newest commit record it holds in
  the facts it already announces to its peers (`announce`, `keep.ts:30`; `fleetFacts`,
  `fleet.ts:158`). A computer that sees a valid commit newer than its own version catches up.
  This one rule covers a commit message that got lost, a coordinator that died halfway through
  sending, and a computer that was off.
- **The main away for good:** no new rounds start. Everyone keeps working on the committed
  version. A new main (after the standby takes over by the existing rules, `takeover.ts`)
  coordinates the next round. Commit records are monotonic: any computer adopts the newest valid
  commit signed by a legitimate main of the setup (its key for that setup epoch), so two mains
  across a takeover can't disagree.

#### What changes in the release choice

The commit record replaces `setupSecurityAndReleaseChoice` as what members follow (`choice.ts:9`).
It is **not** a setup save that stays pending until the standby signs (`setup/shared.ts:296`,
`stageShared`): a standby that's off can no longer hold an update up. The old choice record keeps
being written, for one transition only, so older updaters can catch up (see (d)).

### (b) States

A **computer** is in exactly one of these. In every one of them the app works with it.

| State | Meaning | What the user needs to do |
| --- | --- | --- |
| **Up to date** | Runs the committed release. | Nothing |
| **Getting ready** | Prepare under way for a round; runs its current version. | Nothing |
| **Ready** | Prepared; waits for the others. | Nothing |
| **Switching** | The drain and the switch (seconds; at most the drain's limit). New messages wait. | Nothing |
| **Off** | Not reachable. Runs whatever it ran; catches up when it's back. | Nothing |
| **Catching up** | Back from being off (or left out): prepare and switch for itself, to the committed release. | Nothing |
| **Can't update** | Its prepare or start guard said `cannot`. Runs its current version, fully working. | If `fixable`: one action (Try Again, after the fix the words name). If not: nothing (a newer release is waited for). |

Gone: `behind` (with or without a lock), `failed` with `nextRetryAt`, `updating · waiting` with
no limit, the trial, and "needs an update first".

The **setup** is in one of these: *up to date*, *updating to X (n of m ready)*, *switching to X*,
or *up to date, except: …* (computers that are off or can't update).

The **app** has no update state of its own that blocks anything. A newer or older app works with
every box from 0.8.0 on (see (c3)).

### (c) Open questions, each with a recommendation

#### (c1) A computer that's off (spare-linux)

**Recommendation: it blocks nobody. It's left out, and when it's back it catches up before it
does anything else with your other computers.**

How it catches up:
1. On reconnecting, its link's first peer exchange is the facts announcement, which carries the
   newest commit record. It sees one newer than its version.
2. Before it processes any other peer mail (setup changes, moves, Work Queue, group chats,
   forwarded prompts), it prepares the committed release, fetched from any peer that holds it,
   and switches, using the same steps as a round of one. Peer mail waits unacknowledged and is
   delivered again after the switch (the existing "unknown format → not acknowledged" rule,
   PROTOCOL.md "Format versions during an update", already makes that safe).
3. Its own local work never waits: the app uses it, its sessions and bots run, and its wake-ups
   fire throughout.
4. If its prepare fails, it becomes **Can't update** and resumes peer mail on its old version.
   The wire format rules protect the data, and the reason is shown with the one action.

What the others do meanwhile: nothing different. Its row says "Off: updates when it's back". Work
sent to it waits in their outboxes as it does today for an offline peer. Features that need
every member to read something new (handover, standby, the checkpoint migration: `handover.ts:135-139`,
`takeover.ts:213-216`, `keep.ts:69`) stop asking "does every member say it supports F?". They ask
"does the committed release support F?" instead, because a member reads setup data only after
catching up to the committed release. That's what makes leaving it out safe.

The alternative, blocking everyone until it's back, is exactly 11-21. Not recommended.

#### (c2) The app

**Recommendation: the app is part of its Mac's participant. Sparkle downloads and verifies
during prepare. The fleet's commit, not Sparkle, decides when the app is installed.**

- **One version number** for the app and the box (they're released together; the bundle carries
  `Contents/Resources/box-release`).
- **Prepare:** the Mac's box tells the running app "stage X". The app has Sparkle check and
  download X (`gpui/kuvo/src/updates/sparkle.rs`), and holds the install. Today
  `willInstallUpdateOnQuit` leaves the install to Sparkle (`sparkle.rs:55-60`); this keeps the
  `immediateInstallationBlock` instead. The app tells its box it's staged, and that Mac answers
  `ready` once both its box and its app are staged.
- **Commit:** after its box's switch, the app calls the kept installation block. Sparkle
  replaces the app and relaunches it, and the app restores its windows and selection (drafts
  are the box's already). If you're typing (a keystroke in the last 5 seconds), it waits for 5
  quiet seconds, at most 2 minutes. The app's restart never holds the fleet.
- **An app that can't stage** (feed unreachable, a private build) doesn't block the fleet. Its
  Mac answers `ready` for its box alone. The app installs X at its next launch, before the main
  window opens: if it's older than its box's committed release at launch, it installs the staged
  update first (about 2 seconds).
- **The app not running:** nothing to wait for. At its next launch, as above.
- **The Kuvo bundled in the app** stops being a separate way to update a box. Today the app
  offers it to its own box at every launch (`launch.rs:494`, `offerUpdate`, taken at a quiet
  moment by `offerDue`/`takeOffer`, `update.ts:1559-1620`). In the new model it's only a source
  for the bytes: a box that needs X can take X's pack from the app's bundle instead of the
  network. It's also what puts Kuvo on a new Mac (first install, the 0.7 move). It never
  switches a box on its own.
- **"Get beta updates"** moves to the box as a setup-wide setting, so the fleet and every app
  follow one channel. Today it's the app's (`sparkle.rs:9`, `BETAS_KEY`), which is client logic
  the box should own.
- **Phones** (App Store, TestFlight) can't join a vote. They follow (c3).

#### (c3) Different versions, for the moments that remain

These remain: the seconds between computers switching, computers that are off or can't update,
an app quit before a commit, and phones that update on Apple's schedule.

**Recommendation: no version refusals at all, from 0.8.0 up. Features follow what each box
says it has, and the data formats keep data safe.**

- A box serves every app, phone and peer from 0.8.0 on. Nothing returns 426 or 503 because of a
  version: `clientUpdateRequired`, `computerUpdateRequired` and `legacyPeerUpdateRequired` go
  (see (e)).
- The app already draws by capability (`Gate::Behind`, "The update gate" in PROTOCOL.md: an
  older box is used as it is, without the features it lacks). The fleet-target refusal on top of
  that goes.
- Data is protected by the explicit format versions (848): an unknown format isn't
  acknowledged, and is delivered again once the reader has caught up. Owners take setup-wide
  decisions on their own version and accept older proposal shapes (`authority-first-readers.md`).
- **A rule for every release**, checked by a test lane (see (g)): its routes and peer mail still
  answer the previous releases' apps, phones and boxes back to 0.8.0, and anything a newer
  version can't serve to an older client is shown there as "needs a newer Kuvo Code", never a
  dead end.
- The only floor is 0.8.0 (see (d)).

#### (c4) Smaller ones

- **D3: a computer that keeps failing to get ready.** Recommendation: two aborted rounds, or
  24 hours, caused by the same computer → the next round leaves it out (like an off computer),
  and it shows *Can't update* with its reason. One broken computer can't freeze the fleet for
  good, and a release problem still stops everyone the first time.
- **D4: how long the drain waits** for turns that can't carry across a restart. Recommendation:
  5 minutes. Then the turn is stopped at a step boundary, and the box sends it "continue" on the
  new version (the queue already pauses runs a restart cut off, `engine/queue.ts:5-7`; this
  resumes them instead).
- **D5: the release's own check before publishing.** Recommendation: `scripts/box-release.sh`
  runs the same prepare self-checks (above all `box route --internal`) on Linux and macOS, so a
  bug like 11-21's can't be published at all.

### (d) Older versions

The model arrives in one release, called **R** here. Older updaters can't run R's rules, so R is
installed by **R's own installer code** wherever possible: `kuvo-install update` runs the
release's own `kuvo-link.js update --installer` (`box/src/install/kuvo-install:246-248`).
R's update path has no `layoutHold`, so an older computer's gates don't apply to it.

| Coming from | How it gets R | From then on |
| --- | --- | --- |
| **0.5 to 0.7.2 Mac app** | Sparkle in 0.7.2 installs the 1.0 desktop app, with the same bundle ID (#713 spike). R's appcast entry is marked `sparkle:criticalUpdate` for versions below 0.8.0: this is the forced update. At first launch, the app's move (rehome, #713) puts this Mac's Kuvo in its own home with R's installer. | The new model |
| **A machine set up by 0.5 to 0.7.2** (no self-update) | **Forced:** the first box with R that reaches it (Add a Computer, Set Up Again, or the app on its Mac) installs R over SSH with R's installer, before it joins or is used. You aren't asked; it says "Updating Kuvo on spare-linux from 0.7.2…". | The new model |
| **0.8.0-beta.1 to beta.2x (before fleet saves)** | Their updater takes the newest release a peer holds or GitHub publishes (compat release announcements). R is that release, installed the old way once. | The new model |
| **beta.3x to beta.38 (fleet saves, `layoutHold`)** | Macs: the R app (Sparkle) runs R's installer for its own box (`kuvo-install update`, not `POST kuvo/box/update`, which would use the old code's gates). Linux: the main gets R by Set Up Again over SSH from a box with R, the bootstrap PROTOCOL.md already gives for beta.34. Then the main, on R, commits R. Members still on old code catch up with their old updater: it reads the old choice record, which R keeps writing for this one transition, and its `layoutHold` passes, because everyone else now announces R's layout. | The new model |
| **The 11-21 fleet** (main-linux and mac-mini on hand-patched beta.38, macbook on beta.35, spare-linux off on beta.35) | mac-mini and macbook: the R app. main-linux: Set Up Again from mac-mini over SSH. spare-linux: when it's back, its beta.35 updater takes R from the old choice record (the others then announce R's layout). | The new model |

COMPAT items R adds, each with its end: the old choice record, written beside the commit record
(goes once no member is below R), and reading `bin/` paths for a 0.7 Mac that still writes
there (`checkDrift`, `update.ts:1122`, already COMPAT(0.7 visits)).

### (e) What gets removed

Box (`box/src/link/updates` unless named):
- `layoutHold` and its uses: `choice.ts:38-51`, `update.ts:719-721`, `:854-855`, `:1510-1515`,
  `areas/setup.ts:213`, `setup/shared.ts:213-225` (`membershipLayoutAdmission`: a joining
  member is brought to the committed release as part of joining instead).
- "Every member supports F" gates for features the committed release has: `keep.ts:69`,
  `setup/handover.ts:135-139`, `setup/takeover.ts:213-216` (they become "the committed release
  supports F").
- "Blocked = failed" and the retry backoffs: `update.ts:606-614` (`releaseResult`),
  `:1582-1587` (`recordOfferFailure`), `:1613-1614`, `:1589-1596` (`noteInterruptedOffer`,
  replaced by the switch journal), `:99-100` (`OFFER_REST`, `OFFER_RETRY`), `:1435-1461`
  (`offerFailedAgain`, `offerFailedTwice`, `offerRetryAt`, `offerRests`), `attempt.ts:35`
  (the transfer backoff up to an hour; prepare retries sources within its round), `fleet.ts:45`
  (`TRIED_FOR`).
- The health trial and its lock: `health.ts` (`settleUpdate`, `:212-298`; the five-minute
  window, `:61`), the trial marker and `ON_TRIAL` refusals (`update.ts:60`, `:301`, `:592`,
  `:1416`), `confirmUpdate` (`:1074`) and `keepForRollback`/`restore.sh` (`:337-394`). The
  start guard in the keeper and the `previous` folder replace them.
- File-by-file swapping: `update.ts:886-910` (replaced by staging plus the pointer flip).
- The separate, later engine change: `engine-swap.ts` (581 lines) and `engineApart`
  (`update.ts:511`). The engine switches with everything else.
- The bridge's quiet-moment wait: `box/src/agents/renewal.ts` (299 lines). Holders carry
  turns, and the switch's drain covers the rest.
- The offered-release path as a way to update: `offerUpdate`/`offerDue`/`takeOffer`/
  `startOfferedUpdate` and `updates/offered` (`update.ts:1463-1671`), plus the `boxUpdateRoute`
  (`:1675`), which stays only as a way to hand bytes over.
- Update Now as an escape: `requestUpdateNow` (`update.ts:1340`), `requestFleetRetry`/
  `retryFleetRoute` and `fleet-retry.json` (`progress.ts:161-181`, `attempt.ts:46-69`). See (f)
  for what replaces it.
- The release choice pending on the standby's receipt (`setup/shared.ts:296` for this kind).
- Refusals by version: `client.ts` entirely (`clientUpdateRequired` `:6-18`,
  `computerUpdateRequired` `:26-36`, `legacyPeerUpdateRequired` `:39-49`), and their callers
  `server/router.ts:338-348`, `server/ssh-route.ts:44`, `documents/phone-sync.ts:175`,
  `ownership/messages.ts:259`, `:298`, `:352`, `ownership/edits.ts:181`, `:449`,
  `work-queue/hand.ts:192`, `:325`, and `work-queue/forward.ts`.
- `progress.ts` states `failed`, `behind` with `canRetry` and `nextRetryAt` (`:72-90`), and the
  dismissal store (`:118-159`; the notice hides itself when nothing's needed).
- The main-or-standby-only pull (`fleet.ts:273`, `releaseAuthority`).

App (`gpui/`):
- The fleet refusal: `kuvo_box/src/client.rs:325-338` (in `check_gate`), `gate.rs:68-96`
  (`fleet_allowed`), `boxes.rs:299-318` (`fleet_target`, `fleet_admission`), the saved holds
  `boxes.rs:529-552` (`fleet_holds` in the kept file, ignored on read from R on), and
  `transport.rs:163`.
- `kuvo/src/fleet.rs`: `needs_update_first` (`:49`, `:224-227`), `retry_words` (`:115-118`),
  `Line::Failed` with its retry, and `can_update`.
- `kuvo/src/title_bar.rs:661`: "Dismiss hides until done; failures bring it back."
- Sparkle's own install-on-quit and prompt as the way the app updates (see (c2)).
- The phone: the same refusals and lines in `swift/KuvoClient` and `../kuvo-code-ios`. Tell
  the phone session (see (h)).

Docs: PROTOCOL.md "Updates", "Fleet versions", "Admission" and the parts of "The update gate"
about fleet targets, rewritten to match.

### (f) The update view

The rule: every state shown either says nothing is needed, or gives one action that works.

The locked **Computers A** draft (title-bar chip with its popover; the Computers page's
"Computers & updates" rows) stays as it is: the same layout, rows, segmented progress bar and
Open Computers. These are the minimal changes, **for your OK**:

1. **Chip words.** "Updating to beta.39 · 2 of 4 ready" while getting ready (it counts *ready*,
   not *done*), then "Switching to beta.39…". The chip **goes away by itself** when everyone is
   up to date, or when the only ones left are off. It comes back amber only if a computer
   **can't update for a reason you can fix**: "spare-linux can't update to beta.39".
2. **Progress bar segments:** ready and up to date (green), getting ready and switching (blue),
   off (grey), can't update (amber). No red.
3. **Row lines** (popover and Computers page):
   - `✓ beta.39 · Up to date`
   - `◌ Getting beta.39 ready…` / `✓ Ready for beta.39` / `◌ Switching to beta.39…`
   - `● beta.38 · Off: updates when it's back` (grey, no button)
   - `▲ beta.38 · Not enough space on spare-linux (needs 1.2 GB)` **[Try Again]** (amber)
   - `● beta.38 · Will update with the next release (beta.39 doesn't run here)` (grey, no
     button), for a release that can't run there; nothing you can do, so nothing to press
4. **Buttons.** Per-row **Update now** goes, and so do **Show…** and "needs an update first".
   The only per-row button is **Try Again**, shown only for a fixable *Can't update*. It re-runs
   that computer's prepare, and if it's ready, the main commits it (or the round resumes).
5. **Footer.** Remove "Dismiss hides until done; failures bring it back." and **Dismiss** (the
   chip hides itself). Keep **Open Computers**.
6. **Headline under the title.** Today: "mac-mini failed its update." It becomes a plain line
   only where it helps: "Everything works on beta.38 while your computers get ready." during a
   round, and "spare-linux needs 1.2 GB more space to update." for a fixable cause.
7. **Settings › Updates**: one **Check for Updates** button (it asks the main to look now). If
   the main is off: "beta.39 is ready. It installs on all your computers when main-linux (your main
   computer) is back."
8. **Conversation:** a message sent during a switch shows "Sends when Kuvo finishes updating (a
   moment)", the same waiting look as queued messages. The red "Not sent. This computer needs to
   update…" goes.
9. **Sidebar short words:** "Updating…" only while switching. Nothing for off computers (they
   show as off already), and "Can't update" in amber only when fixable.

The phone gets the same chip, rows and words. Light and dark shots of these will be drafted in
the view step, before it's built (the CLAUDE.md drafts rule), with Computers A as the base.

Where this logic lives: the box states facts (`state`, `reason`, `fixable`, `readyCount`,
`participants`, `round`). The app and the phone only put them into words. "Fixable" and the
counts are the box's, not worked out in a client.

### (g) Tests that would have caught 11-21

All in `box/bridge-tests` (or a new `fleet-rounds.test.js`) with disposable homes: real boxes in
folders of their own, connected by the test relay. They're run on macOS locally and on Linux on
devbox (`scripts/test-box.sh bridge <filter>`). They're deterministic: test clocks and hooks
(`KUVO_TEST_*`, as `health.ts:63-66` does), no sleeps. Each must fail on today's code, for the
reason given.

| # | Scenario | Asserts | Fails today because |
| --- | --- | --- | --- |
| T1 | Four homes (main Linux, a Mac-kind box, two Linux), one stopped. Release N → N+1 with a layout bump. | The three switch. The stopped one stays N. Nothing records `failed`. Then started: it switches **before** it acknowledges any other peer mail. | `layoutHold` (`choice.ts:40`) blocks all; blocked is recorded as failed (`update.ts:612`) |
| T2 | A client that sends the desktop's real requests (sessions, bots, chat, Work Queue) to every home at every step of T1, T4 and T5, and with an app header one release older and one newer. | No 426 or 503 that names a version, at any step. | `computerUpdateRequired` (`client.ts:26`) during `behind` and the trial; `clientUpdateRequired` 426 |
| T3 | Crash at each switch step (`KUVO_TEST_SWITCH_CRASH_AT=draining|stopping|migrating|flipping|starting`), then the keeper starts. | Every part runs one version (link, bridge, engine, plugin, browser keeper, keeper), and it's the new one. | Files swap one by one (`update.ts:893`); the bridge stays on the old build until a quiet moment (`renewal.ts`) |
| T4 | The main is killed after all are ready, before it commits; then again after it signs the commit but before it sends it to all. | First: members stay N, fully working, and the round finishes when the main returns. Second: members that missed the commit get it from a peer and switch. | There are no rounds; a pending choice waits on the standby (`setup/shared.ts:296`) |
| T5 | One online member's prepare fails (a small disk; a release whose `box route --internal` fails). | Nobody switches. Its row is *Can't update* with its reason, and **Try Again** works after the fix. No "retry in". | It's "failed" with a 1 h or 1 day backoff; the bad release is installed and then rolled back by the trial |
| T6 | From **0.7.2**: the #713 0.7.2 Mac fixture, and a machine home set up by 0.7.2. | The app move and the forced install over SSH bring both to R, and they join with no question asked. | No forced install; the 0.7.2 machine waits for a Mac to connect |
| T7 | From **beta.35**, with the real signed beta.35 pack: a fleet with one beta.35 member off. R is bootstrapped (installer on the main; the app's installer on a Mac); the off member comes back. | Everyone ends on R, and the beta.35 member catches up with its own old updater. | `layoutHold` in the old updater blocks while any member is off |
| T8 | A coding agent (held) and an engine turn are running at commit. | The held turn carries on without a break. The engine turn finishes within the drain or is continued on the new version. Messages sent during the switch go after it, each once. | The switch waits for a quiet moment with no limit, and the bridge and engine switch at other times than the link |

T6 and T7 are also the update lane the user asked for: **every update test from now on includes
coming from 0.7.2 and from the oldest supported beta**. Rehearsal on real computers (devbox homes,
and then the user's own four computers on their word) closes step 7.

### (h) Build plan

Each step is one session, merged into local main at its end with its tests. Where new logic
lives: all in `box/`, except the drawing (step 6) and the app's Sparkle hook (step 6), which only
an installed app can do. Steps 3 and 4 are high risk (cross-computer, data): one `reviewer` pass
plus a separate review the lead arranges.

| Step | What | Tests | Estimate |
| --- | --- | --- | --- |
| 0 | **Unlock now (optional, see D1).** Before R: remove the version refusals (box `client.ts` callers, app `fleet_target`/`fleet_holds`), stop recording "blocked" as "failed", and leave members that are off out of `layoutHold`. Shipped as a beta. Stuck computers get it by the installer bootstrap from (d). | T2 (part), T1 (part) | 3-4 h |
| 1 | **Versioned folders, pointer, switch journal, start guard** (keeper on macOS and Linux; crash recovery; the helper bundle by rename). A single computer updates this way. | T3 | 8-12 h |
| 2 | **Prepare**: stage everything, plus the side checks (scratch engine and DB, `box route --internal`, bridge, layout dry run, room). The same checks in `box-release.sh` (D5). | T5 (single computer), T8-release check | 6-8 h |
| 3 | **Fleet rounds**: coordinator, `prepare`/`ready`/`cannot`/`commit`/`abort` mail, the signed commit record and gossip, catch-up-first on reconnect, any member as a source, "committed release supports F" in place of every-member gates. Removes `layoutHold`, blocked = failed, backoffs, retry, offered-path updating. | T1, T4, T5 | 12-16 h |
| 4 | **Drain and no refusals**: one box-wide drain at the switch (D4, with continue for cut engine turns), removing `renewal.ts`'s wait and `engine-swap.ts`; removing `client.ts` and its callers; the compatibility lane (c3). | T2, T8 | 8-10 h |
| 5 | **Older versions**: R keeps writing the old choice record; the app uses `kuvo-install update` for its own box; the forced install for below 0.8 in Add, Set Up Again and join; the critical appcast entry. | T6, T7 | 8-10 h |
| 6 | **App and phone**: drafts of the view changes in (f), light and dark, for the user; then the app's side (gates gone, Sparkle staged and installed at commit, chip, rows, Settings › Updates, the beta channel read from the box). The phone session gets the same brief. | gpui view tests of what's drawn per state; the phone's `--from-batch` run | 8-10 h, plus the phone's own |
| 7 | **Docs and rehearsal**: PROTOCOL.md, `docs/footprint.md` (`versions/`, `current`, `previous`), kuvo-web's updates and troubleshooting pages, CHANGELOG; a rehearsal from 0.7.2 and beta.35 on devbox homes and a Mac test folder; then the user's fleet, on their word. | Rehearsal checklist | 4-6 h |

Total: about 57-76 hours, 8 sessions (plus the phone). Steps 1 and 2 can go in parallel; step 3
needs both.

### Decisions for the user

| # | Decision | Recommendation |
| --- | --- | --- |
| D1 | Ship step 0 (unlock now) before the full build? | **Yes.** It removes the lockout within a day. Everything else takes about two weeks. |
| D2 | A computer that's off: left out, and catches up first when it's back? | **Yes** (c1) |
| D3 | A computer that keeps failing to get ready: left out after 2 rounds or 24 h? | **Yes** (c4) |
| D4 | The drain for turns that can't carry over: 5 minutes, then stop and continue on the new version? | **Yes** (c4) |
| D5 | The release build runs the prepare checks before publishing? | **Yes** (c4) |
| D6 | The app joins the vote: Sparkle stages during prepare, and the app restarts itself at commit (waiting for 5 quiet seconds while you type)? | **Yes** (c2) |
| D7 | No version refusals from 0.8.0 up, and every release keeps serving 0.8.0+ clients and peers? | **Yes** (c3) |
| D8 | Below 0.8.0: forced update with no question (a critical appcast entry for Macs, an automatic install for machines)? | **Yes**, as you decided; listed to confirm the wording "Updating Kuvo on spare-linux from 0.7.2…" |
| D9 | "Get beta updates" becomes one setting for the whole setup, kept by the box? | **Yes** (c2) |
| D10 | The view changes 1-9 in (f) to Computers A | For your OK; shots come in step 6 before building |
