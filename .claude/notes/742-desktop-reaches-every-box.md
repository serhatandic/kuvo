# #742 The desktop app reaches every box: design

Written 2026-11-15, from reading the code. Nothing here is built, and nothing was run. Where I
couldn't confirm something from the code, it says "not verified" and names the piece that checks it.

The user's model, which this follows: "There is no such thing as apps own box. All boxes are the
same. One is selected as the primary to handle stuff such as pairing." And: "the app connects to
each box directly."

## 0. In short

- The desktop app becomes a **paired client device**, exactly as a phone is: a static key and a
  relay secret of its own, kept in the system's keychain. It opens one encrypted session per box,
  through the relay, with the protocol the phone uses (`protocol/PROTOCOL.md`, "Sessions",
  "Pairing"). No box hands it another box's keys or password.
- **The box on the same computer is only the nearest one.** It is reached over its local server
  (as today) because that's faster and works offline. It's one row in the same list of boxes,
  with the same `BoxClient` on top.
- **It pairs once, with the setup's main computer**, and every box of the setup lets it in from
  the main computer's signed `phones` document. That's how a phone already reaches a Mac's
  machines. On the main computer the app pairs itself with no user step; on a member computer the
  user brings a pairing link from the main computer once.
- **No new routes.** Three small box changes are needed (§4.6): the client says what kind of
  device it is, so a desktop isn't treated as an iPhone; a pairing slot of its own; and
  `kuvo/computers` says where each of your computers is reached.
- There is **no LAN path today**, on the phone either: everything goes through the relay. The
  brief asked about "relay and direct (LAN) paths"; I found only the relay (§1.3). A direct path
  is a later piece, and doesn't change this design.

## 1. How the phone does it today

References: `K/` is `swift/KuvoLink/Sources/KuvoLink/`, `C/` is `swift/KuvoClient/Sources/`,
`P/` is `~/Projects/kuvo-code-ios/` (read only).

### 1.1 Identity and keys

- One static X25519 key and one relay secret for the phone, made once and kept in the Keychain
  (`P/KuvoCodeMobile/Stores/LinkStore.swift:610-611`, `kept` at `:626`). The same key for every
  computer.
- A relay secret of its own per relay, derived by HMAC from the one secret, so two relays can't
  tell it's the same phone (`P/Shared/LinkIdentity.swift:19`).
- Per computer it keeps only public facts: `PairedHost {address, key, name, kind}`
  (`K/Link.swift:63`), plus the relay URL and which Mac told it (`LinkStore.Host`,
  `LinkStore.swift:15-29`), in `hosts.json` (`:1945`). No per-computer secret exists.

### 1.2 Pairing

- A host shows `kuvo-link://pair?v=1&relay&host&key&token&name` (PROTOCOL.md "Pairing";
  `K/Frames.swift:234`). The phone opens a Noise IK session whose first payload carries
  `pair: <token>` (`K/Link.swift:243`, `:404-411`).
- The box checks the token once, within 10 minutes, and keeps the phone's key with a new
  `pairing` id (`box/src/link/phones.ts:24-52`). A member of a setup pairs no one: only the main
  computer does (`phones.ts:41`, `box/src/link/areas/phones.ts:70-71`, PROTOCOL.md "Only the main
  computer adds and pairs").
- A phone makes a link for another phone with `POST kuvo/phones/pair`
  (`areas/phones.ts:52-78`).

### 1.3 The relay; no direct path

- One WebSocket per relay, shared by every host on it (`LinkClient`, one per relay URL:
  `LinkStore.swift:1894`; `K/Link.swift:165`, `relayConnection` at `:785`). It pings every 25 s
  and gives up on a pong late by 10 s (`K/Relay.swift:129-130`).
- I searched KuvoLink, KuvoClient, the phone app and `box/src/link` for Bonjour, `NWBrowser` and
  local-network code: there is none. A phone on the same Wi-Fi still goes through the relay.

### 1.4 How it knows your computers

- It pairs with a Mac, then asks that Mac `kuvo/hosts` for its machines: each
  `{name, address, key, relay, machine, anotherMac, removed}` (`LinkStore.swift:690-767`). Each
  becomes a host of its own, reached directly with the phone's same key.
- A machine lets the phone in because its Mac's signed `phones` document lists it
  (`phones.ts:121-158`, `foldPhones`; PROTOCOL.md "Paired phones"). A removal in any phones
  document is kept everywhere.
- `kuvo/hosts` lists machines only. `kuvo/computers` (#711) is the newer list of every computer,
  from any box, but it carries no address or relay today (PROTOCOL.md "Computers").

### 1.5 One live session per box

- One Noise session per host address (`K/Link.swift:185`, `open` at `:375`); requests, followed
  routes and sockets are multiplexed on it by id (`Session`, `:199-225`).
- One `HostModel` per computer (`LinkStore.swift:1166`), and per-computer follows of each
  `…/events` route (`follow`, `:1425-1454`; `lines`, `:1458`).
- The engine's API goes through the same session (`LinkURLProtocol`, `LinkStore.swift:1872`).

### 1.6 Reconnects and a computer that's away

- The relay tells the phone when a host's connection was replaced or left (`gone`), and when
  nothing is at an address (`unreachable`) (`K/Link.swift:817-833`). A handshake caught by a
  restart starts over (`:417`).
- A request with no answer for 5 s is checked with a ping; the check may take 15 s
  (`K/Link.swift:15`, `:20`, `:694-750`).
- Requests safe to send again go again for up to 120 s, waiting 0, 1, 2, 4, 8 s
  (`K/Link.swift:29-33`, `:463-600`).
- A followed route that drops is opened again at once the first time, then after 2, 4, … up to
  30 s (`LinkStore.swift:1445`).
- "Isn't connected" is said only by one shared rule (`C/ClientSessions/Reach.swift:18-45`): the
  link down for 10 s, then a `kuvo/info` check with no answer in 30 s
  (`P/…/HostModel.swift:1874-1896`). Reads that failed only because the link went are read again
  quietly (`Reach.swift:65-78`).
- Coming back to the foreground, the phone lets go of a relay connection that may have died
  (`LinkStore.swift:399-402`, `K/Link.swift:340-357`).

### 1.7 The update gate, per box

- `BoxGate` (`C/ClientSessions/BoxGate.swift:10-46`): a box whose `kuvo/info` features lack a
  name in `protocol/gate-required.txt` is held.
- The phone keeps a set of held computers (`LinkStore.swift:421-463`). A request to a held one is
  answered `503`, `reason: "updating"`, without being sent, unless `BoxGate.isAllowed`
  (`:1059`, `:1461`). Each `kuvo/info` read sets it again (`noteGate`, `:442`).
- A phone can't update a box, so it says who can (`BoxGate.Words.waiting`, `BoxGate.swift:67`).

## 2. How the desktop app connects today

- `gpui/kuvo_box/src/transport.rs:88-116`: one `Transport` trait (`send`, `open`, `refresh`,
  `streams`, `here`, `describe`), blocking, with no async runtime.
- Two transports: `LocalServer` (`local.rs:42`), plain HTTP to `127.0.0.1` with the password
  from the box's `run/serve.json`; and `SshBox` (`ssh.rs`), one `box route` call per request,
  which doesn't stream.
- `BoxClient` (`client.rs:22-26`) wraps one transport and one gate. The gate is already per
  client (`client.rs:125-174`), and matches `BoxGate` (`gate.rs`).
- `kuvo_box` has no WebSocket, no crypto and no keychain code (`Cargo.toml`).

### 2.1 Every place that assumes one box

- `gpui/kuvo/src/main.rs:20-25`: the app makes exactly one client, `BoxClient::this_mac`. With
  no box here it shows `Missing` (`:98-115`) and nothing else.
- `workspace.rs:125`: `client: BoxClient`, the one box, handed to everything in `new`
  (`:222-262`): the landing composer, the areas, the rail's rings, the side panel.
- `workspace.rs:136`, `:143`, `:57`, `:119`: composers, started sessions, history and what's open
  are keyed by session id alone, with no computer.
- `workspace.rs:87`: one `BoxState` for the window. `:428` `connected`, `:670`, `:681`: one name,
  one "held" message.
- `workspace.rs:339`: one `kuvo/waiting` follow feeds the rail's badges. `:1451`: one computers
  read.
- `areas.rs:124`, `:204`, `:1077`: each area view is built with the one client.
- `computers.rs:144` (`this_mac`), `:334`: "Opening another computer's sessions isn't in this app
  yet."
- `rail.rs:117`, `panel/mod.rs:108`, `settings/mod.rs:50`, `first_run.rs:85`, `bots/mod.rs:166`,
  `queue/mod.rs:198`, `groups/mod.rs:77`: one client each.
- `kuvo_box/src/client.rs:103-111`: only `this_mac` and `machine` (SSH) constructors.

What helps: every view already takes a `BoxClient` as a value, and none reaches for a global.
So a view per computer is a matter of which client it's given; the one-box holder is `Workspace`.

## 3. Credentials: does the app pair like a phone, or does a box hand it access?

**It pairs like a phone.** Checked against what the protocol allows:

- A box lets a client in only by its static key: one paired there by token, or one its Macs'
  signed `phones` documents list (`phones.ts:24-26`, `:121-158`). There is no other way in over
  the relay, and this design adds none.
- "A box hands the app access" would mean a box giving out a credential for another box. No box
  holds one: a box's server password is its own (`run/serve.json`), and peers trust each other by
  signed documents, not shared secrets. Building this would be a new way in. Rejected.
- Pairing on the main computer, with no user step, is not a weakening. The app already reads the
  nearest box's `run/serve.json` and may call `POST kuvo/phones/pair` there, as Settings › Phone
  does. Anything running as the user could already do the same. The app then uses that token
  itself, over the relay, as a phone would.
- On a member computer the nearest box refuses to pair (`409`, with the main computer's name).
  That refusal is deliberate: a member that's stolen or shared must not mint devices that reach
  all your computers. The app keeps to it: it shows the box's words and takes a pairing link made
  on the main computer (pasted, or opened as a `kuvo-link://` link).

What the app holds after pairing: its own key and secret in the keychain, and public facts per
computer. Nothing about any box that isn't already in a pairing link.

## 4. The design

### 4.1 In `kuvo_box`: a link transport

A new module `kuvo_box::link`, the Rust twin of KuvoLink's client side, held to
`protocol/vectors/` (the Noise, frames and link vectors both existing implementations run).

- `link::Identity`: the static key and relay secret, with the per-relay secret derived as
  `LinkIdentity` does. Kept by a small `Keychain` trait the app supplies (macOS Keychain; a file
  only the user can read under the app's own folder on Linux until it has a keyring; tests use
  memory). `kuvo_box` never logs or prints it, as with the box's password today.
- `link::Relay`: one WebSocket per relay URL, a reader thread and a writer, the ping rule of
  PROTOCOL.md "The relay". Blocking, to match the rest of `kuvo_box`.
- `link::LinkClient`: one per relay. One Noise IK session per box address; request ids; `gone`,
  `unreachable`, the 5 s check, and sending repeatable requests again, as §1.6.
- `link::LinkBox: Transport`: one per box. `send` and `open` become requests on the session;
  `Stream.reader` reads from a channel; `Closer` sends `cancel`. `streams()` is true, `here()` is
  false. `refresh()` counts up each time the box's session is made anew, so `BoxClient` reads
  `kuvo/info` again and the gate follows a box that updated (as `LocalServer` does for a new
  server, `local.rs:197-201`).
- Sockets (a terminal over the link) need one more `Transport` method. It comes with terminals,
  not here.
- Crates: a WebSocket client with rustls, and Noise. I'd use `snow` for
  `Noise_IK_25519_AESGCM_SHA256` rather than hand-write it, checked against the vectors; both go
  in `gpui/LICENSES-DEPS.md`. Not verified: that `snow` takes our prologue and payload sizes as
  the vectors need. Piece 1 finds out first, and falls back to the primitives (`x25519-dalek`,
  `aes-gcm`, `sha2`) if not.

### 4.2 In `kuvo_box`: your boxes as one list

A new `kuvo_box::boxes::Boxes`, with no GPUI in it, so a Windows or Linux client reuses all of it:

- It starts with the nearest box, if there is one (`place::this_mac`), over `LocalServer`.
- It follows `kuvo/computers/events` on the first box that answers, the nearest first. Each of
  your own computers there (not `apart`) with an address becomes a `LinkBox` and a `BoxClient`.
  One the list drops is closed.
- It keeps the last list on disk (public facts only), so the app opens with your computers even
  when the nearest box is off, and can reach the others with no box here at all. That makes the
  app work on a computer that runs no box: it pairs with a link and is a pure client.
- Per box it states: `nearest`, `connecting`, `connected`, `away` (by the `Reach` rule, ported
  with its numbers), `held` (its gate, with what it lacks), `not paired` (the box turned this
  device away: `refused`), and `apart` (listed, never connected to: #701).
- The nearest box is also in `kuvo/computers` (`here: true`). It's matched by key, so it's one
  row, reached locally. If its local server is down but its link is up, it isn't reached through
  the relay instead: its row says it isn't running here, with the box's own words. (Reaching your
  own computer around its stopped server would hide a real fault.)

### 4.3 Pairing, in the app

- **Nearest box is the main computer, or alone:** the app asks it for a link
  (`POST kuvo/phones/pair`, local server), pairs through the relay, and ends the link. No screen.
  If the box has no relay yet, the app stays a one-computer app and Computers says why, in the
  box's words (`409`).
- **Nearest box is a member:** the box answers `409 {message, primary}`. Computers shows it with
  one field: paste the link from the main computer. The main computer makes it where it already
  makes a phone's (Settings › Phone, which needs the link as text beside its QR code: a UI
  change there, drafts first).
- **No box here:** the same paste field, in the window that says `Missing` today.
- Removing the device is `POST kuvo/phones/remove`, from any client paired there, as for a phone.
  It reaches every computer through the phones documents.

### 4.4 The update gate, per box

Already per `BoxClient`. What's added:

- `Boxes` shows `held` per computer, with who brings it up to date. The nearest box is offered
  the app's release as today's design says (PROTOCOL.md "The update gate", "A Mac, its box
  attached in place").
- A box reached only over the link can't be updated by this app, exactly like a phone's case: it
  says so with `BoxGate.Words.waiting`'s rule (the words need a version that doesn't say "this
  iPhone's"; they live in KuvoClient and gpui's copy, and PROTOCOL.md).
- `kuvo/computers` already says each computer's `features` (`computers.facts`), so a computer can
  be shown as older before the app has connected to it.

### 4.5 What each area shows

Every one of these is a visible change. CLAUDE.md wants two or three drafts, light and dark,
before any is built; the build session draws them. What's below is what the data allows and what
I'd recommend drawing.

- **Sessions sidebar:** one list, sectioned by computer, the nearest first, each section with the
  computer's state mark and its projects inside. "Needs you" on top stays one list across all
  computers, each row naming its computer. A section for a computer that's away keeps what it
  last showed, dimmed, with "isn't connected". Each box's list comes from its own
  `kuvo/sessions/list/events`; the client only places sections.
  - Alternative draft: a computer switcher above the list, one computer at a time (closest to the
    phone's strip). Simpler to build, but hides the lead bot on the other computer, which is the
    complaint this item starts from.
  - A session `away` on one box and live on another is placed once, where it lives (PROTOCOL.md
    "Session lists": "a move between computers is the client's to place, as it sees every
    computer's list"). This is a view rule both clients share, so it goes in PROTOCOL.md with a
    vector; the phone has it in `Moves.swift`.
- **A conversation:** opened with its computer's `BoxClient`. Its title bar names the computer
  when it isn't the nearest. Keys become (computer, session) everywhere in `Workspace`.
- **New session:** the landing composer gets "where": the nearest computer by default, the
  others offered. Models, folders and modes are read from the chosen box.
- **Bots and group chats:** each box's `kuvo/bots` and `kuvo/groups`, merged into one list with
  the computer's name on each, as the phone does (`LinkStore.swift:769-825`, `:922`). A bot on a
  computer that's away stays listed, as last said.
- **Work Queue:** already merged by the boxes. Asked of the nearest box, else the first
  connected one. Nothing is merged in the client.
- **Settings:** each box describes its own settings (#727, PROTOCOL.md "A box's settings"), so
  Settings gets a computer picker at the top and shows that box's pages. Sign-ins that open a
  browser are offered only on the nearest (`action.offered(here)` already).
- **Computers:** `kuvo/computers` as now, plus this app's own state per computer, and the "At
  Work" rows open that computer's sessions (the note at `computers.rs:334` goes). `connect` there
  is still the box reaching its peer; the app's own reconnect is automatic.
- **Rail badges:** `kuvo/waiting` per box. The badge is their sum, which is a view over facts, as
  the phone's is (`LinkStore.swift:1649`).
- **Side panel:** files, changes and branches follow the open conversation's box. The browser
  stays this computer's. Terminals on another computer wait for sockets (§4.1).
- **First run:** the nearest box only, unchanged.

### 4.6 What the box needs (no new routes)

1. **The client says what it is** (`clients.kind`, one line in `FEATURES`, and in
   `gate-required.txt` once clients rely on it). The hello gains `client: "desktop"`; none means
   a phone. The box keeps it with the pairing, carries it in its phones document, lists it in
   `kuvo/phones`, and puts it on the request. Why it's unavoidable: today anything over the link
   is a phone, so a message typed in the desktop app on another computer would be wrapped in
   `<from-phone>` ("They read your reply on a small screen…": `areas/messages.ts:53`,
   `areas/queue.ts:224`, `server/forward.ts:48`). A desktop over the link gets no `fromApp` or
   `fromMac` rights: it stays a remote client. A phone that lies about its kind only loses the
   note.
2. **A pairing slot for a client's own pairing**, so the app pairing itself never ends a QR code
   Settings › Phone is showing (they share `appPairing` today, `areas/phones.ts:56`).
3. **`kuvo/computers` says where each of your own computers is reached**: `address` and `relay`,
   for a paired client or Kuvo Code on that computer, never for an `apart` one
   (`computers.reach`). The peers document already holds each box's address. Why not
   `kuvo/hosts`: it lists machines only, never another Mac, and on a Mac it's built from files
   the Swift app wrote (`mac/machines.ts:105-116`), which the desktop app doesn't write.
4. **Not verified, to check in piece 0:**
   - Whether a member that is a Mac with its own app lets in the main computer's phones. On a
     Mac's box clients come in only while "the phone link" is on (`mac/relay.ts:103`), and a Mac
     with it off signs no phones document (PROTOCOL.md "Documents").
   - Who tells a Mac's box its relay, name and that the link is wanted. Today that's the Swift
     app (`kuvo/link/config`, `mac/link.ts:27-45`); the desktop app sends none. A Mac upgraded
     from the old app keeps what it said (`run/mac-link.json`); a fresh 1.0 Mac may never join
     the relay. If so, the box must own this (a default relay, the computer's name, wanted
     whenever it has peers or paired devices): box work that belongs with #713, and this item
     depends on it.

### 4.7 Away, reconnect, backoff

- Per box, the phone's rules with the phone's numbers (§1.6), so both clients say "isn't
  connected" at the same moment. `Reach` moves into PROTOCOL.md's words already; `kuvo_box` gets
  the same constants and a test against the same cases.
- The relay connection: again at once, then 0.5, 1, 2, … up to 30 s, with jitter (Zed's client
  uses 500 ms to 30 s: `crates/client/src/client.rs:87-88`). On wake from sleep and on a network
  change, the connection is let go and made again, as the phone does coming to the foreground.
- A box that's away costs nothing: no session, no timers beyond the `Reach` check while one of
  its views is on screen, and one cheap retry when the relay says something came from it or the
  nearest box's `kuvo/computers` says it's back (`reach: connected`). That's better than polling:
  the boxes already know.
- From Zed's remote connections (`crates/remote/src/remote_client.rs:161-199`), ideas only: an
  explicit state per connection that the UI draws (connecting, connected, heartbeat missed,
  reconnecting, gave up), and that a missed heartbeat is a quiet state before it's an error. We
  already have the second in `Reach`; the first is `Boxes`' state. We don't take its "give up
  after three tries": a computer that's asleep comes back hours later, so we keep trying slowly.

### 4.8 Performance

- One WebSocket per relay (one, in practice), whatever the number of computers.
- Follows are opened for what's on screen plus what the lists need: per box, the session list,
  `kuvo/waiting` and `kuvo/computers` on one box only. Bots, groups, queue and settings are
  followed while their area is open. A conversation's stream only while it's open.
- Threads: today each follow is a thread blocked on a socket (`follow.rs`). Over the link they
  block on a channel instead, fed by the one reader thread, so ten computers don't mean ten
  times the sockets. If the thread count shows up in a profile, the follows over the link move
  onto one thread; not before.
- The nearest box never goes through the relay, so the common case is as fast as now.
- Session lists resume from `since` after a reconnect (already in `follow_sessions`).

## 5. Security

- **What the app's credentials are:** one key that every box of the setup lets in as a client.
  It's the phone's power exactly: read and drive sessions, answer asks, change settings through
  routes. It is not the app-on-this-computer's power (`fromApp`): no link config, no box keys,
  no proofs for vaults, no app registration.
- **A compromised computer running the app:** whoever runs as the user there can read the key
  from the keychain (with the user's login) and act as that device on every computer of the
  setup, until it's removed. Today the same attacker has only that computer's box, plus whatever
  its box can ask of peers. So this widens what one stolen laptop reaches: from one box to all.
  That is the price of the feature, and the same as a stolen unlocked phone.
  - Removal works from any other device: `kuvo/phones/remove`, carried everywhere by the phones
    documents, and removed stays removed.
  - The key is made on the device and never leaves it; it isn't in any backup the box makes.
  - The sandbox must keep agents' commands from it, as it keeps them from `run/serve.json`: the
    keychain item on a Mac is the app's alone; a file on Linux goes in the fence's denied list
    (`box/src/link/fence/policy.ts`). Box work, in piece 2.
- **A shared machine** (set up by Macs of different setups): `kuvo/computers` marks it and the
  other person's computers `apart`. The app connects to none of them and shows them filed apart,
  with no sessions, as #701 says. Not verified: whether a shared machine lets in each Mac's
  phones (the phone reaches such a machine today through `kuvo/hosts`). If it does, the app may
  connect to the shared machine itself, never past it; piece 0 reads `foldPhones` on that case
  and the note is corrected before anything is built.
- **#701 "Whose"** (PROTOCOL.md "Plan limits", Whose): what's yours alone (resets, Always Allow,
  spend and totals) stays between your own computers. The app adds nothing to that: it never
  sums usage across boxes itself, and reads limits from one box, which already applies the rule.
  A badge summed across boxes counts only your own computers' boxes, since those are the only
  ones it connects to.
- **A member can't widen access:** pairing stays the main computer's alone. The app pairing
  itself works only where the box would pair a phone.
- **The relay** sees addresses and sizes, as for a phone, and never content. The app gets a
  relay address of its own, so the relay can tell that a device talks to these boxes; it already
  can for phones.
- **What a lying box can do to the app:** `kuvo/computers` names where to connect. A box of your
  setup that lies could point the app at an address with a key it controls; the app would then
  talk to that key, and send it whatever the user types there. It can't make another box let the
  app in. To bound it, the app takes a new computer's key only from a box it's paired with, and
  shows a computer it has never connected to before as new, by name. Not solved here: checking
  the main computer's signed peers document in the client, which would remove the trust in the
  answering box (later piece, small once `kuvo_box` has the document format).

## 6. Pieces, in order

Sizes are session-hours for one session, with its tests and review. Box work is in `box/`; app
work is `gpui/`. The phone needs nothing to keep working; where it gains something it's named.

| # | Piece | Where | Size |
| --- | --- | --- | --- |
| 0 | Read and settle the three "not verified" points (§4.6.4, §5), on two test boxes on devbox. Correct this note. | box (reading, a test) | 2 |
| 1 | `kuvo_box::link`: identity, relay, Noise, frames, `LinkClient`, `LinkBox`, held to `protocol/vectors`. A stand-in relay and host in Rust for tests. | app (`kuvo_box`) | 9–12 |
| 2 | Box: `clients.kind` (hello, phone book, phones document, `kuvo/phones`, no `<from-phone>` for a desktop), the client's pairing slot, the fence's rule for a client's key file. PROTOCOL.md and vectors. | box | 5–6 |
| 3 | Box: `computers.reach` (`address`, `relay` in `kuvo/computers`). PROTOCOL.md. | box | 2–3 |
| 4 | `kuvo_box::boxes`: the list of boxes, pairing (self, and by link), per-box state and gate, `Reach`, reconnect and backoff, the kept list. | app (`kuvo_box`) | 7–9 |
| 5 | Drafts for the sidebar, the composer's "where", Settings' picker, Computers and pairing (light and dark), for the user to pick. | app (drafts) | 3 |
| 6 | `Workspace` over `Boxes`: keys by computer, sidebar by computer, conversations and the side panel on their box, rail badges, Computers' rows opening sessions. | app | 10–14 |
| 7 | Bots, group chats, Work Queue and Settings across computers. | app | 6–8 |
| 8 | Pairing link as text in Settings › Phone (desktop and phone), "Devices" naming for the list, docs in kuvo-web, changelog. | app, phone, docs | 3–4 |
| 9 | Later: sockets over the link (terminals elsewhere); checking the signed peers document in the client; a direct LAN path; a member asking the main computer to pair its app, approved from any device. | box and app | not sized |

About 47 to 59 session-hours for pieces 0 to 8. Pieces 2 and 3 can run beside piece 1. Piece 6
waits on the user's pick from piece 5; pieces 1 to 4 don't.

What a user sees first: after piece 6, the lead bot's sessions on the other computer, in the
sidebar, live. After piece 7, its bots and settings.

### Tests

- **Vectors** (piece 1): the Rust Noise, frames and addresses against `protocol/vectors`, as
  KuvoLink and the box do.
- **Stand-ins** (pieces 1, 4): an in-memory relay and a host in Rust beside the existing
  stand-in box (`gpui/kuvo_box/tests/stand_in.rs`). Scenarios, each on what the user sees: a box
  goes away and its section says so after the `Reach` wait, never before; it comes back and its
  list resumes; a held box sends nothing but the allowed routes; a removed device is turned away
  and says to pair again; a followed route survives the relay dropping. Time is the test's own
  clock: no sleeps.
- **A real two-box test** (pieces 2, 3, 4), in `box/bridge-tests` beside `peers.test.js` and
  `setup-two-boxes.test.js`, with `relay-stand-in.js`: a main box and a member; a desktop client
  pairs at the main box, appears in its phones document, and is let in by the member; a message
  it sends there has no `<from-phone>`; the member refuses to pair it; removed at the member, it
  is turned away at both. Run on macOS and on devbox (Linux) through `scripts/test-box.sh`.
- **The Rust client against the real box** (piece 4): one test that starts two real boxes and
  the relay stand-in and drives `kuvo_box::boxes` against them, so the Rust and TypeScript sides
  are checked against each other and not only against vectors.
- **The app** (pieces 6, 7): the existing window tests and shots, with a stand-in of two
  computers: sections, a session opened on the second computer, an away computer, a held one.
- A bug found on the way starts with a test that fails on the old code.

## 7. Questions for the user

1. **When the app runs on a computer that isn't the main one, how does it get paired?**
   Recommended: paste a link made on the main computer, once (works with today's protocol, and
   keeps "only the main computer pairs"). Later, and nicer: the member asks the main computer and
   you approve on any device you already have; that's new protocol, piece 9.
2. **Should the sidebar show every computer's sessions at once, or one computer at a time?**
   Recommended: all at once, in sections, the nearest first. The lead bot on the other computer
   is then always in view. Drafts of both come in piece 5.
3. **"The phone link" switch on a Mac.** With it off, a Mac lets no device in from outside, so
   the desktop app on your other computer couldn't reach it either. Recommended: one switch,
   renamed for what it now does ("Reach this Mac from your other devices"), on by default once a
   second computer or device exists, and the desktop app respects it. The alternative is a
   separate switch for computers and phones, which I don't think anyone would understand.
4. **What the list of paired devices is called**, now that it holds computers too (Settings ›
   Phone, "Paired phones"). Recommended: "Devices", each with its kind. A wording choice that
   also changes the phone app and the docs.
5. **A stolen laptop now reaches every computer of the setup until it's removed** (§5).
   Recommended: accept it, as for a phone, and make removal easy to find from every device. The
   alternative, asking for the Mac's login before the app connects elsewhere, adds a step at
   every launch.
