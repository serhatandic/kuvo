# Message delivery

Work Queue #809 · Draft for the user's OK, 2026-11-22

Too many things fail in Kuvo Code because a client asks a box something and waits for the answer: "The draft's outcome is unknown", a bot's tool call ending "Kuvo on this computer is starting", and the Mini "may be asleep or offline" while its box is only busy. This page says what goes wrong, and the three rules that fix it.

## What we measured

- **A send's own work on the box is small**: claiming its id and recording the answer takes 10 ms (median) to 50 ms (worst of 40), on a loaded Mac, in a test home. The seconds come from what the request waits on. It looks up where the session runs 2 or 3 times (each lookup retried after 10 s, then 20 s), then posts to the engine or bridge, with no time limit. The app drew nothing useful until all of that came back. It also drew a message from three streams that don't agree (the composer's copy, the queue, the conversation), so a message vanished between them.
- **A tool call** starts a new process, which gets 5 s for the link to accept it, with no retry (`ssh-pipe.ts`). While the link starts or restarts, the pipe is chosen before it listens, so every call in that window fails at once.
- **"Asleep or offline"** is decided by silence, not by an answer: the relay drops a box that missed a 30 s ping, and the app ends its session after 5 s plus 15 s without a pong. The pong comes from the link's JavaScript loop, so **a box that's blocked looks the same as one that's gone**.
- **Load versus design.** Under load, the link blocks its own loop with work done inside requests: file locks that wait (`Atomics.wait`, up to 3 s each), fsync chains, SQLite, `spawnSync git`, `ps` on every request on Macs, an attachment sweep on every send, and session lists read again on every tool call. By design, any lost answer becomes a dead end: the app never asked again, and the box answered "unknown" to a send that a restart cut off.

## Three rules

1. **Send once, deliver once.** Every send gets its id when you press Send, and so does every tool call. A client offers the same id again until its box confirms it. The box keeps each id once: asked again, it gives the same answer. One it was partway through when it restarted, it settles: delivered if it arrived, sent now if not. Nothing ever says "unknown".
2. **One open line per client.** A client opens one stream per box. The box pushes each change on it (sessions, queues, conversations, asks, bots, the Work Queue, computers), and the client draws the last state it received. A request only changes something; the result comes back on the line. A computer counts as offline only after its line has been silent for a while (the box sends a heartbeat). A slow answer is never read as offline.
3. **The box answers from what it knows.** Requests are answered from state the box already keeps: where each session runs, the session list, its queue. Slow or blocking work (git, the engine, fsync, sweeps) runs off the request path, and its result arrives on the line. The link's loop never waits on a lock or a child process.

## What this removes

- "The draft's outcome is unknown", and the client-side fence that kept a lost send from being asked again.
- "Kuvo on this computer is starting" for tool calls: the call waits for its id to be taken, through a restart.
- "Asleep or offline" for a busy computer, and the separate rules for pings, `away_after` and checks.
- The 6 to 10 follows each computer's view opens today (one per list), and request/response reads such as the Work Queue's.
- Messages vanishing between the queue and the conversation, and a multi-second "Sending…".

## Removing the computer picker

The title bar's picker goes once this is approved. The app needs:

- **Merged lists.** Sessions, bots and the Work Queue show all your computers, each row carrying its computer. Today the Work Queue (`kuvo/work-queue?owner=all`, with copies in `run/work-queue/peers/`), bots (`kuvo/bots` `elsewhere`, from `run/peer-agents.json`), computers (`kuvo/computers/events`) and the Library are setup-wide. Sessions (`kuvo/sessions/list`), what waits on you (`kuvo/waiting`) and the message queues are this box's only, and no box keeps another's sessions.
- **The simplest way.** Each box already sends its peers its Work Queue and its agents. It sends its session list and what waits on you the same way, and keeps the newest copy of each peer's (`run/peers/<key>/…`, with `at` and `away`). The one line carries the merged lists from the box the app is nearest to, each row with `computer`. A computer that's off shows its last copy, marked with when it was seen. Actions that need it are hidden.
- **Runs on.** The new-session composer gets a "Runs on" choice next to the model. Starting a session or bot goes to that computer's box.
- **Per-session things** (files, terminals, the browser, Changes) follow the session's computer. They use that box's routes, as they do today.
- **Settings** for each computer live under Computers › that computer.

## How it fits #924

#924 (`one-client-process.md`) gives every computer one resident box process, with SSH piped into it. That's the process the one line comes from. Its pipe should queue a call until the process is ready, instead of failing after 5 s. Rule 3 keeps that one process free to answer.

## Already built (branch `kuvo/809-seamless-send`)

These follow rule 1 and the drawing half of rule 2:

- The app draws a message at once, where it goes, keyed by its id, and keeps it until the conversation has it.
- The app asks the box again under the same id when an answer is lost.
- The box settles a send that a restart cut off.
- The bridge keeps queued messages across its restarts.

Still to build: the line, rule 3, ids for tool calls, and the merged lists.
