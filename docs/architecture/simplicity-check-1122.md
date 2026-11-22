# Simplicity check, 22 November 2026

Read-only check of `5ae7178f3` against the user's goal: one page, one mechanism per concern,
nothing left to remove, sized for about 1,000 users with one or two trusted computers. Line numbers
are at this commit. Short box paths start at `box/src/link/`. The page it checks is
`how-kuvo-works.md`.

## In plain words

**The trim worked.** One lease at the relay picks the main. The main saves alone. Updates follow
the main and switch in one step. A move needs only its two computers. One process answers clients.
No votes, standby, ranks or waits on every computer are left on the update path or the main's
saves.

What's left falls into three groups:
1. **A few things users can hit.** Most are in the desktop app, which still decides some things the
   box should (findings 1, 4, 5, 6). Two are in the box: a computer on its own needs the relay to
   save setup-wide settings (2), and the ownership lookup can't find older conversations (3).
2. **Old mechanisms still running beside the new ones**, mostly to serve computers and setups that
   haven't converted yet (8-16). No user sees them, but each one keeps a concern at two mechanisms.
   Nearly all of them go by deletion once 0.8.0's forced update has run, which is trim step 7.
3. **Protection against a lying computer.** Some layers guard against one of the user's own
   computers lying. That isn't a threat here, since the user's computers are trusted (17-19).
   Simplify these when the code is next changed.

On the question asked: **"my own record, else ask the main computer"** is half right. The first
half should be the rule. The second half would undo trim step 5. The answer is in the next section.

---

## The ownership lookup ("where does this live?")

`ownerOf` (`ownership/resolve.ts:211-257`) checks, in this order:
1. Shortcuts for client-owned and main-owned data (`:216-230`).
2. What other computers announce. This applies when exactly one computer that has
   `moves.two-computers` lists the record, unless this computer's own accepted record says it's
   here (`:238-241`; fix 16efd5c1a moved that check first).
3. The "accepted" owner (`acceptedOwner`, `:105-126`). That is this computer's move journal
   (`localMoveOwners`), else the setup-wide `ownershipAssignments` that nothing writes any more
   (`setup/shared.ts:173-178`), else `membership.moved`. Each answer is then followed through
   `replacement:` chains for restored computers (`:38-52`).
4. A move switching away from here, a legacy move notice (`movedOf`, `COMPAT(beta.1)` in
   `peers/known.ts:212-223`), or a recorded departure (`:247-251`).
5. `localRecord`, which the **caller** sets after checking its own stores (`:252`). The callers
   are `ownership/read.ts:257`, `areas/bots.ts:66`, `peers/mail.ts:129` and `services.ts:30`.
6. Any computer's announcement, whatever its version (`:253-255`).
7. The computer the request named (`:256`).

That's three problems:
- **"Is it here?" is answered outside the lookup.** `readOwned` calls `ownerOf`, tries a local
  read, then calls `ownerOf` again with `localRecord` (`ownership/read.ts:257-267`). The router
  calls it without `localRecord` and treats "don't know" as "here" (`server/router.ts:385`,
  `:413-430`).
- **Announcements are partial.** A computer announces its bots, its **40 most recently updated
  sessions** and 100 group chats (`peers/agents.ts:93-98`, `:198`), cut again to fit one mail
  (`:209-215`). Only computers that an older conversation moved through can find it. Anywhere else,
  the router runs the request locally, where it fails as not found, instead of passing it to the
  owner. Users rarely meet this today, because the desktop sends each conversation's requests to its
  own computer. It hits passed-on requests: an agent's tools, the Work Queue across computers, and
  a client still pointed at the old computer.
- **Three sources belong to removed mechanisms:** `ownershipAssignments`, `membership.moved` and
  the legacy move notices.

**Could it be "my own record, else ask the main computer"?** The first half, yes. The second
half, no. Since trim step 5 the main doesn't record where anything lives, and that's what lets a
move need only its two computers. To answer, the main would have to gather every computer's
announcements, which the asking computer already has. Every passed-on request would also fail
while the main is away, which brings back the dependence trim 5 removed.

The rule that fits, in three steps:
1. **Here,** if this computer has it. The lookup asks this computer's own index itself, across
   engine and bridge sessions, bots, groups and queue items. Callers don't decide it.
2. **Else where this computer's own move journal sent it.** That covers a move in flight, and the
   next computer's journal covers a later hop.
3. **Else the one computer that says it has it.** Announcements list every record's ID (IDs only,
   compared by digest as they are now), not just the 40 newest.

Remove:
- the `ownershipAssignments` reads (keep `ownershipRecovery` for restored computers);
- `membership.moved` and `movedOf` as locators;
- the `moves.two-computers` filter, once every supported version has it;
- `legacyMoveAccepted` (`resolve.ts:96-102`) and its six callers in `moves/move.ts`;
- the `checkpointed` branch in `mayActivate` (`:169-170`);
- the `localRecord` parameter, and the second call in `read.ts`;
- the named-computer fallback.

**Cost to the user:** none. Older conversations become findable. One session.

---

## Findings, ranked by what users would hit

Each finding gives the code, what it costs the user, and the simplest fix, removing first.
Findings 1-7 are on paths users take. Findings 8-16 cost no user anything they'd see, but each
keeps a concern at two mechanisms. Findings 17-19 are built for a threat the user ruled out.

### Paths users take

**1. Remove may offer only "Wipe This Computer" for a computer that isn't the last one.**
- *Code:* the app decides the computer is the last one from `computer.here && led().is_none()`
  (`gpui/kuvo/src/computers_manage.rs:263-268`). `led()` is `None` whenever the `kuvo/setup` read
  hasn't arrived or failed (`:109-111`). The sheet then says "Remove your last computer? … Unlink
  alone isn't available" and offers only wipe (`:598-623`).
- *Cost:* the user can wipe this Mac when they meant to unlink it from a setup of several
  computers. They're asked first, but the question is wrong. It's rare (it needs a missing setup
  read), and it can't be undone.
- *Fix:* the box's computer list says, per computer, which removals it allows (unlink, wipe,
  last). The sheet draws that list, and offers nothing while it's missing. Small.

**2. A computer on its own can't save setup-wide settings without the relay.**
- *Code:* a main without a fresh relay lease is fenced (`setup/lease.ts:73-78`), even in a setup
  with no other computer. Saves return nothing (`setup/shared.ts:225`). A personality or time zone
  edit sits as "Saved here. Waiting for the current owner to accept it."
  (`ownership/edits.ts:218`). Restoring labels and pairing answer "Kuvo's relay can't be reached"
  (`work-queue/labels.ts:110`, `areas/phones.ts:88`).
- *Cost:* a one-computer user who's offline, or any user during a relay outage, can't change
  setup-wide settings, and the message names an owner that is this very computer. The fence buys
  nothing here: with no other computer, nothing can take over.
- *Fix:* `primaryFenced` returns false when the setup has no other member (`peersOf` empty).
  Adding a member already needs the relay. One line, plus a test. Many users have one computer.

**3. The ownership lookup can't find older conversations on another computer.** This is the
section above.

**4. Update rows and Try Again are decided in the app.** From the client sweep. I re-read the
first item.
- *Code:*
  - Try Again goes to that computer's own box, or is refused with "This app doesn't reach X yet"
    (`gpui/kuvo/src/workspace.rs:1239-1256`, `computers.rs:922-931`). `kuvo/update/retry` takes no
    computer key (`box/src/shared/routes/box.ts:531`).
  - The update chip and the "can't be used" view mix the app's gate with `compatibility`, and match
    the box's `detail` text against the app's own strings (`workspace.rs:1074-1091`, `:1420-1448`).
  - The app arbitrates between two update sources for one computer (`computers.rs:988-1033`,
    `:625-629`).
- *Cost:*
  - A computer the app doesn't reach directly can't be retried, though the main could drive it.
  - If the box rewords a message, the chip silently changes.
  - Rows can disagree with the main's summary.
- *Fix:* one fleet summary from the main, carrying per computer what to show, the words, and the
  actions. `kuvo/update/retry` takes a computer key and goes to the main. Medium.

**5. The app refuses requests and hides features on its own reading of the feature list.**
- *Code:* `gpui/kuvo_box/src/client.rs:324-343` (holding a box), `:358-364` (`require_feature`),
  and `gate.rs:85-97`. The baseline list is kept in two places (`gate.rs:24`,
  `areas/compatibility.ts:6`). Insights, Library and Areas are hidden by `gate().has(..)`, per the
  client sweep. Meanwhile the box says "Compatibility is for display"
  (`areas/compatibility.ts:8`).
- *Cost:* a box the app holds shows "needs an update" while the box itself would serve the request.
  This is rare now, since all boxes have the baseline features, but it's the same local refusal
  that 11-21 removed from the box.
- *Fix:* the app draws `compatibility` and sends every request. Hiding a button for a feature the
  box lacks stays: that's drawing. Medium.

**6. SSH runs the box under a different engine than the box picks.**
- *Code:* the app's SSH script takes the newest folder in `engine/`
  (`gpui/kuvo_box/src/ssh.rs:114`). The keeper picks the release's own engine, or the one before
  it after a failed start (`box/src/keeper/kuvo-start:38-56`).
- *Cost:* after a rollback, or with a newer engine left behind, SSH requests start on the wrong
  runtime. If that runtime doesn't run there, the computer can't be reached over SSH. Unlikely,
  but it shows as "can't connect".
- *Fix:* the box ships one launcher in `bin/` (for example `bin/kuvo-route`), and the app runs it.
  Small.

**7. Every new setup is created in the old model.**
- *Code:* `settleOwnSetup` creates epoch 1 without `managed` or `checkpointed`
  (`setup/lead.ts:61-76`). On a Mac, `leadsNow` then falls back to the Mac-ranked numbering rule
  (`:22-34`; `work-queue/numbers.ts:162-227`, with its 3-day "lost" rule). The first lease converts
  it (`setup/takeover.ts:82-87`, `:154-156`).
- *Cost:* none while the relay answers. A computer set up while the relay can't be reached stays
  in the old model until it can, and runs the code paths of finding 9.
- *Fix:* create new setups in the current model (certificate and checkpoint at epoch 1). Then the
  old model's only users are unconverted setups, and finding 9 can go on a date. Small.

### Old mechanisms beside the new ones (no one sees them; each keeps a concern at two)

**8. The old update path is still started by the keeper on every computer.**
- *Code:*
  - Health trial: `kuvo-start:748-758`, then `updates/health.ts` (375 lines).
  - Engine swap at a quiet moment: `kuvo-start:759-770`, then `updates/engine-swap.ts` (581 lines).
  - The bridge's quiet-moment and night-window wait (`agents/renewal.ts`, 299 lines; set up at
    `agents/main.ts:611-623`).
  - The restore kit (`kuvo-start:1075-1165`).
  - The `ON_TRIAL` guards in `fleet.ts`, `repair.ts` and `engine-swap.ts`.
  - The SSH keep-up (`add/keep-up.ts`, 264 lines). It brings an older machine up to the version of
    the box that set it up, every 15 minutes, with hour-then-day backoffs. Its `update` action is
    `POST kuvo/computers/:key/update`. It acts only for machines that don't yet announce `kuvo`,
    meaning boxes from before the fleet updater (`:155`, `:206`).

  On a switched computer none of these acts:
  - `pick()` returns early when `bin` is a link (`kuvo-start:41`).
  - The engine swap needs `bin` not to be a link (`:767`).
  - The switch stops the bridge before the new one starts (`:436-466`), so no new build appears
    under a running bridge.
  - Nothing writes `run/update-trial` any more (`updates/update.ts:273`).
- *Cost:* nothing visible. About 1,800 lines and their keeper branches stay alive and tested.
  This is the machinery behind 11-21's half-updated computers.
- *Fix:* delete them once every computer has switched to a `bin` link. That happens on its first
  start of a switch release, so in practice after 0.8.0's forced update. The keep-up goes with the
  0.7 machines.

**9. Setups that haven't converted keep a second authority model in every area.**
- *Code:*
  - Branches on the setup's model: 65 lines on `.checkpointed` in 27 files, 18 calls to
    `usesAcceptedSaves` in 11, and 51 lines on `.managed` in 22.
  - The layout 4 projection (`setup/access.ts:17-20`, `:54-78`, `COMPAT(layout.4 setup)`).
  - Per-kind publishing beside the one signed copy (`setup/shared.ts:149-156`).
  - Mac-enrolled machine rules (`setup/settle.ts:19-71`).
  - The main stored in the old Mac fields `link.mac`, `macs`, `macSigns`, `macNames`
    (`setup/leadership.ts:112-117`).
- *Cost:* nothing visible. Every setup change is written and tested for two models.
- *Fix:* each main already converts on its first lease (`takeover.ts:154-156`). After finding 7
  and 0.8.0's forced update, delete the branches and name the main `primary` everywhere.

**10. Clients reach the box through three launch shapes.**
- *Code:*
  - The resident `serve --resident` (`server/resident.ts`, `main.ts:1115-1128`).
  - The split `run` plus `serve --kept` (`kuvo-start:797-805`, `main.ts:1001-1049`, `:1130-1134`),
    kept so a computer can roll back to a release from before the resident process.
  - The box-services process, owned either by the bridge or by itself, with an `--app` variant
    (`main.ts:1051-1111`).
  - Over SSH, the plain `box route` (account authority) beside `box route-to` (pairing)
    (`ssh.rs:121`; `one-client-process.md`, decision 3).
- *Cost:* nothing visible. The keeper, the update guard and every route test cover two client
  layouts.
- *Fix:* #924 step 7 ("Retire split launch") once no supported rollback target is split. Only two
  versions are kept, so that's one release after the resident one. Whether standalone SSH aliases
  (an unpaired `box route`) stay is **a decision for the user**. I recommend dropping them, since
  every computer is now added to a setup.

**11. The folder layout steps still sit beside the per-file format versions.**
- *Code:* 23 steps, of which 20 are empty (`shared/folder-version.ts:18-70`). There are 12
  `…LayoutReady` checks with 58 calls in 19 files (`:243-375`), plus 19 calls to
  `setupLayoutDeferred`, `deviceGrantsLayoutDeferred` and `legacyFolderTrial`.
- *Cost:* nothing visible since `layoutHold` went. Every check is a "wait until this computer's
  folder says N" branch.
- *Fix:* freeze the layout at 23. Under the 11-21 rule, a change older builds can read needs no
  step, and one they can't read goes to a new path. Delete the checks at or below 23 in step 7.

**12. Pieces of the standby and pending receipts are still there.**
- *Code:*
  - The answer to an old standby's ping (`setup/takeover.ts:55-59`, `COMPAT(beta.40)`, no date).
  - `leadershipPending` (`state.ts:130-131`), deleted but never written.
  - `numberCeiling` (`setup/record.ts:37-38`).
  - Clean-up of `sharedPending` and the rest on every write (`state.ts:228-233`).
  - Parsing of pending receipts and 202 in `shared/setup-local.ts`, and `PENDING_SAVE` in
    `shared/routes/setup-save.ts`.
  - Declarations for kinds with no code (`shared/data-declarations.ts:69-72`).
  - The numbering floors read on every block (`work-queue/numbers.ts:119-124`, `:320-338`), and the
    `quorumGrant` flag (`:772-773`).
- *Cost:* none.
- *Fix:* delete them once no beta.40 computer can connect (after the forced update). Give the
  three markers with no date one. One short session.

**13. Routes kept for clients that are gone.**
- *Code:*
  - `GET kuvo/data/owner`, `POST kuvo/data/messages` and the four `kuvo/data/edits*` routes
    (`server/router.ts:355-373`). No caller in the desktop or Kuvo Helper: the desktop uses only
    `GET kuvo/data/messages/:op` (`gpui/kuvo_box/src/client.rs:656`). Edits through ordinary routes
    already go to the owner (`ownership/edits.ts:287-310`).
  - `kuvo/machines` and `kuvo/hosts` are answered in four places (`router.ts:188`, `:191`;
    `areas/setup.ts:150-165`; `mac/relay.ts:87-88`), with no desktop caller.
  - Also with no desktop caller:
    - `kuvo/phones*` (`router.ts:192-194`);
    - 12 `kuvo/link/*` routes (`mac/routes.ts:153-303`);
    - `kuvo/attention/notice` and `kuvo/mac`;
    - the phone-only sign-in route (`areas/sign-in.ts:334`);
    - phone push: `relay/src/apns.ts`, `registrations.ts`, `notices/push.ts`.
- *Cost:* nothing visible. Four answerers for one route is how "which one answered?" bugs start,
  and each route stays in PROTOCOL.md and the route vectors.
- *Fix:* the explicit data routes go now. The rest go with the 0.7 front in step 7. Whether
  installed phone builds still matter, and so whether push stays, is **a decision for the user**.

**14. The move off 0.7 is still most of the box's compatibility code.**
- *Code:*
  - `rehome/` (5,250 lines) and the Mac front in `mac/` (2,171 lines).
  - The app-folder Mac box: `APP_FOLDER` appears 87 times in 27 files, with no `COMPAT` marker
    (`home.ts:37-40`). Nothing in this repo starts a box with `KUVO_BOX=mac` any more, but the
    desktop still looks for one (`gpui/kuvo_box/src/place.rs:53-60`).
  - 55 `COMPAT(0.7)` markers, and the browser's one-time import of the old app's data
    (`browser/from-app.ts`).
- *Cost:* none. Every wipe, uninstall, backup and permission path has a second layout
  (`setup/wipe.ts` has 12 of the 87).
- *Fix:* mark `APP_FOLDER` as `COMPAT(0.7)` now, so it goes with the rest in step 7 on the user's
  date.

**15. Members copy the whole shared state every 10 seconds, and read the lease every 2.**
- *Code:* each member sends "available" every 10 s (`setup/takeover.ts:161-164`). The main answers
  each time with the whole signed copy and its 256 latest outcomes (`:50-53`;
  `setup/shared.ts:143-147`), and doesn't check what the member already has. Members also ask the
  relay for the lease every 2 s (`takeover.ts:175`). Changes are pushed when they happen as well
  (`documents/changed.ts`).
- *Cost:* steady traffic on laptops and through the relay, for no new information. Lease reads
  are about five times the 100 a second the review planned for. Nothing visible.
- *Fix:* "available" carries the member's sequence, and the main answers only when it has
  something newer. Members read the lease every 10 s, and when `takeoverIn` comes due. Small.

**16. Two counters for one leadership change.** Each certificate carries both `epoch + 1` and the
relay's term (`setup/takeover.ts:78`), and members check both (`setup/leadership.ts:62-66`). The
epoch appears 267 times in 42 files.
- *Cost:* none.
- *Fix:* new certificates use the relay term as the epoch, when that's higher. Do it with
  finding 9, not on its own.

### Built against a lying computer (the user's computers are trusted)

**17. Proof layers on every write passed to another computer.** These follow messages through
moves:
- issuer addresses and forward evidence (`ownership/forward-evidence*.ts`);
- the SQLite receipt index (`ownership/receipt-index.ts`);
- submitter checks with `checking`, `revoked` and `unknown` states (`ownership/submissions.ts`,
  `submitter.ts`).

That's about 1,000 lines. Operation IDs with dedupe at the owner cover the trusted case.
Simplify them when this code is next changed (the review's E4, still open).

**18. Wipe binds its erase to a hash of the data's generation.**
- *Code:* `setup/removal-generation.ts`, `before-wipe.ts`, `wipe.ts`. Revoked members can finish
  prepared moves (`setup/removal-moves.ts`), and SSH probes relay acknowledgements
  (`setup/removal.ts`). Together that's about 1,450 lines.
- *What stays:* the outcome rules (access ends at once, a backup before the wipe, nothing lost).
- *Simpler:* stop the writers, back up, verify, then erase. That needs no generation. Do it when
  the code next changes, not before.

**19. Merging setups is the last wait on every computer.** `computersSupportMerge` refuses while
any member lacks the merge feature, including members that are off (`setup/merge-data.ts:82`,
`:91-94`). Merging is rare. Delete the check once the oldest supported version supports merging.

### Docs and paths

**20.** PROTOCOL.md is 12,758 lines, and it opens as "How Kuvo Code on a phone reaches the agents
on a Mac". The built link lives in a folder named for the removed Swift app
(`swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js`; `scripts/build-box.sh:2`,
`box/src/release/files.ts:126`). A new client's author can't find the contract. Fix:
- point the top of PROTOCOL.md at `how-kuvo-works.md`;
- drop the phone framing once finding 13 is decided;
- move the built file under `box/`, which touches the build and release scripts.

---

## Where nothing is left

- **Votes, majorities, quorums:** none. A number floor read once (`numbers.ts:119-124`) and a flag
  for old members (`:772-773`) are all that remain.
- **The standby role:** no ranks, pins, "Choose standby", successor chains, witnesses, sealed
  epochs or `queryOperation`, in the box or the app.
- **Waits on every computer for updates:** none. The main waits for the computers that are on, five
  minutes at most (`updates/fleet.ts:259-271`). That's the designed rule.
- **Backups before updates or moves:** gone. `backupBefore` is called only for daily backups and
  wipes. The leftover `"update"` reason (`backup/engine.ts:30`) is unused.
- **Version refusals in the box:** gone. The app's own dismissal and stale update lines are gone
  too.
- **Chicken-and-egg on the update path:** none. Members take releases only from the main
  (`fleet.ts:177-190`). The main takes the newest that any computer on runs or offers, including a
  Mac app's bundled release. The keeper switches back when a new version doesn't answer. One
  inherent case remains: a main whose update code is broken but still answers can only be fixed
  through itself, or by making another computer the main.
- **Client-side, clean:** Library and Insights figures, the side panel (Changes, Files, Find),
  notifications, plan-limit rings, move destinations, Work Queue grouping, and the SSH-or-relay
  choice.

## What I checked

I read the lease, the takeover, leadership, shared saves, the ownership lookup and its callers,
moves, the fleet rule, the keeper's launch and switch paths, the client process, backups and wipe,
and the relay's lease store. Two read-only sweeps covered the leftover compatibility code and the
desktop's client logic. I re-checked their top claims in the code myself: findings 1, 5, 6, 8 and
13, and the counts in 9, 11 and 14. Finding 4, the `mac/` route lines in 13, and the line lists in
12 come from the sweeps and weren't all re-read. Findings 2, 3, 7 and 15 are my own reading. I ran
no tests and changed no product code. Linux behaviour was read from the code, not run.
