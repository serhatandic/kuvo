# Ordinary offline notes

Rule 39 reproduced on `56ef8c049`. With base “Start.” and drafts “Start. Alpha.”
and “Start. Beta.”, accepted text held only the first arrival. The second had to
be chosen manually. Two identical drafts also produced one insertion.

| Record | Owner and address | Existing data |
| --- | --- | --- |
| Ordinary session text note | Session computer, `sessionNotes` with session `id` and `note` ID | New versioned records under `run/work/notes/<session>.ordinary.json`. Older files in the notes directory are left in place. |
| Work Queue note | Current item owner, `queueItems` with item ID and queue address, field `note:<id>` | Stored in the item. First revision-aware read durably assigns a stable ID to a legacy note, using its original item, position, author, time and text. |
| Work Queue description | Current item owner, the item's `body` field | Same item and edit history as before. |

The serving Box queues the exact proposal under its setup and operation ID. The
owner reloads current state and checks membership, current ownership and device
authority before a new write. A completed operation can replay its saved receipt.
Session note receipts and accepted text share one durable write under the session
store's exclusive lock. Work Queue receipts share the item's transaction. An
already queued legacy operation keeps its original reply key. The Work Queue
owner records and replays the equivalent setup-wide key, including when it
recognizes a legacy receipt from a known setup member. It also retains the
original receipt for a legacy retry across rollback or a queue-only move. New proposals use the
setup-wide key even when the lister has yet to hear the owner's capabilities.

The text merge records the accepted hunks relative to the proposal's base, with
operation IDs on insertion pieces. It reuses that geometry while it renders
the current text, including when adjacent whitespace or repeated words make a
later diff ambiguous. If an intervening edit changes a disjoint passage against
a newer base, the owner maps that change back to the original base and keeps the
existing insertion anchors. Distinct IDs keep both equal insertions. Same-anchor pieces
sort by their wording, with exact whitespace and punctuation preserved. Equal
wording from history without edit provenance is kept once. For a competing rewrite
the first accepted wording and the competing alternative each retain the other
disjoint changes. An alternative that would exceed the record's text limit stays
in draft recovery. A conservative replacement hunk bounds the work when the
diff matrix would exceed one million cells.

Each record retains at most 32 base snapshots under the existing 256 KiB history
budget, including hunk provenance. A single large head stays readable. Retired
base drafts can be recovered from the serving Box after a restart. Deleting a
session note compacts its snapshots and choices to a tombstone with its receipts.
The Work Queue uses its existing item deletion and compact fence. Deleting an
entire engine or bridge session is a separate operation. If its backend cannot
confirm that session, its note route holds the edit for the owner.

The session note file is selected by the backup inventory under the session's
ownership fence. A notes-bearing Move to Computer bundle uses manifest version 2.
The receiver keeps it held until the existing ownership switch, and rollback
removes only note files installed for that exact move bundle. The source compares
note state before switching and retires its own copy after the destination has
acknowledged. A bundle without session notes keeps version 1.

Ordinary proposals carry plain text and use the paired-client boundary. Engine
metadata and permissions continue through the session writer and local approval
routes. Bot-home memory and legacy bot notes stay with the bot home. Project tools
and vault access keep their existing entry points.
