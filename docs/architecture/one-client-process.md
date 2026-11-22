# One client process per computer

Work Queue #924 · current at `36db4ec2e`

Each enabled computer has one resident box server for local HTTP, SSH and paired
relay clients. The keeper or Kuvo Helper starts it. The engine, agents bridge,
agent holders and browser keeper have their own lifetimes.

| Entry | Current owner and contract |
| --- | --- |
| Local HTTP | `serve --resident` publishes the password-protected loopback address in `run/serve.json` after HTTP and SSH are ready. |
| Relay | Resident owns paired client sessions and peer mail, the lease, setup saves and following, fleet progress, vault and upkeep. The relay service is in `relay/`. Peer mail and paired sessions use distinct protocols. |
| SSH | Both keeper verbs pass requests to resident over an owner-only socket. `route-to` authenticates the addressed box and pairing with Noise and streams response bytes. `route` keeps OS-account authority, the header allowlist and one-answer framing. |
| Private workers | The engine, bridge, session holders and browser keeper keep independent interfaces. Kuvo Helper serves Mac secrets and permissions. |

See `box/src/link/server/{resident,serve,ssh-pipe,ssh-route}.ts`,
`box/src/keeper/kuvo-start` and `mac/helper/Sources/KuvoHelperKit/BoxHelper.swift`.

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

## Launch and rollback

The release selected by `bin` determines the launch. The keeper and Helper
start resident for a release that advertises it. Resident locks its home, writes
a generation to `serve.json` and projects its PID in `link.pid` for older
readers. The readiness judge checks live `kuvo/info` against the selected build
and generation. The private SSH selection persists across restart, preventing an
interrupted request from falling through to one-shot dispatch.

A rollback to an older release restores split `run` and `serve --kept` on
machines (including machine-kind Macs) or legacy Helper `serve` on an own-home
Mac. The older demand markers and both SSH wire contracts remain readable.
A launcher without keeper support can still take GPUI's older SSH fallback.

## Remaining

Steps 1–5 landed: parity fixtures, shared dispatch, background ownership,
both SSH pipes, and the keeper and Helper cutover.

6. Finish mixed-release pilot checks in both directions on Mac and Linux:
   reconnect, lease changes, accepted receipts and two-computer moves, trial
   rollback, and engine, bridge and holder continuity across active and new turns.
   Include a shared sandboxed Codex holder and a separate bridge handover.
7. After the pilot and rollback window, audit marker readers and retire split
   supervision and demand markers. Repeat repair, downgrade and continuity checks
   against supported releases.
