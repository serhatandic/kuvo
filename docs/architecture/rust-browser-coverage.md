# Rust browser coverage (6.4)

The resident owns Chrome for Testing on machine-kind computers. It serves the
browser and show tools through the existing tools relay; there is no separate
Bun browser MCP or keeper process. Chrome's DevTools connection is a child
pipe, never a network debugging port. Clients use the named browser view
commands on the authenticated socket. An own-home Mac still uses its
registered app's browser tools (and opens only that app if necessary).
The native Windows browser remains unavailable, as in the TypeScript box.

## Process proof

The `browser` conformance area (`protocol/conformance/browser.test.mjs`) runs
against both implementations. Its Chrome case uses a local page server, a
read-only executable build, a new disposable profile and synthetic answers.
It checks the following through the actual binary:

- Independent settings, history folding and icons, scoped site data removal
  from an extensionless SQLite cookie database, and the show tools.
- Tool listing/attribution through MCP; pinned ZIP/RPM installation, SHA-256
  refusal, partial download retry (range resume in Rust), one installation
  with concurrent requests, and no Chrome while installing.
- Real Chrome reads a page, returns its numbered snapshot and text, clicks,
  types, scrolls, waits, prints a PDF, captures a PNG, uploads a file, downloads
  bytes, changes and closes tabs, and accepts synthetic sign-in and code
  answers via the existing asks routes without returning the values.
- The view's WebSocket sends distinct JPEGs and accepts frame credit,
  navigation and mouse input; a synchronous page dialog is answered while its
  input is outstanding. The shared frame-policy clock is also tested without
  sleeps. Opening a resident and listing tools start no Chrome; Chrome exits
  after the tools and viewer leave.
- Mac app authority: a proved stand-in host opens the registered browser
  bundle once for simultaneous requests, never opens the desktop bundle as
  the browser, cannot replace a newer registration with a stale report,
  and forwards the actual MCP call. A Mac has no machine view.
- Windows reports the unsupported browser installation state and starts no
  Chrome.

| Environment | Browser conformance | Chrome processes using fixture profile (cold / listed / browsing / idle) |
| --- | --- | --- |
| macOS, TypeScript | 11 passed, 1 Windows-only skip | 0 / 0 / 9 / 0 |
| macOS, Rust | 11 passed, 1 Windows-only skip | 0 / 0 / 9 / 0 |
| Linux, static musl in one 1-CPU/1-GB container | 10 passed, Mac- and Windows-only cases skipped | 0 / 0 / 11 / 0 |
| WSL Ubuntu 24.04, static musl | 9 passed, 3 skipped (no Chrome executable fixture, Mac and Windows) | Not measured |
| Native Windows x64 | 7 passed, 5 platform-specific skips | No Chrome |

The merged Mac binary also passed engine-sessions 81/81, agent-tools 4/4,
agents 12/12, asks 1/1 and terminals 7/7; workspace check with all test
targets and the TypeScript check passed. The terminal scenario protects the
shared process seam when starting either a terminal or Chrome. The Windows
unsigned executable imports `libunwind.dll`: the test payload must carry
that adjacent runtime DLL, exactly as the release workflow and pack
assembler already do. The bare executable alone failed to load. All test
homes and child processes were removed.

For the same copied seed and request sequence, settings, history and sites
answers and the browser files match, including the SQLite schema and rows.
The differential tool treats Chrome's two extensionless Cookies locations
as SQLite databases rather than treating transaction-header bytes as data.
The full-home comparison still reports unrelated resident startup, peer and
usage files. An engine refiling read-count failure was seen once during the
installer slice and passed its immediate exact rerun; its diagnostic is kept
in the scenario.

## TypeScript case map and gaps

“Covered” below means asserted by a process scenario, not just that a
corresponding implementation exists. The legacy raw keeper socket and its
raw DevTools interface are not offered by the native resident; that removes
the separate browser MCP process and raw-socket-specific tests, not the
client-facing browser commands.

| Source scenarios | Native proof / remaining gap |
| --- | --- |
| `box/test/browser-data.test.ts`: history fold, cap/rank, icons, forget/clear; settings and Never; cookie expiry, site names and offline removal | Covered: settings/history/site data scenarios. Ranking limits and unusual host variants partially covered. The old Mac panel history/preferences take-in and competing first edit cases remain open (the current Mac uses the registered app). |
| `box/test/browser-install.test.ts`: XZ/RPM/ZIP parsing; size/link/path checks; pins and test sources; background install; checksum/size/network failure, fallback, repair, locks and data preservation | Covered: actual pinned ZIP/RPM fixture, checksum refusal, one in-flight job, interrupted transfer and retry without a second copy; no Chrome during install. Native library codecs and extraction checks replace the TS codecs. The full malformed archive corpus, every architecture pin, vault fallback, repair/cooldown, retirement and multi-download failure scenarios are still unproven individually. |
| `browser-setup.test.js`: first tool wakes and idles, no network, fresh machine | Process proof for installer and real Chrome, including idle exit. Separate network/backoff and MCP-connected wake-again scenarios not asserted yet. |
| `browser-tools-built.test.js`: closed/open vault tool list and scripts | Shared generated definitions and scripts; native MCP list and attributed tool proof. Full byte-for-byte closed/open vault list variants not separately asserted. |
| `browser-keeper.test.js`, `browser-keeper-version.test.js`: raw key, version, old keeper/replacement, keyboard mappings | The native renderer route and tools relay replace the raw socket/key/keeper version cases. Idle stop and page reopen are implemented; full browser handover during an update is unproven. Exact Mac editing-key translations remain open. |
| `browser-data.test.js` (bridge): live history/icon, site data and integration signed-in | Real history visit and offline cookie/site coverage; isolated Chrome resource fetch for icon and live removal/sign-in metadata implemented. Separate live site removal and integration sign-in transitions unproven. |
| `browser-downloads.test.js`, `browser-download-news.test.js`: attribution, frame/opener, user downloads, own-conversation notices | Actual downloaded bytes and final filename tested; attribution, frame/opener and per-conversation note delivery and artifact recording implemented. Further collision/attribution isolation scenarios unproven. |
| `browser-live-view.test.js`: agents and viewers, resize, input, chosen file bytes | Real named view, frame credit, navigation and mouse/dialog; agent file upload covered. Native resize gate/restore and viewer upload implementation; multi-view geometry and viewer upload edge scenarios unproven. Raw view input is not offered. |
| `browser-review.test.js` and `browser-rereview.test.js`: still/dirty/floor/identical images, resize ordering, quality/retry recovery, raw geometry, concurrent input and navigation, dialog/Stop, external URLs, page facts, uploads, multiple viewers | Shared clock trace and real distinct images, credit and dialog proof. Ordered ordinary actions and separate urgent dialog/credit/Stop processing, bounded channels, capture classes, geometry serialization, page facts and idle restoration implemented. Full relay congestion budget/quality recovery and multi-view capture/resize races remain open; raw screencast and raw DevTools cases no longer apply. |
| `browser-view-mac.test.js`, `browser-view-start.test.js`: Mac refusal and opening a machine view | Mac HTTP refusal and registered-app proof; real machine view and Chrome idleness. Separate relay Mac refusal and cold view-only launch scenario still unproven. |
| `browser-view-socket.test.js`, `browser-websocket-review.test.js`: auth, route/version, binary frames, large/chunked writes, backpressure, relay budget/drop, typed dialogs | Common authenticated socket transport and exact named view route; actual JPEG/credit/typed dialog proof, 72 MiB message limit. Browser-specific paired-client, fragmentation, backpressure and full relay budget scenarios still need coverage. |
| `browser-reopen.test.js`: reopened pages retain conversation and pending site clears | Reopen records and pending-clear implementation present; restart and pending-clear scenario tests remain open. |
| `browser-passkeys.test.js`: passkey request notices, site/ask flags and clearing | Not ported: dedicated unsupported-passkey notification and ask flags. |
| `box/test/own-box-app.test.ts`: one opening, updater, failure rest, gone app | Mac proved-host/concurrent open/stale report and tool forwarding scenario; remaining time-window and update/failed-open cases unproven. |

The shared page scripts and frame policy are reused as files. A current
performance cost is the isolated-world lookup and policy evaluation on each
capture (`browser/manager/view.rs`); caching that world for a stable document
is a candidate for consolidation. The original TypeScript browser keeper,
link browser modules and Mac app opener total 6,602 non-test lines and
309,631 characters excluding indentation (before this row). Native browser
and show modules plus net row integration changes total 7,684 lines and
198,327 characters (0.64 times the replaced text); shared definitions and
page scripts are excluded on both sides.
