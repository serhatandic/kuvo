# Rust usage coverage (row 5.4)

This is an in-progress map, not a parity claim. A case marked **open** has not been reproduced against the Rust implementation. **Partial** identifies a corresponding new scenario which covers only part of the TypeScript case. The known-correction cases are intentionally skipped on the TypeScript oracle rather than weakening the Rust assertions.

Inventory: 392 named or parameterized test declarations. Parameterized declarations are represented by their source line; their variants are still open unless individually demonstrated.

Client call sites: `gpui/kuvo_box/src/areas.rs:1261` (limits, refresh,
reset and follow), `gpui/kuvo_box/src/settings.rs:279` (usage settings), and
`gpui/kuvo/src/insights.rs:23` (Insights overview, summary and detail; this one
is in the thin app rather than kuvo_box). All have resident route specs.

## New boundary scenarios

| Area | Scenario | Scope |
|---|---|---|
| Settings | `usage settings survive restart and stale changes do not replace them` | Both boxes |
| Settings | `usage import is once-only and older clients preserve settings they do not know` | Both boxes |
| Readings | `a stale Codex window cannot replace the current ring` | Rust-only correction |
| Notices | `a crossing of ten percent remaining is told only once in its current window` | Rust-only correction |
| Notices | `usage warnings arrive through the existing in-app notice history` | Rust-only correction |
| Menu | `the Helper menu and warning facts use the same accepted reading` | Rust-only correction |
| Demand | `manual refresh reads Codex on demand, not on an idle startup` | Rust-only design change |
| Demand | `a subscribed rail reads Codex logs without starting an app-server` | Rust-only design change |
| Insights | `Insights catches up before first open and folds changed transcripts without double-counting` | Rust-only design change (supersedes first-slice demand-only index) |
| Insights | `a portable transcript contributes its tokens and cache reads to Insights on either box` | Both boxes |
| Limits | `the empty limits answer states every service and refresh state in the contract order` | Both boxes |
| Cache | `an unreadable accepted-limit cache cannot prevent the resident from opening or be overwritten` | Rust-only format guard |
| Engine plans | `engine credential plans are read with their own sign-ins and display bounded counts and credits` | Rust reader with a loopback maker fixture; TS scratch bridges deliberately disable these provider calls |
| Claude | `Claude Code answers its plan via its own demand-started control process` | Rust native fixture |
| Resets | `a Codex reset is offered without spending, journaled before the maker, and replayed once after restart` | Rust native fixture; account switch also refuses the old offer |
| Resets | `an agent has to ask before spending a reset, and Always Allow is account-specific` | Rust native and engine fixtures through the existing work-ask routes |
| Menu | `a connected Helper menu displays fresh log rings without launching Codex` | Rust passive reader |
| Native Insights | `native Claude calls replace portable estimates without counting private logs or replays` | Both boxes; Rust append, restart and rewrite checks |
| Limits peers | `two computers keep one account ring and never replace it with a peer’s older reset` | Two Rust residents through the real relay, local HTTP maker fixtures |
| Resets | `reset routes refuse missing or unknown accounts without creating a spend` | Both boxes |
| Standalone plans | `standalone Grok, Cursor and Antigravity read their own fixture accounts` | Rust CLI and loopback HTTP fixtures, including a fake Antigravity refill |
| SQLite projection cursor | `Insights decodes only changed SQLite transcript projections` | Real Rust transcript writer, watcher and query |
| Price cache | `a_price_book_is_shared_until_a_catalog_file_changes` | Scoped Rust test |
| Native log cursor | `usage_native_logs.rs` | Scoped Rust tests for append/partial-line/rewrite, Codex running state, Grok multi-model ticks |
| Pace | `usage_readings.rs` | Deterministic window regression, single false refill, same-window alert deduplication, previous-window pace exclusion |
| Pricing | `usage_pricing.rs` | Bundled price resolution, exact fallback, cache writes and fast rates |
| Engine history | `Insights watches the engine SQLite channel and counts only new or changed calls` | Both boxes for initial counts and billed hosted cost; Rust live-writer, mutation and restart checks |
| Antigravity history | `Antigravity generation metadata supplies native calls without reading conversation bodies` | Both boxes for initial counts; Rust live-writer mutation check |
| Engine limits | `OpenCode Go limits read the engine’s saved credentials without starting the engine` | Rust fixture SQLite and loopback maker; database unchanged |
| Usage history | `empty usage history retains service order on both boxes` | Both boxes, including local row and scan timestamp |
| Usage history | `native usage history is deduplicated, priced and assigned to the client’s calendar day` | Both boxes |
| Pricing | `Codex fallback pricing uses the saved counts without reparsing its logs` | Both boxes for fallback price and saved setting |
| Price feeds | `a changed price feed reprices saved history and a bad feed does not replace a good cache` | Rust isolated loopback feed, restart and ETag |
| History/Insights peers | `usage and Insights merge only admitted peer records, retain them across restart and filter by computer` | Both boxes, including mixed Rust/TS in each direction; Rust live delta without a request |
| Insights summary | `Insights groups model aliases and ranks named sessions from indexed usage` | Both boxes; names, filtering, cache rate, prior period, highest-cost standout |

| Cursor history | `Cursor keeps complete account-wide UTC days, reprices saved counts and refuses an incomplete page` | Rust maker fixture (Mac, Lokum, WSL and native Windows verified) |
| Engine subscription history | `engine ChatGPT history uses read-only OAuth windows, not paid API calls or duplicate channels` | Rust read-only maker fixture; TS scratch bridge disables engine credentials (Mac, Lokum, WSL and native Windows verified) |
| Claude grants | `Claude file-owned reset grants spend the maker’s named next grant once, with a saved request` | Rust maker fixture (Mac, Lokum, WSL and native Windows verified) |
| Notice handoff | `a peer remembers a crossing even after its visible notice expires` | Rust correction (Mac, Lokum, WSL and native Windows verified) |

| Peer resets | `a computer without the plan sign-in spends once on the holder, and shares Always Allow decisions` | Rust pair; Mac, Lokum, WSL and native Windows verified |
| Billed channel history | `a copied billed engine message uses the first channel and falls back when it is removed` | Both boxes; Rust empty-channel reconciliation verified on all four platforms |
| Antigravity refusals | `Antigravity never resends a refused bearer or refresh token after restart` | Rust loopback maker; Mac, Lokum, WSL and native Windows verified |
| Antigravity holds | `Antigravity keeps another host’s usable windows while one quota call is held` | Rust loopback maker; Mac, Lokum, WSL and native Windows verified |
| Antigravity local | `Antigravity reads only its running profile’s owned loopback TLS server without a bearer token` | Rust synthetic process/socket fixture and temporary self-signed TLS; Mac, Lokum and WSL verified; native Windows has its own actual-process fixture in ninth slice |
| Notice recovery | `usage_readings.rs`: corroborated recovery and off-toggle crossing | Deterministic scoped Rust tests; Mac 5/5 |
| Peer scheduling | `usage_peers` exact deadline helper; no timer with no reachable member | Rust idle design; verification pending |
| Engine ChatGPT resets | `the engine ChatGPT reset credit uses the saved OAuth identity without starting the engine` | Rust loopback fixture for earliest credit, restart replay and overlapping confirms; Mac, Lokum, WSL and native Windows verified |
| Unsettled spend | `an older unsettled reset is settled before a later confirmation spends another credit` | Rust Codex scripted maker; same old request and a distinct new credit; Mac, Lokum, WSL and native Windows verified |
| Plan tool | `a computer without the plan sign-in spends once on the holder, and shares Always Allow decisions` | Now additionally checks the remote account and reset-holder words through `plan_limits` |
| Codex archive and home roots | `Codex history prefers a live session’s tier to its copied archive and reads each configured home` | Both boxes; exact cost arithmetic, linked root variant on Unix, comma-separated homes |
| Exact pricing | `usage_pricing.rs`, bundled rate fixture with exact f64 bits; native corpus differential | Exact catalog numbers; wire aggregation order; Mac, Lokum, WSL and native Windows verified |
| Native Windows Antigravity | `native Windows finds only the listening language server of its own Antigravity ACP profile` | Actual native process tree and listening sockets, synthetic maker and wrong-profile sibling; Mac, Lokum, WSL and native Windows verified |

| Limits admission | `old Grok transcripts are history, never proof of a current sign-in` | Both boxes, fixture history without a credential |
| Limits admission | Extended standalone, native Codex/Claude and engine Go tests | Rust correction, credential withdrawal without another maker request; uses shared Grok Accounts credential and installation helper |
| Antigravity partial | `Antigravity retains a missing reset-less pool only for its first partial reading, across restart` | Rust maker fixture, old Rust failed |
| Antigravity rail | `Antigravity’s menu takes both rings from the same tightest quota pool` | Rust maker fixture and Helper menu, old Rust failed |
| Upgrade | `an upgrade retains the old accepted reading only for the still-signed-in account` | Rust legacy cache fixture, identity-gated, no CLI launch |
| Cards/rail | `accepted pace and the emptiest of three named accounts determine cards and rail, even after restart` | Rust deterministic accepted legacy windows/pace, plus reset card words in Codex reset scenario |
| Insights lineage | `Insights groups bot chats, overseen starts and nested subagents without expanding their transcripts` | Both boxes; old Rust failed bot totals |
| Insights kept | `a_completed_summary_survives_restart_and_does_not_wait_for_the_catch_up_gate` | Scoped real SQLite test, complete snapshot under same setup only |
| Semantic limits | `different provider window names for the same account do not create a second or stale ring` | Rust correction, old Rust showed two windows |
| Insights retained lineage | `a_manual_engine_header_refresh_repairs_retained_calls_without_recounting_their_origin` | Scoped real SQLite test; old draft failed to attach bot to retained calls |
| Insights pages | `Insights pages share the whole filtered snapshot, list its bots, and refuse invalid pages` | Both boxes; old Rust had empty bot choices; expected destination from actual ownership |
| Insights unowned | `Insights keeps every unowned session beyond the former ranked-list cap` | Both boxes; 201 compact engine fixture sessions |

## Deliberate work-timing changes

The user's revised rule is to keep frequently viewed data ready, incrementally, and to
start rarely used work only on demand.

- Insights begins a catch-up at startup on a blocking worker, subscribing to the
  platform's file watch before the catch-up. A burst of file changes folds the affected
  portable transcript(s) into SQLite using their metadata checkpoint. Reads serve the
  index, not a periodic history walk. A lost watcher event permits a checkpoint
  recovery pass. There is no separate Insights process. The startup/file-change
  conformance scenario checks SQLite before the first client query and before the next
  query, as well as duplicate prevention. Claude/Codex/Grok JSONL roots have
  event-driven per-file offsets with bounded prefix/suffix anchors; observations and
  cursor commit together. Native changes reattribute only saved compact steps, not
  unchanged transcripts; unowned personal observations are not projected into Insights.
  Engine channels now project compact usage and lineage directly from read-only SQLite,
  keyed by file/WAL revision and a mutation watermark with a one-hour overlap; no
  engine process starts. Antigravity uses generation metadata and a revision per
  database (the maker exposes no mutation clock), so only changed databases are
  decoded. Darwin's FSEvents may defer updates to an open SQLite writer until close;
  the native watch has a bounded vnode supplement for live database/WAL files, with
  folder discovery still using FSEvents. No timed scan or new process. The live
  writer scenarios verify both types of SQLite usage source.
  The engine header projection now looks up only the changed calls' session and
  ancestor IDs, rather than selecting every header in the changed channel.
  The engine fixture has 20 unrelated session headers and checks exactly one
  decoded header for its changed calls.
- The summary's SQL queries group by session, model/provider and computer;
  the daily/prior aggregates are also SQL sums. No call-sized vector is built
  just to view a period. A bounded 24-entry, ten-second cache is invalidated by
  index mutations and signed peer-directory changes; incomplete/loading
  summaries are not cached. The named-model/filter/prior/standout scenario
  checks the same route values on both boxes.
- Usage mail uses the existing authenticated seat protocol, bounded multipart
  assemblies and per-sender retained account slots. No periodic source read is
  needed to tell a peer: send recent changed slots on index changes, a full
  snapshot when requested, and an unchanged digest heartbeat. Insights pages
  use the existing per-peer SQLite cursor/epoch and accepted historical origins.
  Rust acknowledges a final nonempty page with a pull at the new cursor (an
  empty final reply stops it), so later local mutations reach the other box
  immediately; TypeScript requests again on its scheduled scan. Peer sender tasks now
  sleep until the next actual heartbeat, pending-page retry, or retained-reading
  expiry, and have no timer when no reachable member needs their data. The both-box
  scenario checks initial and retained data (and mixed versions), with the
  Rust-only no-request live-delta assertion documenting this work-timing change.
- Limits are refreshed on a rail/menu subscription, a manual refresh, turn readings,
  when a reset/projected crossing becomes due, and on a one-minute wake while an
  authenticated client request or stream is active (provider reads are throttled to
  ten minutes between non-manual refreshes). When there are no connected clients and
  no deadline, there is no periodic wake. The immediate rail/menu cases and the idle
  startup case demonstrate the difference. The fourth slice additionally forbids a
  passive quota read from starting a coding agent: Codex uses an already running
  app-server, watched recent quota snapshots from its own JSONL logs, or turn events.
  The app can explicitly refresh to ask the native CLI. Claude and Grok's native probes
  are also explicit-only; HTTP-only sources do not start CLI processes. The rail and
  Helper scenarios verify no Codex initialize or quota RPC occurs while log readings
  reach both surfaces. The deadline wake may read already running providers and HTTP
  makers, never launch a CLI. A separately timed periodic/client-detach
  and deadline scenario remains open.
- The price catalog (about 416 KiB bundled) is parsed once per Insights service;
  a new file stamp on one of its three catalog files invalidates the in-memory
  book. Resolution of an individual model (including misses and aliases) is now
  memoized within that catalog, bounded to 4,096 names. The native deduplication
  query uses separate exact `(provider,dedup)` and `(provider,message)` indexes
  instead of SQLite's provider-wide correlated scan: the query plan now uses both
  key columns, and a 1,350-call fixture selected the same 149 canonical records.
  Cost aggregation remains in SQLite over compact indexed rows, but its ordered
  REAL sum uses the reference's plain IEEE-754 addition instead of compensated
  SQLite `SUM`; the source/slot insertion order and per-token-bucket division
  match the TypeScript wire, not a tolerance in the comparison.
  The transcript writer adds compatible mutation-bounded SQLite tables and
  triggers, keeping the existing transcript schema version. An unchanged ownership
  header and generation allow Insights to select only rows whose projection
  revision advanced. A v1 file not yet opened for writing by Rust, a new database
  generation, or an actual attribution change needs a one-time compact catch-up.
  The fixture updates one of multiple projections and verifies that only one
  projection was decoded. This additive index is a deliberate transcript-file
  inventory difference; the client answers do not change. The price-feed refresh
  uses HTTP ETags, validates data before saving and reprices the compact indexed
  counts on a changed catalog, not the source logs. The client history queries
  aggregate index rows into UTC quarter-hour slots, then convert to requested local
  dates. Explicit refresh catches up the saved cursors.
- Cursor account-wide history is read in complete UTC-day pages, four at a
  time with one shared access exchange and a 20-second round budget. Completed
  old days are kept as raw compact token counts in SQLite; only today, yesterday
  and missing/incomplete days are requested again. It catches up at startup and
  on explicit refresh, or every five minutes with a connected client; another
  fresh account-wide peer reader suppresses maker reads. Changing the price
  catalog reprices the saved counts, with no maker calls. The Cursor fixture
  verifies 32 initial days, the two recent days after restart, a failed page
  preserving the former complete day, and repricing without new requests.
- Engine ChatGPT history is read from its own SQLite credential metadata without
  starting the engine, including the legacy auth-file fallback. It counts only
  zero-cost completed calls from OpenAI, bounds v2 usage by OAuth sign-in,
  deduplicates copied channels and prices saved counts locally. Credential
  changes invalidate the channel cursor, not the log history of other channels.
  TS scratch mode deliberately disables the engine credential call, so its
  reference scenario cannot exercise this signed-in path; the Rust test removes
  the engine rendezvous and checks sign-out invalidation.
- Claude's file-owned grant inventory is cached for one hour per token fingerprint;
  a failed inventory read keeps the last answer. Its normal passive refresh does
  not start Claude Code. The profile must identify the same account before a
  journaled grant is consumed. Production Mac Keychain remains unread.
- Remaining lightness risk: the platform watch fallback (`kuvo_os/src/watch.rs`,
  `start_watcher`) can compare contents on a polling filesystem when a native
  watch cannot start; avoid a history-wide fallback on large provider roots.


## Verification of the first port slice

| Computer | Usage conformance |
|---|---:|
| macOS Rust | 12/12 |
| macOS TypeScript | 4/4 applicable; 8 intentional Rust-only skips |
| Linux static musl, Lokum, one 1-CPU/1-GB container | 12/12 |
| WSL2 Ubuntu 24.04, same static musl binary | 12/12 |
| Native Windows, cross-built gnullvm binary | 12/12 |

The new Rust route tests first exposed the old turn-projection seam passing the conversation's mode
(`build`) rather than its provider account. Three notice/menu scenarios failed with no accepted
sample. Connecting the seam to the model's provider made all three pass. The five scoped pricing
and accepted-reading tests and the targeted turn-projection test also passed. The latest workspace
check was clean; engine-sessions passed 81/81; TypeScript `bun run check` passed.

The seeded side-by-side differential now matches all three route answers: settings GET, stale
settings PUT, and empty limits GET (including the full service order and initial refresh state).
The whole-home inventory still differs. In particular, TypeScript creates its empty usage cache
and Insights SQLite files while idle; Rust's files are demand-driven (the explicit on-demand
index scenario demonstrates that difference). The TypeScript legacy settings file also gains a
setup marker at startup; Rust's local setup-marker migration remains open. Other startup and
random-identity files differ outside this port; no whole-home parity is claimed.

The first WSL test launch used an incorrectly expanded binary path; the corrected invocation
passed all 12. The first native Windows command lost quotes around the area, inadvertently
starting other scenarios; its exact process tree was stopped, all of this run's scratch was
removed, and the correctly scoped retry passed 12/12. Neither is a product failure.
The native build had the pre-existing unused `ANSWERED_WITHIN` warning in `kuvo_work`'s
browser asks module; the Mac debug link warned about the large compact-unwind table, not a
source or workspace-check warning.

Size, non-test characters without leading indentation: TypeScript usage directory 687,546;
new Rust usage/OS/adapter modules 89,206 plus 5,317 added characters in existing Rust files
(94,523 total, about 0.14× the full TS directory). This is a partial port, not the final size ratio.

## Verification of the second port slice

Mac workspace check is clean; Rust usage 18/18, engine-sessions 81/81;
TypeScript usage 5/5 applicable (13 fixture/correction/timing skips) and Bun check clean.
The six-route seeded differential matches all answers including the missing/unknown
reset refusals. Inventory differences remain; in this short startup-only run the Rust
watch catch-up had not yet opened SQLite when the resident stopped, whereas a
long-lived resident has its index ready before the first query (tested). Setup marker,
legacy usage/attribution caches and unrelated startup records still differ.
Cross-platform verification: static musl on Lokum (one capped container) 18/18,
WSL2 Ubuntu 24.04 18/18, native Windows with the gnullvm binary and adjacent
libunwind 18/18. Scratch was removed on all test computers. Both cross-target
build folders were removed. Windows emitted existing target-specific keeper
and work dead-code warnings; the workspace check remained clean.

The first engine-provider fixture revealed a wire spelling mismatch (`integrationID`,
not `integrationId`); corrected and passed. The first reset ask fixture used a
conversation id with an underscore that is excluded by the existing tool contract;
a valid id made the case pass. A stand-in provider connection remained open during
test cleanup once; closing that stand-in's own connections fixed it, and the complete
rerun passed 18/18 in 10.7 seconds. No real provider calls were made.

Cumulative size (same measure as first slice): 151,146 characters in new Rust
modules plus 21,012 added characters in existing Rust files (172,158 total,
0.25× the 687,546-character TypeScript usage directory). The count includes the
adapter and work-ask hooks, excludes tests and the unchanged bundled data.

## Verification of the third port slice

Mac usage 20/20, engine-sessions 81/81, workspace check and Bun check clean.
TypeScript usage 5/5 applicable (15 isolated fixture/correction/timing skips).
All six seeded differential route answers match; inventory gaps remain as above.
The three native JSONL cursor tests, price-book cache test, and six saved-transcript
tests passed. Static musl on Lokum (one capped container), WSL2 Ubuntu 24.04,
and native Windows with libunwind.dll each passed 20/20. Temporary platform
fixtures and binaries were removed from the test computers.

The first changed-projection test queried before the file watcher had folded the
completed turn (the initial checkpoint only held the open turn); the fixture now
waits for the completed mutation revision and token row. The targeted rerun and
the entire 20-case rerun passed. The tests confirm the requested catalog cache
and changed-row cursor rather than only setting internal flags.

Grok's native ACP reader uses its cached-token authentication only when the maker
turns away the first billing request. Cursor's key exchange and plan/credits are
read with its own fixture key. Antigravity's two-host summary merges its named
pools, and a single erroneous full refill cannot replace the 80% accepted reading.
No real maker credentials or API calls were used in tests.

Cumulative size: 209,899 characters in new Rust usage modules and 23,779 added
characters in existing production Rust files (233,678 total, 0.34× the 687,546
TS characters; tests and unchanged catalog data excluded).

## Verification of the fourth port slice

Mac usage 22/22; engine-sessions 81/81; TS usage 6/6 applicable (16 fixture,
correction or timing skips). The same six seeded differential answers match; the
inventory still differs. The peer scenario first reproduced a new regression:
two nearly simultaneous reports meant time-ordering alone could prime the merged
row with the stale 99% reset. Seeding the account's consensus on the first known
reading, before another participant arrives, fixed it. The full Mac rerun passed
both computers' merged ring, new readings and restart. The native Claude case
failed against the previous Rust binary and passes the unchanged TS box; the
Rust-specific append, rewrite and restart checks passed with the watched index.
The 4.8 agent-process guard is included. The shared build has an unrelated
existing dead-code warning (`kuvo_work/src/wakes/fire.rs:129`).

Cumulative size: 266,953 characters in new usage modules and 25,752 added
characters in existing production Rust files (292,705 total, 0.43× the 687,546
TS characters, same measure). Static musl on Lokum (one capped container),
WSL2 Ubuntu 24.04 and native Windows (with libunwind.dll) passed 22/22 each.
All platform scratch and cross-target build folders were removed.
The scoped native-log and accepted-reading tests also passed (three each).

## Verification of the fifth port slice

Mac usage 29/29; TypeScript usage 11/11 applicable, 18 intentional skips.
Static musl on Lokum (one capped container), WSL2 Ubuntu 24.04, and native
Windows cross-built gnullvm with libunwind each 29/29. All platform scratch
and cross-target build folders removed.
Engine-sessions 81/81; workspace check and Bun check pass. The only workspace
warning is the existing unused wake helper. All eight seeded differential route
answers match, including history refresh and Insights summary. Both copies now
have the same fixture computer key; only the actual scan timestamps are normalized
in those two new answer comparisons (their presence and numeric values are checked
in the scenarios). Whole-home inventories differ: Rust's additive SQLite cursor,
price and native-history tables replace the old standalone usage caches; setup
marker and other startup-file differences remain.

The first Mac open-writer SQLite cases reproduced stale history: FSEvents did not
deliver the database/WAL modification while its writer remained open. Closing
the writer confirmed the issue, but the final fixture keeps it open. A bounded
Darwin vnode watch (up to 128 descriptors per SQLite watcher, no extra thread or
process; other platforms retain the existing folder watches) fixes the live
updates. Both engine and Antigravity live mutation assertions pass. The engine
fixture verifies two recent mutation projections decoded rather than the older
call as well. The Go fixture proves a read-only database credential read with
no engine launch, and the database bytes unchanged. All maker answers are
isolated fixtures. The initial differential also caught an alphabetical rather
than source-roster order for empty history; the new service-order scenario
failed on the old order and passes both boxes.

Cumulative size: 340,531 characters in usage/OS/adapter modules and 26,637
added characters in existing production Rust files (367,168 total; 0.53×
the 687,546 TS characters; same measure). Remaining lightness opportunities:
engine session-header lineage is currently selected as compact metadata on each
changed channel (`engine_index.rs`, `read`), even when only one message changed;
Antigravity's writer doesn't expose a mutation clock so a changed database must
read its generation metadata again. Summary still builds its period's compact
events into memory (`insights.rs`, `filter_rows` and `summary`); SQL aggregates
and a reuse cache are open. These are not repeated full source-log reads.

## Verification of the sixth port slice

Mac Rust usage 31/31; TypeScript 13/13 applicable, 18 intentional skips.
Static musl on Lokum (one capped container), WSL2 Ubuntu 24.04 and native
Windows gnullvm with libunwind each passed 31/31. All platform scratch and
cross-target build folders were removed.
Both mixed-version runs of the peer fixture pass (Rust main with TS peer,
and TS main with Rust peer). A fixture race initially left both TS services'
own usage unmerged: a full rerun reproduced it. The scenario now waits for
both usage services to learn the signed computer directory before requesting
a refresh; targeted and full TS reruns passed. A live-delta expectation that
depends on the new cursor acknowledgement is Rust-only, as above.

Engine-sessions 81/81; workspace check and Bun check pass (only the existing
wake helper warning). The seeded differential includes
a real fixture OpenCode Go call in the copied engine SQLite database; all
eight route answers match, including its billed history and normalized
Insight provider/model rankings. Scan timestamps alone are normalized,
as in the fifth slice. Inventory differences remain. The changed-header
fixture and SQL summary cases verify the two requested lightness improvements.
Cumulative size: 395,041 new-module characters and 30,849 added existing
production characters (425,890 total, 0.62× the 687,546 TS characters).
Remaining lightness opportunity: history peer snapshots currently aggregate
the compact 35-day SQLite counts once per changed source batch (not per peer
or heartbeat); a persistent quarter-hour delta index would avoid even that
aggregate on very large histories.

## Verification of the seventh port slice

Mac Rust usage 35/35, followed by the expanded Cursor repricing scenario;
TypeScript 13/13 applicable and 22 intentional skips. The four new Rust cases
failed on the prior binary and pass on the new one. The initial TS engine
ChatGPT fixture did not produce a row because scratch bridges deliberately
return empty engine credentials; that path is tested on the Rust read-only
maker fixture instead. All eight seeded route differential answers match.
Static musl on Lokum (one capped container), WSL2 Ubuntu 24.04, and native
Windows gnullvm with libunwind each passed 35/35, including the expanded
Cursor repricing fixture. All platform scratch and cross target folders were
removed. The shared PC was returned after confirming no test processes remained.
Engine-sessions 81/81; workspace check and Bun check pass, apart from the
existing unused wake helper warning.

Cumulative non-test size: 437,241 new-module characters plus 34,202 added
existing production characters (471,443 total, 0.686× the 687,546 TS characters).

## Verification of the eighth port slice

Mac, static musl Lokum and WSL Rust usage: 40/40; native Windows: 39/39 applicable, one skip (native Windows local Antigravity discovery was still a Rust gap, not a permanent platform exception). TypeScript: 14/14 applicable, 26 Rust-only skips. All remote scratch and cross target folders removed; the shared PC was passed to the next row after checking no test processes remained.
Workspace check and Bun check pass (existing unused wake helper warning); engine-sessions 81/81.
Scoped current-window reading tests: 5/5. The last billed channel removal
regression passed after fixing reconciliation on an empty channel set.
The eight seeded route answers in the standard differential match; the
existing deliberate process/cache/index inventory differences remain.
A second seeded differential containing the checked-in native provider-log
corpus agrees on model names, counts and dates, but exposes a remaining Codex
archive-copy tier mismatch (the live priority reading was not always chosen;
$0.0371 vs $0.0296 on the affected model). The other numeric differences are
floating-point rounding below $0.000000000000001. The archive-priority mismatch
is an open parity gap, not an intentional timing difference.

Cumulative non-test size: 464,513 new-module characters plus 34,471 added
existing production characters (498,984 total, 0.726× the 687,546 TS characters).

## Verification of the ninth port slice

Mac Rust usage 43/43 applicable (one native-Windows fixture skip), TypeScript
15/15 applicable (29 Rust-only/platform fixture skips), engine-sessions 81/81;
workspace check and Bun check pass. The three pair fixtures first waited for a
relay lease before requesting an invitation; main now claims it on demand.
They invite first, as the existing two-computer fixture does, then wait for the
actual lease. The focused rerun and complete Rust/TS reruns passed. The new
flat Codex root initially looked for its sign-in in the parent; the rail and
Helper regressions now both pass after recording the actual credential home.

The eight standard seeded route answers match exactly. The separate native-log
corpus's UTC refresh and Istanbul history answers also have **zero request
differences** with the unchanged exact differential comparator. The former
live/archived tier mismatch and all 24 numeric differences are fixed: price
buckets are divided and added in the bridge's order, positive zero is kept on
empty cost reductions, and `kuvo_wire_sum` performs ordered plain addition
rather than SQLite's compensated `SUM`. The shared price fixture passes at
exact f64 bits (3/3 scoped Rust pricing cases). No comparison tolerance was
introduced. Whole-home inventories still differ for the previously mapped
cache/startup/identity reasons; full engine/peer/Insights numeric matrices
remain to be exercised.

Native Windows discovery now asks the OS for an admitted candidate's command
line with limited-query access and verifies its profile and current listening
PID. A language server may be the child of the ACP process with its explicit
`--gemini-dir`; a different profile is excluded. It does not start a program,
read another process's memory/environment, or send a bearer token to a local
server. The new native fixture uses actual test-owned processes and sockets,
not a simulated `/proc` on Windows. Cross-Mac, Lokum, WSL and native Windows verified.

Cumulative non-test size: 490,289 new-module characters plus 38,482 added
existing production characters (528,771 total, 0.769× the 687,546 TS characters).

## Sign-in admission correction

Work Queue #5053 was observed on TS beta.48 in the field: a Grok weekly
window named an otherwise signed-out computer. Not reproduced in the scratch
TS bridge (provider reads are off). The both-box scenario `old Grok transcripts
are history, never proof of a current sign-in` passes on the scratch TS box:
it indexes a fixture `updates.jsonl` without any credential and requires no live
Grok limit row. A Rust-only regression extends the standalone fixture: after
Grok, Cursor and Antigravity have supplied real fixture plan readings, removing
all three credential files must remove the live rows and the retained readings,
without waiting for another maker request. The old ninth Rust tip failed that
regression (all three stale windows remained on the next GET). After the 4.3
merge, native Grok admission uses the actual Accounts `GrokSecret`,
`grok_secret` and `grok_installed` functions, including the environment key
precedence and installed check; explicit ACP billing uses the matching auth
method. The fixture compares actual Accounts connections with Usage before
and after credential/installation changes.

Credential admission is separate from history: historical native and engine
logs are indexed even after sign-out. Standalone credential file changes and
account-change events withdraw the local admitted row and publish the change to
peers without scheduling a provider read; the file watches use the shared
watcher registry and never create a missing provider folder. A limits GET
reconfirms current local credentials rather than treating yesterday's cache as
sign-in proof. Native Claude also needs the actual OAuth sign-in Accounts checks,
not just a retained account identity record; Codex's native OAuth admission
remains the existing file/Keychain check. Engine plan admission uses the active
credential rows in its own read-only SQLite database, not its last model usage;
removing the credential withdraws its retained plan and a response in flight is
rechecked against current credentials before it can be accepted. No agent is
started for admission, and unchanged checks do not republish identical rows.
An away peer's last reported row is still explicitly last-told evidence, not a
new local sign-in; an old TS peer cannot be made to send an updated account list
by checking files on another computer.

## Additional parity in the tenth slice

- Antigravity keeps a reset-less missing pool for one partial round, persists that
  evidence over restart, and selects both menu rings from the same tightest pool.
- Account windows use the same semantic key as the old box (`limitOf`): name,
  model and duration, otherwise provider ID. Accepted peer/local trails survive
  provider-specific window ID changes; an older reset under another ID is not a
  second window. Old Rust failed the same-account two-ID fixture. The rail,
  card and Helper draw from the same accepted evidence, including grouped cards,
  current-window pace, reset words, and display freshness/time labels. Legacy
  TS window/pace records are imported only after the live maker identity agrees.
- Complete (never loading) Insights summaries have a bounded durable snapshot
  tied to the exact admitted setup/computer scope. A ready query still uses the
  mutation-invalidated SQLite cache; an explicit refresh waits for recount and
  never takes the old-answer path. Compact session headers give bot lineage,
  starter chains, own chats and nested subagents; a manual recount repairs
  retained calls' lineage even if the maker DB stamp did not change, without
  decoding any maker message rows or replacing their counts/origin. The scoped
  old-draft failure (`bot` was null) and fixed test cover this.
- Overview choices, whole-snapshot page shares, known session destinations via
  the existing owner authority, and precise invalid-period/page refusals have
  a both-box fixture. A distinct 201-row engine fixture verifies that the first
  summary list is not truncated at 200. Current local/peer bot looks use the
  directory and the local bot records. Only completed skill tool metadata is
  projected into the index (no tool output or skill file contents); the schema
  upgrade reads compact older projections once, not on a timer.
- Overview's local counted time is the actual incremental observation; a
  connected peer with no changed logs is not marked stale just because that
  observation is old. The peer remains current through authenticated updates,
  with away coverage retaining its last reading. The expanded differential
  before skills correction exposed an actual missing empty `skills` array in
  detail and differing real scan timestamps; no floating-point tolerance or
  comparator change is introduced.

## Verification of the tenth port slice

Mac Rust usage: 53/53 applicable (one native-Windows fixture skip); TypeScript:
20/20 applicable (34 Rust-only/platform skips); engine-sessions: 81/81. Workspace
and Bun checks pass. The shared Grok credential/API-key and skill projection
regressions pass on the rebuilt box. The expanded 13-request seeded differential
matches 12 answers exactly and differs on only `coverage[0].scanned` in the
remaining overview answer (actual observation time, intentionally not normalized).
The unmodified exact comparator reports no remaining skills or numeric differences.
The native-log corpus’s UTC and Istanbul answers also match exactly (2/2).
The full home inventories retain the earlier documented architecture/cache differences.
The three scoped lineage repair/resolver/kept-summary tests pass. Lokum (one capped
static-musl container), WSL and native Windows each pass 53/53 applicable cases;
Windows skips the Unix-only synthetic discovery case and exercises the actual
Windows process/socket fixture instead. All remote scratch and test processes
were removed, as were the cross target directories.

Cumulative non-test size: 549,884 new-module characters plus 40,276 added existing
production characters (590,160 total, 0.858× the 687,546 TS characters).

## Eleventh slice: cached OAuth confirmation and engine plan edges

The previous macOS Claude credential metadata subprocess on each confirmation is
now replaced by a per-Program cache owned by `kuvo_os::keychain::CredentialDate`.
It tracks the actual Keychain search-list order, store file generations and
lock status (metadata only, never store contents or passwords). An unchanged
generation returns the confirmed date; an actual OAuth identity change,
explicit sign-in, sign-out or maker refusal invalidates it. Any other item's
edit to the same store also invalidates conservatively: keeping a confirmed
sign-in after an external deletion is worse than making one extra metadata
query. Unavailable metadata or a transient query error is not cached. The
existing macOS metadata command is used only when this evidence changes or
cannot be confirmed. The scoped test uses a scratch store and lookup stand-in,
including removal and renewal; all scratch accounts bypass actual Keychain
searches at both the caller and OS seam. No tests read, write or search the
user's Keychain, including metadata, or change its search list.

Engine local credential admission now includes the Z.ai alias and excludes
inactive/NULL records. Provider HTTP-date and delay holds use the common
Retry-After conversion and the engine's minimum spacing and persisted
refusal count; an unchanged expired OAuth credential is not sent again.
A renewed credential of the same identity is eligible for a new answer.
After a maker read, the current credential list is rechecked even when the
already-running legacy engine was the authority (never start it to read).
The limits GET also verifies that authority when no local database supplies
credentials, bounded to 250 ms; a timeout is not a sign-out. No unchanged
engine inventory is republished. The old Rust box failed the
Z.ai/null-active fixture and omitted the dated hold's expiry on the wire. It
also sent an unchanged refused OAuth token twice and retained a signed-out
legacy engine's row; both newly corrected route regressions fail on the old
box and pass on the new one. The six focused route cases and the isolated
Keychain-cache test pass on Mac.

### Verification of the eleventh port slice

Mac and Lokum (one capped static-musl container) Rust usage: 59/59 applicable.
TypeScript 20/20 applicable; engine-sessions 81/81; workspace and Bun checks
pass. The exact expanded differential still matches 12/13 route answers,
with the only difference the actual local coverage scan timestamp; the native
UTC/Istanbul corpus matches 2/2. No comparator changes or numeric tolerances.
The cross binaries were built and cross target directories removed. WSL passes
59/59; native Windows passed 58/59 with the peer crossing handoff timing out
again in a focused rerun (not a one-off). The next slice moves remote refresh
fanout after local admission, to remove the duplicate maker read; the combined
tip is held until that Windows regression passes.

Cumulative non-test size: 558,172 new-module characters plus 39,795 added
existing production characters (597,967 total, 0.870× the 687,546 TS characters).

## Twelfth slice: freshness, refresh completion and corrupt evidence

A passive refresh skips a currently confirmed local reading until its spacing
expires, or borrows the same stable account's newest successful reading from
an admitted peer in the current setup. It advertises the local sign-in without
inventing a local checked timestamp, maker reading or notice; old clients'
`accounts` see the borrowed numbers, while `rows` and peer mail retain the
actual source. The marker survives restart and sign-out withdraws the holder.
Older peer mail without the current setup marker cannot suppress a fetch. The
focused two-computer menu fixture fails on the old box (two maker reads) and
passes on the new box. Explicit user refreshes still ask the maker directly
and selected reset/crossing deadlines are not held back by passive spacing;
the complete TS manual-refresh freshness/away/failure/convergence matrix
remains open. Remote refresh fanout moves from request start to completed
local admission; a peer responding to a refresh never fans out a second one.

Refresh completion now considers the whole explicit round's changed maker
check times: one real success means the round succeeds, and an entirely
failed round does not make the last successful completion time look newer.
The old mixed result failed on the prior box; the new mixed/all-failed and
slow-feed cases pass in the initial Mac focused run.

The shared Grok corpus is read through the actual usage routes on both boxes
with fixture compact prices and shifted UTC dates. Complete calls, per-model
event deduplication, caps, billed/fallback prices, unknown models, added
account labels, append and restart are asserted. The fixture's rounded
`0.0008` is tested at the actual wire arithmetic `1000*0.3/1e6+1000*0.5/1e6`
(`0.0007999999999999999`) on both boxes, not a tolerance.

The setup-actor panic from part 6 is fixed separately: a signed replacement
without a removal record uses the TS fallback `Former computer (<first 8
characters>)`. A new signed seed and the moves receipt page both failed on
the old Rust box (connection reset / 409); both Rust and TS pass the focused
cases. The whole-box source sweep also identified a corrupted persisted
negative short-only ring selecting a missing map key (old route reset the
connection). Invalid kept trail bounds/sample order are rejected at load,
invalid local/peer windows cannot become fresh zero readings, and projections
choose only an actual ring. The own code's history slot lookup, lineage map
and rail lookup are optional rather than indexing; peer mail serialization
refuses a bad body rather than panicking. Two other confirmed external missing
map keys were reported to the lead and assigned to this slice for independent
fixes: `kuvo_agents/src/tool_forwarder.rs:136` (cancellation without params)
and `kuvo_work/src/bots/kit.rs:132–133` (incomplete words without here).
Each original scoped test panics on old code and passes on the separate
repair: notifications without a request id are ignored, and incomplete bot
words are unavailable, both as in TS. Keychain stand-in testing never
consults the user's real Keychain.

Usage conformance: Rust 64/64 applicable on Mac, Lokum (musl, capped
container), WSL and native Windows; TS 21/21 applicable on Mac. The
Unix synthetic process fixture and the native Windows process-ownership
fixture skip on their respective other platform. Engine-sessions 81/81,
workspace test check and Bun type check clean. The previous reproducible
native Windows peer-crossing failure is fixed by asking remote holders
only after local admission; its scenario passes on all four platforms.
Cross target directories and remote test scratch were removed, and no
test boxes remained running.
The exact expanded seeded differential matches 12 of 13 answers, differing
only in `coverage[0].scanned` (the real scan time); the native corpus matches
both answers. No numeric comparison tolerance is used. Home inventory
differences remain the deliberate timing/storage differences described above.
Cumulative non-test size: 566,133 new-module characters plus 39,805 added
existing production characters (605,938 total, 0.881× the 687,546 TS
characters; independent repairs in other rows excluded).

## Thirteenth slice: native history corpora and Cursor holds

The shared Claude corpus now exercises the resident's real usage route:
canonical replies, subagent replay, advisor attribution, fast usage, malformed
records and price resolution, with the original test's compact catalogs. The
resident merges the fixture catalog with its bundled one, so the exact Sonnet
price, rather than a fuzzy OpenRouter price, wins on both boxes. Another route
scenario reads a partial appended line only once complete, keeps the counts
through restart, replaces a rewritten shorter file, and limits the display to
the current UTC day and the previous 30. Account removal is exercised via the
Rust route and via the resulting persisted roster on the reference scratch
resident (which does not have its account-mutation bridge running).

The Codex corpus retains its raw JSONL byte forms, moving only timestamps
(including fork/task-start times) into the visible history period. The archive
now falls after the date auto-review became free; all other rates, total and
per-model arithmetic are exact. The prior Rust parser counted a noncompact
record ignored by the reference scanner, adding 19,998 tokens to the fallback
model estimate. Matching the reference's compact `type` markers before
decoding fixes the discrepancy and avoids repeatedly searching whole records
for broad substrings; quota snapshots have a separate match. A native Codex
checkpoint's parser generation invalidates just the old file's derived rows
once, after which its offset resumes normally. The fixture also
checks a saved fallback choice across restart, refusal of an invalid choice,
and clearing the estimate. A three-computer fixture groups one actual fixture
Codex sign-in, adds distinct folders' totals, ignores a duplicate physical
folder and retains another account as its own row.

Cursor's request-based plan, access-token reuse/renewal and a maker's explicit
429 are exercised through its actual routes. A plan's persisted hold/refusal
must also suppress manual refresh and account-wide history after restart (the
old Rust implementation called the maker again). The unchanged credential
uses the retained problem; a replacement sign-in is admitted normally. As in
TS, a hold first met only by history is in-memory until the plan records it.
No actual maker receives any request in these tests.

Both reported Clippy collapsible-if branches in the peer route and the
additional endpoint-lookup branch in the OS seam are flattened without
changing routing or results. A wider Clippy `-D warnings` run stops on
pre-existing lint diagnostics across other modules and Usage's older
implementation; it is not claimed green. The three non-Usage OS diagnostics
were reported to the lead. The final `kuvo-box --no-deps --tests` lint run
reports only the unrelated `serve/library_artifacts.rs:506` argument-count
diagnostic, with no remaining usage-peer warning.

Usage conformance: Rust 69/69 applicable on Mac, Lokum (musl, capped
container), WSL and native Windows; TS 24/24 applicable on Mac. The Unix
synthetic process fixture and native Windows process-ownership fixture skip
on their respective other platforms. Engine-sessions 81/81, workspace test
check, Bun type check, and historical Codex price unit 1/1 pass. The exact
expanded seeded differential matches 12 of 13 answers (only the actual
`coverage[0].scanned` time differs); the native usage corpus matches both
answers. No numeric tolerance was introduced. Cross targets and remote
test scratch removed, no test processes remained.
Cumulative non-test size: 568,146 new-module characters plus 39,805 added
existing production characters (607,951 total, 0.884× the 687,546 TS
characters).

## Fourteenth slice: display, notice visibility and maker edge corpus

The production display component is exercised at a fixed clock for all seven
display matrices: named/time-window/model meters, distinct source computers,
expired sidebar rings versus historical meters, fresh/failed/away intervals,
plain status words, capped credits and extras, and elapsed-time countdown
labels across DST. These checks do not read provider credentials or logs.

A hidden provider/metric is not the same thing as a visible metric whose
alert toggles are off. The old box replayed a crossing when the provider was
shown again (new route fixture failed 1 vs 0). Hidden trails now retain their
actual readings and occurrence counts, but their notice baseline is dormant
until the first visible accepted read. This state survives restart. The
production projection is also exercised with fixed current-window samples
through healthy, close and running-out states, including stale input refusal.
The missing Spark, Copilot premium, OpenRouter key and Z.ai metric aliases
are mapped like TS. The exact secondary Claude card identity still needs
its independently derived card key; the existing hashed plan identity is
not sufficient to reconstruct it.

The shared Cursor plan corpus is read through the actual provider reader and
usage route with loopback fixture responses (7 `mapUsage`, 3 `grokBot`, 3
`mapSummary`, 2 `mapRequestBasedUsage` cases). The route removes the duplicate
Requests line under Total usage, as TS does. Fixture dates are moved into a
current window. The mapper's four fallback-routing and six plan-name pairs,
and the standalone append-missing-on-demand fixture, remain open.

An accepted warning whose receipt was not kept must catch up after restart;
the prior Rust route failed this case. The notice journal keeps the maker's
time rather than the new process time. Sign-ins are confirmed before replay,
only current accepted facts from this computer's pinned forwarder are
eligible, and no provider read is needed. On a later accepted observation or
peer admission the same fact is retried if keeping the history failed;
successes are remembered in a short-lived resident map to avoid repeatedly
opening the journal. A source's notice is visible from the existing merged
notice feed; a nonforwarder does not make a redundant local copy. The
expired/old retained-fact and retry cases are in the same route fixture.

Usage conformance: Rust 73/73 applicable on Mac, Lokum (musl in one capped
container), WSL and native Windows; TS 24/24 applicable on Mac from a fresh
reference build. The Unix synthetic process fixture and native Windows
process-ownership fixture skip on their respective other platforms. Display
component 7/7, accepted-reading component 8/8, engine-sessions 81/81,
workspace test check and Bun check clean. The exact expanded seeded
differential matches 12/13 answers (only the actual `coverage[0].scanned`
time differs), and the native corpus both answers. All scratch and cross
target directories were removed; no test processes remained.
Cumulative non-test size: 570,538 new-module characters plus 40,355 added
existing production characters (610,893 total, 0.889× the 687,546 TS
characters).

## Fifteenth slice: maker fixture parity and retained history reconciliation

The shared Antigravity legacy/name, summary, generation and blob-bound corpora
exercise the production normalizers and bounded decoder. The route fixture
also covers absent tokens with no running profile, rejected local CSRF followed
by cloud fallback, restart without resending that CSRF, and an authless local
403 which must not permanently refuse the profile. The engine plan fixture
checks exact account identities (including case-insensitive Copilot login),
a changed GitHub account, and recovery from an API-key refusal that is not an
expired OAuth sign-in.

Ordinary accepted turns still update the in-memory limits, rings and notice
projection on each observation. They need not rewrite the retained limits
journal within 30 seconds; new account/windows, visibility baseline changes
and new warning facts persist immediately. The focused actual turn scenario
failed on the older resident and passes on the new one. The common timed
sample does not clone the whole persisted state.

A complete native-store inventory now reconciles deleted files after lost
watch events or a restart. A truncated or inaccessible directory listing
cannot establish absence, so it retains known files; broken databases remain
in the inventory and do not evict their last good records. Only file names
and metadata are enumerated in explicit refresh/startup/recovery, with
unchanged transcript bodies still skipped. The both-box Antigravity store
fixture exercises a live WAL, legacy step timing, malformed/oversized stores,
Unix symlink deduplication, repair, last-good rows of a temporarily unreadable
known database, and deletion during operation and while stopped. TS passes;
the previous Rust resident repeatedly retained 255 tokens when the true total
was 145. The corrected route passes on Mac.

Verification: Rust usage 75/75 applicable on Mac, Lokum (static musl in one
capped container), WSL and native Windows; TS 25/25 applicable on Mac. Engine
sessions 81/81, the four scoped shared fixture tests, workspace test check
and Bun check. The exact expanded seeded differential has only the actual
`coverage[0].scanned` time difference (12/13); both native corpus answers
match exactly. All remote scratch directories, copied Node archive and cross
targets removed; no test processes remain.
Cumulative non-test size: 573,825 new-module characters plus 40,355 added
existing production characters (614,180 total, 0.893× the 687,546 TS characters).

## Sixteenth slice: nullable Insights buckets, shared settings and summary edges

A portable usage observation without one cache bucket still contains real known
input/output counts. The reference retains each safe nonnegative integral
number (including numeric strings), using null for the unknown buckets. Its
summary keeps those known tokens but does not estimate the cost of an incomplete
call. Previously the Rust portable index dropped all its buckets when any was
absent. Shared fixtures prove exact 37 known/46 processed tokens and 9/40 cache
rate, and a five-transcript 12 MiB backfill with 50 known tokens (Cursor's
context size is not API usage). The old Rust results were null and zero
respectively, TS passed, and the correction uses the existing nullable peer
buckets and total helper for both native engine and portable projections. A
one-time portable and engine projection checkpoint revision repairs old retained
incomplete records; subsequent unchanged restarts leave checkpoints identical.
Explicit seeded old projection regressions verify both repairs. The engine
fixture also keeps its reported processed total separate from known buckets.
An incomplete bucket is null, with the indexed aggregate marked partial; the
record's explicit `partial` flag is reserved for a missing token object, as
TS does. The mixed partial/complete fixture verifies exact model, agent and
provider breakdown order using the reference SQL alias ordering. The old
Rust resident failed those order and per-call detail assertions. The exact
new seeded differential matches 4/5 usage answers, with only the real coverage
scan time in the fifth; session-list preflight establishes the same owner
and only its unrelated generated revision differs. No floating tolerance.

The current TS and Rust wire contract in a checkpointed setup is a shared
authority: writes to a member are refused with the same 409 response. The new
three-computer fixture checks part-by-part convergence, refused member imports
and writes, return after being away, stale client versions and another restart.
Legacy direct-settings mail remains a separate compatibility gap because those
older TS unit cases deliberately do not install the shared authority.

Further exact scoped mapper and accepted-window matrices cover Argon ambiguity,
legacy exclusions, exact 10% threshold, direct red projection, window jitter
and rollover, missing readings, and level-only windows. Fixed civil-day bounds
cover both DST transitions and fractional offsets. The shared real routes
check viewer-zone grouping, per-level cache rates and unknown rate, and exact
three distinct standout reasons (or none when not enough candidates).

Verification: Rust usage 85/85 applicable on Mac, Lokum (static musl in one
capped container), WSL and native Windows; TS 34/34 applicable on Mac.
One fixture-only `database is locked` reading a checkpoint during an earlier
Mac restart was seen once; a 5s SQLite busy timeout was added to the direct
fixture, the immediate focused rerun and subsequent full gates pass. Engine
sessions 81/81, workspace test check and Bun check, exact expanded seeded
differential 12/13 (only real scan timestamp) and native corpus 2/2. Scoped
decoder/mapper 7/7, accepted-window 12/12 and civil bounds 1/1. Both remote
scratch folders and copied Node archive, Lokum scratch and cross targets
removed; all test processes exited.
Cumulative non-test size: 574,550 new-module characters plus 40,355 added
existing production characters (614,905 total, 0.894× the TS baseline).

## Remaining implementation gaps

- Direct provider limits: all requested provider families now have a local reader fixture. Remaining provider gaps are Antigravity credential/profile races (running-profile fallback, missing token, CSRF and bearer/refresh refusal persistence and independent host holds now have scenarios), Cursor full fallback/hold parity, full engine-reader edge cases, manual refresh freshness/convergence, and full peer refresh/away/notice-state edge cases. Passive duplicate fetch suppression now has a two-computer fixture.
- Reset spending: local Codex credit and Claude grant confirmation, replay journal, local Always Allow and the work-ask path are implemented and exercised. Peer forwarding and on/off decisions now have a two-computer scenario, including a caller restart and replay of the holder’s original request. Engine ChatGPT reset inventory/spending, overlapping confirmations and prolonged unsettled-spend settlement now have focused scenarios. Revoked-member answers, prolonged remote settlement variants and full decision convergence remain open.
- Legacy reading/pace import, partial windows, semantic account windows, cards/rail/display and notice visibility are implemented with focused cases; remaining mixed-version multi-computer account identity, extended notice-state handoff/forwarder election, full card/display edge cases and delivery parity.
- Native Claude/Codex/Grok JSONL parsers and offset checkpoints now fold into the production SQLite index and attach managed calls to portable steps; only the Claude boundary scenario is verified so far. Engine and Antigravity native history, hosted billed usage and pricing feeds/repricing, quarter-hour history and fallback estimates now have route scenarios. Engine ChatGPT and Cursor account-wide history now have native route scenarios; remaining Antigravity busy-store retention and cutoff cases, full history-peer delta/size/freshness/identity tests, historical Insight replay and reset/cursor edge cases, full historical lineage replay, remote bot appearance and remaining summary/detail edge states remain open. Bot lineage, kept summaries, page shares, choices and ownership destinations now have boundary cases; the overview/summary is not advertised as complete.
- Shared-settings legacy peer mail/version interop and setup migration cases beyond the existing authority's shared save path.

## TypeScript case inventory

### `box/test/usage/engine-limits.test.ts`

- L50: each sign-in in Settings › Accounts is a row of its own, read with that sign-in — partial — engine credential plan scenario (individual integration rows; full added-account cases open).
- L91: an OpenRouter key's spend today, this week and this month (UTC's), each starting over when OpenRouter's does; what was bought, for what's left of it — covered by the UTC spend-period route fixture (exact balance and absent extras).
- L108: a Copilot seat its organization pays for by the credit: the organization's credits and spend this month, where the sign-in may read its billing; nothing, never a zero, where it may not — covered by the Copilot organization route case, including transient failure, filtered fee products and no admin rights.
- L139: GitHub holding requests off (a 403 with retry-after) stops the look through your organizations, and the next read looks again — covered by the Copilot organization case (only first organization asked while held).
- L152: ChatGPT's credits and reset credits, read with the engine's sign-in — covered by engine credential plan scenario plus usable credit inventory read and confirmed spending with the saved OAuth identity.
- L161: an API key with no plan isn't listed; a lapsed sign-in says so without asking its maker — partial — read-only engine database alias/active/no-plan/expired fixture (OpenCode Go and OpenAI API key); specific Z.ai no-plan response still open.
- L173: SuperGrok's weekly pool, as the Grok CLI reads it — partial — engine credential plan scenario (xAI weekly reading, not the standalone CLI).
- L180: a SuperGrok plan that ended is listed as ended, with nothing of its last week and no ring — covered by the active/malformed/ended SuperGrok route fixture.
- L196: an xAI answer in a shape Kuvo doesn't know is a read that failed, never a plan that ended — covered by the malformed SuperGrok route case (last accepted ring remains).
- L202: OpenCode Go turning a key away for having no plan lists nothing, rather than asking to sign in again — covered by the read-only engine database alias/no-plan fixture.
- L208: a box starting before its engine answers keeps the engine's sign-ins' holds, and asks nothing early — partial — dated maker hold kept across restart (engine process absent; actual local credential DB); exact unavailable legacy engine startup case open.
- L241: an API key turned away is tried again later, not held to that key as a lapsed sign-in — covered by extended engine plan route with rejected OpenRouter key and successful retry.
- L254: each sign-in says who it is, the same on every computer: a key by the key, Copilot by GitHub's login, SuperGrok by its user — covered by exact domain-separated identity assertions in extended engine plan route (cross-computer grouping tested separately).
- L296: signed in to Copilot as another GitHub account, what was kept of the one before isn't shown as this one's — covered by extended engine plan route (same login with changed case and credential, then different login).
- L320: an account's mark in Accounts is the service its limits are read as — open.
- L326: OpenCode Go's 30-day window shows a forecast after frequent refreshes — open.

### `box/test/usage/insights-checkpoints.test.ts`

- L11: Insights checkpoints local history and counts only changed transcripts after restart — partial — startup/file-change portable index, compact mutation cursor, native Claude append/rewrite/restart scenario; exact all-source case remains open.
- L41: Insights adds engine updates without replaying the older calls in the same database — covered by changed native engine channel scenario with decoded-row count and retained checkpoint.
- L74: learning the box's peer name leaves its unchanged local history checkpoint in place — open — related portable checkpoint scenario above; this exact case not yet ported.
- L111: a large backfill keeps all usage as it releases batches and stays finished after a restart — covered by both-box five-transcript 12 MiB backfill, exact counts and unchanged restart checkpoints; Rust uses a blocking index worker rather than releasing separate processes.

### `box/test/usage/insights-kept.test.ts`

- L26: a period asked again is answered at once with the last summary, marked as of when, while it's read afresh — covered by the scoped complete-summary/restart/gate test (explicit refresh bypasses kept answers).
- L62: a summary that says the box is still reading its usage is never kept — partial — only complete summaries are persisted; scoped restart and changed setup refusal, exact repeated loading fixture open.

### `box/test/usage/insights-refresh.test.ts`

- L36: a refresh counts again before it answers, and the summary says when the box last counted — partial — explicit route recount and scoped retained-header repair; explicit refresh while catch-up is busy remains open.

### `box/test/usage/insights-summary.test.ts`

- L58: a bot's row is its own chat and every session it oversees, subagents and older starts included — covered by both-box bot lineage route scenario and scoped resolver.
- L123: Bots includes all unowned sessions after the old ranked-list limit — covered by 201-session native SQLite scenario on both boxes.
- L133: a refresh attributes retained calls through engine sessions without changing their usage or origin — covered by scoped real SQLite retained-call repair test (zero decoded maker rows).
- L188: each model has one name however its id was written, and narrows by it — covered by named model aliases and filter scenario.
- L215: the cache rate is cached input over all input, at every level — covered by both-box ranked-level fixture, exact ratios for providers, models, computers and sessions and null unknown rate.
- L234: the change is against the period of the same length just before — covered by named summary fixture with a prior-period call.
- L253: at most three sessions stand out, each for one reason, once there are enough — covered by both-box cost, low-cache and three-subagent route fixture, including insufficient candidates filtered by model.
- L267: Insights waits for a new computer’s first complete copy and counts kept usage after it goes away — partial — connected peer coverage and retained-away restart scenario; exact first-page loading states open.

### `box/test/usage/insights.test.ts`

- L49: an Insights worker exit rejects requests and the bridge reopens the persisted index — deliberate in-process design replacement; the restart/catch-up and kept summary cases exercise reopening the retained index without a separate worker process.
- L85: closing the Insights index is idempotent and later requests reject — deliberate in-process design replacement; connection ownership is scoped to each blocking operation, resident shutdown closes handles and restart reopens the index. The actual WAL/backfill restart cases test retained data.
- L93: copies merge before filters, resumed sessions count once, and moved bot calls retain their origin — open.
- L114: a peer account observation enriches the matching local copy and keeps its counts — open.
- L132: a newly available provider call replaces its saved placeholder across filters — open.
- L142: day bounds use the viewer's time zone, including DST transitions — covered by fixed-clock production bounds for spring/fall DST and Kathmandu, plus both-box exact daily totals crossing UTC midnight.
- L154: a cold yearly query and repricing both leave the bridge event loop responsive — open.
- L186: a session page computes shares of the whole filtered snapshot after a scan — covered by both-box 51-session two-page scenario, plus detail and exact invalid-page refusals.
- L207: resume peer pages, reject vouched computers, and price incoming records locally — partial — peer live cursor/delta, mixed version and unadmitted-origin filter; per-peer pricing fixture and multiple-page malformed-record matrix open.
- L231: rewind an in-range cursor, then reset again when the serving peer learns the retired origin — open.
- L281: historical replay converges on complete counts and rejects a conflicting unrelated origin — open.
- L312: isolated Claude, Codex, Grok and Antigravity records join Kuvo steps, and the Kuvo engine uses its SQLite reader — partial — managed native Claude and AG supply portable counts, read-only engine SQLite live cursor; native Codex/Grok integration exact fixture open.
- L423: legacy history stays on its provider slot through sign-in changes, while observed accounts stay pinned — open.
- L458: opening an unversioned Insights database stamps it without losing records — open.

### `box/test/usage/limit-card.test.ts`

- L67: a 5-hour window used at a steady pace says when it runs out, ahead of its reset, and the ring gets a warning dot — covered by accepted pace/card route, exact words and ETA.
- L79: a window used slowly lasts until its reset, and says so with no dot — covered by the slow account in the accepted pace/card route.
- L87: with too few readings nothing is predicted: what's left and when it resets, no more — open — related accepted-window and notice tests above; this exact case not yet ported.
- L95: what the readings said is kept through a restart of the box — covered by accepted pace/card route with exact before/after card and rail.
- L103: each card shows only its own account, named among its agent's others — covered by named three-account route.
- L124: a reset is offered only where one can be used, naming the soonest to expire — partial — real Codex reset card words and action, inventory/confirm fixture; no-credit card edge case open.
- L139: an agent's many accounts share one ring, its card a strip of them emptiest first, fine ones counted — covered by three-account accepted pace/card/rail route.
- L158: each coding agent's last numbers stay on its card, as of when, once it can't be read any more — open.
- L179: an account with no numbers read yet has no ring, rather than an empty one — covered by legacy cache account-switch scenario.
- L208: a weekly window read every minute still shows a forecast after eight hours — open.
- L213: a busy weekly window keeps its forecast through a box restart — open.
- L219: Codex keeps a weekly forecast when its reported reset time moves by a few minutes — open — related accepted-window and notice tests above; this exact case not yet ported.
- L233: a downward correction within the same week keeps the forecast — open.
- L244: a new quota period starts its own forecast after the last reset — open — related accepted-window and notice tests above; this exact case not yet ported.
- L262: Antigravity's unused pools can forecast while Google keeps moving their reset times — open — related accepted-window and notice tests above; this exact case not yet ported.

### `box/test/usage/limit-display.test.ts`

- L24: Usage names windows, supplies their used percentages and lists a shared source once — covered by fixed-clock production display matrix and accepted pace/card route.
- L44: Usage dates each merged meter by its own reading and names the computer that supplied it — covered by distinct accepted source-meter component test; existing peer route exercises accepted merge.
- L56: Usage marks an overdue reset and leaves the historical value intact independently of the sidebar — covered by same-entry meter/sidebar projection test.
- L65: Usage ages readings by their own time and marks away or failed sources old immediately — covered by fixed-clock 29/30/31-minute and away/failed member matrix.
- L77: Usage gives plain next steps instead of exposing the provider’s error — covered by exact status word matrix including no/ended plan and private error.
- L93: Usage draws capped credit balances and extras even without normal windows — covered by credit/extras meter and sidebar level component test.
- L113: Reset labels round up across hour and day boundaries, by elapsed time across DST — covered by fixed-clock exact label and DST matrix.
- L142: Limits include the check time for an ended plan without meters — partial — display component preserves `updatedAt` on ended plan; actual global completion field case open.
- L154: A delayed provider leaves refresh pending and sends an update when the new snapshot is ready — covered by slow maker/limits events route fixture.
- L189: A failed provider refresh keeps the prior successful completion time — covered by mixed/wholly failed refresh route fixture.

### `box/test/usage/limits-peers.test.ts`

- L233: an account signed in on three computers is read from its maker once each round, by one of them, and every one lists it once with its newest reading — partial — two-computer merged account with stale reset rejection and restart; one maker fetch on two computers verified with passive menu borrow/restart/sign-out; three-computer timed convergence open.
- L285: computers that read at the same moment settle on one reader by the next round — open.
- L301: a refresh asked of one computer reads there only what's due, and asks the computer an account is on for one it doesn't have — partial — fanout after local sign-in admission; complete explicitly due/remote-only matrix open.
- L324: a computer that's off keeps its last reading, listed as of when it was read and away, through a restart; what it read is read elsewhere once it's due — open.
- L356: a maker that turns one computer away holds that one only: another reads the account, and a read that failed never stands in for one — open.
- L388: a computer signed in to another account since, whose sign-in doesn't say who it is (Copilot's), reads it rather than taking a peer's reading of the one before — open.
- L440: Refresh on a Mac, whose Claude only its turns tell of, asks the computer that reads that account — open.
- L460: an agent's plan_limits says the numbers its computer's accounts were read with anywhere, and the accounts on your other computers, where — covered by merged remote account/holder tool wording in the peer reset scenario.
- L484: a box whose group chats didn't start takes plan limits, and says not now to group chat mail, so what mustn't be lost comes again — open.
- L495: the link saying who your computers are again tells no one again, and an ask is answered once a minute at most — open.
- L513: a tell lost while a computer's link was off is made up once that link carries mail again — open.
- L535: an account of a service a newer box knows and this one doesn't is listed by its name — open.
- L547: what a peer's account has beyond its windows (′extras′) is kept as told, and only what one is — open.
- L579: a computer you removed goes from the list, with its accounts — open.
- L588: a peer whose clock is an hour ahead holds this box back no longer than one whose clock is right — open.
- L604: what goes between them is the readings, never a sign-in or who the account is at its maker — open.
- L624: what your own machine last told is kept whole through a restart: shown once the link confirms the computer is a peer — partial — two-computer peer reading survives restart, exact old wire/cache fixture still open.
- L653: a weekly forecast combines both computers' readings and survives a fallback to an older reading — open.
- L688: a box still shows the weekly forecast it learned from a peer after it restarts — open.
- L710: usage names a reachable computer while its first limits snapshot is in flight — open.

### `box/test/usage/limits-resets.test.ts`

- L223: the account's row says what it has; asked, the box offers one with a confirmation; confirmed, one is spent, and asked again with it, nothing more is — partial — local Codex reset replay conformance; additional exact TS cases open.
- L265: the box stops after the spend is written down and before its request goes: started again, the same confirmation goes as the same request, and one reset is spent — open.
- L293: the box stops after its maker spent it and before the answer came: started again, its maker says it's done, and nothing more is spent — partial — Codex fixture reports unknown, resident restarts, then maker replies already redeemed under the same request.
- L319: no answer came (its maker failed after spending it): the box says it may have been used, and Retry, with a new confirmation, goes as the same request and uses nothing more — open.
- L341: its maker turns the chosen credit away (expired since): nothing was used, it says so, and the next ask isn't held to that credit — open.
- L358: an account with no resets left, or none at all, is refused before anything is offered — partial — missing and unknown account refusal conformance; zero-inventory test remains open.
- L474: alone, a Mac's Claude row has its windows and extra usage from Claude Code, says its resets show only from Linux, and asking for one says why — open.
- L493: with a machine signed in to the same account, the Mac's row has its resets, and a reset asked on the Mac is used there, once, and the Mac answers when it is — covered by two-computer Claude grant forwarding fixture on isolated file-owned sign-ins.
- L536: Always Allow, set from the Mac, is kept with the account on the machine that spends its resets, and every box says so — covered by two-computer grant forwarding and shared decisions fixture.
- L583: its maker couldn't say a reset went through: Retry, with a new confirmation, goes as the same request, so a second isn't used — open.
- L604: two confirmations at once (two apps): one request goes, and both hear it went — covered by concurrent engine ChatGPT credit fixture.
- L619: Always Allow is the account's: signed in to another account in the same place, agents ask again, and coming back doesn't bring it back — open.
- L643: Always Allow set on one of your computers with the sign-in is kept on the other, and turned off on either, it's off on both — partial — remote caller enables, holder disables, and both observe the decision; both holders signed in variant open.
- L668: the answer from the computer with the sign-in is lost: Retry, with a new confirmation, is asked there under the same id, and nothing more is used — partial — remote unknown and caller restart replay same confirmation; new confirmation and lost relay answer variants open.
- L714: Always Allow, and another account signed in between the offer and the spend: nothing is spent from it, and the allowance goes — partial — stale Codex offer refused and separate local Always Allow on/off case.
- L733: Always Allow on, then off, while the computer with the sign-in is away: the older on, told last, doesn't undo the off — open.
- L760: an off for an account this box doesn't know yet is kept: once it does, the account isn't allowed, and an older on doesn't change that — open.
- L776: a reset still open, asked again while its maker holds requests off: refused for now, and still open, never 'nothing was used' — open.
- L794: a new reset asked well after one that never heard back: that one is settled first from what its maker says, and this one is its own — covered by aged Codex journal and distinct late-credit fixture.
- L831: no answer came from the computer with the sign-in, then it can't read the account (Claude Code signed out there): the Mac's spend stays open, never 'nothing was used', and Retry goes again — open.
- L851: a peer's ask more than 10 minutes after an open spend that went through: that one is settled first, and this one is a reset of its own — open.
- L871: hetzner's clock a minute ahead: Always Allow on there, then off from the Mac within that minute, is off on both — open.
- L888: an agent's reset under Always Allow, with it turned off on the computer with the sign-in: refused there, nothing used — partial — local off decision rejects an unasked confirmation; peer behavior open.
- L904: two confirmations at once, well after one that never heard back: that one settled first, then one fresh request for both — open.
- L919: the computer with the sign-in can't be reached while a spend is open: it stays open and says an earlier try may have gone, never 'nothing was used' — open.
- L937: Claude on Linux: the token is another account's than Claude Code's record says: nothing is spent from it — covered by Claude grant profile mismatch fixture.
- L976: an agent's reset under Always Allow got no answer, then Always Allow is turned off: the user confirming it is theirs, and it goes through as the same request — open.
- L998: Claude on Linux whose record of the sign-in leaves out its organization: its token's account matches by the account itself, and a reset goes — open.
- L1017: signed in to another account in a slot: Always Allow for the one before goes off here, and on your other computers too — open.
- L1044: Claude's token is a newly signed-in account while its record still says the one before: nothing is spent, and the new account's Always Allow stays — open.
- L1070: an agent's reset that never heard back, Always Allow turned off since, and the user's Use a quarter of an hour on: it goes the first time, the agent's settled first — open.
- L1108: signed in to another account while Anthropic is asked who the sign-in is: the reset is spent with the sign-in that was checked, never the new one — open.
- L1132: Claude's record and its token's account: one account only where the accounts match, and the organizations where both say one — open.
- L1144: a decision a peer told stays while the link only leaves that peer out for a moment — open.
- L1165: a spend a box began for a Mac that left the setup is answered as it went, never 'nothing was used' for one that went — open.
- L1196: before its link has said who your computers are (just started), a box uses no one's resets without asking: its agents are told to ask, and nothing is spent — open.
- L1218: the answer to a reset a Mac passed to its machine still comes through after the machine leaves the peer list: it says how it went, never 'may have been used' — open.

### `box/test/usage/limits-sign-ins.test.ts`

- L113: signed out of Cursor in Accounts, its ring is gone at once, and what was read of it with it — partial — standalone fixture removes real credential file, watcher withdraws retained rows before a client reads; Accounts action route variant open.
- L143: Accounts' word on Cursor is the rings': its own key turned down there, no ring, though the key still reads; the Cursor app's sign-in is never read — open.
- L165: a Mac's Claude Code signed in has its ring before any turn told its limits, as Claude Code says them; signed out, none — covered by native usage-control fixture and OAuth credential removal; explicit-only process start is deliberate work timing.
- L186: a newly signed-in Mac Claude publishes its first reading before the next limits round — open.
- L209: what was kept from before is never shown before the agents have said who's signed in, and one signed out since goes from the file — partial — identity-gated legacy upgrade and credential-removal cache withdrawal fixtures.
- L229: each ring has its own provider's windows only: Cursor's own Grok Bot week stays Cursor's, and Grok's row has Grok's — covered by standalone maker fixture.
- L244: a sign-in check that fails (offline) isn't a sign-out: the ring and its readings stay — open.
- L256: an agent that never says how its sign-in stands holds up no reading: the others are read, and it's as it was — open.
- L266: started again, the box lists what it last read of an account still signed in here until its agent says, so the ring doesn't blink out; one signed out meanwhile isn't — partial — legacy upgrade and named card restart, stale identity refused; provider probe timeout variants open.

### `box/test/usage/limits.test.ts`

- L157: an older Claude Code's event, with only the window that limits it, keeps that one — open.
- L165: Claude Code's warning of a limit says how much of which one is used, and close only from 90% (#257) — open.
- L200: each login is its own row with what Claude Code says of it (its windows, plan and extra usage), its resets say they show only from Linux, and Anthropic is never asked with its sign-in — open — related accepted-window and notice tests above; this exact case not yet ported.
- L248: its extra usage, and its reset grants read at most hourly: a failed read of them keeps what was read, never none — partial — Claude grant fixture proves no repeat inventory read on confirmation; failed hourly read and extra usage timing remain open.
- L294: each login is its own row, asked of Claude Code in its own folder, one a minute — open.
- L318: an added account's usage is asked of Claude Code with that account's folder, and never with a token — open.
- L339: a read under way is kept before it's answered: a restart meanwhile doesn't ask again — open.
- L356: a refresh right after a read doesn't read another Claude account within the minute — open.
- L370: a restart while one Claude account is being read doesn't read another in the same minute — open.
- L387: Claude Code not signed in: its row says to sign in again, it isn't started again while that's so, and signed in again, it's asked again — open.
- L409: what a turn hears keeps a window current between reads, even while a read is under way — open.
- L432: the first account is Claude Code's own sign-in, in ~/.claude, the one Terminal uses — open.
- L483: its plan's windows and a model's own are what Codex says for its sign-in — open.
- L499: Codex limits are read again on schedule, each time from Codex — open.
- L510: a configured API key never starts Codex or reads ChatGPT plan limits — open.
- L521: an API key has no plan limits: not listed — open.
- L547: one Claude login on a Mac and a Linux computer is one identity, another login another, and neither says who — open.
- L573: signed in to another account since, what was kept of the one before isn't shown as this one's — open.
- L597: Codex's ChatGPT sign-in is its account and workspace member, the same as the engine's sign-in to it — open — related accepted-window and notice tests above; this exact case not yet ported.
- L611: a switched Codex account is read with its own sign-in — open.
- L631: says each account's windows, how much is used, when each resets and as of when — open — related accepted-window and notice tests above; this exact case not yet ported.
- L666: with the coding agents not running, says so as an error — open.
- L677: Claude's rows say its two rings, the session and the week, each with how close it is, on the route and its event — open.
- L704: a plan with one window has one ring, and one with none has none — open.
- L715: a row says the account's level: its rings' higher one, a balance's or a capped extra's, and none where nothing has a limit — open.
- L736: the box picks which of a maker's windows is each ring — open.
- L750: Antigravity's rings are one pool's, as its reader lists them: Gemini's session is never paired with Claude's week — open.
- L771: every provider's rows and windows have a name — open.
- L796: two rows that would have one name are told apart, as seen from the box that answers — open.
- L813: one bar shows the fuller ring, the session where they're as full, and a ring that started over counts as empty — open.
- L823: a window whose reset has passed has started over: empty, calm, and without the old period's reset or amount — open — related accepted-window and notice tests above; this exact case not yet ported.
- L838: on the route and its event: named accounts and rows, and clients told when a window starts over — open.
- L872: a box that reads arms a timer for the next window to start over, and stopping clears it — open.
- L894: the agents' plan_limits tool says a window that started over isn't read since — open.
- L905: a plan that ended is one, whatever another computer kept of it — open.
- L915: a newly added provider is read immediately before its first scheduled tick — open.

### `box/test/usage/native-usage.test.ts`

- L44: each Codex account's plan is what Codex itself says in that account's own folder: windows, plan, credits and resets — partial — Codex native refresh and reset inventory scenarios; added slot and Spark exact case open.
- L71: a Codex account signed out in its folder shows nothing to read, and Codex isn't started for it — open.
- L81: Codex publishes its limits immediately after sign-in and reports the first later threshold crossing — open.
- L127: a Codex reset credit is spent through Codex: the soonest to expire, once, as the journal's request, even confirmed twice — covered by the local Codex reset replay scenario (including earliest expiry and duplicate confirmation).
- L152: Grok's plan is what Grok itself says in the account's folder, signing in with the sign-in it keeps there — covered by standalone Grok ACP cached-token fixture.

### `box/test/usage/service.test.ts`

- L46: the limits route reads and renders all six plans from their fixture credentials and APIs — partial — native Claude/Codex/Grok/Cursor/Antigravity and engine fixtures; exact all-six contract/display case open.
- L148: an open bridge feed learns of an external sign-in switch and a removed CLI with credentials left behind — open.
- L230: the bridge retries its latest window after the services process restarts — open.
- L267: a provider removed from the admitted accounts during a round is not read — open.
- L285: a Codex login stored only in Keychain is read with the exact account for its home — open.
- L302: Codex auto storage joins pending keyring lookups and prefers the keyring credential — open.
- L326: a Codex reset goes only while Codex says it's signed in to the row's account, and through Codex itself — partial — Codex reset replay scenario refuses changed sign-in; full maker/file divergence scenario open.

### `box/test/usage/usage-antigravity-history.test.ts`

- L37: shared blob fixtures decode exactly as Swift, including fallback time and malformed messages — covered by 16-case scoped Rust decoder corpus (normalized current model naming).
- L42: read-only SQLite batches and pricing match Swift's shared expected days — partial — Antigravity metadata scenario with fixture SQLite; shared expected-day matrix open.
- L61: WAL-only changes invalidate a cached scan, and a later cutoff prunes it — partial — both-box multi-store fixture includes live WAL-only writes; cutoff-specific case open.
- L72: legacy databases without steps, then gaining metadata, are re-read — covered by multi-store history scenario on both boxes.
- L84: oversized blobs are skipped, and an unreadable new database is logged once and left out while the others count — partial — both-box multi-store fixture checks oversized row, broken store and repair; diagnostic count open.
- L116: ACP databases count, and empty stores and stores without generation metadata are skipped quietly — covered by both-box multi-store history scenario (unrelated empty table).
- L143: a conversations folder that can't be listed is logged once and left out while the others count — open.
- L158: missing stores are empty and symlinked stores count only once — covered by both-box multi-store history scenario, symlink variant on Unix.
- L170: own profiles alone, machine-hashed places, and an unreadable new database leaves the rest of the account counted — open.
- L202: deleted databases leave the total after restart and do not block the remaining stores — covered by both-box multi-store history scenario.
- L226: per-database cache reads only changed stores, ages out idle ones, counts changing stores' fresh reads and keeps unreadable stores' last events — partial — both-box multi-store fixture proves live WAL changes and retaining last good rows when temporarily unreadable; idle aging and decoded-store counts open.
- L266: blob bounds match Swift: skip oversized generations and absent step timing, retain embedded timing and the inclusive cap — covered by shared bounded-row corpus, exact count and processed tokens.
- L284: shared missing/empty profile fixtures contain no history, as Swift — open.
- L295: an Antigravity database that can't be read never holds up forgetting the other services' deleted logs — open.
- L326: a new database written to while it's read counts what was read, is read again next round, and never saves $0 — open.
- L367: a busy database read before still moves every round though it's written to during every read, and isn't logged — open.

### `box/test/usage/usage-antigravity-plan.test.ts`

- L24: shared legacy and plan-name parity fixtures — partial — scoped Rust checks all legacy and name pairs; OAuth defaults assertion remains open.
- L29: shared Swift parity fixtures — covered by all seven shared summary cases through the production normalizer.
- L32: exact pools, first usable wins, clamp and Swift rounding — covered by new exact mapper input and shared corpus.
- L38: all three pools retain independent windows in client rows — open.
- L48: Argon is separate and unambiguous — covered by exact production mapper cases for unique enabled weekly pool, duplicate, disabled/invalid flag and ambiguous standard bucket.
- L54: legacy worst pool, blacklist, missing fraction and Argon exclusions — covered by exact production mapper case.
- L60: the usage account reads its running profile's local quotas when cloud endpoints fail — partial — local-only profile fixture; cloud-failure fallback combination not separately exercised.
- L115: Linux /proc maps only own profile and owned listening socket inodes — covered by synthetic process/socket and local TLS route fixture.
- L133: Mac own profile can contain spaces and regex punctuation — open.
- L140: Mac agy stand-in permits no CSRF and language_server still requires it — open.
- L148: Linux agy own-profile stand-in permits no CSRF — open.
- L158: Mac ps/lsof stand-ins require own profile — open.
- L168: with no token file, its quotas come from its own running server, and no credential is sent anywhere — covered by local-only profile route fixture.
- L193: with no token file and it isn't running, it says its usage shows while it runs, and asks no one — covered by extended owned loopback route fixture.
- L211: the overview shows heavy use of the tightest available pool without mixing pools — open.
- L256: one host refusing and optional plan refusing cannot poison usable quotas — open.
- L278: legacy project-plan 403 still permits quota retrieval — open.
- L292: agent OAuth metadata and verified defaults are used by refresh-only own sign-ins — partial — refusal fixture proves refresh-only defaults; metadata combinations remain open.
- L312: profile and predictable state symlinks cannot redirect refusal writes; signout prunes state — open.
- L334: agy with no CSRF is retried after an authless endpoint 403 — covered by extended owned loopback route fixture.
- L347: missing windows without resets survive only one partial round across restart — covered by Antigravity partial-pool restart route.
- L360: refused access refreshes once, valid refresh cache survives expired source expiry — open.
- L380: malformed and unreadable sign-in retain known identity and windows — open.
- L404: access refusal persists through failed refresh and restart without resending old bearer — covered by refusal/restart fixture.
- L423: unwritable or unreadable refusal journal stops further sends — open.
- L443: restart never sends a refused token returned unchanged by refresh — open.
- L459: refused refresh is not retried after access disappears or changes — partial — refusal survives restart; changed access cases open.
- L477: local CSRF refusal survives restart — covered by extended owned loopback route fixture; the rejected token is not resent.
- L491: persisted refusal survives restart and metadata-only credential rewrites — open.
- L510: holds stop further endpoints; a refusal never resends unchanged credential — open.
- L531: complete read, partial and unreadable reads keep prior known windows — open.
- L560: only the main host's own refusal of the token's call can refuse the sign-in — open.
- L583: a token a summary just answered is never refused by a later call, which still tries the daily host — open.
- L603: a daily host's wait keeps the main host's windows, and only that host waits — covered by independent held-host fixture (other quota remains usable).
- L625: a plan or project lookup's wait keeps the windows read, partial, and holds only that call — open.
- L656: the language server's plan status waiting keeps its windows, and only that call waits — open.
- L676: with every host waiting, the read waits for the soonest, and says whether a request left — open.
- L721: A contradicted Gemini reading keeps the other pool readable until Google counts Gemini — open.
- L765: use before Google's window started, or in another pool, leaves Google's unused windows standing — open.

### `box/test/usage/usage-antigravity-tls.test.ts`

- L9: own loopback language server accepts its self-signed HTTPS certificate and CSRF — covered by extended local TLS route fixture (including CSRF refusal and restart).

### `box/test/usage/usage-codex.test.ts`

- L62: Codex's logs: turns from running totals, replays and repeats left out, tiers and model switches, priced by Codex's rules (UsageKit's CodexLogUsageScanner) — covered by shared Codex corpus through both routes, with archive date moved into the display window; see slice 13.
- L67: with a fallback model chosen, a model with no price is estimated at its rates, and the day says so; still an unknown model — covered by shared Codex corpus and actual fallback choice route, exact arithmetic.
- L77: the engine's rows: only those priced at nothing, a message copied between databases once, priced as Codex's (UsageKit's OpenCodeCodexUsageScanner) — partial — engine OAuth history route fixture; full UsageKit corpus open.
- L83: auto-review is priced by its date: the model it ran on then, free from 2026-10-06 (UTC); a snapshot date comes off a model's name — covered by the production pricing component's historical boundary test, plus the shared native corpus's free auto-review and snapshot-name prices.
- L159: an account's sessions and archived sessions: a log in both counted once, as its sessions copy; a row by the account, Codex's total — covered by live priority versus archived copy and exact native corpus differential.
- L183: a log as it grows is read on with its running total, through a restart; one rewritten is read again whole — partial — native JSONL append/saved cursor test; integration and rewritten Codex fixture open.
- L213: on a Mac an added account's history is your own Codex's (its sessions link there): counted once, with yours — covered by linked native root fixture on Mac/Unix.
- L230: two accounts that link to a folder neither is in (your ~/.codex while CODEX_HOME names another): it's the first's, and each keeps its own — open.
- L252: your own Codex reads each home a comma-separated CODEX_HOME names, as UsageKit did; a home with neither sessions nor archived sessions is read itself — covered by the multiple-home live/archive fixture (including flat-home file).
- L277: the fallback model is the box's (and your other computers'): chosen through the route, kept through a restart, a model with no price estimated at its rates; one that isn't a choice is refused — partial — shared corpus validates local choice, restart, refusal and clearing; peer convergence open.
- L339: only a ChatGPT sign-in's rows, priced at nothing, from openai; opencode 2's only from when it signed in; a compaction counts — partial — v1/v2, zero-price and sign-in cutoff verified; compaction fixture open.
- L371: a database that can't be read, and none that could: not known now (never none); one the engine left without its -wal and -shm is read as it is — open.
- L386: an API key's sign-in, or none: nothing of it; before opencode 2 took its sign-ins in, the auth.json beside it says — partial — sign-out verified; API-key credential and legacy auth-file scenario open.
- L400: its row is Codex's: the same ChatGPT account as a Codex account here is one row, both added up; read again only when a database changed — open.
- L456: one database that can't be read while another can: not known now (what was read stays), never kept as whole; one that keeps failing doesn't hold up the rest — open.

### `box/test/usage/usage-cursor.test.ts`

- L51: ${each.fn}: ${each.name} — partial — 15 original plan corpus cases through actual Rust routes (mapUsage, grokBot, mapSummary, request-based usage); independent fallback, append-missing and plan-name variants remain open (see slice 14).
- L91: usage events: each page's events, its total and those it couldn't read; by the day, priced without long-context rates, by family (UsageKit's CursorUsageEvents and appendSpendLines) — open.
- L109: a page that isn't one, or an event that isn't, as UsageKit takes them — open.
- L142: its meters, its credits and its plan, read with its own sign-in; who it is from Cursor's answer, only as a hash — partial — standalone Cursor key/plan/meters/credits fixture; exact identity and full mapper variants open.
- L166: its token is kept while it lasts, and exchanged again once when Cursor turns it away — covered by Cursor route renewal and exact request-count fixture.
- L178: a key Cursor turns away is a sign-in to renew; a 429 holds off as long as it says; no sign-in, no row — covered by Cursor refusal, persisted 429 hold, sign-out and replacement credential route scenario.
- L193: an on-demand spend with no cap: no window, but what the plan has beyond them (′extras′), on its row across your computers — open.
- L209: a sign-in file that can't be read just now keeps the plan's numbers (tried again soon), not a sign-out — open.
- L222: a request-based plan: its requests as its total, said once; the summary's on-demand — covered by the Enterprise request-based plan fixture with exact request cap and on-demand dollars.
- L238: a test's box never asks Cursor itself, whatever sign-in its folder holds — open.
- L265: filled in a UTC day at a time, page after page; then only today and yesterday asked again, and a day Cursor didn't answer whole — covered by Cursor 32-day initial and two-day restart fixture.
- L294: a day whose later page fails, or whose total moved, keeps what was known of it; one never known can't be had, so the read fails rather than say nothing was used — partial — incomplete second page preserves the previous complete day; changed total and never-known day tests open.
- L325: when the time runs out, the days that came are kept, and the next read asks only for the rest — open.
- L351: out of time in the middle of a day of many pages: no page after it is asked — open.
- L368: on the box: Cursor's row by the day in the client's zone, by family; a read that fails keeps the last known; a round with nothing new works nothing out again — partial — Cursor history route UTC, prior complete data retained, repricing without page reads; non-UTC and idle round scenarios open.
- L401: a plan Cursor can't state (no subscription): the account is still known as itself, so two computers' rows of it are one, never added up — open.
- L426: a sign-in file that can't be read just now (being rewritten) isn't a sign-out: what the box had stays — open.
- L445: signed out of Cursor here: no Cursor row from this box — open.
- L460: a row's extras are its newest reading's: one with none says there are none now, though an older box still has some — open.
- L471: a row takes its newest box's reading, never a sum — open.
- L490: read on another of your computers lately: this box doesn't ask Cursor, and keeps what it had — open.
- L515: one of your computers asks Cursor for an account's history at a time, however its plan read went; one that stops, or is away, is taken over — open.
- L571: a box whose reads keep failing (today's events, while older days are known) doesn't say it read the account, so another of your computers takes over — open.
- L587: a 429 that doesn't say for how long: the plan's own wait doubles, as for other makers — open.
- L597: an unknown computer does not affect which peer reads Cursor — open.
- L604: a key Cursor turned away isn't sent again, nor anything while Cursor holds reads off (the exchange's 429, the events' 429), whatever asks — partial — actual plan rejection/429 shared with account-wide history across restart, including explicit refresh; history-only exchange/events hold variants remain open.
- L647: a token turned away midway whose exchange is then held off (429): no page more is sent with it; nor any once the plan's read is held off meanwhile — open.
- L672: a 429 from one of the plan's optional reads holds the rest of them, and the history, off; what was read last stays whole — open.
- L694: a token turned away midway whose key Cursor then turns away too: no page more is sent, and its plan isn't asked again with it — open.
- L709: a token Cursor turns away midway through the history is exchanged again once, and its page asked again; two reads as the box starts share one exchange — open.

### `box/test/usage/usage-grok-hosted.test.ts`

- L62: Grok's transcripts: completed turns by model, its own costs where it said them, else priced; a copied event once per model (UsageKit's GrokLogUsageScanner) — covered by exact shared Grok corpus on both routes.
- L75: the engine's hosted rows: Go's and Zen's billed costs, both tables, a copy in another table or database once; a key of your own, a cost as text, a running compaction never (UsageKit's OpenCodeUsageScanner) — open.
- L109: an account's sessions' transcripts, only their updates.jsonl: a row by the account, Grok's total; read on as one grows, through a restart — covered by both-box shared Grok corpus, append and restart.
- L141: the default account reads your GROK_HOME (your Terminal's sessions too); an added account whose home is the same folder doesn't count it again — open.
- L179: Go's and Zen's each a row, as billed (not estimated): Go's the same row as its plan limits'; OpenCode's total both; the databases only read — open.
- L197: an engine's databases in WAL mode, closed so their side files are gone (a channel not running), are read as they are, every one — open.
- L218: a provider with no use in the window has no row; no database, none; one that isn't the engine's (no message tables) says nothing — open.
- L236: one database that never could be read while another can: the other counts; none readable: what it had stays; unchanged databases aren't read again — open.
- L277: a database that can't be read now, after it was, gives what it last did: never less, logged once, and read again whole next round — open.
- L308: just after a restart, a database that can't be read is not known now: the spend kept from before stays, never less — open.
- L329: both providers' rows come from one read a round, though the engine writes while it's read — open.
- L344: an engine folder that's there but can't be listed: not known now, what the box had stays — open.
- L362: no engine folder known: no source, and a box that says so leaves OpenCode out of what it states — open.

### `box/test/usage/usage-notices.test.ts`

- L22: healthy to close fires once — covered by fixed-clock accepted current-window projection crossing test (#990 current-window rather than old elapsed-window projection).
- L23: close to running out — covered by the same healthy/close/critical sample matrix.
- L24: healthy straight to red fires only critical — covered by fixed-clock accepted-window direct projection crossing.
- L25: under ten once per window — covered by accepted current-window level warning matrix and repeated crossing route.
- L26: cold start records bad baseline — covered by cold almost-out/unknown-cadence accepted reading test.
- L27: reset rollover re-arms — covered by fixed-clock accepted real new-window level warning test.
- L28: reset jitter does not re-arm — covered by same fixed-clock real rollover versus jitter scene.
- L29: recovery then worsening re-fires — covered by scoped accepted-reading test with corroborated recovery.
- L30: under ten recovery re-arms — partial — corroborated current-window recovery and distinct occurrence test; exact threshold fixture open.
- L31: no data never fires — covered by fixed-clock invalid-window rejection preserving baseline.
- L32: level fires almost out only — covered by fixed-clock reset-less/unknown-period threshold scenario.
- L33: no data preserves previous signals — covered by invalid read between accepted samples and subsequent crossing.
- L34: disabled toggles never fire — open.
- L35: per trigger off suppresses only it — open.
- L36: off toggle does not consume edge — covered by scoped accepted-reading off-toggle test.
- L49: evaluator healthy to close once, facts and route — open.
- L50: evaluator close to red — open.
- L51: evaluator jitter and restart do not duplicate — open.
- L52: evaluator all toggles off — open.
- L53: evaluator per trigger off — open.
- L54: evaluator disabled provider prunes baseline — covered by provider hide/re-enable route regression and dormant baseline state.
- L55: evaluator remaining threshold uses remaining regardless display mode — open.
- L56: client delivery failure leaves fact available for retry — covered by retained-fact missing receipt / restart and later observation retry scenario.
- L57: failed read cannot create notices or cache zero — open.
- L58: hidden metric prunes baseline — covered by empty layout hide/show route and dormant fixed-clock trail test.
- L59: pace near empty and projection gate — open — related accepted-window and notice tests above; this exact case not yet ported.
- L60: notices go only to peers in the setup — open.
- L71: large notice history cannot evict the current limits account from peer mail — open.
- L82: turn-only Mac Claude inherits peer window dedup without checked timestamps — open.
- L96: visibility mappings cover model-specific Codex and Antigravity windows — partial — corrected Spark alias and existing Antigravity mappings; exact multi-window layout fixture open.
- L102: exact ten percent remaining does not trigger Almost Out — covered by exact 0.90 accepted level sample, then 0.92 crossing.
- L108: parallel readers use the same occurrence IDs and recovery receives a new one — partial — scoped test checks the distinct rearmed occurrence; simultaneous parallel readers open.
- L116: peer handoff timestamps are normalized to the receiver clock — open.
- L128: one direct account holder forwards even a received peer decision — partial — expired notice-state handoff verified; received fact forwarding and pin election open.
- L142: provider hide and re-enable resets baseline but preserves occurrence counters — covered by dormant accepted trail restart and unique second occurrence test, plus route hide/show scenario.
- L152: notice facts stay in their setup — open.
- L173: setup changes invalidate notice caches while preserving ordinary limit readings — open.
- L185: catch-up is short and warnings disappear when their reset passes — covered by retained current/old/expired warning catch-up route scenario.
- L198: without layout settings only default meters can decide notices — open.
- L208: a decision waits for the directory and pins a forwarder only once — open.
- L224: ordinary turn readings keep the thirty-second disk throttle while new notices persist immediately — covered by actual turn/cache/notice scenario, old Rust failed and patched Rust focused check passed.
- L239: unsupported Devin notice facts from older peers cannot trigger native alerts — open.
- L244: notice wording carries the same named account and window as the panel — open.
- L256: learning the setup immediately shares pending facts despite a recent unscoped limits mail — open.
- L272: an exact secondary Claude card's meter visibility and enablement override the family card — open.
- L285: unmappable Codex card identities fall back to enabled matching meters on any family card — open.
- L294: conflicting pins for the same occurrence converge to the smallest forwarder and persist — open.
- L314: unrelated directory heartbeats do not bypass the heard-mail throttle — open.
- L329: unsupported notification providers and their wording come from box answers — open — related accepted-window and notice tests above; this exact case not yet ported.

### `box/test/usage/usage-settings.test.ts`

- L152: a client reads them, changes a part at a time, and is told; one that isn't what it should be is refused; they stay through a restart — partial — settings route scenarios above.
- L193: the Codex fallback model a box chose before it kept these counts as chosen, and gives way to a change — open.
- L207: come in once: the parts no box has set, never again, never over a part changed since; a fallback model the box can't price with is left out — partial — settings route scenarios above.
- L227: two Macs' imports: the one made first stays on both, a later Mac's first-run defaults never go over it; a change made since goes over both — open.
- L246: three boxes: a change on any reaches the others, the newest change wins on all, part by part; one with a clock behind still counts on from what it has seen — partial — actual current shared-authority route: both boxes refuse member writes (409) and converge main-computer changes part by part; legacy decentralized clock/tie fixture open.
- L287: a box that was off catches up once it says what it holds; settings came from another computer stay through its restart — partial — actual both-box authoritative setup fixture across three computers and returning/restarted member; legacy digest mail case open.
- L310: settings ignore obsolete directory entries and wait for peer membership — open.
- L353: usage settings mail requires a known matching setup — open.
- L385: a box that leaves your setup for another starts over: none of your settings go there, and its own Mac's come in again — open.
- L427: a client of an older app sends the cards and meters it knows: the box keeps a newer app's — covered by both-box layout/known-meter matrix, including explicit whole replacement.
- L449: two boxes of different versions hold the same: a part or field one doesn't know is kept as it came, so they agree and stop telling each other — open.
- L474: a change made from a version of a part that has changed since (another computer's came meanwhile) is turned away, and the client shows the box's; a field a newer box added stays whatever the client sends — partial — three-computer authority scene checks stale version from returning member; prior local unknown-field preservation case; legacy peer-merge variant open.
- L496: a box that followed no setup and joins one starts over: its old settings stay local — open.
- L509: a box whose link doesn't say its setup (its keys not read yet, or a link from before setups, met for a moment as the box updates) keeps its settings: nothing's recorded as no setup, nothing goes, and the setup it names next is taken as theirs — open.
- L529: usage settings reject a future disk format without saving over it — covered by Rust route refusal and byte-identical retained future fixture (TS original unit test throws in constructor).

### `box/test/usage/usage.test.ts`

- L36: Claude's logs: the lines that count, deduplicated, priced, by the day (UsageKit's ClaudeLogUsageScanner) — covered by shared Claude corpus through both routes, exact costs with resident's merged catalogs.
- L58: the bundled prices name each model's rates as UsageKit's do: aliases, fast variants, fuzzy keys, models.dev, none — covered by the shared pricing-expected.json corpus at exact f64 bits.
- L106: an account's logs as days in the zone a client asks for: late evening in UTC is the next day in Istanbul — covered by native usage history timezone scenario.
- L142: a log as it grows is read on from where it was; one rewritten shorter is read again whole; the numbers stay through a restart — covered by native Claude usage route partial append, rewrite and restart (and the Insights boundary scenario).
- L170: a subagent's log replaying its parent's reply counts once; an account removed goes; one with no identity yet is its box's own row — covered by shared corpus and account lifecycle scenarios; reference scratch account removal uses the resulting roster rather than its unavailable mutation bridge.
- L189: a client's days are today and the 30 before it, in its zone — covered by 31-day cutoff fixture and existing Istanbul zone case.
- L201: fetched hourly with what it was last sent, used from the next scan on, kept for the next start; a feed that fails or says nonsense changes nothing — partial — ETag, changed feed, invalid feed and restart fixture; elapsed-hour timing open.
- L235: only a real box's bridge fetches them, a Mac app's or a machine's: never one a test started in a folder of its own — partial — scratch homes disable feed requests except explicit loopback fixtures; exact real-home path matrix open.
- L246: new prices price the next round: a model that was unknown gets its dollars — covered by changed price feed reprice scenario.
- L398: one account used on two computers is one row, its usage added up from both; another account is a row of its own; each box lists them all — partial — native Codex three-computer same-identity and separate-account scenario; exact Claude identity, idle/away timing and removal variants open.
- L477: Codex's use estimated with a fallback model goes to the others saying so; a box from before, which keeps none of that, finds the same digest and asks nothing again — partial — fallback flags excluded from digest and full retell on change implemented; exact older reader scenario open.
- L544: an account too big for a mail is cut from its oldest uses and says so; a long name; what changes after costs no more than its last two days — partial — bounded full/recent slots traffic implemented, live delta case; size-cut test open.
- L574: what the engine's hosted models cost on a machine shows on your Mac as billed, not estimated, in OpenCode's total — covered by local engine billed and peer merged cost scenarios.
- L607: two boxes on one computer reading one folder count it once; a reply written again for each of its parts is kept once — covered by native Codex shared-folder peer scenario and Claude duplicate corpus.
- L634: two computers with one name reading the same path (two Raspberry Pis' /home/pi) are two computers: both count — open.
- L670: refresh asks peers to read now and a restart restores their cached usage — covered by peer merge refresh, live delta and away restart scenario.
- L715: old directory and usage records do not add an unknown computer to the totals — partial — unadmitted Insight origin fixture and authenticated current usage roster; exact kept usage-cache refusal open.

### `box/test/notices-usage.test.ts`

- L24: queue acceptance is not delivery; a later successful hold send consumes the id — open.
- L42: an unsent fact can retry on the next minute without being marked delivered — open.
- L51: a queued notice does not push after its reset passes — open.
- L63: peer notice is kept for Mac without pushing; source can still push once — covered by existing merged notice feed route scenario: exactly one source notice visible on the other computer and no redundant local copy. The Rust merged history avoids duplicating the source's receipt.
- L74: catch-up keeps maker time, has a short age limit, and drops reset windows — covered by retained warning receipt restart, maker timestamp, old/expired refusal and idempotent replay fixture.
- L90: Mac before its hold is ready leaves usage eligible; deliberately disabled phone link consumes it — open.
- L105: partial delivery to two phones retries only the phone whose send failed — open.
- L122: journal write failure retains in-memory marks and retries persistence without throwing — open.
- L142: a usage notification shown by Mac suppresses only its stable id, not another on the same row — open.
- L155: a queued losing forwarder copy is cancelled when a snapshot agrees on another holder — open.
- L168: a catch-up resolving after a reconnect cannot replace the new connection's snapshot — open.
- L181: a newer usage-notices event invalidates an in-flight catch-up — open.
- L196: box phone receipts clamp future dates before keeping them for one day — open.
