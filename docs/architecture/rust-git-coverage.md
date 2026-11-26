# Rust box 6.3b–d: Git, Changes, worktrees and projects, coverage

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
| git-programs 8: what you stage and commit runs its filters and hooks | `hostile::what_you_ask_for…`; Requested Programs conformance: configured clean filter changes the staged blob, pre-commit hook runs |
| git-programs 9: git-lfs as installed stays on, one of the same name elsewhere doesn't | `hostile::a_file_git_lfs_keeps…`; LFS conformance: installed filter makes a touched file appear unchanged in Changes and its file list (Unix); the different-path refusal remains a layer proof |
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
- **Folder commit:** with nothing staged, a subfolder's diff and commit include its changed and new files only, leaving an outside edit uncommitted. List also asserts the subfolder's grouped rows.
- **Bare menu:** a folder with a `.git` pointer to a bare repository lists its branch and refuses switching without a checkout.
- **Hook exit:** the commit succeeds while a post-commit hook's background child still holds the pipes open (Unix).
- **Requested Programs:** staging uses the configured clean filter and committing runs the configured pre-commit hook.
- **LFS:** the installed git-lfs stand-in stores a different blob; after its file's timestamp changes, both Changes routes still say nothing changed (Unix).
- **Login cache:** a Mac login shell's environment reaches commit hooks, private variables do not, and a changed startup file within the minute does not trigger another login read. This scenario fails on the old Rust box.

### Proof

| System | TypeScript | Rust |
| --- | --- | --- |
| macOS | 16/16 | 16/16 |
| Linux (Lokum, one container, 1 CPU/1 GiB; Git 2.39.5) | — | 15 passed, 1 Mac-only skip |
| Ubuntu 24.04 under WSL2 (Git 2.43.0) | — | 15 passed, 1 Mac-only skip |
| Native Windows (Git 2.56.1) | — | 13 passed, 3 skips (Mac login, Unix hook child PID and git-lfs stand-in) |

The Mac's shared workspace check, TypeScript check, and Rust `engine-sessions` conformance (81/81) also pass. The native Windows run exposed the Windows directory query bug in both boxes' path guard; the new regression fails on the old TypeScript build and the old Windows Rust build.

Differential: six requests on copies of one seeded home (PUT and GET projects, file list, the diff of a deleted tracked file, branches, and refused pull-request draft). All six answers, the project record, the repository files (including its git records) and the engine transcript match. The full-home comparison still exits nonzero: 67 unrelated startup/identity/usage/work-queue differences, not hidden by the comparison. That's not a whole-home equivalence claim.

Size: 62,669 net added non-test Rust characters, without indentation (2,127 more for the login cache and Trash deadline), versus approximately 71,601 of the corresponding TypeScript route/functions (including project edit, pull-link and pull-request prompt; excluding the uncalled read/migration routes), 0.88×. No client or generated file is changed.

Mac login refresh now shares the TS one-minute cache, waiting for the first read and for at most one second during later refreshes, keeping the last successful environment if a read fails. The native Trash command now has the TS 30-second deadline; a Rust test of a real child process proves it is stopped at the deadline, not a shared native Trash route scenario. Related behaviour not yet matched/proven in full: fallback Trash uses a private random directory instead of a timestamped directory; native Trash timing through the real route is not proven. The shared conformance uses a local bare remote (no real hosting account); the signed-in GitHub command and its persisted session link still need their equivalent stand-in scenario. The table below records other scenario-level proof gaps.

### TypeScript case map

A case marked partly or proof left has an implementation but does not have the full equivalent scenario in shared conformance. Test numbers are their order in the named source file.

| TS case | Status |
| --- | --- |
| projects-worktrees 1, 3, 4 | Existing list conformance and linked-tree unit test |
| projects-worktrees 2 | C: not ported: unreachable (`projects/events`); see caller audit below |
| projects-worktrees 5 | Edit and Worktrees future-record scenario: read and mutation refuse both newer records without changing bytes |
| projects-worktrees 6 | C: first-seen folder index belongs to Auto’s reviewer, postponed past 0.8 (6.1) |
| projects 1–2 | Edit (including retired terminal setting) |
| projects 3–7 | Remove Project and project removal partial-refusal scenario: main, other, removed-worktree and explicitly filed folders, retry; large paginated removal and the old bridge-only variant remain separate proof gaps |
| projects 8 | Not ported: unreachable (`projects/events`) |
| git-files 1 | List, grouped rows, partial stage, line counts and subfolder filtering |
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
| changes 6–10 | Not ported: unreachable (`changes/count`) |
| changes 11–14 | List, Discard |
| changes 15 | Not ported: unreachable (`git/show`) |
| changes 16 | Discard, first commit including new files; Folder commit proves folder-only diff and commit. Commit-message diff skipped (`commit-diff` not ported: unreachable) |
| changes 17 | Login cache: real Mac commit hooks prove filtered environment, minute-cache reuse, a slow refresh returning the previous read, the completed refresh and a failed refresh retaining the last successful environment |
| changes 18 | Blocking directory discard proof left |
| changes 19, 21, 23 | Discard: missing stamp, invalid path, unborn HEAD |
| changes 20, 22 | Move and nested repository discard proof left |
| changes 24 | Hook exit conformance (Unix); native Windows PID-cleanup scenario skipped |
| changes 25 | Branch scope since merge base; proof left |
| changes 26–27 | Branches, GitLab: local push, failed push preserves commit; retry after failed commit+push proof left |
| changes 28 | Branches: clean/dirty merge and conflict rollback; nothing new and preexisting merge proof left |
| changes 29 | Recorded worktree base used, proof left |
| changes 30 | Branches: linked worktree without record |
| changes 31–32 | Branches, GitLab: draft and push/create fallback; signed-in/signed-out/missing gh and session's pull record proof left |
| changes 33–34 | GitLab: main base, host page and local push; explicit origin/HEAD proof left |
| changes 35 | Dirty submodule discard proof left |
| changes 36–37 | Branches: listing and create/switch; multiple dated branches, tag name, carried edits, hooks and failure proof left |
| changes 38 | Not ported: unreachable (`git/status`) |
| changes 39 | Files/folders: see [coverage](rust-files-coverage.md) |
| changes 40 | Bare menu conformance, including refused switch; status not ported: unreachable |
| git-words 1, 4–7 | `commit-diff`: not ported: unreachable. `settings/move`: served by 6.2, see its migration case |
| git-words 2 | GitLab: prompt uses the computer's instructions |
| git-words 3 | No-origin refusal implemented; proof left |
| git-settings 1–5 | Settings migration is already served by `kuvo_services::git_preferences` (6.2), registered by integrations; see [Settings map](rust-settings-coverage.md) |
| git-programs 1–11, git-common 1–3, git-hostile 1–4 | Layer proof above; Requested Programs and LFS route scenarios; remaining hostile route-level equivalents left |
| worktrees 1–32, worktrees-where 1–12 | 6.3d lifecycle and location preview, with the case map below |
| branch-backups 1–14 | Not ported: unreachable routes `git/backups` and restore; the internal turn-time backup function is not a route caller |

## 6.3d — Worktrees and project removal

The worktree owner lives in `kuvo_services::git::worktrees`, the location preview
in `where_trees`. The owner acquires the complete engine and native session
inventory before removal, keeps the checkout while any session runs in it, and
marks an independent conversation's checkout used. Once the last open
conversation is archived, a private temporary index keeps staged and unstaged
edits as one commit. Restore applies that commit and leaves the files unstaged;
deleting the final conversation also drops the retained ref. The owner wakes on
session/run and engine reconnect events, not on a periodic sweep. The legacy
record file lock is shared with the existing starter and acquired off the
executor. The existing engine creation path and the Settings creation route use
the same safe Git adapter; bot-created projects and worktrees share the empty
first-commit helper. Project removal shares the session archive operation and
bounds concurrent writes at eight. Held native turns count as busy even if not
yet adopted into the current resident's run list.

The location preview shares in-flight first reads, retains up to 16 complete
previews for their exact input, and refreshes on demand after five seconds.
Where it differs structurally from TS: an owned resident task observes events;
typed worktree and restorable records omit absent optional fields; a temporary
directory owns the separate snapshot index; native inventory and the live
engine's rows are combined without allowing a historical idle row to hide a
live run. Root worktrees name exactly their checkout (without a trailing slash),
as exposed by the differential.

### Caller audit

Searched `gpui/kuvo_box/src`, `gpui/kuvo/src`, conformance and route fixtures,
the whole `box/src` tree (including dynamically assembled Git route prefixes),
`browser/tools.ts`, `link/work/tools.ts`, `link/letters`,
`link/peers`, forwarding, agent/MCP tool definitions, and the named handler
functions' internal references. No current retained desktop, tool, peer or
internal request calls the following endpoints (generic forwarding is not a
specialized caller): **not ported: unreachable**.

- `GET kuvo/projects/events`
- `POST kuvo/git/changes/count`
- `GET kuvo/git/show`
- `POST kuvo/git/status`
- `POST kuvo/git/commit-diff`
- `GET kuvo/git/backups`
- `POST kuvo/git/backups/restore`

Turn-time branch backup in TS is a direct function call, not a request to those
two endpoints; its separate internal behavior is not claimed ported by this
audit. The desktop notes mention Suggest a Message as not built. The current
Settings migration scenario does call `kuvo/git/settings/move`; this is
already served by the 6.2 owner, so no second implementation is needed.
Worktree routes (including events, records, remove, restore, location preview)
are served for Settings and session starts; the full worktree features are
advertised. Project removal is retained for the existing shared route
fixture and older clients. The broad `projects` and `git` feature groups
remain unadvertised because their uncalled routes are not served.

### Scenario map

The shared Worktrees area includes the eight existing route fixtures and new
scenarios for locations, archive/restore, first commit and deletion/follower,
future records, restart naming/collision and pushed branch, detached commits,
project removal (partial refusal/retry), shared/working/unknown-session retention,
clean/smudge snapshot, native conversation naming/retention and instruction,
parked-engine location preview, old Mac record handover, and Settings folder/warning/stale removal. The existing
`engine-worktree` cases cover linked/subfolder filing, new session instructions,
delete confirmation, failed start and client cancellation.

| TS scenarios | A: ordinary use, shared proof | B: uncommon edge, later proof | C: replaced |
| --- | --- | --- | --- |
| worktrees 1–32 | 1–2, 4–5, 8–12, 14–28, 31–32 | 3, 6–7, 13, 29 | 30 |
| worktrees-where 1–12 | 1–6, 8–11 | 7 (deep repeated parent names; basic disambiguation proven) | 12 (programs, row 5.5) |
| projects-worktrees 1–6 | 1, 3–5 | — | 2 (unreachable), 6 (Auto postponed) |
| start-cancelled-worktree 1–2 | 1–2 | — | — |

Worktrees A includes the shared busy/unknown engine inventory and the native
retained-session case; a held-native-turn-specific route scenario remains a
separate edge proof. The former separately running coding-agent bridge
(case 30) is superseded by the integrated native store plus retained holder run
state. An older relative folder naming a parent (`../trees`) is refused by the shared
route path guard in TS; the old direct-function scenario bypassed that guard.
The parked-engine case exercises an absolute folder, as Settings supplies,
while an inside-checkout relative folder is also tested.

A shared Settings scenario exposed a real TS HTTP bug: Remove used a raw path
as its action row ID, so the canonical route guard refused its separators
(403) even though the page advertised the action. Rust uses a reversible
URL-safe opaque row ID, not a weakened route guard. The reference keeps the
bug (its equivalent direct owner action is tested); the new Rust action test
fails on the earlier Rust binary too. Windows worktree location queries now
use the existing folder-query path rule, which accepts native separators but
still refuses parent traversal.

The stale `engine-worktree` Plan inheritance assertion was corrected: a newly
started conversation does not inherit Plan, in either box. The linked-worktree
filing and starter-chain assertions remain.

The specifically requested remaining Git proof obligations are classified
separately below (counts are obligations, not full TS test files).

| Class | Obligations |
| --- | --- |
| A (6) | Slow login refresh; failed login refresh; ordinary configured diff conversion/raw file lines and blank lines; requested branch switch with a normal checkout hook and carried edits; ordinary repository and linked checkout (creation, diff and branch routes); golden-home projects |
| B (4) | Broader configured-program combinations through automatic reads; two unusual malformed repository metadata arrangements (existing layer proof); configured-program integration with Undo/branch backup (existing layer proof, not equivalent route proof) |
| C (3) | Auto reviewer cases (postponed); old count/show/status/commit-diff and backup endpoint-specific answers (unreachable); Git settings migration already owned and proven in 6.2 |

The remaining earlier Git/Changes variant rows above still state their precise
partial proofs. In particular signed-in GitHub command/session link, native
Trash timing through the route, and the extra discard/stamp edge variants
are not silently counted as passed.

### Gates and findings

The final macOS binary passes files 16/16, Git 17/17, worktrees 22/22,
engine-worktree 9/9, projects 3/3, engine-sessions 81/81, agent-tools 4/4,
and agents 12/12; Settings Git migration and the durable-record golden home
both pass. The TS reference passes files 16/16, worktrees 22/22, Git 17/17,
engine-worktree 9/9, projects 3/3 and the golden home, with the Settings
refusal noted above. Workspace `cargo check --workspace --tests` and
`bun run check` are clean.
The six-request worktree differential compares identical answers, engine
transcript, repository files and worktree/project records on copies of the
same home; 177 unrelated startup/runtime/identity file differences remain
visible in the full comparison. The golden home includes the TS project list.
A single live pinned OpenCode 2.2.6 Go turn with DeepSeek v4.1 Flash reads a
file present only in its own checkout; archive removes the checkout and
restore brings the file back. The stand-in is used for deterministic cases only.

The source-size comparison at base `85f9cf8e3` (non-test lines and UTF-8
characters without indentation): Rust adds a net 1,895 lines / 45,776 characters;
TS replaced 1,020 lines / 63,388 characters (0.72× by characters). Rust reuses
one first-commit helper and one typed worktree owner for session start and
routes. Worktree sweeps follow session and record changes rather than periodic
polling, skipping inventory entirely when no records exist. Location previews
share their first lookup and keep a bounded 16-entry cache; an expired preview
returns the last complete view while refreshing on demand. A remaining cost is
Settings' detailed list (`git/worktrees.rs`, `Owner::list`): each checkout incurs
status, ahead-count and directory size reads on demand, rather than TS's cached
detail list. No idle worker is started for that view.

The scenario obligations above total 48 A, 10 B, 7 C.

| Area | macOS | Capped Linux musl | WSL musl | Native Windows |
| --- | --- | --- | --- | --- |
| Files | 16/16 | 16/16 | 16/16 | 15 pass, 1 skip |
| Git | 17/17 | 16 pass, 1 skip | 16 pass, 1 skip | 14 pass, 3 skips |
| Worktrees | 22/22 | 22/22 | 22/22 | 22/22 |
| Engine worktree | 9/9 | 9/9 | 9/9 | 9/9 |
| Projects | 3/3 | 3/3 | 3/3 | 3/3 |
| Engine sessions | 81/81 | 81/81 | 81/81 | 81/81 |
| Agent tools | 4/4 | 4/4 | 4/4 | 4/4 |
| Agents | 12/12 | 12/12 | 12/12 | 12/12 |

Windows surfaced a mixed-separator default location and a shared fixture that
expected a literal Unix folder suffix. The preview now assembles a native
path, and the fixture still checks an exact subfolder path using `path.join`.
The first native test package also lacked the cross compiler's runtime DLL;
including the matching x64 `libunwind.dll` resolved its loader error. The
corrected worktree area passed 22/22 on all four platforms; all test homes,
processes and the capped test container were cleaned up.
