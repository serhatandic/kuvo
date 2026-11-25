# OpenCode as one more coding agent

Design, 25 November 2026, at `3e52ff5e2`. The user's decision (11-25): OpenCode is "just another
option", a coding agent next to Claude Code, Codex, Cursor, Grok and Antigravity, not an engine the
box is built around. This page says how the Rust box ([rust-box.md](rust-box.md)) does it.

**Goal: nothing starts OpenCode unless an OpenCode conversation or an OpenCode sign-in needs
it.** Today it starts for almost anything: every `api/…` request except `api/event` and
`api/session/active` (`kuvo_engine/src/proxy.rs:331-334`), the model picker
(`models.rs:648`), new sessions (`start_session.rs:458`), bots and wake-ups (`bot_port.rs:28`,
`wake_port.rs:73`), and every coding agent's turn, which reads the computer's MCP servers through
OpenCode's `/api/config` and `/api/credential` (`kuvo_agents/src/tool_relay/engine_servers.rs:49,109`).
So opening the app starts a 232 MB program even for someone who only uses Claude Code.

## 1. How the box runs it: ACP, one process per computer

OpenCode 2.2.6 speaks ACP. In a scratch home, `opencode acp` answered `initialize` with protocol 1,
`loadSession`, `list`, `resume`, `fork`, `close` and `delete`, image prompts, HTTP MCP servers and
one auth method (`opencode-login`). `session/new` returned an `ses_…` ID with model and mode
choices, and `session/load` on that ID in a new process worked. External editors run OpenCode
the same way, as a plain ACP agent started with `opencode acp`.

- **Adapter.** `AgentKind::OpenCode` in `kuvo_agents` (`accounts.rs:17`), a third flavour on
  4.3's ACP client next to Grok and Antigravity (the TypeScript box has the same shape in
  `box/src/agents/acp.ts`). It runs the pinned binary, never a user's own OpenCode.
- **One process per computer, not one per conversation.** `opencode acp` starts a private
  `opencode serve --stdio` child. Together they used 132 MB + 390 MB RSS before the first turn. One
  ACP connection carries every OpenCode conversation on the computer under one holder, so
  turns survive Rust-to-Rust updates as other agents' turns do. It stops after an idle period, as
  the engine does today (`lifetime.rs`).
- **Kuvo's tools** reach it as an HTTP MCP server from the tools relay (it refuses SSE). Kuvo's
  system prompt goes in the `OPENCODE_CONFIG` file the box already writes (`launch.rs:251-254`).
- **Gate.** A two-hour parity check against 3.2's behaviour comes first. It covers permission and
  question asks, child sessions (`opencode/child-session-updates`), `/compact`, usage and cost,
  model variants, images, steering and stop. If ACP falls short, the same adapter talks to
  OpenCode's HTTP server instead, reusing 3.2's request code. The plan and routes below are the
  same either way.

## 2. The app's `api/…` routes

The app picks a conversation's prefix from the box's answer to `kuvo/sessions/:id/location`
(`Where`, `gpui/kuvo_box/src/messages.rs:20-45`), and a background command's prefix from its
`agents` flag (`background.rs:135-137`). When the box answers `agents` for OpenCode
conversations, the app follows with no change. The app never calls `kuvo/engine/*`.

| Route the app calls | Where | After |
|---|---|---|
| `GET api/session/:id` | `session_context.rs:82` | Agents: `kuvo/agents/api/session/:id` |
| `GET api/session/:id/message?…` (pages) | `messages.rs:429,434` | Agents |
| `POST api/session/:id/model` | `models.rs:515` | Agents |
| `GET api/session/active` | `follow.rs:660` | Agents |
| `GET api/event` (stream) | `messages.rs:44`, `follow.rs:796`, `line.rs:555` | Agents feed `kuvo/agents/api/event` |
| `GET api/shell/:id/output` | `background.rs:137,182` | Agents (`kuvo/agents/api/shell`, as Codex's) |
| `POST api/pty`, `PUT`/`GET`/`DELETE api/pty/:id` | `pty.rs:228-259` | Box-owned, same path and wire, box's own terminals |
| `GET api/pty/:id/connect` (socket) | `pty.rs:269` | Box-owned, same |
| `GET api/integration` | `accounts.rs:437` | Box-owned. Only for boxes from before `kuvo/accounts/catalog`, answered from the cache |
| `POST api/integration/:p/connect/key`, `…/connect/external` | `accounts.rs:540,551` | Box-owned path, handled by the OpenCode adapter (starts it: a sign-in) |
| `POST …/connect/oauth/:a/complete`, `DELETE …/connect/oauth/:a` | `accounts.rs:514-528` | Same |
| Anything else under `api/…` (the passthrough) | `kuvo_engine/src/lib.rs:130-134` | Goes, except for released Macs (decision 2) |

OpenCode's provider IDs (`anthropic`, `openai`…) can't move under `kuvo/agents/api/integration/:p`,
whose IDs are agents (`claude-code`, `codex-cli`, `accounts.rs:55-62`). So those paths stay.
Compact and shell commands already go through box-owned `kuvo/sessions/:id/…` routes. Only the
app's in-process stand-in answers `api/session/:id/compact|shell`. Later, once no box answers
`api/session`, the app can drop `Where::Engine` and `Side::Engine`. That is a small cleanup, not
part of the switch.

## 3. What stays outside the agent

- **Terminals.** The box gets its own PTY: `kuvo_os::pty`, with `openpty` on Unix and ConPTY on
  Windows. `kuvo_services` serves `api/pty` with today's paths and frames, and the cases from
  `box/test/pty-socket.test.ts` ([coverage](rust-engine-coverage.md)) must pass. This replaces
  rust-box.md's "No PTY crate: terminals are the engine's".
- **Sign-in and models.** API keys, OAuth providers and OpenCode Go are OpenCode's accounts,
  handled by its adapter. Connecting and disconnecting start OpenCode; listing does not. The model
  list stays `kuvo/models/offered`, merged from each agent's catalog (`AgentModelSource`,
  `kuvo_engine/src/agent_models.rs:12`). OpenCode's catalog is a cache the adapter writes whenever
  OpenCode runs, from `session/new`'s model choices and after a sign-in. On a computer where it
  never ran, the picker shows OpenCode as an agent to set up, like any other.
- **MCP servers for other agents.** The box reads `box/opencode.json` and each project's own file
  itself, instead of asking OpenCode (`engine_servers.rs`). OAuth for a remote MCP server still
  goes through OpenCode, which starts only for that sign-in (decision 3).
- **Bun.** Cursor's sidecar, package installs (`runtime_packages.rs:33,139`) and the released-Mac
  stand-in run the pinned OpenCode binary as Bun (`BUN_BE_BUN=1`). Any of the three downloads the
  pin on first need, through the agents' install route (`kuvo/agents/api/integration/:id/install`).
  `kuvo_keeper::provision_pinned_engine` (`engine_pack.rs:38`) is exported (`lib.rs:24`) but
  never called. It becomes that install step, or it goes.
- **Usage and limits** stay in `kuvo_services` (5.4). OpenCode's usage and OpenCode Go limits are
  read from its database without starting it (`box/src/usage/usage.ts:943`,
  `engine-limits.ts:65`). ACP usage events from OpenCode turns aren't counted again.

## 4. Existing OpenCode conversations

Nothing is copied, rewritten or deleted in OpenCode's database
(`~/.kuvo-code/data/opencode/opencode.db`). The switch's `backups/before-rust` also holds it.

- **Listed** without starting OpenCode. The box already reads that database read-only, in both
  schemas (`session_list/engine_facts.rs:16-17`). That reader becomes the adapter's inventory, and
  rows say "OpenCode" and name their computer.
- **Same IDs.** An OpenCode conversation keeps its `ses_…` ID, which is also its ACP session ID.
  Queues, send-once receipts, wake-ups, bots, Work Queue links, group seats, ownership and moves
  are keyed by that ID and carry over untouched.
- **Read.** On first open, the adapter takes the transcript once from `opencode session export`
  (OpenCode's own message format, which the app already draws) into the agents' store. It checks
  message counts before using the import. Until then, or if the check fails, it reads the export
  directly.
- **Continued** with ACP `session/resume`, or `session/load` (tried in the probe). OpenCode
  still has the full context.
- **Moves** between computers become OpenCode's own payload in 4.6 (`session export`/`import`),
  replacing the engine half of 2.4 (`session_move.rs`).
- The golden homes in 9.1 and the 9.3 rehearsal check that every old conversation is listed,
  readable and continuable.

## 5. Rows

- **New 4.7, OpenCode as an agent**, after 4.3 and before 4.6, so carry, handover, snapshots
  and moves cover OpenCode like the rest. About 24 hours, lane B:
  - 4.7a: parity spike, ACP or HTTP (2 h);
  - 4.7b: adapter, one held process, catalog cache, sign-in paths, MCP config read by the box, and
    bots and wake-ups pointed at it (`bot_port.rs`, `wake_port.rs`) (6 h ×2);
  - 4.7c: box terminals (5 h, lane D, any time after 1.3, before 4.7d);
  - 4.7d: existing conversations, then remove the engine place: every `Place::Engine` branch
    (`queue/target.rs:17`) and the start triggers in section 1 (5 h).
- **4.3** also has its ACP client carry several sessions on one process (+1 h).
- **8.2** "native engine" becomes the OpenCode agent on Windows.
- **9.0** deletes what's left: `launch`, `lifetime`, `proxy`, `api`, `release` and `model_refresh`
  in `kuvo_engine` (2,425 lines), and renames the crate `kuvo_sessions`. Its queues, receipts,
  delivery, asks, search and list from 3.2 aren't specific to OpenCode and stay.
- **9.1** keeps its plan. OpenCode's data needs no step of its own.
- **Dead from done work:** 3.1's passthrough, on-demand lifetime and engine adoption; the engine
  half of 2.4's moves; `engine_servers.rs` (241 lines, rewritten). 4.1, 4.2 and 4.4 stay as they
  are. The TypeScript bridge's `EngineAgent` (`box/src/agents/engine.ts`, 821 lines, which runs
  engine models inside an agent conversation) is never ported: switching models is a handover.

## 6. What gets simpler

- **One conversation path** instead of two: one feed, one prefix, one `Place`. No `api/…`
  catch-all.
- **Routes the app calls under `api/`**: 14 today, all passed to OpenCode. After: 6 answered by
  the agents surface, 8 box-owned at the same paths, 0 passed through.
- **Starting the app** doesn't download or start OpenCode. That saves about 520 MB RSS (measured)
  for people who don't use it.
- **Lines**: about 2,400 removed plus the branches in queue, asks and starts. About 1,900 added
  (adapter about 800, terminals about 700, import about 400). The gain is in what runs and in one
  path to test, not in lines. Crates stay at 14.

## 7. Risks and decisions

Risks:
- ACP may give less detail than OpenCode's own events (part kinds, cost per step, question asks).
  The 4.7a gate decides, and the HTTP adapter is the fallback.
- Every OpenCode conversation shares one process, so a crash stops them all. The holder restarts
  it and the resume picks up.
- Each OpenCode upgrade must still match the database reader and the export format.

Decisions for the user:
1. **Bun.** Keep the 232 MB OpenCode binary as Kuvo's Bun for Cursor and the released-Mac
   stand-in (one pin, already proven), or add a separate, smaller Bun pin. Recommended: keep it
   for 0.8.
2. **Released 0.7.x Macs.** If they reach a machine's OpenCode through `api/…`, which 7.3 must
   confirm, keep the passthrough only for them, behind a compat marker, until none connect.
   Recommended.
3. **MCP server OAuth.** Keep it in OpenCode for 0.8, which starts it only for that sign-in, or
   move it into the box. Recommended: keep it in OpenCode for now.
