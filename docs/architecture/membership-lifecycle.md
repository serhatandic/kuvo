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

The unlink baseline keeps the existing file identity, size, and change-time
stamp for managed work. Current Boxes also record a logical engine SQLite
digest when they can verify the committed contents. A WAL checkpoint, row
reordering, or change to SQLite planner statistics can pass admission if the
schema, stored values, and rest of the work inventory still match. The Box
uses the physical stamp first and rechecks it after a logical comparison.
The scan is limited to 64 MiB of engine databases and journal files combined, with
a 1.5-second row and table scan budget. A change that exceeds that budget,
uncertain reads, nonregular files, and changed databases recorded by preceding
Boxes can still require a merge. Non-database files use the physical stamp,
and external project trees are outside this admission inventory.

The disposable failure bench exercises real Box setup commands with signed
legacy-shaped documents, as well as a full installer-code exchange and committed
unlink over the stand-in relay. The historical beta binary and a physical
restart are separate checks. The restart fixture runs the production startup
initialization in a new process and keeps the same on-disk account.

A Mac with a rotation-capable helper journals each new plain-code
rotation before asking the authenticated host. The host publishes an entire new
identity in one Keychain update scoped to that helper. The original public key
and installer attempt identify a retry, even if the host saved the keys and the
reply was lost. The Box stores the journal and public setup identity. Fresh private
keys stay in the helper’s Keychain and the Box’s host overlay.

An independent edit detected after the installer confirmed its claim keeps the
local setup and cancels the provisional enrollment. While the original issuer
can reconcile the live invitation, retry the original code. The issuer checks
the complete peer identity and its accepted enrollment operation before
retiring that key pair through a membership save. The applicant clears the
live cancellation journal once the issuer confirms the exact retirement or
verifies that the invitation added no computer.

With a compatible issuer, plain-code confirmation puts a commitment to a
random cancellation secret in the accepted membership for the exact setup,
invitation, computer key, signing key, and Relay address. The applicant keeps
the secret locally and erases it when it adopts the setup. The issuer removes
the commitment through an accepted save before acknowledging activation.
Receipt cleanup sends no unlink or wipe instruction and preserves the roster
slot, the retired signing key, and existing setup and pairing revocations.

When the issuer is gone or the claim has expired, run the installed Box’s
`kuvo-link.js installer-recovery issue` on the applicant under its Box
environment. A refused, expired, or locally edited claim can produce a
receipt. Run `kuvo-link.js installer-recovery apply` on the setup’s current
main computer and paste the receipt at the prompt. Apply archived receipts
one line at a time. The command contacts the authenticated local Box and
reports retirement after the membership save commits. The new primary still
needs the authority and standby required for ordinary roster writes. An entry
awaiting installer activation cannot be selected as standby and its absence
cannot block an eligible computer from taking that role. An adopted member
keeps its installer retry journal without carrying an enrollment fence into
a later promotion. The original deadline controls the unadopted applicant’s
fence, and a later enrollment keeps an abandoned receipt in the local journal
without extending it.

Older claims and authority chains that lose the commitment through a
preceding Box require the usual administrative removal. Issuer loss before the
portable activation save leaves a commitment on the roster. The adopted Box
has erased its secret. A disk restored to its pre-adoption state can still
have that secret, so treat exported receipts and pre-adoption copies of the
claim journal as sensitive until activation or retirement commits.

An older helper needs an update before it can rotate retired keys. Rolling
back to a helper that predates the atomic identity format holds link access
until a compatible helper runs. Moving the Box back to the old app expands the
identity into its three historical Keychain items.

## Native evidence for audit package 9

The current helper’s repository fixture passed three checks using a disposable
macOS Keychain, an ad hoc helper with the test bundle ID, and a stand-in
keeper. It exercised atomic identity rotation, same-attempt retries across
helper restart, refusal of missing or unreadable state, restoration of the
three historical identity items on move-back, and access to migrated Keychain
items.

A separate scratch probe built the pre-rotation helper at
`068f355a80ec84f3c37db6fdd93fe503753bdab5` and the current helper from
`4a1ca192fb76b66f03cfecf721e1c45e398f393c` with test bundle IDs. On the same
disposable Keychain the preceding helper read the original three keys and
returned 404 for rotation. After the current helper rotated them, the
preceding helper returned 503 for identity. Restarting the current helper
returned the same committed keys for the same attempt.

The separate rehearsal login had an existing setup and running engine
processes. The locally archived beta.2 real-ID helper passed static signature
verification and was Apple Development-signed. Installed current and
supported preceding release-helper runs, real launchd during a clean
rehearsal, reboot between legacy release and re-add over SSH and plain code,
and deliberate loss of a native HTTP rotation reply remain unverified. The
Box’s relay stand-in exercises reply loss and setup recovery separately.
Native end-to-end credential opt-in and unlink cleanup with fresh provider
sign-in, real OS fingerprint ambiguity, and the native variants of the
retained-work and grant cases still need a clean Mac login or VM.
