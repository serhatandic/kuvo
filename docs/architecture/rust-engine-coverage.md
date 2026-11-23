# Rust engine coverage

Phase 3.1 runs OpenCode from the installed pin, owns its process, and routes
the engine's API through the resident. The stand-in scenarios are in
[`kuvo_engine/tests/resident.rs`](../../box/crates/kuvo_engine/tests/resident.rs).
They use the real resident, dispatcher, process launcher, discovery file and
WebSocket bridge. The real-engine smoke exercises the same entry points with
OpenCode 2.2.6 on macOS, Linux, WSL2 and native Windows.

## TypeScript reference scenarios

| Source | Cases | Rust coverage |
| --- | ---: | --- |
| `box/test/engine-lifetime.test.ts` | 1 | Cold API demand, concurrent demand, passive event/active reads, busy deferral, parking and wake. Creating a conversation through `kuvo/sessions/new` follows in 3.2. |
| `box/test/pty-socket.test.ts` | 3 | Local password, engine password, directory and query, text and binary in both directions, client close code, and upstream refusal. |
| `box/test/engine-context.test.ts` | 1 | Latest context measurement and summary invalidation belong with session reads in 3.2. |
| `box/test/engine-swap-inboxes.test.ts` | 2 | Pending inbox pagination, incomplete inventory and interrupted ordered transfer belong with queues in 3.2 and release recovery in 7.2. |
| `box/test/engine-own-tools.test.ts` | 4 | Tool registration, compatibility settings, old app-folder exclusion, and generated launch tools follow in 4.1, 6.1 and 7.3. |
| `box/bridge-tests/own-box-engine.test.js` | 3 | Installed-pin startup and adoption cover the worker lifecycle. Helper launch notes, normalization of the old shell launcher’s HOME, and keeper configuration refusal follow in 7.1 and 7.3. |
| `box/bridge-tests/start-session.test.js` | 5 | Session ancestry, marking/refusal, worktree admission, new worktree filing and sibling-folder filing follow in 3.2 and 6.3. |
| `box/bridge-tests/engine-swap.test.js` | 4 | Held turns and ordered sends, rollback after bad launch, bounded hold expiry, and the downgrade fence are the old engine swap. The replacement release paths follow in 3.2 and 7.2. |
| `box/bridge-tests/update-engine-turn.test.js` | 3 | A resident restart keeping the engine is covered. A full release switch with a live turn, waiting on a background shell at release switch, and a bridge turn finishing through handover follow in 4.1 and 7.2. |
| `box/bridge-tests/bridge.test.js`, API forwarding and path vectors | 2 | Authenticated body and header passthrough, a forged client password, refused paths, event streaming/cancellation and all 100 forwarded engine path vectors. Phone message parts and named-agent rewriting, integration enrichment and agent-bridge routes are gaps. |
| Same file, engine unavailable and info timestamp | 2 | Passive and stopped status, retry wording, and the discovery timestamp surfaced in `kuvo/info`. |
| Same file, Start/Restart and update admission | 2 | Install-stage rejection, download-stage admission, an older Stop marker, and typed Start/Restart. Shell-launcher and bridge survival assertions follow in 4.1/7.1. |
| Same file, engine restart using a Mac’s settings and info naming its last launcher | 2 | The installed pin and own settings path replace the old start command. Mac-supplied launch settings and attribution follow in 6.1/7.3. |
| Same file, paired terminal | 1 | Paired caller and path admission are shared with the local transport. Noise request IDs, socket frames and cancellation still need the paired resident transport. |

This maps 35 reference scenarios. The local PTY tests and engine unavailable
case have direct counterparts. The lifecycle, raw API, timestamp and control
cases have coverage at the Rust boundary, with the route-specific gaps
above. Session/context, tools, handover and the old swap cases still need
their owning phases. Model-folder refresh tests use the same engine activity
endpoints, but refresh itself is part of 3.2. The sandbox worktree test
belongs to 6.1.

## Differences in implementation

A Tokio task owns the engine, shares one cold start among requests, and runs
idle probes independently of normal requests. Request leases pin active
work. Event streams stay passive. Only the task commits a park, after
checking that the process and activity observed by its probe are current.

Launch uses the installed OpenCode binary directly. It writes the compatible
discovery record atomically and adds the process start identity. If the
resident is interrupted, the next resident verifies the executable and
identity and checks authenticated HTTP readiness before giving out the
address. Legacy records can be forwarded to, but the resident only stops
an engine whose executable it can identify as its own. Unix uses the process
seam for process groups and adoption. Windows associates the worker with a
recoverable Job Object.

HTTP streams the response and keeps the existing header allowlist. Upstream
redirects stay on the engine's origin. The local WebSocket and future paired
transport use the same dispatcher admission and bounded message channels.
