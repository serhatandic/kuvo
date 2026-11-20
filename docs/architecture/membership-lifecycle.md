# Re-enrollment after unlink

An OS account can use ordinary SSH Add or a plain installer code when its Box is
unused, or when a completed unlink retained an unchanged local inventory. The
SSH discovery card reports the same work check used by the Box under the link
lock. The card is a cached hint. The locked check decides admission even if the
account started, changed, or entered another enrollment after discovery.

- Leaving a setup keeps device denials by issuing setup and exact pairing ID.
  Legacy documents with an identifiable enroller can use that issuer as a
  fallback. Older retained rows whose provenance has already been lost stay
  as local, exact-generation denials. They are never published as a new setup's
  revocations. A fresh pairing ID can authorize the same device again.
- New phone documents carry their setup ID in the signed body. A known issuer
  changing setup IDs must explicitly scope subsequent lists. Legacy documents
  from issuers with an unambiguous scope continue to work. Verified denials for
  the former setup remain effective when a member rejoins it. Relay closes an
  open session when its pairing ID is replaced. The old computer
  roster entry and retired key pair remain revoked at the primary.
- A legacy detach may run with a continuing SSH reservation, with a later
  reservation, or with no reservation. A restart between unlink and add can
  bootstrap a standalone setup. None of those transitions turns foreign
  revocation history into independent work. SSH may retain keys for live
  membership refreshes. A new plain-code attempt on a machine-layout Box
  persists fresh credentials for that code, including when an older detach
  left no evidence on disk. Retrying the same claim reuses those keys.
- Active device grants, pending saves and deliveries, and allocated number
  ranges require separate handling. An independent setup with local work uses
  Merge Setups. SSH still requires explicit confirmation before discarding
  unmatched local pairings when refreshing the same membership. A retained
  session, bot, note, or library edit after unlink requires a merge. Unreadable
  authoritative stores fail closed. Older release records without a baseline
  require a merge if local work exists.

The unlink baseline records file identity, size, and change time for managed
local work. It detects deletion and in-place edits without reading the full
contents of every attachment. Uncertain reads and nonregular files can require
a merge. SQLite checkpoints and equivalent rewrites can also conservatively
require a merge. External project trees are outside this admission inventory.

The disposable failure bench exercises real Box setup commands with signed
legacy-shaped documents, as well as a full installer-code exchange and committed
unlink over the stand-in relay. The historical beta binary and a physical
restart are separate checks. The restart fixture runs the production startup
initialization in a new process and keeps the same on-disk account.

Two boundaries remain. A Mac whose membership keys are supplied by the native
Keychain host still needs a host-backed rotation path to return with installer
code after its old keys are retired. The primary refuses those retired keys.
If local work changes after the installer confirms its claim but before it
installs the accepted roster, the final work check refuses local adoption.
The primary may already show that enrollment. The entry can be removed there.
The outstanding installer claim can still fence local setup operations until
expiry. Merge into the same former setup also checks for retired keys, which
may require a different identity. A transaction spanning remote acceptance
and local work writers would require a separate protocol change.
