# Forwarded decisions, recovery and version boundaries

This records the changes for package 10 of the architecture audit pinned at `15fe20141`. The starting local main was `98d2c601447cd92972edc98c796d17cf6ffe5b0e`.

## Which computer keeps the evidence

| Writer | Durable record | Capture, move and recovery |
| --- | --- | --- |
| Forwarding Box | Origin fence keyed by setup and device operation. The digest binds the intended owner, submitter and normalized intent. The record also names the forwarding Box, area and addressed record. | Machine-local. A retry through that Box stays pinned to its original destination. Capture excludes the canonical origin record and its pre-package-10 compatibility shadow. |
| Accepting service | A pending claim or terminal outcome keyed by setup and device operation. It records the accepting owner, submitter, domain address, digest, response and time where available. | The declaration follows the adapter. Rename follows setup names, device revocation follows membership, and item, session and group decisions reference their respective containers. Computer-local operations retain computer authority. |
| Typed draft owner | Original-owner submission evidence under the immutable submission handle and session. The legacy draft slot stores a replay fence. | The original authority serves outcome lookup after a move. An accepted replacement can recover the historical decision with the original key still retired. Whole moves carry the legacy draft fence so an old draft route cannot execute the same scoped draft at the destination. Listing evidence and pending draft delivery stay local. |
| Older generic mutation writer | A hash slot with a digest, an optional outcome and no recoverable domain address. | Recover as opaque legacy mutation evidence. A matching old origin record identifies its shadow. A historical archive with only the hash and digest cannot establish provenance and has no authority to select a domain, move, or retry an effect. |

The `mutations` slot and additive format 1 remain readable by the previous release. The backup inventory classifies known records by their writer metadata, including old shadows visible beside the canonical origin records. Restore checks versions before placement. It carries decisions at the approved replacement only under matching setup and domain authority, and retains out-of-scope decisions among held recovery files.

Moves carrying evidence use manifest version 3. The source checks the destination capability, and older manifest readers can refuse before importing. An individual queue handoff with evidence uses the distinct `evidenceHand` message. Its sender waits for a capable reader. These paths make the replay fences durable before acknowledging the held move or activating an item. A fence can survive a canceled move. It can answer a known effect or preserve ambiguity. Ownership and authorization still come from the receiving adapter. An independent conflicting decision blocks arrival.

A scoped reply at a new owner compares the exact intent against the accepting owner's recorded digest. An unanswered claim stays ambiguous. For a checkpointed rename, the shared save carries a deterministic operation key into the accepted setup checkpoint. If the new primary can prove only that the operation was claimed or committed under a previous epoch, it refuses a repeat and leaves the name alone. The original authority can still answer from its receipt. Direct paired queue and rename routes consult the durable decision when supplied with the stable operation ID.

## Reader boundaries

The [priority 1 first-reader inventory](authority-first-readers.md) records the subsequent command, recovery and browser version checks and the remaining rollout boundaries.

The [storage and message inventory](stored-formats.md) enumerates the existing file families. Folder versions cover the on-disk layout. The first effectful reader for a socket, command, signed envelope or archive must check its own contract.

| Kuvo-owned boundary | First reader and contract | Verification or remaining work |
| --- | --- | --- |
| Raw SSH route | `box route` parses the route version header before dispatch. Unmarked input uses the deployed contract. | `format-versions.test.js` sends both legacy and future stdin. |
| Addressed SSH and resident pipe | `route-to` authenticates the addressed box and device with Noise. On a resident release both SSH verbs pass through the private `server/ssh-pipe.ts` socket to shared dispatch. The original route header and reply framing still apply. | Preserve the separate OS-account authority of `route`. See [resident launch](one-client-process.md). |
| Raw SSH seed | `takeSeed` checks additive v1 or legacy JSON before writing keys, integrations or engine credentials. | `add-seed.test.ts` verifies legacy and v1 plus future refusal before effects. Setup tokens, seed-settings/install stdin, repair and keeper command output still need independent negotiated contracts. |
| Native Mac host and helper identity | Helper and retired app HTTP dispatch accept legacy and `X-Kuvo-Host-Version: 1`. Box checks the authenticated hello response before sending the actual call. Keychain rotation state independently checks its own v1. | Helper fixture sends future and v1 identity requests. Installed mixed-helper/Keychain rehearsal remains unverified. |
| Vault holder | The server checks the keyed hello and request before routing. The client checks the hello before sending a request and checks the answer before returning it to the caller. V1 preserves the deployed JSON lines. | `settings-vault.test.ts` exercises a legacy hello and future rejection. OpenSSH agent wire messages retain the OpenSSH protocol. |
| Box routes and bridge HTTP | Box routing and bridge dispatch check `X-Kuvo-Route-Version`; Box rechecks the new bridge at a handover. | `forward-version.test.ts` covers current/legacy responses and refusal with zero authorized effects at a replacement bridge when the live grant was revoked. Direct non-Box bridge writers and installed legacy clients still need a complete mixed-release check. |
| Forwarded acts and proposals | Authenticated peer-mail v1 precedes Work Queue dispatch. A scoped discriminator is exclusive, including on malformed and future input. Act, grant, owner-message and offline-edit wrappers refuse explicit future versions. | Future spools are kept during the send scan and count as unresolved during removal. A legacy peer lacking scoped forwarding cannot receive a scoped write on its unscoped lane. |
| Move, handoff and restore | Move manifest reader v0–v3, versioned held move journals, peer-mail `evidenceHand`, backup archive authentication, restore layout gate and restore journal v1. | A staged restore validates receipt formats before installing. Raw future receipt bytes remain in the archive. Older moves lack reconstructed addresses for opaque generic claims. |
| Wipe preparation and directory approval | Preparation source and intent readers check legacy/v1. The external directory receipt reader checks v1 before a resume, overwrite or deletion. The installer checks format before using the local link to decide if a wipe is pending. | Stopping approval still binds the old directory identity and cannot detach by itself across rollout or rollback. Old erasing/detached receipts retain their existing authorization and phase rules. An explicit future version blocks recovery. |
| Relay reservation | The upload reader in the receiver namespace adopts an unmarked reservation only when it matches the source and point. It preserves future journals and their partial bytes. | The Relay-only fixture drives a stopped publish, lost commit reply, erase before delayed commit, and namespace separation on real receiver code. |
| Browser keeper and media | Keeper line reader first admits `Kuvo.key` and dispatches control calls; browser-view readers decode control JSON and the length-prefixed JSON/JPEG framing. | The deployed build and opcodes identify the shape. Cross-release negotiation and pre-effect refusal tests remain open. Chromium CDP and JPEG stay on their native formats. |
| Relay framing, notification and pairing | Relay opcode decoder, Noise hello, push notice decoder, and pairing/rejoin token parser are separate boundaries. | Noise v1 and peer-mail v1 already gate their own inputs. Relay framing, push wrappers and pairing hints still need coordinated version selection with the relay and supported clients. |
| Supervision, rendezvous and legacy stored records | Keeper/start/install and helper process readers own their separate signals, journals and pid/port records. Other folder-covered families are named in the linked inventory. | A folder checkpoint and native state-step fields do not supply a general version negotiation. The browser runtime, live keeper/shared scripts, retired Mac import markers and remaining unmarked family records retain the limits in that inventory. |

## Disposable evidence

The named failure bench resolves and verifies the historical build and the recovery neighbor by pinned Git object and content hash. The macOS test runner preserves phase diagnostics in its output. The Relay-only receiver test runs separately because its name is outside the named bench filter. Linux closure requires the corresponding lokum-local run with the same pinned inputs. The package does not certify installed client, native launchd or fleet-wide rolling combinations.

## Draft issuer declaration

These are retained typed-draft and replay-fence contracts. For the current
desktop same-ID send and restart settlement, see [Message delivery](message-delivery.md).

Rule 55 distinguishes a draft outcome's original accepting Box from the conversation's current owner. The outcome address uses the setup and original accepting key. Generic owner resolution follows only the setup's accepted replacement chain and never a session move. Capture and restore check that the submission handle, metadata, and file key agree before using that address. A recognized outcome outside the destination's authority is held. Older bytes without a provable address retain their legacy storage and gain no inferred issuer.

The draft compatibility fence uses the session address. It follows accepted session ownership and remains eligible for move bundles without carrying the outcome. The v3 bundle and distinct handoff discriminator continue to refuse or withhold evidence from old readers. Lookup still checks current viewer access and receipt authorship. A new pairing for the same device can read the historical outcome, while the old pairing and changed execution intent cannot use it to submit again.

The fixtures cover current owner resolution before and after a move, issuer recovery on fresh-key replacement, a moved replay fence, opaque legacy retention, and refusal to execute the recovered submission. The previous-build fixture invokes its original first readers with current v3 and handoff records. Its legacy controls still reach those readers.

## Bounded submission discovery and move evidence

On the disposable priority 3 fixture at `3e0c591b1`, 480 unrelated subjects and 85
requested submissions produced the following measurements. An open includes a missing
file probe for a possible legacy replay fence.

| Operation | Before | Indexed |
|---|---:|---:|
| First and second submission pages | 1,130 receipt opens each, 960 unrelated | 82 each, zero unrelated |
| Move receipt evidence packing | 1,131 opens, 960 unrelated | 86 opens, zero unrelated |
| One silent eligible peer | 21.2 seconds, incomplete with a cursor | 3.1 seconds, incomplete with the cursor held |

The SQLite index stores addresses, not receipt payloads or execution decisions. A page seeks
by setup, session, reader and original issuer. A paired reader's current grant is checked
before seeking. Its stable device key matches previous generations of its own receipts.
The reader reopens canonical records to verify authority, digest and outcome. Each updated
Box selects at most 41 owner keys and 41 listing keys, using the extra key to decide whether
a 40-row page has a successor. A listing key can require checking the archive and active
outbox, for at most 123 canonical open attempts per Box after indexing. Metadata seeks scale
with approved issuers, and file bytes scale with the selected receipts. Exact handles bypass
the index. Move evidence seeks by session, group or item and opens matching mutations and
only the legacy shadow keys that may belong with the moving session.

The index is machine-local and rebuildable. Its entries are outside backup and retention
policy. Canonical writes and restore publication share the receipt writer locks and update
a matching index generation after the file is durable. A cut before the index transaction
commits leaves its previous directory stamp, and the next reader invalidates that generation.
The owner can still publish a terminal receipt if index coordination is unavailable.
An established writer uses an atomic replacement or unlink. An unexpected directory change
starts a new generation. Cold request paths peek at most 129 names per family, and a
maintenance or foreground indexing pass reads at most 128 records and 4 MiB, allowing a
single oversized legacy record to make progress. A large directory's catalogue is enumerated
once per generation by the keeper in yielding chunks. Interrupted catalogue work can
restart or resume scanning from the saved generation. Retired index rows are reclaimed in
bounded maintenance batches. A move holds if required evidence is still indexing or
unreadable. Local discovery may return a partial page until that evidence is available.

Peer fanout has a shared three-second budget, plus local lookup and routing work. Discovery
sends read-only requests over the live Relay when it can, or leaves a request for the
keeper's spool watcher. Any missing responder, incomplete index, rejected provenance,
or issuer change during fanout keeps the response incomplete. Retry the same position in
that case. Current issuers and approved replacement keys are rechecked before emitting a
merged page. Older peers can still supply their forty-row reply, and may perform a legacy
scan on their own machine. Discovery can reconcile an existing matching receipt using
known evidence. It never dispatches the original operation.
