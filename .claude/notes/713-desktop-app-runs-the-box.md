# #713 The desktop app and this Mac's box: design

Status: design only, written 2026-11-14 against main at `98adca21f` (after the layout move, before
#712), revised after one reviewer pass (§10). Nothing here is built. Read with
`333-own-box-independent.md` and `333-piece-0-results.md` (main checkout's `.claude/notes`,
untracked), `693-own-box-browser.md`, and `protocol/PROTOCOL.md` ("Updates" :2203, "The update
gate" :2428, "The keeper" :6057, "A home of its own" :6156, "Moving this Mac's box" :6199, "This
Mac's box: its helper" :6655, "For released Macs" :8579). Swift paths are under `swift/`, which
stays in the repo as read-only reference (the lead, 11-14).

## 0. In short

- **The box keeps itself, as a machine's does. On a Mac its launchd program is #333's helper, Kuvo
  Code Box.** That's the user's leaning ("a login item that starts the box and keeps it running,
  even with the app closed; the desktop app installs it once"), and I recommend it, with two
  refinements:
  1. **The box installs the helper, not the app.** The desktop app runs one box command from the
     release it carries; the box writes the launch agent (`kuvo-link.js helper install`, built).
     The app never writes a plist or calls `launchctl`.
  2. **The box updates the helper, not the app.** The helper is a bundle in the box's signed
     release, swapped whole (built, #333 piece 7). The app only offers its release
     (`kuvo/box/update {from}`, built).
- **The desktop app's native part is small:**
  - running the box's installer from its bundle;
  - Sparkle, to update itself;
  - opening a System Settings pane;
  - carrying a tiny signed Swift tool for the one-time Keychain hand-over.
  Everything else goes in the box: when and how to move, stopping the old box, 0.7's upgrades, and
  permissions as facts.
- **0.7 → 0.8:** Sparkle in the user's Swift app installs the desktop app (same `run.kuvo.code`,
  same team, same feed key). The desktop app runs first and asks the box's installer what this Mac
  holds. The box then moves itself (most of it is #333's `rehome`, built), starts fresh, or the app
  just attaches.
  - What's new: the box runs the move without the Swift keeper, and brings 0.7's data and engine
    database up to date itself, in its staging copy only.
  - Until the move succeeds, nothing runs (a 0.7 Mac never had a box running anyway), so every
    refusal must be one the user can act on at once.
- **#715:** the box's files and its release are built by the box's own build (`scripts/build-box.sh`
  and a new `scripts/box-release.sh`), from sources in `box/`, never by running an app. The desktop
  app carries that release, and machines update from the same files.

## 1. Today (read, not run)

- **The Swift app keeps its box with its own executable.**
  - `--box`, registered with `SMAppService.agent(plistName: "run.kuvo.code.box.plist")`
    (`swift/KuvoCode/Stores/BackgroundWork.swift:42-44`). The plist's `BundleProgram` is
    `Contents/MacOS/${EXECUTABLE_NAME}` and its `KeepAlive` is `SuccessfulExit: false`. It runs
    the box in `~/Library/Application Support/run.kuvo.code/box` (`S/box`).
  - With `movesOwnBox` on (off in every build), that keeper runs the move into `~/.kuvo-code`
    (`K`). It copies the Keychain items as the app's identity (`KeychainMove`), loads the helper,
    settles what's owed and unregisters itself (`swift/KuvoCode/Keeper/OwnBoxMove.swift`, 528
    lines; `OwnBoxMove.swift:518`).
- **The helper is built and merged, behind the flag.**
  - Kuvo Code Box (`run.kuvo.code.box`) is Swift: `swift/OpenCodeKit/Sources/OpenCodeKit/BoxHelper/`
    (2,077 lines), the product `KuvoCodeBox` (`swift/OpenCodeKit/Package.swift:10,14`).
  - It's built into the app by `scripts/build-box-helper.sh:33-35`, packed into the box's release
    by `scripts/pack-box.sh`, and installed by the box (`helper install|start|status`,
    PROTOCOL.md:6666-6690).
  - It's the responsible process for everything the box runs (piece 0 §1, user checks 2a–3c). It
    holds the box's Keychain copies (§2, 4b/4c) and starts `serve` with the vault's secrets on
    stdin (PROTOCOL.md:6716-6723). It runs only the real `K` (PROTOCOL.md:6691).
- **The move is the box's: `kuvo-link.js rehome`** (PROTOCOL.md:6199-6402). Three of its rules
  assume a Swift keeper ran first:
  - It refuses a box that "hasn't started since Kuvo Code updated" (`box/src/link/rehome/move.ts:216-219`:
    `run/served.json` must name the app's version and the link's).
  - It accepts only the pinned engine 2.2.6's schema, `KNOWN_SCHEMAS = ["59 20261030054400_job"]`
    (`box/src/link/rehome/database.ts:19`). 0.7.2 and beta.1 run engine 2.0.32 (`v0.7.2`'s
    `OpenCodeKit.swift:5`); in #333's path, a keeper-era app migrated the database first.
  - `--new` needs the engine already in the app's folder (PROTOCOL.md:6381).
- **0.5–0.7.x and 0.8.0-beta.1 have no `S/box`.** Their upgrades into a box are Swift, run as the
  app opens:
  - `GroupMigration.run` (`swift/KuvoCode/Stores/GroupMigration.swift:61`, reading the app's
    defaults), `Personalities.moveToBox` (`Personalities.swift:211`), `BoxGroups.takeOldCache`
    (`BoxGroups.swift:121`);
  - and about ten more listed under "For released Macs" (PROTOCOL.md:8579 on): `botEditOutbox`,
    `agent-letters*.json`, `agent-messages.json`, `copilotClaudeModels`, `movesUnderWay`, Settings
    › Git from preferences, `passwords.agentSignIn*`, Keep This Mac Awake.
- **The desktop app (`gpui/`) only connects** (`gpui/kuvo/src/main.rs:24`).
  - `place::this_mac` takes `K` when `box/kind` says `mac`, else `S/box`
    (`gpui/kuvo_box/src/place.rs:47-54`). It reaches the box over `run/serve.json`
    (`place.rs:75-92`, `local.rs`).
  - It has no bundle, signature, updater, release, or way to start or install a box; its window
    says "isn't running". Its executable is `kuvo`.
- **The box's release is made by running the Swift app.**
  - `release.yml:220-232` runs `scripts/app-box-release.sh`, which runs
    `"Kuvo Code" --write-box-files` (`scripts/pack-box.sh:29`).
  - That writes `BoxRelease.write` (`swift/KuvoCode/Stores/BoxRelease.swift:22-55`): `kuvo-link.js`,
    `kuvo-browser.js`, `kuvo-agents.js`, the catalogs (`MachineBox.swift:56,106`, from Swift's
    `Integrations.all`), the bots' words (`swift/KuvoLink/Sources/KuvoLink/Resources/bot-words.json`),
    `box-version`, `box.json`, and the engine's URLs and integrity
    (`swift/OpenCodeKit/Sources/OpenCodeKit/Engine/EngineInstaller.swift:23-40`).
  - It also writes `BoxFiles.write` (`swift/OpenCodeKit/Sources/OpenCodeKit/Engine/BoxFiles.swift`):
    `kuvo-start`, `kuvo-launch` and `kuvo-setsid`, as shell held in Swift strings
    (`RemoteEngine.swift:1051`, `:922-927`, `:542`, `bootFiles` `:1344`), and `.kuvo-boot`.
    `kuvo-wake`, the integrations' launcher, `kuvo-npx` and their runtime and plugin come from
    their `installScript`s (`Wakes.swift:311`, `Launcher.swift:130`, `CommandEnvironment.swift:81`).
  - The app carries the result as data in `Contents/Resources/box-release` (`boxes.json` and the
    pack; `scripts/app-box-release.sh`).
- **What only the Swift app answers or does, with no replacement yet:**
  - the phone's `kuvo/machines`, `kuvo/hosts`, `kuvo/asks`, `kuvo/mac` and `kuvo/engine`, through
    the app's host (`MAC_APP_ROUTES`, `box/src/link/mac/relay.ts:41,72,86`);
  - adding and repairing machines over SSH (`RemoteEngine.swift`), and writing `machines.json` and
    `phone-link.json` (PROTOCOL.md:6588).

## 2. Who keeps this Mac's box (question 1 of the brief)

| | A. The desktop app keeps it | B. The helper, installed by the desktop app (user's leaning) | C. The box keeps itself, like a machine |
| --- | --- | --- | --- |
| Starts at login, runs with the app closed or removed | no, or only while the app is a login item | yes | yes |
| App updates restart the box | yes (0.8's `Keeper.update` problem again) | no | no |
| macOS asks about | the app | Kuvo Code Box | on a Mac, "sh" and each binary by its own name (piece 0 §1) |
| Keychain without files | yes | yes (piece 0, 4b/4c) | no: keys in files |
| Logic a new client rewrites | the whole keeper (Swift's was 2,782 lines) | nothing: install is one box command | nothing |
| Built | no | yes, behind the flag | yes, for machines |

**Recommend B as C on a Mac.** The box keeps itself, as on every machine (`kuvo-start`, signed
updates, `update.ts`). On a Mac its launchd program is the helper. That isn't a third program
with its own logic: the helper is the box's macOS shell, for what only a signed program can be
(TCC's responsible process, the Keychain, `serve`'s secrets).

- **"Started by the desktop app" means once, at install,** through the box's command. After that,
  launchd starts it at login, and the box restarts and updates its pieces itself.
- **If Login Items is off for it,** the app asks the box to start it once (`helper start`), and
  the box opens the bundle with `open` (§3).
- **A rejects itself.** It would rebuild the Swift keeper in Rust, put the box's life in a client,
  and restart the box with every app update, which is what #333 removed.
- **Plain C on a Mac** (`/bin/sh` as the program) loses the shared identity, so each binary is
  asked about by its own name and asked again at each update (piece 0 §1). It also gives up the
  Keychain.

**A future Linux desktop app** runs the same installer from its bundled release, with no helper.
Its box is machine-kind:

- it starts as a machine's does today (the `@reboot` line, `docs/footprint.md`'s exception);
- it keeps its keys in files;
- the app attaches in place as to a machine's box on its computer (`x-kuvo-from: mac`,
  PROTOCOL.md:6549-6556), not through `this_mac`'s `mac` kind. That's a small `kuvo_box` change
  (`place.rs`, `local.rs`'s `Speaker`), not one line.

**A Windows desktop app has no box of its own.** Box code runs only on macOS and Linux. On Windows
the app is a client of the user's other boxes. Its own box would need WSL or a port, which the
app should say plainly. Not in this work.

## 3. What the desktop app must do natively on macOS

1. **Run the box's installer** from its bundle (§4.1), as a child process, and draw its JSON lines.
   That's all the app does to "install the helper".
2. **Update itself with Sparkle**, embedded through `objc2`.
   - It reads the same feed (`SUFeedURL`), verifies with the same `SUPublicEDKey`, and uses the
     same `scripts/appcast.sh`.
   - Updating itself is one of CLAUDE.md's "only in a client" items, and Sparkle keeps 0.7's Swift
     apps able to update into it (§5).
   - The app keeps 0.7.2's and beta.1's `SMAppService.mainApp` registration (Open at Login) as it
     is: same bundle, same login item. Settings shows it.
3. **Open a System Settings pane** when the box says a permission is missing
   (`open x-apple.systempreferences:…`).
4. **Carry `kuvo-keys`, the one-time Keychain hand-over**, in `Contents/MacOS/`.
   - It's a tiny Swift tool built from the helper's package (§3, the helper) around the tested
     `KeychainMove.copy` and `deleteOriginals`. It's signed with the identifier `run.kuvo.code`
     and the team, so it meets the original items' designated requirement and reads them without
     asking (piece 0: access goes by the designated requirement, not the path; to confirm with
     the real builds in piece 0).
   - The box's move runs it (`kuvo-keys copy --home K --helper <the helper the release brings>`,
     then `kuvo-keys delete-originals`). The app's window never does Keychain work.
   - It also reads 0.7's pre-vault internet passwords (`Passwords.swift:252`) into a journaled,
     `0600` hand-over file in `K/updates/rehome/`. The box takes them into its vault once `serve`
     runs (`owed: passwords`), then deletes the file.
   - **Why not the helper:** it can't read the originals (-25293), and editing their access list
     asks once per item (piece 0 §2).
   - **Why not Rust in the app:** it would mean FFI to deprecated security APIs, for code that
     goes away later. If piece 0 finds a nested tool with the app's identifier refused, the
     fallback is that port.
   - It goes only by your word, once no 0.7 Mac is expected to update (PROTOCOL.md's rule for
     0.7), never on a date: a Mac updating late would otherwise lose its keys.

**Not in the app:**

- **Whether permissions are granted.** The helper preflights Screen Recording and Accessibility
  (`CGPreflightScreenCaptureAccess`, `AXIsProcessTrusted`: it imports CoreGraphics already) and
  writes facts the box serves (`kuvo/box/permissions`; not `kuvo/mac`, which the app answers
  today).
  - Files & Folders and Local Network have no preflight. The box says them from what fails
    (`EPERM` in a protected folder, `EHOSTUNREACH` on the LAN), as #333 §4.3 plans.
- **Login Items state.** The box says it (`helper start` answers `disabled`).
- **Opening the helper with Login Items off.** Today's PROTOCOL.md:6686-6690 has the app run
  `open -g -j -n <bundle>`. `helper start` does it itself: `open` goes through LaunchServices, so
  the helper is still its own responsible process.
- **The helper's install, update and removal:** box commands.
- **The old keeper's launch agent:** the box (`launchctl bootout`, §5 case C). There's no `--box`
  handling in the desktop app.

**The notarized helper.** It stays Swift, built from its own sources, signed and notarized. Its
language doesn't matter to any client, it's piece-0-verified and reviewed twice, and Linux and
Windows never need it.

- With `swift/` read-only, its sources (BoxHelper, plus the few OpenCodeKit files it uses) and
  `kuvo-keys` move to a package of their own (`mac/helper/`, question 2).
- `build-box-helper.sh` builds it on its own (not into an app), keeping its ID
  `run.kuvo.code.box`, its team and its entitlements, or macOS forgets the user's grants and its
  Keychain items (piece 0 §1).

**Dev builds never install or move.** `cargo run -p kuvo` takes the real `$HOME`. Installing and
moving run only from a signed `run.kuvo.code` bundle, or into a demo root
(`KUVO_DEMO_MACHINE_ROOT`). Any other build only attaches.

## 4. Where each piece of logic lives

### 4.1 Install, in the box: `kuvo-install` and `kuvo-link.js install`

The desktop app carries what the Swift app carries today, as data (`Contents/Resources/box-release`:
`boxes.json` and the pack), plus `kuvo-install`, a short POSIX shell script from `box/`. No
executable code sits in `Resources` outside the pack.

- **`sh <app>/Contents/Resources/box-release/kuvo-install <verb> --app <bundle>`.** On a new Mac
  there's no engine to run JavaScript with, so the first step is shell:
  1. it checks the pack's SHA-256 against `boxes.json`;
  2. it unpacks it into `K/updates/install/release/`;
  3. it fetches the engine for this system (the manifest's URL and npm integrity, checked as
     `update.ts` checks) beside it;
  4. it hands over to that release's `kuvo-link.js install …` on that engine's Bun.

  The app's code signature seals `boxes.json` and the pack, so no runtime signature check is
  added.
- **`install --status`** reads the folders (`here`'s judgment) and answers one of these. §5's table
  says what each does.

  | Verdict | What this Mac holds |
  | --- | --- |
  | `fresh` | nothing |
  | `ours` | a Mac-kind box in `K` |
  | `app-07` | 0.5–0.7.x or beta.1 data in `S`, no `S/box` |
  | `app-box` | `S/box`, a keeper build's |
  | `moving` | a move under way |
  | `stuck` | stopped past the point it goes back from |
  | `waits` | another move holds the lock |
  | `back` | moved back |
  | `machine` | a machine's box in `K`, nothing in `S` |
  | `two` | a machine's box in `K` and data in `S` |

- **`install --new`** is `rehome --new` with the release and engine from `K/updates/install/`
  rather than the app's folder. Then `helper install --load`.
- **`install --move`** is §4.2.
- **The app** runs `--status`, then the verb it names, and draws the result. For `ours` it offers
  its `box-release` folder (`kuvo/box/update {from}`, as today) and attaches, as #333 piece 6b
  does.

### 4.2 The move without the Swift keeper: `rehome --run`, in the box

The Swift keeper's outside steps (PROTOCOL.md:6404-6451) go into the box:

- **`kuvo-link.js rehome <root> --run --app <bundle>`** runs detached (`setsid`), so quitting the
  app doesn't stop it, and holds the move's lock. It:
  1. asks `--ready`;
  2. stops the old box. For `app-box`, that's the Swift keeper's `POST stop` (its control is in
     `run/keeper.json`, PROTOCOL.md:6087-6096); a keeper running from the replaced bundle's file
     is stopped by its pid, and its `keeper.lock` waited for. For `app-07`, it checks that
     nothing holds `S`'s engine database: an exclusive lock taken briefly at Plan and again before
     Clear, so an orphaned or second 0.7 engine refuses the move. The clear step skips the
     engine's folder (`move.ts:541`), so without this its writes would be lost;
  3. installs the release from `K/updates/install/release/`, and runs `kuvo-keys copy` trusting
     that release's helper (the same designated requirement as its final place in `K/bin`), then
     `Kuvo Code Box keys check`;
  4. runs the move it already has, with two new steps in staging, after Clone and before Rewrite:
     - **Upgrades:** 0.7's and the keeper builds' (§1's list, inventoried in piece 4), each
       reading `S`'s files or the app's preferences (`/usr/bin/defaults export run.kuvo.code -`)
       and writing into staging.
     - **The engine's database brought to the pinned schema:** the release's engine run on the
       staged copy only, isolated as the check step's engine is (`engine-check.ts`: no network,
       settings and home of its own). Then the schema check, then Rewrite.

     `S` is never changed, so the kept copy is the untouched 0.7 `S`;
  5. settles what's owed: `kuvo-keys delete-originals`, then `agent` (the old keeper's label
     booted out, §5 C), then `passwords` once `serve` runs.
  It writes progress to `state.json`, which the app draws from `--status` before `serve` is up.
- **`served.json`'s rule** (`move.ts:216-219`) becomes "the box's own upgrades ran in staging".
  There's no app version left to compare with.
- **Pacing.** `--ready`'s rules were built for a world where the old box keeps running: an hour
  between tries, then waiting for Move Now after three. Here, for `app-07`, nothing runs until the
  move succeeds, so they change:
  - `--run` started by the user (the app's Try Again) has no wait and no try cap;
  - every refusal says, in words, what to do (free space; unlock the Keychain; go online for the
    engine; quit Kuvo Code 0.7 wherever it's open);
  - "Nothing runs on this Mac until this is done" is said plainly.

  `app-box` keeps today's pacing, since its old keeper still runs the box meanwhile.
- **Interrupted.** A move cut short (app quit, logout) resumes at the next `install --status`,
  which runs at each app open. There's no login item yet then, so it waits for the app to open.

## 5. From 0.7 to the desktop app (question 3)

Same `run.kuvo.code`, same Developer ID team, same Sparkle key. To check in piece 0, in the
rehearsal account (it needs the real ID):

- that Sparkle 2 in 0.7.2 and beta.1 takes an archive whose executable differs;
- that it puts the app at the same path;
- that the moved TCC grants and the Keychain's designated-requirement check hold for the new
  build and for `kuvo-keys`.

**Where each kind of Mac starts:**

| Mac | Has | What happens |
| --- | --- | --- |
| `fresh` | nothing | `install --new`: the box in `K`, helper loaded, attach. |
| `app-07`: 0.5–0.7.x, 0.8.0-beta.1 (every user) | data in `S`, no `S/box`, no keeper | `install --move` at once: upgrades and engine database in staging, move, Keychain hand-over, helper loaded, attach. A few minutes. |
| `app-box`: a keeper build (main, try-update; 0.8.0 if it ships) | `S/box`, an `SMAppService` agent pointing into the replaced bundle | See below. |
| `ours` | `K` with `box/kind` `mac`, helper | Attach, offer the release. |
| `back` | moved back, then 0.8 installed again | Nothing starts by itself. The app says the box was moved back and offers **Move Again** (`kuvo/box/rehome {now}`, then `--run`). |
| `machine` | a machine's box in `K` (a Mac set up as one), nothing in `S` | No Mac-kind box is made over it. The app attaches as to a machine's box here (PROTOCOL.md:6549) and says this Mac is set up as a machine. Making it this Mac's own is 183's work. |
| `two` | a machine's box in `K` and data in `S` | As #333 §3.7: no move, said plainly. |

**The `app-box` case.**

- The old keeper sees a new app at its place and makes way once quiet (`Keeper.update`).
- launchd then can't start it again: `BundleProgram` names `Contents/MacOS/Kuvo Code`, which the
  new bundle doesn't have. Piece 0 checks this.
- The desktop app's `install --status` says `app-box`, and `rehome --run` stops whatever is left,
  moves, and boots the old label out.
- A stale Login Items entry may stay. This case exists only on your own Macs (question 5), so it's
  a support path, written in `troubleshooting.md`.

**What runs first** is the desktop app, which Sparkle relaunches. Its window shows the box's
progress (drafts: "Moving your sessions to their new place on this Mac…", with the step), and
starts nothing else meanwhile.

**Who moves the box:** the box (`rehome --run`). The app only starts it. The app's signed tool
lends its identity for the Keychain step.

**Kept intact:**

- Sessions, chats, bots, wake-ups, group chats, worktrees, attachments: `rehome` (built and
  tested), now after the engine database is migrated in staging.
- This Mac's identity: whatever link and signing keys the Keychain holds are copied, never made
  anew. 0.7 Macs likely have none, since the phone link was never on in a release; beta.1's peers
  may. Where there are none, the helper makes them at first use, as for a new Mac. No second box
  ever runs under one identity: the originals are deleted only after Clear, and the helper starts
  nothing mid-move (built).
- Saved passwords and the vault: the vault's items through the copies; 0.7's older internet
  passwords through `kuvo-keys` and `owed: passwords`.
- Integration and bots' keys: the Keychain copies, plus key files the box writes.
- Added Claude Code accounts: renamed (built).
- Permissions: asked once more, for Kuvo Code Box (#333 question 2). The app's own grants stay the
  app's.

**Going back (question 8).** For an `app-07` Mac, Move Back (Settings › This Mac's Box, 30 days)
is the kept copy put back, not `--back`'s keeper layout, which no released app reads. It's a new
`rehome --restore`:

1. The box in `K` is stopped and its helper's agent removed (`helper` uninstall's steps).
2. The Keychain items are made again under the app's names, trusting the app (`keys back`, built).
3. `kept/<date>` is renamed back into `S`: the untouched 0.7 folders and database.
4. `K` is set aside whole into `K/updates/rehome/after-<date>`, not deleted. Work done since the
   move stays there only, and the app says so before you confirm.

Then you install 0.7.2 from its download page; it reads `S` as it always did. Its Sparkle will
offer 0.8 again, and 0.8 then shows the `back` state. `--back` (the keeper layout) stays for
`app-box` Macs only.

**A stepping stone instead?** The other way is a last Swift release with `movesOwnBox` on,
its feed pointing to the desktop app's: 0.7 → that release moves, and that release → 0.8 just attaches.

- Less new code: no `rehome --run`, no box-side upgrades, no `kuvo-keys`.
- But it needs piece 8's UI built in Swift (thrown away a release later), the Swift app built and
  released after #712, and every user to pass through that release. Anyone who skips it (0.8 downloaded
  over a 0.7 install) still needs §4.2.
- **I recommend against it** (question 3). It remains the fallback if piece 0 finds Sparkle won't
  install the desktop app over 0.7.2.

## 6. #715: the box's release, built by the box

| Today (Swift, by running the app) | Afterwards (in `box/`, built by `scripts/build-box.sh` or the new `scripts/box-release.sh`) |
| --- | --- |
| `kuvo-start`, `kuvo-launch`, `kuvo-setsid`, `.kuvo-boot` (shell in `RemoteEngine.swift`) | `box/src/keeper/*.sh`, plain files, byte for byte, the one copy; `.kuvo-boot`'s hash written by the build |
| `kuvo-wake`, the integrations' launcher, `kuvo-npx`, their runtime and the engine's plugin (`installScript`s) | `box/src/keeper/` and `box/src/plugins/`, plain files |
| catalogs (`MachineBox.catalogJSON`, `integrationsJSON(forMac:keysInBox:)`, from Swift's `Integrations.all`) | the table itself moves to `box/src/shared/integrations/` as data; the three catalogs are generated from it by the build |
| bots' words (`bot-words.json`) | moved to `box/src/shared/` |
| the engine's pin (`EngineInstaller.swift:23-40`) | `box/src/shared/engine-pin.json` (version, URLs, integrity per system), which `kuvo-install` reads too; the weekly upstream check updates it |
| `box-version`, `box.json` | written by `box-release.sh` from `VERSION` |
| the helper bundle, from the app's `Contents/Helpers` | built by `build-box-helper.sh` on its own, signed and notarized as a zip, then packed |
| `release.yml` builds the Swift app, then packs | `release.yml`: build the box; build and notarize the helper; pack and sign `boxes.json`; then build the desktop app with `box-release` inside |

- **Byte-for-byte check:** once, a test diffs the new release against the last Swift-made one, so
  machines see no change.
- **Adding machines:** whatever sets up a machine later (the desktop app's or a box's, §8 piece 8)
  takes these files from the release, never a second copy.

## 7. How #333 and #693 reorder (question 4)

- **#333:** pieces 0–7 stay as merged.
  - **Piece 5's keeper half** (Swift) is replaced by §4.2's `rehome --run` (box) and `kuvo-keys`
    (§3.4).
  - **Pieces 6a and 6b's client half** is what `gpui/kuvo_box` does by design (attach as
    `x-kuvo-from: app`, `place.rs`). Left for the desktop app: peers and pairing (#644), and
    `kuvo/box/app` only while #693 isn't done.
  - **Piece 8 (UI)** moves to the desktop app: the move's states, Settings › This Mac's Box (starts
    at login, where it is, Move Back, Uninstall…), permissions for the helper, `two`, `back`. The
    Swift drafts branch (`kuvo/333-piece-8-…`) is reference only.
  - **Piece 9 (rehearsals)** goes from 0.7.2, beta.1, a 0.6 build and a keeper build to the
    desktop app, then back, in the rehearsal account.
  - **`movesOwnBox`** ends as a Swift flag. The desktop app ships when §8's pieces pass.
- **#635** (vault and saved passwords on an attached box: this Mac's key joins the box's Macs
  through the signed setup) is box logic plus the desktop app's drawing. It's needed before 0.8:
  the desktop app has no Swift vault path to fall back on.
- **#644** (Settings › Phone while attached) is a desktop-app Settings page, drafts first, against
  the box's link routes (built).
- **#693** pieces 0–4 stand (1 and 2 merged). Pieces 5–9 need the box in `K` (§8 piece 4):
  - **5 (a Mac-kind box runs its browser)** comes before the desktop app ships, since the desktop
    app has no WebKit panel. `browser.own` is on from the start for a box the desktop app
    installed or moved.
  - **6 (Swift draws the box's browser)** goes. **11 (GPUI live view)** replaces it and is
    required for 0.8.
  - **7 (WebKit scenarios on the box)** stays.
  - **8 (rehearsals)** folds into #333 piece 9.
  - **9 (old path goes)** shrinks to the box's app hand-off (`appBrowser`, `kuvo/box/app/open`).

## 8. Pieces, in order

Sized in wall-clock session hours, start to merge-ready, review included.

- **From main's history** (first commit to merge, not counting work before the first commit):
  - #333 piece 2b: 4,672 lines, 2 h;
  - piece 4: 3,269 lines, 1.5 h;
  - piece 5: 2,471 lines, 17 commits, four review rounds, 3 h;
  - piece 7: 1,291 lines, 1.5 h;
  - 6a: 1.9 h;
  - #679: 5.5 h.
- **The #333 note sized the same pieces at 8–15 h each.** Most of a session goes before its first
  commit, so the ranges below lean on #333's estimates, not on commit spans.

**Tests never touch real data:**

- `KUVO_*` unset;
- a `HOME`, demo root and test bundle family of their own (`run.kuvo.code.trial713.*`);
- a test keychain;
- `launchctl`, `security` and `crontab` stand-ins on the PATH (`KUVO_TEST_UNINSTALL_TOOLS`);
- each box piece also runs on Linux in a devbox test folder;
- anything that needs the real `run.kuvo.code` ID (Sparkle, Keychain identity, TCC) runs only in
  the rehearsal account.

| # | Piece | Where | Tests | Risk | h |
| --- | --- | --- | --- | --- | --- |
| 0 | **Spikes** (rehearsal account for the real ID): Sparkle 0.7.2 → a GPUI bundle with the same ID (installs, path kept, relaunches); `kuvo-keys` (identifier `run.kuvo.code`, nested in it) reads 0.7.2's items with no prompt; TCC grants kept; launchd with the keeper's `SMAppService` agent after its bundle is replaced, and `launchctl bootout` of it; Sparkle through `objc2` in a bundle built without Xcode; engine 2.2.6 migrating 0.7.2's and 0.6's databases. | Mac | a written result each | low | 5–8 |
| 1 | **The helper on its own**: sources and `kuvo-keys` to `mac/helper/`, built and notarized standalone; `helper start` opens it itself with Login Items off; the helper writes permission facts (`kuvo/box/permissions`). | Mac, box | the helper's tests moved as they are; `helper start` with stand-ins | medium | 6–10 |
| 2 | **#715: the box's release built by the box** (§6), byte-for-byte with the last Swift-made one; `release.yml` without the Swift app (uses piece 1's helper). | box, scripts | a release built both ways, diffed; `update --from` takes it; Linux | medium | 10–14 |
| 3 | **`kuvo-install` and `install --status/--new`** (§4.1): every verdict, the engine from the release, `rehome --new` without the app's folder. | box | each verdict from fixtures; `--new` in a demo root with a test release; a cut-short install made again; Linux: `--new` as a machine's start | medium | 8–12 |
| 4 | **`rehome --run`, the upgrades and the database in staging, `--restore`** (§4.2, §5): the keeper's steps ported; an inventory of every preference key and `S` file the Swift app reads as data, each marked "box upgrade" or "dropped, said"; the engine database migrated in staging; the lock check on `S`'s database; `served.json`'s rule; pacing; `owed: passwords`. | box | fixtures from 0.7.2 (with its real 2.0.32 database), beta.1, a 0.6 build and `S/box`, moved by `--run` with a stand-in `kuvo-keys`; a stop at each step, then resume; a held database refuses; `--restore` gives back `S` byte for byte | **high** (data) | 20–30 |
| 5 | **The desktop app's part** (§3): running the installer and drawing its lines, `kuvo-keys` in the bundle, the System Settings panes, dev builds only attaching. | gpui | the install run against a stand-in installer script; a dev build refuses to install | medium | 6–10 |
| 6 | **Packaging the desktop app**: the `run.kuvo.code` bundle, signing, notarization, `box-release` inside, Sparkle embedded, DMG, appcast continuity. (Possibly another issue's; it's needed here.) | scripts, CI | `codesign --verify --strict` on a signed build; it carries the release; the feed's checks | medium | 12–18 |
| 7 | **UI** (drafts first, light and dark): first-run states (setting up, moving with its step, couldn't move with what to do, Try Again, moved back), Settings › This Mac's Box (#333 piece 8's content), permissions for the helper, `two`; the phone draws `moving`. | gpui, phone | the stand-in box's states; before-and-after looks | medium | 10–14 |
| 8 | **What only the Swift app did for this Mac** (§1's last item): the phone's Mac routes (`MAC_APP_ROUTES`), adding and repairing machines over SSH, `machines.json`. **Gates 0.8.** | box, gpui | sized after #712's survey | high | unsized |
| 9 | **Rehearsals and docs**: §5's cases there and back in the rehearsal account; `uninstall.md`, `privacy.md`, `settings.md`, `troubleshooting.md`, `footprint.md`; the changelog. | all | the checklists | medium | 8–12 |

- **Order:**
  - 0 first;
  - then 1 → 2 → 3 → 4 (box), with 5 and 6 alongside;
  - 7's drafts can start now;
  - 8 needs #712's list;
  - #693 pieces 5 and 11, and #635, come before 9;
  - 9 last.
- **Total:** about 95–130 h, plus piece 8.

**Risks:**

1. **Sparkle across an executable change.** Piece 0 checks it. If it fails, the stepping stone
   (§5) is the fallback.
2. **`kuvo-keys`'s identity:** a nested tool with the app's identifier. Piece 0 checks it; the Rust
   port is the fallback; asking once per item (about five prompts, said first) is the last resort.
3. **Migrating 0.7's engine database in staging.** It's opencode's own migration, run on a copy.
   A failure refuses the move with words, and `S` is untouched.
4. **The upgrades' inventory missing something** a 0.7 user kept only in the Swift app's
   preferences. Piece 4's inventory and the 0.7.2 fixture are the guard; what's dropped is said.
5. **Nothing runs while an `app-07` move is refused.** §4.2's pacing and words are the guard, and
   question 7 asks whether that's acceptable.
6. **The legacy keychain is "effectively deprecated"** (TN3137), as #333 noted.

## 8a. Piece 1 as built, and what it leaves piece 4 (2026-11-15)

Built: `mac/helper/` (Kuvo Helper and `kuvo-keys`), `scripts/build-helper.sh`, `helper start`
opening the helper with Login Items off, `kuvo/box/permissions`. PROTOCOL.md, "This Mac's box:
its helper", says each as it is. Three things piece 4 settles before anything runs `kuvo-keys`:

1. **No `kuvo-keys` signed with a real identity exists, on purpose.** `build-helper.sh` makes one
   only ad hoc (its tests) unless asked with `KUVO_SIGNED_KEYS=1`; `build-app.sh` never asks, and nothing else
   does. Signed as Kuvo Code by its team it reads 0.7's Keychain items with no prompt, and its
   guard (the move's lock, `updates/rehome/lock/pid`, naming the process that started it, a
   process older than the lock) holds only against what can't write in `~/.kuvo-code`: a command
   under the sandbox, another user. Any other program of yours can write that lock and run it:
   `passwords` (plain text), `delete-originals`, `copy`. That's wider than 0.7 is today. Piece 4
   picks the guard (a caller signed by the team; the desktop app running it itself on the box's
   word; or it written down, with the user's yes, that 0.7 already gives these to same-user code),
   and only then does a release pass `KUVO_SIGNED_KEYS=1`.
2. **`passwords` writes plain text** (`updates/rehome/passwords.json`, `0600`). Not to ship so:
   hand 0.7's sign-ins over as Keychain items trusting only the helper (as `copy` makes them), or
   down a pipe to the box's server. When the Keychain's own go is piece 4's too (going back to
   0.7.2 within 30 days needs them).
3. **`copy`'s helper check can be raced by a same-user program outside the sandbox.** The check
   is of a path (`bin` or `updates/install` in the home, yours alone, genuine before and after),
   and `SecTrustedApplicationCreateFromPath` reads the path again. Closed only under the sandbox.
   Piece 4: with the guard of 1 it may be enough; else trust by requirement, not by path.

**What only the retired Mac app sends (from #717 part 1, 2026-11-15).** Piece 4's inventory
starts from these; each is carried by the move, or said as dropped. Swift source → what it feeds:

| What an earlier version kept | Swift source (read only) | Box route or file |
| --- | --- | --- |
| This Mac's group chats (`groups.json`, `groups/<id>.json`) | `Stores/GroupMigration.swift:6`, run at `App/AppModel.swift:84` | the agents bridge's `groups/<id>/group.json`, `groups/pending-memory.json` (`box/src/agents/groups/store.ts:391`, `runner.ts:215`, `agents/main.ts:439`) |
| Personalities (`personalities.json`, `personalities-removed.json`) | `Stores/Personalities.swift:182`, `moveToBox` `:211`, `Keeper/MacBoxRunner.swift:436` | the box's library |
| Edits to machines' bots (`botEditsWaiting`, `botEditOutbox`) | `Stores/Bots.swift:390`, `handOverOldEdits` `:764` | `PATCH`/`DELETE kuvo/bots/<id>?on=<machine>` (held: `box/src/link/bots/held.ts`) |
| Machines' bots' keys in the Keychain | `Stores/BotKeys.swift:142` | gone from the Keychain (`kuvo/box/kept-keys` once the box moved) |
| Settings › Git (`git.branchPrefix`, `git.commitInstructions`, `git.pullRequestInstructions`) | `Stores/GitBranches.swift:179`, `App/AppModel.swift:722`, `AppModel+Machines.swift:615` | `POST kuvo/git/settings/move` (`box/src/link/areas/git-words.ts:114`, `changes.ts:1203`) |
| Letters (`agent-letters.json`, `agent-letters-seen.json`, `agent-messages.json`) | `Stores/AgentMail.swift:190`, `Stores/PhoneLink.swift:309`, `App/AppModel.swift:442` | `POST kuvo/link/letters/take` (`box/src/link/mac/routes.ts:221`) |
| Worktree records, kept and removed | `Stores/Workspace.swift:473`, `:1498` | `POST kuvo/worktrees/record` (`box/src/link/areas/worktrees.ts:289`) |
| `passwords.agentSignIn`, `passwords.agentSignInAccounts` | `Stores/Passwords.swift:165`, `:254-257` | the vault (`owed: passwords`) |
| Keep This Mac Awake, off | `Views/Settings/GeneralSettings.swift:12`, `Keeper/OwnBoxMove.swift:70` | the box's own setting (`box/src/link/work/awake.ts`) |
| Not taken in by anything now, the inventory's to say: `machine-groups.json` and `machine-groups/<id>.json` (the last copy of a gone machine's chats), `copilotClaudeModels`, `movesUnderWay` | `Stores/BoxGroups.swift:97-135`, `App/AppModel.swift:687`, `Stores/Moves.swift:171` | none |

(Line numbers as of main `ac5c18908`.)

**What #713 must send in the retired app's place, or machines go wrong** (kept tolerances whose
only sender today is that app):

- **`macID` and `tolerates: 1` in setup** (`swift/KuvoCode/Stores/PhoneLink.swift:679`). Without
  them every Mac with 0.8 that starts a machine's engine holds that machine's updates back: the
  box updates itself only while each Mac in `run/launchers.json` is `true` in `tolerantIDs`
  (`box/src/link/updates/update.ts:186`, `main.ts` setup).
- **`run/mac-installing` while it puts its files on a machine** (`swift/KuvoCode/Stores/Machines.swift:585`,
  `RemoteEngine.swift:604`). Without it the box takes those files for an older Mac's and counts
  drift (`update.ts`, `checkDrift`), holding updates back for 30 days.

Also for piece 4: `kuvo-keys` must be run directly by the process holding the move's lock
(`spawnSync` from it, no shell between, not from `serve` later), or it exits 2.

## 8b. Piece 2 (#715) as built, and the comparison with the last Swift-made release (2026-11-15)

Built: `scripts/box-release.sh` and `box/src/release/files.ts` write, pack and sign the box's
release from `box/` alone (`box/src/keeper`, `box/src/plugins`, `box/src/shared/engine-pin.json`,
`box/src/shared/integrations`, `box/src/shared/bot-words.json`); `release.yml` builds Kuvo Helper
on its own and packs with it, and stops a tag until piece 6 packages the desktop app.
PROTOCOL.md, "Updates", says what a release holds.

**What was compared.** No release artifact was downloaded. The Swift side was the newest local
build of the Swift Mac app (`KuvoBatch`'s compile check of main at `2fe5710a0`, a debug build,
bundle ID `run.kuvo.code.tests`), run as the release ran it: `"Kuvo Code" --write-box-files
<folder>`, with `env -i` and a home and temporary folder of its own. Its generators (`swift/KuvoCode`,
`swift/OpenCodeKit/…/Engine`, `bot-words.json`) hadn't changed between `2fe5710a0` and the commit
this work started from (`git diff`), and none of them is behind `#if DEBUG`.

**How.** (1) Its folder against `scripts/box-release.sh --files 0.7.2`, with `diff -r` and each
entry's mode, the box's side built with that commit's three built bundles in place of HEAD's.
(2) Both packed, with the same ad hoc Kuvo Helper and one throwaway key: main's `pack-box.sh`
over a stand-in app writing the Swift-made folder, against `box-release.sh`; the unpacked trees
with `diff -r`, `box.json` with `cmp`, the entries' kinds, modes and owners, and `boxes.json`'s
manifest field by field.

**Result.** All 16 files the same bytes and modes (the keeper's six, `.kuvo-boot`, the plugin,
the three catalogs, the bots' words, `box-version`, the link, the bridge with its closing
`// kuvo-agents <hash>` line, the browser), the helper bundle the same, the packed `box.json`
byte for byte the same, `boxes.json` the same but for `released` and the pack's hash and size.
Differences, each meant:

- the Swift release packed an empty `bots/` folder (its install script's `mkdir`); an update
  installs only what `box.json` names, so it never reached a box, and it's gone;
- `--files` writes `box.json` with Node's spacing (the Swift app wrote Foundation's; packing
  always rewrote it in Node's, so a pack's is unchanged);
- since then (the separate review): a pack is reproducible, its entries in name order with one
  fixed time and no owners' names, where the Swift-made one had the build's times and tar's own
  order. A box reads neither;
- on a real release, the helper carries its stapled notarization ticket (it was notarized inside
  the app before). Not run: it needs the real identity.

**Kept as a test.** `box/test/fixtures/release/swift-made.json` holds the twelve Swift-made files
that aren't built or named for the version, by SHA-256 and mode, and
`box/test/release-catalogs.test.ts` holds what the box writes for that engine to them; a file
changed on purpose leaves the fixture. `box/bridge-tests/update-real-keeper.test.js` installs a
pack with the real `kuvo-start` and `kuvo-launch` and a stand-in engine, on a Mac and on Linux.

## 8c. Piece 3 plan: `kuvo-install` and `install --status/--new` (2026-11-15)

**What's built here.** Everything is the box's; the desktop app's side (running it, drawing its
lines) is piece 5, and `install --move` is piece 4 (asked now, it says plainly it isn't there yet).

- **`box/src/install/kuvo-install`** (POSIX `sh`, macOS and Linux), written beside `boxes.json`
  and the pack by `scripts/box-release.sh`. `sh kuvo-install status|new|move [--app <bundle>]
  [--root <name>]`. It reads `boxes.json` (version, engine, this system's engine URL and npm
  integrity, the pack's SHA-256), checks the pack, unpacks it into `K/updates/install/release`
  (beside first, renamed whole), takes the engine (`K/updates/install/engine/<version>`: one
  fetched before, the box's own of that version, else downloaded, checked against the integrity
  and run once), then hands over to the release's `kuvo-link.js install`. JSON lines: `{"step"}`
  while it works, the answer last. Every `KUVO_*` it was started with is dropped but a test's.
- **`box/src/link/install/verdict.ts`**: the verdict, from the folders alone.
- **`box/src/link/install/command.ts`**: `install --status | --new | --move`.
- **`rehome --new`** (`rehome/move.ts`) takes the engine's folder as an option (the installer's,
  not the app's folder) and leaves `updates/install` where it is; the move's lock knows an
  `install` process as a holder.

**Verdicts** (`install --status` → `{verdict, verb, status}`; `verb` is what the app runs next):

| Verdict | From | `verb` |
| --- | --- | --- |
| `waits` | the move's lock held by a live process | none |
| `stuck` | a journal at `clear` or later, or one that can't be read | `move` |
| `moving` | a journal before `clear` | `move` |
| `ours` | `K/box/kind` says `mac` (not a making cut short) | `new` only while its helper's launch agent isn't in place, else none |
| `back` | the move's record says it was moved back | none (Move Again is the user's) |
| `two` | a machine's box in `K` (a card, a link, keys or `kind: machine`, not removed) and data in `S` | none |
| `machine` | a machine's box in `K`, nothing in `S` (on Linux, any box in `K`) | none |
| `app-box` | `S/box` ran (`OWN_BOX_RAN`) | `move` |
| `app-07` | anything of the old app's in `S` but its engine download, caches and the desktop app's own folder | `move` |
| `fresh` | none of these | `new` |

- **`--new` on a Mac:** only for `fresh` (and `ours` whose helper isn't in place: a making cut
  short after the box was made). The release's files checked against its `box.json`, then `rehome
  --new` with the installer's release and engine, then `helper start` by the installed link, then
  `updates/install` removed. Real `~/.kuvo-code` only for `run.kuvo.code`, and the reverse.
- **`--new` on Linux:** a machine-kind box in `~/.kuvo-code`, as a Mac adding it would leave it:
  the release's files and the engine in place, then the one start script the add flow runs
  (`add/scripts.ts`, `start`: settings its own, `launch.env`, the `@reboot` line, its id,
  `kuvo-start start`). No helper.
- **Cut short:** each stage is beside-then-renamed, so a second run makes it again.

**The relay and keys of a Mac that never ran the old app** (#636/#742): the box owns them. A Mac's
box in a home of its own with nothing kept in `run/mac-link.json` starts from its own: the default
relay, this Mac's name, "Let my devices connect" on (a question for the user: on or off for a new
Mac). Its keys come from its helper (made at first use, built). Adding a computer asks for the
keys whether or not anything else wants the link yet. Linux: a box that installs itself has no
relay until a Mac sets it up or `kuvo-link.js pair <relay>` (#183's work).

**Tests.** `box/test/install-verdict.test.ts` (each verdict from fixtures);
`box/bridge-tests/install.test.js` (`kuvo-install` against a test release, in a home of its own:
status, `new`, a cut-short one made again, a damaged pack, an engine that isn't what the release
says; on Linux `new` as a machine's start); `box/bridge-tests/own-link.test.js` (the link's own
settings and keys).

**Piece 3 as built (2026-11-15), and what it leaves.** As planned above, with these differences
after the reviewer's pass: `ours` says `new` while its helper isn't in place *or launchd doesn't
have it* (so a start launchd refused, or Login Items off, is tried at the next open: `helper
start`, the whole start); `status` runs on whichever engine the box has, never waiting on a
download; on Linux a removed or turned-off box's folder is set aside in the trash before a new one
is made (but `desktop`), under the move's lock; the box's own relay is for the real
`~/.kuvo-code` alone. PROTOCOL.md, "Installing this computer's box", says each as it is.

- **For piece 4:** `install --move` answers "isn't in this version yet"; `back` has no verb (Move
  Again is `kuvo/box/rehome {now}` then `--run`); `moving` and `stuck` name `move`. A 0.7 Mac has
  no `mac-link.json`, so after its move it starts from the box's own link settings ("Let my
  devices connect" on): if the inventory should carry the old app's `phoneLink` preference
  instead, that's one more "box upgrade" line. `app-07` is broad (anything in `S` but what's
  nobody's): `--run` must take a Mac with nothing real to move without failing.
- **For piece 5:** the app runs `sh kuvo-install status`, then the verb it names, and draws the
  lines; `--root` for a demo copy. On Linux there's no `--app`, so nothing keeps a copy of a Linux
  app from installing into the real `~/.kuvo-code`: the app's own guard (a dev build only
  attaches) is the only one there. Two installers at once aren't locked against each other in
  the shell's part (a second can remove the first's download: a plain failure, tried again).
- **Not built:** the box's own update reusing an engine the installer fetched (it fetches its
  own); a switch for "Let my devices connect" that needs no relay from the client (the desktop
  app's Settings, #644).

## 8d. Piece 4 plan: the move from 0.7, `rehome --run` (2026-11-15)

**What binds** (the user, 11-15), where it changes §4.2, §5 and #333 §3.7:

- Straight from 0.7 to the desktop app. Nothing runs until the move succeeds; every refusal says
  what to do, with Try Again.
- **No going back to 0.7.2**: no `rehome --restore`, no 30-day keep of `S`.
- **`S` is never changed.** The move clones `S` into staging and works there; nothing in `S` is
  renamed, written or opened for writing (its engine database isn't opened at all: reading a
  database that keeps a log beside it can rewrite the log's index). So there's no Clear of `S`, and
  the step named `clear` is only the point the move goes forward from.
- `two` (a machine's box in `K` and 0.7's data in `S`) **merges** `S` into the box in `K`, on a
  staged copy, keeping both where they clash; what can't be merged safely stops it before anything
  is swapped, saying exactly what.
- "Let my devices connect" after the move: on, unless that Mac's link already had it set.
- Words: never "box" or "Kuvo Core"; the computer's name, and "Kuvo on <computer>" for a noun. Each
  step's words come from the box (`progress: {step, title, index, of}`).

**Everything here is the box's** (`box/src/link/rehome`, `box/src/link/install`). The desktop app
runs `sh kuvo-install move` and draws its lines (piece 5); `kuvo-keys` is the helper package's
(`mac/helper`). The move itself is a Mac's alone: Linux has no app folder to move from, and
`install --move` there says there's nothing to move.

**`kuvo-link.js rehome <root> --run --app <bundle> --release <folder> --engine <folder>`**, which
`install --move` starts in a session of its own (the app quitting doesn't stop it) and follows,
printing `{"step", "title", "index", "of"}` lines, then its answer. Holding the move's lock, it:

1. finishes or undoes an earlier try (a journal before `clear` goes back, then this try starts
   afresh; one at `clear` or later is carried on);
2. judges what's here: `app-07`, `app-box`, or `two` (the merge);
3. stops what runs: for `app-box`, the old keeper (`POST stop` on its control, `run/keeper.json`;
   else its pid, then its lock waited for); for the merge, the box in `K` (its keeper's own
   `stop`), started again after, whatever happens;
4. refuses, in words, before anything changes: no room; another disk; a copy of the app into the
   real home; the release or its helper missing; the engine not fetched (offline); `S`'s engine
   database held open by a process (0.7 still open: `lsof`, never an open of the database); its
   schema one nobody tried (read from a clone of the database alone);
5. runs the steps, journaled as before, new ones in **bold**:
   `plan` → `install` (what's in `K` that isn't a box set aside) → **`keys`** (`kuvo-keys copy`
   trusting the release's helper, then `Kuvo Helper keys check`; undone by `keys uncopy` if it goes
   back) → `clone` (`S` into staging; the engine from the installer's, never 0.7's) →
   **`upgrade`** (the inventory's "box upgrade" rows, each reading `S`'s files or its preferences,
   exported once with `/usr/bin/defaults export <id> -`, and writing into staging) →
   **`database`** (the release's engine started once on the staged data, isolated, to migrate it;
   then the schema must be the pinned one and nothing it held gone) → `rewrite` → `check` (as
   before, and the counts below) → `place` → `fix` → `clear` (nothing of `S` cleared; checked first
   that nothing in `S` changed since the clone and nothing holds its database) → `done`;
6. settles what's owed, each tried again at the next run until done: `start` (the helper's launch
   agent, `helper start`), `keychain` (`kuvo-keys delete-originals`), `agent` (the old keeper's
   label booted out, `app-box` only), `passwords` (0.7's saved sign-ins into the vault once
   `serve` answers), then what the installer staged is removed.

**Counts held** (check step, from the staged copy before the upgrades and after everything):
sessions and projects (the engine's database, as before), bots (`bots/<id>`), wake-ups
(`wakes/<session>` and each bot's), worktrees (the engine's and the box's records), vault items,
added accounts (`agents/<agent>/accounts/<id>`). A count that's lower stops the move.

**`served.json`'s rule** goes for `--run`: the upgrades it waited for are run by the move, in
staging. The keeper's `rehome --release` keeps it (nothing runs that any more).

**Pacing.** `--run` is started by the user (the app opening, or Try Again): no wait between tries
and no cap; tries are still counted. `--ready`'s rules stay for the keeper's way.

**The inventory** (`.claude/notes/713-piece-4-inventory.md`): every preference key and every file
in `S` the Swift app read as data in 0.6.6, 0.7.2, 0.8.0-beta.1 and main, from the code at each
tag, each marked **box upgrade** (the move carries it: source shape → the box's file), **already
moved** (cloned as it is, and the box reads it there), **client's own** (the retired app's window
state: the desktop app keeps its own), or **dropped, said** (with what's lost, and said in
`state.json`'s `dropped` for the app to show once). Each upgrade is a function in
`box/src/link/rehome/upgrades.ts` with a fixture test; each can run twice.

**The merge (`two`)**, built after the plain move:

- The box in `K` is stopped; `K`'s data is cloned into staging beside `S`'s; both engine
  databases are brought to the pinned schema; `S`'s is rewritten for `K`'s paths, then its rows
  are added to `K`'s in one transaction, foreign keys checked.
- Rules: a session, message or bot is added (its ID is its own); **the same project** (one ID, or
  one folder) is one project with both sets of sessions, `K`'s row kept; a chat, sent file or
  bot's folder named as one there keeps both (`<name> 2`), with what names it following; a bot
  named as one there gets a suffix in its name; the box's lists (`projects.json`, worktrees)
  are joined by folder; an added account that's there already keeps the newer sign-in, the other
  kept aside (`<name>.from-earlier`), said; `K`'s settings, link, keys and vault stay `K`'s.
- **Stops before swapping**, saying what: a row with the same ID and different content that no
  rule covers; an engine database of `K`'s that can't be brought up to date; anything in `S` that
  only the Keychain holds and a machine's box keeps in files (its vault's values).
- The swap is journaled renames, `K`'s replaced folders kept in `updates/rehome/kept` for 30 days
  (they're `K`'s own, replaced by the merged ones: not a way back to 0.7). The box keeps its kind
  (`machine`), and the app attaches in place.

**Tests** (`box/test/rehome-run.test.ts`, `box/test/rehome-upgrades.test.ts`,
`box/bridge-tests/install.test.js`): fixtures of 0.7.2 (its real 2.0.32 database, made by
`fixtures/rehome/make-engine-fixture.ts` with the real engine and recorded; its migration by the
pinned engine recorded too, and made again by the real engine where `KUVO_TEST_ENGINE` names it),
0.8.0-beta.1, 0.6 and `S/box`, each moved by `--run` with a stand-in `kuvo-keys` and helper; a stop
at each step, then run again; a held database refuses; counts match; `S` byte for byte the same
after every run (a snapshot of every file's bytes, the database's log included); the merge from
fixtures with clashes; a merge it can't do stops before swapping.

## 8e. Piece 4 as built, and where I am (2026-11-15)

**Built** (PROTOCOL.md, "Moving this Mac's box" → The whole move, run by the box; "Installing this
computer's box" → `move`): `kuvo-link.js rehome <root> --run`, which `install --move` starts in a
session of its own and follows; the steps `keys`, `upgrade` and `database`; `progress` and
`dropped` in `kuvo/box/rehome`; what's owed settled at each run (`start`, `keychain`,
`passwords`, `agent`); `box.rehome.run`. Files: `box/src/link/rehome/run.ts`, `run-steps.ts`,
`migrate.ts`, `upgrades.ts`, `passwords.ts`.

**As planned in §8d, with these differences:**

- **`S` is read, never opened as a database.** Its schema is read from a clone of the database
  alone (`plan-database`, removed at once); who has it open to write in is asked of `lsof` (tried
  with the real 2.0.32 engine running: it shows as `opencode`, open for both; a reader, such as a
  backup, holds nothing back).
- **The engine's plugin** (`config/opencode/plugins/kuvo-code.js`), which 0.7.2 wrote for its own
  engine at each start, is replaced by the release's in the staged copy.
- **Saved sign-ins go down a pipe** (§8a.2 settled): `kuvo-keys passwords` writes no file any
  more; it says them on its standard output, which must be a pipe, to the move that runs it, and
  each goes from memory to `POST kuvo/vault/items` on the box's own server. `passwords.json` is
  gone from the design.
- **The preferences are read from the system** (`defaults export`) only for Kuvo Code itself;
  a copy's (a test's, a demo's) from its file with `plutil`.
- **No `--restore`**, no kept copy, and Move Back refused after such a move (the user, 11-15).
- **The engine**: 0.7.2's, 0.6.6's and beta.1's is all 2.0.32 (schema `50
  20261010180456_project_time_archived`). The pinned 2.2.6 migrates it as it starts, in about two
  seconds on the fixture, and acts on nothing it finds: a run cut short is marked suspended,
  queued messages stay queued (tried with both real engines; `fixtures/rehome/make-engine-fixture.ts`
  makes the recorded pair again).

**The inventory** (`713-piece-4-inventory.md`, from the code at `v0.6.6`, `v0.7.2`,
`v0.8.0-beta.1` and main): 127 preference keys and 35 entries in `S` for 0.7.2.

| | preference keys | entries in `S` |
| --- | --- | --- |
| box upgrade (carried by the move) | 36 | 7 |
| already moved (cloned as it is) | 0 | 13 |
| the retired app's own (window state) | 69 | 0 |
| dropped | 22 | 15 |

What it found that no list named: projects and machines were preferences in every release;
`toolApproval`; the marks of what the app added to the engine's settings; integrations' keys
named in the app's temporary folder (the box never renamed those for a box in a home of its
own); `opencode-files`; bots of 0.6 with no `bot.json`; the usage panel's settings.

**Not built: the merge (`two`).** `--run` refuses it plainly and changes nothing, and
`install --status` still says `two` with nothing to run. It's a piece of its own, and these are
what it has to settle before any code (none is a detail):

1. **The box in `K` is another Mac's machine**, running: stopping it (`kuvo-start stop` leaves its
   keeper and link up; a 0.7-era one has no `stop`), and starting it again whatever happens.
2. **Its version.** A machine set up by 0.7 has engine 2.0.32, no bridge and an old keeper. Its
   database must be migrated to merge into, after which its own engine can't read it: the merge
   has to install the release over it too (what the first Mac with 0.8 does over SSH).
3. **Two engine databases into one.** Both at the pinned schema, `S`'s rewritten for `K`'s paths,
   then `ATTACH` and `INSERT … SELECT` in one transaction with foreign keys checked. Rules needed
   per table: sessions, messages, parts, inbox by their own IDs; a project of one ID or one
   folder kept once (`K`'s row); `event` and `event_sequence` of a shared project; credentials
   and accounts (the newer kept, the other aside). Any other row with one ID and different
   content stops it.
4. **Keys.** A machine's box keeps its integrations' keys and its vault in files; 0.7's are in
   the Keychain, and `kuvo-keys` only copies them for the helper. Either `kuvo-keys` gains a
   verb that gives them to the move down a pipe (as `passwords` does), or the merge stops where
   `S` has any.
5. **Settings.** `K`'s engine settings stay `K`'s: `S`'s providers, models and integrations would
   not come along unless they're merged key by key (a clash is the user's to settle).
6. **Whose box it is after.** It stays a machine's (its Mac's setup, its `macs`): this Mac's
   projects and bots then show to that other Mac as the machine's. Is that what "must not be left
   with two" means, or should it become this Mac's own?

**Waits for the rehearsal account** (needs the real `run.kuvo.code` ID, a real login, launchd and
Keychain): `kuvo-keys` signed as Kuvo Code reading 0.7.2's items with no prompt, and its `copy`,
`delete-originals` and `passwords` for real; the helper's short runs (`keys check`, `uncopy`,
`clear`) from the release in `updates/install`; `helper start` after a move; `defaults export`
of the real preferences; `lsof` seeing a running 0.7.2 and its engine; a real 0.7.2, 0.6 and
beta.1 `S` moved whole (the fixtures here are laid out from the code, with a real database); the
`app-box` keeper stopped on its control and its label booted out.

**The reviewer's pass** (one `reviewer`, 2026-11-16). Found and fixed: an empty `bots`, `groups`
or `agents` folder in `S` failed every try (the upgrades took the cloned empty folder away, and
the check missed it); preferences that couldn't be read were taken for none (they stop the move
now); rows of 2.0.32's `session_pending`, which the pinned engine's migration drops, are said;
what the app kept for machines' projects and worktrees, usage API keys and starred models are
said; failures that aren't the person's to act on are one plain sentence (the detail in
`run.log`), and none blames Kuvo Code for an editor writing in a worktree; the old keeper is
named by `ps` before anything is sent or stopped; `two` is refused before anything is stopped
and noted nowhere, with its words from the installer too; the agents' grant is made on a retry;
each installer reads its own answer file. Left as it is, for the user: `kuvo-keys passwords`
gives 0.7's saved sign-ins to whatever can write the move's lock, for as long as they stay in
the Keychain (the same limit as §8a.1; taking them out of the Keychain once they're in the vault
would end it); and worktrees under `S` stay as second copies sharing one git record with those
in `K`.

**Where I am (2026-11-16, after the lead's separate review).** Both MUSTs and all but two of
the SHOULDs are fixed, with tests, on tip `38c1359ce` (main merged, built): the helper's and
launchd's words never reach the person; failures past `clear`, an unreadable journal and the
installer's outer catch say one plain sentence (detail in `run.log` or on standard error); the
engine's update is held to every table; the staged database's integrity and broken links are
checked before the swap; a `K` that's this Mac's own already is refused by `--run` itself; a
settle-only rerun needs no release; links by a whole path into `S` are repointed; more is said
(a `bots.json` that won't parse, 0.6's bots on machines, notifications, analytics, the last
model); the words named. Checks run on that tip: box 28 pass (also with the real 2.2.6), bridge
files pass (`own-box-start`'s "server for clients" test failed once, then passed three times:
not this piece's), static checks, Linux 0 fail. **Left of the SHOULDs:** usage API keys are now
said to no one (who pasted any can't be told without the Keychain: decide to say it to
everyone who used the panel, or not at all); `progress` is said to be null outside the
journal's life in PROTOCOL.md rather than made true.

**Left open by this piece** (decisions, in the report): the guard that keeps `kuvo-keys` to the
box's move against a program of yours outside the sandbox (§8a.1) and the race in `copy`'s
helper check (§8a.3), so no signed `kuvo-keys` yet; whether 0.7's saved sign-ins leave the Keychain once
they're in the vault; what `approveAll` becomes; the steps' and refusals' wording; `S` left on
disk for good, with its worktrees; the usage panel's API keys in the Keychain.

## 8f. Piece 5 plan: the desktop app's part (2026-11-15)

**What's built here**, and where each piece of logic lives.

- **`gpui/kuvo_box/src/install.rs`** (no GPUI; any Rust client's): finds what the app carries
  from its own executable (`<bundle>/Contents/MacOS/kuvo` → `Contents/Resources/box-release/
  kuvo-install`, `Contents/MacOS/kuvo-keys`), runs `sh kuvo-install <verb> --app <bundle>
  [--root <name>]` with every `KUVO_*` dropped, reads its JSON lines as they come (`{"step"}`,
  then the answer), and follows the box's word: `status`, then the `verb` it names, nothing
  else. It judges nothing about the computer.
- **The app's own guard, before the installer's** (the backstop stays `kuvo-install`'s): a build
  that isn't run from a bundle carrying a release (`cargo run`) only attaches. A bundle that
  isn't `run.kuvo.code` installs only into a root of its own (`KUVO_DEMO_MACHINE_ROOT`, a name),
  and `run.kuvo.code` only into `.kuvo-code`. Anything else: refused in the app, the installer
  never started.
- **`gpui/kuvo/src/launch.rs`**: the window's root. It shows the quiet "Starting…" while
  `status` runs, the Welcome card's row while a new Mac is set up (drafts 1 A), and `Home` once
  a box answers.

  | `status` says | The app |
  | --- | --- |
  | `fresh` (`new`) | runs `new`; the Welcome row says "Setting up Kuvo on this Mac…" with a spinner, then waits for the box's server and opens the first run on the same card |
  | `ours`, no verb | attaches, offers the release it carries (`kuvo/box/update {from}`, where the box has `box.update`) |
  | `ours` (`new`: its helper isn't in place) | runs `new` (the box's `helper start`), then attaches |
  | `app-07`, `app-box`, `moving`, `stuck` (`move`) | runs `move` and draws its answer. Until piece 4 that's the box's refusal, in its words, with Try Again; a box that still answers (`app-box`'s) is attached to all the same |
  | `waits` | the box's words where it gave any, Try Again |
  | `back`, `machine`, `two` | attaches where a box of this Mac answers; else says it isn't running. Their own screens are piece 7's (`two`'s card) and 8's (a machine's box attached in place) |
  | refused or failed | the box's `message`, as it is, under the row, with Try Again (Return) |
  | no JSON, exit 2 | "Kuvo couldn't be set up on this Mac." and the installer's last line of stderr: an app's mistake, said plainly |

- **Permissions.** The box says them (`kuvo/box/permissions`); the app draws a heads-up (drafts
  3 C's card) over the conversation for the first one the box says to bring up, opens the pane
  the box names, and, for one macOS has you add by hand, shows the drag tile. The tile is a GPUI
  window (`WindowKind::PopUp`: a non-activating panel over System Settings) whose icon is a GPUI
  drag with `external_drag_payload` (GPUI at our pinned revision hands a drag that leaves its
  window to AppKit as a file drag: `gpui_macos/src/window.rs`, `start_external_drag`). No
  hand-written `NSDraggingSource`. A small `#[cfg(target_os = "macos")]` module finds System
  Settings' window (`CGWindowListCopyWindowInfo`) and moves the tile beside it. The tile goes
  once the box says the permission is allowed.
- **Box change (small, the box's facts and words):** `kuvo/box/permissions` gains `bundle` (the
  helper's bundle on that Mac, what's dragged), and `list`: each permission with its `name`,
  `allowed`, `settings` (the address of its System Settings pane), `byHand`, and `asks` (whether
  a client brings it up now: not allowed, and not put off). `POST {later: "<id>"}` puts one off
  ("Not Now"), kept in the box so every client agrees and it isn't asked at each open.
- **`kuvo-keys`**: the bundle's layout names its place (`Contents/MacOS/kuvo-keys`), and the box
  finds it through `--app <bundle>`; the app's window never runs it. Tested with a stand-in
  bundle. Putting a real one there is piece 6's (and none signed exists until piece 4's guard).

**Not here, said plainly:** B's card for a move (piece 7, with the box's step words); `two`'s
card; Settings › This Mac; a machine's box attached in place; packaging (piece 6).

**Tests.** `kuvo_box/tests/install.rs`: a stand-in `kuvo-install` (a shell script the test
writes, answering from a file): `fresh` → `new` → done, each refusal and failure, `waits`
(exit 4), output that isn't JSON, exit 2, `KUVO_*` dropped, a dev build and a mismatched bundle
refusing before anything runs. `kuvo/tests/launch.rs`: the window through each state, Try Again
by click and Return. `kuvo/tests/permissions.rs`: the heads-up, Not Now, the tile opening and
going once allowed. Box: `own-box-helper.test.js`.

**Piece 5 as built (2026-11-16).** As planned above, with these differences:

- **A move's steps are drawn** now that piece 4 sends them: the box's `title` on this Mac's row
  and `index` of `of` as a thin bar under it. A move still at work from an earlier open (`waits`
  with `status.progress`) is drawn where it is and asked about again every two seconds, with
  nothing run beside it. A move that stopped says "Nothing runs on this Mac until this is done."
- **`ours` is waited for** (20 s) rather than attached to once: at login the app and Kuvo Helper
  start together.
- **Steps said while `status` is still asked** (a computer with no engine has it fetched before
  anything is judged) turn the quiet line into "Setting up Kuvo…", not the Welcome card: an app
  update unpacks its release at `status` too, and a returning user must never see a welcome.
- **A copy of the app keeps to its own names** (`install::names_for`): its bundle ID and the root
  it was given, or nothing. Only Kuvo Code itself and a build in no bundle use the real ones.
- **What stopped here isn't passed over for another computer**: with no Kuvo answering on this
  Mac, a failed setup is said even where the kept list has other computers to open.
- **The tile gives up** when System Settings' window has been gone two seconds, and after ten
  minutes whatever happens.

**BOX-NEEDED** (marked in the code where there's a place for it):

1. Words for the installer's own steps (`checking`, `engine`), and whether a `status` that
   fetches is a first setup, so a new Mac's download can be drawn in the Welcome card.
2. Words for a verdict with nothing to run (`two`, `machine`, `back`, `waits` without progress):
   `status` gives none, so the app says only that Kuvo isn't running.
3. The row's lines ("Setting up Kuvo", "Moving your sessions", "Couldn't set up Kuvo", "Couldn't
   move your sessions", "Nothing runs on this Mac until this is done.") and the heads-up's
   sentences are the app's; a phone drawing the same would need them from the box.
4. When an agent needed a permission (the box can only say it isn't allowed), so the heads-up
   comes at that moment; a way back from Not Now (Settings › This Mac, piece 7); Full Disk
   Access, Files and Folders, Local Network and Login Items as facts.
5. The installer's own messages still say "box" in places (`command.ts`, `kuvo-install`): they
   are drawn as they are.

**The reviewer's pass** (one `reviewer`, 2026-11-16). Blocker, fixed with a test: a copy of the
app refused by the guard fell back to the real names when it attached (the real password,
Keychain item and folder). Fixed too: the new Mac's download said nothing; a failed setup opened
another computer's sessions without a word; the heads-up let clicks through to what's under it;
a Kuvo a second from up was said not to run; an orphaned tile; an object printed after the
answer took its place; a permission the box no longer lists counted as allowed; only a System
Settings address is ever opened; a test asserted words its own stand-in made up. Left: no cap
on how long the installer may run (the script's own timeouts hold); Not Now has no way back yet.

**The lead's separate review** (2026-11-16): no MUST. Fixed, with tests: a copy's root is
compared with the real home by place (the same folder, its name in any letters' case, never a
link), not by spelling; a bundle's ID is read from a binary plist too; waiting on something else
at work is capped (30 minutes, then said with Try Again) and says nothing of a move where there's
no move's step; a Not Now that can't be kept answers `500 {message}`; the helper's bundle is
checked to be there before the drag is offered. `box.permissions.list` names the new fields
(the desktop app shows nothing where a box hasn't them, so no gate list needs it); PROTOCOL.md
says any client may send `{later}`.

**Not seen in a running window.** Rendered offscreen and driven in GPUI's test window. For the
rehearsal account: the real installer from a signed bundle; the tile as a panel over the real
System Settings, the drag into its list (the bundle's URL is handed without a trailing slash),
the tile following its window; macOS's own alert beside the heads-up; VoiceOver and larger text.

## 8f. Piece 7 as built (2026-11-16): the screens the user picked

Drafts: branch `kuvo/713-piece-7-drafts-the-desktop-app-s`, `.claude/notes/drafts/713-p7/`. Shots:
`gpui/shots/screens-launch-*` and `screens-settings-this-mac-*`.

- **A move (B):** `launch.rs` draws a card of its own ahead of the Welcome (no steps' dots): the
  box's steps as a list (`kuvo/box/rehome`'s new `steps`, handed on by the installer's `status`),
  where it is (`progress.index`), where it stopped (`failed.step`, else the step last heard of)
  and why in the box's words above the list, the note and Try Again. `stuck` reads "Couldn't
  finish moving". Done: "This Mac is ready", with what wasn't brought along (`dropped`) and, where
  that Mac's Kuvo says there's such a program, that macOS may ask again for it; then the app.
- **A new Mac (A):** the Welcome's row, "Setting up · <the installer's own words>" (`kuvo-install`'s
  steps have `title` now).
- **Welcomed after a move:** the move's `upgrade` step writes `run/welcomed.json`.
- **Settings › This Mac (A), permissions (C + A's group):** the box's section `computer`
  (`settings-pages.ts`), drawn by kind with no code of the app's own for it. A permission's row
  opens System Settings; the app's `HeadsUp::opened` matches that address to
  `kuvo/box/permissions`' list and puts the tile beside it.
- **`two`:** `status` says it in words (`message`); the app says them in a card with Continue,
  then opens with what's here.

**BOX-NEEDED / left:** the titles of the cards and their fixed lines are the app's (a phone
drawing the same needs them from the box); `two`'s card comes up at each open (nothing keeps
that it was read); what a move dropped is said only by the open that watched it finish;
"Restart…" is `kuvo-start restart` (the engine and its agents, not the link's own process), and
on a helper-run Mac it wasn't tried on a real one; Login Items is read with `launchctl
print-disabled`, not tried against a real switched-off helper from this page; Uninstall and the
folder aren't on the page; the phone doesn't draw described settings yet, so it has no This Mac
page.

## 8g. Piece 8 survey (2026-11-16): what only the Swift app answered for this Mac

Read: `box/src/link/mac/relay.ts` (`MAC_APP_ROUTES`, `macFront`, `toApp`), `mac/machines.ts`,
`mac/app-route.ts`, `mac/keeper-runs.ts`, `swift/KuvoCode/Stores/PhoneLink.swift` (`respond`,
`answer`), `PhoneLink+Machines.swift`, `PhoneLink+Mac.swift`, the phone's `MacShelf.swift` and
`LinkStore.swift` (what it asks and decodes), #636's note. No #712 note is in the repo; this is
from the code.

**The one seam.** A phone's request on a Mac goes through `macFront`. The five areas of
`MAC_APP_ROUTES` go to the app's host (`link/route`). A Mac with only the desktop app has a box
in a home of its own (`OWN_HOME_MAC`) and no app host (`openAppHost()` is null: the desktop app
registers none, and must not, being a client). So today each of these answers `503 "Kuvo Code
isn't open on this Mac."` for good, but the two lists the box already reads. The box's own
clients over its server (the desktop app) never pass `macFront` at all.

**The rule built:** in a home of its own, with no app host registered, the box answers all five
areas itself, for a phone and for its own clients alike. While an app's host is registered (the
retired app with `movesOwnBox`, never shipped), a phone's asks go to it as before; in the app's
own folder (`S/box`, the retired app) nothing changes.

| Route | Swift app did | Now |
| --- | --- | --- |
| `GET kuvo/machines`, `/events` | the list, with each SSH connection's state | **In the box already** (from `machines.json`), but worded for a closed app ("Kuvo Code isn't open on …", always `notConnected`). **Moved:** in a home of its own the box says its own facts: `connected` / `notConnected` by whether it hears from that computer, a try under way as `installing` with the try's title, `hosts` from the box's `~/.ssh/config` reader |
| `POST kuvo/machines {destination}` | checked over SSH, installed, added | **Moved:** one here already is said at once (`known`), nothing run. Else the box's own add (#636, `add/flow.ts`), waited for: `200 {machine, known}`, `400` for an address, `502 {message}` when it failed. Where the add must ask something (a host key it doesn't know, a computer of another setup) it's stopped and answered `409` in words: add it from Kuvo Code on the Mac. (The Swift app never asked either: an unknown key failed with "connect once in Terminal".) |
| `DELETE kuvo/machines/<id>` | removed from this Mac, runs on | **Moved:** the box's own removal (`DELETE kuvo/computers/<key>`, #735), by the machine's id |
| `POST kuvo/machines/<id>/uninstall` | uninstalled, queued 30 days if out of reach | **Moved:** the box's own uninstall, waited for: `200 {}`, `502 {message}` when it couldn't. Not queued (#636's choice) |
| `DELETE kuvo/machines/<id>/addresses/<address>` | forgot one address | **Moved** (the box writes `machines.json`) |
| `POST kuvo/machines/<id>/connect`, `/disconnect`, `/start` | the SSH connection's button | **Dropped** (#636: no connection is held; a computer runs by itself). Never offered (`action` is absent); asked anyway, `409` in words |
| `POST kuvo/machines/<id>/home` | Make This Mac Its Home | **Dropped, for the user to confirm:** a computer has no home Mac now, your setup's main computer replaces it. `409` in words |
| `POST kuvo/machines/<id>/same-as` | Same Computer As, by hand | **Dropped, for the user to confirm** (#636 left it out: folded by its own id when added). `409` in words |
| `GET kuvo/hosts` | machines' addresses for phones | **In the box already** (`phone-link.json`, which the box writes since #636) |
| `DELETE kuvo/hosts/<id>` | forgot a removed machine, told peers | **Moved:** taken out of `phone-link.json`; a peer still named is let go as a removal does |
| `GET kuvo/asks`, `/events`, `POST kuvo/asks/<id>` | the panel browser's asks, and its machines' | **In the box already** for its own browser (`areas/asks.ts`). **Moved (routing only):** a Mac's box in a home of its own answers them as a machine's does. What it lists is its own browser's, which a Mac's box doesn't run until #693 (pieces 4 and 5): empty until then, where it was a 503. Each box lists only its own (the phone already follows each computer) |
| `GET`/`PATCH kuvo/mac` | background work (its `SMAppService` keeper), Keep This Mac Awake (the app's preference), `awakeNow` | **Moved:** `keepsAwake`/`awakeNow` are the box's own already (`kuvo/work/settings`). `backgroundWork` is said from Kuvo Helper's Login Items switch: `on`, `needsApproval` when it's switched off there (the phone then says to allow it on the Mac), `off` where no helper keeps this box (a test's copy). Turning it on from a phone can't be done (only you switch Login Items): answered as the app did when macOS waited for you |
| `POST kuvo/engine/restart` | restarted the engine while the app ran the box | **In the box already** while a Swift keeper ran it (`boxAnswers`). **Moved:** in a home of its own the box restarts it itself (`kuvo-start restart`, as Settings › This Mac does) |
| `POST kuvo/engine/start` | 404 | 404 still (`engine.start` is a machine's): the router answered it on a Mac once nothing stood in front, so it's refused there now |

**Also asked: `machines.json` and "repairing machines".**

- `machines.json` writers: the retired app (in `S/box`); the box in a home of its own:
  `add/flow.ts` (add, another address), `peers/manage.ts` `forgetSaved` (remove), `rehome/upgrades.ts`
  (the move from 0.7 merges the app's list), and now `mac/own-machines.ts` (an address forgotten).
  Readers: `mac/machines.ts`, `add/flow.ts`, `mac/routes.ts`, `places.ts`. `phone-link.json`:
  written by `add/flow.ts` `keepHost` and now `DELETE kuvo/hosts/<id>`; read by `mac/machines.ts`.
  Nothing in the desktop app reads or writes either.
- "Repairing": the Swift app repaired on every connect (newer files put there, the phone link set
  up again, `setupFailed`). In the box: boxes update themselves from signed releases, and Set Up
  Again (`POST kuvo/computers/<key>/setup`, #636) runs the whole flow at its address. Nothing left
  to move; the phone has no button for Set Up Again (its Machines page is the old one).
- **What other computers ask of a Mac** goes by mail (`peerMail`), taken by the box itself with no
  app (documents, agents, letters, group chats, computers' rename and removal). Nothing there was
  handed to the app.

**Left for other pieces:** the phone's Machines page still draws the Swift app's model (Connect,
Same Computer As, Make This Mac Its Home); it should move to `kuvo/computers` and
`kuvo/computers/add` (a question's card, the steps), which is phone UI and needs drafts.
`MAC_APP_ROUTES` and `toApp` stay only for the retired app's two layouts and go with it.

**As built, after the review (2026-11-16).** `box/src/link/mac/own-machines.ts`, `mac/own-mac.ts`,
`answersAlone` in `mac/relay.ts`; tests `box/bridge-tests/own-home-mac-routes.test.js` (a phone
over the relay, no app) and two in `computers-add.test.js` (the real add, remove, uninstall).
The reviewer's pass led to: a first-time add listed as `installing`; an address already here
answered at once with no SSH; `kuvo/engine/start` a 404 on a Mac; Forget going through a
removal's refusals when your computers still name the machine; the old app attached refusing
the three writes; Login Items asked once a request, its last answer standing when launchd is
slow; one `peerReach` for `kuvo/computers` and `kuvo/machines`. Not done from it: `COMPAT(…)`
marks (that list is for released Macs, 0.7 and beta.1, held to PROTOCOL's section by a test;
this code is for the phone's old page, and says in its header when it goes).

**Open:** `POST kuvo/machines` answers at the add's end, not once the check has passed as the
Mac app did (the row shows `installing` meanwhile); the phone still offers Same Computer As
and, for a machine an old list marks as another Mac's, Make This Mac Its Home, which answer 409;
`helperAtLogin` is a `launchctl` run in the box's own process (5 s at most) on each `kuvo/mac`;
`launchctl print-disabled` and `kuvo-start restart` under Kuvo Helper weren't tried on a real Mac.

## 8h. Piece 6a as built (2026-11-16): the bundle, built locally

`scripts/build-desktop-app.sh` makes `build/Kuvo.app`, signed ad hoc, with the release, Kuvo
Helper and `kuvo-keys` where piece 5 looks for them; `scripts/check-desktop-app.sh` checks it and
dry-runs a copy. What's inside, how to build, and what piece 6's rest (the release's step) adds:
`.claude/notes/713-p6a-bundle.md`.

## 8i. Piece 4b design (2026-11-16): putting `two` together

Design only, against main at `c15dd6c4d`, revised after one `reviewer` pass (end of this section).
Nothing here is built.

**Today nothing is lost while this waits.** `--run` refuses `two` before anything is stopped
(`rehome/run.ts:262`), `install --status` names no verb for it (`install/verdict.ts:93`), and the
app shows its card and opens what's there (§8f, piece 7). `K` runs with its data, and `S` stays on
disk as it was.

### What `two` is, read from the code (corrects the brief)

**No released Kuvo Code could set a Mac up as a machine.**

- 0.6.0 through 0.8.0-beta.1 refuse anything but Linux with glibc:
  `guard parts.first == "Linux"` (`v0.7.2:Packages/OpenCodeKit/Sources/OpenCodeKit/Engine/RemoteEngine.swift:228`;
  the same at `v0.6.6:202` and `v0.8.0-beta.1:238`).
- A Mac became a possible machine on 2026-11-07 (`c0b7b1bcc`, unreleased Swift on main, in no tag),
  and in the box's own add (#636, `add/flow.ts:74-83`, a launch agent at `add/scripts.ts:145`).

So **a 0.7-era `K` (engine 2.0.32, no bridge, 0.7's keeper) can't exist.** `K` is always a
box-era machine. It has:

- `bin/box-version` and `kuvo-start` (`box` `:117`, `engine` `:275`, `stop` `:312`, `off` `:330`);
- its link with its keys in `run/link.json` (`state.ts:171-175`);
- its settings in `box/opencode.json` (`kuvo-launch:146`);
- its integrations' keys as files in `box/keys` (`home.ts:137`), plus the older `keys/` that
  `kuvo-launch:136-142` still writes;
- a vault of encrypted files, locked after each restart until a client unlocks it
  (`vault/store.ts:1-6,180`);
- its coding agents' default sign-ins and history in `agents/<kind>/home`, where a Mac's own box
  uses your `~/.claude`, `~/.codex`, `~/.grok` (`agents/machine.ts:27`, `agents/accounts.ts:74-81`);
- a keeper that holds the Mac awake whenever it's on power (`kuvo-start:125-140`);
- its own signed updates, which keep the old database for a rollback (`updates/update.ts:280-289`).

**It can arise two ways:**

1. On your own Macs, from unreleased builds.
2. After 0.8 ships, from a box's add. The add judges a Mac with only an old app's folder `free`
   (`setup/judge.ts:214-225`: no box is found there, and `appsOf` changes only `joins`). So it adds
   that Mac **with no question** (`add/flow.ts:209`) and seeds nothing (`:261`). That Mac's 0.7
   app keeps running its own engine in `S` beside the new box until it updates. That makes two
   Kuvos on one computer, which the add itself creates.

**Hence the user's call 1:** stop the add from making `two` at all. The merge is then for Macs that
ran unreleased builds.

### Two runs, not one

1. **The merge** (`rehome --run` on `two`) puts `S` into `K`. `K` stays a machine. Every part of
   it goes back until `K` starts again.
2. **The adopt** (`rehome --adopt`, owed by the merge, and usable for `machine` too) makes `K`
   this Mac's own. It has its own journal and its own conditions: `K`'s vault unlocked or empty,
   its agents' homes turned into accounts. It runs once those hold.

**Why split:** the reviewer found that making `K` Mac-kind breaks its vault (it reads as tampered,
not locked) and its agents' default sign-ins (questions 4 and 6 below). Neither can be fixed at
merge time without an unlock key that only another device has. Split this way, the data merge
never waits on a device, and the adopt can't lose what the merge brought.

### The six questions of §8e, answered

1. **Stopping `K`.**
   - **Answer:** first bring `K` to the release this Mac carries, and let that update be confirmed
     (question 2). Then pause it whole with a new keeper verb, `kuvo-start pause`: link, server,
     keeper, then engine and bridge, as `off` does, but without `run/off`. It leaves
     `run/link.needed` and `serve.needed` alone, so a go-back starts the link again.
   - **What keeps it paused is a mark of its own**, `run/paused`, naming the run's pid: written at
     `pause`, removed at `start`. From the swap on there's also `updates/rehome/forward`. Every
     start path honours both: `kuvo-start`'s
     `box`, `engine`, `start`, `restart` and `serve`, `kuvo-launch`, and the link's `box adopt`,
     `setup`, `box seed`, `leave` and `uninstall`.
     - That covers another Mac's Set Up Again and Remove too: their scripts call `K`'s own
       programs, which are this release's after the update (`add/scripts.ts:158-232`).
     - It doesn't cover the shell steps of that script: Remove's `DELETE FROM credential` and its
       `rm -rf`, at `:224-227`. Hence question 3's fingerprint.
   - **Before the swap a `run/paused` whose process is gone is let go,** and `K` starts. A later
     try starts afresh from `plan`.
   - **After a crash mid-swap,** `forward` with no live holder makes `kuvo-start box` (at login,
     or from the launch agent) run `kuvo-link.js rehome --resume` itself, which undoes the swap.
     So `K` isn't left down until someone opens the app here.
   - **`K` starts again whatever happens:** in the run's `finally`, and as an owed `start`.
   - **Rejected:**
     - `stop`: it leaves the keeper and the link up, writing in `run` while we clone.
     - `off`: it reads as removed (`runsAt` says `gone`, `judge.ts:259-263`). Then
       `machineBoxThere` is false, the verdict becomes `app-07`, and piece 4's `install` step
       sets `K` aside in its trash (`move.ts:477-502`).
     - Using the move's lock as the pause (`rehome/state.ts:315-336`). The lock is held for the
       whole run (`move.ts:718-724`), so honouring it would block the starts that run under it:
       the merge's own `update`, piece 4's `helper start` in `settle` (`run.ts:151,202-216`),
       Linux `install --new`'s start (`install/command.ts:320-371`), and the adopt's start.
     - Reading `K` while it runs: its database's log and its files change under the clone.
2. **`K`'s version.**
   - **Answer:** the merge runs only when `K` is at exactly this release (the same `box-version`
     and engine pin, so the same schema, `database.ts:19`).
     - **Older:** `K` updates itself, offered this release (`update.ts:775` `offerUpdate`, then
       `update`), while it runs.
     - **Confirmed first:** no `run/update-trial` and no `updates/rollback` left. Otherwise the
       keeper's `restore.sh` could later put the pre-update `opencode.db` back over the merged one
       (`update.ts:280-289`, `kuvo-start:245-258`). This is checked again in `check`.
     - **Newer:** refused, asking to update Kuvo Code here first.
     - **Refused in words** when its update is held back (`updatesBlocked`, `update.ts:182-186`),
       resting after a failed try (`offerRests`, `:763-797`), or its engine stopped on purpose
       (`:379`).
   - **Rejected:**
     - Migrating `K`'s database in staging while its engine stays old: its own engine then can't
       read it.
     - Merging into a newer `K`: this release's merge doesn't know that schema.
3. **Two engine databases into one, and `K` unchanged meanwhile.**
   - **Order:**
     1. `S`'s database is migrated (piece 4's `database`).
     2. The renames are planned (below).
     3. `S`'s staged copy is rewritten for `K`'s paths, renames included. So projects outside git,
        whose IDs come from their folders (`database.ts:146-150`), match `K`'s by ID or folder
        afterwards.
     4. `K`'s clone is checkpointed.
     5. On it: `ATTACH` `S`'s, `BEGIN IMMEDIATE`, one rule per table, `foreign_key_check` (nothing
        newly broken), `COMMIT`, `integrity_check`.
   - **Rules, for every one of the pinned schema's 24 tables** (`fixtures/rehome/engine-2.2.6.sql`):
     - **Rows with IDs of their own** (`session_v2`, `session_message`, `session_part`,
       `session_inbox`, `instruction_entry`, `instruction_state`, `blob_ref`, `permission`,
       `workspace`, `schedule`, `job`, `event`): added. The same key with the same content is one
       row. The same key with other content stops the merge (call 7).
     - **Content-addressed** (`blob`, `instruction_blob`): added where missing, and checked to be
       equal where both have them.
     - **`project`:**
       - **The same ID** (a repository's root commit, or one folder outside git): one project.
         `K`'s row stays; `S`'s values fill only what's empty.
       - **The same folder under two IDs:** `K`'s ID, with `S`'s rows pointed at it, as
         `rewriteDatabase` repoints IDs (`database.ts:146-159`).
       - **`project_directory` and `worktree`:** joined by their key, `K`'s row kept.
     - **`event_sequence`:**
       - **A session's:** added.
       - **A shared project's:** `K`'s row kept. The recorded 2.2.6 database has such a row
         (`'@ROOT@', 0`) and no `event` rows at all. `S`'s events on a shared project stop the
         merge until spike A says what they are.
     - **`credential`:** both rows kept. The newer by `time_updated` stays `active`; the older is
       set inactive. Said.
     - **`account`:** both rows kept. `account_state` points at the newer of the two for the same
       email and URL. Said.
     - **`control_account`** (key: email, URL): the newer kept. The older goes into the Keychain
       through the helper (`keys keep-aside`), never into a file. Said.
     - **`account_state`, `kv`, `migration`:** `K`'s. A `kv` key of `S`'s that differs stops the
       merge until spike A rules on it.
   - **The check is exact:** for each table, `S`'s rows are added + taken as the same + kept
     aside + stopped. A row that doesn't add up stops the merge.
   - **`K` unchanged since the clone.** At `clone`, `K`'s live state is fingerprinted: the
     database and its log (size, modification time), `run/off`, `bin/kuvo-start`, `box/keys`,
     `keys`, `run/link.json`, `machine-id`, `integrations`, and the agents' credential files that
     Remove deletes (`add/scripts.ts:229-231`). It's compared again under the lock
     just before `swap`. Any difference: the merge goes back, saying the computer was changed from
     elsewhere meanwhile.
   - **Rejected:**
     - Bringing `S`'s sessions in through the engine's API: there's none that keeps IDs, times
       and parts.
     - Two databases side by side: the engine opens only `opencode.db` (`migrate.ts:37-39`).
     - `INSERT OR REPLACE`: it overwrites.
4. **Keys.**
   - **In the merge, `K` is still a machine.** Its key files stay as they are.
     - `S`'s Keychain items are copied for the helper as piece 4 does (`kuvo-keys copy`), but
       never this Mac's link identity, since `K`'s is the one your setup knows
       (`copy --except link`). Undone by `keys uncopy`.
     - Nothing secret is written into a file.
     - 0.7's saved sign-ins: `owed: passwords`, into `K`'s vault, which needs it unlocked (as a
       machine's always does).
   - **In the adopt:**
     - `K`'s key files (`box/keys`, `keys`, the link's keys in `run/link.json`) are taken into
       the Keychain by a new helper verb, `keys adopt`, journaled and undone by `keys uncopy`.
     - The files are moved into `updates/rehome/kept`, not deleted, until the adopt is `done`.
     - The same key in both: the newer (the item's modification date against the file's) is used.
       The other is kept as an item named `<name>.from-earlier`. Said.
   - **The vault converts before the adopt pauses `K`, inside `K`'s running link.** Once
     `box/kind` is `mac`, a machine-format vault reads as tampered
     (`store.ts:66,104-106,180,217`). A machine's store key lives only in its running link's
     memory (`store.ts:1-6,36-38`), so no other process can read the values, and a paused `K` is
     locked again. So:
     - The adopt asks `K`'s link whether its vault is empty or unlocked (`kuvo/vault`'s state).
     - If unlocked, the link itself, as its own child process with the values on a pipe (as
       `kuvo-keys passwords` does), hands each value to Kuvo Helper from `K/bin`. The helper
       keeps them as Keychain items and re-signs the vault's files with the Mac's file key, which
       it holds.
     - The link writes the converted files beside its own (`box/vault.mac/`) and registers their
       generations. It goes on using its machine store until the pause.
     - The adopt then pauses `K` and swaps `box/vault.mac` in before the kind flips.
     - Only an unlocking device lets this run. The desktop app has no unlock yet (#635), so today
       that's the iPhone (call 3). Spike A proves it end to end.
   - **Rejected:**
     - A `kuvo-keys` verb piping Keychain secrets into files: it widens §8a.1, and leaves secrets
       in files on a Mac.
     - Refusing whenever `S` has a key: that's nearly every 0.7 Mac with an integration.
     - Converting at the first unlock after the flip: the flipped box no longer reads the
       machine's format.
5. **Settings.**
   - **Answer:**
     - `K`'s engine settings stay `K`'s.
     - `S`'s are added key by key where `K` has none: providers, integrations' servers, agents,
       instructions.
     - On a clash `K`'s value stays. `S`'s goes in `updates/rehome/merge/aside/settings.json`
       (`0600`, as `K`'s own settings file is), named in what's said.
     - A single value (the default model) is `K`'s.
     - The usage panel and `welcomed` come from `S`: they were this Mac's own, and a machine has
       none.
     - Settings › Git: `S`'s only where `K` has none (`moveGitSettings` as built).
     - Keep This Mac Awake applies only once adopted (call 6).
   - **Rejected:**
     - `S` winning: it changes how the other Mac's sessions there run.
     - Asking clash by clash: the merge runs detached, with nobody to answer.
     - Dropping `S`'s settings: this Mac's providers would be lost.
6. **Whose it is after: this Mac's own, still in the other Mac's setup** (call 4).
   - **Answer:** the adopt makes its kind `mac`, started by Kuvo Helper, keys in the Keychain.
     - It keeps its link identity, so it stays a member of the setup it follows: the other Mac
       reaches it as the same computer, now a Mac with Kuvo of its own rather than a machine it
       keeps (`setup.role` stays `member`, checked by a test).
     - Its settings move from `box/opencode.json` to `config/opencode`.
     - The machine's launch agent and crontab line go (`owed: machine-start`).
   - **Agents' default sign-ins.** Each `agents/<kind>/home` becomes an added account
     (`agents/<kind>/accounts/<id>`, labelled "<name>'s"), with its sessions' provider pointed at
     `<provider>@<id>`. Your own `~/.claude` (`~/.codex`, `~/.grok`) is the default from then on.
     That's what `S`'s 0.7 sessions used, so both keep working (spike A checks it).
   - **Rejected:**
     - **Staying a machine.** Its keeper holds a Mac someone uses awake whenever it's on power.
       macOS asks about each program by its own name. The desktop app treats it as someone else's
       machine (`place.rs` takes only `mac`). The other Mac's Set Up Again can rewrite it.
     - **Leaving the other Mac's setup:** that Mac loses the computer, and joining setups is #183's
       phase 7, not built.

### The merge's steps (`rehome --run` on `two`: a way of its own, `way: "merge"`)

The merge never runs piece 4's `install` step, which sets `K` aside (`move.ts:477-502`).

**Order.** `plan` → `update` → `pause` → `keys` → `clone` → `upgrade` → `database` → `rename` →
`rewrite` → `merge` → `check` → `swap` → `start` → `done`. Each is journaled with stop points, as
piece 4's are.

1. **`plan`** (changes nothing):
   - checks: one APFS disk; room for both databases ×1.1; the release and its engine; `S`'s
     database not held (`lsof`); `S`'s schema one the move takes; `K`'s version and its update
     confirmed (question 2); `K` not mid-update (`run/.updating`);
   - if agents are at work on `K`, it waits, said (call 5).
   - For `app-box`, the old keeper is stopped first (`stopOldKeeper`, as built).
2. **`update`:** `K` brought to this release by its own update, then confirmed. Safe if killed:
   the update's own rollback, before anything of the merge's exists.
3. **`pause`:**
   - Agents asked once more, just before.
   - Then `kuvo-start pause`, under the move's lock.
   - Checked: nothing runs from `K/run` (`runningIn`).
   - A session the other Mac started in the seconds between is cut short. The engine marks it
     suspended at its next start, as it does after any stop.
4. **`keys`:** question 4's merge part.
5. **`clone`:**
   - `S` into staging, as piece 4 does.
   - `K` (everything but `bin`, `engine`, `updates`, `trash`, `tmp`) into `staging/k`, with
     `cp -cRPp`, which keeps times.
   - Counts of both noted, and `K`'s fingerprint.
   - `K`'s live folders aren't touched until `swap`.
6. **`upgrade`, `database`:** piece 4's, on `S`'s staged copy alone.
7. **`rename`:** plans each clash's new name: chats, group chats, bots, personalities, folders. It
   adds those to the path map.
8. **`rewrite`:** piece 4's, on `S`'s staged copy, with that map.
9. **`merge`:** the database (question 3), then the folders, entry by entry, into `staging/k`.
   The rule for each of `K`'s top-level entries (from `home.ts`, `kuvo-launch` and the add's
   scripts):

   | Entry | Rule |
   | --- | --- |
   | `bots/<id>` (`newID("bot_")`, `bots/make.ts:163`), `wakes/<session>`, `attachments/<session>` | added by ID; the same ID with other bytes stops it |
   | a bot's, a group chat's or a personality's name | the same name gets ` 2` (what names it follows) |
   | `chats/<name>`, `groups/<id>`, `workspace/<name>` | the same name gets ` 2`, through step 7's map |
   | `agents/<agent>/accounts/<id>` and `agents/accounts.json` | added by ID. The same account (agent and sign-in): newer kept, the other `<id>.from-earlier` (a folder, like any account). Said |
   | `agents/<kind>/home` | `K`'s (`S` has none: 0.7 used your own home). Turned into an account by the adopt |
   | `agents/usage/settings.json` | `S`'s where `K` has none, else `K`'s |
   | `integrations/<name>` (servers' sign-ins) | the same: newer by its files' times as cloned, the other `.from-earlier` |
   | `box/projects.json`, `box/worktrees.json`, `bots.json`, letters, personalities | joined by folder or ID, `K`'s entry kept where both |
   | `data/opencode/snapshot/<project>/<hash>` | git objects joined: each pack or loose object `K` lacks is copied in, and `S`'s refs go under `refs/kuvo-earlier/`, so nothing is unreachable. Spike A checks what the engine prunes |
   | `data/opencode/worktree/<name>`, `shell`, `storage`, `tool-output` | added by name; the same name with other bytes stops it |
   | `config/opencode`, `box/opencode.json` | question 5 |
   | `run/link.json`, `box/keys`, `keys`, `box/vault`, `box/kind`, the card, `machine-id`, `state`, `browser`, `desktop` | `K`'s |
   | `cache`, `tmp`, `S`'s `search` | dropped (rebuilt as sessions change) |
   | `machines.json`, `phone-link.json` from `S`'s machines | not taken: a member doesn't add machines (`setup/main-only.ts`). Said, naming each (call 10) |
   | anything else in both, with other bytes | stops it, naming the entry (call 7) |

10. **`check`:**
    - the database's exact accounting and integrity, nothing newly broken;
    - each count (sessions, messages, projects, bots, wake-ups, worktrees, vault items, accounts,
      group chats) is `K`'s + `S`'s − what was taken as the same, never fewer;
    - `S` unchanged since `clone`;
    - `K`'s fingerprint unchanged;
    - `K`'s update still confirmed;
    - nothing runs from `K/run`.
11. **`swap`:**
    - `updates/rehome/forward` written first.
    - For each top-level entry of `K` that changed: `K/<entry>` is renamed to
      `updates/rehome/kept/<date>/<entry>`, then `staging/k/<entry>` to `K/<entry>`. Each rename
      is journaled.
    - **This goes back.** Nothing writes in `K` meanwhile (question 1), so until `start` a failure
      renames `K`'s own back and removes `forward`.
12. **`start`:** this is the point the merge only goes forward from.
    - `forward` and `run/paused` removed, then `kuvo-start` (`box`): `K` runs, still a machine.
    - `K`'s record says `way: "merge"`, `state: "done"`, with `S`'s fingerprint.
    - Owed: `passwords`, then `adopt`.

**After a done merge, `K` is never judged `two` again.**

- `S` stays on disk and `K` is still a machine, so `machineBoxThere` and the app's data are both
  still true (`install/verdict.ts:88-95`).
- So the verdict reads the record first: a done merge with an unchanged `S` fingerprint is
  `merged`, never `two`. Its verb is `move` only while the adopt is owed and its conditions hold;
  else there's no verb.
- An `S` changed since (an old 0.7 opened again) is said, never merged a second time.
- **Until the adopt runs**, the desktop app treats `K` as this Mac's: `place.rs` takes a
  machine-kind `K` whose record says `way: "merge"`, `state: "done"`, as this Mac's own, attached
  in place (`Speaker` for a machine's box attached in place, `local.rs:28`). That's a small
  `kuvo_box` change in piece E.

**Safe if killed.**

- Before `swap`, `K`'s live folders are as they were. With the lock's holder gone, `K` starts
  again by itself.
- In `swap`, `forward` keeps `K` stopped until the next `install --move` undoes it. The app runs
  `install --move` at its next open, since `status` names `move` for a journal under way.
- `S` is never written.

### The adopt's steps (`rehome --adopt`, owed; a journal of its own)

Run by `install --move` while it's owed, and runnable for `machine` (call 4).

**Conditions, said while they don't hold:**

- `K`'s vault is empty or unlocked;
- no agents at work;
- Kuvo Helper in `K/bin`.

**Steps:**

1. **`pause`:** as the merge's.
2. **`keys`:**
   - `keys adopt` (link, integrations), undone by `keys uncopy`;
   - the vault's values handed to the helper, its files re-signed, its generations registered;
   - the key files moved into `kept`.
3. **`agents`:** each `agents/<kind>/home` becomes an added account, its sessions repointed
   (question 6).
4. **`settings`:** `box/opencode.json` into `config/opencode`.
5. **`kind`:** `box/kind` written `mac`, last. Each step until here goes back.
6. **`start`:** forward only. `helper start`, then owed: `machine-start` (the machine's launch
   agent and crontab line booted out and removed) and `keychain`.

### What the other Mac sees and does

- **Before:** nothing changes. The update in the merge's step 2 is `K`'s own, as any update is.
- **During** (minutes):
  - `K` reads "Can't be reached" in its Computers (`reach: away`, PROTOCOL.md "Computer display
    words").
  - Its sessions on this Mac can't be opened, and what it sends is kept for later.
  - Set Up Again and Remove started then do nothing to `K`'s data: `K`'s own programs refuse
    while it's paused, and a change from their shell steps sends the merge back (question 3).
- **After the merge:** reached again, and what was kept goes at once (`peers/mail.ts:137`).
  `K` is still its machine, with this Mac's sessions in it.
- **After the adopt:**
  - the same computer and key, its card saying `kind: mac`, `app: true`;
  - its list shows this Mac's projects under this Mac's name, as any member's;
  - Set Up Again and Uninstall over SSH are refused for it (piece G, call 8);
  - its `machines.json` record stays, as an address.
- **Nothing to do there,** but unlocking `K`'s vault from a device that can, if it has items.

**What a phone sees.**

- Paired through the setup: the same computer throughout. It's unreachable during, and from
  `two`'s card on it says "Moving".
- A phone paired only with this Mac's earlier identity (beta.1 or keeper builds; no 0.7 release
  had the link on, §5) loses it. It pairs again, said in the done screen (call 9).

### Words (drafts, the user's: call 11)

**Steps** (`progress.title`):

1. "Checking this Mac"
2. "Updating Kuvo on <name>"
3. "Pausing Kuvo on <name>"
4. "Taking this Mac's keys along"
5. "Copying your sessions"
6. "Bringing your earlier settings along"
7. "Bringing your sessions up to date"
8. "Naming what's there twice"
9. "Pointing your sessions at their new place"
10. "Putting your sessions together"
11. "Checking nothing's missing"
12. "Putting everything in place"
13. "Starting Kuvo on <name> again"

The adopt: "Making Kuvo on <name> this Mac's own".

**Done:** "This Mac is ready. Your sessions from the earlier Kuvo Code are on Kuvo on <name> now."
Then the `dropped` and kept-aside lines:

- "Two sign-ins to Claude (you@example.com) were on this Mac. The newer one is in use; the other
  is kept aside."
- "Your Notes chat was here twice, so the earlier one is now Notes 2."
- "Kuvo on <name> becomes this Mac's own once its vault is unlocked: open Kuvo Code on your
  iPhone."
- "Phones paired with this Mac before need pairing again."

**Refusals.** Each one says that nothing was changed and that Kuvo on <name> is running again, and
offers Try Again:

- "Kuvo on <name> is newer than this Kuvo Code. Update Kuvo Code, then try again."
- "Kuvo on <name> can't update yet: <Mac> still has an older Kuvo Code. Update it there, then try
  again."
- "Kuvo on <name> couldn't update to this version earlier, and tries again tomorrow."
- "Kuvo on <name> was stopped from <Mac>. Start it there, then try again."
- "Kuvo on <name> is working in 2 sessions. Your sessions are put together once they're done."
- "Kuvo on <name> was changed from another computer while your sessions were being put together.
  Try again."
- "Your earlier sessions and Kuvo on <name> both have <the chat “Notes”, a session “Fix login”,
  the file data/…> with different contents, so nothing was put together. Send
  ~/.kuvo-code/updates/rehome/run.log to Kuvo's support."
- Piece 4's refusals, as they are (room, disk, `S` held open, schema, engine, release).

**The `two` card** (piece 7) becomes the move's card. Its steps come from the box, so no new
screen is needed (call 11: confirm that, or have drafts made).

### Pieces, in order (sized from history: a piece with review ≈ 1–2 h wall)

| # | Piece | Where | Tests | h |
| --- | --- | --- | --- | --- |
| P | **Prevention** (call 1): the add refuses (or asks about) a Mac whose earlier Kuvo Code kept sessions (the probe looks for its engine database, not just the folder, which demo copies have too) | box `add/scripts.ts`, `add/flow.ts`, `setup/judge.ts` | `computers-add.test.js`: an app's folder with a database, and one without | 1 |
| A | **Spike**: what 2.2.6 keeps in `kv`, `event` on a project, `credential.active`, `account_state`; whether snapshot git prunes; how often the catch-all would stop on real layouts; the vault's signatures and generations through the helper; agents' homes turned into accounts with sessions repointed. Two real databases merged by hand in a scratch folder, then the real engine started on the result and its sessions opened. Written result | box test folder, helper | a written result | 1.5–2 |
| B | **Fixtures, update and pause**: a K-world made by the real add scripts in a test home (at this release, an older one, one not yet confirmed) with a 0.7.2 `S` (`rehome-run-world.ts`) and every clash above; `kuvo-start pause` and the lock honoured by every start path; `K` updated and confirmed first | keeper, box | bridge: paused, a killed holder releases, `forward` holds, each start path refuses, the other Mac's Set Up Again while paused; Linux for the keeper part | 2 |
| C | **The database merge** (`rehome/merge-database.ts`) | box | per rule and per refusal; exact accounting; the real 2.2.6 engine on the merged database (`KUVO_TEST_ENGINE`) | 2 |
| D | **The folders' merge** (`rehome/merge-files.ts`), the renames' map, snapshots' objects and refs | box | per entry rule; an unknown clash stops it | 2 |
| E | **The merge's run**: `way: "merge"` in `run.ts`, `two` given verb `move` and `merged` judged in `verdict.ts`, fingerprint, swap going back, `kuvo-start box` resuming after a crash mid-swap, counts, words; `place.rs` taking a merged `K` as this Mac's | box, `gpui/kuvo_box` | `rehome-run.test.ts`: a stop at each step, then run again; `S` byte for byte; `K` back byte for byte after every failure; a change in `K` meanwhile sends it back; a done merge judged `merged`, never run again | 2–2.5 |
| F1 | **The adopt's keys**: `kuvo-keys copy --except link`; the helper's `keys adopt`, `keep-aside`, `uncopy` for them | `mac/helper` (Swift) | helper tests with a test keychain | 1.5–2 |
| F2 | **The adopt's run**: the vault converted by `K`'s running link before the pause, agents' homes to accounts, settings, `kind`, `owed: machine-start`; for `machine` too | box | setup role kept; a vault unlocked, locked and empty; sessions of a default account still open after; launch agent and crontab stand-ins | 2–3 |
| G | **The other Mac's side**: Set Up Again and Uninstall refused for a Mac with Kuvo of its own; the phone's `kuvo/machines` lists it as a computer | box | `computers-add.test.js` | 1 |
| H | **Rehearsal and docs**: a real 0.7.2 `S` with `K` added by a real box from another Mac, merged and adopted; `troubleshooting.md`, `machines.md`, the changelog | all | the checklist | 1.5–2 |

- **Order:** P, then A. Then F1 (C's `keys keep-aside` is its verb) and B. Then C and D,
  alongside, then E. F2 after E, G with it. H last.
- **Total:** about 17–22 h, plus the rehearsal account's time. Review: high risk (data, another
  computer), so the lead's separate review comes after E and after F2.

### The user's calls (with what I recommend)

1. **Stop the add from making `two`.** Today a box adds a Mac that has an earlier Kuvo Code's
   sessions with no question.
   - *Recommend:* refuse it in words, "<name> has sessions from an earlier Kuvo Code. Update Kuvo
     Code on <name> first.", and make that a 0.8 gate.
   - It's a refusal, so it's yours.
2. **Then whether the merge gates 0.8.** With 1, only Macs that ran unreleased builds can be `two`,
   and until the merge they lose nothing (`K` runs, `S` kept).
   - *Recommend:* build it right after P, but don't hold 0.8 for it, unless you know of a Mac
     outside your own that's `two`.
3. **The adopt waits for `K`'s vault to be unlocked** when it has items. Today only the iPhone can
   unlock it; the desktop app can once #635 is built.
   - *Recommend:* accept, said in the done screen.
   - The alternative is a machine-format reader kept in Mac-kind boxes for good.
4. **Whose it is after** (question 6).
   - *Recommend:* this Mac's own, still a member of the other Mac's setup, and the same adopt for
     `machine` (a Mac set up as a machine, nothing in `S`).
   - The other way, staying a machine, saves F1 and F2 but keeps the Mac awake on power, asks
     permissions per program, and lets the other Mac rewrite it.
5. **Agents at work on `K` when the merge would pause it.**
   - *Recommend:* wait until they're done, as updates do, and say so.
   - The other way is a "Stop Them and Continue" button.
6. **Keep This Mac Awake after the adopt.** As a machine, `K` kept the Mac awake whenever on power.
   As this Mac's own, the box's setting applies: `S`'s if you had turned it off, else on while
   agents work. The other Mac's sessions there then stop when this Mac sleeps.
   - *Recommend:* `S`'s setting, said once.
7. **Clashes no rule covers stop the merge** (the same ID or file name with other contents),
   where your rule says "keep both".
   - *Recommend:* stop and say. Spike A counts how often that would happen; if it's more than
     rare, add a rule rather than keep both blindly (two rows with one ID can't both be kept).
8. **Refusing Set Up Again and Uninstall over SSH** from the other Mac for a computer that's now a
   Mac with Kuvo of its own: "<name> runs Kuvo itself now. Remove it from your computers instead."
   - *Recommend:* yes.
9. **Phones paired with this Mac's earlier identity** (beta.1 or keeper builds) pair again.
   - *Recommend:* accept, said.
   - The alternative is keeping two identities on one box, which nothing supports.
10. **`S`'s machines on a Mac that's a member.**
    - *Recommend:* say each by name, with "add it again from <main Mac>".
    - They aren't taken into `K`'s list, which a member can't keep.
11. **The words above**, and whether `two`'s card simply becomes the move's card (no new drafts).
12. **How long what's kept is kept.**
    - *Recommend:* `K`'s replaced folders and moved key files 30 days, then deleted.
    - Accounts kept aside stay until you remove them (they're accounts like any other).
13. **A settings clash.**
    - *Recommend:* `K`'s wins, and this Mac's is kept aside and named.
    - The other way is this Mac's winning.
14. **The computer's name after.**
    - *Recommend:* the name your setup knows it by (`K`'s), which you can rename.
    - The other way is this Mac's own name.

### The reviewer's pass (one `reviewer`, 2026-11-16)

**Blockers, all fixed in this version:**

1. **`K`'s own update could roll back over the merged database** (`update.ts:280-289`,
   `kuvo-start:245-258`). Now the update must be confirmed before `pause`, and again at `check`.
2. **"Nothing writes in `K` while held" wasn't true.** Every start path but `box` ignored the
   hold, and another Mac's Set Up Again and Remove run their own shell (`add/scripts.ts:158-232`).
   - Now every start path and the link's setup commands honour the move's lock.
   - A fingerprint of `K` taken at `clone` sends the merge back if anything changed.
   - A try before `swap` always starts afresh.
3. **A machine's vault reads as tampered once the kind is `mac`, not locked**
   (`store.ts:66,104-106,180,217`), and the desktop app can't unlock. Now the adopt is a run of
   its own: it converts the vault before the flip, and waits for an unlock (call 3).
4. **The adopt would have switched `K`'s agents' default sign-ins to your own home**
   (`agents/machine.ts:27`, `accounts.ts:74-81`), losing their history. Now each becomes an added
   account, its sessions repointed.

**Should-fix, fixed:**

- `rewrite` now runs before `merge`.
- The go-back window ends at `start`, and key files are moved, not deleted.
- Rules for `agents/accounts.json`, usage settings, agents' homes, `data/opencode/worktree`,
  `browser`, `workspace`, `desktop`, `tmp`, and the top-level `keys`.
- `pause` keeps `link.needed` and `serve.needed`.
- Sign-ins are kept as inactive rows or Keychain items, never a secrets file.
- Snapshots' refs are kept.
- What phones see.
- More user calls (3, 6, 7, 8, 9).
- Pieces F1 and F2 split, and the total re-sized.
- The missing refusals.
- Agents asked again at `pause`.

**Minor, fixed:**

- P looks for a database, not just the folder.
- The words now match the steps.
- `install --move`, not `--status`, finishes a merge.
- The merge has its own way, never piece 4's `install` step.
- The move's lock is used instead of a new mark.
- The fixture's project row is named.
- `bots/make.ts:163`.
- Files' times are kept by `cp -p`.

**The second pass** (the same `reviewer`, on the revision) found two blockers the revision
brought, both fixed:

1. **The move's lock as the pause blocked the starts that run under that lock.** Now `run/paused`
   is a mark of its own, held only from `pause` to `start`.
2. **A separate, paused process can't read a machine's vault.** Now `K`'s running link converts
   it while unlocked, before the pause.

Also fixed:

- a done merge was judged `two` again: now `merged`, and the desktop app takes `K` as this Mac's
  until the adopt;
- the fingerprint now covers the agents' credential files;
- a crash mid-swap is undone by `K`'s keeper itself;
- F1 comes before C;
- `search` has one rule;
- one line number.

**Left for spike A** (none checked by anyone):

- the engine's `kv`, project events and snapshot pruning;
- `credential.active`'s meaning;
- whether keeper builds' `K` take an offered update;
- the vault conversion through `K`'s link and the helper, end to end.

## 9. Questions only the user can answer

1. **Who keeps this Mac's box?**
   - *Recommend: your leaning, with the box doing the installing.* The helper (Kuvo Code Box) is
     the login item. The desktop app has the box install it once, from the release it carries,
     and the box updates it after that. The app never runs the box itself.
2. **The helper stays Swift, in its own small package (`mac/helper/`, with the one-time
   `kuvo-keys`), or is rewritten in Rust?**
   - *Recommend: stays Swift.* It's 2,000 lines that are tested and verified on real macOS. A
     rewrite costs 15–25 h and new risk for no user-visible change.
3. **From 0.7: straight to the desktop app (the box moves itself), or through a last Swift release
   that moves first?**
   - *Recommend: straight.* No more Swift UI to build and throw away. A stepping stone still needs
     the straight path for anyone who skips it.
4. **Is 0.8 blocked on everything the Swift app did for this Mac** (§8 piece 8: adding and
   repairing machines, the phone's Mac routes), or may 0.8 ship saying plainly what's not there
   yet?
   - *Recommend: blocked on adding machines and the phone's routes.* Without them, a 0.7 user
     loses features they use.
5. **Will 0.8.0 (Swift, keeper, flag off) ship before the desktop app?**
   - *Recommend: no.* The `app-box` case then exists only on your own Macs, as a support path.
6. **Name:** the desktop app's window and menu say "Kuvo". Sparkle installs it at the old path,
   `/Applications/Kuvo Code.app`. Keep that name, or rename the bundle (and the helper, "Kuvo Code
   Box")?
   - A rename touches the Dock and Spotlight, the helper's `AssociatedBundleIdentifiers` grouping
     in Login Items, the bundle path the box keeps (`run/app.json`), and the docs' screenshots.
   - It's brand, so it's yours; drafts come with piece 7.
7. **On a 0.7 Mac nothing runs until the move succeeds** (it never had a box). Each refusal says
   what to do, with Try Again. Is that acceptable, or should the box be able to run in the old
   folder meanwhile?
   - *Recommend: acceptable.* After piece 4 the refusals left are rare and fixable (space, a
     locked Keychain, being offline, 0.7 still open). Running in the old folder would need the
     helper to run outside `K`, which its security rule refuses (PROTOCOL.md:6691).
8. **Must going back to 0.7.2 be possible?**
   - *Recommend: yes, for 30 days, as §5 says:* the 0.7 folders put back as they were, with work
     done since kept aside, not carried back. It's cheap, since the kept copy is already there.

## 10. Review

One `reviewer` pass on the first version. What it found, and what changed:

- **Blocker: every 0.7 move would be refused for its database's schema.** 0.7.2 runs engine 2.0.32,
  and `rehome` knows only 2.2.6's schema (`database.ts:19`). Now the pinned engine migrates the
  staged copy, isolated, before Rewrite (§4.2); there's a 0.7.2 fixture with its real database,
  and piece 4 is resized.
- **Blocker: nothing runs the box while a move can't happen** (no keeper, and the helper runs only
  `K`). Now §4.2's pacing (user-started tries, no cap, plain words), refusals turned into steps,
  and question 7.
- **Blocker: going back wasn't true.** `--back` restores a keeper layout and a migrated database
  that 0.7 can't read; nothing settled `owed: agent`; a round trip stranded the Mac. Now
  `--restore` puts back the untouched kept copy, removes the helper's agent, and sets `K` aside;
  there's a `back` verdict with Move Again, and question 8.
- **The 0.7 upgrades were under-counted** (three of about thirteen, plus keeper builds' unsent
  queues). Now piece 4 starts with an inventory, each item kept or "dropped, said".
- **The Keychain copies trusted a helper not yet in `K/bin`.** Now they trust the release's
  helper, unpacked first (§4.2 step 3).
- **An unpacked release with a helper bundle under `Resources`** went against the bundle layout and
  needed a new seal check. Now the app carries the pack as data, as today; `kuvo-install`
  unpacks it. The old question 4 is gone.
- **The Rust Keychain port** was high-risk temporary code. Now `kuvo-keys` is a tiny Swift tool
  from the helper's package, with the app's identifier; the port is the fallback. It's removed by
  your word, not on a date.
- **Case C's launchd details were wrong** (`BundleProgram` names `Kuvo Code`; `KeepAlive`; a
  missing plist). `--box` and unregistering are out of the app; the box boots the label out, and C
  is a support path.
- **A running 0.7 engine wasn't detected.** Now there's a lock check on `S`'s database at Plan and
  before Clear.
- **Real data:** dev builds only attach; spikes needing the real ID run in the rehearsal account.
- **Piece 8 hid a ship gate** (machines over SSH, `machines.json`). It's named as gating 0.8, with
  question 4.
- **Smaller:**
  - upgrades run in staging, after Clone;
  - `owed: passwords`;
  - 0.5/0.6 fixtures;
  - §6's missing files and the catalog's source;
  - the missing verdicts and rows;
  - no preflight for Files & Folders or Local Network;
  - the rename's costs;
  - the order (helper before release);
  - the calibration's weight;
  - the link key claim (0.7 likely has none);
  - Linux attaching as a machine's box;
  - Open at Login inherited.
- **Not checked by anyone:**
  - Sparkle 0.7.2's behaviour;
  - opencode migrating a 2.0.32 database to 2.2.6;
  - whether users' 0.7 Macs hold `phone-link` items;
  - the sandbox's walls around `K/updates/install`.
