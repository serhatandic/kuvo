# Rust library, artifacts, marketplace, integrations, skills and MCP catalog coverage

Row 5.5 ports the Library, Artifacts, Marketplace, Integrations, Skills, MCP Catalog and `kuvo/programs` routes to `box/crates/kuvo_services` (wired in `box/crates/kuvo-box/src/serve/`). The work is split into two mergeable parts:

- **5.5a**: Library (`kuvo/library*`, `kuvo/setup/library/events`), Artifacts (`kuvo/artifacts*`, `kuvo/setup/outputs*`), Attachments (`attachment-paths`, `message-block`), and Setup Lists cases 17, 23 and 28.
- **5.5b**: Marketplace (`kuvo/marketplace*`, loopback MCP relay, OAuth/token vault connections, skill projection & setup-wide removal), Integrations (`kuvo/integrations*`, `kuvo/keys*`, startup integration upgrades), Skills (`kuvo/skills*`, `kuvo/skill-trash*`, `kuvo/skill-library*`), MCP Catalog (`kuvo/mcp/catalog*`, `mcp-setup`), and `kuvo/programs`.

## Platform verification (5.5a)

- **macOS (aarch64-apple-darwin)**:
  - `cargo test -p kuvo_services --lib --test library_artifacts`: 41 lib tests + 6 integration tests pass.
  - `KUVO_CONFORMANCE_AREA=artifacts`: 3/3 pass against both `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box`.
  - `KUVO_CONFORMANCE_AREA=library`: 12/12 pass against both `box/dist/kuvo-link.js` and `box/target/debug/kuvo-box`.
  - Differential run (`node protocol/conformance/differential`): all 16 request answers (excluding live disk `free` bytes), `artifacts/index-v1.sqlite` schema & tables (`artifact`, `scope`, `state.lines`), `artifacts/copies/`, `artifacts/log/*.jsonl`, `artifacts/settings.json`, `library/log.jsonl`, `library/backfill.json`, and thumbnail PNG bytes match 1:1.
  - Golden home (`protocol/conformance/compat/generate.mjs` current fixture): identical artifact & library reads (223 copy bytes).
  - Scoped clippy (`cargo clippy -p kuvo_services -p kuvo-box --no-deps --all-targets`): zero warnings in `artifacts/`, `library/`, `tests/library_artifacts.rs`, and `kuvo-box/src/serve/library_artifacts.rs` (16 pre-existing warnings remain in `kuvo_services::vault`).
  - `cd box && bun run check`: passes.
- **Linux (x86_64-unknown-linux-musl under WSL Ubuntu-24.04)**:
  - `KUVO_CONFORMANCE_AREA=artifacts`: 3/3 pass.
  - `KUVO_CONFORMANCE_AREA=library`: 12/12 pass.
- **Native Windows (x86_64-pc-windows-gnullvm)**:
  - `KUVO_CONFORMANCE_AREA=artifacts`: 3/3 pass.
  - `KUVO_CONFORMANCE_AREA=library`: 12/12 pass.

## Deliberate structural differences from TypeScript (5.5a)

1. **Decoupled peer and engine seams (`kuvo_services` vs `kuvo-box`)**: `kuvo_services` defines narrow traits (`SessionSource`, `OutputsPeers`, `LibraryPeers`) instead of importing peer or engine globals. `kuvo-box/src/serve/library_artifacts.rs` adapts `kuvo_peers::{Reader, RelayHandle, memory_read::Handle}` and loopback engine SSE.
2. **Typed domain model with serde at the edges**: `Kind`, `VersionStatus`, `Trigger`, `WhoKind`, `NoCopy`, `PinBy`, `LibraryKind`, `HandoverKind`, `ArtifactError` / `StoreError` are typed Rust enums/structs; `kuvo_store::format::ObjectFormat::additive` enforces format-version checks on `artifacts/log/*.jsonl`, `artifacts/settings.json`, and `library/log.jsonl` before any disk rewrite.
3. **Eager synchronous capture on async artifact writes**: `ArtifactStore::{change, change_all, before, shown}` capture file stamps and register an RAII `WaitingGuard` synchronously before returning their `Future`, matching TS `async` eager execution up to the first `await` while preventing queue leaks on cancellation.
4. **One platform seam per OS difference**: `CopyPolicy` isolates disk free space (`statvfs` on Unix vs `GetDiskFreeSpaceExW` on Windows); `Thumbnails` isolates `sips`/`qlmanage` behind `ThumbnailPlatform::MacOs` vs `ThumbnailPlatform::Other`.
5. **Thumbnail cache filename hash**: Uses integer millisecond mtime (`as_millis()`) instead of Node's floating-point `stat.mtimeMs`; cached PNG bytes are identical.

## Cross-map cases adopted from other rows

| Reference | Owner | Status (5.5a / 5.5b) |
|---|---|---|
| `rust-adding-coverage.md:149`, `rust-engine-coverage.md:434`, `rust-move-coverage.md:266`, `rust-setup-coverage.md:262`, `rust-updates-coverage.md:423` (`artifacts/*`, `library/*`, `box/marketplace/*`, `run/docs.changed`) | 5.5 | `artifacts/*` and `library/*` covered in 5.5a conformance + differential; `box/marketplace/*` in 5.5b |
| `rust-agents-coverage.md:300` (`artifacts.test.ts` artifact tool semantics) | 5.5 | Covered in 5.5a (`artifacts/recorder.rs`, `tests/library_artifacts.rs`, `protocol/conformance/library.test.mjs`) |
| `rust-backups-coverage.md:44` (retained artifact bytes & pruned version report) | 5.5 | Covered in 5.5a (`artifacts/store.rs`, `tests/library_artifacts.rs`, `protocol/conformance/library.test.mjs`, golden home check) |
| `rust-engine-coverage.md:128` (`worktrees-where.test.js` case 12, `kuvo/programs`) | 5.5b | Scheduled for 5.5b |
| `rust-peers-coverage.md:46` (`setup-lists.test.ts` cases 17, 23, 28) | 5.5a | Covered in `protocol/conformance/library.test.mjs` ("merged Library and Outputs views include away peers' kept copies and pending computers") |
| `rust-setup-coverage.md:78,100`, `rust-workqueue-coverage.md:97` (attachment path relocation in `offline-merge.test.ts` & `setup-handover-shared.test.js`) | 5.5a | Covered in `kuvo_services::artifacts::attachments` unit tests + `kuvo_engine::{attachments,attachment_retention}`; work-queue item-attachment dedup stays with 5.2 |
| `rust-store-coverage.md:46` (`artifacts`, `artifacts-route`, `integrations-upgrade`, `marketplace`, `skill-removal` format version refusal) | 5.5 | `artifacts`, `artifacts-route`, `library` covered in 5.5a (`tests/library_artifacts.rs`, `protocol/conformance/library.test.mjs`); `integrations-upgrade`, `marketplace`, `skill-removal` in 5.5b |

## Part 5.5a TypeScript cases

### `box/test/artifacts.test.ts`

| TypeScript case | Status | Coverage or owner |
|---|---|---|
| [its edit, heard on the engine's feed, is a version of its memory, with the words that changed marked](../../box/test/artifacts.test.ts#L165) | Covered | `kuvo_services::tests::library_artifacts::bot_memory_edits_word_diff_multi_edit_turns_and_outside_edits` |
| [several edits in one turn are one version, and the next turn makes the next](../../box/test/artifacts.test.ts#L192) | Covered | `kuvo_services::tests::library_artifacts::bot_memory_edits_word_diff_multi_edit_turns_and_outside_edits` |
| [a change made outside any turn (you, in Edit Bot) is a version by no agent](../../box/test/artifacts.test.ts#L209) | Covered | `kuvo_services::tests::library_artifacts::bot_memory_edits_word_diff_multi_edit_turns_and_outside_edits` |
| [a captured change cannot hide a newer write while waiting for git](../../box/test/artifacts.test.ts#L223) | Covered | `kuvo_services::tests::library_artifacts::queued_reads_never_hide_newer_writes` |
| [only the last queued read can say what the file will be recorded as](../../box/test/artifacts.test.ts#L236) | Covered | `kuvo_services::tests::library_artifacts::queued_reads_never_hide_newer_writes` (`ArtifactStore::as_seen` + eager `WaitingGuard`) |
| [a queued pin cannot hide a newer write](../../box/test/artifacts.test.ts#L248) | Covered | `kuvo_services::artifacts::store::ArtifactStore::pin` (no `Waiting` pushed for pin) |
| [a before that will be skipped cannot hide a newer write](../../box/test/artifacts.test.ts#L262) | Covered | `kuvo_services::artifacts::store::ArtifactStore::before` (`should_wait` guard) |
| [a queued before cannot hide the current file](../../box/test/artifacts.test.ts#L274) | Covered | `kuvo_services::artifacts::store::ArtifactStore::as_seen` (`Waiting::Before` byte check) |
| [a changed size or mtime needs no content read before changeAll](../../box/test/artifacts.test.ts#L284) | Covered | `kuvo_services::artifacts::store::ArtifactStore::as_seen` (`StatStamp` fast path) |
| [a bot folder that cannot be watched reports the filesystem failure](../../box/test/artifacts.test.ts#L302) | Covered | `kuvo_services::artifacts::folders::BotFolders` (returns watch error on missing/unwatchable dir) |
| [files a command makes during the bot's turn are that turn's; its caches and tools' folders aren't kept](../../box/test/artifacts.test.ts#L319) | Covered | `kuvo_services::tests::library_artifacts::bot_folders_and_turn_end_capture_command_files_excluding_scratch` |
| [a newer save the watch didn't tell, made while an earlier one waits its turn, is what the Shelf keeps](../../box/test/artifacts.test.ts#L343) | Covered | `kuvo_services::tests::library_artifacts::queued_reads_never_hide_newer_writes` + `BotFolders::settle` |
| [a file still settling as the turn ends is read once](../../box/test/artifacts.test.ts#L365) | Covered | `kuvo_services::artifacts::folders::BotFolders::settle` |
| [a preview can read an unchanged image file after its artifact copy is lost](../../box/test/artifacts.test.ts#L408) | Covered | `kuvo_services::artifacts::store::ArtifactStore::image_preview` |
| [a picture a command made and removed in the same turn keeps what it was, then its deletion](../../box/test/artifacts.test.ts#L431) | Covered | `kuvo_services::artifacts::store::ArtifactStore::change` (eager snapshot) |
| [a file a tool deletes before the record saw it keeps what it was, from the deletion's own change](../../box/test/artifacts.test.ts#L451) | Covered | `kuvo_services::artifacts::recorder::Recorder::heard` (`before` from patch) |
| [your edit to its memory, outside any turn, is by no agent and shows what changed since the watch began](../../box/test/artifacts.test.ts#L462) | Covered | `kuvo_services::tests::library_artifacts::bot_memory_edits_word_diff_multi_edit_turns_and_outside_edits` |
| [a line it grew reads as the words it added, not as the line rewritten](../../box/test/artifacts.test.ts#L478) | Covered | `kuvo_services::artifacts::changes::tests::word_diff_marks_changed_words_and_preserves_growth` |
| [a file a command made that the watch never told is that turn's all the same, once it looks again](../../box/test/artifacts.test.ts#L495) | Covered | `kuvo_services::artifacts::folders::BotFolders::missed` |
| [what a command made that the watch never told, nor its looks again, is found as its turn ends](../../box/test/artifacts.test.ts#L516) | Covered | `kuvo_services::tests::library_artifacts::bot_folders_and_turn_end_capture_command_files_excluding_scratch` |
| [without a watch (Linux, out of watches), a bot's are found as its turn ends](../../box/test/artifacts.test.ts#L542) | Covered | `kuvo_services::tests::library_artifacts::bot_folders_and_turn_end_capture_command_files_excluding_scratch` |
| [what a command made is kept, but not the scratch it left in a tmp folder](../../box/test/artifacts.test.ts#L554) | Covered | `kuvo_services::tests::library_artifacts::bot_folders_and_turn_end_capture_command_files_excluding_scratch` |
| [scratch a command left in a tmp folder, recorded before it was left out, is gone from the record when the box starts again, unless you kept it](../../box/test/artifacts.test.ts#L569) | Covered | `kuvo_services::artifacts::store::StoreInner::load` (scratch pruning on startup) |
| [a file a kept reply cited as what it made, read as the box starts again, is shown then, with its versions as they were](../../box/test/artifacts.test.ts#L602) | Covered | `kuvo_services::artifacts::prepare::citation_update` + `StoreInner::load` |
| [a project's are found as the turn ends: outside git by a walk, in git as git status lists them](../../box/test/artifacts.test.ts#L631) | Covered | `kuvo_services::artifacts::turnfiles::turn_files` |
| [in a Mac's home folder, the app's caches and logs that change meanwhile aren't the agent's](../../box/test/artifacts.test.ts#L655) | Covered | `kuvo_services::artifacts::turnfiles::skip_entry` |
| [in the home folder, what Kuvo Code keeps there (a file you sent with the message) isn't the agent's](../../box/test/artifacts.test.ts#L681) | Covered | `kuvo_services::artifacts::turnfiles::skip_entry` (`.kuvo-code`) |
| [are its outputs (each agent), not what was committed before it](../../box/test/artifacts.test.ts#L714) | Covered | `kuvo_services::artifacts::turnfiles::from_git` (`--since=@...`) |
| [is the bot's, in the turn it was downloaded in, as a download on its shelf](../../box/test/artifacts.test.ts#L752) | Covered | `kuvo_services::artifacts::recorder::Recorder::noted` |
| [cleared from the browser's folder after a week, it still opens from the record's copy, marked gone](../../box/test/artifacts.test.ts#L768) | Covered | `protocol/conformance/library.test.mjs` + `kuvo_services::artifacts::store::ArtifactStore::check_gone` |
| [for a conversation this computer doesn't have, says so and records nothing](../../box/test/artifacts.test.ts#L797) | Covered | `kuvo_services::artifacts::recorder::Recorder::noted` (404 when session unknown) |
| [a report it writes and shows is listed for the session and its project, as a document that was shown](../../box/test/artifacts.test.ts#L807) | Covered | `kuvo_services::tests::library_artifacts::library_handovers_filtering_hiding_backfill_and_peer_merging` |
| [a picture a tool made is an image, and a subagent's work is its parent's turn](../../box/test/artifacts.test.ts#L820) | Covered | `kuvo_services::artifacts::recorder::Recorder::context` (parent resolution) |
| [the engine's conversation run for one of this bridge's isn't recorded twice](../../box/test/artifacts.test.ts#L832) | Covered | `kuvo_services::artifacts::recorder::Recorder::session_info` (`engineFor` hidden check) |
| [a file the bot marks final is pinned, first in its list](../../box/test/artifacts.test.ts#L840) | Covered | `protocol/conformance/library.test.mjs` + `kuvo_services::artifacts::recorder` (`mark_final`) |
| [its bots' files are listed for the chat, each by its bot, with a hand-off marked](../../box/test/artifacts.test.ts#L855) | Covered | `kuvo_services::tests::library_artifacts::group_chat_handoff_and_living_document_from_wakeups` |
| [what a bot makes in its own conversation for its part of a chat is the chat's too, but its other work isn't](../../box/test/artifacts.test.ts#L887) | Covered | `kuvo_services::tests::library_artifacts::group_chat_handoff_and_living_document_from_wakeups` |
| [versions and their copies are there after the box starts again](../../box/test/artifacts.test.ts#L906) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` + conformance |
| [past the budget for older versions the oldest go, still listed; the newest and memory stay](../../box/test/artifacts.test.ts#L919) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [what you keep keeps every version past the budget; once you stop keeping it, they go as the next version passes it](../../box/test/artifacts.test.ts#L940) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [what you keep is still kept after the box starts again](../../box/test/artifacts.test.ts#L970) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [a deleted file stays listed as gone, with what it was](../../box/test/artifacts.test.ts#L981) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [a likely secret is listed but never copied nor shown, not even what changed](../../box/test/artifacts.test.ts#L996) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [a file its name doesn't tell the kind of is served as text only when it is; a Pages document is served as one](../../box/test/artifacts.test.ts#L1015) | Covered | `kuvo_services::artifacts::kinds` + `routes.rs` |
| [in a git project, code keeps only its change; a document there gets copies; what git ignores gets none](../../box/test/artifacts.test.ts#L1029) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [what git ignores, or won't say of (a file beyond a symbolic link), keeps no copy](../../box/test/artifacts.test.ts#L1057) | Covered | `kuvo_services::artifacts::policy::CopyPolicy` |
| [in a repository git won't read, nothing keeps a copy](../../box/test/artifacts.test.ts#L1074) | Covered | `kuvo_services::artifacts::policy::CopyPolicy::learn` (fail-closed `repo_unreadable`) |
| [with the disk down to its floor, what a new version replaces keeps no copy, but the newest, memory and what's pinned keep theirs](../../box/test/artifacts.test.ts#L1082) | Covered | `kuvo_services::artifacts::store::StoreInner::add_version` (`disk_floor`) |
| [your budget for older versions is kept, and older ones past it go at once](../../box/test/artifacts.test.ts#L1099) | Covered | `protocol/conformance/library.test.mjs` + `tests/library_artifacts.rs` |
| [a file that can't be read for a moment isn't taken as deleted](../../box/test/artifacts.test.ts#L1117) | Covered | `kuvo_services::artifacts::store::ReadOutcome::Unreadable` |
| [a long log is written again whole, and everything's there after the next start](../../box/test/artifacts.test.ts#L1133) | Covered | `kuvo_services::artifacts::store::StoreInner::compact` |
| [restore: only in a bot's folder or the project it was made in](../../box/test/artifacts.test.ts#L1151) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` |
| [restore: puts it back as a version by you, keeping what it replaced so it can be undone](../../box/test/artifacts.test.ts#L1166) | Covered | `protocol/conformance/library.test.mjs` + `tests/library_artifacts.rs` |
| [restore: never through a file someone left by the name it's put back through](../../box/test/artifacts.test.ts#L1191) | Covered | `kuvo_services::artifacts::store::StoreInner::restore_now` (`O_CREAT|O_EXCL` atomic temp) |
| [a document two wake-ups rewrote is a living page](../../box/test/artifacts.test.ts#L1211) | Covered | `kuvo_services::tests::library_artifacts::group_chat_handoff_and_living_document_from_wakeups` |
| [a turn's end and the next turn's start in one burst: the next turn keeps its message and its versions](../../box/test/artifacts.test.ts#L1228) | Covered | `kuvo_services::artifacts::recorder::Recorder::heard` |
| [no 'before' is made up from a file that changed again since the edit](../../box/test/artifacts.test.ts#L1250) | Covered | `kuvo_services::artifacts::changes::tests::unapply_exactly_reconstructs_exact_and_rejects_drifted` |
| [a file made and then edited in one turn is new in that turn's version](../../box/test/artifacts.test.ts#L1265) | Covered | `kuvo_services::artifacts::store::StoreInner::add_version` |
| [in a git project, a turn's several edits to code all show in its changes](../../box/test/artifacts.test.ts#L1275) | Covered | `kuvo_services::artifacts::store::StoreInner::add_version` (patch accumulation) |
| [paging through a list made in one moment misses nothing and repeats nothing](../../box/test/artifacts.test.ts#L1289) | Covered | `kuvo_services::artifacts::store::StoreInner::list` (composite `updated|id` cursor) |
| [a screenshot a Claude Code session took, then read, is one of its outputs; a file it only read isn't](../../box/test/artifacts.test.ts#L1330) | Covered | `kuvo_services::artifacts::recorder` (Claude screenshot read heuristic) |
| [a picture a read brings back as its file is the conversation's; the picture of a read that brought none back isn't](../../box/test/artifacts.test.ts#L1353) | Covered | `kuvo_services::artifacts::recorder` (read image attachment check) |
| [what show_file puts in a Claude Code conversation is one of its outputs, a picture or any other file](../../box/test/artifacts.test.ts#L1366) | Covered | `kuvo_services::artifacts::recorder` (`show_file` handler) |
| [a file reached through a link to the project is the same output as by its own path](../../box/test/artifacts.test.ts#L1382) | Covered | `kuvo_services::artifacts::paths::real_of` + `ArtifactStore::resolve_files` |
| [a file recorded under two paths before is listed once, the latest; paging still misses nothing and repeats nothing](../../box/test/artifacts.test.ts#L1398) | Covered | `kuvo_services::artifacts::store::StoreInner::list` (canonical-path dedup) |
| [the first output line reads its committed scope without a scan or a recording wait](../../box/test/artifacts.test.ts#L1422) | Covered | `kuvo_services::artifacts::routes` (`kuvo/artifacts/events` reads index directly) |
| [a turn-end folder scan can wait on the disk while outputs answer](../../box/test/artifacts.test.ts#L1449) | Covered | `kuvo_services::artifacts::store` (reads don't await `write_lock`) |
| [a list says whose, and a followed list sends a line now and one on each change](../../box/test/artifacts.test.ts#L1478) | Covered | `protocol/conformance/library.test.mjs` + `box/test/fixtures/routes/artifacts.json` |
| [what you keep in a conversation's followed list goes to the top, marked yours, as you keep it, and back in its place as you stop](../../box/test/artifacts.test.ts#L1494) | Covered | `protocol/conformance/library.test.mjs` + `artifacts/routes.rs` (`kuvo/artifacts/events`) |
| [artifact storage reads legacy history, stamps writes, and refuses future records before compaction](../../box/test/artifacts.test.ts#L1521) | Covered | `kuvo_services::tests::library_artifacts::secrets_git_policy_budget_restore_and_format_refusal` + conformance |
| [the link's artifact service reports an unsupported artifact format](../../box/test/artifacts.test.ts#L1555) | Covered | `protocol/conformance/library.test.mjs` ("future artifact index formatVersion is refused with 409 without touching the log") |
| [artifact collection from a home project excludes the app's own data](../../box/test/artifacts.test.ts#L1566) | Covered | `kuvo_services::artifacts::turnfiles::skip_entry` |

### `box/test/library.test.ts`, `artifact-budget.test.ts`, `attachment-paths.test.ts`, `artifacts-route.test.js`, `attachments-go.test.js`, `setup-lists.test.ts` (17, 23, 28)

| TypeScript case | Status | Coverage or owner |
|---|---|---|
| [a file an agent shows, a picture it generates and a file you attach are in the Library](../../box/test/library.test.ts#L91) | Covered | `kuvo_services::tests::library_artifacts::library_handovers_filtering_hiding_backfill_and_peer_merging` + conformance |
| [its code edits, writes, a bot's notes, what it opens in the panel, logs and its own screenshots aren't](../../box/test/library.test.ts#L114) | Covered | `kuvo_services::library::store::LibraryStore::hand` + `tests/library_artifacts.rs` |
| [files inside `<agent>/skills` stay out of the Library](../../box/test/library.test.ts#L137) | Covered | `kuvo_services::library::store::in_skills_dir` + `tests/library_artifacts.rs` |
| [a page it built is named by its title, and a subagent's is its parent's, at the turn](../../box/test/library.test.ts#L149) | Covered | `kuvo_services::library::store::html_title` + `tests/library_artifacts.rs` |
| [newest first, by whose, by kind and by words in its name, its bot's or this computer's](../../box/test/library.test.ts#L170) | Covered | `protocol/conformance/library.test.mjs` + `tests/library_artifacts.rs` |
| [Remove from Library hides it, the file stays, and it's still hidden after a restart](../../box/test/library.test.ts#L183) | Covered | `protocol/conformance/library.test.mjs` + `tests/library_artifacts.rs` |
| [a picture whose file is gone opens from the record's copy; one with nothing kept says so](../../box/test/library.test.ts#L198) | Covered | `protocol/conformance/library.test.mjs` + `tests/library_artifacts.rs` |
| [filled once from what came before it: saved conversations' shown files, pictures and your files](../../box/test/library.test.ts#L216) | Covered | `protocol/conformance/library.test.mjs` ("library backfill runs once from saved sessions") + `tests/library_artifacts.rs` |
| [each computer's items with the computer they're on, newest first; one away or too old is said](../../box/test/library.test.ts#L251) | Covered | `kuvo_services::tests::library_artifacts::library_handovers_filtering_hiding_backfill_and_peer_merging` + conformance |
| [one that can't be read is said, and the rest still shown](../../box/test/library.test.ts#L273) | Covered | `kuvo_services::library::peers::merged` + conformance |
| [another computer's part as last read is listed at once after a restart, with when it was seen](../../box/test/library.test.ts#L279) | Covered | `kuvo_services::library::peers::{keep_part, kept_part}` + conformance |
| [another computer asks only for the list, an item, its file or thumbnail, or removing one](../../box/test/library.test.ts#L304) | Covered | `kuvo_services::library::peers::library_for_peer` |
| [a size that rounds up to a thousand of its unit is said in the next one](../../box/test/artifact-budget.test.ts#L5) | Covered | `kuvo_services::artifacts::budget::tests::size_that_rounds_up_to_a_thousand_uses_next_unit` |
| [engine and native attachment fields relocate while quoted evidence and command history remain exact](../../box/test/attachment-paths.test.ts#L18) | Covered | `kuvo_services::artifacts::attachments::tests::engine_and_native_attachment_fields_relocate_while_quotes_stay_exact` |
| [queued prose, escaped blocks and unsent legacy paths stay as written](../../box/test/attachment-paths.test.ts#L47) | Covered | `kuvo_services::artifacts::attachments::tests::queued_prose_escaped_blocks_and_unsent_paths_stay_as_written` |
| [a followed list, asked over SSH, answers with what it says now and ends](../../box/bridge-tests/artifacts-route.test.js#L71) | Covered | `kuvo_server` NDJSON route handling + `protocol/conformance/library.test.mjs` |
| [a picture's bytes come back whole](../../box/bridge-tests/artifacts-route.test.js#L77) | Covered | `protocol/conformance/library.test.mjs` ("artifact version binary bytes come back whole over route command") |
| [bytes sent as text that aren't text come back whole too](../../box/bridge-tests/artifacts-route.test.js#L84) | Covered | `kuvo_server` `route` command `bodyBase64` encoding + conformance |
| [a refused artifact index reports its format over SSH](../../box/bridge-tests/artifacts-route.test.js#L90) | Covered | `protocol/conformance/library.test.mjs` ("future artifact index formatVersion is refused with 409 without touching the log") |
| [a broken image response reports a retryable failure and healthy previews still return the bytes](../../box/bridge-tests/artifacts-route.test.js#L100) | Covered | `kuvo_services::artifacts::routes` (complete buffered read before replying) |
| [attachments-go.test.js (all 7 attachment retention cases)](../../box/bridge-tests/attachments-go.test.js#L127) | Covered | `kuvo_engine::{attachments,attachment_retention}` (already ported in 3.2/2.4) + `kuvo_services::artifacts::attachments` |
| [setup-lists.test.ts case 17: Library first state includes away computer's kept files on never-opened bot tab](../../box/test/setup-lists.test.ts#L822) | Covered | `protocol/conformance/library.test.mjs` ("merged Library and Outputs views include away peers' kept copies and pending computers") |
| [setup-lists.test.ts case 23: Activity's first Outputs state includes kept file made by delegated session on another computer](../../box/test/setup-lists.test.ts#L948) | Covered | `protocol/conformance/library.test.mjs` ("merged Library and Outputs views include away peers' kept copies and pending computers") |
| [setup-lists.test.ts case 28 (Library): merged views name a computer until its first copy arrives and distinguish an away computer](../../box/test/setup-lists.test.ts#L1064) | Covered | `protocol/conformance/library.test.mjs` ("merged Library and Outputs views include away peers' kept copies and pending computers") |
| [startup-backfill.test.ts: saved usage is available while the old artifact journal is still being read](../../box/test/startup-backfill.test.ts#L23) | Open | 5.3 / usage service startup isolation (usage crate) |

## Part 5.5b TypeScript cases (pending)

Will be populated and proved in Part 5.5b (`marketplace.test.ts`, `skill-removal.test.ts`, `skills.test.js`, `integrations.test.js`, `integrations-host.test.js`, `integrations-upgrade.test.ts`, `integrations-upgrade-host.test.js`, `mcp-catalog.test.ts`, `mcp-setup.test.ts`, `mcp-refused-live.test.js`, `worktrees-where.test.js` case 12, and route fixtures `integrations.json`, `skills.json`, `names/skill-names.json`). Settings pages (`settings-mcp.test.ts`, `settings-skills.test.ts`) belong to row 6.1.
