// A client's message as fields, and the parts of messages (PROTOCOL.md, "A message's parts").

import { list, literal, number, object, type RouteShape, string, words, any } from "../shape";

import { OWNER_MESSAGE_INTERRUPTED, OWNER_MESSAGE_PENDING } from "./ownership";
import { oneOf } from "../shape";

const REFUSED = object({ message: string });

/** A comment on one line of the changes. */
export const REVIEW_COMMENT = object({ file: string, line: number, side: words("new", "old"), snippet: string, text: string });

/** A message's parts, as clients draw it (`metadata.kuvo.parts`, or read from one from before). */
export const MESSAGE_PARTS = object({
  words: string,
  "widget?": string,
  "comments?": list(REVIEW_COMMENT),
  "paths?": list(string),
  "agents?": list(object({ name: string, session: string })),
  "phone?": literal(true),
  "mail?": object({ name: string, session: string }),
});

export const MESSAGE_ROUTES: RouteShape[] = [
  // Send a message into a session; to an owner elsewhere it's saved here and answered 202.
  // (The engine's answer, or its coding agents' bridge's, as it gave it.)
  { route: "POST kuvo/sessions/:id/send", feature: "messages.send", answers: { 200: any, 202: OWNER_MESSAGE_PENDING, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: oneOf(OWNER_MESSAGE_INTERRUPTED, REFUSED), 503: REFUSED } },
  // The parts of messages from before parts were kept (`texts`, at most 500).
  { route: "POST kuvo/messages/parts", feature: "messages.send", answers: { 200: object({ parts: list(MESSAGE_PARTS) }), 400: REFUSED, 405: REFUSED } },
];
