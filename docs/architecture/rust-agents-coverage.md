# Rust agent foundation coverage

Phase 4.1 is split into holder transport (4.1a), resident runtime and accounts
(4.1b), and tools and sidecar (4.1c). The five vendor adapters are 4.2–4.5.
The OpenCode process stays under the engine supervisor. Its agent tool entry
and the model and session projections share the resident routes.

## Holder transport

| Case | Boundary and outcome |
|---|---|
| H1 | [Agents conformance](../../protocol/conformance/holder.test.mjs). A successor replays an active turn, receives an acknowledged open ask again, answers once, and takes the same child to its end. Wrong keys and older generations are refused. The old controller is fenced. The finished holder removes its journal and record. |
| H2 | A five-megabyte line and the following line reach the current controller with a tiny journal. A complete replay request for the pruned range is refused, and the current controller can still write. |
| H3 | The live controller receives a control request in the Claude framing mode. The replay omits it so the CLI can redeliver it on reinitialization. |
| H4 | A pinned turn streams over five megabytes, replays each line in order, and releases the pin and compacts on the successor's command. |
| H5 | Two successors offer the same generation. Only one can send input after taking control. |
| H6 | An orphan closes its agent's stdin and removes its recovery files. |
| H7 | A detached agent finishes. The next controller reads its last output and exit without starting another child. |
| H8 | Stopping the agent also closes the listener owned by its subprocess. |
| H9 | Backpressure from an old controller cannot block a successor from replaying the journal and reading new output. |
| H10 | A raw worker keeps blank lines and carriage returns when streaming and when replaying. |
| H11 | A disconnected holder with a full, pinned journal still stops its agent when it times out. |
| H12 | An agent exits while a descendant in its own session keeps its output open. The controller hears the exit within the TypeScript holder's two-second allowance. |
| R1 | [Rust controller process test](../../box/crates/kuvo-box/tests/agent_holders.rs). Kill the original controller while the stand-in agent waits for a reply. A replacement controller launched from another release directory reads the record, replays the turn, answers the ask, and checks that both PIDs are unchanged. This tests the process and wire boundary used by a resident replacement. The test uses the same Rust build in both directories. |
| R2 | The Rust controller scans a scratch recovery directory with a stale Rust record and a TypeScript record. It returns neither as adoptable and leaves the TypeScript record untouched. Unix removes the stale Rust files. Native Windows preserves files when it cannot match the recorded start identity. |

The holder uses the existing OS socket and process-group abstractions. Windows
uses a same-SID named pipe derived from the computer's persisted socket key.
The holder uses a separate secret to authenticate the controller. The wire
remains `hold/1`. The recovery record includes the Rust holder format marker and OS process start identity, and only those records
are adoption candidates. The Rust controller gets exclusive socket halves
and the holder gives each direction bounded queues. Replay reads the disk
journal through its index. The adapter supplies the rule for which
outstanding request methods can survive a transfer.

## Verification

| Platform | TypeScript holder conformance | Rust holder conformance | Rust controller process tests |
|---|---:|---:|---:|
| macOS | 12/12 | 12/12 | 2/2 |
| Linux x86-64, lokum | — | 12/12 | 2/2 |
| WSL2, Ubuntu 24.04 | — | 12/12 | 2/2 |
| Native Windows x64 | — | 12/12 | 2/2 |

Linux and WSL used the same static musl build compiled on the Mac. The native
Windows run used static libunwind and the pinned Node 24 runner. The workspace
compile check, targeted Clippy with warnings denied, and TypeScript check passed.

The two H1 transcript files matched byte for byte after substituting the PIDs.
They include wire messages in both directions, the on-disk journal text, the
updated recovery metadata and generation, and the absence of the temporary
files after exit. To reproduce, set `KUVO_CONFORMANCE_AREA=agents` and
`KUVO_CONFORMANCE_HOLDER_TRANSCRIPT` to a new file for each binary before
running `node --test protocol/conformance`, then compare those two files.

The Rust holder applies a size limit to local frames and bootstrap data. It
also cleans up its owned process tree after the agent exits. Windows uses job
termination for a stop and a numeric exit status. POSIX signals remain
available on Unix.

## Resident runtime progress (4.1b)

The account index uses the saved `agents/accounts.json` format. The tests
read a legacy array, retain an added account's original home and unknown
fields, and refuse to rewrite a future-version index. A new account gets
its own folder and provider ID. The default account keeps its provider ID
and leaves home selection to the adapter.

The shared roster instantiates a factory for each configured account. It
keeps an adapter and its cached answers when the account label changes.
Account checks keep the last sign-in after a transient failure and ignore
an answer begun before credentials changed. A failed model lookup keeps
only the same account's previous catalog. The sign-in attempt book scopes
status, completion and cancellation to the initiating provider and accepts
a late-page code when the adapter says a code is needed.

The Rust controller replacement scenario now calls `Runtime::activate`.
It builds the stand-in adapter's roster, uses the recorded provider to find
it, and starts complete replay at the oldest active turn's cursor. The
controller API is unchanged. The saved session header reader supplies the
working directory without opening the transcript. The keeper switch case
passed on macOS, Linux (lokum), WSL2 and native Windows. It replaced the
resident during a running fixture turn and checked that the same holder
and child finished it. Authenticated keeper and resident proof belongs to
4.4.

On all four platforms, the foundation passed seven account tests, two
resident dispatcher tests, three holder process tests and twelve binary
conformance cases. The account fixture ran against both boxes on macOS.
The normalized HTTP replies and saved files matched, including the
legacy account home.

The vendor entry point is `Agent::open_thread`. Each thread receives a
new prompt or attaches the resident's sink to a recovered turn through
`Thread::start`. The sink handles transcript events, vendor-state
checkpoints and independent permission and question promises.

Saved-session checks read the bridge's header, native conversation state,
legacy JSON transcript and SQLite transcript. The binary transcript case
pages a saved conversation in both directions on both boxes. The replies
and unchanged transcript files match. On the first write Rust stages a
legacy import beside the original array and publishes it after commit.
Updates keep the existing message's position and carry the bridge's
usage and artifact projections. A late checkpoint or transcript write
after deletion cannot recreate the conversation.

The resident turn sink waits for a transcript writer to acknowledge each
event and saves native state before acknowledging that checkpoint.
Permission and form requests are scoped to their session and turn.
Tests cover withdrawing an ask, duplicate request IDs in separate
sessions, denying a late ask, and withholding acknowledgement until a
checkpoint has been written.

The transcript and sink checkpoint passed on macOS, Linux (lokum), WSL2
and native Windows. The two-box transcript fixture compared five HTTP
responses and the saved SQLite bytes, and the account fixture compared
the normalized replies and account index.

4.1b owns the read-only Blocks loader, command analysis and preflight
evaluation. The Rust tests cover the four command parsing cases in
`bridge.test.ts` and the `Sessions.checkPermission` cases for Blocks,
unavailable settings, native Plan mode, Auto outside the sandbox and
paths outside the project. Row 6.1 owns editing Blocks, the Settings
pages and sandbox setup and can reuse this evaluator.

The resident starts and interrupts a saved coding-agent session through
the compatibility routes. The macOS binary case follows a stand-in
through two turns and checks the saved conversation and vendor state.
The keeper case also checks that its replacement resident connects the
recovered adapter to the shared sink and saves the replayed text and
final outcome. The holder and child PIDs are unchanged. The controller
contract and roster factory signature are unchanged.

The compatibility permission and form routes now use the resident's
ask book. The macOS fixture sees a running coding-agent session in the
shared session list, changes Blocks while that turn is asking, and
verifies that an attempted approval cannot release the action. It then
rejects the ask and checks that a delayed reply cannot answer it again.
Unified ask cards and delivery routes remain open.

The shared session actions work on coding-agent conversations too.
`kuvo/sessions/<id>` with its `actions`, `rename`, `pin` and `archive`
finds the conversation with the agents before asking the engine. The
saved header keeps the bridge's own notes (`kuvoAgent` and a subagent's
`kuvo.messages`) when a client replaces the metadata, and reads carry the
computer's Summarize setting for Claude Code, Codex and Grok. Delete stops
a running turn first, refuses new turns meanwhile, and removes the
subagents' conversations unless a fork's steps still show them. Archive
stops a turn that waits on an approval or a question. The binary case
(`agent-sessions`) passes on both boxes and their normalized replies
match. Delete's confirmation now counts the coding agents' rows in a
worktree, as the TypeScript list does. Holding the box queue on Archive
and Stop belongs with the delivery work below. The engine's `Auto` run
that a session forgets on delete stays with Auto.

`POST kuvo/sessions` starts a coding agent's conversation on the shared
start rules (project, folder, approval, parent and wake checks). The
engine starts only when the start needs it: its own conversations, a
worktree, or a parent it keeps. The new conversation's ID uses the
engine's ascending format, its header is the bridge's, and a task's
message keeps its client notes (`kuvo.parts`) on its row. The binary
case passes on both boxes and the normalized answers, header and user
row match. Session actions, the agent start and the holder's exit allowance passed
on macOS, Linux (lokum), WSL2 and native Windows: the holder cases,
`agent-accounts` and the three `agent-sessions` cases through the binary,
and the Rust account, saved-session and ask tests.

Open: the engine's refusal for a model without an ID uses
different words from the TypeScript model rule (engine start, 3.2), and
wake-up notes on a coding agent's conversation wait for the shared note
writer.

Each agent's native state lives in its own slot of the bridge's
`agent.json` (`threads.<provider>.state`), beside the inbox, Undo notes
and the other agents' slots. A turn opens with its agent's slot, and a
checkpoint rewrites only that slot. A conversation from before slots
gives its single state to the agent that wrote its last reply, as the
bridge does, the first time the record is saved.

Each conversation has a line of messages waiting for its agent, kept in
`agent.json` (`inbox`) in the bridge's shape (`kuvo_agents::inbox`). The
resident's sessions apply the bridge's rules to it: a message goes into the
turn under way where its agent takes one partway, now when nothing runs
(a queued one too, unless others wait before it), or after the turn; a turn
that ends on its own starts the next, steered ones first; a Stop or an
error holds what waits until Steer or Send Now; one sent twice goes once;
one handed to the turn and taken back meanwhile is sent if the agent took
it and cancelled if it didn't; a text file goes into the words. An
uncertain handoff saves its receipt before the message is sent
(`ActiveTurn::steer_receipt`), keeps everything behind it waiting, and is
settled from the agent's own conversation every second, never by sending
it again, also by the next resident. A message whose agent can't start is
kept, failed. A handoff token the box cancelled refuses its late message.
Twenty Rust scenarios with a scripted agent (`kuvo_agents/tests/inbox.rs`)
port `sessions.test.ts` cases 3–7, 10, 11, 13–17, 48 and 54–58 and 61, and
Codex's uncertain-handoff rule. A twenty-first case ports case 18 and
bab62eb80: deleting waits eight seconds for a turn that won't stop, and its
late finish can't write the conversation again. They passed on macOS, Linux (lokum), WSL2
and native Windows with the agent conformance areas. The compatibility prompt route now puts the
message in the line (`delivery`, `metadata`, `handoffToken`) and interrupt
takes `resume`.

The shared conversation routes reach that line too. The box's queue
(`kuvo/sessions/<id>/send`, `queue`, `queue/<item>/steer|now`, `reorder`,
removal, `stop` and `queue/resume`) asks one typed target for a
conversation, the engine's inbox or the coding agents' line, the same
questions: what waits, steer or queue one, take one back, stop with or
without resuming, whether it runs, its recent rows and its transcript. The
agents' line and turn events reach the queue in the engine's words
(`session.inbox.*`, `session.execution.*`, `session.idle`), so Stop holds
and a turn's end drains as for the engine's conversations. A handoff to the
agents waits on the resident's own process. The binary case (the stand-in
agent, Rust only) sends through `send`, queues behind a running turn, sees
the item in the queue, stops, finds it held, resumes, and sees it go as the
next turn; it passed on macOS, Linux (lokum), WSL2 and native Windows. The
engine conformance areas give the same results as before. Moves and
wake-ups into a coding agent's conversation still go through the engine and
remain open, as do Auto and summaries.

Turns go on through a resident update. The resident that makes way hands
each turn going on in a held agent to the next one
(`run/agents-handover.json`, read once). The handover holds the asks the
turn was waiting on and the approvals you gave for the rest of it. After
that, the old resident answers nothing that is waiting. It doesn't turn asks
down as it goes, so the agent hears only the next resident's answer. A
message waiting behind a turn whose agent won't go on with it is kept for
the next resident and marked steered. So is one sent while the resident
makes way. The next resident sends them once, in order, as it starts.

The next resident takes up its agents' turns before anything else starts.
Each turn runs as a turn of the message that began it, if that turn is still
the conversation's last, still open, and on the same agent. Otherwise the
adapter is told to let its agent go (`Adoption::let_go`). Adapters keep their
own ask IDs, so a replayed ask shows once, under the ID it had before. A
client's read of the approvals waits up to ten seconds for the asks the
earlier resident showed; `?live=1` doesn't wait. An answer for an ask that
isn't waiting is refused.

A take-up that fails keeps what was shown of the turn, with its tools
stopped and its steps closed, and ends it as failed. One that goes through
writes the turn again from the agent's lines, by the same IDs, and drops the
rows the replay didn't write again. At start, a turn no agent goes on with is
closed as the bridge closes it (`interrupted`, reason `restart`), and a
summary waiting for it is dropped. A turn whose agent couldn't start, or
stopped reporting, ends as failed in its conversation.

Five Rust scenarios (`kuvo_agents/tests/updates.rs`) port
`held-resume.test.ts` 1–3 and `sessions.test.ts` 8, and carry an ask and an
approval through the handover. The binary case (`agent-turn-fixture` 4)
replaces the resident while a held stand-in's turn waits on an approval,
through the shared JSON-RPC connection. The stand-in's replay comes a second
late, yet the next resident's first read shows the ask once. A stale answer
is refused, the answer reaches the agent once, and the message queued before
the update goes next, once. Without the wait, that first read is empty. The
keeper switch case still passes. The update scenarios and all four
`agent-turn-fixture` cases passed on macOS, Linux (lokum), WSL2 and native
Windows, with the holder, account, saved-session, line and agent conformance
cases. On Windows the test harness stops the resident by force, so nothing is
handed over there. The ask still comes again, and the case waits for it.

The current beta's golden home (`compat/current-world.ts`) now also keeps a
waiting message and an agent's slot in its coding-agent conversation, written
by the TypeScript store. The `agent-sessions` case copies that conversation,
its account index and the account's folder into a fresh home and opens them
through either box. Both close the conversation's open turn as a restart's
and keep its waiting message, its agent's slot and the account index. Their
normalized answers and files match. The traces differ only in the run's
scratch folder, the generated account ID, and an empty Undo record
(`snapshots`) that the bridge adds as it closes the turn; Undo is row 4.6.

Carried to the rows that bring them:

- Production vendor factory registration, as each adapter lands (4.2–4.5). Codex's is in (4.2).
- `sessions.test.ts` 9 (a program update's drain) and 12 (a model whose
  account is signed out): 4.2, with the first program updater and production
  account status.
- 59–60 and 127–139 (turns an agent starts on its own, and Stop or Send Now
  while one is handed over): 4.2 and 4.4, whose agents start them.
- Moves and wake-ups into a coding agent's conversation: 4.6, with carry and
  handover.

## Tools, shared RPC and sidecar progress (4.1c)

`kuvo_agents::rpc` is the agent-neutral JSON-RPC connection over a held
agent's lines: bare messages for Codex's app-server and `jsonrpc: "2.0"`
for ACP agents. One task owns the holder's input and output, numbers
requests from a chosen first ID, and hands the agent's own requests and
notifications to the adapter, which answers by ID. It reads each journal
line once unless the holder sends an unanswered request again,
acknowledges what it took in every 200 ms and before it lets go, and
keeps the last 8,000 characters of stderr. An answer after a timeout, or
to an earlier resident's request, reaches the adapter as a stray. An
exit ends open requests with the last three stderr lines or the exit
code. A lost holder connection ends them as exit -1 (`lost`); a fenced
one hands the agent over quietly. Six scenarios cover these rules and a
process test runs a request, the agent's own request and its exit
through a real holder. The holder suite's first two TypeScript cases
(offset first IDs, a fenced connection, a successor answering) map to
them.

`kuvo_agents::runtime_packages` installs an agent's npm packages (the
Cursor SDK for its Bun sidecar) the first time they are needed, into
`runtime/<agent>`, with the engine's Bun, its cache in the box's own
folder and emptied after each install, one install per agent at a time,
and the same `package.json` and messages as the bridge. Unlike the
bridge, it stages the install beside the folder and publishes it with a
rename, and retires an old folder by renaming it before deleting it, so
an interruption never leaves a folder whose versions look installed. Two
scenarios run it with a stand-in installer. The bridge has no tests of
its own for this.

`kuvo-box mcp` is the stdio MCP forwarder an agent starts in its sandbox
(`kuvo_agents::tool_forwarder`). It sends each message to the tools
relay with the session's key, never through a proxy, keeps the relay's
MCP session, and forwards a cancel only for a call still open. While the
relay restarts it waits up to 80 seconds and sends a call again with the
same `kuvo.operation`; a browser call is never sent twice. Three
scenarios against a stand-in relay cover these rules, including
[toolrelay](../../box/test/agents/toolrelay.test.ts) case 1. When the
tool's time runs out it answers "Kuvo Code's tools didn't answer."
where the bridge's script passed on its timer's abort message. The
relay itself (keys, programs, the computer's MCP servers and stand-ins,
cases 2–9) and the browser's MCP mode remain open.

The agent-slot store, the JSON-RPC connection (with its real-holder
process test), the runtime package installer and the stdio forwarder
passed on macOS, Linux (lokum), WSL2 and native Windows.

`kuvo_agents::tool_relay` is the relay itself, for Kuvo Code's tools. It
listens on 127.0.0.1 and gives each conversation a key of its own, which
stays while any of its agents' threads holds it. The address and keys are
kept in `run/agents/relay.json`, so an agent held through an update reaches
its tools at the same address with the same key. The next resident keeps
only the keys of the conversations whose agents it took up. Each call is
its own conversation's whatever it names, with an operation of its own: a
forwarder's operation is kept, so a call sent again after a restart is done
once.

On a computer the tools are a program run outside the sandbox, one per
conversation, shared by its agent's connections. Their requests are
numbered anew, so a cancel reaches only its own connection's call. What the
program asks the agent is refused at once, and a call past its time is
cancelled there.

Five scenarios with stand-in tools port toolrelay cases 2–6; the stand-in
program is the test binary itself. Cases 7 and 8 are not ported: no caller
since the Swift app was dropped. They cover an app's own tools server (the
bridge's `PUT kuvo/tools`, which only the Swift app sent) and the desktop
compatibility path that only that server's tools took.

The computer's own MCP servers are relayed too, each at `/mcp/<name>`, to
every agent. The agent is given only the relay's address and its
conversation's key. A command's server starts for each of an agent's
connections, found on the `PATH`, with its keys and the conversation in its
environment, and stops when the agent leaves it. A URL's server is reached
with its headers, and the relay reads its start before passing it on. A
server that can't be reached, or won't start, or turns down its sign-in
doesn't stop the turn: the agent connects to a stand-in of the relay's own
with no tools, whose instructions say why, and a call that fails partway
says why as the tool's error. A turned-down sign-in is renewed and tried
again, and the host is asked once, for each setup, to connect the server
again. Once the server is set right, the agent is given it at another
address and connects afresh.

What the servers are and how they're renewed comes from a host trait
(`ServerHost`). So does running a command in the sandbox: the engine
supplies the servers (3.x) and the sandbox supplies the command runner
(6.1). Three scenarios cover an unreachable server, the sign-in loop (the
bridge's `relay-live` case) and command servers per connection. The
relay's nine scenarios passed on macOS, Linux (lokum), WSL2 and native
Windows, with the agent conformance areas. The update case now also checks
that both turns' holders end with their turns.

A server the relay stood in for because it couldn't be reached or start
is checked again at an agent's next message: connected to and let go. The
message waits a second at most for the answer, and the check goes on
without it. One still not there is checked a few minutes on, then twice as
long each time, up to half an hour; a change to how it's set up checks it
at once. Once it answers, its agent is given it afresh. A command's server
no connection has used for an hour stops. Four scenarios port the `acp`
cases for these (given afresh once it answers, not holding a message up,
not started again at each message and checked less often) and the idle
stop.

The resident starts the relay with its address and keys in
`run/agents/relay.json`. On a computer it listens at once where it was,
keeping only the keys of the conversations whose agents it took up; the
update case checks that it keeps its port. The computer's servers come from
the engine while it runs (never started for them): its settings for the
conversation's folder, in the engine's order, without Kuvo Code's own,
those turned off or a marketplace app the folder's scope leaves out, with
your sign-ins added and renewed as they run out. They're read again at most
every few seconds, and the last known stay while the engine can't say. A
refused sign-in connects the server again for the folder and for the
engine. Kuvo Code's own tools are the program the engine's settings name
(`kuvo-browser`). An agent in the sandbox reaches them through
`kuvo-box mcp`, which forwards to the relay and waits for it through an
update. Every resident runs in a box home of its own, a Mac's too, as the
bridge did (its keeper starts it as a machine's in every home), so this
holds on a Mac as on any computer. Three scenarios cover the engine's
servers, connecting again and what an agent is given. An agent-turn case
on a Mac home shows it end to end: a held agent in the sandbox calls a
tool through the forwarder before the resident is replaced and again
after, and both calls reach Kuvo Code's tools as its conversation's.

What later rows plug in:

- **4.2–4.5 (each adapter):** the factory takes the resident's `ToolRelay`.
  For each thread it calls `agent_tools(session, holder, fenced)` for Kuvo
  Code's tools, `computer_servers(directory)` and
  `relayed_servers(session, holder, directory, &found, brief)` for the
  computer's (`brief` for ACP agents), and `servers_key` at each turn to
  decide whether to start its agent again. It calls `drop_relayed` when the
  thread ends.
- **6.1 (sandbox):** a `Sandbox` launcher given to `EngineServers::new`
  runs a command's server in the box's sandbox (toolrelay case 9). Until
  then a command's server runs as it is.
- **6.4 (browser keeper):** the engine's start settings name the
  `kuvo-browser` program (`ToolPlaces::set_program_settings`), with
  `engine-own-tools`.

## TypeScript scenario inventory

### Holders, reload and repair

Case numbers use the order in the cited source file.

| TypeScript source | Port and remaining owner |
|---|---|
| [hold.test.ts](../../box/test/agents/hold.test.ts), 19 | 1 → R1, diagnostic events checked through H7. 2 → H1. 3 → H7. 4–5 → H2. 6–7 → H4. 8 → managed program install cleanup (4.3). 9 → H4. 10 → H2. 11 → H8. 12 → H9. 13 → H5. 14 → R2. 15 → H6 and H11. 16 → H1. 17 → H3. 18 → socket path fallback integration (4.1b). 19 → startup cancellation and fallback (4.1b). |
| [held-resume.test.ts](../../box/test/agents/held-resume.test.ts), 3 | 1–3 → `kuvo_agents/tests/updates.rs`, with `agent-turn-fixture` 4 through the binary. R1 covers the underlying process and replay. |
| [hold-claude.test.ts](../../box/test/agents/hold-claude.test.ts), 1 | H3 covers holder suppression, with real CLI reconnection and request cancellation in 4.4. |
| [bridge-update.test.ts](../../box/test/agents/bridge-update.test.ts), 2 and [bridge-update-cursor.test.ts](../../box/test/agents/bridge-update-cursor.test.ts), 2 | Holder continuity → R1. Adapter readiness, same conversation and next-turn reuse → 4.1b, 4.2, 4.4 and 4.5. |
| [agents-renewal.test.js](../../box/bridge-tests/agents-renewal.test.js), 4 and [update-stops-nothing.test.js](../../box/bridge-tests/update-stops-nothing.test.js), 5 | R1 covers the holder portion of replacement. Resident admission, actual keeper selection, readiness and rollback → 4.1b and 7.2. The historical bridge-specific standby, bridge build, pre-holder switch and quiet-stop rules belong to the TypeScript transition in 9.1. |
| [folder-version.test.js](../../box/bridge-tests/folder-version.test.js), lines 287, 301 and 326 | Runtime startup relies on the resident's layout gate. The agent-runtime assertion belongs to 4.1b. Keeper and release trial rollback → 7.1 and 7.2. |
| [peers.test.js](../../box/bridge-tests/peers.test.js), 17 | Local live coding-agent status feeds the resident's peer status (4.1b). Legacy machine roster → 2.6 and 7.3. |

### Accounts, roster, tools and sidecar

| TypeScript source | Remaining owner |
|---|---|
| [accounts-check](../../box/test/agents/accounts-check.test.ts), 20, [account-refresh](../../box/test/agents/account-refresh.test.ts), 8, [added-account-route](../../box/test/agents/added-account-route.test.ts), 3, [own-homes](../../box/test/agents/own-homes.test.ts), 4, and [signin-mode](../../box/test/agents/signin-mode.test.ts), 3 | 4.1b owns account index, home selection, checks, catalog cache and sign-in state. Vendor sign-in commands and authentication messages → 4.2–4.5. Accounts-check 18 covers Work Queue GitHub authorization in 5.2. |
| [sign-in.test.js](../../box/bridge-tests/sign-in.test.js), 11 | Agent route cases 2–4, 8 and 10–11 → 4.1b. OpenCode methods and status → 3.2. Setup and remote transport assertions use the resident dispatcher. |
| [renewal](../../box/test/agents/renewal.test.ts), 14, [sandbox-claude-renewal](../../box/test/sandbox-claude-renewal.test.ts), [program-updates](../../box/test/agents/program-updates.test.ts), 9, [update-notices](../../box/test/agents/update-notices.test.ts), 4, and [claude-mac-update](../../box/test/agents/claude-mac-update.test.ts), 3 | 4.1b owns shared renewal scheduling and roster updates. Provider commands and policies → 4.2–4.5, sandbox → 6.1, full release workflow → 7.2. |
| [toolrelay](../../box/test/agents/toolrelay.test.ts), 9 | 1 → `kuvo_agents/tests/tool_forwarder.rs`; 2–6 and `relay-live` 1 → `kuvo_agents/tests/tool_relay.rs`, with the relay's parts of four `acp` cases (re-checks and the idle stop). 7 and 8 not ported: no caller since the Swift app was dropped. 9 with 6.1. Tool handlers remain with their service or work area. Compatibility removal and sandbox allowlists → 7.3 and 6.1. |
| [engine-own-tools](../../box/test/engine-own-tools.test.ts), 4 | 6.4: the browser keeper owns the `kuvo-browser` tools program the engine starts with. |
| [bridge](../../box/test/agents/bridge.test.ts), 45, and [bridge.test.js](../../box/bridge-tests/bridge.test.js) | Resident route, roster and agent runtime checks → 4.1b. Tool transport and sidecar launch → 4.1c. Bridge-only HTTP hop and split process lifecycle retire with the TypeScript bridge in 9.1. |
| [sessions](../../box/test/agents/sessions.test.ts), 158 | 22 ported: 3–7, 10, 11, 13–18, 48, 54–58 and 61 to `kuvo_agents/tests/inbox.rs`, 8 to `kuvo_agents/tests/updates.rs`. 9 and 12 (program drains, signed-out models) → 4.2; 59–60 and 127–139 (turns an agent starts itself, Stop and Send Now while a message is handed) → 4.2 and 4.4. Vendor wire, transcript specifics → 4.2–4.5. Carry, handover, review and subagent projection → 4.6. |
| [codex](../../box/test/agents/codex.test.ts), 121, [acp](../../box/test/agents/acp.test.ts), 73, [claude](../../box/test/agents/claude.test.ts), 136, and [cursor](../../box/test/agents/cursor.test.ts), 31 | 4.2–4.5 own vendor protocols and per-agent tests. The adapter-independent JSON-RPC seam and runtime package install belong to 4.1c. |
| [agent-tools.test.js](../../box/bridge-tests/agent-tools.test.js), [artifacts](../../box/test/artifacts.test.ts), [subagents](../../box/test/agents/subagents.test.ts), [carry-agents](../../box/test/agents/carry-agents.test.ts), [to-agents](../../box/bridge-tests/to-agents.test.js) and [agents-handover](../../box/bridge-tests/agents-handover.test.js) | Agent registration and tools dispatch share 4.1b/c. Tool handler semantics span 2.4 ownership and messages, 4.6 subagents and transfers, 5.1 starts and bot work, 5.5 artifacts, and 6.1 permissions. |
| Agent account imports in [usage tests](../../box/test/usage), and [settings-agents-live](../../box/bridge-tests/settings-agents-live.test.js) | 5.4 owns usage and limits. 6.1 owns the engine Agents settings file and the live engine settings page. Both consume 4.1b account state. |

### Cases carried from the engine coverage map

All of the following remain with the shared resident-facing runtime (4.1b),
with worktree mechanics in 6.3 and tool relay integration in 4.1c. Details
and existing partial coverage remain in [the engine map](rust-engine-coverage.md).

| TypeScript case | Number(s) |
|---|---|
| message-parts, queue-items, resident-delivery | 7; 25; 1 |
| session-actions, session-pages, session-lists | 2 and 10; 3; 14 |
| queue.test.js, start-session, typed | 44–50; 1; 5 |
| conversation-activity, session-asks | 2; 2 |
| models-offered, model-aliases | 1–2, 5, 8–9, 11–12, 14–15; 1–3 |
| worktrees | 12, 30 |
| attachment-paths, attachments-go | 1; 1–4 |
| session-models | 6–9, 11, 15–16, 18, 21, 23–24, 26–27, 31–32 |

The golden-home check above covers the account index and a saved agent
conversation. The legacy `run/agents.json` projection is the peer directory,
regenerated at start (2.6 and 7.3). Authenticated vendor runs belong to
4.2–4.5 (Claude Code's to 4.4), and live adoption during a fleet rollout to
7.2.

## Claude Code (4.4)

4.4a covers account discovery, native sign-in and the model catalog. The
[agents-claude conformance cases](../../protocol/conformance/agents-claude.test.mjs)
use the same Claude CLI stand-in with both residents. They exercise a signed-out
account, code submission and confirmation, account-specific default models,
a retained Console directory, and replacement of a saved setup token after
native sign-in. The model catalog uses Claude's initialize reply directly.
It provides a provisional list while that reply is pending.

Two more cases run a saved conversation through each resident. One starts a
new Claude conversation and saves its native ID in the agent's slot of the
session record. The other starts from a record in the slot shape
(`threads["claude-code"].state`, beside another agent's slot): both boxes
resume that Claude conversation and keep the other slot. A Rust-only case
switches the keeper between two Rust releases during a turn; the
replacement resident finishes it with the same holder and Claude process.
The TypeScript reference passes eight cases and skips the release switch;
the Rust box passes all nine.

The [live case](../../protocol/conformance/agents-claude-live.test.mjs)
(area `agents-claude-live`) runs only when `KUVO_CONFORMANCE_CLAUDE` names
a launcher for a signed-in, isolated Claude Code. A real Haiku turn waits on
an approval to run a harmless command while the keeper switches Rust
releases. The replacement resident shows the same approval again and
answers it. The same holder and Claude process run the command and finish
the turn.

4.4b's [holder scenario](../../box/crates/kuvo-box/tests/claude_threads.rs)
runs two Rust runtimes against the same external fixture. It verifies the
holder and child PIDs across adoption, replays text with its holder position,
withdraws an old hook, redelivers one open approval, and persists an
Allow Always suggestion only for the session. It also completes a late
steered message, withdraws an ask on cancellation or Stop, stores the native
ID from `/clear`, resumes that ID after process shutdown, and redacts a
credential from the departing process's diagnostics.

[agents-claude-turns](../../protocol/conformance/agents-claude-turns.test.mjs)
ports `claude.test.ts` scenarios to both boxes. A scripted stand-in plays
each scenario's Claude Code lines and records what it reads, and the cases
check the saved transcript, the session's totals and the asks. Rust now
follows the reference adapter's projection: tools as the app names them,
an edit's file change from Claude Code's own hunks, a command's output and
notes, server tool results, Claude Code's notes, retries and limit
warnings, and summaries it makes by itself. A turn paid by an API key adds
what Claude Code says it cost. The shared turn projection gained
`TurnEvent::Cost` (additive) and the session's token and cost totals.
Tools still running when a turn ends are stopped. A summary reported only
once it's done gets its own row. A repeated note is written once. Stop
tells Claude Code no for an open ask.

`POST kuvo/agents/api/session/:id/compact` is now declared with the other
coding-agent session routes, and the Rust resident serves it from the shared
session runs. A summary goes first in line, now or once the turn under way
ends. It is refused in the reference bridge's words for a subagent's
conversation, a model whose agent isn't here, an agent that only summarizes
by itself (`Agent::compacts`, false unless an adapter says so), a
conversation the agent hasn't started, and a pending undo. The Rust resident
counts a conversation as started when the agent's slot has vendor state,
because Rust has no handover `seen` cursor yet. 4.6 owns that cursor. The
case "one whose agent has a conversation saved but hasn't seen this one is
refused, saying why" passes on the reference and is a `todo` on Rust until
4.6 makes it pass. The summary's row carries
its own ID with `reason: "manual"`, and the conversation's last outcome stays
as it was. The Rust session store keeps a
transcript's data pictures as files under `attachments/<session>/transcript-pictures`,
as the reference store does.

For the permission cases, the reference bridge gets the computer's settings
from a small engine stand-in that serves `box/opencode.json`, the file the
Rust resident reads Blocks from. The Rust resident gained these, all
agent-neutral:
- A card names its step and tool call (`source`).
- A question turned down with words passes those words to the agent.
- A path outside the project is asked as `external_directory` when your
  rules ask about working there (`TurnPolicy::card`, defaulting to the ask
  as it is).
- `POST kuvo/agents/api/session/:id/agent` changes a conversation's mode
  with an `agent-switched` row, as the reference does. It is declared with
  the other session routes.

Claude Code is started in its permission mode (`--permission-mode`). Its
hook leaves a plan-mode edit to it, and its own background output under
`/tmp/claude-<uid>` counts as its own.

Seen once: on lokum, in a one-CPU container, at d50d27912, "the Rust keeper
keeps an actual Claude adapter turn across a release switch" failed with
`The replacement resident never finished the held turn.` (adapter.mjs
`until`, agents-claude.test.mjs:205), after 32.5 s. Two reruns of the same
build passed, as did WSL and native Windows. It failed again at 058ce659d
and at 6aa76a156, the second time with the case's new report: the first resident had ended the
turn as failed ("The adapter stopped reporting its turn.") and the next one
never took it up. Cause: once the holder confirmed the handover, the first
resident's Claude thread dropped the turn's sink. Its turn runtime then read
the closed channel as an agent gone quiet, and wrote a failed end before that
resident exited. The failure needed the resident to live on briefly after the
handover, which one CPU allowed. A handed-over turn's sink now stays open for
as long as that resident is there. The holder scenario checks this, and the
keeper case reports the last rows, what Claude Code heard and the keeper's
log if it ever fails.

A turn's end is written before its Claude Code's holder hears that it ended.
If the resident goes between the two, the next resident finds a holder still
in a turn that has ended, and lets it go: that Claude Code stops, and the
conversation's next message starts one that resumes it. Before this, the
next message failed with "Claude Code has a recovered turn." Native Windows
showed this, and the holder scenario covers it.

Shared changes for 73–85: a turn its agent couldn't start ends with the
agent's own reason. Before, a closed channel could win the race and say "The
adapter stopped reporting its turn." A turn you stopped while it started, and
which then couldn't start, ends stopped (`TurnWriter::stopped`, additive). An
agent exit's end carries no error, as in the reference, which writes none for
a stopped turn.

A message handed to a running turn has a receipt before it is sent: the
UUID Claude acknowledges it by. The receipt stays unresolved while a live or
recovered turn hasn't acknowledged the message. After that, the message
counts as delivered only if Claude's transcript has it. The TypeScript
adapter has no receipts. It reports unacknowledged messages as missed when
the turn ends, and Rust keeps that path.

### TypeScript scenarios

Numbers follow source order in [claude.test.ts](../../box/test/agents/claude.test.ts).
The two rows expanded from the bot and session steering loop count separately.

| Cases | Coverage and remaining owner |
|---|---|
| 1–7 | 4.4b stream events, cancellation before launch, limit scope, error messages and expired sign-in. Skill resolution and startup cancellation also need 4.1b/c. |
| 8–10 | 4.1b route and queue recovery, including a late cancelled handoff and steering across resident reconnect. 4.4b owns the Claude stream and message acknowledgements. |
| 11–13 | The holder scenario covers 11, a late steered result, and its receipt. 4.4b owns the other steering and missed-message cases, and 4.1b owns retry of missed messages. |
| 14–28 | The background model is ported from the reference, with Claude feeding it first. `kuvo_agents::background` is agent-neutral: commands an agent left running, with their output and Stop through the agent, and the other tasks it keeps going. The resident publishes them as the `bridge` place of `run/work/background.json`, and kuvo-box serves `kuvo/work/background`, its session and command-output routes, and their followed forms from that file, as the reference's link does. 14, 16 and 18, and 15's bar, are ported to agents-claude-turns on both boxes: a foreground command moved to the background keeps its start time; a command left running shows in the bar with what it printed until it ends; a finished one shows its final output file; and a changed summary setting doesn't end Claude Code while its command runs. Seams: 15's own turn, 17 and 27 (turns Claude Code starts on its own) are 4.6's. 19, 25 and 26 (waking the session, and its note, when Claude Code or the box stopped with work going) are 5.1's wake-ups. 20, 21 and 28 (staged Undo) and 23 (an account removed) are the session line's, 4.1b's. 22 and 24 (an update keeps the background work going) need the next resident to take up what an adopted Claude Code keeps going, with 7.2. The engine's own place in the file (its shells and active subagents) and reading its commands' output are the engine's, with 5.1c; until then the output route says `unread` for an engine command. Subagents at work under a conversation are 4.6's to add to the model. |
| 29–42 | 4.6 subagent sessions, background lanes, messaging and handover. 4.4b supplies Claude's wire events. |
| 43–49 | Ported to agents-claude-turns on both boxes: the summary window Claude Code is started with and its setting between messages, the saved context, a tool's picture kept as a file, server tool results, its own notes, repeated hook failures and limit warnings. |
| 50 | Ported to agents-claude on both boxes. Claude's model probe asks each model's window (`set_model`, then `get_context_usage`), once per model it resolves to, and the shared catalog model carries it as `context` (additive). "Never summarize" holds only for a model of 900,000 tokens or more, as the conversation's turns or the list say; the case checks it through what Claude Code is started with. Until a turn says, a conversation's `contextWindow` is its model's listed window, as in the reference (`withContextWindow`). That read goes through the engine's sessions, which these cases run without, so only its source is checked here. |
| 51–54 | 4.4a conformance covers account-selected default aliases and the concrete name and ID of a default outside the listed models. |
| 55–57 | Ported to agents-claude on both boxes: before Claude Code has listed its models, the usual ones answer to its default and none is called default; a conversation on its default is never told a model, so Claude Code runs its own; and an older Claude Code that doesn't say how much each holds gets its list's word (a million where it says 1M, else 200,000). |
| 58–61 | 4.6 conversation transfer and chosen model across computers. 4.4b supplies Claude's native transcript and memory locations. |
| 62–66 | 4.1c MCP server wiring, status, credentials, reconnect and deadlines. 4.4b supplies Claude's control request boundary. |
| 67–72 | 4.4b Claude launch, native state and session environment. 4.1c owns relay wiring and 6.1 owns sandbox policy and launch. |
| 73–85 | Ported to agents-claude-turns: a Claude Code that stops before it takes your message fails the turn as Claude's SDK says it, with the last whole line it said. Secrets, stacks and over-long lines are left out. An earlier or replaced Claude Code's words don't count, nor does a warning from a turn that went through. One quitting after it took your message stops the turn with no error. These pass on both boxes. 82–84 too: Claude Code's words after it stopped count for as long as its holder waits (2 s), a line still coming is left out, and you can stop the turn meanwhile. The reference bridge did neither under its holder, so a piece of a secret could show. a6701cc39 fixed that: its holder waits for Claude Code's error output too, and says whether all of it came. On Windows, what Claude Code left running ends with it (its job), so nothing speaks after it, and 82 doesn't apply there. 77 and 85 need the box's sandbox: they go with 86–92, and 85's token is checked here. |
| 86–92 | 89 is ported to agents-claude on both boxes: signed out, the Claude Code a conversation has open goes, and its next message starts another. Rust stops it as you sign out; the reference stops it at that next message. 86–88 and 90–92 need the box's sandbox: its own sandbox off inside the box's, attachments copied where the sandbox reads them, starting again inside it once it's on, and a worktree's repository fences. They go with 6.1, which owns the policy and the launch (Rust has only the OS-level Seatbelt and Linux wrappers so far), and 6.3 for worktree paths. So do 77 and the sandbox half of 85. |
| 93–103 | Ported to agents-claude-turns on both boxes: Ask First, Allow Always for a command and for edits, Deny with the edit's preview, Approve All and `.env`, paths outside the project and Claude Code's own files, Blocks, Plan mode and leaving it, and questions answered, dismissed or turned down with words. |
| 104–109 | Ported to [agents-claude-turns](../../protocol/conformance/agents-claude-turns.test.mjs) on both boxes. After Stop, the reference bridge under its holder starts Claude Code again, while its own test expects the same process to take the next message. Rust does that, and only Rust checks it. |
| 110–114 | Ported to agents-claude-turns on both boxes, with a refusal for a conversation Claude Code hasn't started. |
| 115–122 | Ported to agents-claude-turns on both boxes, except the `session.compaction.*` events, which the conformance cases don't read. |
| 123–127 | Ported to agents-claude-turns on both boxes: what changed in its instructions is said once, ahead of the next message Claude Code takes, never ahead of a command you type, and not after it summarized. In 126 the reference's SDK closes Claude Code on an error result, so its turn ends as that exit and the line hands the message over again (4.1b). The Rust resident's Claude Code goes on and fails the turn, as the reference adapter's own test expects, and only Rust checks what follows. Its half for a message Claude Code never took, because it quit, waits on 4.1b's retry of missed messages. 4.6 owns carry and handover. |
| 128–131 | 131 is ported to agents-claude on both boxes: a probe for a new Claude account's models doesn't wait on one for the old account, and the old one can't change the models offered. The reference lets the old one answer and ignores it, while Rust stops it. 128–130: the adapter's side is here, and the ring is 5.4's. Claude Code's `rate_limit_event` becomes `TurnEvent::PlanLimit` (additive, agent-neutral): the windows the reference's ring keeps for one reading (`heardWindows`: id, used, minutes, resetsAt, model), with the sign-in it was heard under, as the reference's fingerprint of `.claude.json`'s account. A reading heard after the sign-in changed counts for no one. A Claude Code started under another sign-in, or one taken up whose sign-in isn't known, starts again at its next turn, going on with its conversation. The turn projection hands each reading, with the account the turn runs on, to `PlanLimits::heard`, which does nothing until 5.4 brings the ring (`Projection::with_limits`). Unit tests (`heard_windows`, the projection's hand-off) and the holder scenario cover it. 5.4 owns the ring, its generation (a reading from before the ring dropped an account), and the routes that serve it, with `TurnEvent::PlanLimit` as their input. |
| 132–136 | 4.6 subagent definition and policy projection, with 6.1 for Agents settings. |

| Other TypeScript source | Coverage and remaining owner |
|---|---|
| [claude-mac-update.test.ts](../../box/test/agents/claude-mac-update.test.ts), 3 | Cases 1–2 have 4.4a binary conformance for setup-token replacement and in-place Console import. Empty unused directories are removed. The Rust migration keeps unfamiliar files in a folder with neither marker. |
| [hold-claude.test.ts](../../box/test/agents/hold-claude.test.ts), 1 | H3 covers holder replay suppression. The 4.4b holder scenario covers reinitialization, cancellation of an old hook, and redelivery of a pending approval. |
| [claude-code.test.js](../../box/bridge-tests/claude-code.test.js), 7 | 4.6 owns the import of local Claude subagent definitions. The scanner must preserve the existing name, type, link and size limits. |
| [sessions.test.ts](../../box/test/agents/sessions.test.ts), late completion after delete and first Undo after a quit | 4.1b owns session guards and transcript loading. 4.4b provides held results and awaits durable sink events. |

A Claude sign-in that a run turns away (`authentication_failed`) closes that
Claude Code. The account then reads as ended, even though `claude auth
status` still says signed in, until its credentials are written again. The
holder scenario covers this. The resident still needs to check the account
again when such a turn ends, and to refuse a message at once while the
account is signed out (`signedOut` in the TypeScript sessions). Both belong
to 4.1b's shared session runs. The catalog probe gives Claude its settings as
a file, so a Claude Code installed as a `.cmd` script can list its models.

### Verification

| Platform | Holder scenario | `agents-claude`, Rust | `agents-claude`, TypeScript | Live Haiku keeper switch |
|---|---:|---:|---:|---|
| macOS | 1/1 | 9/9 | 8/8, 1 skipped | Passed. Claude Code 2.1.330 ran under a Seatbelt profile that denies the Keychain. |
| Linux x86-64, lokum | 1/1 | 9/9 | — | — |
| WSL2, Ubuntu 24.04 | 1/1 | 9/9 | — | Passed. Claude Code 2.1.330 linux-x64, checked against its manifest. |
| Native Windows x64 | 1/1 | 9/9 | — | — |

Linux and WSL used one static musl build compiled on the Mac. Windows used
an LLVM MinGW cross build. Lokum ran in a container limited to one CPU. The
live case ran with a dedicated test account's setup token and nothing else.
The token reached only the Claude process, as `CLAUDE_CODE_OAUTH_TOKEN`. The
case used a scratch home, Claude configuration and XDG folders, and dropped
every inherited variable. On WSL the token sat in the scratch folder with
mode 600 and was deleted with it. The live run also showed that Claude
Code 2.1.330 records a sent message's UUID as its transcript's user entry.
Steer receipts rely on that.

The complete home comparison follows, under "Claude conversation differential".

### Claude account and catalog differential

The differential runner can install the same labelled Claude stand-in on both
copies of a scratch home when `KUVO_CONFORMANCE_AREA=agents-claude`. The seed
has a previously saved Claude Console account in `agents/claude/console`.
The model picker reads the same model IDs, aliases, and account ID. The saved
account index and the native fixture in the Console directory compare unchanged.

The full inventory also compares unrelated startup state and records these
remaining differences.

| Difference | Owner or reason |
|---|---|
| Picker generation values and absent generations for accounts with no model catalog | The values are per-instance IDs. 4.1b owns reporting generations for signed-out accounts. Other vendor accounts are registered by 4.2, 4.3 and 4.5. |
| `agents/runtime/claude` | The TypeScript fixture installs its SDK symlink here. Rust speaks stream-json directly. |
| `agents/limits.json`, `agents/usage/`, `run/usage-attribution.json` | 5.4 owns limits, usage collection and Insights. |
| `agents/sessions/`, `run/agents.json`, `run/glance-source.json`, `run/lists/sessions.json` sequence | 4.1b owns resident session storage and its shared projections. |
| `run/agents/` and its `relay.json`, `box/marketplace/mcp-relay-v1.json`, engine defaults plugin and generated config folders | 4.1c owns agent and engine tool relay. Marketplace integration also uses 6.4. |
| `box/approval.json`, `run/fence/` | 6.1 owns permissions and sandbox policy and their files. |
| `artifacts/`, `library/` | 5.5 owns artifacts and Library backfill. |
| `bots/`, `run/bots-seen`, `wakes/`, `run/work/` | 5.1 owns bot, wake-up and work publishers. 4.1b supplies agent background state. |
| `run/github-sign-in`, `run/work-queue/` | 5.2 owns Work Queue setup and access. |
| `groups/` | 5.3 owns group chats. |
| Legacy arrays and maps in `run/link.json` | 2.1 owns the signed setup representation, 2.2 owns peer projections, and 7.3 owns the released compatibility fields. |
| `run/link.version` | 7.2 owns release/version projection. |
| `run/route/ssh` | 1.4 uses one private resident IPC endpoint. The TypeScript reference creates the old route socket directory. |
| `run/queues-scanned.json` | 3.2 owns OpenCode queue reconciliation with its stand-in. |
| `run/notices/` | 2.2 owns peer notices. 5.1 owns the resident scan that publishes notices from work. |

### Claude conversation differential

`protocol/conformance/differential/claude-conversation.mjs` writes a home with one Claude Code
session, the stand-in's script and a request sequence. Two turns run, one with a tool call and both
with usage. Then the sequence reads the transcript after each turn, the background list, and the
session's background bar. Give the runner `KUVO_DIFFERENTIAL_CLAUDE=<folder>/claude.json` with
`KUVO_CONFORMANCE_AREA=agents-claude`. A transcript read with `idles: n` waits for n ended turns.
The runner hides only per-run values: row times, the conversation's last-change times, and the
conversation ID Claude Code is given. It stops the holders each run leaves before it deletes the
home.

The comparison found three Claude differences, now fixed and covered by both-box cases:

- The session's background bar lacked `activity`, subagent titles and `asking`
  (`agents-claude-turns`, "a command it leaves running shows in the bar").
- A turn's context reading had no window until the turn's result said one. TS uses the window
  Claude Code reported for the model: when its models were listed, or as an earlier turn ended
  (`agents-claude`, "what Claude Code said a model holds as a turn ended counts for the next
  conversation on it").
- A session read keeps the listed window in `session.json`, as the bridge does.

It also found that a Rust resident handed its agents over on every stop, and each holder waited
15 minutes for a resident that wasn't coming. TS's bridge hands them over only when it makes way
(a release switch or a newer build). On SIGTERM, SIGINT or a closed stdin it stops every held
agent. Now the keeper's stop request says whether another resident comes: yes for a release
switch or a rollback, no when turned off or paused. The resident hands over only then; otherwise
`Directory::stop_all` asks each holder to stop. That's SIGTERM where there is one, and on any
system a `<id>.stop` request beside its record, which a Windows holder watches. Without a request,
a resident its keeper keeps comes back unless it's off or paused, and one no keeper keeps stops for
good. `agents-claude` "a box stopping for good stops the Claude Code it holds" covers both boxes:
the agent and its holder go, and the next start ends the cut-off turn as interrupted. The keeper
switch case still keeps the turn.

Besides the files in the table above, these differences remain:

| Difference | Owner or reason |
|---|---|
| Row IDs: TS mints ascending IDs, Rust derives them from the prompt ID | 4.1b owns the shared turn rows. |
| `time.origin` on every row, `time.order` on user rows | 4.1b owns the rows. 5.4 reads the origin for usage attribution. The order is `background::WorkOrder`. |
| Text parts: TS writes `{type, text}` and `time.streamed` on the step; Rust writes `state: null` and `time {created, streamed}` on the part | 4.1b owns the shared turn projection. |
| `<from-phone>` block and `metadata.kuvo.parts` on a prompt that isn't from the desktop link | 7.3 owns message decoration (see the engine coverage). |
| `session.json` `contextWindow` before a turn says one | TS keeps the listed window as a side effect: it reads the session before sending to check that the model takes pictures. Rust keeps it on a session read. Both answer the same window to clients. The send-time read goes with attachments (3.2, 7.3). |
| `agent.json` `snapshots` and the thread's `seen` | 4.6 owns staged Undo's file keeping and the handover's seen cursor (the open `todo` case). |
| `run/lists/sessions.json` `running`, `working` and `seq` at the stop | TS's list was last written while the second turn ran, before its writer caught up with the end. Rust's shows the ended turn. This is reference timing; 4.1b owns the list. |
| `run/agents/tmp/claude/catalog-settings.json` | Rust's model probe writes its settings there. TS passes them to the SDK. |
| `run/work/background.json` `engine` place | The engine's place is written by the engine watcher (5.1). Rust writes the coding agents' `bridge` place. |
| `run/work-queue/model.json`, `run/work/told.json` | 5.2 and 5.1. |

## Codex (4.2)

`kuvo_agents::codex` runs your own `codex` through its app-server
(`codex app-server`, bare JSON-RPC on its pipes). One app-server serves all
of an account's conversations, each one of its threads, under a holder
(`kind: codex`, `key: ""`, the turns under way by Codex thread). It starts
when first needed, initializes once, and stops after fifteen minutes with
nothing on it. When the summary setting or the sign-in it started with
changed, it starts again once nothing uses it. The factory is registered in
the resident and takes its `ToolRelay`: each turn gives the thread Kuvo
Code's tools and the computer's servers, and a closed thread drops them.
The sandbox's own app-servers (one per session, and your account's) are
6.1's; until then every Codex runs outside it, as the reference does with the
sandbox off.

[agents-codex](../../protocol/conformance/agents-codex.test.mjs) runs a
scripted app-server ([codex.mjs](../../protocol/conformance/codex.mjs)) under
both boxes: account, sign-in and models, and turns. A Rust-only case
switches the keeper's release while a turn waits on an approval. The next
resident takes up the same held app-server (same holder and child,
initialized once), asks again under the same ID, and its answer reaches
Codex once.

Shared changes: the JSON-RPC connection says each message's holder line
(`Incoming::*::sequence`), and can note, prepare and unpin
(`Rpc::{note, prepare, unpin, seen, is_closed}`). `TurnInput::permissions`
now follows the reference's `approvesAll`: Approve All only while no rule of
the session's or the agent's holds anything back, Blocks are known and stop
nothing, and it isn't Plan or Auto outside the sandbox
(`Policy::approves_all`). The instructions note and picture helpers moved
from `claude/` to the crate. `tool_relay::unproxied_loopback` keeps the
relay's loopback addresses off any proxy an agent is given (`NO_PROXY`,
`no_proxy`); Codex and Claude Code launch with it. The agent fixture's
resident-replacement cases now switch releases through the keeper, as an
update does. A separate case turns the keeper off and checks that the held
agents stop and the next resident closes their turn.

Stop on what a conversation left running is served for every agent: `DELETE
kuvo/agents/api/session/:id/background` stops each command its agent left
running through `Background::stop`. The work's `end_background` (5.1c-7) asks
the agents first (`AgentSessionSource::stop_background`), then the engine. Its
callers still look a conversation up in the engine only, so a coding agent's
conversation reaches it once 5.1 counts those conversations as someone's work.
Both boxes pass for Codex (agents-codex) and Claude Code (agents-claude-turns).

### TypeScript scenarios

Numbers follow source order in [codex.test.ts](../../box/test/agents/codex.test.ts): its 121
`test`s. Its two `test.each` tables are the named rows after them.

| Cases | Coverage and remaining owner |
|---|---|
| 1 | Rust adapter: receipts before the steer, `thread/read` by client ID. Conformance case open (4.2). |
| 2 | Unit test `a_model_at_its_own_limit_waits_alone`. |
| 3 | 4.6 (spawned agents). |
| 4–10 | `codex::terminals` keeps what Codex left running in the background model. It's listed as each turn ends, before the end is said, and again every 30 s while any runs. A terminal with a command shows in the bar with what it printed, a terminal without one is a task, and a failed first list is the task `unknown:<thread>`. The app-server stays busy for them, and its holder notes `backgroundSessions`. Ported to agents-codex on both boxes: 4 (in the bar with its output); 5–7 (a failed list isn't none, it's listed again until it ends, and it isn't woken). Unit tests: 6 (a failed later list keeps them), 8 (a reset forgets their times), 9 (a replacement item gets a fresh start), and a command's Stop through `thread/backgroundTerminals/terminate`. 10 is ported on both boxes: Stop on what it left running (`DELETE kuvo/agents/api/session/:id/background`) ends each command through Codex. The reference cleans the thread's terminals, `thread/backgroundTerminals/clean`; a terminal without a command (a task) isn't stopped by Rust, which stops commands only. Taking up the terminals of an idle app-server after an update is 7.2's, with Claude Code's. |
| 11–12 | Ported to agents-codex on both boxes: a Codex that refuses `initialize` is stopped each time it's tried, and one stopped while it starts (its account signed out) is stopped at once, not after the 30 s it's given. A stop now cuts a start short (`Shared::halt`), and signing out halts a start before closing the threads that wait on it. |
| 13–14 | Rust: a stop while the holder starts, or while its sign-in is read, fails that start once the holder answers, and stops it (the same `Shared::halt`); a holder is never dropped half-started. |
| 15 | Not applicable: a conversation's actor handles one command at a time, so closing it waits for its start. The app-server it started is the account's, shared, and stops when idle. |
| 16 | 6.1 (vault and the sandbox). |
| 17–19 | Ported to agents-codex on both boxes, including an image it made (a picture) and one it couldn't make for a limit. |
| 20–22 | Ported. |
| 23 | Ported to agents-codex on both boxes. As the resident starts, Codex's replies an older bridge saved with its directives as words are read as a reply is now (`codex::read_kept_citations`, through `SessionStore::rows_saying`), and so is a title with one. Another agent's words quoting one stay as they were. The reference also keeps a list of the files it found (`sessions.cited`), which nothing reads. |
| 24–28 | Ported. |
| 29–32, 34 | Open (4.2): answers turned down with words, the asking words, a refused steer with a card up. |
| 33, 55 | Ported. |
| 35–49, 57 | Open (4.2): telling a sleeping Codex its answer, trying again into a sleep. |
| 50 | Ported. |
| 51–54, 56, 58–60, 62, 64 | Open (4.2): answers at a turn's edge, two cards, counting refusals. |
| 61, 63, 65 | Ported. |
| 66, 72, 73 | Ported (the policies Codex starts with; Plan declines an edit itself). |
| 67–71 | Ported: Approve All answers Codex's own asks and MCP tool asks with no card; Ask First asks for another server's tool and not for Kuvo Code's; in Plan another server's tool is its yes-or-no form; an MCP server's form comes back typed. |
| 74–89 | 6.1 (Codex run whole in the sandbox, bots' memory with 5.1, attachments copied in). |
| 90 | Ported (Computer Use plugins off at launch). |
| 91–92 | Rust passes the relay's servers into the thread; conformance with the engine's servers open (4.2). |
| 93–105 | 4.6 (your subagents as roles, spawned agents, writing to them). |
| 106–108, 110, 111 | Ported. 106 reloads the thread in place; the reference starts its app-server again. |
| 109 | Ported (Fast, Standard as null, nothing where none was picked). |
| 112–113 | Ported to agents-codex on both boxes. The commands a turn waits on are listed while they run, with what they printed, and Stop on one stops its turn: `foreground::Foreground` in the turn runtime, for every agent's `shell` step. One that ends leaves the list, and what it printed can still be read. `DELETE kuvo/agents/api/shell/:id` stops one listed command through `Background::stop`. |
| 114 | Rust: a removed account (or the box stopping) ends the turn as stopped; the conformance case for removal is open (4.2). Signing out mid-turn is ported: Codex is told to sign out and the turn fails as "Codex stopped.", as the reference has it. |
| 115–119 | Ported. |
| 120 | Ported to agents-codex on both boxes: Stop during a summary tells Codex to stop its own turn, and the summary row says it failed. |
| 121 | Ported to agents-codex on both boxes: a thread it goes on with hears what changed in its instructions with your next message, once. |
| `test.each`: a terminal's start relative to the lead's instruction (4) | Rust: a listed command keeps the start heard from its `item/started` (unit test). Holding the lead's finish on it is the work watcher's (5.1c-6b), which reads that start from the background file. |
| `test.each`: thinking, a step or more words after asking (3) | Ported to agents-codex on both boxes. |

Also still open for 4.2: `sessions.test.ts` 9 and 12, 59–60 and 127–139; the
program updater; the live GPT-6-Luna run.

### Verification

| Platform | agents-codex, Rust | agents-codex, TypeScript | agent-turn-fixture |
|---|---:|---:|---:|
| macOS | 57/57 | 56/56, 1 skipped (Rust-only) | 6/6 |
| Linux x86-64, lokum (musl) | 48/48 at 04774ec91 | — | 6/6 |
| WSL2, Ubuntu 24.04 (musl) | 57/57 | — | 6/6 |
| Native Windows x64 | 57/57 | — | 6/6 |

On Windows, a cross build's `libunwind.dll` sits beside the box. The keeper
cases put that folder on the PATH.
