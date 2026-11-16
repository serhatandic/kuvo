# A private update feed for the user's own copy (2026-11-16)

The user moves to the desktop app before 0.8, the first release with an update feed. Their copy
(Kuvo, `run.kuvo.code`, signed with their Apple Development identity, built on the Mac mini)
reads a **private feed on the Mini** over the tailnet. It updates itself through Sparkle like
anyone's. Nothing is published. The tool is `scripts/private-feed.sh`; the updater itself is #759's
(`.claude/notes/759-desktop-app-updates.md`).

## What was chosen

- **Reach: the tailnet, over https, with `tailscale serve`.** The MacBook isn't always on the
  Mini's LAN, and Sparkle and ATS want https. A plain-http tailnet URL would need an ATS
  exception in the app's Info.plist, so it isn't used.
  - Tailscale.app (the standalone variant here) can serve a *port*, not a folder; Tailscale's docs
    say "due to macOS app sandbox limitations".
  - So a login agent runs Python's file server on **127.0.0.1:8790** only, and `tailscale serve`
    puts it on **https://`<the Mini's MagicDNS name>`:8443**, with Tailscale's certificate.
  - Only the user's tailnet reaches it, not the LAN or the internet (that would be `funnel`).
    With `--bg` it lasts across restarts.
  - `setup` reads the name from `tailscale status` and keeps it in `feed-url`.
- **Key: a separate private-feed key, kept in the Mini's login keychain** ("Kuvo private update
  feed key", made by `setup`).
  - The real Sparkle key isn't on the Mini: there's no `https://sparkle-project.org` item (checked
    without reading it). It stays on the MacBook.
  - Copying it to the Mini would give a second machine the power to sign updates every 0.7 copy
    accepts. The cost of keeping it off is a manual step at 0.8 (below).
- **Folder: `~/Library/Application Support/KuvoPrivateFeed`.** It's outside the repo and outside
  the swept `~/.kuvo-code/tmp`, one folder that `uninstall` removes.
  - `feed/` is what's served: `appcast.xml`, plus the last 3 `Kuvo-<version>.dmg` and their
    `box-<version>.tar.gz` packs.
  - Each build's `BOX_URL` points into `feed/`, so the box release names a pack that exists.
- **Versions: betas only, `0.8.0-beta.N`**, from beta.3 (the rehearsal was beta.2).
  - `build` with no version takes the next beta after the feed's newest.
  - It refuses a version that isn't a beta: a box on a private `0.8.0` would never take the real
    0.8.0 as newer. It also refuses one that isn't newer.
  - The feed's items go in **no channel**, so a copy with "Get beta updates" off (perhaps carried
    over from 0.7) still gets them. They link to no release page.
- **Building and signing** go through `build-desktop-app.sh` with `PRIVATE_FEED=1`. It's the only
  way `run.kuvo.code` gets a feed other than the real one: https, never the real feed or key, and a
  real `SIGN_IDENTITY` (default `Apple Development`, as codesign finds it). That identity signs
  Kuvo Helper, kuvo-keys, the app and the disk image. A plain local build still names no feed.
- **The box releases' key** is read from the Mini's keychain inside the script, as for the
  rehearsal. `build-desktop-app.sh` now takes it out of its environment at once and hands it only
  to `box-release.sh`, never cargo or the helper's build.

## An exception, for the user to confirm: a signed kuvo-keys

As the Foreman asked (the rehearsal did the same), the user's copy keeps **kuvo-keys signed with
the Apple Development identity** (`KUVO_SIGNED_KEYS=1`). It needs it to hand 0.7's Keychain
items over at the move. `build-helper.sh` and `sign-desktop-app.sh` say no signed kuvo-keys is to
exist until #713 piece 4 settles its guard. Signed as `run.kuvo.code` by the user's team, it can
read Kuvo Code's Keychain items without a prompt, and three such builds sit in the feed's folder,
reachable on the tailnet. If the user doesn't want that, a build without it is the
`WITHOUT_KEYS` route; that's not built here.

## Once, in the user's Terminal on the Mini

```
cd ~/Projects/kuvo-ai && scripts/private-feed.sh setup
```

This makes the folder and the key, loads the login agent
`~/Library/LaunchAgents/run.kuvo.private-feed.plist` (`/usr/bin/python3 -I -m http.server 8790
--bind 127.0.0.1`), then prints the Tailscale command. The user runs that themselves (it changes
Tailscale's settings):

```
/Applications/Tailscale.app/Contents/MacOS/Tailscale serve --bg --https=8443 http://127.0.0.1:8790
```

It exposes that one local port to the user's tailnet only, over https. If the tailnet hasn't
turned Serve on, Tailscale prints a link to allow it in the admin console.

To undo: `… serve --https=8443 off`, then `scripts/private-feed.sh uninstall` (the agent and the
folder). The key stays unless `--key` is given, since the copy trusts no other.

Optional, to try the signing first:
`SIGN_IDENTITY="Apple Development" scripts/heavy.sh mac-build scripts/private-feed.sh dry-run`
runs the dry run below with the copies signed as the user's will be.

## Each build, in the user's Terminal on the Mini

```
cd ~/Projects/kuvo-ai && scripts/heavy.sh mac-build scripts/private-feed.sh build 0.8.0-beta.3
```

After that, `… private-feed.sh build` takes the next beta by itself.

1. The script reads the box key and the feed's key from the keychain (macOS may ask to allow
   access the first time).
2. It builds, signs, makes the disk image, signs the feed item, and puts both in the feed. The
   files go in first and the feed that names them last.
3. It prints the disk image's tailnet URL. The user downloads it once on the MacBook for the
   **first install**:
   - The app isn't notarized, so a browser download is quarantined and Gatekeeper refuses it.
     Fetch it with `curl -fLO <url>` in Terminal instead (curl sets no quarantine).
   - Updates after that are fine: Sparkle clears quarantine on what it installs.
4. From then on the copy finds each new build at its daily check, or with Kuvo › Check for
   Updates….

`scripts/private-feed.sh status` shows what's in the feed and whether it's served.

## At 0.8: on to the real feed

**The user's copy can't update itself onto the real feed.** Sparkle lets an update change the
EdDSA key *or* the code-signing certificate, never both. Its docs say it changes the key only
for a Developer ID app, and (with `SUVerifyUpdateBeforeExtraction`, which the app sets) only
through a Developer ID–signed disk image. The user's copy is Apple Development–signed and trusts
the private key.

So at 0.8, the user installs the 0.8 disk image once by hand, dragging it over their copy.

- **What carries over:** the bundle ID is the same, so the app's data and settings stay. From then
  on the copy reads the real feed with the real key.
- **What may not carry over:** the signer changes from Apple Development to Developer ID, for the
  app and for Kuvo Helper. macOS keeps what was allowed (privacy permissions) and some Keychain
  access by the signer, so it may ask again. The same happens at the **first private install**,
  where the user's 0.7, signed with Developer ID, gives way to an Apple Development–signed copy.
  This is not checked here.
- **Afterwards:** `scripts/private-feed.sh uninstall --key` and `serve --https=8443 off`.

If the user wants no manual step at 0.8, the other choice is signing the private feed with the
**real** key. Their copy would trust it from the start, and 0.8 would only change the
certificate, which Sparkle allows. That needs the real key in the Mini's keychain, moved there by
the user, so the Mini could sign updates for every 0.7 copy. Not built.

## The proof

`scripts/heavy.sh mac-build scripts/private-feed.sh dry-run`:

1. `setup` in a temporary folder, with a temporary keychain (never on the search list, no
   idle lock).
2. The login agent's own command, from its plist, by PID on 127.0.0.1:8791.
3. Two `build`s on the `PRIVATE_FEED` path with `run.kuvo.code.privatefeedtest`, ad hoc (or
   `SIGN_IDENTITY`), over http, with `KEEP=1`. A non-beta and a not-newer version are refused.
4. The old build, installed from the feed's disk image, opened by its program, finds beta.2,
   checks it with the feed's key, and installs it when quit by PID. `check-desktop-app.sh` passes
   on the result.
5. The server's log shows the feed and the disk image fetched from it. Then `uninstall --key`,
   and everything goes.

What only the real build does: `run.kuvo.code`, https, and the box key from the keychain.
