# Rust box timer inventory (4.8)

The timer counter is in `kuvo_os::timer_fires`. The reporter is debug-only,
request-driven, and installed only in the isolated measurement fixture. Count a
completed branch, not a registered/cancelled deadline. The release build has no
counter or signal handler. `scripts/measure-idle-box.mjs` measures the resident
and, separately, the keeper and resident together. We also follow the hot lists
on one authenticated line. The platform filesystem fallback (notify's polling
watcher where the OS cannot deliver a change) is not an application timer.

A permanent interval that runs with no actual work is not permitted. The cases
below are classified by the reason they need a wake-up, not by the syntax of
`interval`, `sleep`, or `sleep_until` in the implementation.

## Resident owners and followed views

| Owner | Trigger when nothing new arrives | Why it is needed |
|---|---|---|
| Agent discovery; catalog; account status | None | Read native installation, credential identity and cached list; process starts only for an actual turn/sign-in. |
| Agent program maintenance | Daily; Agents settings opens immediately | A scheduled update check; unchanged executable stamps do not run the agent's version command. |
| Agent sign-in attempts | Actual retained-attempt expiry | No entries: no timer. |
| Codex / ACP processes | Actual last-use deadline, only for a quiet process | Attached/busy routes or sign-in changes notify the owner; a process exit/retirement is an event. |
| Native agent holder | Orphan / stop escalation / output drain / ended retention only | Native wait for exit and stop-file watching; reader channels deliver output. Unsupported native process wait uses an active-process fallback. |
| Native agent relayed command servers | Actual process idle deadline | The local MCP command's connection usage and exit change the deadline. No server: no timer. This is not the OpenCode runtime's Kuvo tools server. |
| Session inventory | None | Engine/agent updates and file notifications. A missed file notification is explicit recovery, not 60s inventory. |
| Engine supervisor | Actual process idle/health/recovery deadline | No worker and no recovery: no timer. Native exit waits; start readiness is an active bounded probe of external OpenCode. |
| Settings and archives | None | Configure/store, setup and session events instead of 1s or 10s rebuilds. |
| Setup lease | None for sole computer; renewal/expiry deadline with members | Fenced shared leadership. Join reservation and outstanding setup actions acquire the shared lease first. |
| Keeper | Restart/trial deadline only | Native child exit, stop signal/socket and durable stop/selection changes. |
| Updates | Scheduled feed deadline and real in-flight release work | Active trial/progress/retry phases only, including real lost/recovering peer. |
| Work Queue exchange | Actual outstanding reservation/receipt/handing/retry | Owner files, peer/retell/setup/session changes. Unchanged acknowledged copies not resent. |
| Work Queue follow | Actual date/peer standing/visible GitHub reading | Date is part of this answer even if empty; GitHub has no general push of the selected query. |
| Remote work | Outstanding calls/rows/answers and retention only | Call channel, peer changes and durable records; no read of the engine for an empty setup. |
| Moves | Individual actual transfer/hold/cleanup/retention deadlines | Journal, command, engine and relay events. The fast part acknowledgement drives the current transfer without a full journal inventory. Peer move stage announcements have a real one-minute compatibility deadline only while moving. |
| Durable delivery | Actual unresolved I/O or remote contact retry | Initial recovery, outbox/receipt and ownership/engine changes. Held/known away owner waits for actual routing change. |
| Held bot edit | Actual lost reply / retryable refusal | Edits, peer return, ownership and grants; no 15s scanning. |
| Engine queue limits | Actual saved queue reset/retry | Queue files and engine state; no 60s check of an empty queue. |
| Late work answer | Actual untold answer retry | Change to asks, routing or engine state; no empty five-minute scan. |
| Peer waiting | Actual age-out of a kept asking activity copy | Roster/reach/copy changes and one relevant age boundary. |
| Wake-up owner | Next due wake or active user-defined watch check | Scheduled user work, plus folder changes. No default 2s round. |
| Approvals | Actual retry or engine-start grace | A request or answer is an event; no empty 2s scan. |
| Letters | Actual undelivered mail/reply retry | Outbox, peer and relay change. No empty 3s round. |
| Marketplace | None when no installed item | Install, vault, settings and folder changes. |
| Library and artifact store | None on pristine idle | Store first actual use (including a real before-edit baseline or historical artifact recovery), stable change subscription, file/record/citation changes; peer copy contents hashed, no unchanged pushes between new boxes. Existing legacy copy freshness is a wire compatibility rule. |
| Library/Outputs follows | Actual peer-copy age; direct query fallback only for old peers without snapshot feed | Local entries, peer changes, and watched item/source files. Artifact event coalescing is only after new work arrives. |
| Background list / work / bots / waiting / Helper / browser-asks / wakes | Actual row expiry or date/state boundary | Native files, session, roster and publisher changes. Browser's exact PID exit removes stale asks; no changes to browser download. |
| Queue/receipt/ask follows | Actual reset-display date or failed read retry | Durable queue and receipt files, engine inbox/asks/session updates. No fixed 1–2s refresh. |
| Command output follow | Only an actual live Claude command whose output is available via `get_task_output`, not pushed | Codex/foreground output changes notify; non-agent file/list changes and ended commands need no timer. |
| Git follow | Debounce a real worktree or Git metadata change; retry a failed watcher only | Native worktree and resolved index/HEAD/ref watchers; shared watch registry handles filesystems without native notifications. |
| Keep Mac awake | Retry a failed actual caffeinate launch | Settings/local work change; native child wait. |
| Vault SSH agents | Actual registration idleness/retry | Registration file change, accepted connection and listener exit. |
| Daily backup / retained engine backup / skill trash | Actual daily configured backup or retained-item expiry | Missing/disabled/empty means no deadline; real failed work retries. |

## Deliberate transport and active-only waits

These are actual protocol traffic or a bounded caller/ongoing-operation wait,
not background idle scanning. They must not be removed by an idle optimization.

- `kuvo_peers/relay.rs`: relay keepalive and reconnect backoff. Even a single
  computer may receive remote clients or an invitation, so the wire stays open.
- `kuvo_peers/setup_lists.rs`, `library_artifacts.rs`: live peer copy heartbeat
  only while connected members exist; the old wire peers age copies. Active
  move notice expiry and paired-trust grace are actual deadlines.
- `kuvo_server/line.rs`, `local.rs`, `serve/agents/rows.rs`: authenticated line,
  streaming HTTP and SSE heartbeat/retention; per-client only. Line replay
  reconnect is a bounded attempt. The keepalive interval is part of the client
  contract.
- `usage_peers.rs`, `usage_history_peers.rs`, `usage/limits.rs`, `usage/insights.rs`:
  usage owner's actual connected peer digests, grant/change/away expiry,
  rate-limit reset and demand deadlines, connected-client Cursor HTTP refresh,
  subscribed SSE heartbeat and externally scheduled price lookup. No timer for
  usage peer work when no reachable members; the usage owner retained its own
  tests and policy.
- Installer, SSH adding, shared numbering forward, backup relay, release and
  keeper handover: actual joining/number/backup transaction resend/timeout,
  launch/acknowledgement/old process stop, with no loop if no such transaction.
- Move data streams have real in-progress transfer keepalives. File locks,
  bounded stop/settlement waits and external command startup have local retry
  or timeout deadlines only inside those calls (`session_move`, `wake_port`,
  `notes`, `transcript`, `model_refresh`, `work/watch/stopping`, `wakes/routes`,
  `queue/github`, `bots/edit`, `bots/held`, `bots/own_keys`, `work agent tools`,
  `server/ssh`, `keeper/mac_helper`, `keeper/windows`, `install_released`).
- Native agent RPC acknowledgements, turn deadlines, uncertain steer receipts,
  model-tool package/check retry, and active terminal list reads are scoped to
  actual turns, pending calls or an active terminal where the agent has no
  ended-list push. ACP's actual record completion deadline and Claude's command
  output reads are separate adapter constraints, not idle work.
- Vault autopilot recheck is within an outstanding approval of a compatibility
  engine/bridge identity; the external reader has no subscribed identity stream.
  It ends with the approval, not at resident startup. Vault ask patience is a
  real request deadline. The authorization decision is unchanged.
- Work catch-up performs bounded recovery for unread persisted work at startup;
  holding/settling a real child turn, late prompt receipt and engine background
  age boundaries are actual work. WatchHub's 500 ms grace expires a released
  watch once, not periodically. A failed native watcher gets a recovery deadline;
  a working watcher does not.

## Background work at low priority (4.9 handoff)

The common platform-owned process launch seam is `kuvo_os::process::Spawn`,
including resident and recoverable process groups/jobs. Native agent launch
enters via `kuvo_agents::hold::Directory::start` and the holder worker; OpenCode
enters via `kuvo_engine::launch::Worker::launch`. Native-agent relayed command
servers, explicit program maintenance (`kuvo_agents::program_updates`), and
Git subprocesses are separate callers. The holder itself and the user-visible
agent turn must remain distinct from optional background commands: the caller
knows which is foreground or background, and the OS seam knows how to set the
priority. This row changes when they start, not their priority.
