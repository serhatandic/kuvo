# One client process per computer

Work Queue #924 · Reconciled with main at `73d327e5d`

Each enabled computer should run one Kuvo box server for its desktop app, SSH clients and paired relay clients. The keeper or Kuvo Helper starts it. A server-only release replaces that process while the engine, agents bridge and agent holders keep their own lifetimes.

## Current paths

| Entry | Current owner and contract |
| --- | --- |
| Local HTTP | The machine keeper starts `serve --kept` on `run/serve.needed`. An own-home Mac's Helper starts `serve` with secrets on stdin, and its shell keeper supervises workers. Both publish `run/serve.json` for password-protected loopback HTTP. A machine-kind Mac uses the keeper's machine launch path. See `box/src/keeper/kuvo-start`, `box/src/link/server/serve.ts` and `mac/helper/Sources/KuvoHelperKit/BoxHelper.swift`. |
| Relay | The keeper starts machine `run` on `run/link.needed`, and pairing/enrollment can start it too. Own-home Mac `serve` already hosts its relay. These owners also handle peer mail, setup, fleet updates, vault and upkeep. The relay service is in this repo's `relay/`. Peer mail and client requests have separate protocols. See `box/src/link/main.ts`, `mac/relay.ts` and `relay/host.ts` under the link sources. |
| SSH | `box route-to` starts per request, authenticates the addressed box and the current pairing with Noise, and can stream raw response bytes. `box route` uses OS-account authority with its existing header allowlist and one-answer framing, including the first NDJSON line. See `box/src/link/server/ssh-route.ts` and `box/src/link/main.ts`. |
| Worker and private interfaces | The engine, agents bridge, session holders and browser keeper keep their private interfaces. The current relay owner holds the vault socket. The Helper host interface handles Mac secrets and permissions. The updater and health judge can outlive a request. |

`box/src/link/server/router.ts` is the route inventory by registration and predicate across HTTP, relay and SSH. It covers box lifecycle and backups, setup and computers, devices and notices, browser and vault, settings and providers, sessions and queues, and work and data. Include its early branches for offline edits, session notes, owner admission and forwarding, local add and setup save, plus the allowlisted engine and agent fallbacks. Setup's supported authority actions include handover, merge and recovery. The relay lease in `box/src/link/setup/lease*` selects the main, the main commits setup saves alone, and members follow verified state. Moves need only source and destination.

Preserve these entry differences when sharing dispatch:

- Local HTTP verifies its password and distinguishes the nearby app and attached desktop. Relay and addressed SSH derive identity from pairing, with pairing checks and revocation at their boundaries. Keep legacy SSH authority and the holder's proven, allowlisted operations separate.
- The Swift Mac app source is gone. Keep 0.7 compatibility through the Mac front for machines, hosts, asks, Mac settings and engine routes. Preserve its callback, fallback and retired-path rules for installed older clients. The machine adapters and own-home Mac box also answer their existing variants.
- Local HTTP upgrades the browser view. Relay supports that view and its allowlisted engine sockets. SSH has response streaming. Preserve each adapter's paths, limits, response framing and cancellation. Preserve the current notice hold and pairing-change behavior per entry during the refactor.

## Decisions

1. Keep `serve` and the private `run/serve.json` discovery contract. Current desktops recognize the command as well as the record.
2. Serve with zero paired clients while enabled. Start outgoing relay and optional work on demand.
3. Keep both SSH verbs and their wire contracts. Pipe their exchanges to the resident server over a private socket. Verify addressed identity and pairing there and retain the alias's existing authority.
4. The first release must preserve active work and accepted operations. Allow a short measured reconnect, using existing feeds and receipts to recover.
5. Keep old CLI, records, routes and feature contracts through the rollback window. Remove split ownership once supported readers and rollback no longer need it.

## Build plan

Start with the current launch paths. Roll out one installation at a time. Estimates are focused wall-clock engineering time including verification.

The step 3 opt-in is `kuvo-link.js serve --resident` on a machine-kind box with the split client processes stopped. It uses the usual `serve.json` and projects its PID into `link.pid` for older readers. Supervisor selection and restart move in step 5.

| Step | Scope and exit check | Estimate |
| --- | --- | --- |
| 1. Freeze contracts | Add parity fixtures for local, relay and SSH. Cover version and role refusals, request and response bytes, NDJSON, held requests, cancellation and upgrade differences. Use the existing one-file route benches. | 3–4 h |
| 2. Share dispatch | Use one request entry and one process context around the dispatcher. Keep transport authentication and Mac compatibility selection at their boundaries. Prove parity with current launch paths and live pairing changes. | 4–5 h |
| 3. Unify background ownership | Bring relay, peer mail, lease and main-only saves/following, vault, notices and periodic work under opt-in resident `serve`. Keep durable setup and receipts and give each job one owner. Test zero pairings, relay loss/recovery, pairing rotation and restart during setup or fleet progress. | 5–7 h |
| 4. Pipe SSH | Carry both stdio contracts over an owner-only socket, including cold bootstrap behavior. Verify target and pairing in the resident process and test revocation, cancellation, streamed bytes and safe reconnect. | 4–6 h |
| 5. Switch supervisors and trial | Use one owner lock and generation-checked readiness and cleanup. Wire keeper and Helper plus pause, stop, rehome and uninstall. Preserve the main's signed prepare-and-follow releases and the atomic `bin` link switch. The independent judge probes live `kuvo/info` in the new generation and keeps worker checks. Prove SSH and relay reachability, failed trial and restoration of the old launch paths after rollback. | 6–8 h |
| 6. Pilot mixed releases with live work | Test old/new main and members in both directions, including lease changes, accepted receipts and direct two-computer moves. Measure reconnect with existing clients. On Mac and Linux record engine, bridge, holder and provider PIDs across update and rollback, and prove active and new turns work. Include shared Codex holders with sandbox enabled and a separate bridge handover run. | 6–9 h |
| 7. Retire split launch | After the pilot and rollback window, audit marker readers and remove obsolete launch paths and demand markers. Repeat mixed-version, repair, downgrade and worker-continuity checks against the supported contracts. | 2–4 h |

No UI change is planned. Keep transport and discovery changes in the box where existing clients can use them. Record any client reconnect gaps found in the pilot.
