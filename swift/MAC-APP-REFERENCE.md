# The Swift Mac app: kept only to read

The Swift Mac app is retired. Kuvo Code 1.0 is the desktop app in `gpui/`. The Swift app gets no
more updates, CI doesn't build or test it, and it has no test batch.

Its code stays here **only to read**, for two things:

- **Logic that still belongs in the box** (#668): the reviews found hours of logic in this app
  that the box has to own. Read it here, then build it in `box/`.
- **What the desktop app has to match**: how each screen, state and flow behaved, as people
  knew it.

**Never edit it.** Don't fix, update or reformat it, and don't move logic into it. A finding goes
to the box or the desktop app.

The one exception was the box's release, which this app's build made until the box made its own
(#715, below). Nothing that ships builds this app now. While its compile check still runs
(`scripts/batch-build.sh --compile-only mac`, after each merge) and a change to a shared package
breaks it, make the smallest change here that compiles again, with nothing else in it, and say so
in your report.

## What's the retired app

- `KuvoCode/`: the app (views, stores, design, keeper, resources), and `KuvoCode.xcodeproj`
- `KuvoCodeTests/`: its tests
- `Tests/`: its end-to-end runs (`E2E`), measurements (`Perf`), and the scripts' guards against
  the real app (`Guard`)
- `UsageKit/`: the menu bar's usage tracker, a fork of OpenUsage that only this app uses
- `Config/`: its build settings, Info.plist and entitlements

## What's not, and is kept up

The phone (`../kuvo-code-ios`) builds against these packages, and the desktop app's tests read
some of them, so they're maintained like any other code (and this app's compile check, above,
says when a change to one breaks it):

- `KuvoClient/`, `KuvoLink/` (with the box's link as built, `kuvo-link.js`), `KuvoVoice/`,
  `OpenCodeKit/` and `Textual/`

The box's other two bundles, `kuvo-agents.js` and `kuvo-browser.js`, used to be built into
`KuvoCode/Resources`. They're the box's own, so they're built into `box/dist/` now, and the app's
project takes them from there.

**Kuvo Helper moved out (#713).** The program this Mac's box runs under (it was named Kuvo Code
Box) isn't this app's any more:

- Its code (`OpenCodeKit/Sources/OpenCodeKit/BoxHelper/`, `Sources/KuvoCodeBox/`), its tests
  (`BoxHelperTests.swift`) and its entitlements (`Config/KuvoCodeBox.entitlements`) are in
  `mac/helper/`, a package of its own, kept up like any other code. `scripts/build-helper.sh`
  builds and signs it there, with no app (it replaces `scripts/build-box-helper.sh`).
- `kuvo-keys`, built beside it, does the Keychain's hand-over this app's keeper did
  (`KuvoCode/Keeper/OwnBoxMove.swift`), for a box's move that no Swift keeper runs. Only ad hoc,
  for its tests: a build signed with a real identity makes none unless asked (`KUVO_SIGNED_KEYS=1`), and this
  app's build (`scripts/build-app.sh`) never asks. No signed `kuvo-keys` is to exist until #713
  piece 4 settles what keeps it to the box's move.
- This app still reads that code (`BoxHelperBundle`, `KeychainMove`, `LoopbackServer`, …), so
  OpenCodeKit passes it on for the Mac (`OpenCodeKit/Sources/OpenCodeKit/HelperKit.swift`, and the
  dependency in its `Package.swift`): nothing in this app changed. Both go with the box release
  below. `scripts/build-app.sh` puts the helper `build-helper.sh` built into `Contents/Helpers`,
  as before.
- This app's own words still say "Kuvo Code Box" in a few places (its move is behind
  `movesOwnBox`, off in every build): left as they are. So is `KuvoCodeTests/OwnBoxMoveTests.swift`,
  which expects that name in the launch agent the box writes and in a message, and would fail on
  them now; this app's tests aren't run.

## What it was still built for, and what's left

Three things only this app's code made. The box makes the first two itself now (#715):

- **The box's release**: the box builds it from its own sources, with no app
  (`scripts/box-release.sh`, `box/src/release/files.ts`). What this app wrote into it
  (`"Kuvo Code" --write-box-files`: `KuvoCode/Stores/BoxRelease.swift`, with OpenCodeKit's
  `BoxFiles`) has its one copy in `box/`: the keeper's shell files (`box/src/keeper`; here they're
  strings in `OpenCodeKit/…/Engine/RemoteEngine.swift`, `Wakes.swift`, `Launcher.swift` and
  `CommandEnvironment.swift`), the integrations and the popular MCP servers
  (`box/src/shared/integrations`; here `KuvoCode/Stores/Integrations.swift`, `MCPCatalog.swift`
  and `MachineBox.swift`), the engine's pin (`box/src/shared/engine-pin.json`; here
  `EngineInstaller.swift`) and the bots' words (`box/src/shared/bot-words.json`, which KuvoLink's
  `Resources/bot-words.json` is now a link to, so KuvoLink still builds, for this app and the phone). **The copies here go
  into no release**: a change to what a box runs is made in `box/`, never here, and these aren't
  kept in step. `.github/workflows/release.yml` doesn't build this app.
- **`protocol/vectors/routes/box`**: the integrations and the MCP catalog are written from the
  box's table (`scripts/route-vectors.sh`). Only the Mac's bot kit (`mac-bot-kit.json`) still
  comes from this app's `RouteContractTests` (`scripts/route-vectors.sh --mac-kit`, through
  `scripts/test-app.sh`).
- **The Mac app's What's New**: `scripts/whats-new.py` writes `KuvoCode/Resources/whats-new.json`.
  Nothing builds it into a release now.

`scripts/build-app.sh` still builds this app, for trying an update over an installed copy
(`scripts/try-update.sh`).
