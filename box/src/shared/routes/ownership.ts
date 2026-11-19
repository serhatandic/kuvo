import { any, bool, list, nullable, number, object, oneOf, string, words, type Infer, type RouteShape } from "../shape";

const REFUSED = object({ message: string });
const OWNER = object({
  key: nullable(string), name: string, local: bool, reachable: bool,
  scope: words("primary", "session computer", "group host", "bot home", "local computer", "client"),
});
const READ = object({
  owner: OWNER, stale: bool, state: words("received", "neverReceived"),
  lastReceivedAt: nullable(number), "data?": any,
  unavailable: object({ files: list(string), messages: list(string) }),
});
const PENDING = object({ status: words("pending"), operation: string, owner: string, message: string });
const COMMITTED = object({ status: words("committed"), operation: string, owner: string, acceptedAt: number });
const INTERRUPTED = object({ status: words("interrupted"), operation: string, owner: string, message: string });
const RECEIPT = oneOf(PENDING, COMMITTED, INTERRUPTED);

export type MessageReceipt = Infer<typeof RECEIPT>;
export const OWNERSHIP_ROUTES: RouteShape[] = [
  { route: "GET kuvo/data/owner", feature: "data.ownership", answers: { 200: READ, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: REFUSED } },
  { route: "POST kuvo/data/messages", feature: "data.ownership", answers: { 200: COMMITTED, 202: PENDING, 400: REFUSED, 403: REFUSED, 409: oneOf(INTERRUPTED, REFUSED), 503: REFUSED } },
  { route: "GET kuvo/data/messages/:operation", feature: "data.ownership", answers: { 200: RECEIPT, 403: REFUSED, 404: REFUSED } },
];
export const OWNER_MESSAGE_PENDING = PENDING;
export const OWNER_MESSAGE_INTERRUPTED = INTERRUPTED;
export const OWNER_MESSAGE_COMMITTED = COMMITTED;
