# Message delivery

Work Queue #809 · Approved design · status at `36db4ec2e`, 2026-11-22

Too many things fail in Kuvo Code because a client asks a box something and waits for the answer: "The draft's outcome is unknown", a bot's tool call ending "Kuvo on this computer is starting", and the Mini "may be asleep or offline" while its box is only busy. This page says what goes wrong, and the three rules that fix it.

## Baseline measurements, 22 November 2026

- **A send's own work on the box is small**: claiming its id and recording the answer takes 10 ms (median) to 50 ms (worst of 40), on a loaded Mac, in a test home. The seconds come from what the request waits on. It looks up where the session runs 2 or 3 times (each lookup retried after 10 s, then 20 s), then posts to the engine or bridge, with no time limit. The app drew nothing useful until all of that came back. It also drew a message from three streams that don't agree (the composer's copy, the queue, the conversation), so a message vanished between them.
- **A tool call** starts a new process, which gets 5 s for the link to accept it, with no retry (`ssh-pipe.ts`). While the link starts or restarts, the pipe is chosen before it listens, so every call in that window fails at once.
- **"Asleep or offline"** is decided by silence, not by an answer: the relay drops a box that missed a 30 s ping, and the app ends its session after 5 s plus 15 s without a pong. The pong comes from the link's JavaScript loop, so **a box that's blocked looks the same as one that's gone**.
- **Load versus design.** Under load, the link blocks its own loop with work done inside requests: file locks that wait (`Atomics.wait`, up to 3 s each), fsync chains, SQLite, `spawnSync git`, `ps` on every request on Macs, an attachment sweep on every send, and session lists read again on every tool call. By design, any lost answer becomes a dead end: the app never asked again, and the box answered "unknown" to a send that a restart cut off.

## Three rules

1. **Send once, deliver once.** Every send gets its id when you press Send, and so does every tool call. A client offers the same id again until its box confirms it. The box keeps each id once: asked again, it gives the same answer. One it was partway through when it restarted, it settles: delivered if it arrived, sent now if not. Nothing ever says "unknown".
2. **One open line per client.** A client opens one stream per box. The box pushes each change on it (sessions, queues, conversations, asks, bots, the Work Queue, computers), and the client draws the last state it received. A request only changes something; the result comes back on the line. A computer counts as offline only after its line has been silent for a while (the box sends a heartbeat). A slow answer is never read as offline.
3. **The box answers from what it knows.** Requests are answered from state the box already keeps: where each session runs, the session list, its queue. Slow or blocking work (git, the engine, fsync, sweeps) runs off the request path, and its result arrives on the line. The link's loop never waits on a lock or a child process.

## What the completed design removes

- "The draft's outcome is unknown", and the client-side fence that kept a lost send from being asked again.
- "Kuvo on this computer is starting" for tool calls: the call waits for its id to be taken, through a restart.
- "Asleep or offline" for a busy computer, and the separate rules for pings, `away_after` and checks.
- The 6 to 10 follows each computer's view opens today (one per list), and request/response reads such as the Work Queue's.
- Messages vanishing between the queue and the conversation, and a multi-second "Sending…".

## Removing the computer picker

The picker still ships. Its approved removal depends on these changes:

- **Merged lists.** Sessions, bots and the Work Queue show all your computers, each row carrying its computer. The Work Queue (`kuvo/work-queue?owner=all`, with copies in `run/work-queue/peers/`), bots (`kuvo/bots` `elsewhere`, from `run/peer-agents.json`), computers (`kuvo/computers/events`) and the Library are setup-wide. The original session list (`kuvo/sessions/list`), waiting (`kuvo/waiting`) and message queue routes still read one box. Their setup-wide topics use `kuvo/setup/…/events`.
- **How the lists travel.** Each box mails its session summaries, waiting items and queues to its peers. It keeps the newest copy it received from each peer (`run/peers/<key>/lists-v1.json`). The one line carries the merged lists from the box the app is nearest to, each row with `computer`. A computer that's off shows its last copy, marked with when it was seen. Actions that need it are hidden.
- **Runs on.** The new-session composer gets a "Runs on" choice next to the model. Starting a session or bot goes to that computer's box.
- **Per-session things** (files, terminals, the browser, Changes) follow the session's computer. They use that box's routes, as they do today.
- **Settings** for each computer live under Computers › that computer.

## How it fits #924

[#924](one-client-process.md) gives every computer one resident box process, with SSH piped into it. That's the process the one line comes from. Its pipe should queue a call until the process is ready, instead of failing after 5 s. Rule 3 keeps that one process free to answer.

## Already built

- GPUI shows a send at once, keyed by its ID, and keeps it until the conversation has it.
- GPUI retries an interrupted send with the same ID. The box settles an interrupted
  claim against its queue or transcript. The agents bridge persists queued messages.
- Machine and compatibility tool calls wait for the resident box. Sends, session
  starts, Work Queue edits and approved commands keep restart receipts. Durable
  delivery for all tool calls is still partial.
- Work Queue imports and numbering, move bundles, attachments, release
  verification and some sandbox work have moved off the resident loop.
  Other blocking work remains.

- The box shares session summaries, waiting and message queues, and its new-session model and
  agent catalog with its peers. The line serves setup-wide lists and a Runs on catalog. Away
  rows keep their last-seen time and hide their actions. Starts are addressed to the chosen box,
  where its identity is checked before it starts the session (branch `kuvo/delivery-4-merged-lists`).
- The nearest box can open a bot's chat or a session on another computer and carry its live turn,
  queue and asks on the same line. The nearest box queues messages in its durable outbox and
  delivers them to the owner.
- The nearest box keeps the newest page it carried of each such conversation
  (`run/peers/<key>/conversations/`, without inline attachments) and answers it, marked `away`,
  while that computer is restarting, updating or off. The desktop reads a remote conversation's
  newest page through its own box, so it shows that last transcript with one quiet line instead
  of an empty page (`line.kept-conversation`, PROTOCOL.md).

Still to build or verify end to end: the desktop's switch to merged topics, picker removal
and Settings relocation, the rest of the off-loop work, and full tool-call
delivery and restart coverage.

## Building: the line (branch `kuvo/delivery-3-one-line`)

Rule 2, for one computer at a time. The merged lists come later and ride the same line.

**Added on the box.** One route, `kuvo/line` (feature `line`), answered in whichever process took the request: the resident serve, or the relay host where the launch is still split.

- `GET kuvo/line?line=<id>&after=<n>&watch=<topics>` opens it. `watch` holds the client's topics, `{name: route}`, as base64url JSON. Every topic is a route the box already follows: a list's `…/events` (one JSON line per state), or a conversation's events (`api/event`, `kuvo/agents/api/event`). The box runs each one through its own dispatcher, under the same pairing and authority as the line.
- `POST kuvo/line/watch` with `{line, watch}` changes the topics. It answers at once. What it changes comes back on the line.
- **What comes on the line** is one JSON line each. Every line but the heartbeat has its number `n`, counted per line:
  - `hello`: `{line, fresh, beat}`, sent first.
  - `{t, n, d}`: what topic `t` said: a state, or one event.
  - `{t, n, refused: {status, message}}`: the box turned the topic down. It ends after a 4xx. After a 5xx (the engine isn't ready), the box tries it again by itself.
  - `{t, n, gap: true}`: an event topic's source dropped and came back, so events may be missing. The conversation is read again.
  - `{beat}`: every 5 s, with no number.
- **Replay rule.** The box keeps each line's last 4 MB of numbered lines. Its topics keep running for 2 minutes after the stream drops. A reconnect with the same `line` and `after` gets every line after `after`, then the live ones. If the box doesn't know the line (it restarted, or the 2 minutes passed) or the lines are gone, it answers `fresh: true` with a new id. Each topic then starts over from its current state, and the client takes those as a snapshot.
- On a box that can't keep a stream (a one-shot SSH `box route`), `once=1` answers each topic's current state and ends. That client asks again every 5 s, with one request for all topics.

**Added in the client** (`gpui/kuvo_box/src/line.rs`). One `Line` for each computer's `BoxClient`, started by the first view that follows something.

- It reconnects with its position after any drop.
- 15 s without a line or a heartbeat counts as a drop.
- The line's silence is what makes a computer offline. "Offline" shows only after 30 s of silence, never because one request was slow or failed.
- A drop is never handed to a view. Each view keeps drawing the last state it received, its asks and queue included, through a restart.

**Removed.**

- Every separate follow stream: the session list, the conversation's events, its asks, queue, subagents and side threads, bots, group chats, the computers list, notices, usage, waiting, moves, the Library, Settings pages, Changes, wake-ups, background work, accounts, and the `poll_if_missing` fallback. Each `follow_*` in `kuvo_box` stays as the view's handle and becomes a topic on the line.
- The polling reads the line replaces: the conversation's 5 s fallback read, the lists' 30 s reads, the per-stream backoffs and quiet-retry rules, and `EVENT_QUIET_TIMEOUT`.
- "What it asks you couldn't be read" and "The queue couldn't be read" for a dropped connection.
- A computer's `Away` coming from one failed `kuvo/info`.

**Also built.**

- An event topic whose source says nothing at all for a minute is opened again by the box, with a gap. A stalled engine stream never freezes a conversation.
- A queued message still on its way keeps Steer, Send Now, Edit and Remove. `DELETE …/queue/<id>?arriving=1` takes it out as it arrives (`409`, asked again), or says it went to the agent already (`404 delivered`).

**Next, on the same line.**

- A box-side snapshot of the open conversation, so the client stops reading its page after each step.
- The Work Queue's own events, which replace its request/response reads.
- Steer and Send Now settled by the box for a message still on its way.
