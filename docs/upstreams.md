# Upstreams

Kuvo Code is built on other projects, and keeps up with them. **The lead (Kuvo Foreman) checks each upstream below every week** and tells the user what it found. Sessions don't check them at their start: they update one only when their task needs it. Either way, this is how to check and update each. (CLAUDE.md points here.)

## The opencode engine

Kuvo Code runs one pinned build of `@opencode/cli`. A new opencode release brings model and provider support, fixes and new agent features, so the app should move to it quickly.

- **Where it's pinned, for every box's release**: `box/src/shared/engine-pin.json` (the version, and the tarball URL and npm `integrity` of each of the four builds: Linux `x86_64` and `aarch64`, macOS `aarch64` and `x86_64`). `scripts/box-release.sh` writes the version into the keeper's files and the builds into `box.json` and `boxes.json`, and refuses a pin whose URLs don't end in its version.
- **What's newest**: `https://registry.npmjs.org/@opencode/cli-darwin-arm64/latest` gives the `version` and `dist.integrity`; `@opencode/cli-linux-x64/latest` and `@opencode/cli-linux-arm64/latest` give the Linux builds', `@opencode/cli-darwin-x64/latest` Intel Macs'. The release notes are on [anomalyco/opencode](https://github.com/anomalyco/opencode/releases).
- **Updating**: move the current pin into `engine-pin.json`'s `previous`, then
  update the version, URLs and integrities for all four builds. The release
  check (`scripts/engine-check.mjs`) verifies the link and bridge against both
  versions. Check the release notes for API or event changes, then try a
  session with a tool call, a permission request and a Changes review.
- The sandbox's plugin and Auto's reviewer count on how the engine's `read` takes another file for one that isn't there (`ReadTool.alternatePath`, and `FileAccess.authorizeRead`'s `siblingOf`, which doesn't ask about it again): in 2.2.5, one plain file beside it, never a link, by a rule `box/src/link/fence/lookalike.ts`'s `alikeName` covers. On each update, `strings` the binary and check that's still so; if it now takes a link, a file elsewhere, or names `alikeName` doesn't fold together, change `lookalike.ts` and the plugin (`fence/files.ts`) to match.
- `box/src/link/engine/compatibility.ts` works around engine bugs with providers through the engine's own settings, on each box for its own engine (today: GitHub Copilot's Claude models rejecting `thinking.block_binding`, anomalyco/opencode#55160). On each update, check whether the engine handles them now, and remove what it does.
- Do patch and minor updates on your own when all of that passes. Ask first if the engine's API changed in a way that needs real work, and say what changed.

## Integrations' servers

`box/src/shared/integrations/table.ts` names each integration's server: official URLs, and npm packages pinned to a version (mcp-server-apple-events, @softeria/ms-365-mcp-server, caldav-mcp, @doist/todoist-mcp; and chrome-devtools-mcp in `servers.ts` beside it). A box's release carries them as its catalogs (`scripts/box-release.sh`). Check the packages on npm (`https://registry.npmjs.org/<package>/latest`) and move the pins, reading their changes for new keys or settings; check that the official URLs still answer, and whether a service now has its maker's own server (move it off the browser or an open-source package when it does).

## Code copied from other projects

- **browser-use**'s page reader, `box/src/browser/buildDomTree.js` (version in `scripts/third-party-licenses.sh`): ours is 0.4.2's, unchanged, and 0.4.2 is the last browser-use release that has the file; later ones read pages through Chrome's DevTools protocol, which WebKit doesn't offer. Check new releases in case it comes back, and port fixes to it by hand if the agent's browsing needs them.
- **Brand marks** in `gpui/assets/marks` come from OpenUsage and models.dev. See
  `gpui/LICENSES-DEPS.md` and `THIRD_PARTY_LICENSES.txt`.

## Packaging and tooling

Also check, and update the same way (build, test, its own commit):

- Sparkle: version and checksum in `scripts/sparkle.sh`. Test updates through the desktop release workflow.
- `dmgbuild` in `scripts/build-dmg.sh`, the GitHub Actions in `.github/workflows/*.yml` (Dependabot also opens pull requests for these), and the `xcode-27` runner label once a newer Xcode's runner is out.
- kuvo-web's npm packages (`npm outdated` there), when you work on the site. Pushing kuvo-web deploys kuvo.run, so push only when the user says so.

## Every update

Its own commit, never mixed into the user's task. Build, run the affected tests, run `./scripts/third-party-licenses.sh` and commit `THIRD_PARTY_LICENSES.txt`, update any doc page or README that names the version, and add a `CHANGELOG.md` line when people would notice. If you can't check for updates (no network), say so instead of skipping silently.
