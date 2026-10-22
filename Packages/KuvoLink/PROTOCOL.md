# Kuvo Link

How Kuvo Code on a phone reaches the agents on a Mac or a machine, from anywhere,
through a relay that can't read what passes through it.

- **Hosts** serve an engine: the Mac app, and a bridge on each machine (`kuvo-link.js`,
  run by the engine's own runtime). A host connects out to the relay; it never listens.
- **The phone** connects out to the relay too, and opens an encrypted session with each
  host it's paired with.
- **The relay** matches connections by address and passes bytes along. It keeps no
  content. It also sends Apple push notifications, whose content it can't read either.

Everything below is implemented twice, in Swift (`Sources/KuvoLink`, for the Mac app and
the phone) and in JavaScript (`Sources/KuvoLink/Resources/kuvo-link.js`, for machines). `Vectors/` holds the
test vectors both run against.

## Keys and addresses

Each device (a phone, a Mac, a machine) has:

- a **static key**, an X25519 key pair, which identifies it to the devices it's paired with;
- a **relay secret**, 32 random bytes, which it shows only to the relay.

Its **address** is the first 16 bytes of `SHA-256("kuvo-link address v1" || secret)`. The
relay checks a connection's secret against the address it claims, so it needs no accounts
or storage, and only the device itself can receive what's sent to its address.

In text (links, JSON) keys, secrets, addresses and tokens are base64url without padding.

## The relay

One WebSocket per device, binary messages, first byte the kind:

| Kind | From | Bytes | Meaning |
| --- | --- | --- | --- |
| `0x01` hello | device | `secret(32)` | Must come first. A second connection with the same address replaces the first (closed with 4002). |
| `0x01` welcome | relay | `address(16)` | The secret was fine; this is your address. |
| `0x02` send | device | `to(16) payload` | Pass `payload` to the device at `to`. |
| `0x02` deliver | relay | `from(16) payload` | Something sent to you. |
| `0x03` unreachable | relay | `to(16)` | Nothing is connected at `to`; what you sent is dropped. |
| `0x04` register | phone | JSON | `{"token", "environment": "sandbox"\|"production", "topic", "senders": [address]}`: send pushes to this APNs token, only from these addresses. Kept for 90 days; the phone sends it again whenever it connects. |
| `0x05` push | host | `to(16) payload` | Push `payload` (a mail frame, below) to `to`'s phone. |
| `0x06` pushed | relay | `to(16) status(1)` | 0 sent, 1 no phone registered, 2 not one of its senders, 3 Apple refused. |

`payload` is at most 65,600 bytes. The relay closes a connection that sends anything else
(4000), a wrong secret (4001), far too much too fast (4003), or one too many from the same
IP address (4005). A device a little too fast, or sending faster than the one it sends to
takes in, is held back: the relay stops reading from it for a moment.

An APNs push carries `{"aps": {"alert": {"title": "Kuvo Code", "body": "New activity"},
"mutable-content": 1}, "f": "<sender's address>", "k": "<payload, base64>"}`. The phone's
notification extension decrypts `k` with the key of the host at `f`, and replaces the text,
the category (for its buttons) and the thread.

## Link frames

What one device sends another, inside the relay's `payload`:

```
kind(1) session(4) body
```

`session` is a random number the phone picks for each session, so frames from an older one
can't be confused with a newer one.

| Kind | Body |
| --- | --- |
| `0x01` | Handshake message 1, phone to host |
| `0x02` | Handshake message 2, host to phone |
| `0x03` | A transport message: one encrypted message (below) |
| `0x04` | Close: no body. Either side stops the session. |
| `0x05` | Mail: one message, encrypted for a device that may not be connected (`session` is 0) |

## Sessions

Sessions use **Noise_IK_25519_AESGCM_SHA256** ([the Noise protocol](https://noiseprotocol.org/noise.html)),
with the prologue `kuvo-link v1`. The phone starts, knowing the host's static key from
pairing; the host learns the phone's from the first message and checks it.

The first message's payload is JSON:

```json
{"v": 1, "name": "My iPhone", "app": "0.1.0", "pair": "<token>"}
```

`pair` is there only while pairing. The host answers if the phone's key is one it has
paired, or if `pair` is the token it's showing; otherwise it sends a close. Its answer's
payload:

```json
{"v": 1, "kind": "mac", "name": "My MacBook", "app": "0.4.0", "engine": "2.0.30"}
```

`kind` is `mac` or `machine`. Each side checks `v` and closes the session on a version it
doesn't know.

Transport messages then go both ways, in order (the relay keeps the order of one
connection). Any message that doesn't decrypt ends the session.

## Mail

Pushes and other messages for a device that may not be connected use
**Noise_K_25519_AESGCM_SHA256** (both static keys known), with the prologue
`kuvo-link mail v1`: one handshake message carrying the payload. A host's push carries a
notification, below.

## Notifications

What the Mac shows as a notification goes to each paired phone too, as the relay's `push`
with a mail frame, sealed with the sender's static key for the phone's. Its payload, JSON:

```json
{"t": "notify", "title": "Fixer", "subtitle": "kuvo-ai", "body": "Wants to run a command: npm test",
 "category": "permission", "thread": "ses_…", "data": {"session": "ses_…", "request": "per_…"}}
```

| Field | Meaning |
| --- | --- |
| `title` | Who's telling: a bot's name, a session's title, a group chat's name. |
| `subtitle` | Optional: the project, or who in a group chat. |
| `body` | What the Mac shows: the request, the question, the last words of a reply. |
| `category` | What it's about, below. |
| `thread` | What it's grouped under: the session's id, or the group chat's. |
| `data` | Where it opens: `session` or `group`; `request` for a permission or a form, with `requestSession` when the session asking isn't the one shown (a subagent's), and `answerable: "0"` when its notification mustn't answer it from buttons (it can't show all it covers, or some of its text shows as dots); `ask` for a browser ask. |

| Category | When |
| --- | --- |
| `permission` | An agent asks to do something. `request` is the permission. |
| `question` | An agent asks a question, or for a form filled in. `request` is the form. |
| `ask` | An agent browsing asks for a sign-in, a code or a step. `ask` is it. |
| `finished` | A run finished. |
| `failed` | A run stopped with an error. |
| `report` | A bot's run finished: a wake-up's or a watch's news. |
| `message` | Something in a group chat. |

A phone should ignore a category it doesn't know, and show the notification plainly.

The payload stays under 2,500 bytes, so the push fits the 4 KB Apple takes: a body that
doesn't fit is cut, ending in "…". Keys are sorted, as both sides write them.

Who sends it:

- **The Mac**, for its own sessions, its group chats and browser asks, and the sessions on
  a machine it's connected to. Only while you're away from it (no input for two minutes, or
  its screen locked), or while a phone has a session open (the phone leaves out what's on
  its screen); at the Mac, its own notifications are enough.
- **A machine's bridge**, for that machine's sessions (permissions, questions, runs finished
  or failed, bots' news), while no Mac is connected to it. A Mac that's connected touches
  `~/.kuvo-code/run/present/<its id>` each minute and removes it when it quits; the bridge
  tells your phones only when none of those files is less than 150 seconds old. The Mac
  tells them only for 120 seconds after it last touched its file, so the two never both do.

## Messages in a session

Each transport message decrypts to:

```
headLength(2) head(JSON) body
```

and is at most 65,519 bytes, so longer bodies go in pieces. The phone asks, the host answers,
like HTTP:

| Head | Meaning |
| --- | --- |
| `{"t": "req", "id": 7, "m": "GET", "p": "api/session?limit=50", "h": {}, "end": true}` | A request. `id` counts up from 1 for each session. `end: false` means `data` pieces follow. |
| `{"t": "res", "id": 7, "s": 200, "h": {"content-type": "…"}, "end": false}` | The response's status and headers, with the first piece of its body. |
| `{"t": "data", "id": 7, "end": false}` | More body. Event streams (`api/event`) go on until cancelled. |
| `{"t": "cancel", "id": 7}` | Either side stops request 7. |
| `{"t": "ping"}`, `{"t": "pong"}` | Is the other end still there? The relay's own pings only reach the relay. The phone pings before sending on a session that's been quiet (a host started again since closes the session, and the phone opens a new one before anything's lost), and while a request waits with nothing from the host (no answer ends the session). Answer at once. |

`h` holds `x-opencode-directory` and `content-type`; the host adds the engine's password
itself. A host serves:

- `api/…`: passed to its engine, all of its API. A paired phone is yours, with the reach the app
  has on that computer: sessions, files, terminals and commands, accounts and their keys, the
  engine's settings, MCP servers. Only a path that isn't what it seems is refused (403): one with
  `..` (plain or percent-encoded), a backslash, a fragment, control characters, a space at
  either end, or a sign joined into one character with a mark after it. The commands the engine
  runs for agents: `GET api/shell` (those running in the folder `x-opencode-directory` names),
  `GET api/shell/<id>` and `GET api/shell/<id>/output?cursor=&limit=`; their `shell.created`,
  `shell.exited` and `shell.deleted` come with `api/event`.
  A subagent's session (`metadata.sessionID` of the `subagent` step that started it) is a session
  like any other, which is how the phone watches and talks to one, as the Mac's subagent tab does:
  read its messages, prompt it (`steer` while it works: it reads it at its next step; once it's
  done, it carries on), `POST api/session/<id>/interrupt` to stop it (the agent waiting on it
  hears it was cancelled, and carries on), and choose its model. Choose it without interrupting:
  an interrupt, even with `resume`, ends that agent's wait, and it gives up on the subagent.
- `POST kuvo/shell/<id>/stop` (`{"session": "ses_…"}`, which may be left out): stops a command an
  agent left running, as Stop over the Mac's composer does: SIGTERM to its process group, so the
  engine sees it end and tells the agent, with what it printed; only if that can't be sent, the
  engine's own remove (which deletes the output, and the agent hears the command wasn't found).
  Answers `200 {"stopped": "signal"}` or `200 {"stopped": "removed"}`, or `404 {}` when no such
  command runs. A machine's bridge does it there (from 0.5.1), with the Mac closed too.
- `kuvo/info`: what the host is and runs, and whether its coding agents' bridge is up
  (`agents`; a machine's bridge says so from 0.6.0), and `started`, when the engine running now
  came up, in seconds since 1970 (null while none runs; a machine's bridge from 0.9.2): a restart
  changes it, so a phone tells the new engine from the old; a Mac also names its folder of chats
  (conversations of no project), and `integrations`, each integration's name by its MCP server's
  (`{"microsoft-365": "Outlook and Microsoft 365", …}`), for a card when one asks something.
- `kuvo/agents/api/…`: the coding agents you already have (Claude Code, Codex, Cursor, Grok,
  Antigravity), through the agents bridge beside the host's engine, which answers the same paths
  as the engine for them: sessions, messages, prompts, permissions, questions, events
  (`kuvo/agents/api/event`) and models (`kuvo/agents/api/model`, only signed-in agents'). Its
  sessions carry `metadata.kuvoAgent` and run where that bridge does; a new session with one of
  their models (providers `claude-code`, `codex-cli`, `cursor`, `grok-cli`, `antigravity`) is
  made there, never on the engine. A Mac answers for its own; a machine's bridge (from 0.6.0) for
  the machine's, with the Mac closed too: each machine signs in to them on its own, and has its
  own Custom Auto lists. `kuvo/info` has `agents: true` while the bridge is up; otherwise these
  answer 503. All of its API, but for the two routes only the app wires things with:
  `api/kuvo/engine` (where the engine is) and `api/kuvo/groups` (group chats' own steps, which a
  phone takes through `kuvo/groups`). The same on both (`Vectors/link.json`, `agentRoutes`).
  - Accounts: besides each agent's own sign-in, ones you added, each its own provider
    `<agent's provider>@<id>` (`claude-code@1f2e3d4c`), named `Claude Code · Work` in
    `kuvo/agents/api/integration` (`metadata.accountID`, `metadata.accountLabel`). Their models
    and sessions are theirs. `kuvo/agents/api/kuvo/accounts`: `POST {agent, label}` (agent:
    `claude`, `codex`, `cursor`, `grok`, `antigravity`) → `{data: {providerID}}`, then sign in as
    any; `PATCH …/accounts/<provider> {label}`; `DELETE …/accounts/<provider>`.
    `metadata.signInEnded` true: its maker ended the sign-in; mark the account (the Mac says
    "Sign-in ended") and offer to sign in again.
  - Signing in to an agent: `POST kuvo/agents/api/integration/<provider>/connect/oauth {methodID,
    remote}` → `{data: {attemptID, url, instructions, mode, acceptsCode?, userCode?, time}}`; `GET
    …/connect/oauth/<attempt>` → its status (`pending`, with `url`, `instructions`, `userCode` and
    `message` as they come, for an agent installed first;
    `complete`; `failed` with `message`; `expired`); `POST …/connect/oauth/<attempt>/complete
    {code}` for one with `acceptsCode`; `DELETE …/connect/oauth/<attempt>` to give up. A key:
    `POST …/connect/key {key}`. With `remote: true` the page is for another device (the phone),
    so it doesn't come back to the computer's `localhost`: Claude Code's ends on a code to paste
    back (`acceptsCode`), Codex's and Grok's are device codes (`userCode`, shown beside the page,
    to copy), Cursor's finishes by itself. An agent whose sign-in can do that has
    `metadata.remoteSignIn` true in `…/integration`; for the others (Antigravity, whose Google page
    comes back to `localhost`) `remote: true` answers 400: sign in on the computer it runs on.
    Signed in, an account's Custom Auto limit goes, as when you sign in there.
  - Custom Auto: a session's model `{providerID: "auto", id: <list ID>}` runs down that list,
    going on with the next model at a usage limit or an error. `kuvo/agents/api/kuvo/auto`: `GET`
    and `PUT {lists}` → `{data: {entries, lists: [{id, name, entries: [{providerID, modelID,
    variant?}]}], limits: [{key, until, scope, known}]}}` (the first list's ID is `auto`;
    `entries` is the first list's; `key` is a provider for an account's limit, `provider/model`
    for a model's; `until` in ms); `DELETE …/auto/limits` tries them all again. The event
    `kuvo.auto.updated` says the lists or their limits changed. An account with a `provider`
    limit whose `until` hasn't come is at its usage limit: mark it (the Mac says "Usage limit ·
    Resets in …").
  - A session with the agents may switch to any model (`…/session/<id>/model`), the engine's
    too: it runs those through an engine session of its own, which carries
    `metadata.kuvo.engineFor` and is never shown (hide it in the engine's session list).
    `…/session/<id>/fork {before}` makes a new session from a message.
- Plan accounts (Copilot, OpenCode, GitLab, Poe, a provider's API key), on a Mac and (from
  bridge 0.7.5) a machine, as Settings › Accounts has them: the phone sees and signs in to them,
  and (from bridge 0.8.0) reads them as the app does. A machine's plans that its Mac
  also has a key or a sign-in for that stays the same (Copilot's) follow the Mac's: each time
  the Mac connects, its own replaces what's in use there. Change those on the Mac, or with the
  Mac's own routes; a change made on the machine lasts for plans the Mac has none of, and for
  sign-ins that renew themselves (ChatGPT's, GitLab's), which each machine keeps its own of.
  - `GET api/integration` and `GET api/integration/<id>`: each plan, its ways to sign in
    (`methods`) and what it's signed in as (`connections`: label and status).
  - A key: `POST api/integration/<id>/connect/key {key, answer?}`.
  - A sign-in page: `POST api/integration/<id>/connect/oauth {methodID, answer?}` → the attempt
    (its URL, `mode`, and `acceptsCode`); `GET …/connect/oauth/<attempt>` for its status; `POST
    …/connect/oauth/<attempt>/complete {code}` for one that ends on a code; `DELETE
    …/connect/oauth/<attempt>` to give up. A page that sends the browser back to `localhost` on
    the computer (GitLab's, Poe's, ChatGPT's) finishes there with `kuvo/sign-in/return` (below).
    Device codes (Copilot, OpenCode Console, SuperGrok) and keys work from anywhere.
  - Its sign-ins: `GET api/credential` (with what each is signed in with), or `GET
    kuvo/accounts/credentials` → `{credentials: [{id, integrationID, label, active}]}`, the list
    alone; `POST api/credential/<id>/activate` uses one; `DELETE api/credential/<id>` removes one.
  - On a Mac only: `GET kuvo/accounts/ended` → `{providers: [id]}`, the plans whose sign-in in use
    ended (the service refused it; the Mac says "Sign-in ended"); `POST
    kuvo/accounts/<provider>/forget` is Disconnect, as Settings › Accounts does it: its sign-ins
    and keys on the Mac, and the copies on your machines.
  What a phone may ask an engine for is the same on both (`Vectors/link.json`, `routes`).
- On a Mac, `kuvo/hosts`: the machines this phone may reach directly, with their addresses
  and static keys, so pairing once with the Mac reaches them all; each with `machine`, its id
  (as a bot's `machine`), and `anotherMac`: its engine is another Mac's, which gives it its
  settings, so this Mac's (`kuvo/config`) don't reach it (as it was when it last connected).
- `POST kuvo/sign-in/return` with `{"url"}`: where a sign-in's page sent the browser back to the
  computer (`http://127.0.0.1:<port>/…?code=…`: a plan's that finishes there, an MCP server's at
  `127.0.0.1:19876`, a coding agent's), which a phone can't load. The computer loads it, as the
  browser would have: only its own address (127.0.0.1, localhost or [::1], over http, at a port,
  with a query; a fragment is left off), and no further than it answers (a redirect isn't
  followed). `200 {"status"}` with the status it got: 2xx or 3xx, it took it; `400 {"message"}`
  for an address that isn't one; `502 {"message"}` when nothing there answered (start the
  sign-in again). Send it to the computer the sign-in is on: a Mac for its own, a machine's
  bridge (from 0.9.0) for the machine's, the engine's and the coding agents'. The same check on
  both (`Vectors/link.json`, `signInReturns`).
- On a Mac, `kuvo/config`: the engine's settings as Settings has them, the whole document (the
  same that `opencode.json` holds); your machines get them too (but for what only makes sense on
  the Mac), so a machine's settings are changed here. `PATCH kuvo/config` with `{"changes":
  [{"path": ["mcp", "servers", "linear", "disabled"], "value": true}, …]}` changes some, as
  Settings does (a `null` value removes the key, and a level that isn't there is made; a path
  goes through objects only, so a list is sent whole, at its own path: 400 otherwise), and
  answers with the whole document; machines take it a few seconds later, and a change to the
  rules reaches open sessions. A change can say what it read at its path (`"was"`, `null` for
  nothing): changed since, nothing is changed and it answers `409 {"message"}`, so read again.
  `409 {"message"}` too while the file can't be read (it has comments, say): it's changed by
  hand then. Settings › Permissions keeps its choices and rules under `permissions` (a list of
  `{action, resource, effect}`, effect `allow`, `ask` or `deny`) and its blocks under `policies`. A machine's bridge (from
  0.9.0) answers `kuvo/config` and `kuvo/mcp/catalog` with `409 {"message"}`: its Mac changes them.
- On a Mac, `kuvo/mcp/catalog`: Settings › MCP Servers' popular servers, `{"servers": [{id,
  title, summary, access, added, integration, macOnly}]}`: `access` is `signIn`, `open` or
  `local`; `integration` when Integrations added it (it's kept there, with its sign-in: don't turn
  it off or remove it); `macOnly` for one that uses the Mac's own apps (Chrome DevTools), which
  never reaches your machines, so leave it out of a machine's list. `POST kuvo/mcp/catalog/<id>`
  adds one as its Add button does and answers with the settings, as `kuvo/config`; turn one off
  or remove it through `kuvo/config` (`mcp.servers.<name>`). The rest is the engine's: each
  server's status from `GET api/mcp`, connecting again with `POST
  api/experimental/mcp/<name>/connect`, a server's sign-in through `api/integration/<id>/connect/…`
  (its page comes back to `127.0.0.1:19876`: `kuvo/sign-in/return`), and the approvals saved for
  a project with `GET api/permission/saved?projectID=` and `DELETE api/permission/saved/<id>`.
- `POST kuvo/engine/restart`: the computer's engine starts over, as Settings › Engine's Restart
  does on a Mac; what's running in it stops. It answers `202 {}` first: the link goes down with
  the engine, and the computer is back once it answers again (`kuvo/info`). A machine's bridge
  (from 0.9.0) starts its engine over as it was last started (with its coding agents' bridge and
  the link's), and a Mac connected to it goes on with the new one; one never started from a Mac
  answers `409 {"message"}`.
- `kuvo/bots`: bots by their session, with their `id` (`bot_…`, as group chats name their
  members; a machine's bridge from 0.9.4), their name, look and job, whether their wake-ups
  are paused, the next one (`wakes`, left out when none waits), how many things each watches,
  `machine` (the machine it lives on, left out for the Mac), and `agents`: its conversation is
  with the coding agents, so it's read and written through `kuvo/agents/api/…` (a group chat's
  members carry the same flag). A Mac lists all of yours, wherever they live. A machine's bridge
  (from 0.6.0) lists the bots that live on that machine, as their folders are now, with the Mac
  closed too: their name, look and job as the Mac last wrote them there (`bot.json`, which the
  Mac writes when they change and each time it connects; one it hasn't written yet isn't
  listed). For a machine's bot, its machine's list is the newer while it answers.
- On a Mac, making a bot as New Bot does. `GET kuvo/bots/templates` is what New Bot offers:
  `{"templates": [{id, name, job, instructions, look, personality, wantsProject}],
  "personalities": [{id, name, description, custom}], "looks": {colors, shapes, faces},
  "startingModel": {"mac": {providerID, id, variant} | null, "<machine id>": … | null},
  "projects": [{directory, name, machine}]}`. A template's `personality` and a project's
  `machine` (null for the Mac) are left out when there's none. `startingModel`'s keys are the
  places a bot can live (the Mac and each machine), each with the model New Bot starts with
  there. `POST kuvo/bots` with `{name, look?, personality?, model?, machine?, project?,
  template?}` makes one: `personality` by id (`preset.*`, or `custom.*` for one you made);
  without `look` or `personality`, the template's, or a random look; without `model`, that
  place's starting model; a `project` on a machine makes it there (`machine` is then ignored).
  It answers `201 {"bot": <as kuvo/bots lists one>, "firstMessage"}`: from a template, the bot
  waits, and `firstMessage` is its job for your composer, to edit and send; otherwise it's null
  and the bot writes first. Errors are `{"message"}`: 400 for a missing name or an unknown
  template, personality, machine or project; 409 while its machine isn't connected; 422 for a
  model that can't run there (its account isn't signed in there); 503 when it couldn't be made.
- On a Mac, `POST kuvo/groups` with `{name, members, purpose?}` (members by bot `id`, at least
  two) makes a group chat on the Mac, as New Group Chat does: `201 {"group": <as kuvo/groups
  lists one>}`, 400 `{"message"}` without a name or two bots, 404 `{"message"}` for a bot that
  isn't there. Moving it to a machine is the Mac's Move to Computer. A machine's bridge (from
  0.9.4) answers these three with `409 {"message": "Make it from your Mac; it can live here."}`:
  the Mac keeps the list of bots and chats.
- On a Mac, `kuvo/named`: who you can name with `&` in a message, as the Mac's composer
  offers them: `{"agents": [{session, name, kind, machine, project, about}]}`, every bot
  (`kind: "bot"`, `about` its job) then the latest sessions with a title (`kind: "session"`,
  `project` its project's name, `"Chats"` for a chat); `machine` is null on the Mac. Insert
  `&` and the name, and send the message as it is: the Mac adds to a prompt it passes on
  (`api/…` and `kuvo/agents/api/…`) a block naming each agent the text names, which the
  agent reads, so a machine's bridge, with the Mac closed, sends it without. Show a message
  without it: it ends the text, from a line `<named-agents>` (then a heading line, then
  `- &<name> (<session>): <what it is>` lines) to a line `</named-agents>`, before the
  `<review-comments>` and `<attached-files>` blocks. Show each `&<name>` it lists in bold
  (a bot's in its color), opening that session.
- A prompt a phone sends (`POST api/session/<id>/prompt`, and `kuvo/agents/api/…` alike) gets a
  block at the end of its text that tells the agent it's from the phone: a line `<from-phone>`,
  the words in `FromPhone.swift`, and a line `</from-phone>`. The Mac adds it, and a machine's
  bridge (from 0.9.4) in the same words with the Mac closed; send the text without it. Show a
  message without it (everything from a line `<from-phone>` to the line `</from-phone>`), as
  sent from the phone. It tells the agent to keep replies short and to show files with
  `show_file`.
- An agent shows a file in the conversation with `show_file`, one of Kuvo Code's tools (on the
  Mac and machines, for the engine and the coding agents): a tool call named
  `kuvo-browser_show_file` with input `{"path": "<full path on the computer it runs on>",
  "caption"?}`, which fails when the path isn't a file. Show a completed one in the
  conversation's flow, as `generate_image`'s: a picture (by its extension) large, any other
  file as a row that opens it, its caption under it; get its bytes with `kuvo/file` from the
  computer the session runs on.
- `kuvo/wakes/<session>`: a session's (or chat's) wake-ups and watches, which its
  agent sets with `kuvo-wake`, as the bar over the Mac's composer shows them: `{"wakes": [{id,
  due, note, every}], "watches": [{letters, every, check, note}], "paused"}` (`due` in seconds
  since 1970, `every` like `1d` or null; paused while it's archived). `POST
  kuvo/wakes/<session>/cancel` with `{"wake": "<id>"}` or `{"watch": "<letters>"}` cancels one;
  `POST kuvo/wakes/<session>/now` with `{"wake": "<id>"}` sends it now (one that repeats still
  comes back at its time). Both answer `200 {}`, `404 {}` when there's no such one, `409
  {"message"}` while its conversation is on its way to another computer (a machine's bridge
  from 0.9.3), or `503 {"message"}` when it couldn't go. A Mac answers for its sessions and its machines'; a
  machine's bridge (from 0.6.0) for its own sessions and bots, with the Mac closed too (a
  session that keeps none answers an empty list). A wake-up reaches the conversation as a
  message starting with `[Wake-up] `. `GET kuvo/wakes` lists every session with a wake-up or a
  watch at once, for the marks beside them in a list: `{"sessions": [{session, next, watching}]}`
  (`next`, the soonest, in seconds since 1970, left out when it has none). Bots' come with
  `kuvo/bots`. A Mac lists its sessions' and those it has read of its machines'; a machine's
  bridge (from 0.7.4) its own.
- On a Mac, `kuvo/asks`: what agents ask you for while they browse, on the Mac or a machine: a
  sign-in (the fields the page has, and the accounts Passwords keeps for the site, never
  their passwords), a one-time code, or a step only you can do. `kuvo/asks/events` is the same
  list as a response that doesn't end: a line of JSON now, and another each time it changes.
  `POST kuvo/asks/<id>` answers one: `{"answer": "signIn", "username", "password", "save"}`
  (or `"saved": "<account>"`, which the Mac fills in from Passwords itself), `{"answer":
  "code", "code"}`, `{"answer": "done"}` or `{"answer": "declined"}`. What's typed goes into
  the page, never to the agent.
- On a Mac, `kuvo/groups`: your group chats, with their bots (face, name, session), who's
  talking, whether one is under way or paused, and the last thing said; `kuvo/groups/events`
  streams that list as it changes, and `kuvo/groups/<id>/events` one chat's latest messages,
  the same way. `POST kuvo/groups/<id>` writes to it (`{"text"}`); `…/stop`, `…/keep-going`
  and `…/viewed` do what they say; `…/undo` (`{"message"}`) takes a message of yours back and
  answers with what was taken. A wrap-up can carry `tasks`, who does what: each with `id`, `bot`,
  `task`, `state` (`proposed`, `waiting`, `working`, `done` or `failed`), `started`/`finished`,
  and `after` (the bots whose parts come first; it waits for them);
  `…/start` (`{"message"}`, the wrap-up) hands each proposed part to its bot's own conversation.
  While a bot talks, one chat's events carry `activity`, what it's doing, its last step:
  `{"kind": "tool", "call"}` (a tool call as a message's `steps` hold them, without its output),
  `{"kind": "thinking", "text"}` (the line its thinking is on) or `{"kind": "writing"}`. Show
  them under who's talking, so a long turn doesn't look stuck. A line of one chat's events whose
  messages are as they were leaves `messages` out (only who's talking or what they're doing
  changed): keep the ones you have. The first line always has them (a machine's bridge from 0.9.0).
  A bot's message with `report` (a task's id) is it reporting back on its part. Times are
  seconds since 1970.
  A Mac lists all of yours: its own, and those that live on your machines (whose bots all live
  there), which it follows from there; each with `machine`, the machine it lives on (by id, as a
  bot's `machine`; none for the Mac), and `moving` while it's on its way to another computer
  (one chat's events say it too): what you'd write waits until it's there, and `POST
  kuvo/groups/<id>` and `…/undo` answer 409 meanwhile, as they do while its machine is away.
  A machine's bridge (from 0.7.1) answers the same routes, in the same shapes, for the chats that
  live on it, with the Mac closed too: the coding agents' bridge there runs them (503 while it
  isn't up or isn't answering: ask again; 404 only when none run there). One moving between computers isn't listed
  where it's going until it's there. Moving one is on the Mac (Move to Computer), not the phone.
- `kuvo/file?uri=<file:// URI>`: a file on that computer that an agent's reply or tool names
  (a picture it made, a file it shows you), by its `file://` address, percent-encoded as a query
  value: `200` with its bytes and `content-type` (from its extension; `application/octet-stream`
  when unknown), `404 {"message"}` for one that isn't there or isn't a file, `413 {"message"}`
  over 25 MB, `400 {"message"}` for an address that isn't `file://`. A Mac answers for its own
  files; a machine's bridge (from 0.9.4) for the machine's, with the Mac closed too.
- On a Mac, `kuvo/usage`: the menu bar's usage panel as its Bars view lists it (each provider's
  name, plan, brand color, mark as a PNG to tint, notice, whether it's outdated, and its lines as
  UsageKit writes them), whether a refresh is under way and when the last one ended;
  `kuvo/usage/events` streams it as it changes, and `POST kuvo/usage/refresh` refreshes it all.
  A Mac without the usage panel answers 404. Times are seconds since 1970.

### Sockets

A request with `h.upgrade = "websocket"` opens a WebSocket on the host's engine; today only a
terminal's (`api/pty/<id>/connect`). Its body is empty. The host answers `res` with `s: 101`
once the engine's socket is open, or another status and `end: true` if it won't open. Then
each `data` message for that `id`, either way, is one message on the socket: text, or binary
if `h.type = "binary"`. Text from the engine too long for one message goes in several;
binary that long doesn't go. The phone closes it with `cancel` (or `data` with `end: true`);
when the engine closes it, the host sends `data` with `end: true`.

## Pairing

A host shows a link, as a QR code:

```
kuvo-link://pair?v=1&relay=<relay URL>&host=<address>&key=<static key>&token=<16 bytes>&name=<name>
```

The phone opens a session with `pair` set to the token. The host checks it (and on a Mac
asks you to confirm), then keeps the phone's static key. A token works for 10 minutes and
once.

On a machine the bridge keeps its state in `~/.kuvo-code/run/link.json` (mode 600):

```json
{"relay": "wss://…", "secret": "…", "key": "<static private key>", "phones": [{"key": "…", "name": "…", "added": 1760000000, "from": "mac", "address": "…"}]}
```

The Mac sets the phones paired with it (`"from": "mac"`), so they reach the machine too,
and takes a phone off when you unpair; phones paired with the machine itself (`"pair"`)
stay. `address` is the phone's on the relay, where its notifications go: the Mac passes on
what it knows, and the bridge notes it each time the phone comes in. The machine's keys stay when no phones are left, so pairing again gives the same
address. The bridge reads the file every 30 seconds: a phone taken off loses its sessions,
and with no phones and no pairing waiting the bridge stops. From 0.6.0 the bridge notes when a
phone last used the machine (`run/link.seen`): while one has in the last week, the engine
stays up for it, rather than its watchdog stopping it after a day without the Mac, and a Mac
quitting leaves it running too; so the machine's sessions and bots are there when the phone
asks. A phone lost, or unpaired somewhere the machine doesn't hear of, stops keeping it a week
after it was last used.
