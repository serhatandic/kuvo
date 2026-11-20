# Deferred operations and computer action receipts

Revision-aware phone and desktop edits have the same operation routes and owner checks. A base
revision makes an ordinary Work Queue, bot, clock, personality or label route a proposal under the
same contract as `kuvo/data/edits`. The allowed kinds and editable fields come from the owner
adapters. No ordinary edit can grant account access, unlock a vault, or change membership or
execution permissions. Release selection still requires the local holder and a verified release
manifest. Standby and handover accept a paired device at a current setup member. The primary's
existing lease, membership and acknowledgement checks control those changes. Backup status is a
separate read whose response is safe to send to the paired client on either transport. Backup
configuration and recovery still use the local authenticated app and the existing vault grant.

A serving Box persists device key and pairing generation with deferred work, alongside the
existing operation identity and address. The owner checks the grant again before a fresh write or
native side effect. A caught-up source that sees a rejection can request only replay. A member
owner waits for fresh access after reconnect before using a cached grant. A committed operation
or an ambiguous native claim keeps its previous outcome. Message owners can replay a recorded
outcome even when the conversation is unavailable. Authorization for setup changes
is checked before the primary stages the proposal. A staged proposal follows standby
acknowledgement and epoch fencing. Splits still follow the protocol's accepted partition policy.
A bot model change that moves a conversation between engines needs a live call to its owner.
The migration has side effects beyond the queued edit receipt.

New envelopes have separate peer and pending-record fields so an older Box cannot dispatch them
under its earlier rules. Existing completed receipts keep their IDs. Old untracked message and
edit proposals can recover outcomes but cannot newly execute at an updated owner. The older
held-bot adapter also includes provenance in its authenticated record. It combines a bot's held
fields only under the same grant, and keeps a denied draft visible. An unattributed held-bot
record from a managed setup is left visible for review. Pre-managed Box-to-Box bot queues retain
their earlier machine authority for compatibility.

The listing Box issues or forwards rename and can recover the signed receipt without submitting a
second rename. Rust callers can retain the receipt through 202 and structured 409 and distinguish
those statuses from a refusal. Standby and handover expose the Box's existing position and HTTP
result through Rust. Merge, credential selection and connection preferences already have Rust
entry points. The immutable last-wipe receipt and removal plan are owned by the removal workstream.

## Follow-up scope

- The generic short-lived peer action spool still uses the sending Box's authority. A protocol
  revision should carry per-initiator authority on actions that can wait there, including legacy
  unrevisioned shared-data and setup actions, and cover proof-aware receipt queries after leadership
  changes. This change covers the explicitly deferred message, edit and bot outboxes.
- The locked desktop views and parked phone app still need their actual action and receipt
  journeys when those workstreams resume. The typed composer still has a Rust plain-message
  shortcut and client interpretation of the Box's answer. Transport selection and first-install
  bootstrap stay client-local.
- Legacy peers that have not upgraded can still act on envelopes sent using their original
  protocol. Current peers hold new device-aware work until the destination supports its checks.

## Remaining relay admission decisions under rules 17, 31 and 37

The generic forwarder covers live requests that can outlast a connection. `forwardAct` issues a
transport ID, writes the request to the source spool, and can resend it until its deadline.
`takeAct` authenticates the member Box, then uses the sender and request ID for its durable claim
and outcome. A received write can therefore cross a reconnect. Direct revisionless Work Queue
edits, setup and computer actions, and device-management calls share this path. An initiating
device grant is absent from those envelopes. The revisionless owner-write dispatcher and the
`kuvo/data/owner` read still have desktop-only branches. Rule 17 needs route-by-route parity
there after the adapters can carry the right authority. A new transport ID for a new attempt also does not
identify a previously claimed domain operation.

Carry authenticated provenance for new device-initiated mutations at the specific route that
knows the action. Use the same setup and pairing-generation checks as deferred edits, with a
separate wire discriminator so an older owner cannot ignore them. The source forwards provenance
derived from the authenticated request. The receiving adapter rechecks its operation's authority
alongside ownership immediately before its first effect, including after a wait on an engine or
standby. A recorded signed outcome or durable claim remains queryable after revocation. A retry after an
unfinished native claim needs a service-specific recovery rule. An old envelope with unknown provenance can recover a known result where supported.
The source can retain rejected edits for the caller to recover and obtain a new operation ID for
an explicit retry. Transport IDs continue to correlate packets. Durable domain IDs and result
retention belong to each operation.

Classify the adapter by operation before applying a device rule. A preparation-status request
uses a peer GET and can advance source preparation. A queue-agent call carries conversation
provenance validated by its submitting Box. The receiver trusts that assertion from a member Box
and applies the queue's own policy. Reads, autonomous Box work, installer claims, and final
removal cleanup have distinct actors. A remote request may
start an operation that still requires a separate local confirmation. Keep the detached backup
worker's exact-request spool proof and the Relay archive adapter's membership, reservation and
replay checks. Coordinate membership, credential and removal flows with their owners before
changing those adapter contracts.

A closing test should drop a forwarded write before receipt, revoke the initiating grant, and
reconnect a stale receiver while withholding fresh membership. Check zero fresh executions,
then exercise the same operation after it had already committed with its reply lost. Check
one outcome across restart and a changed request with the same operation ID. Repeat with an
older peer, an agent tool call and Relay backup retry. The existing durable-claim regression
proves refusal when its journal cannot be written. It does not exercise device revocation
in this generic lane.

## Remaining typed composition decisions under rules 17, 37 and 57

The Box already decides model availability, commands and keyword effects in `typedRoute` and
`typed/decide.ts`. The Rust client still skips that route for ordinary text. It synthesizes a
restore or side-thread response using the last model-list refusal and the local side tag.
The desktop composer also uses those cached facts to prevent an existing-session send and to
decide which draft appears optimistically. Those decisions can disagree with the Box after a
model or owner change. The current typed route also caches successful answers after doing its
step, by conversation and send ID, within a bounded window. This covers a lost reply after
the answer was recorded. A crash between the step and the receipt, or reusing an ID with
changed text, is a different case. The separate native command and shell calls currently
lack that typed receipt identity.

Version a full draft submission for the owner's decision path, regardless of prefix or client
kind. Keep the current typed request available during the client transition. Carry the stable
send ID, raw text, delivery intent, side target, reviewer, and the existing quote, attachment and
annotation inputs. Reuse owner adapters for accepted effects so the result names the message
or operation that was created. After a lost reply, retry or poll the same operation. Keep local draft editing, optimistic rendering, quotes, attachment collection,
and keyword highlighting as presentation. Initial discovery and session creation keep their
current roles. Transport can select SSH or Relay without deciding what the draft means.
The owner reads current facts and returns an explicit send, restore, side-thread, or execution
outcome. Adapters that can cause effects need a claim bound to the normalized draft and stable
operation ID, an owner fence, and a replayable result. An ambiguous native claim requires
reconciliation. Commands and shell operations keep live admission and must not acquire offline
delivery through a generic retry.

Existing Box typed tests cover plain text, unready models, command lookup, side questions,
lost replies and recorded answers after restart. The Rust vectors currently check that the
local shortcut matches the shared examples, so they characterize the duplication. A closing
integration test will change the model's readiness between listing and sending, then submit
the same draft with equivalent desktop and phone identities. It should compare the owner
decisions for plain text, side-thread text, slash and shell forms, assert correct quote and file
handling, and suspend or crash the owner at the native claim/receipt boundary. Keep the desktop
and phone screen work with their existing projects.

### Scoped peer forwarding checkpoint

Paired client mutations now enter a capability-gated envelope through the Work Queue,
revisionless owner-write, rename, leadership, device and computer action routes. The serving Box
seals the exact scoped spool record and pins an explicit client operation to its intent and
destination. Delivery IDs still correlate responses. The owner uses the existing durable
mutation ledger for a scoped claim and checks authority before admitting new work. Scoped records
retain receipts in that ledger so an old operation ID cannot silently execute again when a
transport receipt expires. A legacy client that supplies no operation can replay delivery within
the request, but needs to reconcile an ambiguous reply before issuing another operation.

Specialized receipts remain authoritative for setup and removal. A signed removal-status lookup
is a separate transport intent and reaches the removal adapter instead of reusing its mutation
digest. Pre-existing unscoped mail still points to a member Box, and old senders retain their
legacy contract. An updated serving Box rejects a paired-device write to an owner that does not
advertise scoped forwarding. A receipt stays at its accepting Box unless the service already
replicates it. The submitting Box fences retries that resolve to a different owner.

### Owner draft checkpoint

`POST kuvo/sessions/:id/draft` is advertised as `typed.draft`. The desktop transport opts in
before submitting and carries the raw text separately from the local quote rendering. The
owner decides against the current conversation and performs the action. It reuses the same
native adapters as the established routes. Existing clients can continue using the earlier
`/typed` decision route. The app uses its cached model list for presentation and initial
discovery. It does not veto an existing-session draft on that basis.

The owner uses one digest-bound draft ledger for direct and queued submission. An ordinary
draft can wait in the existing conversation outbox. The disk and peer envelopes have
draft-specific fields, so an older Box cannot deliver the draft as a plain prompt. Keyword,
shell, side-target and Send Now submissions require the owner live. The new live transport does not opt into automatic
retransmission. A claim with no proven outcome is explicitly ambiguous and never authorizes
a fresh execution. A confirmed earlier effect in a compound draft prevents restoration of the
original complete draft. Older desktop Box versions retain the earlier client behavior.
A queued receipt can be queried on the listing Box. The desktop follows it in the existing
outgoing row. Interruption or loss of that receipt keeps the visible draft and its attachments
available for reconciliation without offering a new send. The outgoing presentation is local
to the open conversation. A move may interrupt a listing receipt before a lost answer from the
former owner arrives. The listing Box keeps that operation pinned to the original submission.
This checkpoint leaves the parked side thread and mobile client work
with their existing projects.

## Submission evidence after an owner change

The `typed.submission-recovery` feature adds `POST kuvo/sessions/:id/submit` and
`GET kuvo/sessions/:id/submissions[/:submission]`. An eligible client reads the current scope
with `?scope=1` before its first send. The submission handle binds the setup, owner key and
client's stable message ID. The client fixes the route and handle for the life of that draft,
including its clones. The owner binds the complete request and exact submitting grant to a
digest. A computer-local grant also binds the Box it came through. A caller that loses the
reply can read the same handle, optionally qualifying the read with its digest.

A lister forwards ordinary work only to the pinned owner while that owner can still admit it.
The owner records its claim before invoking native effects. An unfinished claim fences retries
and is readable as unknown. A proven pre-execution refusal is distinct. A retryable
non-admission frees only that exact digest for retry on its original authority. Where a lister
still holds such an operation for automatic delivery, recovery reports the pending delivery.
Commands, side work and Send Now stay live. During supported rollback, the owner retains a
compatibility fence in the legacy draft ledger before creating the new claim. Fresh legacy
clients still use the older route. The desktop allows each unpinned draft one attempt, since
it cannot prove the earlier owner on a retry. Legacy records cannot be discovered retroactively without submission
metadata.

Handoff changes the current owner in the lookup, while the original owner keeps answering for
its own claims. A currently enrolled peer can forward a read to that authority or its accepted
replacement. The lookup verifies who supplied owner evidence and matches the original
submitter to the current reader. A renewed pairing for the same device can read the original
generation's history. A different generation cannot admit its old operation. A listing Box
can also report the receipt it saved under the original digest. Conflicting digests remain
visible, and recovery advances an interrupted receipt only using a matching authoritative
acceptance and timestamp. A reply arriving after owner change can advance the same archived
receipt. It cannot enqueue its draft.

Owner evidence is part of the normal encrypted backup inventory. An accepted replacement can
answer with the restored original identity and acceptance time. The restored receipt grants
no execution authority. If every copy of the result is unavailable, the API reports unknown
and the client reconciles against the conversation. A submission that never reached a Box,
or a pending-only lister that lost its disk before delivery, has no surviving draft record
to discover. The device APIs and desktop library can discover any saved evidence after a
relaunch. The installed screen work stays with its presentation owner.

The delivery loop scans the active outbox. New terminal records move to a separate receipt
directory, freeing the active slot. A cut between archiving and unlinking is recovered by
preferring the archived receipt and removing the shadowed active copy. Settled owner claims
keep the answer, timestamp, handle, digest and original grant. Accepted drafts drop redundant
recovery payloads. Interrupted submissions retain text and attachment data or references
under the submitting device's current access. Large peer responses give a bounded outcome
and name the computer where the client can read the full recovery payload. Evidence does
not expire automatically. Future deletion needs a retained identity fence or a versioned
admission floor before it can discard an old record. Machine-local lister receipts and
forwarding-origin pins stay separate from portable submission evidence. The forwarding pin
keeps its legacy compatibility fence until prior keepers stop accepting that wire contract.
