# Releases and betas

CLAUDE.md points here. See `CONTRIBUTING.md` for build and changelog instructions.

- **Releases come only from the pipeline**: `.github/workflows/release.yml`. A manual run on `main` builds and checks the box release and GPUI bundle, signs and notarizes them, then uploads the dry-run artifacts. The manual dry run does not publish or include the DMG in its uploaded artifacts. Tags stop at verification while the first GPUI publication gate is closed. Don't build or publish a release by hand. Tags come only on the user's word.
- A manual run with `rust_packs` also builds the five Rust box target packs and uploads them with their signed manifest and compatibility pack in `KuvoBox-Rust-dry-run`. To rehearse beta.48 catch-up, use a higher `version` and pass the signed bridge `boxes.json` as `typescript_bridge`.
- Kuvo Code 1.0 is the desktop app in `gpui/`. The planned first public release lets installed 0.7 copies update into it after publication is enabled.
- **The user tries every update first**, the way people will get it, before any release or beta tag: `PROMPT=1 scripts/try-desktop-update.sh` rehearses the Sparkle update with dedicated test bundles and a local feed.
- **After publication is enabled**, an annotated `vX.Y.Z-beta.N` tag on `main` will go through the same pipeline as a prerelease, with the changelog's Unreleased notes as they are (don't rename the heading); Sparkle will offer it to clients opted into beta updates. The update feed keeps the newest release and any newer beta (`scripts/merge-appcast.py`). Sparkle ignores anything after a dash, which is why the build version is `X.Y.ZbN`.
