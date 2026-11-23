# Where each change is documented

**Parked**: the website and its docs get no changes and no docs branches until the user says go.
When they resume, this is the rule: a change that affects what users see, run or rely on isn't done
until the docs in `../kuvo-web` match it.

- Update them in the same task. Before reporting, check the pages mapped below for the files you
  touched and say which you updated, or that none needed it.
- Document only what you confirmed in the code; where they disagree, the code wins: fix the docs
  and say so.
- Commit kuvo-web separately, saying what changed for users. Pushing it deploys kuvo.run: only
  when the user says so.
- Set `updated:` to today on every page you change. Keep docs short: what people do and see, no
  internals, no exact limits or tables the app already labels.

The website and docs live in a separate repo, **kuvo-web**, checked out next to this one (`../kuvo-web`). Kuvo Code's docs are Markdown files in `src/content/docs/code/`, ordered by `src/lib/docs-nav.ts`, and its landing page is `src/pages/code/index.astro` (the site also hosts Kuvo Box under `/box`, and kuvo.run's own page is `src/pages/index.astro`).

User-facing means, for example: anything in the window, the side panel, the menu bar's usage panel, menus, settings or keyboard shortcuts; what the agent can do (tools, browsing, permissions); file and folder paths; environment variables users set; requirements (macOS version, hardware, downloads); install, update and uninstall steps; what Kuvo Code stores or changes on the user's Mac; and the network connections it makes.

## The map

All paths on the right are in `../kuvo-web`.

| When you change… | Check these files |
| --- | --- |
| `gpui/` (desktop app: areas, conversations, settings and box connection) | `src/content/docs/code/sessions.md`, `work-queue.md`, `machines.md`, `settings.md`, `privacy.md`, `installation.md`, `shortcuts.md`; this repo’s `README.md` and `gpui/README.md` |
| `box/src/browser/` (built to `box/dist/kuvo-browser.js`), `box/src/link/browser/`, `scripts/browser-runtime.py` (the browser on every box: its keeper, tools, live view, the box's own setup of it) | `src/content/docs/code/machines.md`, `browsing.md` (local and remote browser), `panel.md`, `privacy.md` (what it keeps in `~/.kuvo-code`, Chrome for Testing and Rocky Linux downloads), `uninstall.md`, `shortcuts.md`; this repo's `CONTRIBUTING.md` (The box browser); run `scripts/build-built.sh` after a change (its `KEEPER` is a hash it writes: never bump it) |
| `box/src/shared/integrations/` (the integrations and popular MCP servers a release carries), `box/src/keeper/kuvo-npx` (integrations, their keys, `kuvo-npx`) | `src/content/docs/code/integrations.md`, `privacy.md` (keys, the `integrations` folder, connections), `uninstall.md` (Keychain items), `architecture.md` (third-party pieces), `settings.md` |
| `box/src/install/kuvo-install`, `box/src/link/install/*`, `box/src/link/mac/link.ts` (a computer's first box installed from the release an app carries; a Mac's box's own relay and "Let my devices connect") | `src/content/docs/code/privacy.md` (`~/.kuvo-code/updates/install`, the engine download from npm, the relay), `uninstall.md` (the `@reboot` line and launch agent, `trash/earlier-box-*`), `installation.md` and `troubleshooting.md` (GPUI invokes this installer from `gpui/kuvo/src/launch.rs`) |
| Files, folders, defaults or website data Kuvo Code creates on the user's Mac | `src/content/docs/code/privacy.md`, `uninstall.md`, `troubleshooting.md` |
| `scripts/icon.py`, `mac/AppIcon.icon` (the app icon) | `public/code/icon.png`, `icon-dark.png`, `public/code/apple-touch-icon.png` and `public/code/og.jpg` (redraw with `python3 scripts/og-image.py code`) must show the same mark, and so must this repo's `docs/images/icon.png` (the README) |
| The version number | `src/lib/site.ts` (`code.version`), and `CHANGELOG.md` here |

## Screenshots

Screenshots in `kuvo-web/public/code/screens`, and the README's `docs/images/screenshot-light.webp` and `screenshot-dark.webp` here (the same shot as the website's), show the real app. If a UI change makes one visibly wrong, tell the user it needs recapturing. Capture desktop screens offscreen with the `shots` binary documented in `gpui/README.md`. When capturing a live demo, use a separate bundle and scratch box home. Capture the window only (`screencapture -l`), never the full screen. `crop-usage.webp` is the Bars view rendered with sample numbers, not a capture.
