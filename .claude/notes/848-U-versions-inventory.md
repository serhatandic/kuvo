# #920 · Storage and message version inventory

Baseline is piece T at `819e4f5a6`. Scope is the computer's Kuvo home (including the old Mac home when imported) and the Box's public protocol. The ownership draft is organized by user data. The rows below group files that share one versioning contract. Counts are of families, including generated or third-party families. An "unmarked" row has no independent version for its contents at baseline. A build string, ID, timestamp, revision, cursor or encryption algorithm is separate from a schema version.

## Stored formats

| Family | Locations and source | At baseline and reader behavior |
| --- | --- | --- |
| Link state | `run/link.json`, `link/state.ts` | Unmarked. The reader folds beta peers into legacy state. Piece U reads old files, stamps new writes and refuses unknown versions. |
| Signed documents | `run/link.json` docs and signed exports, `link/documents/format.ts` | Signed `v:1`; `n` orders revisions. Incoming documents reject other versions. Piece U applies the same version check on local reads. |
| Phone trust and sync | `run/phone-trust.json`, `link/documents/phone-sync.ts` | Unmarked tracking record. Signed membership and phone lists are independently protected by the document format. |
| Setup discovery and welcome | `run/setup-card.json`, `run/welcomed.json`; `link/setup` | Unmarked. The card and welcome reader check known fields. Missing cards can be discovered by old link/process files. |
| Mac link and remembered machines | `run/mac-link.json`, `machines.json`, legacy `phone-link.json`; `link/mac` | Unmarked. Readers default missing settings and recognize legacy connection fields. |
| Peer directory and reach | `run/{agents,mac-agents,peer-agents,peer-reach,peer-reach-checked,reach-again}.json`; `link/peers` | Unmarked. Known fields are selected, timestamps drive freshness, and build/feature fields describe reachable software. |
| Add, reinstall and seed | `run/{adding,keep-up,machine-helper-job,seeded}.json`, `updates/installed.json`, `link/add` and `link/install` | Unmarked. Requests and receipts use state/step names. No common unknown schema check. |
| Work Queue ledger | `run/work-queue/numbers.json`, `link/work-queue/numbers.ts` | Unmarked. Old readers normalize lease/claim fields and ignore extra keys. Piece U stamps version 1 on the next write and refuses unsupported versions before allocating. |
| Work Queue categories/status migration | `run/work-queue/{model,categories}.json`, `before-statuses/`, `link/work-queue/migrate.ts` | `model:2` marks the status model. The old migration keeps a whole snapshot and converts entries without statuses. Higher model numbers are currently treated as done. |
| Work Queue items and history | `run/work-queue/queues/<owner>.json` and old backups, `link/work-queue/store.ts` | Unmarked item/file shapes. Known fields are normalized, corrupt files can be set aside, and queues with unfamiliar owners stay on disk. |
| Work Queue labels, delivery and receipts | `run/work-queue/{labels,acted,seen,queues-carried,queues-scanned}.json` and the journals in `link/work-queue` | Unmarked. Validates fields by operation and uses IDs/timestamps for retries. |
| Managed attachment files | `run/work-queue/files/`, session attachments, `link/server/attached.ts` | Byte streams. File type/name/hash, size and identity belong to owner metadata. Consumers interpret the media type. |
| Engine database | `data/opencode/opencode*.db{,-wal,-shm}` | Engine migration history is checked during the staged legacy move. |
| Conversation metadata and queue | `run/{queues,typed-sent}.json`, agent session data, `link/engine`, `agents/store.ts` | Unmarked JSON; readers normalize old shapes. |
| Coding agent native storage | `agents/` native homes and named instances, including provider auth files and logs such as Claude/Codex JSONL, Cursor chat files, Antigravity SQLite/Protobuf | Provider-owned schemas. Kuvo reads provider formats it understands and leaves native migration to the provider. No uniform Kuvo mark. |
| Letters, receipts and calls | `run/outbox/`, `run/{letters-seen,refusals,told,served,sent,rows}.json` where used, `link/letters` | Unmarked. Letter IDs and timestamps protect deduplication and replay. Old app letters have their own import. |
| Pending work and work status | `run/{asks,answers,awaited,background,decided,goals,homes,oversight,reviews,side-questions,started-elsewhere,stopped,unanswering}.json` where used, session notes, `link/work` | Unmarked. Each reader selects known state and keeps separate delivery records as applicable. |
| Bot metadata and edits | `bots/<id>/{bot,settings,kit,job}.json`, `run/{bots-held-taken,bots-left,deleted-there,edited}.json`, `link/bots` | `bot.json` and kit have `kuvo:1`; edit/spool state is unmarked. Existing app move fills missing pieces and edits use field timestamps. |
| Bot text and ownership markers | `bots/<id>/{MEMORY.md,notes.md,session,directory,via}`, workspace and skills | User text/opaque files or single-value markers. The app move keeps existing files. |
| Wake-ups and watches | `wakes/`, bot schedules and `wakes.json` records, `link/work/wakes.ts` and keeper | Unmarked. Time, recurrence, delivery and cancellation are interpreted by existing readers. |
| Group chats and bridge delivery | `groups/<id>/{group,messages}.json`, `pending-memory.json`, `agents/{peer-memory,peer-turns,peer-outbox,peer-work}.json` where used; `agents/groups` | `group.json` and group settings use `kuvo:1`. Group reader skips newer records. Transcripts and peer spools are unmarked. Old aggregate/per-chat layout is imported with a kept copy. |
| Projects and worktrees | `box/{projects,worktrees,found-folders,folders}.json` where used, actual worktrees and Git refs | Unmarked Kuvo JSON, shape-based import. External files and Git data follow their own tools. |
| Git preferences and backups | `box/git.json`, per-repository backup objects and refs, Git route consumers and `shared/git.ts` | Unmarked Kuvo settings. Git owns its objects. The old settings import has a completion flag. |
| Artifacts | Artifact index and content-addressed copies, `agents/artifacts` | Unmarked index. Per-file `n` is a content revision. |
| Search database | `session_search.sqlite`, search routes | Schema owned by the engine, with no independent Kuvo mark. |
| Moves and handoffs | `run` move journals and receipts, archives and attachment bytes, `link/moves`, `link/work-queue/hand.ts` | Unmarked journal and bundle fields. Workflow stages, checksums and IDs provide recovery and duplicate protection. |
| Computer settings and local tools | `box/{settings,settings-flows,pages,work,auto,approval,commands,permissions,permissions-later,models,model-names,kuvo-models,compatibility}.json` where used; bundled integrations and templates, `link/areas` | Most owned settings are unmarked; bundled bot words use `kuvo:1` and the reader validates it. Other bundles track the enclosing release. |
| Engine/plugin/MCP configuration | `config/opencode/{opencode.json,plugins/}`, `box/opencode.json`, other programs' TOML/JSON configs, `bin/{integrations,integrations-mac,mcp-catalog,bot-kit,bot-words}.json` | Engine, plugin or bundle owned. Kuvo explicitly rewrites only known keys when importing or configuring. |
| Local fences and audit | Sandbox allowed list, policy JSON, audit JSONL, Ask First decisions and broker receipts, `link/fence`, `agents/fence.ts` | Permission helper process notes carry `v:1`; most other records are unmarked. A policy revision is separate from its schema. |
| Personalities | `bots/personalities.json`, legacy `personalities{,-removed}.json`, signed export | Library writer emits `kuvo:1`; the reader selects the library fields and does not gate on the marker. Edit versions and tombstones determine merge order. Wire document is signed v1. |
| Time zone | `time-zone.json` as used by `link/zone/setting.ts` | File uses `kuvo:1`. Change count orders accepted changes. The parser declines an unknown marker. |
| Vault key wrapper | `vault/key.json`, `link/vault/store.ts` | `v:1` wrapping format. An unknown version cannot unlock the key. |
| Vault integrity records | `vault/{items,grants}.json` and authenticated indexes, `link/vault/store.ts` | Signed envelope `v:2`, legacy plain JSON handled by the existing reader. Generation tracks integrity, not schema. |
| Vault values, activity and pause | `vault/values/`, `activity.jsonl`, `paused.json` | Existing per-type encoding. Value sealing has an encryption contract; activity/pause records lack a common version. |
| SSH trust and agent sockets | `known_hosts` material, local agent sockets, `link/add/ssh.ts`, `link/vault/ssh*.ts` | OpenSSH text/wire contracts. A socket path identifies a process, not a schema. |
| Sign-ins and integration credentials | Managed account index and agent homes, `box/keys/`, integration key/token files, engine auth, Mac Keychain | Unmarked Kuvo metadata and opaque provider secrets. Existing seed/import and sign-in logic decides on a provider basis. |
| Plan limits and usage readings | `agents/usage/` cache/state, `limits*.json`, `usage.json`, `usage-delivered.json`, append-only call summaries | Unmarked. Each has its own fallback or refresh rules. |
| Cursor fallback cache | `agents/usage/cursor-history.ts` | `version:1`. Unknown cache versions are rebuilt. |
| Usage settings | `agents/usage/settings.json`, legacy app Usage settings | Writes `kuvo:1`. Field stamps order merges and the old app importer records what it imported. |
| Insights SQLite | `agents/usage/insights.sqlite{,-wal,-shm}`, `agents/usage/insights-store.ts` | Unmarked SQLite application schema (`user_version=0`). Piece U uses a transaction to adopt the deployed tables as 1, and refuses other versions. |
| Browser profile | `browser/` Chromium data and CDP, `link/browser`, `browser` | External Chromium versions and format rules. Kuvo keeps the profile local and pins the browser build. |
| Browser history and settings | browser-owned Kuvo settings/visits/icons, `history.json`, `visits.jsonl`, `clear-sites.json` where used | Unmarked local shapes; legacy app history and settings import on first start. Icons are opaque bytes. |
| Desktop settings | `desktop/` preferences and remembered connections, other settings in macOS preferences | Unmarked application data or platform-managed preferences. Readers understand old defaults/layouts. |
| Runtime rendezvous | `run/{engine,bridge,serve,keeper,tools,host}.json`, `*.pid`, `*.version`, socket names, `shared/records.ts` | Unmarked records; build and release values are descriptive. Some readers verify the PID/command before trusting a process. |
| Runtime markers and locks | `run/{stopped,paused,attached,serve.needed,work-awake.json}`, lock directories and replacement tokens | Presence/timestamps/ownership, unmarked. Checked by corresponding keepers. |
| Repair and update state | `run/{repair,update,engine-swap}.json`, `update-trial`, rollback script, `updates/rollback/` | Unmarked journals and generated scripts. Current recovery checks the trial and returns the whole installed set to the previous release. |
| Installed release | signed `boxes.json`, `box-<version>.tar.gz`, extracted manifest and binaries | Signed `kuvo:"box/1"`; release/engine versions select supported binaries. Enclosing pack checked by size/hash, never installed if verification or the engine compatibility check fails. |
| Rehome journal and staged trees | rehome record, path plan, data copies, backout and counts under the move's work/home | Journal decoder requires `v:1`; step names checkpoint the move. Most staged metadata files lack separate schema tags. Piece U calls unchanged 0.5–0.7 import under the shared 0→1 declaration. Count checks, staging and rollback still apply. |
| Logs and disposable caches | `run/*.log`, diagnostic JSONL, logs and downloaded pricing/model/package data | Unmarked or upstream-owned. Readers tail text, tolerate incomplete final lines where supported, or regenerate caches. |
| Legacy Mac source | legacy app preferences plist, JSON, app data, bot/group/letter files named in `PROTOCOL.md` “What the move carries” | Product release, legacy path and decoder choose the import. The new reader writes into staging only. No retroactive rewrite of source files. |

## Message formats

The route registry in `box/src/link/server/router.ts`, Mac relay dispatcher in `box/src/link/mac/relay.ts`, and bridge dispatcher in `box/src/agents/server.ts` are the route indexes. `protocol/vectors/routes/` indexes endpoint examples, routing and response shapes. Stream events are included with their endpoint family. The relay protocol's type byte, the encrypted link type byte, and each mail discriminator are separate from schema versions.

| Family | Messages and baseline reader rules |
| --- | --- |
| WebSocket relay | hello/welcome, send/deliver, unreachable, register, push/pushed, gone. Unmarked binary framing. Deployed relay and Swift use the fixed opcodes and lengths in “The relay”. Unsupported opcodes from relay are skipped. |
| Link framing and handshake | handshake 1/2, transport, close, mail. Headers are fixed by opcode. Session hello has `v:1` and Noise IK prologue v1; host closes an unsupported hello. |
| Encrypted session messages | req/res/data/cancel/ping/pong and WebSocket stream frames. Inherit established session v1. Unknown heads have no operation. |
| Peer mail and legacy Mac mail | Unmarked JSON inside Noise K mail v1. `peers`, `attention`, `agents`, `ask`, `zone`, `docs`, `phoneSync`, `letter`, `delivered`, `call`, `rows`, `move`, `moving`, `seat`, `seatPart`, `limits`, `workQueue`, `botEdit`, `botEdited`, `release`. Older parsers dispatch on known fields. Piece U stamps all `mailFor` messages and validates authenticated input before dispatch. |
| Signed document payloads | peers, phones, personalities. Signed v1 plus revision `n`. Rejects unknown schemas and preserves signed bytes when forwarding. |
| Document exchange | digest/give/ask/part, phones resync/ack. Was unmarked beyond mail prologue and signed documents. Now inherits peer-mail schema. Readers compare digest, verify parts and authenticate completed documents. |
| Work Queue peer exchange | summaries, multipart, asks/blocks/claims, forwarding/actions/acks, handoff records and receipts. Was unmarked beyond mail prologue. Now inherits peer-mail schema. Unknown subkinds have no operation. |
| Agent seat mail | turn/state/result/steer/stop/drop/work/held/report/memory via `seat`, including reassembled parts. Was unmarked beyond outer mail. Now inherits peer-mail schema. Receivers dispatch by seat kind. |
| Usage and Insights peer mail | limits, plan reset, usage history/settings, Insights pull/page. Was unmarked beyond outer mail. Now inherits peer-mail schema. Observation/cursor/epoch and edit stamps track data separately. |
| Fleet release mail | want/metadata/none/part. Was unmarked beyond outer mail. Now inherits peer-mail schema. Embedded release manifest verifies `box/1` and signature before taking parts. |
| Device push notice | relay/APNs wrapper and Noise K notify payload. APNs wrapper unmarked; notify shares mail prologue v1 and uses existing Swift decoding. |
| Pairing and connection hints | pairing URL/token, setup/rejoin hints in “Pairing” and “Your setup”. Legacy and current URL/JSON layouts coexist through explicit field/route checks. No common version field. |
| Box-owned client route requests and replies | `kuvo/info`, setup/computers/devices/phones/link, session lists/messages/actions/models/queue/asks/work, bot/group/wake/move/project/worktrees/artifacts, settings/usage/limits, sign-in/tools/vault/permissions/sandbox, engine/update/rehome/browser/Git APIs and their NDJSON streams. Unmarked shapes, capability names and legacy aliases gate use. Piece U puts one version on routed request/reply bodies for HTTP, session transport and SSH. |
| Agents bridge HTTP API | direct `api/{info,event,session,integration,kuvo,...}` including local peer spool/ACK/gave-up/directory calls. Previously unmarked payloads; routed via Box or served directly by the bridge. Piece U stamps authenticated replies and refuses explicit unknown request versions. |
| Upstream engine and tools APIs | forwarded `api/*`, event feeds, tool relay, engine WebSocket. Underlying APIs depend on engine features/protocol; the outer Box route version covers its dispatch and relay response, and does not version provider JSON independently. |
| Mac host service | authenticated hello, link/identity, link/route and host/keeper requests in “This Mac's box”. Proof context and known routes, unmarked payloads; old host continues to receive legacy shapes. |
| Browser keeper and streaming sockets | CDP, `Kuvo.hello` plus browser view/WebSocket and binary media. Build fingerprint and existing path/opcode identify the format. Socket upgrades bypass the ordinary route marker; browser data endpoints go through it. |
| Vault broker / local agent sockets | unlock, grants, mediated use and SSH agent protocol. Separate local socket protocols and legacy shape checks; no common route schema for those sockets. |
| SSH and keeper command channels | setup, pair, seed, repair/install, tool commands, box route stdin/stdout. Fixed command-specific shapes and older fallbacks. Piece U versions `box route` replies and accepts an optional version header on stdin. |

## Counts and upgrade rules

- 50 stored families. Fifteen include at least one schema marker or migration history. The other 35 lacked an independent Kuvo schema mark or rely on an external format. Several of the 15 contain companion files that are unmarked.
- 19 message families. Three already had an explicit schema version or inherited a negotiated one (hello/framing, session messages, signed documents). The other 16 lacked an independent payload mark, even where the encryption prologue was versioned. Payloads within a family are listed in the route, mail and socket registries above.
- Piece U adds independent versions to three stored formats (`link.json`, `numbers.json`, Insights) and two wire contracts (peer mail and route bodies, including direct bridge HTTP). It wraps the legacy desktop import and aligns cached signed-document validation.
- Future readers add a tested step at index n for n→n+1. JSON changes run in memory on read, then persist using the owner's atomic locked write. SQL changes commit with the schema mark in the same transaction. Neither runs an upgrade function for a version ahead of the reader.
- Keep the initial formats additive while pre-version boxes remain. They can consume v1 and may omit the stamp if they rewrite a file. Before sending a genuinely incompatible message or writing a format a rollback build cannot read, teach neighbouring builds how to accept it or refuse it and negotiate the writer version. Use signed document versions, a new advertised capability or separately deployed relay opcodes as appropriate.

## Remaining rollouts

The audit groups aliases, dynamic files and routes by ownership and encoding. Legacy layout selection,
third-party schemas, unchanged lock files and opaque bytes are recorded so they can be considered on
their own terms. The new writer/reader adapters currently cover the formats listed above. Untouched
durable formats and file writers, including the Work Queue item store and move journals, still need a
per-owner migration audit. A binary relay change needs the relay deployment and neighbouring client
readers. Existing mobile and desktop callers can continue to omit the route header; their request
writers and UI for incompatibility have not been changed.

For #922, surface the format name and supported version in update/recovery status. Keep the existing
file and show which peer did not accept a message, with retry or upgrade available. Route consumers
can use `unsupportedFormatVersion`; boxes currently log refused mail locally.
