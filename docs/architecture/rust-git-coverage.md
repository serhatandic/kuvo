# Rust box 6.3b–c: Git, Changes, worktrees and projects, coverage

Parts B and C of row 6.3 in [rust-box.md](rust-box.md). Part A (files and folders) is
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
environment (yours, with `GIT_TERMINAL_PROMPT=0`). The Changes routes now run through it (6.3c below). Its layer proof is unit and scenario tests in `safe_tests.rs`:

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
| git-programs 2–7: Changes, the branch menu, switching branches, a review, Undo, backups start none | Layer half: `hostile::what_the_box_runs_by_itself…`, `…what_you_ask_for…` (the commands they run); the Changes and branches routes are served; equivalent hostile route scenarios and review/Undo/backups **left** |
| git-programs 8: what you stage and commit runs its filters and hooks | `hostile::what_you_ask_for…`; the routes are served; equivalent hook/filter scenario **left** |
| git-programs 9: git-lfs as installed stays on, one of the same name elsewhere doesn't | `hostile::a_file_git_lfs_keeps…`; Changes showing it unchanged **proof left** |
| git-programs 10: a program name settings can't switch off isn't read | `hostile::one_whose_program_has_a_name…`, table (`unswitchable`, `hookeq`) |
| git-programs 11: git's own watcher on a Mac | `hostile::gits_own_watcher…` |
| git-common 1–3: commondir elsewhere, a bare repository made around a broken `.git`, own/none/worktree as ever | `common::…` (the layer's refusals); Changes' Commit served; equivalent route proof **left** |

Where the layer's tests ran: macOS (20 of 20; the TS hostile case 4 of 4), native Windows (the
cross-built test executable over SSH, git 2.56: 9 of 9, the hostile and `commondir` cases being
Unix-only; the `git` conformance area 4 of 4 on `kuvo-box.exe`), and Ubuntu 24.04 under WSL2 (a
static musl build, git 2.43: 20 of 20; the `git` conformance area 4 of 4). The table's expected
arrays, printed by the TS box on a Mac, hold on Windows and Linux too, where git's own watcher is
never kept on.

## Changes and project edits (6.3c)

`PUT kuvo/projects`, `GET kuvo/git/files` and its events, and `POST kuvo/git/{changes,stage,unstage,discard,commit,push,branches,switch,merge,pull-request/draft,pull-request}` use the existing route shapes. The Rust route boundary is `git/routes.rs`; reading a diff, staging and committing, porcelain status rows and stamps, and branch actions are separate modules. Every Git operation goes through the argument gate and shared runner in `git/safe.rs`. Followers of a folder share a read: a newly following client waits for a read begun after it subscribed. The index, HEAD and its log are checked every second. Filesystem events are used on macOS and WSL's own filesystem, with periodic reads on other systems. Discards use the listed stamp and preserve intervening or blocking files in the Trash.

The `git` conformance area has these new cases (short names used in the table):

- **Edit:** a project is kept by its given folder, edited and brought back; newer list refused on GET and PUT, untouched.
- **List:** Changes lists, follows and edits the repository's real files (partial staging, move, deletion, untracked file, single-file and new-file diff, follower update, new follower, stage, unstage, stale discard, actual trash contents, commit staged only).
- **Branches:** branches, worktree merge and a pull request to a local remote (switch, push, clean draft, push on Create, linked worktree without record, dirty/clean merge and conflict rollback).
- **Discard:** discard checks the listed stamp, keeps blocking entries and unstages a first commit (literal path, missing stamp and invalid path, parent file, binary/large diff, first commit).
- **GitLab:** a failed push keeps the commit, and GitLab's pull-request page names the branch and base (local push URL, draft and prompt, title/body, no live network or hosting account).
- **Directory queries:** both Windows separators and the same parent-traversal guard for the three folder query routes. The new case failed on the old TS and the native Windows Rust box, before the shared path guard was corrected in both.

### Proof

| System | TypeScript | Rust |
| --- | --- | --- |
| macOS | 10/10 | 10/10 |
| Linux (Lokum, one container, 1 CPU/1 GiB; Git 2.39.5) | — | 10/10 |
| Ubuntu 24.04 under WSL2 (Git 2.43.0) | — | 10/10 |
| Native Windows (Git 2.56.1) | — | 10/10 |

The Mac's shared workspace check, TypeScript check, and Rust `engine-sessions` conformance (81/81) also pass. The native Windows run exposed the Windows directory query bug in both boxes' path guard; the new regression fails on the old TypeScript build and the old Windows Rust build.

Differential: six requests on copies of one seeded home (PUT and GET projects, file list, the diff of a deleted tracked file, branches, and refused pull-request draft). All six answers, the project record, the repository files (including its git records) and the engine transcript match. The full-home comparison still exits nonzero: 67 unrelated startup/identity/usage/work-queue differences, not hidden by the comparison. That's not a whole-home equivalence claim.

Size: 60,542 net added non-test Rust characters, without indentation, versus approximately 71,601 of the corresponding TypeScript route/functions (including project edit, pull-link and pull-request prompt; excluding the uncalled read/migration routes), 0.85×. No client or generated file is changed.

Related behaviour not yet matched/proven in full: Mac login refresh is per requested action rather than the TS minute cache; fallback Trash uses a private random directory instead of a timestamped directory, and the native Trash command is not yet time-limited. The shared conformance uses a local bare remote (no real hosting account); the signed-in GitHub command and its persisted session link still need their equivalent stand-in scenario. The table below records other scenario-level proof gaps.

### TypeScript case map

A case marked partly or proof left has an implementation but does not have the full equivalent scenario in shared conformance. Test numbers are their order in the named source file.

| TS case | Status |
| --- | --- |
| projects-worktrees 1, 3, 4 | Existing list conformance and linked-tree unit test |
| projects-worktrees 2 | Not ported: no client (`projects/events`) |
| projects-worktrees 5 | Edit: project format read and write refusal; worktree mutation left |
| projects-worktrees 6 | First-seen folder index, not called by PUT; proof left |
| projects 1–2 | Edit (including retired terminal setting) |
| projects 3–7 | Remove Project (`DELETE kuvo/projects`), with the engine (row 5.1); left |
| projects 8 | Not ported: no client (`projects/events`) |
| git-files 1 | List, grouped rows, partial stage and line counts; subfolder filtering proof left |
| git-files 2 | Conflict row proof left |
| git-files 3 | List covers move; spaced names, staged new discard and nested repository proof left |
| git-files 4 | List, Discard: one-file, new-file, binary and large diff; old path and binary false positive proof left |
| git-files 5 | Branches |
| git-files 6 | Branches, GitLab partly; detached and recorded base proof left |
| git-files 7 | List, follows index changes |
| git-files 8, 13, 18 | Files/folders: see [coverage](rust-files-coverage.md) |
| git-files 9, 12, 21 | List, staged-only commit; staged elsewhere and cross-folder rename count proof left |
| git-files 10–11 | Discard confirmation for lost staged content; proof left |
| git-files 14 | List, stale new-file discard; committed-since proof left |
| git-files 15 | Discard, blocking parent kept and file restored |
| git-files 16, 20, 22–24 | Staged deletion, old path preserved, identical disk version, intent-to-add and untracked after cached removal; proof left |
| git-files 17 | Discard, big file; cap at 1,000 names proof left |
| git-files 19 | List, a newly following client sees a new read; simultaneous read-sharing proof left |
| git-files 25 | Discard, missing stamp refused |
| git-files 26–27 | Nested repo stamp and read-in-progress stamp; proof left |
| git-files 28 | List, index change; filesystem-only and branch-only reset proof left |
| changes (bridge) 1 | List, tracked and new file diffs (whole-list diff and old-file stamp proof left) |
| changes 2–5 | New-file stamps, blank lines, converters, outside repository: proof left |
| changes 6–10 | Not ported: no client (`changes/count`) |
| changes 11–14 | List, Discard |
| changes 15 | Not ported: no client (`git/show`) |
| changes 16 | Discard, first commit including new files; folder-only and commit message diff proof left (`commit-diff` not ported: no client) |
| changes 17 | Requested commands use filtered environment and refresh the Mac login environment; hook scenario proof left (refresh per action rather than TS's one-minute cache) |
| changes 18 | Blocking directory discard proof left |
| changes 19, 21, 23 | Discard: missing stamp, invalid path, unborn HEAD |
| changes 20, 22 | Move and nested repository discard proof left |
| changes 24 | Requested git settles on child exit even if a hook holds pipes open; proof left |
| changes 25 | Branch scope since merge base; proof left |
| changes 26–27 | Branches, GitLab: local push, failed push preserves commit; retry after failed commit+push proof left |
| changes 28 | Branches: clean/dirty merge and conflict rollback; nothing new and preexisting merge proof left |
| changes 29 | Recorded worktree base used, proof left |
| changes 30 | Branches: linked worktree without record |
| changes 31–32 | Branches, GitLab: draft and push/create fallback; signed-in/signed-out/missing gh and session's pull record proof left |
| changes 33–34 | GitLab: main base, host page and local push; explicit origin/HEAD proof left |
| changes 35 | Dirty submodule discard proof left |
| changes 36–37 | Branches: listing and create/switch; multiple dated branches, tag name, carried edits, hooks and failure proof left |
| changes 38 | Not ported: no client (`git/status`) |
| changes 39 | Files/folders: see [coverage](rust-files-coverage.md) |
| changes 40 | Bare repository menu implemented, proof left; status not ported: no client |
| git-words 1, 4–7 | Not ported: no client (`commit-diff`, `settings/move`) |
| git-words 2 | GitLab: prompt uses the computer's instructions |
| git-words 3 | No-origin refusal implemented; proof left |
| git-settings 1–5 | Settings migration: not ported here, no client for `settings/move` |
| git-programs 1–11, git-common 1–3, git-hostile 1–4 | Layer proof above; route-level equivalents of the hostile cases remain |
| worktrees 1–32, worktrees-where 1–12 | Worktree lifecycle/Settings routes outside this desktop slice (some appear in the generic worktrees route fixture); files/folders cases in the files coverage map |
| branch-backups 1–14 | Not ported: no client for `git/backups` and restore |

### Routes not needed by the desktop

Searched the desktop Rust sources, shared conformance tests and their route fixtures. The following have no caller or conformance request: **not ported: no client**.

- `kuvo/projects/events`, `kuvo/worktrees/events`
- `kuvo/git/show`, `kuvo/git/changes/count`, `kuvo/git/status`, `kuvo/git/commit-diff`, `kuvo/git/settings/move`, `kuvo/git/backups` and `kuvo/git/backups/restore`
- `kuvo/plugins/search`, `kuvo/providers/models`, `kuvo/sessions/read`, `kuvo/sessions/glance/events`

The desktop does not call any `kuvo/worktrees` routes; the generic worktrees route fixture is separate from the Changes slice. The grouped `projects` and `git` features are not advertised as complete (some routes in the groups are not served); the individual Changes features implemented here are advertised.

The remaining 6.3 cases in the other maps include the golden-home projects (`protocol/conformance/compat.test.mjs`), project removal from 5.1, and `projects-worktrees` / `start-cancelled-worktree` in [rust-engine-coverage.md](rust-engine-coverage.md).
