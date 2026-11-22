// What's written in a conversation's composer, sent (PROTOCOL.md, "What a typed message means").

import { any, bool, type Infer, list, oneOf, nullable, number, object, type RouteShape, string, words } from "../shape";
import { OWNER_MESSAGE_PENDING } from "./ownership";
import { MODEL_ACTION } from "./sessions";

/** What "/" offers, as 0.7.x clients ask it: nothing (Kuvo has no typed keywords or commands). */
const TYPED_OFFERS = object({ keywords: list(any), commands: list(any) });

/** What goes back to the composer. */
const TYPED_RESTORE = object({ text: string });

/** What a typed text meant: `send` (what was written, as a message; the composer's files and quotes
 * go with it), or what goes back to the composer, saying why (`restore`, `error`; `action`, what to
 * offer doing about it where the conversation's model can't run, as `kuvo/sessions/models` says
 * it); `ambiguous` where the box can't tell whether a draft's message went. */
export const TYPED = object({
  "send?": object({ text: string, "quoted?": bool }),
  "restore?": TYPED_RESTORE, "ambiguous?": bool, "error?": string, "action?": MODEL_ACTION,
});

export type TypedAnswer = Infer<typeof TYPED>;

const REFUSED = object({ message: string });
/** A send it couldn't answer for: why, and what goes back to the composer (as `restore`). */
const SEND_REFUSED = object({ message: string, "restore?": TYPED_RESTORE, "ambiguous?": bool });
const DRAFT_ANSWERS = {
  200: object({ answer: TYPED, "message?": string, "acceptedAt?": number }), 202: OWNER_MESSAGE_PENDING, 400: REFUSED, 403: REFUSED,
  404: SEND_REFUSED, 405: object({}), 409: oneOf(SEND_REFUSED, object({ status: string, operation: string, owner: string, message: string })), 503: SEND_REFUSED,
};
const SUBMISSION_PAGE = object({ currentOwner: nullable(string), complete: bool, "setup?": string, "canSubmit?": bool, "unknown?": bool, "next?": string,
  outcomes: list(object({ submission: string, originalOwner: string, digest: string, submitter: any, evidence: words("owner", "listing"),
    status: words("committed", "notAdmitted", "pending", "unknown"), "conflict?": bool, "retryable?": bool,
    "acceptedAt?": number, "effect?": any, "decision?": any, "draft?": any, "draftAvailable?": bool, "recoveryComputer?": string })) });

export const TYPED_ROUTES: RouteShape[] = [
  { route: "POST kuvo/sessions/:id/draft", feature: "typed.draft", answers: DRAFT_ANSWERS },
  { route: "POST kuvo/sessions/:id/submit", feature: "typed.submission-recovery", answers: DRAFT_ANSWERS },
  { route: "GET kuvo/sessions/:id/submissions", feature: "typed.submission-recovery", answers: { 200: SUBMISSION_PAGE, 400: REFUSED, 403: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/sessions/:id/submissions/:submission", feature: "typed.submission-recovery", answers: { 200: SUBMISSION_PAGE, 400: REFUSED, 403: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/sessions/:id/typed", feature: "typed", answers: { 200: TYPED_OFFERS, 404: REFUSED } },
  { route: "POST kuvo/sessions/:id/typed", feature: "typed", answers: { 200: TYPED, 400: REFUSED, 404: SEND_REFUSED, 503: SEND_REFUSED } },
];
