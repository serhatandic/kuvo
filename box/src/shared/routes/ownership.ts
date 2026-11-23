import { any, bool, list, nullable, number, object, oneOf, string, words, type Infer, type RouteShape } from "../shape";

const REFUSED = object({ message: string });
const OWNER = object({
  key: nullable(string), name: string, local: bool, reachable: bool,
  scope: words("primary", "session computer", "group host", "bot home", "local computer", "client"),
});
const READ = object({
  owner: OWNER, stale: bool, state: words("received", "absent", "neverReceived"),
  refresh: words("live", "unavailable", "failed", "withheld", "metadataOnly"),
  lastReceivedAt: nullable(number), "data?": any,
  unavailable: object({ files: list(string), messages: list(string) }),
});
const SUBMISSION = { "session?": string, "submission?": string, "digest?": string } as const;
const PENDING = object({ status: words("pending"), operation: string, owner: string, message: string, ...SUBMISSION });
const COMMITTED = object({ status: words("committed"), operation: string, owner: string, acceptedAt: number, ...SUBMISSION });
const INTERRUPTED = object({ status: words("interrupted"), operation: string, owner: string, message: string, ...SUBMISSION });
const RECEIPT = oneOf(PENDING, COMMITTED, INTERRUPTED);

const CHOICE = object({ id: string, field: string, base: any, accepted: any, proposed: any });
const EDIT_VIEW = object({ revision: string, fields: any, deleted: bool, choices: list(CHOICE), owner: string, stale: bool });
const EDIT_WAIT = object({ operation: string, status: words("pending", "interrupted"), message: string });
const EDIT_DECISION = object({ operation: string, status: words("committed", "choice", "deleted", "refresh"), revision: string, choices: list(string) });
const EDIT_CAPACITY = object({ operation: string, status: words("capacity"), message: string });
const EDIT_RECEIPT = oneOf(EDIT_WAIT, EDIT_DECISION, EDIT_CAPACITY);
const EDIT_ATTACHMENT = object({ id: string, file: any });
const EDIT_BODY = object({ operation: string, ref: any, baseRevision: nullable(string), "fields?": any, "create?": bool,
  "delete?": bool, "resolve?": object({ id: string, use: words("accepted", "proposed") }),
  "notes?": list(object({ id: string, text: string })), "attachments?": list(EDIT_ATTACHMENT) });

const NOTE_EDIT_BODY = object({ "operation?": string, baseRevision: nullable(string), "fields?": object({ text: string }),
  "create?": bool, "resolve?": object({ id: string, use: words("accepted", "proposed") }), "computer?": string });
export const OFFLINE_EDIT_ANSWERS = { 200: EDIT_DECISION, 202: EDIT_WAIT, 400: REFUSED, 403: REFUSED, 409: oneOf(EDIT_RECEIPT, REFUSED), 413: oneOf(EDIT_CAPACITY, REFUSED), 503: REFUSED };
export type MessageReceipt = Infer<typeof RECEIPT>;
export const OWNERSHIP_ROUTES: RouteShape[] = [
  // A session's text notes as their owner has them: the active note ids, revision and choices.
  { route: "GET kuvo/sessions/:session/notes", feature: "data.session-notes", answers: { 200: EDIT_VIEW, 400: REFUSED, 403: REFUSED, 409: REFUSED, 503: REFUSED } },
  // One note's text, revision, open choices and whether it was deleted.
  { route: "GET kuvo/sessions/:session/notes/:note", feature: "data.session-notes", answers: { 200: EDIT_VIEW, 400: REFUSED, 403: REFUSED, 409: REFUSED, 503: REFUSED } },
  // Create or edit a note against the revision read (an offline edit); 202: saved here.
  { route: "PATCH kuvo/sessions/:session/notes/:note", feature: "data.session-notes", body: NOTE_EDIT_BODY, answers: OFFLINE_EDIT_ANSWERS },
  // Delete a note at a known revision; its id is never used again.
  { route: "DELETE kuvo/sessions/:session/notes/:note", feature: "data.session-notes", body: NOTE_EDIT_BODY, answers: OFFLINE_EDIT_ANSWERS },
  // The owner's view of a record for offline editing: fields, revision, choices, deleted.
  { route: "GET kuvo/data/edits", feature: "data.offline-edits", answers: { 200: EDIT_VIEW, 400: REFUSED, 403: REFUSED, 409: REFUSED, 503: REFUSED } },
  // Propose an edit against `baseRevision`, changed fields only; 202: saved here, poll its receipt.
  { route: "POST kuvo/data/edits", feature: "data.offline-edits", body: EDIT_BODY, answers: { 200: EDIT_DECISION, 202: EDIT_WAIT, 400: REFUSED, 403: REFUSED, 409: oneOf(EDIT_RECEIPT, REFUSED), 413: oneOf(EDIT_CAPACITY, REFUSED), 503: REFUSED } },
  // Edits pending on this box, with text recoverable after a restart.
  { route: "GET kuvo/data/edits/outbox", feature: "data.offline-edits", answers: { 200: object({ entries: list(object({ ref: any, owner: string, receipt: EDIT_RECEIPT })) }), 403: REFUSED, 503: REFUSED } },
  // An edit's receipt (committed, choice, deleted, refresh, pending) and its recoverable text.
  { route: "GET kuvo/data/edits/:operation", feature: "data.offline-edits", answers: { 200: object({ receipt: EDIT_RECEIPT, recovery: any, "attachments?": list(EDIT_ATTACHMENT) }), 403: REFUSED, 404: REFUSED, 503: REFUSED } },
  // Who owns a record, and this box's last observation of it (received, absent, never received).
  { route: "GET kuvo/data/owner", feature: "data.ownership", answers: { 200: READ, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: REFUSED } },
  // Send to a conversation whose owner is elsewhere: saved here before 202, delivered by its owner.
  { route: "POST kuvo/data/messages", feature: "data.ownership", answers: { 200: COMMITTED, 202: PENDING, 400: REFUSED, 403: REFUSED, 409: oneOf(INTERRUPTED, REFUSED), 503: REFUSED } },
  // A message receipt kept on this box: pending, committed (`acceptedAt`) or interrupted.
  { route: "GET kuvo/data/messages/:operation", feature: "data.ownership", answers: { 200: RECEIPT, 403: REFUSED, 404: REFUSED } },
];
export const OWNER_MESSAGE_PENDING = PENDING;
export const OWNER_MESSAGE_INTERRUPTED = INTERRUPTED;
export const OWNER_MESSAGE_COMMITTED = COMMITTED;
