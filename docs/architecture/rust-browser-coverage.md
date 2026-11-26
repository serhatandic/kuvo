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

The 6.4 merged Mac binary also passed engine-sessions 81/81, agent-tools 4/4,
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

### Follow-up 6.4b

The passkey port and all 12 normal-use outcomes in A below now pass on both
boxes on macOS. The passkey case was also run against the previous Rust
binary first and failed on the missing notice.

| Environment | Browser conformance | Chrome processes (cold / listed / browsing / idle) |
| --- | --- | --- |
| macOS, frozen TypeScript | 15 passed, 1 Windows-only skip | 0 / 0 / 9 / 0 |
| macOS, Rust | 15 passed, 1 Windows-only skip | 0 / 0 / 9 / 0 |
| Linux, static musl, lokum (one 1-CPU/1-GB container) | 13 passed, 3 platform-specific skips | 0 / 0 / 11 / 0 |

The final cross-site-frame passkey assertion also passed separately on both
Mac implementations and in the Linux area. Mac Rust engine-sessions 81/81,
agent-tools 4/4, agents 12/12, asks 1/1, workspace check with all test targets,
and the frozen TypeScript check passed. WSL and native Windows were not
rerun; this change uses the existing shared Chrome pipe, and Windows browser
support itself remains row 8.2. No real browser profile was used; the
temporary homes and the Linux container were removed.

## TypeScript case map and gaps

The original cases are grouped by the user outcome, not by the number of
internal assertions. A is normal use, B is deferred (rare), C is superseded.
The browser passkey notice is a separate parity requirement.

### A — normal use (12/12 proven; 0 outstanding)

| Source scenarios | Process scenario in `protocol/conformance/browser.test.mjs` |
| --- | --- |
| `browser-live-view.test.js`: file chosen in the viewer | Real Chrome: chooser, named upload, page reads exactly the chosen filename and bytes. |
| `browser-live-view.test.js`, `browser-review.test.js`: resize and restore | Real Chrome: viewer viewport, agent's 1280-wide viewport, restored viewer viewport. |
| `browser-keeper-version.test.js`, `browser-view-start.test.js`, `browser-reopen.test.js`: handover | Cold view and restored page after resident restart. The native resident saves and reopens pages rather than swapping a raw keeper. |
| `browser-setup.test.js`: failed initial download and backoff | Failed pinned source, persisted retry time, no second request during backoff. |
| `browser-setup.test.js`: wake again after idle with MCP connected | Real Chrome: same MCP process opens a page after Chrome has stopped. |
| `browser-downloads.test.js`: name collision | Real Chrome: both same-name downloads retained with distinct names and exact bytes. |
| `browser-data.test.js`: live site data removal | Real Chrome: remove localhost cookies, retain the other site's cookies. |
| `browser-data.test.js`: integration sign-in transition | Real Chrome: synthetic catalog cookie marks the integration signed in, removal marks it signed out. |
| `browser-view-mac.test.js`: relay refusal | Authenticated desktop relay session gets the Mac's refusal, without starting Chrome. |
| `browser-view-start.test.js`: cold view-only launch | Open the authenticated panel socket while Chrome is stopped; it starts and the page opens. |
| `box/test/own-box-app.test.ts`: updater | Registered app update blocks an opening until the app registers again. |
| `box/test/own-box-app.test.ts`: failed opening | Proved host refuses, second call respects the rest, registration recovers. |

Passkeys (`browser-passkeys.test.js`): real Chrome, model-facing notice,
quiet conditional request, viewer event, late-view hello, asks before and after
the request, a cross-site frame named by the containing page, and clearing
when the page leaves the site. The current desktop client does not render the passkey events or ask flag yet; the resident sends
the same notice contract as TypeScript. Client presentation is separate from
this server rewrite.

### B — later (rare; 12 groups)

| Source scenarios | Reason for deferral |
| --- | --- |
| `box/test/browser-data.test.ts`: full ranking cap and unusual host variants | Ordinary folding, expiry, forgetting and scoped removal are proven; only very large histories and uncommon host spellings remain. |
| `box/test/browser-data.test.ts`: competing first history/preferences edit | Two first edits arriving simultaneously during initial migration are uncommon. |
| `box/test/browser-install.test.ts`: malformed archive corpus and every architecture pin | Pinned ZIP/RPM, checksum refusal and partial download retry are proven; corrupt archives beyond checksum and each individual platform package are uncommon fixtures. |
| `box/test/browser-install.test.ts`: vault fallback, damaged-build repair, cooldown, retirement, multi-download failure | Requires broken installations, concurrent failures or stale builds; initial download, backoff and interrupted-transfer recovery are proven. |
| `browser-tools-built.test.js`: full byte-for-byte lists for both vault states | Shared definitions and actual attributed MCP list/call are proven; exact whole-list comparison is a redundant catch for exceptional vault list regressions. |
| `browser-downloads.test.js`, `browser-download-news.test.js`: frame/opener and unrelated-conversation note isolation | Uncommon concurrent downloads from frames or newly opened tabs; ordinary download and filename collisions are proven. |
| `browser-live-view.test.js`: multiple viewers' geometry and upload edge cases | Multiple devices adjusting the same live page or invalid/oversized files are less common than a single viewer and a valid file. |
| `browser-review.test.js`, `browser-rereview.test.js`: capture quality/recovery, relay congestion, concurrent capture/resize races | Sustained congestion and simultaneous viewers resizing are uncommon; ordinary frames, credit, clock, input and dialog are proven. |
| `browser-view-socket.test.js`, `browser-websocket-review.test.js`: paired-client/fragmentation/backpressure browser-specific variants | Common authenticated socket transport applies; pathological fragmented inputs or simultaneous large writes require separate transport scenarios. |
| `browser-reopen.test.js`: retained session claim and pending site-clear recovery | The page address is restored on restart and offline site deletion is proven; exact lost-process recovery of attribution and pending clear is separate. |
| `box/test/own-box-app.test.ts`: exact time windows, interrupted wait, stale update note, missing bundle | Common update, refusal/rest and successful registration are proven; the exact boundary timing and abandoned launches remain exceptional cases. |
| `browser-passkeys.test.js`: repeated request and ten-minute site flag expiry | The explicit and conditional request, cross-site frame, site flags and navigation clear are proven; exact repeated-loop and expiry boundaries remain. |

### C — superseded (5 groups)

| Source scenarios | Replacement |
| --- | --- |
| `box/test/browser-data.test.ts`: old Mac panel history/preferences take-in | Own-home Mac uses the registered app; machine launch has no old app data to take in. |
| `box/test/browser-install.test.ts`: TS codec-specific cases | Native archive libraries, safe extraction and the pinned installer; malformed archive corpus stays in B. |
| `browser-keeper.test.js`, `browser-keeper-version.test.js`: raw socket/key/keeper versions | Native resident owns Chrome's pipe and serves the authenticated named view and tools relay. |
| `browser-keeper.test.js`, `browser-review.test.js`: exact raw Mac editing-key translations and raw geometry/input | Named view commands and the client's existing platform key mapping, not raw DevTools commands. |
| `browser-review.test.js`, `browser-rereview.test.js`: raw screencast and raw DevTools cases | Native capture, frame credit, named view events and shared frame-policy clock. |

On native Windows the resident's info omits `browser.view`. The desktop's
new-tab menu only offers Browser when that feature is advertised, so a fresh
panel has no unusable Browser control. The install route reports the platform
is unsupported; browser support itself remains row 8.2.

The shared page scripts and frame policy are reused as files. A current
performance cost is the isolated-world lookup and policy evaluation on each
capture (`browser/manager/view.rs`); caching that world for a stable document
is a candidate for consolidation. The original TypeScript browser keeper,
link browser modules and Mac app opener total 6,602 non-test lines and
309,631 characters excluding indentation (before this row). Native browser
and show modules plus net row integration changes after 6.4b total about
7,896 lines and 204,310 characters (0.66 times the replaced text); shared
definitions and page scripts are excluded on both sides.
