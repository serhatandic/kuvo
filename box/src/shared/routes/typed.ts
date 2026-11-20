// What a typed message means, and what a conversation's composer offers after "/" (PROTOCOL.md,
// "What a typed message means").

import { any, bool, type Infer, list, oneOf, nullable, number, object, type RouteShape, string } from "../shape";
import { SESSION_AUTOPILOT } from "./autopilot";
import { OWNER_MESSAGE_PENDING } from "./ownership";
import { MODEL_ACTION } from "./sessions";

/** A keyword a conversation offers: its name ("loop", typed "/loop"), its name in a list, what it
 * does in a line, and what follows it, in grey until something's written (null: it takes nothing). */
export const OFFERED_KEYWORD = object({ name: string, title: string, summary: string, argument: nullable(string) });

/** What "/" offers in a conversation: its keywords, then the engine's commands and yours (its own
 * /review as /review-quick where the keyword has the name; none a keyword hides). */
export const TYPED_OFFERS = object({ keywords: list(OFFERED_KEYWORD), commands: list(object({ name: string, "description?": string })) });

/** What goes back to the composer: `text` into an empty one; beside words written since, `beside`
 * in front of them where it's given, else `text`. */
export const TYPED_RESTORE = object({ text: string, "beside?": string });

/** What a typed text meant: what the box did (`goal`, `goalCleared`, `askedBot`, `autopilot`,
 * `wakeUp`, `sideQuestion`, `handoff`, `review`), then what the client does, in this order
 * (`showGoal`, `agent` (the mode to switch to: Plan, or back from it), `toggleAutopilot`, `compact`, `shell`, `command`, `send`, `sideThread`), and
 * what goes back to the composer, saying why (`restore`, `error`; `action`, what to offer doing
 * about it where the conversation's model can't run, as `kuvo/sessions/models` says it). The composer's files go with
 * `send` or `command` (else they stay); its quotes with `send` when `quoted` (else they stay). */
export const TYPED = object({
  "keyword?": string,
  "goal?": any, "goalCleared?": bool, "askedBot?": bool, "autopilot?": SESSION_AUTOPILOT,
  "wakeUp?": object({ due: number, every: string, note: string }),
  "sideQuestion?": any, "handoff?": any, "review?": any,
  "showGoal?": bool, "agent?": string, "toggleAutopilot?": bool, "compact?": bool, "said?": string,
  "shell?": string, "command?": object({ name: string, arguments: string }),
  "send?": object({ text: string, "typed?": string, "quoted?": bool, "steer?": bool }),
  "sideThread?": bool,
  "restore?": TYPED_RESTORE, "ambiguous?": bool, "error?": string, "action?": MODEL_ACTION,
});

export type TypedAnswer = Infer<typeof TYPED>;

const REFUSED = object({ message: string });
/** A send it couldn't answer for: why, and what goes back to the composer (as `restore`). */
const SEND_REFUSED = object({ message: string, "restore?": TYPED_RESTORE, "ambiguous?": bool });

export const TYPED_ROUTES: RouteShape[] = [
  { route: "POST kuvo/sessions/:id/draft", feature: "typed.draft", answers: {
    200: object({ answer: TYPED, "message?": string, "acceptedAt?": number }), 202: OWNER_MESSAGE_PENDING, 400: REFUSED, 403: REFUSED, 404: SEND_REFUSED, 405: object({}), 409: oneOf(SEND_REFUSED, object({ status: string, operation: string, owner: string, message: string })), 503: SEND_REFUSED,
  } },
  { route: "GET kuvo/sessions/:id/typed", feature: "typed", answers: { 200: TYPED_OFFERS, 404: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/sessions/:id/typed", feature: "typed", answers: { 200: TYPED, 400: REFUSED, 404: SEND_REFUSED, 503: SEND_REFUSED } },
];
