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
