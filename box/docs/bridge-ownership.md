# Work Queue 925: coding agents and box services

The bridge owns live agent adapters, holders, turn and permission delivery, the conversation
store, the models and credentials needed to drive those agents, and agent program lifecycle
operations. Reviews, side questions, handoffs, Custom Auto, and group seats start or
continue conversations and belong with the bridge. Their timers schedule agent work. Ordinary
wake-ups and cross-computer delivery already belong to the keeper and link.

| Concern | Owner | Reason |
| --- | --- | --- |
| Plan limits, rings, reset journals, history, pricing, Insights, usage settings and usage peer mail | The link's box services process, in `box/src/usage` | The reader uses credentials, provider endpoints and saved logs. It can read while the coding agents are stopped. The bridge publishes bounded observations from turns and account changes. |
| Artifact index, recording, file copies, restoration and listing | The link's artifact service in `box/src/link/artifacts` | It observes both conversation sources and owns the file records it serves. No live agent state is required to read a saved version. |
| Work Queue GitHub credentials | The link's Work Queue service | GitHub CLI authenticates the Work Queue on that computer. Its device-code attempt can continue while the bridge restarts. |
| Voice vocabulary | The link's voice service | It reads conversations and the project to build the voice transcription vocabulary. |
| Engine hang watchdog | The link's box services process under the keeper | Engine supervision continues when the agents bridge is unavailable. |

The managed agent account roster and catalog transactions stay with the adapter lifecycle. The
reader uses each bridge-confirmed sign-in for its current credential. Provider reads,
backoff and peer state belong to the usage reader. Existing
storage paths and client route envelopes are reused. A private authenticated loopback
rendezvous serves link routes directly and preserves the legacy bridge URLs and event feed.

The installed bridge bundle advertises the ownership split. The keeper can restart the services
with the agents bridge down and defers while an older bridge still owns those stores. Rollback
quiesces the services before replacing the bridge with a legacy bundle. Process records are
excluded from rehome data, and uninstall and release health include the new owner.
