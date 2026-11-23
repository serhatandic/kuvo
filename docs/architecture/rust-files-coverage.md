# Rust box 6.3a: files and folders, coverage

Part A of row 6.3 in [rust-box.md](rust-box.md): a file's info, text and bytes, the folder lists,
finding files and suggested folders. Part B (Git, Changes, worktrees, projects) comes later.

The Rust side is `box/crates/kuvo_services/src/files/` (`address.rs`: which file a request names,
with one platform seam for `file://` addresses; `opened.rs`: types, kinds and opening a regular
file, with one seam for opening without waiting and execute bits; `folders.rs`: folder lists, a
new folder and a folder's tree; `git.rs`: git's list of a repository's files, as the box runs git
by itself; `find.rs`: ranking files and reading web addresses; `order.rs`: Finder's order of names;
`mod.rs`: the routes). The
resident registers it in `box/crates/kuvo-box/src/serve.rs`.

## Proof so far

- Conformance: `KUVO_CONFORMANCE_AREA=files` (`protocol/conformance/files.test.mjs`) passes on
  both `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box` (macOS).
- Differential: 28 requests over `kuvo/file`, `kuvo/file/info` and `kuvo/file/text` (each kind,
  a missing file, a folder, a trailing slash, `withMode`, `max`, bad addresses, `POST`) answer the
  same from both boxes. The home inventories still differ only in what other areas write.
- Differential, folders: 26 requests over `kuvo/folders` (GET and POST), `kuvo/folders/tree` and
  `kuvo/files/find` answer the same, but for `home`, the account folder the harness gives each
  box its own of (`…/a/account`, `…/b/account`).
- A TS bug fixed (with a test that fails on the old code, in `files-find.test.ts`): finding files
  and listing folders cut the query at a second `?`, which the box's server sends as it is, so a
  typed `https://…/a?b=1` lost `?b=1` and a folder named with a `?` wasn't found.
- Unit tests in `kuvo_services`: address reading, types and kinds, `max` as JavaScript reads it,
  text decoding, Finder's order, ranking files, web addresses.

## The TypeScript cases

| TS case | Status |
| --- | --- |
| file-info: each kind by extension, bytes measured, 413 over the limit | Conformance (`info by kind and size`) |
| file-info: missing and non-file paths, 400 for another scheme, 405 for POST | Conformance (`missing files, folders and other addresses`) |
| file-info: folder and text routes with literal URI characters | Conformance (text by `path64` and `uri`; `kuvo/folders` with `files=1`, `hidden=1` in `a folder's folders …`) |
| file-info: byte downloads keep the announced size when a file grows or shrinks | Ported (`Download::reply` streams the announced length, errors if short); **left**: a deterministic Rust test (the TS one interleaves writes with the generator, which HTTP can't) |
| file-info: served by the other computer's `box route` command | **Left**: depends on the Rust `box route` command (not in this slice) |
| file-info: remote downloads opt into execute bits | Conformance over HTTP (`bytes with their type, length and, asked, mode`); through `box route`: **left** |
| file-info: metadata and bytes require the box password | Covered by `kuvo_server`'s own auth for every route; no files-specific case |
| file-info: read permission checked | Conformance (`unreadable files are refused`), info, bytes and text |
| file-addresses: each name comes back as the file it names | Conformance (`each name a client encodes …`) |
| files-find: in a repository, name before path match, shorter first, git-ignored skipped | Conformance (`finding files …`) |
| files-find: outside a repository, a walk skipping .git, node_modules and builds | Conformance (`finding files …`, `every file under a folder …`) |
| files-find: refused without a folder, as `kuvo/folders` refuses | Conformance (`finding files …`) |
| files-find: what's typed reads as a web address only where it is one | Conformance, every vector through the route (`what's typed reads as a web address …`) |
| folders-suggested: recent projects first, then repositories in Projects and Developer | **Left** (`kuvo/folders/suggested` not ported) |
| folders-suggested: a home reached through a link, each folder once | **Left** |
| folders-suggested: many folders, a bounded list | **Left** |
| folders-suggested: nothing to suggest | **Left** |
| folders-suggested: the route answers this computer's home and folders | **Left** |
| files-find (new): a `?` in what's typed or in a folder's name | Conformance (`finding files …`), and the TS test |

Counts: 19 cases; 12 covered, 2 partly, 5 left (all of folders-suggested).

## What's left

- `GET kuvo/folders/suggested`. It reads this computer's session list (the kept list,
  `run/lists/sessions.json`), which the Rust box keeps inside `kuvo_peers` (`pub(crate)`): it needs a
  read-only seam from there (or the engine's sessions) and a conformance case whose stand-in engine
  has project sessions, so both boxes read the same ones. Its folder half (repositories in
  ~/Projects and ~/Developer, links, macOS's asked-about folders) is plain file work.
- Finder's order (`order.rs`) follows the root collation for Latin letters, digits, spaces and
  ASCII punctuation; names in other scripts are ordered by code point, where ICU may differ.
- A folder that can't be read for another reason (500) says the system's words, which Node and
  Rust phrase differently.
- `kuvo/file` bytes through the Rust `box route` command, and a Rust test of a file that changes
  while it's sent.
- OSes: tested on macOS only. Linux and native Windows (the `file://` seam takes drive letters and
  UNC hosts, untested) are left.
