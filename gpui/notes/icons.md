# Icons

**Ours:** `kuvo_ui/src/icon.rs`, SVGs in `assets/icons/`. Gallery page: `gallery/src/pages/icons.rs`.
Shots: `shots/icons-light.png`, `shots/icons-dark.png` (with the type ramp: "Icons and type").

## What Zed does (studied, not copied)

Files read: `crates/icons/src/icons.rs`, `crates/ui/src/components/icon.rs`,
`crates/assets/src/assets.rs` (GPL-3.0), and GPUI's `elements/svg.rs` and `assets.rs`
(Apache-2.0). Zed's SVGs are Lucide-derived (ISC); we use none of them.

- A typed enum of icon names, each mapped to `icons/<name>.svg`; tests check every name has a
  file and no file is orphaned.
- 16×16 line icons, one stroke weight, round caps and joins.
- Sizes from 10 to 48 px; a "square" size adds padding for hit areas.
- GPUI draws an SVG as an alpha mask filled with the element's text colour, so the colour
  comes from the caller, never from the file.
- The app's `AssetSource` serves them; spinning icons use a timed rotation.

## What we take as behaviour

- A typed `IconName` enum, generated with the file table by one macro, so a name without a
  file can't compile. The SVGs are embedded with `include_bytes!`, so nothing loads from disk.
- Lucide's icons (`assets/icons/MAPPING.md`), converted to a 16-unit grid with a 1.2 stroke and
  round caps and joins. Filled details (dots, the stop square) are fills, not strokes.
- **One line weight at every size** (the design reviews of 2026-11-16): an SVG's stroke grows
  with the size it's drawn at, so with one file per icon the rail's 22 px icons drew 1.65 px
  lines, the title bar's 18 px 1.35 and a row's 14 px 1.05: "different design systems". Each
  size is now served its own copy (`icons/<size>/<name>.svg`, from `Assets`) with the stroke
  set to land at `STROKE_PX` (1.4 px) on screen; 12 and 10 px thin in step with their size.
  Zed draws every icon at 14–16 px, so one file is enough there; we use 14 to 22.
- Sizes: rows 14, toolbars 16, the title bar 18 (in 32 px buttons), the rail 22 (in 36 px
  tiles), cards 20 (#779).
- Tinted by the caller: the `icon` grey at rest, everywhere (rail, title bar, toolbars, rows).
  What's open or on is told by its wash; its icon only steps up (`icon_on`), never to the
  text's near-black, so a selected icon doesn't read as a bolder one. A ghost
  button's icons follow the same rule. The sidebar and panel buttons in the title bar have no
  "on" wash: the sidebar or panel beside them shows it (Finder, Codex).
- A test loads every icon through the asset source GPUI uses.

## Next

More icons as screens need them (status rings, providers' logos are a brand question).
