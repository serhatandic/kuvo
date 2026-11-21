# Whole-system review at 6edef250d

Read-only review of `main` at `6edef250df8c6c5f529d080dda494ea4f1758c42` (21 November 2026). It
includes the update unlock, the `bin` switch, rule 57, the #668 leftovers, the Computers page
(scr1) and the installer fix `1219637d4`. Step 2 of `simple-updates.md` (the fleet rule) is not
on main yet. Line numbers are at this commit. Short box paths start at `box/src/link/`.

## In plain words

**The multi-computer layer is overbuilt for this app, and it is where every 11-21 lockout came
from.** The layer has a main computer, a standby that must confirm every setup change and keep
the main's lease alive, a ranked line of successors, epochs with rules for merging split
histories, majority votes for Work Queue numbers, and waits on every computer before choosing a
release. Most of it exists because the computers have no referee they can trust, so every
decision needs a second computer to agree. The user has now decided on that referee: a short
lease at the Kuvo relay, which runs all day on the VPS. With the lease there, the standby, the
votes, the ranks, the split merging and the all-computer waits have nothing left to do.

What users hit today, most common first:

1. **With two computers, every setup change stops whenever the other computer sleeps.** The
   standby is what keeps the main's lease alive. Pairing a phone, adding or removing a computer,
   labels, time zone, personalities, a new release choice and new Work Queue number blocks all
   answer "Your main computer is waiting for its standby." Most users have two computers, and
   one is usually a laptop.
2. **With two computers, losing the standby for good is a dead end.** Removing it needs its own
   approval. Until then the main can't pair phones, add a replacement computer, restore a
   backup, or remove the dead one.
3. **With two computers, a takeover gives the user nothing.** The new main needs a standby
   before it can change anything shared, and the only candidate is the computer that just
   failed.
4. **Updates can't move forward while any computer is old or the standby is asleep.** The main
   won't choose a release while a computer that's on announces an older folder layout. The
   choice also waits for the standby, and only one choice can be pending. That was the 11-21
   lockout: hours on beta.38, then "Waiting for lokum-tailscale to support Kuvo folder layout
   23" while every other computer ran beta.40, and the only way out was running installers by
   hand. 21 of the 23 layout steps that caused these waits change nothing on disk.
5. **Update Now can't advance the setup, and computers ahead of the choice show as behind.**
   Update Now only retries the setup's current choice (beta.38). It never looks for beta.40.
   Three computers on beta.40 read "beta.38 available", and the summary said "0 of 4 done".
6. **An update fails when the backup destination is away.** Before switching, each update
   writes a full verified backup to the configured destination. That destination is usually the
   other computer, and an asleep or unplugged one fails the update. The failure is then retried
   an hour later, then a day later. Moves do the same.
7. **With two computers, Work Queue numbers can stop for good.** Each new block of numbers
   needs a majority of the computers that were members when ranking started. That group never
   changes. Remove or lose one of two computers and new items wait for a number forever.

The fix is mostly removal. Put one lease at the relay. Let the main decide alone and the others
copy. Let every computer follow the main's version. Take the standby, the votes, the epochs'
merge rules, the all-computer waits and the second update installer out. Each removal's cost to
the user is listed under "The first question". The trim plan is seven steps. It starts with the
update path, because every later fix has to arrive through it.

---

## 1. The revised rule list

The 58 rules are #848's (Work Queue #848; rules 28-54 also in
`.claude/notes/848-data-list-draft.md`). Each is marked KEEP, REWRITE or DROP against the
target: one lease at the relay agrees on the main, and the main decides everything else. The
rules added on 11-21 follow the 58. **The lead shows the user only the REWRITE and DROP rows.**

### Rewritten and dropped (for the user)

| # | Was | Now | Why |
|---:|---|---|---|
| 2 | Users can't unlink the primary until they pick another primary. | **REWRITE:** Unlinking the main never leaves the setup without one. Kuvo hands the role to another computer first, an always-on one unless you pick, then unlinks. | Today it refuses with "Choose another main computer first." (`peers/manage.ts:330`), and the hand-over itself can be refused (finding 9). Same outcome, no dead end. |
| 13 | Updates are fleet-wide. The primary decides the version and every machine follows. **A machine on an older version is updated before it's used.** | **REWRITE:** The main decides the version. Every computer that's on gets it ready beside the one it runs, then switches in one step when the main does. A computer that's off or can't get ready stays fully usable on its version and catches up by itself. No computer is refused for its version. | "Updated before it's used" is what locked the user out on 11-21. The user's 11-21 rules (one version per computer, prepare then switch, never locked) replace it. |
| 22 | A client too old for the setup's version asks to be updated before it connects. | **REWRITE:** An older client keeps working with what it understands. Features it lacks aren't shown, and it offers its update. It is never refused for its version. | D7 (no refusals from 0.8.0 on). A refusal is a lockout. |
| 31 | Only the primary changes membership, device access, **ownership assignments**, the setup's **release choice** and security policy. Members can tighten locally at once; granting waits for the primary. | **REWRITE:** Only the main changes membership, device access, the setup's version and security policy. Members can tighten locally at once; granting waits for the main. | Moves no longer need the main (rule 46). "Release choice" becomes "the setup's version" (rule 13). |
| 32 | The standby takes over after a few hours and advances the counter. Once a standby exists, the primary confirms a shared change only after the standby durably accepts it. | **DROP**, replaced by: **One main at a time, agreed through a short lease at the Kuvo relay.** If the main lets its lease lapse (about a minute), another computer takes it within a few minutes, always-on computers first. A main that can't renew its lease stops setup-wide changes and says so; everyday work goes on. Kuvo warns when the main runs on the relay's own computer. | The user's decision (11-21 ~17:xx). The standby's confirmation is what stops every setup change when a laptop sleeps (findings 1-3). |
| 33 | The standby keeps the shared data, counter, removals, number blocks and pending transfers. Only the designated standby may take over, and replacing it needs the old one retired first. | **DROP**, replaced by: **Every computer keeps a copy of the main's setup state, sent as soon as it changes, so whoever takes over starts from the newest copy it has.** | The standby role goes with rule 32. The retirement rule is what makes a lost standby a dead end (finding 2). |
| 46 | A move pauses the source, verifies the copy, **then the primary records the new owner** before the destination runs or edits it, showing "moving" until then. | **REWRITE:** A move keeps the item's ID and names both computers. The source pauses, the destination verifies the complete copy, and it runs the item only after the source lets go, showing "moving" until then. Nothing moved runs twice or is lost. | Only the two computers in the move need to agree. Today a move between two members fails while the main or its standby is away, and it can be refused when the setup's shared state is full (finding 10). |
| 49 | A daily backup, and one before a move, wipe or **format-changing update**; keep 30 daily copies; show the last verified point. | **REWRITE:** A daily backup and one before a wipe; keep 30 daily copies; show the last verified point. **A backup never holds up an update or a move.** If the destination is away, the operation goes ahead and the backup status says what was skipped. | Updates switch back in one step, and the 11-21 format rule keeps old versions reading their data, so a backup before an update protects nothing new. Today an away destination fails the update (finding 6). |
| 53 | The failure tests … check offline edits, late revocations, **standby takeover and rejection of the old primary's changes**. | **REWRITE:** … check offline edits, late revocations, takeover through the relay lease, the relay going away and coming back, a lost computer in a two-computer setup, and a returning old main stepping down. | Tests follow the new mechanism and the two-computer case most users have. |
| 54 | Stop an update halfway, restart with mixed versions, erase a disk and restore … | **REWRITE:** The same, plus updates from 0.7.2 and from beta.35, with one computer off. Nobody is refused at any step. | The user's rule: update tests include updates from before 0.8. |
| 56 | Every stored format and message carries a version. Neighbouring versions work together, and every format change ships with a tested upgrade step. | **REWRITE:** Every stored format and message carries a version. A change older builds can still read needs no folder step. A change they can't read is written to a new place they never open, so going back to an older version always works. Any two versions work together: features wait, nothing refuses. Every upgrade step is tested, from 0.7.2 and beta.35 too. | The 11-21 format decision (`authority-first-readers.md`). Folder layout steps that change nothing set off the fleet-wide waits (findings 4, 5). |
| 57 | One request API for all clients, with a feature list each computer announces **(the existing update gate)** … | **REWRITE:** One request API for all clients, with a feature list each computer announces. New features add requests. Clients never get logic, and a computer is never refused for its version. | Drops the word "gate"; the gate's refusals are gone (step 0). |

### Kept, unchanged

Rules 1, 3-12, 14-21, 23-30, 34-45, 47, 48, 50-52, 55 and 58 are **KEEP**. They state outcomes
(any computer can lead, equal computers, devices pair once, unlink versus wipe, one owner per
record, offline edits merge, backups and restore, thin clients, owner declarations, the bench in
every check). Two notes:
- **Rule 39** stays as the user accepted it on 11-21: the bounded fallback for long repetitive
  notes is good enough.
- **Rule 41** (only the main hands out number blocks; an item can wait for a number) stays as
  written. Only its quorum mechanism goes (finding 7).

### Added on 11-21 (the user's rules)

| # | Rule |
|---|---|
| N1 | **One version per computer.** Every Kuvo part on a computer switches in one step. It's fully usable on the old version until then and on the new one after. Going back is the same switch in reverse. |
| N2 | **Prepare everywhere, then switch together, in seconds, minutes at worst.** The main switches once every computer that's on is ready, or at most five minutes after the others are. A slow or failing computer is left out and catches up by itself. |
| N3 | **No dead ends.** Every waiting state says what it waits for, and either clears by itself or offers one action the user can take. Nothing manual and no hand-run installers. |
| N4 | **The main decides everything else** (setup changes, Work Queue numbers, the version, layouts). The others copy in the background. There are no standby confirmations, votes or waits on every computer. |
| N5 | **Threat model:** the user's own computers are trusted. Defences that only matter against one of their own computers lying are edge cases, not requirements. |

---

## 2. Ratings against the revised list

**C** covered, **P** partial, **X** contradicted on a path users take. **Carried** means the
previous audit's rating at `45410efb8` stands. I didn't re-run its evidence, and nothing I read
in this pass changes it. **Re-rated** rows were checked here. H is
`box/bridge-tests/setup-handover.test.js`.

**Totals: 20 C, 32 P, 6 X across the 58, and N1-N4 at 1 P, 3 X.** At 45410efb8 it was 27 C,
30 P, 1 X against the old wording. The six X are rules 13, 18, 32, 41, 49 and 56. Most of the
change comes from the rewritten rules, and from measuring two-computer setups, which the old
ratings didn't separate. Rule 25 moves from X to P now that Rename is wired.

| # | Decision | Now | Evidence |
|---:|---|:---:|---|
| 1 | KEEP | C | Carried. Linux leads (`setup/lead.ts:51-80`). |
| 2 | REWRITE | P | Re-rated. The desktop app now has Change main computer (`gpui/kuvo/src/computers_manage.rs`, be6d34c3e). Unlinking the main still refuses (`peers/manage.ts:330`) instead of handing over, and the hand-over refuses while any member is old (`setup/handover.ts:65-66`). |
| 3 | KEEP | P | Re-rated. The desktop Remove now offers Unlink or Unlink + wipe (be6d34c3e). The phone doesn't yet. |
| 4 | KEEP | C | Carried. |
| 5 | KEEP | P | Carried (phone). |
| 6 | KEEP | P | Carried. |
| 7 | KEEP | P | Re-rated from C. Takeover works with 3+ computers (H1227, H1315). With two, the new main can make no shared change (finding 3). |
| 8 | KEEP | C | Carried. |
| 9 | KEEP | P | Carried. |
| 10 | KEEP | P | Carried. |
| 11 | KEEP | C | Carried. |
| 12 | KEEP | P | Re-rated from C. Everyday work does continue (H666). But "only adding or removing waits until the primary is back or another takes over" fails twice: changes also wait while the main is reachable and its standby sleeps (finding 1), and after a two-computer takeover (finding 3). |
| 13 | REWRITE | X | Re-rated. `layoutHold` on the choice (`areas/setup.ts:213`, `updates/update.ts:752-754`, `:883-884`). The choice waits on the standby (`setup/shared.ts:296`). Packs come only from the main or standby (`updates/fleet.ts:274-278`). Update Now can't advance (`updates/update.ts:1452-1453`). Live 11-21 evidence. |
| 14 | KEEP | P | Carried. |
| 15 | KEEP | P | Carried. |
| 16 | KEEP | P | Re-rated. The desktop app draws the reinstall offer (be6d34c3e). The phone and remote clients don't. |
| 17 | KEEP | P | Carried. The app still decides held boxes, the notifier and candidates (findings 18-21). |
| 18 | KEEP | X | Re-rated from P. "Unlink happens right away" for a dead machine fails when that machine is the last standby. Removal needs its approval and returns 503 meanwhile (`peers/manage.ts:481-488`, H1649). |
| 19 | KEEP | P | Carried. |
| 20 | KEEP | C | Carried. |
| 21 | KEEP | P | Carried. |
| 22 | REWRITE | P | Re-rated. The box's refusals are gone (step 0, 39973e171). The app still holds a box lacking its baseline features (`gpui/kuvo_box/src/gate.rs:85-97`), which is acceptable under the rewrite. Phone and installed runs not checked. |
| 23 | KEEP | P | Carried. |
| 24 | KEEP | P | Carried. |
| 25 | KEEP | P | Re-rated from X. Rename is now wired on the desktop (`gpui/kuvo/src/computers.rs:889`). The phone isn't. |
| 26 | KEEP | P | Carried. |
| 27 | KEEP | P | Carried. |
| 28 | KEEP | P | Carried. |
| 29 | KEEP | P | Carried. |
| 30 | KEEP | C | Carried. |
| 31 | REWRITE | P | Re-rated. The main alone stages changes, but only with a standby's confirmation (`setup/shared.ts:232`, `:296`), and moves still go through it (`ownership/transfer.ts:96-152`). |
| 32 | DROP → relay lease | X | Re-rated. The standby grants the lease (`setup/takeover.ts:124-134`). There is no lease at the relay. |
| 33 | DROP → members keep copies | P | Re-rated. Members do take the main's published state (`setup/shared.ts:327-340`). The standby role, its confirmations and its retirement remain. |
| 34 | KEEP | P | Carried. |
| 35 | KEEP | C | Carried. |
| 36 | KEEP | C | Carried. |
| 37 | KEEP | C | Carried. |
| 38 | KEEP | C | Carried. |
| 39 | KEEP | C | Re-rated from P. The user accepted the bounded fallback (decisions, 11-21). |
| 40 | KEEP | C | Carried. |
| 41 | KEEP | X | Re-rated from P. "Shows as awaiting a number" turns permanent once fewer than a majority of the original voters remain. That's one of two in a two-computer setup (`work-queue/number-quorum.ts:116`, `setup/checkpoint.ts:192-211`; `numbering-proof.md`: "Electorate reconfiguration is not implemented"). |
| 42 | KEEP | C | Carried. |
| 43 | KEEP | C | Carried. |
| 44 | KEEP | P | Carried. |
| 45 | KEEP | C | Carried. |
| 46 | REWRITE | P | Re-rated from C. Move fidelity holds (previous audit), but it needs the main and its standby, and can be refused for capacity (`ownership/transfer.ts:149-152`). |
| 47 | KEEP | P | Carried. |
| 48 | KEEP | C | Carried. |
| 49 | REWRITE | X | Re-rated. The update and move wait on a full backup to the destination, and fail when it's away (`updates/update.ts:905`, `:933`; `moves/move.ts:539`; `backup/engine.ts:341-344`). |
| 50 | KEEP | C | Carried. |
| 51 | KEEP | P | Re-rated. In a two-computer setup whose standby died, the main can't add the replacement or restore (`add/flow.ts:74`, `backup/restore.ts:67`; finding 2). |
| 52 | KEEP | C | Carried. |
| 53 | REWRITE | P | Re-rated. The bench exists. It has no relay-lease case (none exists yet) and no case of a two-computer standby lost for good. |
| 54 | REWRITE | P | Re-rated. `box/test/update-switch.test.ts:58` converts 0.7.2 and beta.35 folders. Fleet updates from 0.7.2 and beta.35 with one computer off (simple-updates T5, T6) aren't built. |
| 55 | KEEP | C | Carried. |
| 56 | REWRITE | X | Re-rated. 21 of 23 folder steps are empty "reserve" steps (`shared/folder-version.ts:18-67`), and each bump makes the fleet wait (findings 4, 5). |
| 57 | REWRITE | P | Re-rated. The rule 57 merge moved desktop update decisions to the box (b71495a7e). The app's local gate and its notifier rules remain (findings 18, 20). |
| 58 | KEEP | C | Carried. |
| N1 | added | P | The `bin` switch moves every part at once for releases that carry it (`updates/update.ts:860-923`, `keeper/kuvo-start:875-999`). The old file-by-file installer, health trial, later engine swap and the bridge's quiet-moment wait are still in the code (finding 8). |
| N2 | added | X | There is no fleet-wide get-ready-then-switch. A computer installs when it sees the choice, and an update includes a full backup (finding 6). New releases wait 3 days (`updates/update.ts:85`, `:729`), checked every 6 h. |
| N3 | added | X | Findings 2, 4, 5, 7 and 11. |
| N4 | added | X | Findings 1, 3, 7 and 12. |

---

## 3. The first question: is #848's machinery overengineered, and what can go?

**Yes.** For about 1,000 users, mostly with one or two trusted computers, the layer solves
problems they don't have and creates the ones they hit. The reason is structural. Without a
referee, safety needs a second computer to agree. With two computers, that second computer is
the one that's asleep, so every guarantee turns into a wait. The relay already carries all mail
between computers, so it already decides whether they can work together. Making it the referee
adds no new dependency. With it as referee, everything below can go.

| Remove | Where today | What replaces it | What it costs the user |
|---|---|---|---|
| **The standby's lease** (ping, pong, lease every 10 s) | `setup/takeover.ts:124-140`, `setup/lease.ts:48-79` | The main renews a 60 s lease at the relay. It stops setup-wide changes when its own lease lapses. | If the relay is down, nobody can change setup-wide things or take over. That's already true today, since all mail goes through the relay. |
| **Standby confirmation of every save**, pending saves, 202 "pending" receipts, one pending save per kind | `setup/shared.ts:228-303`, `:395-482`; `shared/setup-local.ts`; `PENDING_SAVE` callers | The main saves at once and sends the new state to every member as soon as it's saved. A receipt is just the operation ID. | A setup change confirmed in the last moment before the main's disk dies can be lost. It would need the main's disk to fail before its copy reached any member. |
| **The standby role**: selection, the pin, replacement, retirement and its approval, first-standby revisions, "Choose standby" | `setup/handover.ts:271-363`, `setup/takeover.ts:190-251`, `setup/leadership.ts:65-77`, `:206-238`, `peers/manage.ts:477-494`; the Choose standby sheet in `gpui/kuvo/src/computers_manage.rs` | Nothing. Any member can take over when the lease lapses. | The user can no longer name the next main in advance. "Make main" already covers choosing the main itself. |
| **The ranked successor chain**: observed-uptime ranking, rank refreshes saved as membership changes, probes of higher ranks, pins | `setup/reliability.ts`, `setup/takeover.ts:209-212`, `:254-289`, `:320-332`; `successorOrder` in membership | Always-on computers claim the lapsed lease at once, others after one more minute. The relay gives it to the first claim. | A laptop could become the main when no always-on computer is up. When a server later comes back, it doesn't take the role back on its own; the user can use Make main. |
| **Epoch forks and merging split histories**: higher epoch wins, rank at the last common checkpoint breaks ties, revocations merged across branches, superseded first standbys, successor-certificate history broadcast every 10 s | `setup/leadership.ts:126-285`, `setup/takeover.ts:80-97`, `:165-171`, `setup/checkpoint.ts:73-183` | A term number from the relay. A returning old main sees a higher term and steps down. It made no setup changes while its lease was lapsed, so nothing is merged or dropped. | None. Today's "the losing side's setup changes are dropped" goes away. |
| **Majority Work Queue number blocks**: voters, votes, proposals and the fixed electorate | `work-queue/number-quorum.ts`, `work-queue/numbers.ts:534-660`, `setup/checkpoint.ts:185-219` | The main hands out blocks from its counter. Before giving a block out, it records the counter's new high mark with its lease at the relay, so a new main starts above it. | None. Numbers never repeat, and the "can't get a number again" dead end goes. |
| **The older numbering paths**: Mac-ranked numbering with its 3-day "lost" rule, and the standby-saved ledger | `work-queue/numbers.ts:151-273`, `:328-353`, `setup/lead.ts:22-34` | The same single counter. | None once 0.8.0's forced update has run (decision D8). |
| **Every wait on all computers**: `layoutHold`; migrations that wait until every member supports something (checkpoints, successor chain, merge); the hand-over's "every member must support it"; first-standby selection waiting on every member | `updates/choice.ts:38-53`, `setup/shared.ts:212-225`, `:252-267`, `updates/keep.ts:69`, `setup/takeover.ts:216-218`, `:244`, `setup/merge-data.ts:99`, `setup/handover.ts:65-66`, `:133-140` | One version per computer. Members follow the main's version (N1, N2). A feature is used on each computer once that computer announces it. | None. |
| **The release choice as a setup save** | `updates/update.ts:657-775`, `updates/choice.ts`, `RELEASE_CHOICE` in shared state | The main announces the release it holds and the version it runs (simple-updates step 2). Members read those announcements. | None. The main keeps writing the old record for one release so beta.35 to beta.40 members can follow. |
| **Three setup authority models at once**: Mac-led legacy, layout 4 standby delegation, checkpointed, successor chain (80-90 branches on `.successorChain` and `.checkpointed`) | `setup/record.ts:106-111`, branches across `setup/*`, `add/*`, `work-queue/*` | One model: the main's signed, versioned setup state. The main converts its own state in one step, and members follow by version. | None, if done after the update fix (trim step 1). |
| **Moves recorded by the main** (the setup-wide `ownershipAssignments`) | `ownership/transfer.ts:60-160`, `ownership/resolve.ts` | A hand-over between source and destination, with the move's own journal. Each computer announces what it owns. | A move still needs both of its computers on (true today). It no longer needs the main. |
| **Pre-update and pre-move backups** | `updates/update.ts:905`, `:933`, `moves/move.ts:539` | Daily backups as they are. The switch keeps the old version, and old versions keep reading their data. | No backup point right before an update. The daily one is at most a day old. |

**What stays:** one main, chosen automatically and changeable with Make main. Signed membership
and device lists published by the main. Revocations that reach every computer. Local work that
never waits. Owners per record, offline edits and merges, moves, backups, wipe. These are the
user's outcome rules, and none of them needed the standby.

**Relay check (asked by the user).** The lease is one small record per setup: holder, term,
expiry, and the Work Queue high mark. At about 1,000 setups renewing every 10 s, that's about
100 tiny writes a second, trivial for the existing VPS. It needs one new relay message kind
(claim, renew, read) and a small file that survives restarts. The relay's source isn't in this
repo: it runs as the `kuvo-link-relay` container on Hetzner, so this step also changes that
code. The relay can't check setup membership, so a lease is keyed by a hash of a secret only the
setup's computers hold (trusted computers, N5). After a relay restart it should accept renewals
from current holders for two minutes before allowing any takeover.

**The user's own setup:** the relay and the main are the same Hetzner machine, so losing it
loses both, and nothing can take over. Kuvo should warn about this (decided), and the Mini
should become main once the new design lands (suggested, not done).

---

## 4. Findings

Each finding says what the code does, what the user sees, how likely it is, and the simplest
fix (removing before adding). **MUST** covers normal users and common failures. Edge cases are
listed last.

### MUST

**1. With two computers, setup changes stop whenever the other computer sleeps.**
- *Code:* only the standby renews the main's lease (`setup/takeover.ts:124-134`: the `lease`
  message is the only call to `grantLease`). An expired lease fences the main
  (`setup/lease.ts:73-78`). A replacement standby must be another available member
  (`setup/takeover.ts:227-235`), and a two-computer setup has none.
- *What users see:* "Your main computer is waiting for its standby. Try again when they can
  reach each other." (`setup/lease.ts:81`) when pairing a phone (`areas/phones.ts:86`; H1439
  asserts the 409), adding a computer (`add/flow.ts:74`, `add/invite.ts:135`, `:241`), changing
  the main (`setup/handover.ts:61`), restoring labels (`work-queue/labels.ts:110`) or restoring
  a backup (`backup/restore.ts:67`). Time zone and personalities stay "pending"
  (`areas/settings-pages.ts:666`, `areas/personalities.ts:42`). The release choice stays pending
  (`setup/shared.ts:296`), which is item 2 of the 11-21 notes.
- *Likelihood:* high. Most users have two computers, and one is a laptop that sleeps every
  night.
- *Simplest fix:* the relay lease (trim step 2). Delete the standby lease.

**2. Losing the standby for good is a dead end in a two-computer setup.**
- *Code:* removing the computer that is the last standby needs a leadership proposal that the
  standby itself approves (`peers/manage.ts:481-488`). While that proposal is pending the main
  is fenced (`setup/lease.ts:65`). Superseding it needs another available member
  (`setup/takeover.ts:179-185`), and there is none.
- *What users see:* H1649 shows removal answering 503 and pairing 409 while the standby can't
  answer. If the standby's disk died, or the laptop was sold or stolen, that never ends. The
  user can't remove it, can't add a replacement (`add/flow.ts:74`), and can't restore it from a
  backup (`backup/restore.ts:67`, `:106`). This contradicts rules 18 and 51. The only way out
  is wiping and setting up again.
- *Likelihood:* medium. Replacing or losing a laptop is ordinary, and it's the recovery flow the
  backups exist for.
- *Simplest fix:* the relay lease, with no standby role (trim steps 2-3). Removing a computer
  becomes an ordinary change the main makes alone.

**3. With two computers, a takeover leaves the new main unable to change anything shared.**
- *Code:* every takeover sets `sharedSaves: true` (`setup/takeover.ts:350`), which makes the
  setup "require a standby" (`setup/record.ts:106-108`). Without one, shared saves return
  nothing (`setup/shared.ts:232`), membership changes report busy (`setup/access.ts:69`),
  settings answer 503 "pending" (`areas/personalities.ts:42`, `:65`, `:97`,
  `areas/settings-pages.ts:666`), and the release choice can't be saved. The only possible
  standby is the old main that just failed.
- *What users see:* after a laptop main has been closed for three minutes, the server becomes
  main, then can't pair a phone, add or remove a computer, or choose a release until the laptop
  is back. If the old main is gone for good, finding 2 applies.
- *Likelihood:* high whenever a two-computer takeover happens. The default main is the first
  computer set up, usually the Mac (`setup/lead.ts:51-80`).
- *Simplest fix:* the relay lease, with the main saving alone (trim steps 2-3).

**4. Updates wait on every computer that's on, and the fix can only arrive through the broken
path.**
- *Code:* the main won't choose a release while any reachable member announces an older folder
  layout (`updates/choice.ts:38-53`). It checks before saving the choice (`areas/setup.ts:213`)
  and twice while installing (`updates/update.ts:752-754`, `:883-884`). A member that hasn't
  said its layout counts as 0, except beta.35 as 18 (`updates/choice.ts:50`). The choice is
  then a setup save that waits for the standby (`setup/shared.ts:296`). Only one save per kind
  can be pending, so a newer choice is refused while an older one waits
  (`setup/shared.ts:245-250`, "a save is already pending", `areas/setup.ts:222`). Members
  update only to the chosen release (`updates/fleet.ts:263-278`).
- *Why it can't resolve itself:* an old member can only stop holding the choice by updating,
  and it can only update to a release the main has chosen.
- *What users saw on 11-21:* the setup stayed on beta.38 for hours with the beta.35 standby
  unable to confirm. "Waiting for lokum-tailscale to support Kuvo folder layout 23" held beta.40
  after every other computer already ran it. The only way out was hand-running each release's
  installer over SSH or from the DMG, which is the exempt path (`installer` in
  `updates/update.ts:752`, `:880`, fixed in `1219637d4`).
- *Likelihood:* certain at every layout bump while any old computer is on.
- *Simplest fix:* simple-updates step 2 as designed (trim step 1). Remove `layoutHold` and the
  choice-as-save. Members follow the main's announced release.
- *Not covered by that design:* the folder layout must not change in the release that ships the
  fix, because that release still has to pass today's `layoutHold`. Until it lands, hand-running
  installers stays the only way out.

**5. 21 of 23 folder layout steps change nothing, and each one set off finding 4.**
- *Code:* `shared/folder-version.ts:18-67`. Steps 2-10, 12-16 and 18-23 are `() => {}` with
  comments like "Reserve …". The layout number became a fleet-wide gate through `layoutHold`,
  and it also feeds fourteen readiness checks such as `successorRevisionLayoutReady` and
  `setupMergeLayoutReady` (`shared/folder-version.ts:237-396`).
- *What users see:* every one of these bumps made every computer wait for every other.
- *Likelihood:* certain at each bump.
- *Simplest fix:* freeze the layout at 23. Under the 11-21 rule, a change older builds can read
  needs no step, and an incompatible one writes to a new path. Delete the readiness checks once
  the oldest supported version writes 23 (trim steps 1 and 7).

**6. An update fails when the backup destination is away, and backups make updates slow.**
- *Code:* both installers wait for a full verified backup before switching
  (`updates/update.ts:905`, `:933`). `backupBefore` throws when the destination disk isn't
  connected or the backup computer can't be checked (`backup/engine.ts:341-344`). The throw
  becomes a failed update, which is retried after an hour, then a day
  (`updates/update.ts:626-632`, `:97-98`, `:1564`). Moves do the same (`moves/move.ts:539`).
- *What users see:* with backups on (the agreed default destination is another of their
  computers), a computer's update or move fails while that computer sleeps or the disk is
  unplugged. Each update also takes as long as a full backup, against the user's "seconds,
  minutes at worst".
- *Likelihood:* medium (users who turned backups on), and certain for slow updates when they
  have.
- *Simplest fix:* remove both pre-update calls and the pre-move call. Keep the daily backup and
  the one before a wipe (rule 49 as rewritten).

**7. Work Queue numbers can stop for good, and three numbering systems run side by side.**
- *Code:* in a ranked setup, each new block needs a majority (`floor(n/2)+1`,
  `work-queue/number-quorum.ts:116`) of the computers that were members when ranking started
  (`setup/checkpoint.ts:192-211`, `:243-245`). The design note says plainly: "Electorate
  reconfiguration is not implemented. If fewer than a majority of the original voters remain in
  the setup, new numbers wait" (`docs/architecture/numbering-proof.md`). Beside it run Mac-ranked
  numbering with a 3-day "lost" rule (`work-queue/numbers.ts:169-226`) and the
  standby-confirmed ledger (`work-queue/numbers.ts:688-720`).
- *What users see:* with two computers, both must be on for each new block of 50. Remove or lose
  one, and after the current block runs out, new items never get a number again. A restored
  computer has fresh keys, which don't count as a voter (rule 51).
- *Likelihood:* medium-high. Replacing a computer is common.
- *Simplest fix:* the main's counter, with its high mark recorded alongside the relay lease
  (trim step 3). Delete the quorum, the voters and both older paths.

**8. Two update installers live in one function, and only one is ever used.**
- *Code:* `update()` (`updates/update.ts:605-1069`) has the atomic `bin` switch (`:860-923`) and
  the old file-by-file installer (`:925-1025`). The old one brings its rollback kit (`:346-405`),
  the health trial (`updates/health.ts`, 350 lines), the engine swap at a quiet moment
  (`updates/engine-swap.ts`, 581 lines), the bridge's own quiet-moment wait
  (`agents/renewal.ts`, 299 lines) and the keeper's restore path (`keeper/kuvo-start:1001-1090`).
  A box running this code only takes releases at least as new as itself, which all carry the
  switch keeper (`updates/update.ts:777-779`), so the old installer can't run.
- *What users see:* nothing directly. But `ON_TRIAL` ("still being checked: try again in a few
  minutes") still refuses updates (`updates/update.ts:611`, `:1527`), and "Something is running
  there now" (`:786`) belongs to the dead path.
- *Likelihood:* the cost is maintenance, and code that is never exercised.
- *Simplest fix:* delete it, as simple-updates' removal list already says (trim step 1).

**9. Changing the main, and so unlinking it, is refused while any member is old, off ones
included.**
- *Code:* "<name> needs to connect with a newer Kuvo Code before you can change the main
  computer." (`setup/handover.ts:65-66`), plus feature checks on every reader
  (`setup/handover.ts:133-140`). The hand-over is also refused while the main is fenced
  (`setup/handover.ts:61`). Unlinking the main needs a hand-over first (`peers/manage.ts:330`).
- *What users see:* they can't retire their old main while a computer that's been off for a
  week runs an older version.
- *Likelihood:* medium.
- *Simplest fix:* drop the all-member checks (members follow by version, N1, N2). Unlinking the
  main hands over automatically (rule 2 as rewritten; trim step 4).

**10. Moves need the main and its standby, and can be refused when the shared state is full.**
- *Code:* each move is a setup save of `ownershipAssignments` (`ownership/transfer.ts:96-152`),
  so it waits on the lease and the standby. All setup-wide state shares one signed document of
  at most about 170 KB (`setup/shared.ts:82`), and every moved record adds an entry. The refusal
  reads "The main computer has reached its saved ownership capacity."
  (`ownership/transfer.ts:152`).
- *What users see:* moving a session from pc4 to pc5 fails while pc2 (the main) or its standby
  is away. A heavy mover eventually hits a capacity refusal they can do nothing about.
- *Likelihood:* the main-or-standby wait is medium with three or more computers; capacity is
  rare.
- *Simplest fix:* a hand-over between the two computers (rule 46 as rewritten; trim step 5).

**11. Update Now can't move the setup forward, and computers ahead of the choice show as
behind.**
- *Code:* once a setup has a choice, Update Now only retries that choice
  (`updates/update.ts:1452-1453`). On a main already running it, the retry answers 409 "This
  computer is up to date, is making progress, or cannot retry yet." (`updates/progress.ts:164`).
  The feed is checked only every 6 h, and a release is taken 3 days after it's out
  (`updates/update.ts:85`, `:729`). Separately, a computer newer than the choice falls through
  to `behind` (`updates/progress.ts:79-80`, `:93`), and the summary counts it as not done
  (`:129-133`).
- *What users saw on 11-21:* three computers on beta.40 read "beta.40 · beta.38 available", and
  the notifier said "Updating to beta.38 · 0 of 4 done".
- *Likelihood:* certain whenever a newer release exists than the choice.
- *Simplest fix:* in the fleet rule, Update Now on the main checks the feed now and takes the
  newest signed release, with no 3-day wait. A computer at or above the main's version is up to
  date. A main behind a member takes that member's version (trim step 1). `simple-updates.md`
  assumes Update Now already "looks at the feed now", which isn't true at this commit.

**12. Members can only get a release pack from the main or the standby, in 30 KB mails through
the relay.**
- *Code:* `updates/fleet.ts:274-278` (`releaseAuthority`); parts of 30 KB, 16 at a time
  (`updates/fleet.ts:30-45`); backoff up to an hour (`updates/attempt.ts:35`). Yet the same
  member downloads the engine, up to 400 MB, straight from its URL
  (`updates/update.ts:829-842`), and the main downloads the pack from GitHub
  (`updates/update.ts:739`).
- *What users see:* updates are slow; packs up to 100 MB go as thousands of relay mails. When
  both the main and the standby are away, no member updates, even with internet.
- *Likelihood:* slow updates are certain; a stuck member is medium.
- *Simplest fix:* members fetch the pack from the signed manifest's URL and check its hash, as
  the main does. Peer transfer is kept only as a fallback for a build with no reachable URL
  (trim step 1).

### Should fix (real, smaller)

**13. Old setups stay on old authority models because migrations wait for every computer.**
Moving to checkpoints waits until every peer supports them (`updates/keep.ts:69`). The successor
chain waits the same way (`setup/takeover.ts:216-218`), and so does a merge
(`setup/merge-data.ts:99`). One computer that's off and old keeps the whole setup on layout 4
logic, so all three models stay live and tested. *Fix:* one model, converted at the main in one
step, after the update fix (trim step 4).

**14. Leadership flips every time a laptop main closes for three minutes.** The takeover delay
is the 60 s lease plus 2 minutes (`setup/lease.ts:17`). Each flip writes a checkpoint with
archive pages of every recorded operation (`setup/checkpoint.ts:221-253`) and bumps the epoch.
*Fix:* with the relay lease, always-on computers claim first and the main stays on the server.
Laptops claim only when no always-on computer is up.

**15. Old pending receipts and sealed epochs are kept for looking up past outcomes.**
`queryOperation` walks sealed primaries and legacy witnesses so a client can learn how a save
ended under an earlier epoch (`setup/shared.ts:404-446`, `setup/checkpoint.ts:124-161`). Once the
main saves at once, a save either succeeded in the reply or didn't. *Fix:* delete (trim step 3).

**16. Two processes answer clients.** `serve` (the desktop app) and the link (phones, other
computers) answer the same routes with different context (`docs/architecture/one-client-process.md`,
#924, designed and waiting). *Fix:* #924 as designed, after the trim. **A reminder for the user:
#924 is designed and waiting.**

**17. Pre-0.8 compatibility.** 84 `COMPAT(...)` markers in `box/src` (57 for 0.7, 16 for beta.1,
8 for beta.30, 3 for layout 4), the 0.7 Mac leadership and numbering, the Mac's front for the
retired app (`mac/relay.ts`, `mac/machines.ts`). Each is marked to go "on the user's word, once
no Mac from before 1.0 connects". *Fix:* after 0.8.0's forced update (decision D8) has run, set
a date and remove them in two or three sessions (trim step 7).

### Client-side logic (desktop app)

Rules 17 and 57 say the box decides and the client draws. These places still decide in the app.
I re-read the first five and the update-chip rule; the rest come from a read-only sweep of
`gpui/` and weren't re-read.

**18. The app holds boxes with its own gate, which the box already describes as display only.**
- *Code:* the app works out held, behind or open from the raw feature list
  (`gpui/kuvo_box/src/gate.rs:85-97`) and refuses every request outside its own allowed list
  before it reaches the box (`gpui/kuvo_box/src/client.rs:324-343`). The box already sends
  `compatibility` and says: "Compatibility is for display. Progress toward a release does not
  affect which routes may run." (`areas/compatibility.ts:8`). The baseline list is kept twice
  (`gate.rs:24`, `areas/compatibility.ts:6`). Per the sweep, the allowed-while-held list also
  differs from the phone's (`swift/KuvoClient/Sources/ClientSessions/BoxGate.swift:39`).
- *What users see:* a box the app holds shows "needs an update" while the box itself would
  serve the request.
- *Likelihood:* only for boxes without the baseline features, so rare now. But it's the same
  local-refusal design that 11-21 removed from the box.
- *Fix:* the client draws `compatibility` and sends every request. PROTOCOL.md "The update gate"
  (`:4094`) changes to match. The same change goes to KuvoClient for the phone.

**19. The Mac app runs its bundled installer's update every time it attaches, ignores the
result, and so bypasses the main.**
- *Code:* `gpui/kuvo/src/launch.rs:333-337`. The installer path skips the main's choice and
  `layoutHold` (`updates/update.ts:752`, `:880`, and `1219637d4`: "Let a release's own installer
  proceed when the main computer's choice is older"). The offer route the design relies on,
  `offer_release` (`gpui/kuvo_box/src/install.rs:463`), has no callers.
- *What users see:* a Mac's box jumps to whatever the app carries, ahead of the main. That's a
  second way versions change, against rule 13 and N4. A failed install shows nothing.
- *Likelihood:* certain on every Sparkle update of the app.
- *Fix:* in trim step 1, the app hands its bundled release to its box, and the box keeps and
  announces it (`simple-updates.md`, "Where releases come from"). The main then takes it within
  seconds. Run the installer only when the box can't start.

**20. Update notifier state is decided in the app.**
- The app overrides the box's per-client `dismissed` with a dismissal it saved itself
  (`gpui/kuvo_box/src/boxes.rs:446-454`, against `updates/progress.ts:154-156`). The notifier can
  stay hidden when the box says to show it.
- It keeps old rows' update lines when a list comes from a box without `update.fleet`
  (`boxes.rs:439-445`), so stale "Updating…" or "Failed" lines and buttons can stay up.
- When the update chip shows is decided by mixing the app's gate with `compatibility`, and by
  comparing the box's `detail` text with the app's own strings
  (`gpui/kuvo/src/workspace.rs:1053-1070`). If the box rewords its detail, the chip changes
  silently.
- *Fix:* the fleet summary from trim step 1 carries `shown` and the words. The app draws them.

**21. The app picks who can become main or standby.** `gpui/kuvo/src/computers_manage.rs:145-172`
filters candidates by reach and feature names. The box applies stricter rules
(`setup/handover.ts:65-66`, `:314-315`), so a user can pick someone and then get a 409. *Fix:*
standby candidates go with trim step 4. For Change main computer, the box's `kuvo/setup` lists
`canLead` per computer and the sheet draws that list.

**22. Smaller, from the sweep (not re-read):**
- The app chooses which computer's list, Update Now and Dismiss to use
  (`gpui/kuvo_box/src/boxes.rs:664-684`).
- It arbitrates between two boxes' update offers for one computer
  (`gpui/kuvo/src/computers.rs:988-1033`).
- It decides Show… versus Update now (`gpui/kuvo/src/fleet.rs:70`,
  `gpui/kuvo/src/title_bar.rs:562-575`).
- It marks rows held from its own gate and polls them every 10 s (`boxes.rs:780-785`, `:863`).
- It runs its own pairing retry every 60 s (`boxes.rs:809-844`).
- It picks which computer to suggest switching to (`gpui/kuvo/src/workspace.rs:1474-1483`).

Most of these go away with the fleet rule: one lister, which is the main, and one summary. Do the
rest when that code is next touched.

The sweep also checked these, and they're fine: SSH-or-relay choice (`connections.rs`, a
per-device choice by PROTOCOL.md `:537-542`), finding the local box (`place.rs`), Sparkle, and
stream backoff.

### Edge cases (attacker-only or several coincidences)

- **E1. The relay is down.** Today every lease and all peer mail stop, so every multi-computer
  main is fenced. With the relay lease it's the same, but the box can say so plainly ("Kuvo's
  relay can't be reached: setup changes wait"). Not a regression.
- **E2. The main and the relay on one machine** (the user's own setup). Losing that machine
  loses both. Warn about it (decided).
- **E3. Personalities filling the shared state.** The same ~170 KB limit can refuse a large
  personality library ("These existing preferences exceed the setup's saved preference
  capacity.", `setup/seed.ts:283`). With moves gone from shared state (trim step 5), the
  headroom grows.
- **E4. Proof and evidence layers built against a lying computer.** Issuer addresses and
  forward evidence that follow messages through moves, a SQLite receipt index, and
  re-authorising a revoked device's queued work (`ownership/forward-evidence.ts`,
  `ownership/receipt-index.ts`, `ownership/submissions.ts`, `queued-device-authority.md`). With
  trusted computers (N5), operation IDs with owner-side dedupe cover the user case: a message is
  delivered once, and "delivered" appears only after the owner accepts it. Simplify only when
  the code next needs changing; it doesn't block anyone today.
- **E5. Installer-code enrollment after the issuing main is lost** (provisional claims and
  cancellation commitments, PROTOCOL.md "Adding from any client"). It needs the main lost during
  a 15-minute code window. Re-adding is a fine answer.
- **E6. Merging setups rewrites "#NNN" in item text and notes and keeps aliases by origin**
  (PROTOCOL.md "Merging two setups"). It's rare after 0.8.0 (#801). Keep it as built, but don't
  extend it.

---

## 5. The trim plan

Ordered so that nothing locks anyone out midway. Each step leaves every computer usable on its
version, and each step's release reaches computers through the update path as it stands after
the step before it. Estimates are session wall-clock at today's pace.

| Step | What | Removes | Tests (each fails on today's code) | Wall-clock |
|---|---|---|---|---|
| **1. Updates follow the main** (simple-updates step 2, widened) | The fleet rule as designed: `staged` and `cannot` facts, the main switches when every computer that's on is ready or five minutes after the others, members follow the main's running version. Remove `layoutHold`, the choice as a setup save (keep writing the old record for followers up to beta.40), offers, backoffs and the old installer (finding 8). Update Now looks at the feed now with no 3-day wait. A computer at or above the main's version is up to date, and a main behind a member takes its version. Members fetch packs from the manifest's URL, with peer transfer as a fallback. No backup before an update. The Mac app hands its bundled release to its box instead of installing it, and the box keeps and announces it. The fleet summary says whether the notifier shows, and with what words. **Ships with the folder layout unchanged (23)** so it passes today's checks. | Findings 4, 6 (update), 8, 11, 12, 19, 20; part of 5 | simple-updates T1, T3. Plus: a member ahead of the main shows up to date; Update Now on the main takes a newer signed release; an update goes ahead with the backup disk unplugged. | 60-90 min |
| **2. The relay lease** | Relay (its own repo and container, deployed first): claim, renew, read; term, expiry, Work Queue high mark; a two-minute grace after restart. Box: the main renews every 10 s; a main whose lease lapses stops setup-wide changes and says "Kuvo's relay can't be reached"; takeover is a claim after expiry, always-on computers at once and others after one more minute; a returning main sees a higher term and steps down; warn when the main is the relay's host. Keep answering the old standby's ping for one release so beta.40 standbys don't take over. | Finding 1, 14; the standby lease | A two-computer setup with the other computer stopped still pairs a phone and adds a computer. The relay restarts and nobody takes over. The main is cut off and the other takes over; the old main steps down. | 60-90 min, plus the lead's relay deploy |
| **3. The main saves alone** | Shared saves commit at the main and go to every member at once. Delete pending saves, 202 receipts, one-pending-per-kind, `queryOperation` over sealed epochs, `sharedSaves` and "requires standby". Work Queue blocks come from the main's counter, with the high mark sent with the lease. Delete the quorum and both older numbering paths. | Findings 2 (with step 4), 3, 7, 15 | Remove one of two computers and new items still get numbers. After a two-computer takeover the new main pairs a phone. A dead standby is removed at once. | 45-75 min |
| **4. One authority model, no standby** | Delete the standby role, successor ranks, rank refreshes, pins, probes, fork merging and certificate gossip. The term from the relay is the epoch. Convert legacy, layout 4 and checkpointed setups at the main in one step. Delete all-member migration and hand-over checks. Unlinking the main hands over automatically. UI: remove "Choose standby" and show only the main in Setup roles. **This UI change needs the user's OK first (drafts rule).** | Findings 2, 9, 13 | Unlink the main of a two-computer setup with the other computer on an older version. Old layout 4 and checkpointed fixtures convert and keep their members. | 60-90 min |
| **5. Moves without the main** | A hand-over between source and destination; each computer announces what it owns; read old `ownershipAssignments` for one release, then stop writing it. No backup before a move. | Finding 10; part of 6 | Move pc4 to pc5 with pc2 off. The source dies mid-move, then retries, and the move still runs once. | 45-75 min |
| **6. Docs and bench** | PROTOCOL.md "Your setup" and "Saving setup-wide changes" shrink to the new model; rules 53 and 54 bench cases (the relay lease, a lost two-computer standby, 0.7.2 and beta.35 with one computer off, simple-updates T5 and T6); `docs/footprint.md`; CHANGELOG; kuvo-web docs per `docs-map.md`. | — | The new bench cases. | 45-60 min |
| **7. Clean-up after 0.8.0** | Once 0.8.0's forced update has run and the user names a date: drop the layout readiness checks for layouts at or below 23; drop `COMPAT(0.7)`, `beta.1` and `beta.30` paths and the Mac front for the retired app. | Finding 17; the rest of 5 | Each removal's own fixtures, from 0.7.2 and beta.35. | 2-3 sessions of 30-45 min |

Steps 1 and 2 can run side by side: step 2's box part depends on the relay, not on step 1. Step
3 needs step 2, because number safety needs the lease. Step 4 needs step 3. Step 5 can follow
step 3 in parallel with step 4. Step 6 comes last of the trim. Step 7 waits for the user's date.
**#924** (one client process) follows the trim.

**Decisions for the user, all from the lead's list:**
1. The rewritten and dropped rules above (2, 13, 22, 31, 32, 33, 46, 49, 53, 54, 56, 57).
2. Removing "Choose standby" from the Computers page that was just locked (step 4). I recommend
   removing it: it names a role that no longer exists.
3. Accepting that a setup change made in the last moment before the main's disk dies can be lost
   (rule 33's replacement).

---

## What I checked

I read the product code at the exact commit for the setup layer (`setup/lease.ts`, `takeover.ts`,
`shared.ts`, `access.ts`, `record.ts`, `handover.ts`, `checkpoint.ts`, `removal.ts`), updates
(`update.ts`, `choice.ts`, `keep.ts`, `fleet.ts`, `attempt.ts`, `progress.ts`), numbering
(`numbers.ts`, `number-quorum.ts`), moves (`ownership/transfer.ts`), backups (`backup/engine.ts`),
the folder layout (`shared/folder-version.ts`), the app's gate (`gpui/kuvo_box/src/gate.rs`,
`client.rs`), PROTOCOL.md "Your setup" and the design notes named above. One read-only sweep of
`gpui/` found the client findings, and I re-read the ones marked as re-read. I used the existing bench
tests H1439 and H1649 as evidence for the fence and the retirement wait. I ran no tests: the code
and those tests settle each claim, and a bench run would only repeat them. I made no product
changes; this file is the only one written.
