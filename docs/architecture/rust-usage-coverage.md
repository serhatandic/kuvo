# Rust usage coverage (row 5.4)

This is an in-progress map, not a parity claim. A case marked **open** has not been reproduced against the Rust implementation. **Partial** identifies a corresponding new scenario which covers only part of the TypeScript case. The known-correction cases are intentionally skipped on the TypeScript oracle rather than weakening the Rust assertions.

Inventory: 392 named or parameterized test declarations. Parameterized declarations are represented by their source line; their variants are still open unless individually demonstrated.

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
| Demand | `a subscribed rail asks for a current reading without an idle polling loop` | Rust-only design change |
| Insights | `Insights catches up before first open and folds changed transcripts without double-counting` | Rust-only design change (supersedes first-slice demand-only index) |
| Insights | `a portable transcript contributes its tokens and cache reads to Insights on either box` | Both boxes |
| Limits | `the empty limits answer states every service and refresh state in the contract order` | Both boxes |
| Cache | `an unreadable accepted-limit cache cannot prevent the resident from opening or be overwritten` | Rust-only format guard |
| Engine plans | `engine credential plans are read with their own sign-ins and display bounded counts and credits` | Rust reader with a loopback maker fixture; TS scratch bridges deliberately disable these provider calls |
| Claude | `Claude Code answers its plan via its own demand-started control process` | Rust native fixture |
| Resets | `a Codex reset is offered without spending, journaled before the maker, and replayed once after restart` | Rust native fixture; account switch also refuses the old offer |
| Resets | `an agent has to ask before spending a reset, and Always Allow is account-specific` | Rust native and engine fixtures through the existing work-ask routes |
| Menu | `a connected Helper menu requests fresh rings and the same accepted reading reaches its feed` | Rust demand reader |
| Resets | `reset routes refuse missing or unknown accounts without creating a spend` | Both boxes |
| Pace | `usage_readings.rs` | Deterministic window regression, single false refill, same-window alert deduplication, previous-window pace exclusion |
| Pricing | `usage_pricing.rs` | Bundled price resolution, exact fallback, cache writes and fast rates |

## Deliberate work-timing changes

The user's revised rule is to keep frequently viewed data ready, incrementally, and to
start rarely used work only on demand.

- Insights begins a catch-up at startup on a blocking worker, subscribing to the
  platform's file watch before the catch-up. A burst of file changes folds the affected
  portable transcript(s) into SQLite using their metadata checkpoint. Reads serve the
  index, not a periodic history walk. A lost watcher event permits a checkpoint
  recovery pass. There is no separate Insights process. The startup/file-change
  conformance scenario checks SQLite before the first client query and before the next
  query, as well as duplicate prevention. Native-file offset cursors and complete
  native history coverage remain open.
- Limits are refreshed on a rail/menu subscription, a manual refresh, turn readings,
  when a reset/projected crossing becomes due, and on a one-minute wake while an
  authenticated client request or stream is active (provider reads are throttled to
  ten minutes between non-manual refreshes). When there are no connected clients and
  no deadline, there is no periodic wake. The immediate rail/menu cases and the idle
  startup case demonstrate the difference. A separately timed periodic/client-detach
  and deadline scenario remains open.
- Potential further improvements: `usage/insights.rs`'s `scan_changed` reparses
  about 416 KiB of bundled price catalogs per change batch and re-reads compact
  projections within a changed SQLite transcript; cache price resolution and add
  a per-transcript mutation cursor when native log cursors are introduced. The
  platform watch fallback (`kuvo_os/src/watch.rs`, `start_watcher`) can compare
  contents on a polling filesystem when a native watch cannot start; avoid a
  history-wide fallback on large provider roots.

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

## Remaining implementation gaps

- Direct provider limits: Claude native readings and engine credential readers have local fixtures. Claude Linux reset grants, standalone Grok, Cursor, Antigravity, full engine-reader edge cases, and peer account merge/refresh and read hold/backoff parity remain open.
- Reset spending: local Codex credit confirmation, replay journal, local Always Allow and the work-ask path are implemented and exercised. Claude grants, peer forwarding/decisions, prolonged unsettled-spend settlement and other reset edge cases remain open.
- Reading preservation across TS-to-Rust upgrade, multi-computer account identity and notice forwarder election, all card/rail/detail presentation and notice visibility cases.
- Native provider histories and engine SQLite scan, pricing feeds/repricing, history quarter-hour slots, Insights cursor exchange and lineage, model naming and full summary readiness. The partial Insights overview is explicitly partial and the summary is not advertised as complete.
- Shared-settings legacy peer mail/version interop and setup migration cases beyond the existing authority's shared save path.

## TypeScript case inventory

### `box/test/usage/engine-limits.test.ts`

- L50: each sign-in in Settings › Accounts is a row of its own, read with that sign-in — partial — engine credential plan scenario (individual integration rows; full added-account cases open).
- L91: an OpenRouter key's spend today, this week and this month (UTC's), each starting over when OpenRouter's does; what was bought, for what's left of it — open.
- L108: a Copilot seat its organization pays for by the credit: the organization's credits and spend this month, where the sign-in may read its billing; nothing, never a zero, where it may not — open.
- L139: GitHub holding requests off (a 403 with retry-after) stops the look through your organizations, and the next read looks again — open.
- L152: ChatGPT's credits and reset credits, read with the engine's sign-in — partial — engine credential plan scenario (credits and zero reset inventory).
- L161: an API key with no plan isn't listed; a lapsed sign-in says so without asking its maker — open.
- L173: SuperGrok's weekly pool, as the Grok CLI reads it — partial — engine credential plan scenario (xAI weekly reading, not the standalone CLI).
- L180: a SuperGrok plan that ended is listed as ended, with nothing of its last week and no ring — open.
- L196: an xAI answer in a shape Kuvo doesn't know is a read that failed, never a plan that ended — open.
- L202: OpenCode Go turning a key away for having no plan lists nothing, rather than asking to sign in again — open.
- L208: a box starting before its engine answers keeps the engine's sign-ins' holds, and asks nothing early — open.
- L241: an API key turned away is tried again later, not held to that key as a lapsed sign-in — open.
- L254: each sign-in says who it is, the same on every computer: a key by the key, Copilot by GitHub's login, SuperGrok by its user — open.
- L296: signed in to Copilot as another GitHub account, what was kept of the one before isn't shown as this one's — open.
- L320: an account's mark in Accounts is the service its limits are read as — open.
- L326: OpenCode Go's 30-day window shows a forecast after frequent refreshes — open.

### `box/test/usage/insights-checkpoints.test.ts`

- L11: Insights checkpoints local history and counts only changed transcripts after restart — partial — startup/file-change portable index scenario; native checkpoints and restart case open.
- L41: Insights adds engine updates without replaying the older calls in the same database — open.
- L74: learning the box's peer name leaves its unchanged local history checkpoint in place — open — related portable checkpoint scenario above; this exact case not yet ported.
- L111: a large backfill keeps all usage as it releases batches and stays finished after a restart — open.

### `box/test/usage/insights-kept.test.ts`

- L26: a period asked again is answered at once with the last summary, marked as of when, while it's read afresh — open.
- L62: a summary that says the box is still reading its usage is never kept — open.

### `box/test/usage/insights-refresh.test.ts`

- L36: a refresh counts again before it answers, and the summary says when the box last counted — open.

### `box/test/usage/insights-summary.test.ts`

- L58: a bot's row is its own chat and every session it oversees, subagents and older starts included — open.
- L123: Bots includes all unowned sessions after the old ranked-list limit — open.
- L133: a refresh attributes retained calls through engine sessions without changing their usage or origin — open.
- L188: each model has one name however its id was written, and narrows by it — open.
- L215: the cache rate is cached input over all input, at every level — open.
- L234: the change is against the period of the same length just before — open.
- L253: at most three sessions stand out, each for one reason, once there are enough — open.
- L267: Insights waits for a new computer’s first complete copy and counts kept usage after it goes away — open.

### `box/test/usage/insights.test.ts`

- L49: an Insights worker exit rejects requests and the bridge reopens the persisted index — open.
- L85: closing the Insights index is idempotent and later requests reject — open.
- L93: copies merge before filters, resumed sessions count once, and moved bot calls retain their origin — open.
- L114: a peer account observation enriches the matching local copy and keeps its counts — open.
- L132: a newly available provider call replaces its saved placeholder across filters — open.
- L142: day bounds use the viewer's time zone, including DST transitions — open.
- L154: a cold yearly query and repricing both leave the bridge event loop responsive — open.
- L186: a session page computes shares of the whole filtered snapshot after a scan — open.
- L207: resume peer pages, reject vouched computers, and price incoming records locally — open.
- L231: rewind an in-range cursor, then reset again when the serving peer learns the retired origin — open.
- L281: historical replay converges on complete counts and rejects a conflicting unrelated origin — open.
- L312: isolated Claude, Codex, Grok and Antigravity records join Kuvo steps, and the Kuvo engine uses its SQLite reader — open.
- L423: legacy history stays on its provider slot through sign-in changes, while observed accounts stay pinned — open.
- L458: opening an unversioned Insights database stamps it without losing records — open.

### `box/test/usage/limit-card.test.ts`

- L67: a 5-hour window used at a steady pace says when it runs out, ahead of its reset, and the ring gets a warning dot — open — related accepted-window and notice tests above; this exact case not yet ported.
- L79: a window used slowly lasts until its reset, and says so with no dot — open — related accepted-window and notice tests above; this exact case not yet ported.
- L87: with too few readings nothing is predicted: what's left and when it resets, no more — open — related accepted-window and notice tests above; this exact case not yet ported.
- L95: what the readings said is kept through a restart of the box — open.
- L103: each card shows only its own account, named among its agent's others — open.
- L124: a reset is offered only where one can be used, naming the soonest to expire — open — related accepted-window and notice tests above; this exact case not yet ported.
- L139: an agent's many accounts share one ring, its card a strip of them emptiest first, fine ones counted — open.
- L158: each coding agent's last numbers stay on its card, as of when, once it can't be read any more — open.
- L179: an account with no numbers read yet has no ring, rather than an empty one — open.
- L208: a weekly window read every minute still shows a forecast after eight hours — open.
- L213: a busy weekly window keeps its forecast through a box restart — open.
- L219: Codex keeps a weekly forecast when its reported reset time moves by a few minutes — open — related accepted-window and notice tests above; this exact case not yet ported.
- L233: a downward correction within the same week keeps the forecast — open.
- L244: a new quota period starts its own forecast after the last reset — open — related accepted-window and notice tests above; this exact case not yet ported.
- L262: Antigravity's unused pools can forecast while Google keeps moving their reset times — open — related accepted-window and notice tests above; this exact case not yet ported.

### `box/test/usage/limit-display.test.ts`

- L24: Usage names windows, supplies their used percentages and lists a shared source once — open.
- L44: Usage dates each merged meter by its own reading and names the computer that supplied it — open.
- L56: Usage marks an overdue reset and leaves the historical value intact independently of the sidebar — open.
- L65: Usage ages readings by their own time and marks away or failed sources old immediately — open.
- L77: Usage gives plain next steps instead of exposing the provider’s error — open.
- L93: Usage draws capped credit balances and extras even without normal windows — open.
- L113: Reset labels round up across hour and day boundaries, by elapsed time across DST — open.
- L142: Limits include the check time for an ended plan without meters — open.
- L154: A delayed provider leaves refresh pending and sends an update when the new snapshot is ready — open.
- L189: A failed provider refresh keeps the prior successful completion time — open.

### `box/test/usage/limits-peers.test.ts`

- L233: an account signed in on three computers is read from its maker once each round, by one of them, and every one lists it once with its newest reading — open.
- L285: computers that read at the same moment settle on one reader by the next round — open.
- L301: a refresh asked of one computer reads there only what's due, and asks the computer an account is on for one it doesn't have — open.
- L324: a computer that's off keeps its last reading, listed as of when it was read and away, through a restart; what it read is read elsewhere once it's due — open.
- L356: a maker that turns one computer away holds that one only: another reads the account, and a read that failed never stands in for one — open.
- L388: a computer signed in to another account since, whose sign-in doesn't say who it is (Copilot's), reads it rather than taking a peer's reading of the one before — open.
- L440: Refresh on a Mac, whose Claude only its turns tell of, asks the computer that reads that account — open.
- L460: an agent's plan_limits says the numbers its computer's accounts were read with anywhere, and the accounts on your other computers, where — open.
- L484: a box whose group chats didn't start takes plan limits, and says not now to group chat mail, so what mustn't be lost comes again — open.
- L495: the link saying who your computers are again tells no one again, and an ask is answered once a minute at most — open.
- L513: a tell lost while a computer's link was off is made up once that link carries mail again — open.
- L535: an account of a service a newer box knows and this one doesn't is listed by its name — open.
- L547: what a peer's account has beyond its windows (′extras′) is kept as told, and only what one is — open.
- L579: a computer you removed goes from the list, with its accounts — open.
- L588: a peer whose clock is an hour ahead holds this box back no longer than one whose clock is right — open.
- L604: what goes between them is the readings, never a sign-in or who the account is at its maker — open.
- L624: what your own machine last told is kept whole through a restart: shown once the link confirms the computer is a peer — open.
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
- L493: with a machine signed in to the same account, the Mac's row has its resets, and a reset asked on the Mac is used there, once, and the Mac answers when it is — open.
- L536: Always Allow, set from the Mac, is kept with the account on the machine that spends its resets, and every box says so — open.
- L583: its maker couldn't say a reset went through: Retry, with a new confirmation, goes as the same request, so a second isn't used — open.
- L604: two confirmations at once (two apps): one request goes, and both hear it went — open.
- L619: Always Allow is the account's: signed in to another account in the same place, agents ask again, and coming back doesn't bring it back — open.
- L643: Always Allow set on one of your computers with the sign-in is kept on the other, and turned off on either, it's off on both — open.
- L668: the answer from the computer with the sign-in is lost: Retry, with a new confirmation, is asked there under the same id, and nothing more is used — open.
- L714: Always Allow, and another account signed in between the offer and the spend: nothing is spent from it, and the allowance goes — partial — stale Codex offer refused and separate local Always Allow on/off case.
- L733: Always Allow on, then off, while the computer with the sign-in is away: the older on, told last, doesn't undo the off — open.
- L760: an off for an account this box doesn't know yet is kept: once it does, the account isn't allowed, and an older on doesn't change that — open.
- L776: a reset still open, asked again while its maker holds requests off: refused for now, and still open, never 'nothing was used' — open.
- L794: a new reset asked well after one that never heard back: that one is settled first from what its maker says, and this one is its own — open.
- L831: no answer came from the computer with the sign-in, then it can't read the account (Claude Code signed out there): the Mac's spend stays open, never 'nothing was used', and Retry goes again — open.
- L851: a peer's ask more than 10 minutes after an open spend that went through: that one is settled first, and this one is a reset of its own — open.
- L871: hetzner's clock a minute ahead: Always Allow on there, then off from the Mac within that minute, is off on both — open.
- L888: an agent's reset under Always Allow, with it turned off on the computer with the sign-in: refused there, nothing used — partial — local off decision rejects an unasked confirmation; peer behavior open.
- L904: two confirmations at once, well after one that never heard back: that one settled first, then one fresh request for both — open.
- L919: the computer with the sign-in can't be reached while a spend is open: it stays open and says an earlier try may have gone, never 'nothing was used' — open.
- L937: Claude on Linux: the token is another account's than Claude Code's record says: nothing is spent from it — open.
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

- L113: signed out of Cursor in Accounts, its ring is gone at once, and what was read of it with it — open.
- L143: Accounts' word on Cursor is the rings': its own key turned down there, no ring, though the key still reads; the Cursor app's sign-in is never read — open.
- L165: a Mac's Claude Code signed in has its ring before any turn told its limits, as Claude Code says them; signed out, none — open.
- L186: a newly signed-in Mac Claude publishes its first reading before the next limits round — open.
- L209: what was kept from before is never shown before the agents have said who's signed in, and one signed out since goes from the file — open.
- L229: each ring has its own provider's windows only: Cursor's own Grok Bot week stays Cursor's, and Grok's row has Grok's — open.
- L244: a sign-in check that fails (offline) isn't a sign-out: the ring and its readings stay — open.
- L256: an agent that never says how its sign-in stands holds up no reading: the others are read, and it's as it was — open.
- L266: started again, the box lists what it last read of an account still signed in here until its agent says, so the ring doesn't blink out; one signed out meanwhile isn't — open.

### `box/test/usage/limits.test.ts`

- L157: an older Claude Code's event, with only the window that limits it, keeps that one — open.
- L165: Claude Code's warning of a limit says how much of which one is used, and close only from 90% (#257) — open.
- L200: each login is its own row with what Claude Code says of it (its windows, plan and extra usage), its resets say they show only from Linux, and Anthropic is never asked with its sign-in — open — related accepted-window and notice tests above; this exact case not yet ported.
- L248: its extra usage, and its reset grants read at most hourly: a failed read of them keeps what was read, never none — open — related accepted-window and notice tests above; this exact case not yet ported.
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
- L152: Grok's plan is what Grok itself says in the account's folder, signing in with the sign-in it keeps there — open.

### `box/test/usage/service.test.ts`

- L46: the limits route reads and renders all six plans from their fixture credentials and APIs — open.
- L148: an open bridge feed learns of an external sign-in switch and a removed CLI with credentials left behind — open.
- L230: the bridge retries its latest window after the services process restarts — open.
- L267: a provider removed from the admitted accounts during a round is not read — open.
- L285: a Codex login stored only in Keychain is read with the exact account for its home — open.
- L302: Codex auto storage joins pending keyring lookups and prefers the keyring credential — open.
- L326: a Codex reset goes only while Codex says it's signed in to the row's account, and through Codex itself — partial — Codex reset replay scenario refuses changed sign-in; full maker/file divergence scenario open.

### `box/test/usage/usage-antigravity-history.test.ts`

- L37: shared blob fixtures decode exactly as Swift, including fallback time and malformed messages — open.
- L42: read-only SQLite batches and pricing match Swift's shared expected days — open.
- L61: WAL-only changes invalidate a cached scan, and a later cutoff prunes it — open.
- L72: legacy databases without steps, then gaining metadata, are re-read — open.
- L84: oversized blobs are skipped, and an unreadable new database is logged once and left out while the others count — open.
- L116: ACP databases count, and empty stores and stores without generation metadata are skipped quietly — open.
- L143: a conversations folder that can't be listed is logged once and left out while the others count — open.
- L158: missing stores are empty and symlinked stores count only once — open.
- L170: own profiles alone, machine-hashed places, and an unreadable new database leaves the rest of the account counted — open.
- L202: deleted databases leave the total after restart and do not block the remaining stores — open.
- L226: per-database cache reads only changed stores, ages out idle ones, counts changing stores' fresh reads and keeps unreadable stores' last events — open.
- L266: blob bounds match Swift: skip oversized generations and absent step timing, retain embedded timing and the inclusive cap — open.
- L284: shared missing/empty profile fixtures contain no history, as Swift — open.
- L295: an Antigravity database that can't be read never holds up forgetting the other services' deleted logs — open.
- L326: a new database written to while it's read counts what was read, is read again next round, and never saves $0 — open.
- L367: a busy database read before still moves every round though it's written to during every read, and isn't logged — open.

### `box/test/usage/usage-antigravity-plan.test.ts`

- L24: shared legacy and plan-name parity fixtures — open.
- L29: shared Swift parity fixtures — open.
- L32: exact pools, first usable wins, clamp and Swift rounding — open.
- L38: all three pools retain independent windows in client rows — open.
- L48: Argon is separate and unambiguous — open.
- L54: legacy worst pool, blacklist, missing fraction and Argon exclusions — open.
- L60: the usage account reads its running profile's local quotas when cloud endpoints fail — open.
- L115: Linux /proc maps only own profile and owned listening socket inodes — open.
- L133: Mac own profile can contain spaces and regex punctuation — open.
- L140: Mac agy stand-in permits no CSRF and language_server still requires it — open.
- L148: Linux agy own-profile stand-in permits no CSRF — open.
- L158: Mac ps/lsof stand-ins require own profile — open.
- L168: with no token file, its quotas come from its own running server, and no credential is sent anywhere — open.
- L193: with no token file and it isn't running, it says its usage shows while it runs, and asks no one — open.
- L211: the overview shows heavy use of the tightest available pool without mixing pools — open.
- L256: one host refusing and optional plan refusing cannot poison usable quotas — open.
- L278: legacy project-plan 403 still permits quota retrieval — open.
- L292: agent OAuth metadata and verified defaults are used by refresh-only own sign-ins — open.
- L312: profile and predictable state symlinks cannot redirect refusal writes; signout prunes state — open.
- L334: agy with no CSRF is retried after an authless endpoint 403 — open.
- L347: missing windows without resets survive only one partial round across restart — open.
- L360: refused access refreshes once, valid refresh cache survives expired source expiry — open.
- L380: malformed and unreadable sign-in retain known identity and windows — open.
- L404: access refusal persists through failed refresh and restart without resending old bearer — open.
- L423: unwritable or unreadable refusal journal stops further sends — open.
- L443: restart never sends a refused token returned unchanged by refresh — open.
- L459: refused refresh is not retried after access disappears or changes — open.
- L477: local CSRF refusal survives restart — open.
- L491: persisted refusal survives restart and metadata-only credential rewrites — open.
- L510: holds stop further endpoints; a refusal never resends unchanged credential — open.
- L531: complete read, partial and unreadable reads keep prior known windows — open.
- L560: only the main host's own refusal of the token's call can refuse the sign-in — open.
- L583: a token a summary just answered is never refused by a later call, which still tries the daily host — open.
- L603: a daily host's wait keeps the main host's windows, and only that host waits — open.
- L625: a plan or project lookup's wait keeps the windows read, partial, and holds only that call — open.
- L656: the language server's plan status waiting keeps its windows, and only that call waits — open.
- L676: with every host waiting, the read waits for the soonest, and says whether a request left — open.
- L721: A contradicted Gemini reading keeps the other pool readable until Google counts Gemini — open.
- L765: use before Google's window started, or in another pool, leaves Google's unused windows standing — open.

### `box/test/usage/usage-antigravity-tls.test.ts`

- L9: own loopback language server accepts its self-signed HTTPS certificate and CSRF — open.

### `box/test/usage/usage-codex.test.ts`

- L62: Codex's logs: turns from running totals, replays and repeats left out, tiers and model switches, priced by Codex's rules (UsageKit's CodexLogUsageScanner) — open.
- L67: with a fallback model chosen, a model with no price is estimated at its rates, and the day says so; still an unknown model — open.
- L77: the engine's rows: only those priced at nothing, a message copied between databases once, priced as Codex's (UsageKit's OpenCodeCodexUsageScanner) — open.
- L83: auto-review is priced by its date: the model it ran on then, free from 2026-10-06 (UTC); a snapshot date comes off a model's name — open.
- L159: an account's sessions and archived sessions: a log in both counted once, as its sessions copy; a row by the account, Codex's total — open.
- L183: a log as it grows is read on with its running total, through a restart; one rewritten is read again whole — open.
- L213: on a Mac an added account's history is your own Codex's (its sessions link there): counted once, with yours — open.
- L230: two accounts that link to a folder neither is in (your ~/.codex while CODEX_HOME names another): it's the first's, and each keeps its own — open.
- L252: your own Codex reads each home a comma-separated CODEX_HOME names, as UsageKit did; a home with neither sessions nor archived sessions is read itself — open.
- L277: the fallback model is the box's (and your other computers'): chosen through the route, kept through a restart, a model with no price estimated at its rates; one that isn't a choice is refused — open.
- L339: only a ChatGPT sign-in's rows, priced at nothing, from openai; opencode 2's only from when it signed in; a compaction counts — open.
- L371: a database that can't be read, and none that could: not known now (never none); one the engine left without its -wal and -shm is read as it is — open.
- L386: an API key's sign-in, or none: nothing of it; before opencode 2 took its sign-ins in, the auth.json beside it says — open.
- L400: its row is Codex's: the same ChatGPT account as a Codex account here is one row, both added up; read again only when a database changed — open.
- L456: one database that can't be read while another can: not known now (what was read stays), never kept as whole; one that keeps failing doesn't hold up the rest — open.

### `box/test/usage/usage-cursor.test.ts`

- L51: ${each.fn}: ${each.name} — open.
- L91: usage events: each page's events, its total and those it couldn't read; by the day, priced without long-context rates, by family (UsageKit's CursorUsageEvents and appendSpendLines) — open.
- L109: a page that isn't one, or an event that isn't, as UsageKit takes them — open.
- L142: its meters, its credits and its plan, read with its own sign-in; who it is from Cursor's answer, only as a hash — open.
- L166: its token is kept while it lasts, and exchanged again once when Cursor turns it away — open.
- L178: a key Cursor turns away is a sign-in to renew; a 429 holds off as long as it says; no sign-in, no row — open.
- L193: an on-demand spend with no cap: no window, but what the plan has beyond them (′extras′), on its row across your computers — open.
- L209: a sign-in file that can't be read just now keeps the plan's numbers (tried again soon), not a sign-out — open.
- L222: a request-based plan: its requests as its total, said once; the summary's on-demand — open.
- L238: a test's box never asks Cursor itself, whatever sign-in its folder holds — open.
- L265: filled in a UTC day at a time, page after page; then only today and yesterday asked again, and a day Cursor didn't answer whole — open.
- L294: a day whose later page fails, or whose total moved, keeps what was known of it; one never known can't be had, so the read fails rather than say nothing was used — open.
- L325: when the time runs out, the days that came are kept, and the next read asks only for the rest — open.
- L351: out of time in the middle of a day of many pages: no page after it is asked — open.
- L368: on the box: Cursor's row by the day in the client's zone, by family; a read that fails keeps the last known; a round with nothing new works nothing out again — open.
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
- L604: a key Cursor turned away isn't sent again, nor anything while Cursor holds reads off (the exchange's 429, the events' 429), whatever asks — open.
- L647: a token turned away midway whose exchange is then held off (429): no page more is sent with it; nor any once the plan's read is held off meanwhile — open.
- L672: a 429 from one of the plan's optional reads holds the rest of them, and the history, off; what was read last stays whole — open.
- L694: a token turned away midway whose key Cursor then turns away too: no page more is sent, and its plan isn't asked again with it — open.
- L709: a token Cursor turns away midway through the history is exchanged again once, and its page asked again; two reads as the box starts share one exchange — open.

### `box/test/usage/usage-grok-hosted.test.ts`

- L62: Grok's transcripts: completed turns by model, its own costs where it said them, else priced; a copied event once per model (UsageKit's GrokLogUsageScanner) — open.
- L75: the engine's hosted rows: Go's and Zen's billed costs, both tables, a copy in another table or database once; a key of your own, a cost as text, a running compaction never (UsageKit's OpenCodeUsageScanner) — open.
- L109: an account's sessions' transcripts, only their updates.jsonl: a row by the account, Grok's total; read on as one grows, through a restart — open.
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

- L22: healthy to close fires once — open.
- L23: close to running out — open.
- L24: healthy straight to red fires only critical — open.
- L25: under ten once per window — open.
- L26: cold start records bad baseline — open.
- L27: reset rollover re-arms — open — related accepted-window and notice tests above; this exact case not yet ported.
- L28: reset jitter does not re-arm — open — related accepted-window and notice tests above; this exact case not yet ported.
- L29: recovery then worsening re-fires — open.
- L30: under ten recovery re-arms — open.
- L31: no data never fires — open.
- L32: level fires almost out only — open.
- L33: no data preserves previous signals — open.
- L34: disabled toggles never fire — open.
- L35: per trigger off suppresses only it — open.
- L36: off toggle does not consume edge — open.
- L49: evaluator healthy to close once, facts and route — open.
- L50: evaluator close to red — open.
- L51: evaluator jitter and restart do not duplicate — open.
- L52: evaluator all toggles off — open.
- L53: evaluator per trigger off — open.
- L54: evaluator disabled provider prunes baseline — open.
- L55: evaluator remaining threshold uses remaining regardless display mode — open.
- L56: client delivery failure leaves fact available for retry — open.
- L57: failed read cannot create notices or cache zero — open.
- L58: hidden metric prunes baseline — open.
- L59: pace near empty and projection gate — open — related accepted-window and notice tests above; this exact case not yet ported.
- L60: notices go only to peers in the setup — open.
- L71: large notice history cannot evict the current limits account from peer mail — open.
- L82: turn-only Mac Claude inherits peer window dedup without checked timestamps — open.
- L96: visibility mappings cover model-specific Codex and Antigravity windows — open.
- L102: exact ten percent remaining does not trigger Almost Out — open.
- L108: parallel readers use the same occurrence IDs and recovery receives a new one — open.
- L116: peer handoff timestamps are normalized to the receiver clock — open.
- L128: one direct account holder forwards even a received peer decision — open.
- L142: provider hide and re-enable resets baseline but preserves occurrence counters — open — related accepted-window and notice tests above; this exact case not yet ported.
- L152: notice facts stay in their setup — open.
- L173: setup changes invalidate notice caches while preserving ordinary limit readings — open.
- L185: catch-up is short and warnings disappear when their reset passes — open — related accepted-window and notice tests above; this exact case not yet ported.
- L198: without layout settings only default meters can decide notices — open.
- L208: a decision waits for the directory and pins a forwarder only once — open.
- L224: ordinary turn readings keep the thirty-second disk throttle while new notices persist immediately — open.
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
- L246: three boxes: a change on any reaches the others, the newest change wins on all, part by part; one with a clock behind still counts on from what it has seen — open.
- L287: a box that was off catches up once it says what it holds; settings came from another computer stay through its restart — open.
- L310: settings ignore obsolete directory entries and wait for peer membership — open.
- L353: usage settings mail requires a known matching setup — open.
- L385: a box that leaves your setup for another starts over: none of your settings go there, and its own Mac's come in again — open.
- L427: a client of an older app sends the cards and meters it knows: the box keeps a newer app's — open.
- L449: two boxes of different versions hold the same: a part or field one doesn't know is kept as it came, so they agree and stop telling each other — open.
- L474: a change made from a version of a part that has changed since (another computer's came meanwhile) is turned away, and the client shows the box's; a field a newer box added stays whatever the client sends — partial — settings route scenarios above.
- L496: a box that followed no setup and joins one starts over: its old settings stay local — open.
- L509: a box whose link doesn't say its setup (its keys not read yet, or a link from before setups, met for a moment as the box updates) keeps its settings: nothing's recorded as no setup, nothing goes, and the setup it names next is taken as theirs — open.
- L529: usage settings reject a future disk format without saving over it — open.

### `box/test/usage/usage.test.ts`

- L36: Claude's logs: the lines that count, deduplicated, priced, by the day (UsageKit's ClaudeLogUsageScanner) — open.
- L58: the bundled prices name each model's rates as UsageKit's do: aliases, fast variants, fuzzy keys, models.dev, none — open.
- L106: an account's logs as days in the zone a client asks for: late evening in UTC is the next day in Istanbul — open.
- L142: a log as it grows is read on from where it was; one rewritten shorter is read again whole; the numbers stay through a restart — open.
- L170: a subagent's log replaying its parent's reply counts once; an account removed goes; one with no identity yet is its box's own row — open.
- L189: a client's days are today and the 30 before it, in its zone — open.
- L201: fetched hourly with what it was last sent, used from the next scan on, kept for the next start; a feed that fails or says nonsense changes nothing — open.
- L235: only a real box's bridge fetches them, a Mac app's or a machine's: never one a test started in a folder of its own — open.
- L246: new prices price the next round: a model that was unknown gets its dollars — open.
- L398: one account used on two computers is one row, its usage added up from both; another account is a row of its own; each box lists them all — open.
- L477: Codex's use estimated with a fallback model goes to the others saying so; a box from before, which keeps none of that, finds the same digest and asks nothing again — open.
- L544: an account too big for a mail is cut from its oldest uses and says so; a long name; what changes after costs no more than its last two days — open.
- L574: what the engine's hosted models cost on a machine shows on your Mac as billed, not estimated, in OpenCode's total — open.
- L607: two boxes on one computer reading one folder count it once; a reply written again for each of its parts is kept once — open.
- L634: two computers with one name reading the same path (two Raspberry Pis' /home/pi) are two computers: both count — open.
- L670: refresh asks peers to read now and a restart restores their cached usage — open.
- L715: old directory and usage records do not add an unknown computer to the totals — open.

### `box/test/notices-usage.test.ts`

- L24: queue acceptance is not delivery; a later successful hold send consumes the id — open.
- L42: an unsent fact can retry on the next minute without being marked delivered — open.
- L51: a queued notice does not push after its reset passes — open.
- L63: peer notice is kept for Mac without pushing; source can still push once — open.
- L74: catch-up keeps maker time, has a short age limit, and drops reset windows — open.
- L90: Mac before its hold is ready leaves usage eligible; deliberately disabled phone link consumes it — open.
- L105: partial delivery to two phones retries only the phone whose send failed — open.
- L122: journal write failure retains in-memory marks and retries persistence without throwing — open.
- L142: a usage notification shown by Mac suppresses only its stable id, not another on the same row — open.
- L155: a queued losing forwarder copy is cancelled when a snapshot agrees on another holder — open.
- L168: a catch-up resolving after a reconnect cannot replace the new connection's snapshot — open.
- L181: a newer usage-notices event invalidates an in-flight catch-up — open.
- L196: box phone receipts clamp future dates before keeping them for one day — open.
