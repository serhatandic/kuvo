# #713 piece 6a: the desktop app as a bundle, built locally (2026-11-16)

The part of piece 6 ("Packaging the desktop app", `713-desktop-app-runs-the-box.md` §8) that needs
no identity: a real `Kuvo.app`, signed ad hoc, for rehearsing the update from 0.7 in the rehearsal
account. Signing for release, notarization, Sparkle, the disk image and the feed are not here
("What the release's step adds", below).

## How to build

```sh
# Rust as gpui/README.md sets it up; Bun and Node on the PATH (or BUN=, NODE=); Xcode.
VERSION=0.8.0-beta.2 ZIP=1 scripts/heavy.sh mac-build scripts/build-desktop-app.sh
```

- Makes `build/Kuvo.app` (and `build/Kuvo-<version>.zip` with `ZIP=1`: carry the zip, not a
  Finder copy, to another account or Mac, so the signature arrives as it is). About 32 MB.
- `VERSION` defaults to `0.0.0`. For a rehearsal give one above what's installed: the bundle's
  `CFBundleVersion` is what Sparkle compares (`scripts/build-version.sh`: `0.8.0-beta.2` is
  `0.8.0b2`), and a box takes the release an app carries only when it's newer than itself.
- `APP_ID` defaults to `run.kuvo.code`. **Such a bundle is Kuvo Code itself: opened, it sets up
  or moves into the real `~/.kuvo-code`.** Open it only in the rehearsal account. For a copy to
  try anywhere, build a family of its own: `APP_ID=run.kuvo.code.trial1 OUT=<folder>`, opened with
  `KUVO_DEMO_MACHINE_ROOT=<a folder's name in your home>` (its helper is `run.kuvo.code.trial1.box`
  and its kuvo-keys is signed as the copy, so nothing of it meets the real one's in the Keychain
  or launchd; a copy's box is made but not started: the box starts one by itself only in
  `~/.kuvo-code`).
- Always ad hoc: `SIGN_IDENTITY` set to anything else is refused.
- `KUVO_BOX_SIGNING_KEY` (the box releases' key, PEM) signs `boxes.json` where it's given.
  Without it the release is signed with a key made for that build and kept nowhere. Installing a
  new Mac and the move from 0.7 work all the same (`kuvo-install` doesn't check the signature: the
  app's own seals the folder). What doesn't: a box that's there already refuses the release as an
  update (`kuvo/box/update {from}` takes only a pinned key's), so rehearsing "a newer app updates
  its box" needs a bundle built with the real key, which is the user's to give. The app shows
  nothing of that refusal (`422`, dropped in `launch.rs`), so in a rehearsal it's silent.
- The build stops where the box's built files aren't what their sources build
  (`scripts/check-built.sh`), since the release packs them as they are in git.
- A `run.kuvo.code` bundle left where Finder or Spotlight look can be opened by macOS for
  anything that asks for Kuvo Code by its ID (it's a second, newer Kuvo Code to Launch Services).
  Keep it out of the way: carry the zip to the rehearsal account and remove `build/Kuvo.app`.

## What's inside

| In the bundle | What | From |
| --- | --- | --- |
| `Contents/MacOS/kuvo` | the app | `cargo build --release -p kuvo`, stripped; built with `KUVO_APP_VERSION` so it names the bundle's version to the relay and your computers |
| `Contents/MacOS/kuvo-keys` | the one-time hand-over of 0.7's Keychain items; only the box's move runs it (`--app <bundle>`) | `scripts/build-helper.sh`, signed ad hoc with the app's identifier |
| `Contents/Resources/box-release/` | `boxes.json`, `box-<version>.tar.gz`, `kuvo-install` | `scripts/box-release.sh`, from this checkout's `box/` |
| …the pack's `bin/Kuvo Helper.app` | Kuvo Helper (`<ID>.box`), with the app's icon | `scripts/build-helper.sh`, ad hoc; only in the pack, never loose in the bundle, so macOS knows one Kuvo Helper: the installed one |
| `Contents/Resources/AppIcon.icns`, `Assets.car` | Kuvo Code's icon as it is (`swift/KuvoCode/AppIcon.icon`, compiled by `actool` as Xcode did) | |
| `Contents/Resources/LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`, `LICENSES-DEPS.md` | the terms, as the Mac app carried them, and the Rust dependencies' | |
| `Contents/Info.plist` | `gpui/kuvo/mac/Info.plist` plus the ID, version, build number, oldest macOS (26.0) and icon | |

The app's icons and fonts are compiled into `kuvo`; the bundle needs no other resources. The app
has no entitlements: what macOS asks about is asked of Kuvo Helper.

## Checking one

- `scripts/check-desktop-app.sh <Kuvo.app>` (the build ends with it): `plutil -lint`; `codesign
  --verify --deep --strict` on the bundle; kuvo-keys there and signed as the app; `boxes.json`'s
  signature is its key's and says whether boxes trust that key; its version is the app's; the pack
  is the one it names; every file in the pack is what the pack's `box.json` says (what
  `kuvo-link.js install` checks before making anything); Kuvo Helper in the pack still signed, as
  `<ID>.box`.
- `scripts/check-desktop-app.sh --run <Kuvo.app>`: that, then a dry run of a **copy** with the ID
  `<ID>.check`, in a temporary home folder: `kuvo --install-status` (new in this piece: the app
  runs its installer's `status`, prints the answer and quits, with no window and nothing run
  after it). Without a root of its own the copy must refuse; with one, a new home is `fresh`
  with `new` to run. It fetches the engine (the network; about a minute), and leaves nothing.
- In the rehearsal account, before opening the app: `build/Kuvo.app/Contents/MacOS/kuvo
  --install-status` says what that account holds (`app-07` with `move` for one with 0.7's data)
  and sets up, moves and starts nothing; it does stage the release and the engine in
  `~/.kuvo-code/updates/install` (replacing an older staged release, and tidying a staged engine
  the box there already has), as opening the app would first. So only in that account.

Run here (2026-11-16, this Mac, `VERSION=0.8.0-beta.2`): the build, the check and `--run` pass
(`verdict: fresh`, `verb: new`); `cargo test --workspace` passes. The window of the bundled app
was never opened here, with either ID.

## What the release's step adds (the user's, just before the release)

1. **Developer ID signing**, inside out, with `--options runtime --timestamp`: Kuvo Helper
   (`SIGN_IDENTITY` to `scripts/build-helper.sh`, as `release.yml` does today), `kuvo-keys`
   (`KUVO_SIGNED_KEYS=1`: none signed with a real identity exists until that's decided, see
   `build-helper.sh`; the move from 0.7 needs it to read 0.7's Keychain items without a prompt for
   each), then the app. `build-desktop-app.sh` refuses an identity on purpose: this is a new path
   in it or in `release.yml`, reviewed as a release change.
2. **Notarization**: the helper zipped, notarized and stapled *before* it's packed (as
   `release.yml` does); then the whole app (`ditto -c -k --keepParent`, `notarytool submit
   --wait`, `stapler staple`).
3. **The box releases' key** (`KUVO_BOX_SIGNING_KEY`) for `boxes.json`, and `BOX_URL` the
   published pack's. The same pack and `boxes.json` should be the ones published beside the app.
4. **Sparkle**: the framework in `Contents/Frameworks` (trimmed and signed as
   `scripts/build-app.sh` did), started from Rust (piece 0's spike: through `objc2`), and
   `SUFeedURL`, `SUPublicEDKey` (the same key 0.7 trusts), `SUEnableAutomaticChecks`,
   `SUVerifyUpdateBeforeExtraction` in Info.plist. Until then this app doesn't update itself.
5. **The disk image and the feed**: `scripts/build-dmg.sh`, `scripts/appcast.sh` and
   `merge-appcast.py` name the Swift app's bundle ("Kuvo Code.app", `KuvoCode.dmg`); the feed must
   go on from 0.7's (`sparkle:version` above `0.7.2`), and `release.yml`'s tag gate lifted.
6. **Decisions for the user** (nothing here picks one for good):
   - the bundle's name. Built as `Kuvo.app` with the display name "Kuvo" (the app's own menu and
     window say Kuvo); 0.7 is "Kuvo Code.app". Sparkle keeps the installed bundle's file name, so
     an updated Mac keeps `/Applications/Kuvo Code.app` showing as "Kuvo" in the Dock and menu
     bar unless the name is settled one way;
   - the oldest macOS (26.0 here, as 0.7 and Kuvo Helper; the Rust program itself would run on
     older);
   - Apple silicon only here (Kuvo Helper is built for arm64 alone).
7. `swift/KuvoCode/AppIcon.icon` is read from the retired app's folder: it moves (to `mac/` or
   `gpui/`) before `swift/KuvoCode` is deleted.

## For the rehearsal

- Ad hoc signing means: Gatekeeper refuses a quarantined copy (carry the zip over without
  quarantine, or clear it with `xattr -dr com.apple.quarantine`); macOS takes each ad hoc build
  for a new program, so its permissions (TCC) and Login Items approval for Kuvo Helper are asked
  again per build; and `kuvo-keys`, signed ad hoc, is nobody to the Keychain, so reading 0.7's
  items asks for the login password per item (or is refused) where a Developer ID build wouldn't.
  Piece 0's spikes on Sparkle, the Keychain and TCC need the real identity and stay the user's.
- Sparkle 0.7 → this bundle can't be rehearsed from this build alone: it has no Sparkle, and a
  feed entry needs the update key's signature. Replacing `/Applications/Kuvo Code.app` by hand
  with this bundle rehearses everything after the swap (the verdict, the move, the helper).
