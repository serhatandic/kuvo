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
- `sessions.test.ts` 9 (a program update's drain) is ported to
  `kuvo_agents/tests/inbox.rs` (4.2), with a second case: a message sent while
  its program updates, even one steered into a turn under way, waits in line,
  kept, and goes once the update lets it go. `SessionRuns::hold_program` holds
  the program's turns for every account with it until those under way finish
  (here, or in an agent kept running across residents), or fails at its
  deadline and lets them go. An installer an earlier resident left running
  (`agents/program-installing`) holds them too. 12 (a model whose account is signed out) is ported to agents-codex on
  both boxes. Before a turn its agent is asked (not one taken up), an agent
  installed and signed out, as last known and asked anew (at most 10 s each),
  turns it away at once as failed (`signin`, naming the computer). This is
  shared, for every agent (`SessionRuns::signed_out`).
- 59–60 and 127–139: see the sessions row above. The handing cases are ported
  (4.2); turns an agent begins on its own go with the agents that begin them
  (Claude Code's with 4.6, the ACP agents' in 4.3), and kept files and the
  handover with 4.6.
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
| [hold.test.ts](../../box/test/agents/hold.test.ts), 19 | 1 → R1, diagnostic events checked through H7. 2 → H1. 3 → H7. 4–5 → H2. 6–7 → H4. 8 → `acp::antigravity::forget_releases` (unit test, 4.3): an earlier Antigravity release stays until the kept one is whole and no holder may use it. 9 → H4. 10 → H2. 11 → H8. 12 → H9. 13 → H5. 14 → R2. 15 → H6 and H11. 16 → H1. 17 → H3. 18 → socket path fallback integration (4.1b). 19 → startup cancellation and fallback (4.1b). |
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
| [renewal](../../box/test/agents/renewal.test.ts), 14, [sandbox-claude-renewal](../../box/test/sandbox-claude-renewal.test.ts), [program-updates](../../box/test/agents/program-updates.test.ts), 9, [update-notices](../../box/test/agents/update-notices.test.ts), 4, and [claude-mac-update](../../box/test/agents/claude-mac-update.test.ts), 3 | 4.1b owns shared renewal scheduling and roster updates. Provider commands and policies → 4.2–4.5, sandbox → 6.1, full release workflow → 7.2. The program updater, its drain, its routes, the Accounts rows' versions and update notices are 4.2's (program-updates and update-notices). Grok's and Antigravity's `Agent::program` are 4.3's (`Flavour::program`), with no case yet. |
| [toolrelay](../../box/test/agents/toolrelay.test.ts), 9 | 1 → `kuvo_agents/tests/tool_forwarder.rs`; 2–6 and `relay-live` 1 → `kuvo_agents/tests/tool_relay.rs`, with the relay's parts of four `acp` cases (re-checks and the idle stop). 7 and 8 not ported: no caller since the Swift app was dropped. 9 with 6.1. Tool handlers remain with their service or work area. Compatibility removal and sandbox allowlists → 7.3 and 6.1. |
| [engine-own-tools](../../box/test/engine-own-tools.test.ts), 4 | 6.4: the browser keeper owns the `kuvo-browser` tools program the engine starts with. |
| [bridge](../../box/test/agents/bridge.test.ts), 45, and [bridge.test.js](../../box/bridge-tests/bridge.test.js) | Resident route, roster and agent runtime checks → 4.1b. Tool transport and sidecar launch → 4.1c. Bridge-only HTTP hop and split process lifecycle retire with the TypeScript bridge in 9.1. |
| [sessions](../../box/test/agents/sessions.test.ts), 158 | 22 ported: 3–7, 10, 11, 13–18, 48, 54–58 and 61 to `kuvo_agents/tests/inbox.rs`, 8 to `kuvo_agents/tests/updates.rs`. 9 to `inbox.rs`, 12 to agents-codex (4.2). 59 and 133–136 to `inbox.rs` (4.2): a message taken as its turn ends goes once; Stop, with or without resume, and Send Now while one is being handed; a handed one cancelled after Send Now starts nothing later. 60 (a message its agent never got to goes with the next) is the handover's seen cursor → 4.6; Codex never reports one. Turns an agent begins on its own come from Claude Code and the ACP agents only (the reference's `onOwnTurn`): Claude Code's (127–129, 132, 137, 139's own half and 136's second half) → 4.6, and the ACP agents' are 4.3's (agents-grok); those waiting on kept files (130, 131, 138) → 4.6. Vendor wire, transcript specifics → 4.2–4.5. Carry, handover, review and subagent projection → 4.6. |
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
| 14–28 | The background model is ported from the reference, with Claude feeding it first. `kuvo_agents::background` is agent-neutral: commands an agent left running, with their output and Stop through the agent, and the other tasks it keeps going. The resident publishes them as the `bridge` place of `run/work/background.json`, and kuvo-box serves `kuvo/work/background`, its session and command-output routes, and their followed forms from that file, as the reference's link does. 14, 16 and 18, and 15's bar, are ported to agents-claude-turns on both boxes: a foreground command moved to the background keeps its start time; a command left running shows in the bar with what it printed until it ends; a finished one shows its final output file; and a changed summary setting doesn't end Claude Code while its command runs. Seams: 15's own turn, 17 and 27 (turns Claude Code starts on its own) are 4.6's. 19, 25 and 26 (waking the session, and its note, when Claude Code or the box stopped with work going) are 5.1's wake-ups. 20, 21 and 28 (staged Undo) and 23 (an account removed) are the session line's, 4.1b's. 22 and 24 (an update keeps the background work going) are covered by 7.2 U90: an idle held Claude process is adopted with the same command identity, start, output and Stop, without starting a turn. The engine's own place in the file (its shells and active subagents) and reading its commands' output are the engine's, with 5.1c; until then the output route says `unread` for an engine command. Subagents at work under a conversation are 4.6's to add to the model. |
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
| Picker generation values and absent generations for accounts with no model catalog | The values are per-instance IDs. 4.1b owns reporting generations for signed-out accounts. Codex's, Grok's and Antigravity's accounts are registered (4.2, 4.3); 4.5 registers its own. |
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

## Grok and Antigravity over ACP (4.3)

`kuvo_agents::acp` is one ACP client for every agent that speaks it, each a flavour of its own
(`acp/grok.rs`, `acp/antigravity.rs`; OpenCode is the third in 4.7). The reference ran one agent
process per conversation. Here a process carries several conversations, each one of its ACP
sessions, under one holder (`kind: grok|antigravity`, `turns` and `sessions` by ACP session).

- **Sharing.** Conversations whose launch matches share a process: the same mode, sign-in and
  compaction threshold, and for Antigravity the same program (`agy` or the earlier agent). The
  key is read before each turn without starting anything (`Flavour::launch_key`). A process
  that's outdated (a new program, a stop it can't go on from, an answer that never came) takes no
  new sessions and stops once nothing uses it. One that nothing has used for 15 minutes stops.
- **Opening sessions.** What a process says before `session/new` answers is kept for that session
  (at most 1024 updates) and handed to it in order. What a load replays is dropped. A process
  that ends fails each turn on it with its own words. A turn whose access ran out partway, after
  the first minute, carries on in a new process.
- **Wire.** It speaks over the shared `Rpc` (JSON-RPC 2 framing), not the `agent-client-protocol`
  crate. That crate's connection can't take a holder's replayed lines or the IDs an earlier
  resident sent. ACP's shapes are read into Kuvo's own types at the edge (`acp/wire.rs`).
  `Rpc::send` gives a request's ID before its answer, for the holder's turn note.
- **Light (4.8).** Listing accounts and models never starts Grok or `agy`. Grok's sign-in comes
  from `GROK_HOME/auth.json` or `XAI_API_KEY`. Its catalog is the one a turn's `session/new` last
  listed, kept per account under `run/agents/grok/catalogs`. Before any turn it offers one
  provisional "its default model", which picks no model. Antigravity reads its home's
  `kuvo-sign-in.json` and `kuvo-models.json`. Refresh doesn't start it, and its sign-in check
  runs only when you sign in.
- **No plan mode** (user, 11-25). Neither agent is offered one. A conversation saved in plan mode
  goes on in the normal mode, and its saved value stays on disk, unread. `Mode::read_only` is the
  box's own, for 4.6's reviews and side questions (Grok's `--permission-mode plan`,
  Antigravity's `plan`), never a mode anyone picks. The reference's case 21 and the plan halves
  of 22 and 23 are not ported.
- **Agent-started turns.** The resident takes one through `SessionContext::own_turns`. It runs at
  once when nothing runs, or after the turn under way when that turn is on the same agent (the
  conversation's `own`, before what waits in line), as `TurnKind::AgentStarted`, shown as
  "Grok carried on by itself." One not taken is turned away: the agent is told to cancel, and
  what it begins after 20 quiet seconds is offered again.
- **Work Queue #992.** For an Antigravity account signed in before `agy` (Google's earlier agent
  and its `acp_token.json` in its home):
  - With `agy` installed, it's signed in only once `agy` takes that sign-in. Until then its row
    offers one Sign In, which first asks `agy` to use the earlier sign-in (`Takeover`, no page).
    Only if `agy` refuses does Google's page come, as for any sign-in.
  - Without `agy`, it runs on the earlier agent and says "Install agy, then sign in to move it
    there".
  - It never says "update".
  - Starting a sign-in without `agy` fails with its reason and the fix ("Antigravity needs Google's
    agy CLI on this computer. Install it in Settings › Accounts…"). The agents' sign-in route keeps
    every agent's reason (`refuse`). The reference's generic "couldn't be started" is in its
    `kuvo/sign-in/*` routes (6.1a), not ported yet.
  - agents-antigravity proves these with a stand-in token in a temp home. **Checked in the update
    rehearsal:** whether the real `agy` takes a real earlier token it didn't write.

### TypeScript scenarios

Numbers follow source order in [acp.test.ts](../../box/test/agents/acp.test.ts), 73 cases.

| Cases | Coverage and remaining owner |
|---|---|
| 1–2 | `acp::projection`: pictures, links, sound clips and unknown kinds' words; context and window. Unit tests for the projection; agents-grok's turn case on both boxes. |
| 3–6 | Reads that end only as the turn does (`Flavour::reads_unsaid`): unit `a_read_kuvo_did…`, agents-antigravity's read through Kuvo. 5 (in the sandbox) → 6.1. 6: Grok's reads stay as it says (its flavour doesn't settle them). |
| 7–8 | `acp::tools::kuvo_tool`: wrapped Kuvo tools, tool search, server-prefixed names; a call renamed later carries its new name on its next event. Titled names of Kuvo's own tools need its tool list (`KnownTools`), which the tool program's listing brings (6.4). |
| 9 | A subagent's session shows its row, named by its task (unit). Its own steps → 4.6. |
| 10–13 | Antigravity's questions (permission asks on `interaction_…` calls), declined words on its step; Grok's `x.ai/ask_user_question` (agents-grok, both boxes) and declined words as its answer; forms (`elicitation/create`) through `codex::elicitation_field`. |
| 14 | Agent-started turns: agents-grok, both boxes. |
| 15–20 | Asks: allow-once picked for Once and Always (Always is Kuvo's to keep); reject-once for No, the step declined; Approve All answered for you (agents-grok's ask case, agents-antigravity's yolo case). Carrying on after a refusal of your settings' (never after yours), at most three times a turn. |
| 21 | Not ported: no plan mode (user, 11-25). |
| 22–23 | Grok's mode in its command, a change starting another process; Antigravity's yolo or default by `session/set_config_option`, and a conversation to ask first that can't leave yolo doesn't start. Plan halves not ported. |
| 24–27 | Skills and your subagents as Grok plugins and Antigravity custom agents: not in the Rust box yet (no skills module in `kuvo_agents`) → 4.6 with subagents. |
| 28 | Stop: agents-grok, both boxes. |
| 29 | Closed while starting: `Shared::halt` and `shutdown` (the same as Codex's 11–14); no case yet. |
| 30 | Stop while it waits on you: an ask settled after Stop answers cancelled (`Settled::Asked { cancellable }`); no case yet. |
| 31–33 | Background tasks: 31 (a reported start wins) and 32 (busy keeps the process) in `thread.rs`; agents-grok shows them as at work. 33 (killed from outside: the conversation woken with what was lost) → 5.1, which owns waking a conversation; Rust only logs it. |
| 34–36 | Steps, edits with their change, one to-do list, cost as the conversation's total (unit tests; agents-grok's turn case). |
| 37–44, 46, 65–66 | The box's sandbox → 6.1. |
| 45, 47–64 | The computer's MCP servers through the relay: the relay's behaviour is 4.1c's (`kuvo_agents/tests/tool_relay.rs`). Here each session is given the relay's addresses and its own key, and drops them as it closes. |
| 67 | `agy` stopping as it signs in: its reason in a line (`stopped_words`), the traceback to the log. |
| 68 | Reading and writing through Kuvo with the sandbox off (agents-antigravity's read). |
| 69–72 | `/compact` exactly as typed with no instructions; Grok's threshold as a share of its window (`GROK_AUTO_COMPACT_THRESHOLD_PERCENT`); its own summaries as one row. In `thread.rs` and the projection; no conformance case yet. |
| 73 | Antigravity can't be asked to summarize (`compacts()` is Grok's only). |

[antigravity-agy.test.ts](../../box/test/agents/antigravity-agy.test.ts): signing in with Google's
code and the models listed then (agents-antigravity, both boxes); not installed (its reason
kept: Rust only, the reference's route says only "The sign-in couldn't continue"); an account
from before `agy` (#992 wording, Rust only, the reference still says "update"). A wrong code, someone else's models, sign-out, the version and Update use the
same code (`Antigravity::{verify_sign_in, saved_models, forget, program}`) and have no case yet.

Not ported yet: Antigravity's transcript follower for its subagents (its rows and sessions → 4.6).
Its other half, a prompt the transcript says is answered though ACP never said, is
`Flavour::record_answer`: the actor looks every second and ends the turn on two matching looks.

Size is counted in non-whitespace characters, without tests.

- `acp` and its modules are 1.37× `acp.ts`, `grok.ts` and `antigravity.ts`.
- They are 1.30× once the base also counts what the reference keeps elsewhere and `acp` does
  itself:
  - from `turn.ts`: `startStep`, `ensureStep`, `endStep`, `toolStart`, `toolRename`,
    `toolCalled`, `stopTools`, `hasTool`, `toolInput` and `toolStep` (the projection's steps and
    merged calls);
  - from `agent.ts`: `signOutWith`, `ContextWindows` and `model`;
  - from `kuvotools.ts`: `isKuvoTool` and `kuvoToolName` (`KnownTools`);
  - from `shared/agent-credentials.ts`: `grokCredential` and `grokHome` (`grok_secret`, which
    usage reads too).
- Forms are Codex's (`codex::elicitation_form`), not a copy.
- What it does beyond the reference: several conversations on one process, updates kept until
  `session/new` answers, and Grok's catalog for listing without starting it.

### Verification

| Platform | agents-grok, Rust | agents-grok, TypeScript | agents-antigravity, Rust | agents-antigravity, TypeScript |
|---|---:|---:|---:|---:|
| macOS | 8/8 | 6/6, 2 skipped (Rust-only) | 10/10 | 6/6, 4 skipped (#992, Rust-only) |
| Linux x86-64, lokum (musl) | 8/8 | — | 10/10 | — |
| WSL2, Ubuntu 24.04 (musl) | 8/8 | — | 10/10 | — |
| Native Windows x64 | 8/8 | — | 10/10 | — |

On native Windows the stand-in `agy` is `agy.CMD`. Antigravity's sign-in starts it as the
conversations' holder does (`Spawn::executable`). It first failed two sign-in cases there, which
started it by its bare path.

Live runs:

- **Passed (2026-11-26): live Antigravity turn (Gemini 4 Hydrogen).** `agents-antigravity-live` ran
  through the Rust box on macOS with `agy` 1.5.18 and read-only copies of the fresh sign-in files
  in a temp home (token untouched, no refresh, temp home deleted after). Gemini 4 Hydrogen replied
  `"OK"`, streamed `session.created`, `session.inbox.enqueued`, `session.inbox.delivered`,
  `session.execution.started`, `session.execution.succeeded` and `session.idle` to the app's feed,
  and ended cleanly in 7.0 s.
- **Pending: live Grok turn on its cheapest model.** Neither the Mac nor lokum has a sign-in to copy
  (no `auth.json`, no `XAI_API_KEY`).

Seen once on the Rust box (11-25, the Mac, under a busy test lane), passing on the rerun:
agents-codex "Stop during a summary" (no `turn/interrupt` sent), and agents-claude-turns "a plain
sentence keeps its words" (the stand-in Claude Code exited 127 before speaking).

## OpenCode's older conversations and bots (4.7d, in progress)

Conversations made on the engine before OpenCode was a coding agent here, and bots on OpenCode's
models, move to the OpenCode agent. Removing the engine's place (`Place::Engine`, its start
triggers and the `api/…` passthrough) is still to come.

- **Listed without OpenCode.** `acp/opencode_history.rs` reads OpenCode's database read-only
  (`session_v2`) into the agents' inventory, each row as OpenCode's server gave it
  (`Session.Info`), marked as the OpenCode agent's. A row the engine also lists shows once.
- **Read on first open.** The first read or change takes the conversation into the agents' store
  from `opencode session export <id> --standalone`. Its rows are byte for byte the ones OpenCode's
  server answered (checked on 2.2.6), so the app draws them as before. The rows are counted
  against OpenCode's database first. `--standalone` keeps the export on its own server, never
  OpenCode's background service, so nothing stays running. Nothing in OpenCode's database is
  changed by Kuvo.
- **Continued.** It keeps its `ses_…` ID, also its ACP session (`acpSession`), so its next turn
  resumes it (`session/resume`) with all its earlier turns.
- **Bots.** A bot's conversation on any model with a provider is made with the coding agents
  (`kit::runs_on_agents`). Editing a bot still uses the agents-only check until the engine goes.
- **Older records' provider names.** `claude`, `codex` and `grok` (with an `@account`) are the
  agents' own, never OpenCode's (`Provider::of_model`); their state stays under the name it was
  kept under. 4.7b had sent them to OpenCode (found by 5.4 in Insights).

| Case | Both boxes | Notes |
|---|---|---|
| agents-opencode-history: it's listed with its title, and listing starts no OpenCode | yes | Rust also: no OpenCode process, no engine record |
| agents-opencode-history: it opens with its whole transcript | yes | same rows and IDs as the reference made |
| agents-opencode-history: it goes on with a turn that has all that came before | yes | the model hears the earlier turns |
| bots-opencode (Mac, machine): a bot made from a template on one of its models waits | yes | from `bots.json` |
| bots-opencode (Mac, machine): it's told who it is and where it runs, its conversation marked as its | yes | from `bots.json`; Rust: on the agent |

| Platform | agents-opencode-history | bots-opencode | terminals |
|---|---|---|---|
| macOS | 3/3 both boxes | 4/4 both boxes | 7/7 both boxes |
| WSL2 (static musl, OpenCode 2.2.6) | 3/3 both boxes | 4/4 both boxes | 7/7 both boxes |
| Native Windows | — (the reference makes the home) | 4/4 Rust | 7/7 Rust |

Also native Windows: agents-opencode 14/14. Measured on the Mac (debug build, a home holding a
copy of this Mac's OpenCode database, 446 conversations listed): after the app's opening requests
one process, the box, 172 MB, and no OpenCode; after opening one older conversation (its export
ran and ended) still one process, 153 MB. 4.7b's main had 2 processes and 805–915 MB.

Verified on the Mac: both areas on both boxes; bots 36/36, agents-opencode 14/14 and
engine-sessions 81/81 on Rust; `agents-opencode-live` passed with a real OpenCode Go turn (its
location is now asked again while the new conversation isn't listed yet, as the app does: the
first run met that 503 before any turn). The reference makes the older home, so these run where it runs
(Mac and Linux).

Left: removing an older conversation lists it again from OpenCode's database (it needs a
"forgotten" mark); a database from before `session_v2` isn't listed; the engine's place.

## The box's own terminals (4.7c)

The app's terminals were OpenCode's: `api/pty` passed to its server, so opening one started
OpenCode. They are the box's own now, at the same paths, answers and socket frames, so the app is
unchanged. `kuvo_os::pty` opens a pseudo-terminal (`posix_openpt` on Unix, a ConPTY on Windows)
and starts the program in it through the shared `Spawn`, owned as any child: its group on Unix,
its job on Windows. `kuvo_services::terminals` serves `api/pty` (made, listed, read, resized,
renamed, removed) and the `api/pty/:id/connect` socket.

- **As OpenCode answers.** Measured on the pinned 2.2.6: `{location, data}` answers, `pty_` IDs,
  `Terminal <last 4>` titles, `-l` added for a POSIX shell, `exited` with its code kept until
  removed, `204` on removal and `PtyNotFoundError` after. The socket sends what was printed since
  `?cursor=` (none for `-1`; `0` when it isn't a number), then a binary note
  `\0{"cursor":n}`, then output as text. OpenCode counts in UTF-16 units and keeps the last 2 MiB,
  sent again in 64 KiB pieces; so does the box. The shell exiting closes sockets `1000`; a socket
  opened after that is closed `4404 session exited`; a client's close code comes back to it.
- **Light.** Nothing runs for terminals until one is made. Each running terminal is one task; on
  Unix its output is read without a thread (the controller registered with the runtime), on
  Windows one thread reads the ConPTY's pipe. Removing one hangs up on its programs, then stops
  them; nothing is left (the case checks its shell is gone).
- **Its shell's environment** is the box's own, without `KUVO_*`, `OPENCODE_*` and `BUN_*`, with
  `TERM=xterm-256color`. OpenCode's terminals had OpenCode's: its server password and Kuvo's
  folders as `XDG_*` homes. The case checks no password reaches the shell.
- **The engine's terminal socket proxy is gone** (`proxy::socket`), and the engine no longer stays
  up for a terminal (its busy check asked `api/pty`).

### Cases

| Case (terminals) | Both boxes | Notes |
|---|---|---|
| A terminal is the computer's shell in the folder asked for, listed for that folder only | yes | |
| What's typed reaches it, as text or binary, at the size it's drawn at, and what it prints comes back | yes | Rust also: no password in its shell |
| A socket opened again takes up where its cursor says, and its note says where the output got to | yes | |
| Its shell exiting closes its sockets; it's kept, with its code, until removed | yes | |
| A terminal removed while its socket is open closes it | yes | its shell is gone after |
| A long burst is kept as OpenCode keeps it: its last 2 MiB, sent again in 64 KiB pieces | yes | Windows' console repaints the burst, so only the pieces are checked there |
| A terminal's socket is refused without serve's password, only for a terminal and a known route version | yes | statuses compared; the reason words are each server's |

`box/test/pty-socket.test.ts` (3 scenarios) maps onto the first, second and last cases; its
stand-in engine is gone with the engine's part in terminals. `kuvo_os` has three pty tests (the
program's console and size, typing and resizing, the exit code) and `kuvo_services` five
(cursor units, the 2 MiB window, characters split across reads, the shell's environment, IDs).

### Differences from OpenCode's terminals

| Difference | Why |
|---|---|
| The shell doesn't get OpenCode's environment (its password, Kuvo's folders as `XDG_*` homes) | A user's shell shouldn't hold a server password or read its settings from Kuvo's folders. |
| A folder that isn't there starts the shell in the home folder (OpenCode's shell failed to start there) | `cwd` still says the folder asked for. |
| A terminal is read, changed and removed by its ID from any folder (OpenCode looked in the request's folder) | IDs are unique; the app always names the folder it made it in. |
| Refusals say `401 Unauthorized` and so on, where the reference says `401 Refused` | Clients read the status only. |
| Terminals end when the box restarts or updates | OpenCode's ended with the engine too; the reference held an engine switch while one ran (`engine-swap.ts`). |

### Verification

| Platform | terminals, Rust | terminals, TypeScript |
|---|---:|---:|
| macOS | 7/7 | 7/7 (on the pinned OpenCode) |
| WSL2, Ubuntu (static musl) | 7/7 | — |
| Native Windows x64 (ConPTY, Git Bash) | 7/7 | — |

Also on the Mac, Rust: engine-sessions 81/81.

## OpenCode as a coding agent (4.7b)

New conversations on OpenCode's models run on the OpenCode agent: the pinned binary as `opencode
acp`, the third flavour of the shared ACP client (`acp/opencode.rs`). One process carries every
OpenCode conversation on the computer (one launch key) and stops when it's been idle. Existing
conversations stay on the engine until 4.7d. The reference runs them all on its engine, so the
`agents-opencode` cases assert only what the app reads, on the pinned OpenCode with a labelled
stand-in model (`opencode-model.mjs`).

- **Where a conversation starts.** A model whose provider isn't a coding agent's is OpenCode's
  (`Provider::of_model`): its agent account is `opencode-agent`, since `opencode` is Zen's
  provider. It starts on the agent where the box runs the coding agents and the pinned OpenCode
  is on the computer (`launch::agent_here`), else on the engine, which still serves its
  conversations.
- **What the agent is given.** Ask First on the tools Kuvo asks about (`OPENCODE_CONFIG_CONTENT`,
  so the box's own rules decide), its questions as elicitation forms, compaction, and child
  sessions. Kuvo's tools come over HTTP from the relay. The computer's MCP servers are read by
  the box from the settings files (`tool_relay/settings_servers.rs`), never by asking OpenCode.
- **Picker and accounts without starting it.** `models/kept.rs` keeps one compact list in
  `run/engine/catalog.json`, best first:
  1. OpenCode's own answers, whenever it runs;
  2. the models its agent offered a conversation (`run/agents/opencode/catalogs`);
  3. on a computer where it ran before, a list made once from its database: the providers you're
     signed in to and Zen's free models, only those that chat with tools and aren't retired or
     early. The 13 MB catalog is read once, keeping only the fields used.

  Where it never ran, the picker offers "OpenCode (its default model)" like an agent not set up;
  choosing it, a sign-in or Refresh starts it. `api/integration` answers from the same list.
- **Asks.** `kuvo/sessions/:id/asks` lists and answers an agent conversation's asks through the
  agents (`asks::Target::Agents`), for all six agents, worded as `session-asks.ts` words them.
  Always says "in this conversation". A case per agent kind failed on main's Rust box with "The
  engine isn't running."
- **Kuvo's tools and wake-ups.** A tool's caller is found through the agents first
  (`BotEngine::session_found`), so an agent conversation's tools work without the engine. Its
  wake-ups are kept as an agent's (`via`, `KUVO_WAKE_VIA`), and the app's wake-up route finds it
  without starting the engine. Cases in agents-opencode and agents-grok failed on the old code.
- **Usage counted once.** OpenCode keeps its turns in its database, which usage reads, so its
  agent's transcripts aren't counted again (`insights.rs`). The case failed on the old code (two
  calls, 220 tokens, against the reference's one call and 110).
- **Title and tokens.** OpenCode titles its own sessions. As a turn ends, a conversation still
  without a title takes it from `session/list` (placeholders aside). A turn's tokens are the
  prompt answer's `usage` (`Flavour::answers_usage`), not how full the context is.
- **Light.** The app opened with no OpenCode conversation starts no OpenCode. Measured on debug
  builds, on a home holding a copy of this Mac's OpenCode database, after the app's opening
  requests (picker, conversation lists, accounts, feeds):

  | Box | Processes | RSS |
  |---|---:|---:|
  | main (`01bf21c8e`) | 2: the box, `opencode serve` | 805–915 MB (box 180, OpenCode 650–750) |
  | this branch, first open (list made from disk) | 1 | 210 MB |
  | this branch, opened again (list kept) | 1 | 139 MB |

  With an empty database the branch's box is 88 MB and main's 93 MB plus a 356 MB OpenCode.

### Cases

| Case (agents-opencode) | Both boxes | Notes |
|---|---|---|
| A new conversation's turn streams its words | yes | |
| A command it asks to run waits on the app's asks; Allow Once runs it | yes | |
| Its question is the app's card; the answer chosen there is what it hears | yes | elicitation form → own question |
| Stop ends the turn where it is, and the next message is answered | yes | |
| A message sent while it works reaches it before it's done | yes | see the differences |
| Another model picked goes with its next turn, with its effort | yes | |
| A picture sent goes to the model as a picture | yes | |
| /compact summarizes the conversation with its model, and it goes on after | yes | |
| A subagent it starts runs, and its work shows on its step | yes | its own steps → 4.6 |
| Its wake-ups are its own: set by its tools or from the app, and Wake Up Now reaches it | yes | |
| Its usage is counted once | yes | |
| It's titled as OpenCode titles it, and its turn's tokens are what the model said | yes | |
| Where OpenCode never ran, the picker offers it as an agent not yet listed | Rust only | the reference starts OpenCode to list |
| Once a conversation ran, the picker and its accounts list what OpenCode offered it | Rust only | as above |

`agents-opencode-live`: one real OpenCode Go turn (DeepSeek V4.1 Flash) on the agent, in a temp home
with only a non-renewing key copied in. New app-asks cases in agents-grok, agents-codex,
agents-claude-turns and agents-antigravity, and a wake-ups case in agents-grok, run on both
boxes.

### Differences from the engine path

| Difference | Why |
|---|---|
| A message sent mid-turn waits for the turn's end, then goes as the next prompt | ACP refuses a second `session/prompt` during a turn; the engine injects it. The model still hears both (the steer case). |
| The rows are the agents' (`{type, content}`), with their ids, step times and no `finish` | Every agent conversation's shape (4.1b); the app reads both. |
| The conversation's total tokens leave out OpenCode's title request | ACP reports the turn; the engine's session total adds its title model's call. Usage (insights) counts both alike. |
| No `model.variant: "default"` and no per-row `cost` | The engine's own fields; a turn's cost is the conversation's (`TurnEvent::Cost`). |
| Efforts are absent for models in a list made from disk or from the agent | Only OpenCode's own answers have them; refreshed whenever it runs. After 4.7d it never runs for the picker, so 4.7d needs another source (the agent's `effort` option as each model is used). |
| A subagent's asks aren't listed | Agents' subagents are 4.6's. |
| A read-only run is refused; Plan goes in the normal mode | No Plan mode (user, 11-25); read-only runs are 4.6's. |
| Settings can't list or connect OpenCode's providers against the Rust box | `kuvo/sign-in/*` isn't ported (6.1a). Signing in starts OpenCode's own server for the sign-in only. |
| Bots on OpenCode's models still get engine conversations | They move to the agent with existing conversations in 4.7d, which rewrites the four `bots` cases that make one on the pinned OpenCode. |

### Differential runs

- **Seed against real.** On a temp home holding a copy of this Mac's OpenCode database, the
  picker's list made from disk (OpenCode not started) against OpenCode's own answers (`refresh=1`
  starts it): 41 models each, no difference.
- **One conversation, both boxes.** The same requests (start, send, location, rows, the
  conversation, asks, the active list, the picker) on copies of one home with the pinned
  OpenCode and the stand-in. It found the missing title, the tokens (110 input and 0 output for
  100 and 10), usage counted twice, and the tools' and wake-ups' gap, all fixed above. Left:
  - the place and row shape in the table above;
  - `got` (the reference starts in the postponed Auto mode and says it got Ask First; Rust never
    starts in it);
  - the send answer's `data` (Rust answers `{}`; the app reads only the status,
    `kuvo_box/src/client.rs:500`);
  - efforts in the picker.

  Home files: Rust writes the agent's (`agents/sessions/<id>`, `run/agents/opencode`, the kept
  list); the reference writes OpenCode's server record and plugin.

### Size

Non-test characters without indentation, against main (`01bf21c8e`):

- **This row's Rust: 37.2k.** The flavour is 10.2k. The kept model list is 11.5k (`kept.rs`, and
  its use in `models.rs` and `proxy.rs`): Rust only, since the reference starts OpenCode instead.
  The settings servers are 2.4k, the asks' agents target 3.2k, and the rest is wiring.
- **Base:** `agents/agents/grok.ts` (the sibling flavour, 19.3k) and `agents/servers.ts` (7.8k), 27.0k.
- **Ratio:** 0.95× without the kept list, 1.38× with it.

### Verification

| Platform | agents-opencode, Rust | agents-opencode, TypeScript | agents-grok, agents-antigravity, agents (Rust) |
|---|---:|---:|---:|
| macOS | 14/14 | 12/12, 2 skipped (Rust-only) | 10/10, 11/11, 12/12 |
| Linux x86-64, lokum (musl, 1 CPU, 1 GB) | 14/14 | — | 10/10, 11/11, 12/12 |
| WSL2, Ubuntu 24.04 (musl) | 14/14 | — | 10/10, 11/11, 12/12 |
| Native Windows x64 | 14/14 | — | 10/10, 11/11, 12/12 |

Each platform ran the pinned OpenCode of its own (2.2.6, from the engine pin). `agents-opencode-live`
passed once on the Mac with a real OpenCode Go turn. Also on the Mac, Rust: engine-sessions 81/81,
engine-models 19/19, engine-asks 9/9, agents-codex 100/100, agents-claude 20/20,
agents-claude-turns 59 and 1 todo (4.6), bots 40/40, usage 53/54 (1 skipped), agent-tools 3/3.
`work-elsewhere` fails one case ("a lead on another computer lists, stops…", the "asks first for
now" wording) on main's Rust box too.

The slower runners found two races in the cases, both fixed:

- A message sent while a stopped turn was still ending waited in line, as it should. The Stop
  case now sends once the turn has ended.
- A conversation just made wasn't on the list yet: its location answers 503 until it is, and the
  app asks again. The cases ask again too.

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
| 1 | Ported to agents-codex on both boxes, with and without the resident starting again: a steer whose acknowledgement is lost is found in the thread's history by its client ID, shown once, never sent again, and not carried into the next message. |
| 2 | Unit test `a_model_at_its_own_limit_waits_alone`. |
| 3 | 4.6 (spawned agents). |
| 4–10 | `codex::terminals` keeps what Codex left running in the background model. It's listed as each turn ends, before the end is said, and again every 30 s while any runs. A terminal with a command shows in the bar with what it printed, a terminal without one is a task, and a failed first list is the task `unknown:<thread>`. The app-server stays busy for them, and its holder notes `backgroundSessions`. Ported to agents-codex on both boxes: 4 (in the bar with its output); 5–7 (a failed list isn't none, it's listed again until it ends, and it isn't woken). Unit tests: 6 (a failed later list keeps them), 8 (a reset forgets their times), 9 (a replacement item gets a fresh start), and a command's Stop through `thread/backgroundTerminals/terminate`. 10 is ported on both boxes: Stop on what it left running (`DELETE kuvo/agents/api/session/:id/background`) ends each command through Codex. The reference cleans the thread's terminals, `thread/backgroundTerminals/clean`; a terminal without a command (a task) isn't stopped by Rust, which stops commands only. 7.2 U90 covers taking up the terminals of an idle app-server after a keeper update: same holder and app-server PIDs, same command identity, start, output and Stop, no new turn. |
| 11–12 | Ported to agents-codex on both boxes: a Codex that refuses `initialize` is stopped each time it's tried, and one stopped while it starts (its account signed out) is stopped at once, not after the 30 s it's given. A stop now cuts a start short (`Shared::halt`), and signing out halts a start before closing the threads that wait on it. A start that fails or is halted waits (at most 10 s) for its holder to end (`Rpc::ended`). Shutdown waits for a start under way, so no holder is left starting as the resident goes: on Windows one outlived the resident by about 2 s with its journal open. An app-server's holder notes it `initialized` once it is (launched with `initialized: false`). A next resident stops one left uninitialized, its resident gone mid-start, rather than take it up: Codex would refuse a second `initialize`. That case is Rust-only, because the reference doesn't mark it. The fixture cleanup (`stopFixtureHolders`) stops holders whose records appear late and waits for those still starting. The case's cleanup waits for every stand-in Codex to end, and on a failure it shows the holders' records and lifecycle. On native Windows the case passed 10 runs in a row. |
| 13–14 | Rust: a stop while the holder starts, or while its sign-in is read, fails that start once the holder answers, and stops it (the same `Shared::halt`); a holder is never dropped half-started. |
| 15 | Not applicable: a conversation's actor handles one command at a time, so closing it waits for its start. The app-server it started is the account's, shared, and stops when idle. |
| 16 | 6.1 (vault and the sandbox). |
| 17–19 | Ported to agents-codex on both boxes, including an image it made (a picture) and one it couldn't make for a limit. |
| 20–22 | Ported. |
| 23 | Ported to agents-codex on both boxes. As the resident starts, Codex's replies an older bridge saved with its directives as words are read as a reply is now (`codex::read_kept_citations`, through `SessionStore::rows_saying`), and so is a title with one. Another agent's words quoting one stay as they were. The reference also keeps a list of the files it found (`sessions.cited`), which nothing reads. |
| 24–28 | Ported. |
| 29–32, 34 | Ported to agents-codex on both boxes: answers turned down with words (in a turn, again and again, in Plan), the asking words, and a refused steer with a card up. |
| 33, 55 | Ported. |
| 35–49 | Ported to agents-codex on both boxes. An answer goes into the turn and ends the sleep it's in. A sleep that starts after its answer went in is told again once. Turned down while it sleeps, it's tried again into that sleep, after 1 s and then double each time up to 64 s, or into one that started in its place. One it heard and worked on isn't retold. Ported from the reference into `codex/thread.rs` (`steer_answer`, `answers_on_item`, `retry`). The stand-in sleeps and turns steers down as the reference's own fake does. |
| 57 | Ported to agents-codex on both boxes. |
| 50 | Ported. |
| 51–54, 56, 58–60, 62 | Ported to agents-codex on both boxes: answers at a turn's edge, two cards, a message answering both, and counting refusals (stopped as finished with no work between, never with work). |
| 64 | Rust: a message from the app (synthetic) doesn't answer an open card (`Turn::steer`). No client route sends one in either box (the reference's `synthetic` is the bridge's own). Its proof goes with app notes into a coding agent's conversation (4.6). |
| 61, 63, 65 | Ported. |
| 66, 72, 73 | Ported (the policies Codex starts with; Plan declines an edit itself). |
| 67–71 | Ported: Approve All answers Codex's own asks and MCP tool asks with no card; Ask First asks for another server's tool and not for Kuvo Code's; in Plan another server's tool is its yes-or-no form; an MCP server's form comes back typed. |
| 74–89 | 6.1 (Codex run whole in the sandbox, bots' memory with 5.1, attachments copied in). |
| 90 | Ported (Computer Use plugins off at launch). |
| 91–92 | Rust gives each server the relay finds as `mcp_servers.<name>`: its relay URL, the conversation's own `Authorization`, never the server's keys. A changed set (a sign-in renewed) loads the thread again (`Loaded::servers`). The servers, their keys and a refused sign-in are the relay's, proven in `tool_sources.rs` (4.1c). A both-box case needs the engine's settings under the Rust box, which no agent case runs yet. Claude Code's equivalent isn't in conformance either, so it goes with the relay (4.1c). |
| 93–105 | 4.6 (your subagents as roles, spawned agents, writing to them). |
| 106–108, 110, 111 | Ported. 106 reloads the thread in place; the reference starts its app-server again. |
| 109 | Ported (Fast, Standard as null, nothing where none was picked). |
| 112–113 | Ported to agents-codex on both boxes. The commands a turn waits on are listed while they run, with what they printed, and Stop on one stops its turn: `foreground::Foreground` in the turn runtime, for every agent's `shell` step. One that ends leaves the list, and what it printed can still be read. `DELETE kuvo/agents/api/shell/:id` stops one listed command through `Background::stop`. |
| 114 | Ported to agents-codex on both boxes: an added account removed while its Codex works ends the turn as stopped, without Stop. Signing out mid-turn is ported as well: Codex is told to sign out and the turn fails as "Codex stopped." |
| 115–119 | Ported. |
| 120 | Ported to agents-codex on both boxes: Stop during a summary tells Codex to stop its own turn, and the summary row says it failed. |
| 121 | Ported to agents-codex on both boxes: a thread it goes on with hears what changed in its instructions with your next message, once. |
| `test.each`: a terminal's start relative to the lead's instruction (4) | Rust: a listed command keeps the start heard from its `item/started` (unit test). Holding the lead's finish on it is the work watcher's (5.1c-6b), which reads that start from the background file. |
| `test.each`: thinking, a step or more words after asking (3) | Ported to agents-codex on both boxes. |

The program updater is shared, in `kuvo_agents::program_updates`: which version
you have, the newest, the installer that owns it (its own updater, an npm
global, a Homebrew keg or cask), and the update. Its outcome is kept for every
client. Its records (`agents/program-draining`, `agents/program-installing`)
are the reference's, so mixed versions see each other's, and a next resident
takes one up. An agent says how its program is updated (`Agent::program`).
Codex's own installer updates it with `codex update`; otherwise npm's
`@openai/codex`. After an update, each account with the program runs it
(`Agent::program_updated`): Codex's app-server starts again once nothing uses
it. Unit tests cover the versions, the owners and a real program's update, a
failed one, a timed-out hold and one a next resident takes up. Claude Code
updates the same way (`claude update` on its settings' channel, else npm's
`@anthropic-ai/claude-code`), and each conversation's Claude Code starts again
at its next message.

### Accounts rows, updates and the agents' feed (4.2)

The reference's `box/src/shared/routes/index.ts` declares all of `kuvo/agents/`
as pass-through to its bridge, so the bridge's routes had no specs. The Rust box
serves each `kuvo/agents/` route the app uses from its own spec. These came
with this row, each 1:1 and for every agent:

- `GET kuvo/agents/api/integration` and `…/:provider`: Settings › Accounts'
  rows (`account_rows`). Each agent's facts are kept by kind, for all five
  agents: its sign-in methods, its installer, and how signing out goes. An
  account shows them whatever adapter runs it here. Grok's and Antigravity's
  `Agent::program` is 4.3's (`Flavour::program`); Cursor reads no program, as in the reference.
- `POST kuvo/agents/api/integration/:provider/install`, the maker's installer.
- `POST kuvo/agents/api/kuvo/updates[/:provider]`,
  `DELETE kuvo/agents/api/kuvo/updates/:provider?at=`, and the update notices
  (`program_updates::notices`, `agents/update-notices.json`).
- `GET kuvo/agents/api/session/active`, `GET kuvo/agents/api/session/:id` and
  `POST kuvo/agents/api/session/:id/model`. The reference takes a model whose
  provider is no agent's as an engine model its bridge follows (`engineTurns`).
  The Rust box refuses it: a coding-agent conversation carried to the engine is
  4.6's.
- `GET kuvo/agents/api/event`, the agents' feed, which the app follows on the
  line for Accounts. It's one agent-neutral broadcast (`feed::Feed`) on the
  resident's agents.

Proven on both boxes by `agents-updates` (program-updates.test.ts's cases and
update-notices.test.ts's, with Codex and a stand-in npm registry,
`KUVO_TEST_NPM_REGISTRY`) and `agents-session-reads`. Both areas follow the feed
through the line. The reference's case that an update throwing as it's asked
ends failed has no Codex equivalent; Rust's `update` can't throw, and a program
that can't start ends as failed with what it printed.

The reference's feed sends these. Rust sends them now:
`server.connected`, `server.heartbeat`, `integration.updated`, `model.updated`,
`kuvo.updates.notice`, `kuvo.updates.notices.updated`,
`kuvo.agent.account.changed` (Codex), `session.created`, `session.renamed`,
`session.metadata.updated`, `session.deleted`, `session.model.selected`,
`session.agent.selected`, `session.background.updated`, `permission.asked`,
`permission.replied`, `form.created`, `form.replied`, `form.cancelled`,
`session.inbox.enqueued`, `session.inbox.delivered`, `session.inbox.cancelled`,
`session.inbox.delivery.changed`, `session.execution.started`,
`session.execution.succeeded`, `session.execution.failed`,
`session.execution.interrupted` and `session.idle`. Each conversation's event
names its folder (`location`).

The rest go with the open rows that bring their sources:

- A turn's steps as they stream (`session.step.started`, `session.step.ended`,
  `session.tool.*`, `session.usage.updated`, `session.synthetic`,
  `session.compaction.started`, `.ended` and `.failed`, `session.shell.started`
  and `.ended`, `shell.created`, `shell.exited`): Rust writes them to the
  transcript as they come, and the app reads conversations on the engine side.
  The feed's copy goes with transcripts, 4.6.
- `session.updated` (a conversation carried in), `session.viewed`,
  `session.revert.*`, `kuvo.review.updated`, `kuvo.btw.*` and `kuvo.handoff`:
  4.6.
- `kuvo.group.*`: 5.3. `kuvo.limits.updated`: 5.4. `kuvo.auto.updated` (Custom
  Auto's lists): 6.1.

The live case (`agents-codex-live`) ran once on macOS with Codex CLI 0.171.0 on
GPT-6-Luna, picked from the box's own model list. Its turn ran a command for 25
seconds across a release switch. The next resident took up the same app-server
(the same holder and child) and finished the turn, and the command's file was
there. Codex asked no approval for it. The case used a scratch HOME,
CODEX_HOME and PATH, and dropped every inherited variable. The sign-in was a
read-only copy in the scratch Codex home, made minutes after its last refresh,
so Codex had no reason to refresh it. It went with the scratch folder.

By non-whitespace characters, the adapter (`codex` and its modules, without
tests) is 1.10× `codex.ts` and `codex-citations.ts`. The updater is 1.51×
`updates.ts` and `maintenance.ts`. That count includes the session hold,
whose reference is in `sessions.ts`, and leaves out the reference's
installer, which isn't ported here. It's to be looked at again in 9.0.

### Verification

| Platform | agents-codex, Rust | agents-codex, TypeScript | agent-turn-fixture | Live GPT-6-Luna keeper switch |
|---|---:|---:|---:|---|
| macOS | 94/94 | 78/78, 2 skipped (Rust-only) | 6/6 | Passed |
| Linux x86-64, lokum (musl) | 94/94 | — | 6/6 | — |
| WSL2, Ubuntu 24.04 (musl) | 94/94 | — | 6/6 | — |
| Native Windows x64 | 94/94 | — | 6/6 | — |

| Platform | agents-updates, Rust | agents-updates, TypeScript | agents-session-reads, Rust | agents-session-reads, TypeScript |
|---|---:|---:|---:|---:|
| macOS | 9/9 | 9/9 | 4/4 | 4/4 |
| Linux x86-64, lokum (musl) | 9/9 | — | 4/4 | — |
| WSL2, Ubuntu 24.04 (musl) | 9/9 | — | 4/4 | — |
| Native Windows x64 | 8/8, 1 skipped (links to programs are Unix's) | — | 4/4 | — |

On Windows the PATH finds `codex.CMD`; the command shown to update it by hand is still
"codex update", as people type it.

"Two asked in one turn" answered before Codex began the sleep it asked from, on WSL, and that answer was told again as the sleep began, as the reference does. The case now waits for the sleep, as the next case does.

Seen once: one case of agents-codex failed against the reference on macOS after the merge of main at c8d59d9da and passed on two reruns (unnamed in that run's output).

The reference's answer to the sleep that replaces one that turned it down (case 47) is the answer as it was or as retold, depending on which reaches it first. The case accepts either.

On Windows, a cross build's `libunwind.dll` sits beside the box. The keeper
cases put that folder on the PATH.

## Background work priority (4.9)

An agent's process and the commands it starts are background work. The resident,
keeper and holder (which reads the agent's output and sends its stream to the
resident) keep their ordinary priority. User-opened terminals keep their ordinary
priority; terminals launched by agents inherit the agent's lower priority. There
are no reserved cores. A new agent or heavy background start waits during critical
memory pressure without stopping work already running. Linux uses the kernel's
`MemAvailable` estimate with a 64 MiB floor because it has no system-wide critical
pressure classification.

The initially planned switch to ordinary priority when a conversation is opened
was dropped: an unprivileged process on macOS and Linux cannot undo the lowered
nice value, and children launched after promotion would inherit the wrong class.
The agent therefore stays at low priority from launch; the conversation's holder
and resident do not. No privileged helper or desktop change is needed.

The existing holder, agent and engine-session cases exercise the same conversation
routes, transcripts and handover on both implementations; there is no TypeScript
priority policy to port. `kuvo_os`'s priority-tree scenario queries the class of a
real child and the ordinary process it launches, without altering the parent.
The memory-floor scenario supplies critical pressure at the OS seam and proves a
real child is deferred until pressure clears. The wake-up tool now uses the same
process owner and priority seam as the other agents' commands.

### Responsiveness on macOS

An exact build of the preceding revision and the new build each ran three idle
turns and three turns while ten bounded CPU burners ran as ordinary descendants
of a stand-in Codex agent. The measurements start at the desktop prompt and end
at the first feed event announcing the single immediate reply's completed turn;
live step deltas in this feed belong to 4.6. Each phase started below a one-minute
load of 8. No test used the user's conversations or working folders.

| Milliseconds, three samples | Idle | With ten CPU burners |
| --- | --- | --- |
| Before | 100, 113, 118 | 239, 115, 246 |
| After | 125, 132, 146 | 102, 121, 115 |

Before, the resident, holder, agent and all ten children reported nice 0. After,
the resident and holder reported nice 0 and the agent and all ten children nice
20. The loaded reply after the change took 64/81/75 ms to the agent's emission and
38/40/40 ms from that emission to the desktop feed, within one second of idle.
The baseline was already within the target under this bounded synthetic load;
the result does not claim to reproduce every kind of real machine contention.

The two-turn Claude conversation differential was also run against the reference,
the preceding Rust revision and the new revision. The Rust-to-Rust request
answers matched. The TS-to-Rust comparison reports the same 34 request-difference
paths before and after (transcript metadata, generated message IDs and shape
differences already outside this row). Complete inventories are not byte-identical:
the two Rust runs differ in per-run identity, timestamps, SQLite fingerprints and
the asynchronous Insights scan's progress when the fixture stops. No new saved
priority or memory-floor state is introduced.

An agent start waiting for memory can be cancelled by Stop, replacement or
shutdown before it launches a holder; a start already admitted still uses the
holder's bounded handshake and normal turn lifetime. The old shared engine's
supervisor waits inside its start operation, so an outstanding start at critical
pressure can defer that supervisor's drain command until pressure clears. The
legacy engine launch is being removed in 4.7d rather than adding another
supervisor mechanism here.

### Verification

| Platform | Parent / agent / ordinary descendant priority | Memory admission |
| --- | --- | --- |
| macOS | 0 / 20 / 20 in the normal-priority measurement; seam scenario also passed in the low-priority test lane | Defers, resumes and cancels immediately; real holder creates nothing on cancellation |
| WSL2 (static Linux musl) | nice 0 / 15 / 15; agent and descendant I/O class 24576 (idle) | Defers and resumes |
| Native Windows | normal (32) / below normal (16384) / below normal (16384) | Defers and resumes |

The PC process lists were clear before and after and the disposable folders were
removed. The Linux proof ran in WSL; there was no matching pre-existing container
image for the capped run on lokum. The platform readbacks preceded the
platform-independent cancellation wiring; its new tests passed on macOS.

Final conformance (engine-sessions, holder, all actual agent adapters, account and
session reads, agent turn fixture, agent tools, wakes, agent bots and engine
golden): Rust 348 passed of 349, with the existing 4.6 seen-cursor TODO; the
reference 325 passed of 349, with 24 expected skips. Both engine-sessions runs
passed 81/81. The fixture-enabled Rust binary passed all eleven account/session/
turn fixture cases, including Stop and keeper handover. The real Claude adapter
case cancels a new start without creating a holder, then starts and hands over
the conversation normally. Its existing account-probe fixture now seeds dummy
local sign-in files, as required by the file-only check introduced in 4.8.

The final differential again matched the Rust-to-Rust request answers, with no
new TS-to-Rust request difference paths. One real OpenCode Go turn on DeepSeek
v4.1 Flash passed in the isolated live case; its scratch key and home were
removed. Workspace test-target and TypeScript type checks passed.

## 4.11 — letters now, and next-step delivery

Deliberate difference from TypeScript, requested by the user (Work Queue #5057):
`send_message` uses Send Now by default, even for a busy conversation owned by a coding
agent. `delivery: "steer"` waits for its next step; `delivery: "queue"` waits for its
turn. An idle conversation starts a turn, unless it was stopped with a held queue.
Local confirmed answers retain `text` and `isError` and add a delivery disposition;
mail to another computer remains asynchronous and cannot promise its eventual
disposition before the receiving computer confirms it.

- Claude Code and Codex hand the input to the running native turn, with the existing
  native receipt gate. A confirmed native refusal of Send Now uses its interrupt/resume
  fallback; an uncertain receipt is not sent twice.
- Grok, Antigravity and OpenCode share ACP: a second active prompt is not supported
  ("Session already has an active ACP prompt"). Send Now cancels the active prompt
  and starts the selected input; a next-step request remains marked until a
  tool finishes or a step ends, then uses that same path. The adapter emits an
  internal `StepBoundary` for that real boundary (including a hidden tool);
  synthetic closing rows of a failed or stopped turn do not fire it. No pending
  input means no queue file read on step notifications.
- The engine's native next-step inbox remains in use until 4.7d removes that place.
- Cursor is added by 4.5: its adapter must add the case from
  `agents-steering.test.mjs`—hold a running prompt at a tool step, write a default
  letter and an explicit next-step message, assert their delivery before the
  original turn ends (or the same cancel/resume at the boundary if no native steer),
  once each. Then check that an explicit after-turn message waits. An adapter
  without native steering emits `StepBoundary` only while its turn is actually
  advancing, not as it writes the closing rows.

| TypeScript scenario or contract | Rust proof |
|---|---|
| `agent-tools.test.js`: 171 named letter, 180 stopped queue, 220 legacy wait, 226 no such name, 235 message limit | `tools`, `agent-tools.test.mjs` (receipt and new timing), `work-elsewhere.test.mjs` (remote letter timing) |
| `agents/sessions.test.ts`: 261 message while working, 292 Stop holds, 324 idle queue, 368 program drain | `kuvo_agents/tests/inbox.rs`; new delivery flag remains a steer rather than being downgraded for a non-steering agent |
| `agents/sessions.test.ts`: 1610 native steer, 1632 non-native steer waits for turn | `agents-steering.test.mjs` (Claude Code, Codex, Grok, Antigravity); the non-native case deliberately now fires at a reported boundary |
| Queue Send Now and steer, stopped/held line, retry and reorder (`queue-items.test.ts`, `queue-stop.test.ts`) | Existing `engine-sessions`, new `agent-tools` case; `kuvo_agents/tests/inbox.rs` selected message goes first |
| OpenCode ACP and engine next-step boundary | `agents-opencode.test.mjs`, `engine-steering.test.mjs` on the pinned engine and a labelled model |

The side-by-side synthetic busy-engine scenario compared the same three requests and
both resulting homes. The new reply has only the added delivery field; the new queue
file is unpaused with a resume timestamp, as for Send Now. The engine's message
payload and all other answers match (each box generates its own message ID).
A baseline comparison with the pre-fix Rust binary has the same unrelated home
differences (setup keys, usage index, generated timestamps and runtime directories).
The new field and the saved resume are deliberate, not comparison exclusions.

The existing remote-work approval case explicitly seeds the Auto mode it describes
and checks each implementation's exact refusal: Rust presents it as Ask First until
6.1 (no permission behavior changed).
The shared route fixture now waits for the engine's actual event stream before
injecting a live event. Native Windows repeatedly missed the first finish event
because the initial route answered before the stream subscribed; with the gate,
the unchanged work scenario passed (15/15). One unrelated agent-program-update version
check failed once and passed its single focused rerun; its test already includes
the program's reported state on failure. The unchanged TypeScript Codex background
terminal Stop case timed out once and passed its single focused rerun.

Platform proof — the exact final binaries (cross-checked by hash) ran the new
steering area (11/11), agent tools (4/4), OpenCode (16/16, including its long
active prompt and next tool completion), and the native engine's next-step
case (1/1) on macOS, static Linux musl under WSL, and native Windows. The
preceding cross binaries also passed engine-sessions 81/81, holders 12/12,
tools 10/10 and remote work (WSL 6/6; Windows 5/5 and the Unix-only timer skip).
The final native work suite passed 15/15 with the synchronized fixture. On macOS,
the final engine-sessions (81/81), holders (12/12), Grok (10/10), Antigravity
(11/11), Claude routes (20/20), session reads (4/4), native session and turn
fixtures (10/10), tools (10/10), work (15/15), remote work (6/6), and engine
golden home (1/1) passed; notices passed with the existing machine-side skip.
The final Claude turn and Codex guards passed (59 passed, one existing 4.6 TODO;
100/100, respectively). The move and letter guards passed (70 passed, one
two-binary reach-file comparison skipped). The TypeScript
reference ran the shared engine, agent, tools and work scenarios and the golden
home; new steering behavior is deliberately Rust-only. The inbox integration
tests (28/28) and ACP projection unit tests (4/4) also passed.
All model-facing scenarios here use stand-ins; no live sign-ins were copied.
