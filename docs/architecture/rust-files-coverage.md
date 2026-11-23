# Rust box 6.3a: files and folders, coverage

Part A of row 6.3 in [rust-box.md](rust-box.md): a file's info, text and bytes, the folder lists,
finding files and suggested folders. Part B (Git, Changes, worktrees, projects) comes later.

The Rust side is `box/crates/kuvo_services/src/files/` (`address.rs`: which file a request names,
with one platform seam for `file://` addresses; `opened.rs`: types, kinds and opening a regular
file, with one seam for opening without waiting and execute bits; `mod.rs`: the routes). The
resident registers it in `box/crates/kuvo-box/src/serve.rs`.

## Proof so far

- Conformance: `KUVO_CONFORMANCE_AREA=files` (`protocol/conformance/files.test.mjs`) passes on
  both `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box` (macOS).
- Differential: 28 requests over `kuvo/file`, `kuvo/file/info` and `kuvo/file/text` (each kind,
  a missing file, a folder, a trailing slash, `withMode`, `max`, bad addresses, `POST`) answer the
  same from both boxes. The home inventories still differ only in what other areas write.
- Unit tests in `kuvo_services`: address reading, types and kinds, `max` as JavaScript reads it,
  text decoding.

## The TypeScript cases

| TS case | Status |
| --- | --- |
| file-info: each kind by extension, bytes measured, 413 over the limit | Conformance (`info by kind and size`) |
| file-info: missing and non-file paths, 400 for another scheme, 405 for POST | Conformance (`missing files, folders and other addresses`) |
| file-info: folder and text routes with literal URI characters | Text by `path64` and `uri`: conformance. The `kuvo/folders` half: **left** (route not ported) |
| file-info: byte downloads keep the announced size when a file grows or shrinks | Ported (`Download::reply` streams the announced length, errors if short); **left**: a deterministic Rust test (the TS one interleaves writes with the generator, which HTTP can't) |
| file-info: served by the other computer's `box route` command | **Left**: depends on the Rust `box route` command (not in this slice) |
| file-info: remote downloads opt into execute bits | Conformance over HTTP (`bytes with their type, length and, asked, mode`); through `box route`: **left** |
| file-info: metadata and bytes require the box password | Covered by `kuvo_server`'s own auth for every route; no files-specific case |
| file-info: read permission checked | Conformance (`unreadable files are refused`), info, bytes and text |
| file-addresses: each name comes back as the file it names | Conformance (`each name a client encodes …`) |
| files-find: in a repository, name before path match, shorter first, git-ignored skipped | **Left** (`kuvo/files/find` not ported) |
| files-find: outside a repository, a walk skipping .git, node_modules and builds | **Left** |
| files-find: refused without a folder, as `kuvo/folders` refuses | **Left** |
| files-find: what's typed reads as a web address only where it is one | **Left** |
| folders-suggested: recent projects first, then repositories in Projects and Developer | **Left** (`kuvo/folders/suggested` not ported) |
| folders-suggested: a home reached through a link, each folder once | **Left** |
| folders-suggested: many folders, a bounded list | **Left** |
| folders-suggested: nothing to suggest | **Left** |
| folders-suggested: the route answers this computer's home and folders | **Left** |

Counts: 18 cases; 6 covered, 3 partly, 9 left.

## What's left

- Routes: `GET kuvo/folders` (with `files=1`, `hidden=1`), `POST kuvo/folders`,
  `GET kuvo/folders/tree`, `GET kuvo/files/find`, `GET kuvo/folders/suggested`.
- `kuvo/file` bytes through the Rust `box route` command, and a Rust test of a file that changes
  while it's sent.
- OSes: tested on macOS only. Linux and native Windows (the `file://` seam takes drive letters and
  UNC hosts, untested) are left.
