# Kuvo

Kuvo runs coding agents, with any model, on all of a user's computers. Every computer runs a box (`box/`: the link, the coding agents' bridge, shared code) around the opencode engine, unmodified. The desktop app (`gpui/`, Rust on GPUI) is a thin client of every box. macOS and Linux today; Windows (WSL2 and native) is being added. The Swift Mac app and the phone app are gone: Swift remains only in Kuvo Helper (`mac/helper/`).

Every session reads this file first (`AGENTS.md` and `GEMINI.md` link here). Rules marked (strict) never bend; the index below has the details.

## Rules

- **A box decides, a client draws (strict).** All logic that isn't drawing goes in `box/`: never in a client, never only for this Mac. Logic already in a client: don't extend it; move it or list it. If something can't be on the box, ask first.
- **Equal computers, one setup (strict).** Each box works with no client and talks to the others peer to peer through the relay; none is a hub. Lists merge across computers, rows name their computer, "Runs on" picks where work starts.
- **The Rust box (strict).** `box/` is being rewritten in Rust ([design](docs/architecture/rust-box.md)). Behaviour ports 1:1; the code is idiomatic Rust, never TypeScript translated line by line (real types, `Result` errors, ownership for resources, tasks and channels, one seam per platform). It is proven by the conformance cases against both boxes, side-by-side runs on the same data, and a coverage map of the TypeScript tests.
- **The TypeScript box is lightly frozen** until the Rust box replaces it: fixes and small features only, each with a scenario test.
- **Plain words.** Users read computer names, never "box".
- **Updates:** one version per computer, prepared everywhere then switched together; never locked, nothing manual; mixed versions keep working together.
- **UI drafts first (strict).** Before any new screen, control or visible change, draft two or three options (light and dark) with a recommendation; the user picks. Match the app's existing look and every state.
- **GPT models (Codex sessions) never change UI** (views, layout, visible text, styling): that goes to Claude sessions.
- **Tests (strict):** scenarios through real components; a bug fix starts with a test that fails on the old code; deterministic. Run only the tests for what you changed, no full suites. No AI reviews unless the user asks.
- **Never kill by pattern (strict).** No `pkill`, `killall` or `pgrep … | kill`; kill only what you started, by PID.
- **Footprint contained (strict).** Kuvo writes only in its own folders (`~/.kuvo-code` on a computer), gone when they're deleted; tests and demos use folders of their own.
- **Built files are never hand-edited (strict):** `kuvo-link.js`, `kuvo-agents.js`, `kuvo-browser.js`, `BRIDGE_VERSION`, `KEEPER`, `answers.json`, `features.json`. Generated bundles and vectors never go in git. Run `scripts/build-built.sh`. A feature name is one line, in order, in `FEATURES`.
- **Never push, tag or open a PR (strict).** Commit tested work to your branch; the lead merges and pushes on the user's word. No GitHub polling.
- **Commits:** small, plain messages; no Co-Authored-By or AI lines; no personal paths or secrets (the repo will be open source).
- **The repo holds what an open-source reader needs.** Working notes, drafts and shots stay in `.claude/`, which is ignored: never force-add it.
- **Changelog:** a line under Unreleased in `CHANGELOG.md` for anything people would notice.
- **Brand:** ask the user before icons, logos, colors or taglines.
- **The website is parked:** no `../kuvo-web` changes or docs branches until the user says go.
- **No outside contributions:** issues only, never PRs.

## Index

- `docs/architecture/how-kuvo-works.md`: how the boxes work (main computer, updates, ownership, agents, how clients connect) and where new code goes. Read first.
- `docs/architecture/message-delivery.md`: sending without waiting on a box.
- `docs/architecture/one-client-process.md`: one client process per computer.
- `docs/architecture/small-computer.md`: the resource budget; what starts on demand.
- `docs/architecture/agent-accounts.md`: coding-agent sign-ins and accounts.
- `docs/architecture/windows.md`: the Windows plan (WSL2 and native).
- `docs/architecture/rust-box.md`: the box rewritten in Rust: shape, contract, switch-over and phase plan.
- `docs/working-here.md`: how sessions work (reporting, the user away, subagents, handovers), designing UI, GitHub.
- `docs/testing.md`: read before writing a test; how to run them, Linux (lokum) and Windows checks.
- `docs/your-settings-pages.md`: adding a page to the desktop app's Your settings.
- `docs/footprint.md`: what Kuvo may leave on a computer, and the exceptions.
- `docs/releases.md`: releases only from tags, through the pipeline, on the user's word.
- `docs/upstreams.md`: the engine and other upstreams; the lead checks them weekly.
- `docs/docs-map.md`: which website page each change touches (parked).
- `CONTRIBUTING.md`: setup, builds, Rust through `scripts/cargo-locked.sh`, route scenarios, what branches don't edit by hand, compatibility markers.
- `protocol/PROTOCOL.md`: how boxes and clients talk. Routes are documented in their definition (`box/src/shared/routes`); `scripts/build-built.sh` writes `protocol/ROUTES.md` for a readable list. Add to PROTOCOL.md only a rule a client must know that the route can't say, in a few lines.
