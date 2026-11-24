# Rust box 6.3b: Git, Changes, worktrees and projects, coverage

Part B of row 6.3 in [rust-box.md](rust-box.md). Part A (files and folders) is
[rust-files-coverage.md](rust-files-coverage.md).

The Rust side is `box/crates/kuvo_services/src/git/`: `projects.rs` (the projects list as
`box/projects.json` keeps it, and the project each worktree's folder and each other folder is filed
under), `trees.rs` (a project's linked worktrees, read from what git keeps as small regular files,
never by running git, looked for again off the request path each 15 seconds as `linkedTrees` does),
`mod.rs` (the routes). The resident registers it in `box/crates/kuvo-box/src/serve.rs`.

## Proof so far

- Conformance: `KUVO_CONFORMANCE_AREA=git` (`protocol/conformance/git.test.mjs`) passes on both
  `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box` (macOS): none kept; kept projects with
  their other folders, the box's worktree records and a linked worktree made with `git worktree
  add`; a list from a newer box refused (409, its words).
- Differential: `GET kuvo/projects` on a seeded home (projects, records, odd fields) answers the
  same from both boxes.
- A TS bug fixed (the conformance case fails on the old TS): a project marked removed by hand with
  something other than `true` (`0`, `"yes"`) was answered as it was written, against the route's
  shape (`removed?: bool`); both boxes now say `removed: true` where it reads as removed and leave it
  out where it doesn't, as it's filed.
- Unit tests in `kuvo_services`: reading the list (versions, filtering), filing other folders and
  worktrees, a project's linked worktrees naming it back.

## The TypeScript cases

| TS case | Status |
| --- | --- |
| projects-worktrees 1: the projects list says which project each worktree's sessions belong to (kept, removed, restorable) | Conformance (`kept projects …`: records `worktrees`, `restorable`, `handedOver`, `removed`) |
| projects-worktrees 2: a worktree made after a client began following is filed at once | **Left**: `kuvo/projects/events` |
| projects-worktrees 3: a git worktree made elsewhere is filed under its project, one that's a project of its own isn't | Conformance (`kept projects …`), unit test (`a_projects_linked_worktrees_name_it_back`) |
| projects-worktrees 4: other folders filed under their project, a worktree of one too, not a project of its own or a removed one's | Conformance, unit test (`other_folders_are_filed_under_their_project…`) |
| projects-worktrees 5: project and worktree edits refuse future records | Reading half: conformance (`a list from a newer box is refused`); edits **left** |
| projects-worktrees 6: the first-seen folder index preserves a future file | **Left** (project edits) |
| projects 1: a project added from any client is kept as named, and changed there | **Left**: `PUT kuvo/projects` |
| projects 2: a saved terminal project is returned as an ordinary one, cleaned on its next edit | Reading half: conformance (`terminal` left out); cleaning on edit **left** |
| projects 3–7: Remove Project archives sessions (worktrees', other folders', older ones), the engine down, removed worktrees, folders the client shows | **Left**: `DELETE kuvo/projects`, with the engine (row 5.1's project removal) |
| projects 8: the list followed, a line now and another when it changes | **Left**: `kuvo/projects/events` |
| worktrees (32), branch-backups (14) | **Left**: worktrees and Git routes |
| Changes (`kuvo/git/changes`, `…/count`, `kuvo/git/files`, `kuvo/git/status`, …) | **Left**: see below |

Counts so far: projects-worktrees 6 (3 covered, 1 partly, 2 left), projects 9 (1 partly, 8 left);
worktrees and branch-backups not started.

## What's left

- The Changes read (`kuvo/git/changes/count`, `kuvo/git/changes`, `kuvo/git/files`,
  `kuvo/git/status`). Each runs git in the repository, which the TS box does only through its safe
  git runner (`ownArgs`/`ownArgsLater` in `src/shared/git.ts`: its own options, the programs a
  repository's settings name switched off, a `commondir` that leads elsewhere refused). That runner
  is ported first, whole and tested, then the reads on it.
- `kuvo/projects/events`, `PUT kuvo/projects`, `DELETE kuvo/projects` (Remove Project, with the
  engine). The `projects` feature isn't advertised until they're there.
- Unknown fields of a project record: the TS box answers them as written; the Rust box answers the
  route's own fields (its answer is typed by the route's shape).
- The 6.3 cases of the other maps: golden-home projects (`protocol/conformance/compat.test.mjs`),
  project removal from 5.1, `projects-worktrees` and `start-cancelled-worktree` in
  [rust-engine-coverage.md](rust-engine-coverage.md).
- OSes: macOS. Windows and Linux are left.
