# Kuvo Link

How boxes and clients talk. Every computer runs a box (`box/src`). Kuvo Code's desktop app
(`gpui/kuvo_box`) reaches its own computer's box over loopback HTTP, and your other computers' boxes
through the relay or SSH. Boxes talk to each other as peers, by end-to-end encrypted mail through
the relay. Read [How Kuvo works](../docs/architecture/how-kuvo-works.md) first.

**What's where.** Every route is documented in its definition in `box/src/shared/routes`: its
feature, what it takes and answers, and a comment saying what it's for. `scripts/build-built.sh`
writes them into `protocol/ROUTES.md`, a readable list kept out of git. This file holds only what a route can't say:
the principles, the wire below the routes, and the few rules per area that a client must know.
Add a rule here only when the route can't say it, and keep it to a few lines. Shared test vectors
are in `vectors/`.

**Principles.**
- **A box decides, a client draws.** A box answers facts in its own words: labels, confirmations,
  why something can't be done, and the actions a row offers. A client shows them. It works nothing
  out that another client would need too.
- **Equal computers.** Each box works with no client and no hub. Lists merge across computers.
  Each row names its computer (`computer: {id, name}`), and a client sends a row's actions to that
  computer's own box.
- **Ids, and delivery once.** A client gives what it sends (a message, a draft, a save) an id of
  its own. It sends it again under the same id until a box says it has it. A box takes an id once.
- **Features, not versions.** A client checks `kuvo/info`'s `features` before it uses a route
  ("Features and mixed versions").

The retired Swift Mac app and iPhone app left wire names behind (`phone`, `phones`, `host`, `mac`)
and the relay's push kinds. Nothing new uses them.

## Keys and addresses

Each device (a computer's box, a desktop app) has:
- a **static key** (X25519), which identifies it to those it's paired with;
- a **relay secret** (32 random bytes), shown only to the relay.

Its **address** is the first 16 bytes of `SHA-256("kuvo-link address v1" || secret)`. The relay
checks a connection's secret against the address it claims, so only the device itself receives
what's sent to its address. In text (links, JSON), keys, secrets, addresses and tokens are
base64url without padding.

## The relay

One WebSocket per device, binary messages, the first byte the kind (`relay/`).

| Kind | From | Bytes | Meaning |
| --- | --- | --- | --- |
| `0x01` hello | device | `secret(32)` | Comes first. A second connection with the same address replaces the first (closed 4002). |
| `0x01` welcome | relay | `address(16)` | The secret was fine; this is your address. |
| `0x02` send | device | `to(16) payload` | Pass `payload` to `to`. |
| `0x02` deliver | relay | `from(16) payload` | Something sent to you. |
| `0x03` unreachable | relay | `to(16)` | Nothing is connected at `to`; what you sent is dropped. |
| `0x04`–`0x06` | | | Push registration and pushes for installed phones (APNs). No current client uses them. |
| `0x07` gone | relay | `address(16) reason(1)` | A device you sent to on this connection has a new connection (0) or none (1, or any other value). Your sessions with it are over. |

- Known kinds keep their lengths. Once welcomed, a device skips a kind it doesn't know, so a new
  kind goes only after the welcome.
- The relay handles one connection's sends in order. What comes back for them (`unreachable`, and
  the `deliver` of a send to the connection's own address) comes back in that same order. A box
  relies on this to tell a start the relay dropped from one that got there.
- `gone` is sent once and never queued. It goes, before anything the new connection sends, to every
  device that this connection sent to or received from (up to 1,024 and 256). Each of them ends its
  sessions with that address. A handshake already on its way starts over under a new session number.
- A box never forgets a session while its relay connection stays up. It ends one only by telling
  the other side (a close), or because of what the other side did: a close, a new handshake,
  `unreachable` or `gone`. A frame on a session the box doesn't have gets a close.
- `payload` is at most 65,600 bytes. The relay closes a connection for anything bigger (4000), a
  wrong secret (4001), far too much too fast (4003), or too many from one IP address (4005), and
  holds back one a little too fast. A device counts as there while it answers pings or sends
  anything, so a client keeps its pong from waiting behind a big body.

## Link frames

Inside the relay's `payload`: `kind(1) session(4) body`. `session` is a random number the client
picks for each session, so frames from an older session are never taken for a newer one's.

| Kind | Body |
| --- | --- |
| `0x01` | Handshake message 1, client to box |
| `0x02` | Handshake message 2, box to client |
| `0x03` | One encrypted transport message ("Requests on a session") |
| `0x04` | Close (no body): either side stops the session |
| `0x05` | Mail: one message sealed for a device that may not be connected (`session` 0; "Mail between computers") |

## Sessions

**Noise_IK_25519_AESGCM_SHA256**, prologue `kuvo-link v1`. The client starts with the box's static
key, and the box checks the client's key in the first message. The payloads are JSON:

- first message: `{v: 1, name, app, client: "desktop", pair?}`. `pair` is there only while
  pairing ("Pairing"). If `client` isn't `desktop`, the client is an installed phone.
- answer: `{v: 1, kind: "mac" | "machine", name, app, engine}`. The box answers only a key it has
  paired, or the token it's showing; anything else gets a close. Each side closes on a `v` it
  doesn't know.

After that, transport messages go both ways, in order. One that doesn't decrypt ends the session.
On a legacy setup, a pairing that waits for a standby-backed save answers `waiting` and `save`, and
opens no session. The client then retries with `save` in its first message.

**Reach.** A computer is away only once its line has been silent for 30 s; after 15 s, a client
reopens the line. A slow or failed request never makes a computer away, a dropped stream is never
shown (each view keeps its last state), and a read that failed because the link went is read again.

## Requests on a session

- Each transport message decrypts to `headLength(2) head(JSON) body`, at most 65,519 bytes. A
  longer body goes in pieces.
- Heads: `{t: "req", id, m, p, h, end}` (`id` counts up from 1 per session; `end: false`: `data`
  pieces follow), `{t: "res", id, s, h, end}` with the body's first piece, `{t: "data", id, end}`,
  `{t: "cancel", id}` from either side, and `{t: "ping"}`/`{t: "pong"}`. Event streams go on until
  they're cancelled.
- A client pings while a request waits with nothing back. With no answer, it ends the session
  after 15 s, plus 1 s for each 64 KB it sent that hasn't been answered yet. A ping is answered at
  once.
- `h` carries `x-opencode-directory`, `content-type`, `idempotency-key` and `upgrade`. The box adds
  the engine's password itself.
- **Paths.** A route is picked by its path before `?`; methods can be any case. Each path part and
  query value is percent-decoded once and encoded again (RFC 3986), and empty parts are dropped.
  The box answers `403` for a fragment, a space at either end, a bad escape, a backslash or control
  character, `%2F` inside a part, a `.` or `..` part, or `..` in a query value.
- **Shapes.** Every answer from a box's own routes matches its shape in `box/src/shared/routes`: only the named
  fields, and a nullable field sent as `null`, never left out. A refusal is JSON (`{message}`, or
  `{}`), never an empty body. A `kuvo/…` route the box doesn't have answers `404 {}`.
- `api/…` goes to the engine, and `kuvo/agents/…` to the coding agents' bridge, on the same paths
  (`503` while `kuvo/info`'s `agents` is false). Sessions on the coding agents carry
  `metadata.kuvoAgent`. A new session on one of their providers is made there, never on the engine.
- **Sockets.** A request with `h.upgrade = "websocket"` (today only `api/pty/<id>/connect`) has an
  empty body. The box answers `res` with `s: 101` once the socket is open, or with another status
  and `end: true`. After that, each `data` frame for that `id` is one socket message, either way: text, or binary
  with `h.type = "binary"`. The client closes with `cancel`, the box with `data` `end: true`. On
  the box's own HTTP server, the same upgrade takes Basic auth.

## Sending without waiting

The full rules are in [Message delivery](../docs/architecture/message-delivery.md). What a client relies on:

- A send gets its id when you press Send. The client draws it at once, keyed by that id, keeps it
  until the conversation has it, and retries only under that id.
- A box takes an id once (`link/server/sent-once.ts`: `idempotency-key` on `…/prompt`,
  `kuvo/sessions/<id>/send`, `…/queue/add`). A repeat waits for a send still on its way, gets the
  same answer for an hour after one that went, and is tried again after one refused or never
  delivered. A message taken out of the queue is forgotten, so Undo can add it back under its id.
- A request that arrived whole goes on even when its sender's session ends.
- After a restart, the box checks a pending send against the queue and transcript. It counts it
  delivered if it got there, and sends it now if it didn't.
- Routes that make something take an operation id (`request` on `POST kuvo/bots`,
  `x-kuvo-operation` on `POST kuvo/work-queue/items`). The same id gets the same answer.

## The line

Feature `line`. A client opens one stream to each box, and everything it follows there comes on it.

- `GET kuvo/line?watch=<topics>[&line=<id>&after=<n>][&once=1]`. `watch` is base64url JSON
  `{name: route}`. Each route is one the box already follows: a list's `…/events` (one JSON state a
  line) or a conversation's events. The box runs each topic through its own routes as the line's
  asker, so a topic is allowed exactly what its route is. `POST kuvo/line/watch` changes the
  topics. A `404 {fresh: true}` means the box no longer knows the line: open it again with every
  topic.
- On the line, one JSON object a line:
  - `{hello: {line, fresh, beat}}` first;
  - `{t, n, d}`: what topic `t` said (a state, or one event);
  - `{t, n, refused}`: after a 4xx the topic ends; after a 5xx the box retries it;
  - `{t, n, gap: true}`: an event source dropped or went quiet. Read what it's about again;
  - `{beat}`, every `beat` ms while the line is quiet.
- **Resuming.** Reopen with the same `line`, and the last `n` as `after`. If the box still keeps
  the lines (4 MB, for 2 minutes after the drop), it replays every one after `after`, with
  `fresh: false`. Otherwise it answers `fresh: true` with a new id, and each topic starts over from
  its state now. Read again whatever the event topics are about.
- **Once.** Where no stream can be kept (a one-shot `box route` over SSH), ask with `once=1`. The
  box sends the hello and each topic's state, then ends. Ask again every few seconds.
- **Older box.** A `404` for `kuvo/line` means a box from before the line, which is updating. Retry
  as after a drop, keep what each topic last said, and never show the `404`.

Setup-wide topics publish the kept peer copies in their first state. `pendingComputers`
names reachable setup members whose first copy is still coming. An empty copy is complete.
Away peers keep their last-seen rows and cannot act on this line.

Notice IDs remain stable with their owner. Read the setup's `dock` count directly to avoid
counting an ask twice across Sessions and Bots. Activity counts only current work and exposes
last-seen sessions separately. Activity Output bytes and archived-session actions go to each row's
owner. The archive owner rechecks its own confirmation revision before a write.

A conversation's owner supplies its directory and tool readiness. Search asks reachable peers
at query time and reports `awayComputers` separately from `unavailableComputers`. The matching
snippets travel in memory and each owner keeps its own transcript index.

## Setup-wide lists

Features `line.setup-lists`, `line.merged-sessions` and `line.merged-waiting`. The nearest box
merges what every computer in the setup last told it. A client follows the merged topics on that
one box's line (`kuvo/setup/…/events`). Each topic sends a whole snapshot.

- Every row has `computer: {id, name}`; key rows by computer and row id. The answering box's rows
  have `here: true`. Another computer's have `away`, and while it's away `lastSeen` (ms). One rule
  says it, the box's (`standingOf`): a computer quiet under a minute isn't away, nor one that said
  less than ten minutes ago it was switching releases, which has `away: false` and `updating: true`
  instead (show its rows as they are, saying "Updating…"). Runs on still offers neither.
- Rows carry `actions` (`open`, `manage`, `review`, `pause`, `resume`, `messages`); away rows have
  none. A client sends each action to the row's own box, which checks it.
- A remote project's key is `<computer-id>:<directory>`. `kuvo/sessions/page` pages it from the
  answering box's last copy, even while that computer is away.
- **Runs on.** `kuvo/setup/runs-on/events` lists where a conversation can start. This computer
  comes first (`here`), with `pending` until its own catalog is read. To start one, send
  `POST kuvo/sessions` to the chosen box with its id as `computer`. A wrong box answers `409`
  before it makes anything.
- `kuvo/setup/waiting/counts` gives setup totals. `kuvo/waiting/events` stays per computer.
- **Another computer's conversation.** `GET kuvo/sessions/<session>/location`, asked of the
  nearest box, says where a conversation lives, from that box's kept lists alone. It answers `404`
  only when the conversation is unknown, and `503` while its own list can't say yet. To read the
  conversation through the nearest box, use `kuvo/peer-session/<computer>/<session>/<resource>`, as
  a GET or a line topic. Actions go to the owner's own routes.
- **Its last copy** (`line.kept-conversation`). The nearest box keeps the newest `messages` page it
  carried of each such conversation (64 for each computer, 2 MiB each, inline attachments cut).
  While that computer isn't answering, the same read answers `200` at once with that copy and
  `away: {computer, lastSeen?, updating?}`. Show it, readable, with one quiet line, keep the
  composer, and read again until the page comes without `away`.
- **Composer delivery** (`typed.delivery`). A client posts a draft, with its own id, to its nearest
  box (`…/delivery?computer=<id>`), which records it before it answers `202` and delivers each
  conversation's oldest draft first, with no client connected, retrying while the owner can't
  answer. The owner takes each id once; only its receipt moves delivery to the next draft, and a
  known move stops delivery rather than reach a second owner. A draft still at the nearest box can
  be removed (with a `receipt`, then `…/ack`), steered or sent now there.

## Features and mixed versions

Computers in a setup update one after another, and a box meets clients and peers of other versions.

**Features.**
- `kuvo/info`'s `features` names what a box answers. A client checks a name before it shows
  anything that needs it, and never compares `app` or `bridge` versions.
- A name never changes meaning, and a client ignores names it doesn't know. A named route can
  still refuse for now, with a message to show: `503` while the engine or bridge is down, or `409`
  for a machine's settings routes until its settings are its own.
- Peer mail is gated by the peer's features too (`work-queue.forward`, `bots.held` and others). Such
  mail waits until the peer's list names the feature.
- The lists are `FEATURES` in `box/src/shared/features.ts` (every box), and `MACHINE_FEATURES` and
  `MAC_FEATURES` in `shared/routes/mac.ts`; every route names one. Add a name on a line of its own,
  in `LC_ALL=C sort` order. Git merges the file by union, so change nothing but names. `kuvo/info`,
  `api/…` and sockets are always there.

**Format versions.** Every stored file and every wire body says which format it is, and readers take
every older one. The inventory is in [stored formats](../docs/architecture/stored-formats.md).
- A Kuvo folder's layout is in `folder-version.json` (no file: layout 0). Before it writes
  anything, the startup gate refuses a future layout, or a migration under way to one. Each step
  from n to n+1 is tested, can be replayed, and is checkpointed in that same file. Layout 1 keeps
  the existing files, so builds from before the markers still run on them after a rollback.
- JSON files carry `formatVersion` (none: version 0). Readers upgrade older files in memory and
  write the version at their next ordinary write. They report a version they don't support before
  writing anything over it.
- Peer mail keeps the Noise prologue. The JSON inside carries `formatVersion: 1`; with none, it's
  the original body, same shape. A box drops mail of a version it doesn't know before dispatch,
  without acknowledging it, and logs it.
- Routes: the desktop sends `x-kuvo-route-version: 1`, and replies say `routeVersion`. A request
  with no version gets the original shapes. A request for an unknown version gets
  `400 {code: "unsupportedFormatVersion"}`, and an unknown stored format found before the reply
  starts gets `409` with that code. A client takes replies with no marker and declines an unknown one.

### The update gate

Each box reports `compatibility: {state, missing?}` in `kuvo/info` and on each Computers row,
measured against `gate-required.txt`. A client draws it: `open` (the box has everything), `behind`
(it works with the features it has, and offers an update) or `held` (the client shows the update in
place of the areas). A box with no `compatibility` is used as it answers. A target that's staged, or newer, never
refuses work by itself. Older builds may still answer `503` or `426`
(`computer-update-required`, `client-update-required`), or refuse in the SSH hello. Peers judge
each other only by the `features` they advertise.

## Mail between computers

Boxes send each other mail through the relay. Each mail is a relay `send` of a mail frame (kind 5,
session 0), sealed with **Noise_K_25519_AESGCM_SHA256** (both static keys known), prologue
`kuvo-link mail v1`, in one handshake message. A sealed frame is at most 65,600 bytes; text is cut
to fit, and big payloads go in parts. Every mail carries `at` (ms). A box opens mail only from
computers in its setup.

### Documents

What the setup shares travels as signed documents. Each origin signs its own, any box passes a
copy on, and no clock decides anything.

- **Keys.** Besides its static key, each box has an Ed25519 signing key. Trust comes from
  enrollment: a member gets the main's signing key, and the main gets the member's. The main's
  `peers` document names every box with both keys. A box made anew is a new origin.
- **Format** `{d, s}`, base64url. `d` holds the JSON `{v: 1, kind, origin, n, body}` exactly as
  the origin wrote it. `s` signs `"kuvo-link doc v1\0"` followed by those bytes. Boxes keep and
  forward the bytes unchanged. `n` goes up by one with each change (from 1 to 2^53−1). A document is
  at most 512 KB. Vectors: `vectors/docs.json`.
- **Which copy wins.** A verified copy wins when it comes from an origin this box takes that kind
  from and has a higher `n`. At equal `n`, the copy whose bytes' SHA-256 sorts first wins, so every
  box keeps the same one. A box that sees its own document further on than it knew moves its count
  past it.
- **Kinds** (`shared/documents.ts`): `peers` (the main's roster, with `setup {id, epoch, primary}`),
  `phones` (device grants, "Pairing"), `personalities`, `setup-shared`, `setup-checkpoint` and
  `leadership` (legacy). A member takes `peers` only from its setup's main, and never at a lower
  epoch.
- **Exchange** (`docs` mail): a box sends `{give}`, or a big document in parts (40,000 characters
  each). It also sends a digest, `{have: [[origin, kind, n, h]]}`, when it connects to the relay and
  every ten minutes. A box that gets a digest answers with what the sender lacks, and with its own
  digest when the sender has something newer. A document a box takes goes on to the other peers
  that would take it, never back to where it came from.
- **After reconnecting**, a member asks the main (`phoneSync`) for its signed `peers`, `phones` and
  shared values, and closes revoked devices' sessions as removals arrive.

## Your setup

One **main computer** (`primary`) signs who is in the setup, and every other computer is a
**member**. `epoch` is the main's term. It goes up by one with each change of authority and never
goes down on any box. The setup's id is `set_` followed by 16 random bytes. The first box you set up
on its own makes epoch 1. See [membership](../docs/architecture/membership-lifecycle.md).

- **Lease.** The main holds a 60-second relay lease (`POST /setup-lease`) and renews it every 10 s.
  When the lease expires, an always-on enrolled computer may claim it at once; others wait one more
  minute. The new holder signs the next epoch's certificate from the accepted snapshot. A transfer
  starts when the previous lease expires, which fences the old holder out. Without a fresh lease,
  the old main can't make setup-wide writes. A computer that's alone in its setup can save offline.
- **Handover** (`setup.handover`). Any member forwards it to the main. The main asks the target, by
  peer mail, to confirm the same setup and epoch. If the target can't confirm, the old main stays.
  To unlink the main while members remain, hand over first.
- **Main only** (`setup.mainOnly`). Only the main adds computers and pairs devices; members forward
  those requests to it. Renames and removals can go to any member.
- **Conflicts.** A member refuses a lower epoch of its setup. If another enroller offers the same
  epoch, the member keeps its main and records `conflict`. A discovery card is a hint and grants no
  trust.
- **Saving setup-wide changes.** The main that holds the lease commits membership, device grants,
  settings, personalities, labels and your starred models, then signs and publishes the accepted snapshot. Members verify
  it, keep it for the next main, and forward setup-wide changes to the main. When authority is stale
  or can't be reached, the operation waits or is refused. An unverified change is never applied.
  `shared/data-declarations.ts` declares the data kinds and their owners.
- **Join code** (`POST kuvo/computers/add/code`): the relay, the main's relay identity and a
  private 192-bit token (only its hash is stored), bound to the setup and epoch, unused for 15
  minutes at most. After a lost answer, the joining computer retries with the same code for 24 h. A
  computer already in another setup must unlink first. Only the sign-ins and keys you picked are
  carried; those already at the destination stay.
- **Merge** (`setup.merge`). The main you keep stays main, and every computer of the joining setup
  becomes a member, with its data left on its own disk. Joining Work Queue items get a new block of
  numbers and keep their ids, with `set_<origin>:#<n>` aliases. Revocations beat stale grants,
  conflicting preferences become offline-edit choices, and deletions stay deleted.
- **Unlink and wipe** (`computers.removal`). The main commits the removal without contacting the
  target. When the target comes back, it checks the main's signed instruction before it acts. It
  acknowledges only once its engine, its agents' sign-ins and its integration keys are cleared. A
  wipe runs and checks any configured backup first.
- **Reaching a computer.** A client keeps Auto, SSH or Relay for each computer; Auto tries SSH,
  then the relay. Over SSH, `kuvo-start route-to` runs Noise IK (prologue `kuvo-ssh v1`), and the
  device must be paired there. Everything else means the same over SSH and the relay.
- An older authority model (standby, ranks) is still read (`setup/takeover.ts`) until a setup
  converts.

## Data owners

Feature `data.ownership`. A conversation, bot or group chat lives on this computer if this computer
has it. Otherwise it lives where this computer's own move journal sent it, and failing that, on the
one computer whose `owns` claims it ("Peers"). With none of those, the answer is `404`. Setup-wide
kinds belong to the main, and a Work Queue item belongs to its queue.

- A read says `state` (`received`, `absent`: the owner confirmed it's gone, or `neverReceived`)
  and `refresh` (`live`, `unavailable`, `failed`, `withheld`). A page reused after a failed refresh
  is `stale` and keeps its original time. After a remote refusal, cached pages stay hidden until a
  fresh read succeeds.
- Sending to a remote owner: the serving box saves the message under its id before it answers
  `202`, and the owner's receipt carries `acceptedAt`. Pending messages and receipts survive a
  restart. If nobody can know whether the owner accepted it, the client shows "interrupted" and opens
  the owner's conversation before it offers to send again.

### Offline field edits

Feature `data.offline-edits`. A client reads `revision`, then submits a new operation id, `ref`,
`baseRevision` and only the fields it changed. A `202` means the serving box saved the proposal:
poll its receipt there. A conflict answers `choice` ids. The owner's view shows the original, the
accepted and the proposed values, and a choice is settled by a new operation.

- Text (session notes, a Work Queue item's description and notes) merges against the revision the
  author edited. Separate insertions at the same place are joined in ascending UTF-16 order. A
  competing rewrite keeps the owner's wording, and offers the other as an alternative.
- An edit made from a retired base gets `refresh`. A deletion beats late edits: the receipt says
  `deleted` while Recently Deleted (30 days) has the item, and after that an id fence stays.
  Recovering the text never restores the item.

### Authority carried with deferred work

The serving box records the grant the request came with: a device's key and its exact pairing
generation, the computer itself for the local app or shell, or the verified computer for legacy
signed SSH. A request body can't choose its grant, and pairing the same key again doesn't authorize
older work. The owner checks the sender, the current ownership and the grant before any new work.
A finished outcome may be replayed after a revocation, but an operation that wasn't accepted never
runs under a revoked grant. Envelopes that carry the device go only to owners that advertise
`data.queued-device-authority`.

## Peers

- **Lists.** Each box mails `setupLists` with its own rows only. A receiver takes only a complete,
  newer snapshot from a current peer, and stamps each row with its computer, reach and actions.
  Live reads of another computer's conversation (`sessionFollow`) are leased, for reads only.
- **Agents mail** (`agents`: `at`, `list`, `features`, `setup`, `computer`, `owns`, `moved`, `zone`)
  goes out on each change and every ten minutes. It carries the box's bots and its 40 latest
  sessions. `owns` holds 6-character hash prefixes of up to 5,000 record ids: that's how any
  computer finds an older conversation. A box that just connected adds `ask: "agents"`, and its
  peers answer at once (each at most once per 10 s). Only the first 300 feature names are kept.
  Past that, the list counts as unknown, and a client must not call the box older.
- **Moved.** `agents.moved` lists what moved away in the last two days, each with the box it lives
  on now; the newest word wins. After the relay says `unreachable`, a peer counts as not answering
  until any mail comes from it.

### Letters

Letters go from agent to agent: `{letter: {id, from, to, text, wait, at, answers?, read?, note?}}`,
sent to the box that lists `to`.

- **Receiver.** It drops a letter over a day old, and delivers each `id` once (at most 20 in ten
  minutes from one box), never to a subagent, a seat or an archived conversation. It answers
  `delivered {id, ok, why?, retry?, answer?, lives?}`: `retry` is "not now" (engine starting, bridge
  down, a move under way), `lives` where the conversation lives now.
- **Sender.** It takes `delivered` only from the box it sent to, and resends each minute for two
  days of online time. A letter that finally fails is told to its sender once, never sent again.
- `wait`: the receiver watches for ten minutes and sends the answer back as a letter with `answers`.
- **Notes** (`note`) are the box's own word for a conversation that now lives elsewhere. A note is
  resent each minute for 30 days, and its id is remembered as long, so it's never delivered twice.

### Calls, starts and rows

- **Calls** (`work.elsewhere`). A call runs `list_sessions`, `stop_session`, `compact_session` or
  `answer_ask` on the computer where the sessions are. The lead's box fills in `as` from its own
  records, never from the agent's words. Each call id runs once, and the answer comes back as a
  letter. An allow goes no further than both computers' rules would go without asking, and never
  for a session in Ask First.
- **Starts** (`work.elsewhere.start`) are taken only while that computer allows work from your
  other computers, capped by the lead's approval. A repeat gets the same answer, and a start cut
  short finishes from where it was. Nothing is queued for a box that isn't answering: with no answer
  (45 s for a start, 20 s for a read) the tool says it couldn't confirm, so nothing starts late or
  twice.
- **Rows** push sessions whose lead lives on the receiver; an empty `leads` clears them.
- **Group chat seats** (`groups.seats`). A chat runs on its host box. A member bot whose home is
  another box takes its turn in a seat there, and only words travel. The home takes each turn id
  once. The host asks again every 15 s, and stops the turn after 45 s of silence or past the turn
  limit.

## Moves

A client asks the source once and can then go away; the boxes finish the move by peer mail. One
thing moves at a time.

1. `check`: the destination says whether it can run the item. Nothing is held yet.
2. The source holds the item, lets it settle (a minute at most) and packs it (1 GB at most).
3. `offer` and `ready` agree the bundle's `generation` and SHA-256 `hash`, which every later step
   names, so a stale offer never replaces the current bundle. `part`s of 30,000 bytes follow, 16 in
   flight, each acknowledged, until the destination answers `arrived` or `failed`.
4. The source checks again, and starts over if anything new arrived (three tries, then `409`).
5. `switch`: the destination records the ownership durably, starts the item up and answers
   `switched`, keeping one receipt per move id for good. The source retires its original.

Until the switch is asked for, a failure goes back to the original. After that, the source resumes
the original only on `dropped {neverSwitched: true}`. While an item moves, prompts and queue changes
get `409`, letters get `retry`, and wake-ups are held. The source names the new home for two days
(`moved`). What the move left on the source is removed a week after the item first runs at the
destination, and only on the destination's word. A client drops a moved conversation's row from the
source only once the item is listed where it lives now.

## Conversations

### The queue

Every box carries its sessions' queues itself, the engine's and the bridge's alike, the same for
every client, and with no client connected.

- When a run ends well, the next waiting message goes 1.5 s later, unless the queue is paused. A
  Stop, a failed run, or a queue found idle after an engine restart pauses it. Only Resume, Send
  Now or a message of yours sent as `steer` unpauses it; a letter, a wake-up or a note doesn't.
- **A restart isn't a Stop.** A run cut short by a restart ends `interrupted` with
  `reason: "restart"`, and neither holds nor pauses the queue.
- Order: `held` (in its order), then the engine's next step, then its queue, then what waits on the
  box. While the queue is paused, what the box sends on its own waits on the box.
- **Your message while a command runs** (a `user` message without a box note) moves what the agent
  waits on, a command or a foreground subagent, to the background after 3 s, so it reads you now.
  Other agents' letters, notes and wake-ups never do.

### A conversation's asks

The box words every card. A client never makes the first answer of the box's own asks the default.
Permission and form replies go over a live connection to the current owner, never queued in peer
mail. The box where the work runs enforces Full Access confirmation, starts from other computers,
Permissions, sandbox paths and vault consent. OS access and vault unlock are granted per computer,
never by a pairing or a setup save. A browsing ask listed by two boxes has one id: the first answer
counts.

### Words are the box's

Every word a client shows comes from the box, drawn unchanged: a step's `title` and `doing` and its
failure `line`; a computer's `title`, `detail` and `notice`; account and service titles; a model's
`refusal` and `action`; approval options' `title` and `detail`; update lines; a summary's `title` and `workingTitle`. A client acts on the
values (`reach`, `working`, `state`), never on the words. Every client groups tool steps the same
way: the steps between two blocks of agent words form one group, a group of two or more folds to a
counted line, and a failed step still shows. A write that sends `was` gets `409` if the session
changed since it was read.

### Session lists

- A session's `kind` is the first of these that matches: `subagent` (it has a parent), `seat`,
  `hidden`, `bot`, `chat`, else `project` (by its worktree, then `kuvo.filed`, then its folder). A
  client shows no kind it doesn't know.
- `since=<at>` resumes a list. A cursor the box can't resume from gets `full: true`. `pending`
  names places not read yet, and their sessions are never reported gone.
- A client draws project sessions under their project, chats under Chats, and subagents under their
  parent. It never sorts, slices or counts a project's list itself (`shown`, `total`, `more`).
  Vectors: `vectors/session-lists.json`.
- A session marked `kuvo.branchedFrom`, `kuvo.handoffFrom` or `kuvo.startedBy` shows under that
  conversation when both are in the same list, newest first. A subagent is never grouped this way.
  A client with a conversation open follows `kuvo.carriedTo` when it's archived: the new
  conversation takes its place, draft and all.
- **Read state is the box's.** A session is unread while `time.idle` is later than `time.viewed`.
  `POST …/view {idle}` keeps the later value and tells every client `session.viewed`. A session
  that another conversation started and leads is read for you as each turn ends, unless the turn
  has news for you or asks you something. `POST kuvo/sessions/read` marks one box's sessions read;
  asks still wait.

### What waits on you

One rule, the box's (`asked`): a permission or question from the session or its subagents, a command
the sandbox stopped, a vault ask, a browsing ask, or a work ask. `needsYou`, a
project's `asking`, a bot's `now.asking` and `kuvo/waiting` all come from it. Each box counts its
own. A client adds them up across computers, but never adds the three areas together. A session
that waits on you is never counted as at work.

- A bot's `state` is the first of these that holds: `needsYou`; `blocked` (stopped on an error or
  a limit and not seen since); `thinking`; `working`; `done` (a turn ended that you haven't seen);
  `asleep`; `idle`. Vectors: `vectors/bot-states.json`.
- A bot shows offline while its computer's `reach` is `away`: that's the client's to show. Seen
  (`POST kuvo/bots/<id>/seen`) clears `done` and `blocked` on every client.

### Models

- A model `runs`, or has `missing`: `model` (pick one), `signIn`, `key`, `install`, or `gone` (its
  account is signed in but the model isn't listed any more). `gone` is said only when the list is
  known to be complete. A model the box can't speak for yet is left out, and a client neither
  refuses it nor asks for a pick.
- `starting` is the model named, if it runs and isn't retiring; else the engine's default; else the
  first that runs. `known` says whether each side could answer. `listing` names accounts still
  listing their models, whose models a client doesn't refuse. The same rule decides `list_models`
  and starts from other computers.
- Starred models (`models.starred`) are setup-wide: the main keeps them, and a member passes a change
  on to it (`503`, or `409`, with a message while the main can't save it).
- A model's `speeds` and its provider's picked `speed` (`models.speed`; Codex's Fast today) are each
  computer's own. A picker shows Standard | Fast only for a model with `speeds`. With none picked,
  the box asks for nothing, and the agent's own settings decide.

### What a typed message means

Kuvo has no typed keywords or commands: what's written goes to the agent as written, "/" and "!"
included. A client sends a draft through the conversation's box with `POST kuvo/sessions/<id>/draft`
(`typed.draft`), once for each `id`. The box checks the model first, then sends. The answer is
`send` (it went), `restore {text, error, action?}` (put the text back and show why) or `ambiguous`
(the box can't tell whether it went).

### A message's parts

- A client sends fields, never blocks: `POST kuvo/sessions/<id>/send` (`messages.send`). The box
  writes what the agent reads, and keeps the parts in `metadata.kuvo.parts`. Clients draw from the
  parts, and never read or write blocks.
- What the agent reads, in order: the words, then blocks for the answered widget, the agents named with `&` (`<named-agents>`, from everyone the box lists then),
  the review comments, and the files note. A `&` name the box doesn't list is refused (`409`), never
  dropped. For a group chat, a client sends only the words, and the chat's own computer writes the
  block. Vectors: `vectors/message-parts.json`, `vectors/routes/names/agent-naming.json`.
- Older messages have no parts: `POST kuvo/messages/parts` reads them, once each, before the page
  shows.
- In `words`, each quoted line is `> line`, then a blank line, then the answer; pictures are
  `[Image N]`, counted from 1 in attach order. Files go to the client's own computer's box as
  `file://`, to another's as bytes. One that can't be read, or is too big (over 50 MB, or 100 MB in
  all), stops the send; it's never quietly left out.
- **What the box writes in.** The box's own messages to an agent (its sessions' news, a late answer of yours) are user messages marked `metadata.kuvo.note {from, line}`. A
  client shows the `line` as a line of what happened, never as your bubble, and leaves it out of
  Find, copy, export and history. Sessions' news (`metadata.kuvo.news`) and wake-up cards
  (`metadata.kuvo.wake`) are drawn as given. Engine and bridge notes (`synthetic`) show nothing.
- **Subagents.** A subagent's session is a session: read it, `steer` it, `…/interrupt` it. Never
  interrupt one to change its model. A coding agent's subagent says how it takes messages in
  `metadata.kuvo.messages.via`: `direct`, `parent` (relayed through its parent), or `none`.

## Work

Bots lead sessions; this is how the box keeps that work.

- A bot's session and everything under it carry `kuvo: {startedBy, by: {bot, name}, depth}`, and a
  client marks rows by `kuvo.by`. A session's finish is told to its starter once, however it's heard.
- **Homes.** The box remembers where each departed conversation lives now, by the newest word: its
  own moves, peers' `moved`, a letter's `lives`. A conversation the box still has always beats a
  home.
- **Answering asks.** A bot allows only what its own conversation could do without asking. In the
  sandbox, it allows a command only if the session's own command would run sandboxed too. **A
  session in Ask First always asks you**, whatever the bot's mode, on every computer. Only you can
  let a command out of the sandbox.
- **Limits you set** (`atOnce`, `perDay`, `depth`, `tokensPerDay`). Each start is noted before the
  check, so starts at the same moment count each other.
- **Background work** is one fact, worked out on the box. `[Session finished]` waits while anything
  the session left is still running, but services that only listen don't hold it.
  The process list and output read the same shell on its box. The bridge validates the session
  when resolving a shell by ID after it leaves the active list. Claude Code supplies a live SDK
  tail and a final output file. Codex reports liveness through its terminal list and output
  through command events. The terminal list supplies neither an exit code nor past output.
  Grok reports background tasks through ACP. Antigravity tracks background timers as subagent
  work. Cursor reports subagents within a turn. The bridge's final `session.idle` clears the
  session's turn from Activity. Any remaining background work counts separately.

## The coding agents' state

Every route that needs the coding agents' bridge answers from one state the box works out
(`link/areas/agents-down.ts`):

- **Restarting** (a planned handover, a fresh start, a quick crash). A request waits up to half a
  minute for the bridge, and only then gets `503 {reason: "agentsRestarting"}`. A client keeps
  what it last read, asks again, and never shows this as an error.
- **Down.** `503 {reason: "agentsDown", message, action?}`. The `action` is Try Again
  (`POST kuvo/agents/start`), which repairs files from the release and starts the bridge again. A
  client shows `message` as it is.
- Peers hear the state as `computer.agents: {state, why?}`.

## Updates

- **Releases.** A box takes only releases signed by a key pinned in `kuvo-link.js` (`UPDATE_KEYS`).
  `boxes.json` is `{manifest, signature, key}`: an Ed25519 signature over `"kuvo-box v1\n"`
  followed by the manifest's bytes. The pack's `box.json` lists each file's SHA-256, plus `engines`
  and `downgradeSafe`. A box holds the newest signed release and passes it on to its peers
  (`release` mail, in 30,000-byte parts), checked against the signed hash.
- **Prepared everywhere, switched together** (`update.fleet`). The main picks the target. Each
  computer prepares it beside the running release, checks the staged pieces, and reports `staged`
  or `cannot`. The main switches once every reachable computer is ready, or five minutes after the
  first one is; away computers catch up when they're back. The switch is one rename. If the new
  release doesn't start and answer within two minutes, the old one comes back. Running turns and
  background work are let finish first. Staging, or being behind, never blocks ordinary work. Rows
  carry the box's `line` and `phase` (`getting-ready`, `ready`, `switching`, `done`, `off`), which
  only move forward.
- **Never** in another folder than `~/.kuvo-code`. Never while the engine is stopped on purpose, or
  the version is unknown. Never to an older version, or to the same version unless the files
  differ. Never over an update that's still on trial.
- **Repair** restores missing or changed files from the signed release the box runs, by SHA-256,
  and never changes the version.
- **The health judge.** An update on trial is confirmed after five minutes of `kuvo/info` answering
  with every feature it had, every server answering, and nothing restarting more than three times.
  A piece failing three looks rolls the code back (never the data), and that version rests a day.
- **The bridge while it works.** The keeper starts a compatible new bridge as a standby. If every
  active turn can transfer, the old bridge hands them over (`run/agents/handover.json`), and the new
  one replays them into the same sessions. Turns that can't transfer wait for a quiet moment. A
  restart never cuts a turn.

### A new engine

Two engines never run on one box's data, and a turn can't move between engines, so a newer engine
takes over between turns. The release check makes sure the previous engine can open what the new one
wrote (`downgradeSafe`). The swap waits for a quiet half minute. Failing that, it holds new turns
during the night window or after a day, and stops holding after half an hour. While it holds, sends
to the engine wait on the box and are never dropped. A new engine that fails goes back where that's
safe. For a versioned release with a different engine, new engine turns wait while existing turns
and background commands finish, for at most five minutes. If not quiet, the waiting work goes to
the current engine and the release tries again no sooner than a minute later. Once switched, the
saved inbox and waiting work go in order after the keeper confirms the release or restores the
previous one. A compatible engine stays running across the release switch.

## Notifications

- Each box tells only its own news, once, and words every notice itself. No client words a box's
  news.
- What counts as news (`link/notices/news.ts`): a reply to you; any question, permission or
  failure; and what an agent sends on purpose (`notify_user`). Wake-ups, watches and
  replies to another agent never count, though they still leave the conversation unread.
- Words: one short sentence, never a command line, a path or code. Bodies are cut at a word to 100
  characters; titles and box-worded lines to 60.
- Categories: asks are `permission`, `question`, `ask` and `blocked`; news is `finished`, `failed`,
  `report`, `message`, `update` and `usage`. A category a client doesn't know is shown plainly.
- `kuvo/notices` lists the last ten minutes, oldest first. A client shows each notice once, by
  `id`, and shows none of those on its first read. `telling: false` means that box sends no news
  until its bridge runs.
- (Unverified) `kuvo/attention` lets a client hold the box's phone pushes back. No current client
  sends it.

## Pairing

A box shows a pairing link, as a QR code or as text:

```
kuvo-link://pair?v=1&relay=<relay URL>&host=<address>&key=<static key>&token=<16 bytes>&name=<name>
```

A device opens a session with `pair` set to the token. A token works once, for ten minutes. The
setup's main keeps the device grants (each a key and a pairing id). Every member forwards pairings
and removals to it by peer mail, and the main checks the asker again before it saves.

- Kuvo Code's desktop app pairs with no link: it asks the box on its own computer
  (`POST kuvo/devices/pair`, `kind: "desktop"`), which gets the grant from the main. The app's keys
  are its own, never given to a box; it learns each computer's `key`, `address` and `relay` from
  `kuvo/computers`. The same key gets the same pairing; a revoked key needs a new invitation.
- The routes keep the wire name `phones`; a grant's `kind` is `desktop` (or `phone`). A removal
  names its pairing, so an older pairing id can't revoke a newer one. A committed removal closes the
  device's sessions, and reaches away members with the setup's membership.
- `kuvo-link.js uninstall` first has the main revoke this computer's desktop key, and reports the
  uninstall unfinished until that revocation and the cleanup are confirmed.

## Windows box primitives (#699)

The plan is in [windows.md](../docs/architecture/windows.md).

- **WSL.** A WSL 2 box is a Linux box (Linux paths, process records, Unix sockets), which knows
  it's in WSL by the kernel release (`microsoft-standard`, `WSL2`). `kuvo/info`, `kuvo/computers`
  and peer announcements may carry `wsl: {version: 2, distro?, host?}` (`host`: the Windows
  hostname). Over Windows SSH, Add picks a WSL 2 distro (Ubuntu first) and installs there.
  `sshDistro` in `kuvo/computers` means: reach the box with
  `wsl.exe -d <distro> --cd ~ --exec /bin/sh .kuvo-code/bin/kuvo-start route-to` (or `route`); a
  distro name is ASCII letters, digits, `_`, `.` and `-`, starting with a letter or digit, at most
  80 characters. A hidden Windows task holds the Linux keeper at sign-in. Changes polls repositories
  on Windows drives, and ignores mode-bit differences there.
- **Paths** on native Windows are sent with forward slashes, drive or UNC share included
  (`C:/Users/Ada/P`, `//Server/Share/P`). A client sends back the same spelling, case intact. The
  box refuses relative, current-drive, device and extended paths, streams and dot segments.
  Vectors: `vectors/file-addresses/windows.json`.
- **Profile and processes.** A native box lives in `%USERPROFILE%\.kuvo-code`, once the profile's
  owner SID and ACL are checked. Process records pair `pid` with `startIdentity`, the process's
  creation FILETIME as a decimal string. A missing or mismatched identity gives no right to attach
  to the process, signal it or remove its record.
- **Private IPC.** Named pipes `\\.\pipe\kuvo-<hex>`, where hex is the HMAC-SHA256, keyed with
  `run/socket.key`, of `kuvo-local-ipc-v1`, NUL, the user SID, NUL, and `ssh` or `holder:<id>`.
  Pipes are made with a user-only DACL and `PIPE_REJECT_REMOTE_CLIENTS`. The client checks the
  server's SID before it sends a key. Framing and keys are as on Unix.
- The sandbox and Auto aren't on Windows yet: both say `reason: "windows"`, in the box's words.

## This computer's box

A box keeps itself running with no client. Its keeper (`bin/kuvo-start keep`) starts the engine,
the coding agents' bridge and the link again when they end, unless they were stopped on purpose
(`run/stopped`). On a Mac, Kuvo Helper (`mac/helper`, which holds no box logic) runs
`serve --resident`, keeps the box's keys in the Keychain, and shows Kuvo in the menu bar
(`kuvo/box/at-work`) for as long as it runs the box. The desktop app reads `run/serve.json` and talks
to the box on loopback ([one client process](../docs/architecture/one-client-process.md)).

- `box/kind` says `mac` or `machine`; anything else counts as a machine.
- A box that an earlier Kuvo Code installed in the app's folder moves itself into `~/.kuvo-code`
  (`box.rehome`, `kuvo-link.js rehome`). The box decides and does each step, journalled so the move
  can be cut short and resumed. The app only shows it.
- `kuvo-install status|new|move|update`, which a release carries, is how the app learns what a
  computer holds and puts a box there. It decides nothing itself.
- A server that hangs (it doesn't answer, sleeps and uses under 2% of a CPU, at every look for ten
  minutes) is stopped and started again by whatever keeps it, at most three times in six hours.
- The engine writes a session's metadata whole. So every writer reads it, changes only its own keys
  and writes it back, taking its turn by a lock folder per session (`run/notes`).

## Per area

What a client must know that the routes don't say.

- **Projects.** Each box keeps its own projects, and every client reads and changes them there. A
  client files a session whose folder is listed in `worktrees` under that project, never as a project
  of its own. A `removed` project shows again only when one of its sessions comes back.
- **Chats.** A chat is a conversation with no project. A session's `kuvo.chat` mark, naming its own
  folder, makes it a chat wherever it's read. A mark naming another folder is no mark.
- **Settings.** Each computer keeps its own engine settings and keys, and nothing copies them. A
  box describes its own settings pages (`kuvo/settings`): its sections, rows by kind, and every word
  and icon by name. A client shows them under that computer, never one computer's under another's
  name. Changes are checked against the revision read, and go through the box's own routes.
- **Your time zone.** One pick for all your computers, carried in the agents mail. A pick is counted
  past what the box had, and is at least its time in ms. The higher count wins, then the name that
  sorts later, so every box keeps the same one. Until you pick one, each box goes by its own clock.
- **Personalities.** One library, met box to box. For each personality, a removal wins over any
  change and stays noted; otherwise the change counted highest wins, then the words that sort last
  (`vectors/personalities.json`).
- **Approval.** Kept on the box with the conversation: `askFirst`, `acceptEdits`, `auto` (what runs
  inside the sandbox goes ahead), or `fullAccess` (only once confirmed on that computer). It fails
  closed. Auto asks first while the sandbox isn't running, and Full Access comes down to Auto while
  it isn't confirmed. It stays set to what you picked, and comes back to that once it can. In Auto, a
  reviewer on the box judges what's beyond the sandbox's walls.
- **Autopilot** sits beside approval and is never part of it. Approval says what may run without
  asking; Autopilot says what may be decided without asking. It's off (Interactive) until you turn
  it on.
- **Sandbox.** Each box runs its agents' commands in a sandbox wherever it can, with no switch. The
  way out is one command at a time (`sandbox_ask`): the box itself runs an allowed command outside
  the sandbox. Also Allowed rules belong to a project, at most 8 each; a bot's own folder and the
  chats count as projects.
- **Working outside the project** is Ask by default on every box. Your settings' rules still win.
- **Git.** A repository's own settings never start a program when the box runs git there. Every
  client asks the box a folder is on, and no client runs git for it. A discard sends the `stamp` from
  `kuvo/git/files`, and is refused (`409`) if the file changed since. New files go to the Trash.
- **Worktrees.** A worktree goes when its last session is archived or deleted and no agent works
  there. Uncommitted work is saved first, and the worktree is listed as `restorable`.
- **Bots on another computer** (`bots.held`). Any box takes a change to a bot that lives elsewhere
  (`202 {held}`), and delivers it by id, once. Each field carries its age, so no box reads another's
  clock, and a field changed there since is never overwritten by an older one.
- **Vault.** Each box keeps its own vault, and nothing is synced. No answer an agent can get holds a
  value: the box fills an item into a page, a command's environment or a temporary file. The vault's
  files are signed and counted by generation; one that goes back pauses agents' use.
- **Artifacts and the Library.** The coding agents' bridge on each box keeps what agents made and
  what was handed to you. The box passes those routes on as they are.
- **Signing in.** An account's state, line, actions and words are the box's. A client groups by
  `state` (an unknown one is `signedOut`), shows only `listed` accounts and unknown actions
  disabled, and never offers a way in of its own; a browser sign-in starts with `kuvo/sign-in/start`.
- **Plan limits.** Rows with the same `service` and `identity` (a hash, never the account's id) are
  one row from any box: the latest reading wins for each window, a `problem` shows only when every
  box has one, and a peer unheard for 25 minutes is `away`. One computer reads each account; another
  takes over once its next read is two minutes overdue. The box supplies `level` (`calm`, `near`,
  `over`) and the words; a client sets no threshold. A reset is spent once, journalled first under
  the maker's request id.
- **Usage.** A client takes a service's totals from one box, never adding boxes together. The box
  that read a limit decides its notice. Usage settings are shared in one setup, each part changed
  whole, the newest (`{v, by}`) winning.
- **Insights.** The box makes the whole screen (`…/insights/summary`); a client draws it as it
  comes. An answer with `asOf` is the last one kept for that query: ask again shortly. Peers sync
  usage records (`insights` mail), and each computer prices them with its own catalog.
- **The Work Queue.** Each item lives once, in its owner's queue (a session, bot, group chat or
  `usr_you`) on its owner's box; every box shows every other's. The statuses are fixed (open:
  `needs-you`, `in-review`, `in-progress`, `waiting`, `todo`, `backlog`; closed: `done`,
  `canceled`), set by you or an agent, never worked out from activity. The box decides what may be
  done (`can`, `why`), counts, filters and searches. Each item has one `#n` in its setup, from blocks
  the main grants. In a message, `#243` is an item when it starts a word outside code and links, not
  after "PR", "issue" or "step"; `gh:owner/repo#12` is GitHub's, read and never written. A hand-off
  (`hand`) leaves the item in place until the receiver takes it, and an edit (`act`) runs where the
  item is kept; both once per id.
- **Marketplace.** Every write happens in the box. The setup's main accepts app definitions and
  reviewed skill bytes, and publishes them in signed setup state, so an offline computer shows the
  last accepted copy and catches up later. Credentials stay in each computer's vault. A refresh never
  changes what you installed. Retries of an agent's add carry the same `request`.

## The browser

**A machine's browser** is Chrome for Testing, pinned by `kuvo-link.js`. It's fetched only from
Google's bucket, over HTTPS with no redirects. Its size and SHA-256 are checked before it's unpacked.
A newer pin comes only with an update, and sign-ins (`browser/profile`) survive every new build.

**The live view** (`browser.view`). `GET kuvo/browser/view` is a socket: a WebSocket on `serve`
with Basic auth locally, or a WebSocket request in a paired client's Noise session through the
relay (peers can't open it). A plain GET gets `426`. Opening the view starts the browser. Wait for
`Kuvo.hello` and the page facts. If the browser fails to start, the box sends `Kuvo.error` and
closes the socket.

- Text messages are newline JSON `{id?, method, params?}`. A request with an id gets `{id, result}`
  or `{id, error: {message}}`. Only the view's own methods are taken, never raw CDP.
- The box sends `Kuvo.pages`, `.viewport`, `.frame`, `.status`, `.pointer`, `.cursor`, `.dialog`,
  `.fileChooser`, `.external` and `.viewQuality`. Through the relay, a newer state for a page
  replaces one not yet sent. The client sends `Kuvo.view` (a null target stops pictures),
  `.frameAck` (only once a picture is shown), `.frameDropped`, `.go`, `.input` (CDP's input
  fields), `.upload` (50 MiB at most), `.hold`, `.claim` and `.dialogAnswer`.
- A frame is binary: 4 bytes of big-endian header length, the header JSON
  `{method: "Kuvo.frame", params}`, then the JPEG. Through the relay, a frame over 60,000 bytes is
  dropped, never split, and the next one is smaller (`Kuvo.viewQuality`). A viewer has at most two
  unacknowledged frames, at 15 fps at most.
- While an agent works in a page, a viewer can't change the page's size; otherwise the latest view
  or input sets it. A client asks you before it opens another app's URL. Vectors:
  `vectors/browser-view.json`.

## For released Macs

What a box does only for released Kuvo Code Macs, 0.5 to 0.7.2 and 0.8.0-beta.1, which still
connect to machines; the 0.7.2 → 0.8 update path depends on it. Each item is marked in the code
`COMPAT(<version> <item>)`: `scripts/compat.sh` lists the code, `protocol/compat-kinds.txt` says what
each item is kept for, and `box/bridge-tests/compat.test.js` holds the code's markers to the list
below. An item goes as its kind says: on the user's word once no such Mac connects, or once what it
takes in has been taken in.

**Kuvo Code 0.5 to 0.7.2** (the box notes such a Mac in `run/attached`):
- Its files: it puts its own engine, bridge and keeper over the box's. A newer `run/attached`
  starts a repair from the release the box runs. <!-- compat: 0.7 files -->
- Messages it writes itself (blocks and text, `…/prompt`, `queue/add` with `text`): the box still
  fills in the parts. <!-- compat: 0.7 own-envelope -->
- Its keeper: the box keeps its own copies of `kuvo-start` and `kuvo-launch`, and takes away the
  old watchdog's mark before it stops the engine. <!-- compat: 0.7 keeper -->
- Its browser's keeper compares numbers: the box's says `keeper: 51`, above all of them. <!-- compat: 0.7 keeper-number -->
- Its browser's pages kept across a stop, as addresses alone (`run/pages.json`). <!-- compat: 0.7 pages-addresses -->
- Its Stop (`run/stopped`) and its engine: an engine without `notes.box` is started again with
  the box's settings once nothing runs. <!-- compat: 0.7 engine-stop -->
- Its visits (`run/older-mac`): they never hold the fleet's updates. <!-- compat: 0.7 visits -->
- Its settings, made the box's own by `kuvo-link.js box adopt`; until then the settings routes
  answer `409`. <!-- compat: 0.7 settings -->
- It leaves a machine's engine running while something waits there. <!-- compat: 0.7 leave-running -->
- It reads a bot's job from `bot.json`, so the box writes it there too. <!-- compat: 0.7 bot-json-job -->
- It gives the box its worktree records (`POST kuvo/worktrees/record`). <!-- compat: 0.7 worktrees -->
- Its kit (`bots/kit.json`, `wakes/timezone`, `KUVO_TZ`), carried into the box's own facts. <!-- compat: 0.7 bot-kit -->
- Its agents bridge's installs and copied sign-ins in `agents/runtime`, taken away. <!-- compat: 0.7 agents-runtime -->
- Plan limits from before `rows`: each box's `accounts` keep peer readings' numbers. <!-- compat: 0.7 limits-own-numbers -->
- Its own group chats (`groups.json`), moved into the coding agents' bridge once. <!-- compat: 0.7 mac-group-chats -->
- Its Settings › Git, given to each box once (`POST kuvo/git/settings/move`). <!-- compat: 0.7 mac-git-settings -->
- The branch prefix a box kept in `box/git.json`, taken into its settings once. <!-- compat: 0.7 box-branch-prefix -->
- `kuvo-keys`: the Keychain items it made for this Mac's box, handed to the box's helper as the
  box moves into its own home. <!-- compat: 0.7 kuvo-keys -->
- The whole move of its sessions, settings and saved sign-ins into this Mac's box (`kuvo-link.js
  rehome`). <!-- compat: 0.7 move -->
- A box still in the retired app's folder, whose app ran the coding agents' bridge and wrote why it
  gave up (`run/agents-stopped`). <!-- compat: 0.7 app-agents-stopped -->
- Account discovery at first launch through `kuvo/accounts/found` and `kuvo/accounts/found/use`. <!-- compat: 0.7 first-launch -->

**Kuvo Code 0.8.0-beta.1:**
- Its letters and the messages it kept for an away machine, handed to the box's outbox. <!-- compat: beta.1 mac-letters -->
- Letters left in a machine's `run/outbox`, sent by the link. <!-- compat: beta.1 outbox -->
- `run/mac-agents.json`, removed. <!-- compat: beta.1 mac-agents -->
- It signs no documents: its peers list (`peers`, `legacy` in link.json) counts as its peers
  document, never passed on. <!-- compat: beta.1 peers-list -->
- `run/connected` and `run/present`, cleared as the bridge starts. <!-- compat: beta.1 connected -->
- It lets go of deleted bots by `bots/deleted.json`. <!-- compat: beta.1 deleted-bots -->
- It changes a machine's settings with `kuvo-link.js box read`, `box change` and `box key`. <!-- compat: beta.1 box-read-change -->
- It runs moves itself, over SSH: its `moved` is taken under the box's own word. <!-- compat: beta.1 mac-moves -->

**A box from before beta.31** takes this release once:
- During a trial, the keeper ends a server started before the new `kuvo-link.js` was written. <!-- compat: beta.30 old-code -->
- `kuvo-start rollback` moves a rollback's data aside before running its `restore.sh`. <!-- compat: beta.30 rollback-data -->
- A Kuvo Helper from before beta.31 is handed over once by its `serve`. <!-- compat: beta.30 helper-handover -->

**Layout 4 setups:** older standby proposals carry the full projection, leadership certificates may
carry devices beside the roster, and a standby can be chosen before setup checkpoints exist. <!-- compat: layout.4 setup -->
