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

The one exception, until the box makes its own release (#713): the release still builds this app
(below), so it has to keep compiling against the shared packages. When a change to one of them
breaks it (`scripts/batch-build.sh --compile-only mac`, after each merge, says so), make the
smallest change here that compiles again, with nothing else in it, and say so in your report.

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
  (`KuvoCode/Keeper/OwnBoxMove.swift`), for a box's move that no Swift keeper runs.
- This app still reads that code (`BoxHelperBundle`, `KeychainMove`, `LoopbackServer`, …), so
  OpenCodeKit passes it on for the Mac (`OpenCodeKit/Sources/OpenCodeKit/HelperKit.swift`, and the
  dependency in its `Package.swift`): nothing in this app changed. Both go with the box release
  below. `scripts/build-app.sh` puts the helper `build-helper.sh` built into `Contents/Helpers`,
  as before.
- This app's own words still say "Kuvo Code Box" in a few places (its move is behind
  `movesOwnBox`, off in every build): left as they are.

## What it's still built for, until the box does it (#713)

Machines and the protocol still need three things that only this app's code makes today. Each
runs the app's code as it is, without editing it:

- **The box release machines update from**: `.github/workflows/release.yml` builds this app and
  runs `"Kuvo Code" --write-box-files` (`KuvoCode/Stores/BoxRelease.swift`, with OpenCodeKit's
  `BoxFiles`). `scripts/pack-box.sh` and `sign-box.mjs` then pack and sign the release. This is
  the only way machines get updates today, so release.yml and its scripts stay as they are.
- **`protocol/vectors/routes/box`**: `scripts/route-vectors.sh` writes these from the app's
  `RouteContractTests` (the integrations, the MCP catalog, the Mac's bot kit), through
  `scripts/test-app.sh`.
- **The Mac app's What's New**: `scripts/whats-new.py` writes `KuvoCode/Resources/whats-new.json`
  as release.yml builds the app.
