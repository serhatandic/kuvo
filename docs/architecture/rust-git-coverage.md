# Rust box 6.3b: Git, Changes, worktrees and projects, coverage

Part B of row 6.3 in [rust-box.md](rust-box.md). Part A (files and folders) is
[rust-files-coverage.md](rust-files-coverage.md).

The Rust side is `box/crates/kuvo_services/src/git/`: `projects.rs` (the projects list as
`box/projects.json` keeps it, and the project each worktree's folder and each other folder is filed
under), `trees.rs` (a project's linked worktrees, read from what git keeps as small regular files,
never by running git, looked for again off the request path each 15 seconds as `linkedTrees` does),
`safe.rs` (the safe git layer, `shared/git.ts`: `EVERY`, `OWN`, `GIT_QUIET`, `own_args`,
`asked_args`), `mod.rs` (the routes). The resident registers it in `box/crates/kuvo-box/src/serve.rs`.

## Proof so far

- Conformance: `KUVO_CONFORMANCE_AREA=git` (`protocol/conformance/git.test.mjs`) passes on both
  `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box` (macOS): none kept; kept projects with
  their other folders, the box's worktree records and a linked worktree made with `git worktree
  add`; a list from a newer box refused (409, its words).
- Differential: `GET kuvo/projects` on a seeded home (projects, records, fields the route doesn't
  name, `removed` written other ways) answers the same from both boxes.
- OSes: the conformance area passes on macOS (both boxes), native Windows (the cross-built
  `kuvo-box.exe`, run directly over SSH) and Linux (a static musl build under WSL), 4 of 4 each.
- A TS bug fixed (the conformance case fails on the old TS): a project marked removed by hand with
  something other than `true` (`0`, `"yes"`) was answered as it was written, against the route's
  shape (`removed?: bool`); both boxes now say `removed: true` where it reads as removed and leave it
  out where it doesn't, as it's filed.
- Fields the route doesn't name are answered as they're kept, as TS answers them
  (`RouteReply::json_as_kept` in `kuvo_server`; the route's typed answer refuses unknown fields).
- On Windows a folder that starts at a drive's top (`/work`, `\\work`) is a full path, as Node's
  `path.isAbsolute` reads it there.
- Unit tests in `kuvo_services`: reading the list (versions, filtering), filing other folders and
  worktrees, a project's linked worktrees naming it back.

## The safe git layer (6.3b2)

`kuvo_services::git::safe` ports `ownArgs` and `askedArgs` with everything they call: the same
arguments in the same order, the same refusals (`Err(Refused::…)` where TS answers `undefined`),
the same budget (whole milliseconds left, none left refused before git runs) and the same
environment (yours, with `GIT_TERMINAL_PROMPT=0`). No route uses it yet, so there's no conformance
or differential case; the proof is unit and scenario tests in `safe_tests.rs`:

- The arguments against the TS box's for the same inputs: 144 rows (`ownArgs`/`askedArgs` for 36
  commands in four repositories: none named, filters/converter/hook/watcher named, a filter and a
  hook whose names have `=`), the expected arrays as TS printed them.
- A hostile repository (`hostile`; the same case in TS, `box/test/git-hostile.test.ts`): its
  settings and an included file name core.hooksPath with hooks, a hook in settings, core.fsmonitor,
  core.pager, core.editor, diff.external, a diff driver with textconv and command, a merge driver,
  a required filter, credential.helper, protocol.ext with an `ext::` remote, core.sshCommand with an
  ssh remote, gpg.program with commit and tag signing, and an alias to `!cmd`. A control shows git
  itself runs them; 22 commands the box runs by itself (Changes reads, staging, write-tree, a commit,
  a tag, a branch, a checkout, `gc --auto`, `credential fill`) through `own_args` start none; what
  you ask for runs its hooks but no fsmonitor, pager, diff program or signature check. Both pass.
- Where they differ from TS, deliberately: one blocking call (no `…Later` twin: an async route runs
  it on a blocking thread); the TS twins differ in one place, `strayCommonLater` treating git that
  can't start or runs out of time as "not in a repository", where the sync one refuses: the Rust layer
  always refuses. Where git-lfs and git-crypt are installed is looked up each time, not kept per
  `PATH`. `PATH` is split as the system splits it (`;` on Windows). git's stderr is dropped, not read.
- A TS bug fixed in both: `write-tree` was counted a plain read, so its settings weren't read first,
  but it writes the index: a hook named in settings (`post-index-change`) ran, and a file that looked
  changed since went through the repository's clean filter. Snapshots (Undo) and worktree saves run
  it. The hostile case fails on the old TS.
- A transport gap closed in both: `OWN` switched off no program a repository's settings name for
  reaching another repository (`protocol.ext.allow` with an `ext::` remote, `core.sshCommand` with an
  ssh one, `remote.*.uploadpack`, `url.*.insteadOf` rewriting even a local path), nor its
  `core.askPass`. Now what the box runs by itself reaches only a repository at a local path
  (`fetch`, `ls-remote`, `clone`, `pull`, `push`, `fetch-pack`, `send-pack` with any other remote,
  and `remote update/show/prune/set-head`, `submodule update`, `request-pull`, `archive --remote`
  and `maintenance`, are refused: `Refused::Remote`), and only through git's local transport
  (`protocol.allow=never`, `protocol.file.allow=always` and each of ext, ssh, git, http and https
  `never`: settings on the command line outweigh a repository's own `protocol.<name>.allow`, which
  `protocol.allow` alone doesn't). `OWN` also gets `core.askPass=`. What you ask for is unchanged.
  The hostile case covers each (fails on the old TS); the backups' local fetch and clone still pass.
  Left as they are: `--no-optional-locks` and `gc.auto=0` would change what the box's own commands
  do, `diff.external` is covered by `--no-ext-diff`, and the environment (`GIT_ASKPASS`,
  `SSH_ASKPASS`) is the caller's.

| TS case | Status |
| --- | --- |
| git-programs 1: the repository's programs are really there | `hostile::the_repositorys_programs_are_really_there…` |
| git-programs 2–7: Changes, the branch menu, switching branches, a review, Undo, backups start none | Layer half: `hostile::what_the_box_runs_by_itself…`, `…what_you_ask_for…` (the commands they run); the routes **left** |
| git-programs 8: what you stage and commit runs its filters and hooks | `hostile::what_you_ask_for…`; the route **left** |
| git-programs 9: git-lfs as installed stays on, one of the same name elsewhere doesn't | `hostile::a_file_git_lfs_keeps…`; Changes showing it unchanged **left** |
| git-programs 10: a program name settings can't switch off isn't read | `hostile::one_whose_program_has_a_name…`, table (`unswitchable`, `hookeq`) |
| git-programs 11: git's own watcher on a Mac | `hostile::gits_own_watcher…` |
| git-common 1–3: commondir elsewhere, a bare repository made around a broken `.git`, own/none/worktree as ever | `common::…` (the layer's refusals); Changes' Commit **left** |

Where the layer's tests ran: macOS (20 of 20; the TS hostile case 4 of 4), native Windows (the
cross-built test executable over SSH, git 2.56: 9 of 9, the hostile and `commondir` cases being
Unix-only; the `git` conformance area 4 of 4 on `kuvo-box.exe`), and Ubuntu 24.04 under WSL2 (a
static musl build, git 2.43: 20 of 20; the `git` conformance area 4 of 4). The table's expected
arrays, printed by the TS box on a Mac, hold on Windows and Linux too, where git's own watcher is
never kept on.

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
- The 6.3 cases of the other maps: golden-home projects (`protocol/conformance/compat.test.mjs`),
  project removal from 5.1, `projects-worktrees` and `start-cancelled-worktree` in
  [rust-engine-coverage.md](rust-engine-coverage.md).
