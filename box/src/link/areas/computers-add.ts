// `kuvo/computers/add…` (PROTOCOL.md, "Computers", Adding one; `computers.add`): adding a computer
// over SSH from this box, for any client: whether it adds them and the hosts to suggest, its tries
// as they go (their steps, questions and words, all the box's), an answer to what one asks, one
// stopped or forgotten; and, for a computer it set up, setting it up again and uninstalling Kuvo
// Code from it. The work is `add/flow.ts`; a client only asks and draws.

import { parsed, type Body } from "../../shared/json";
import type { Adding } from "../../shared/routes/box";
import { addressProblem, cleanAddress, suggestedHosts } from "../add/address";
import { beginAdd, beginSetup, beginUninstall, canAdd, sshAddressOf, takenAddresses } from "../add/flow";
import { allTries, answerTry, endTry, tryFollowers } from "../add/tries";
import { followed } from "../follow";
import type { AnswerPiece, RouteContext } from "../server/context";

export const COMPUTERS_ADD_ROUTE = /^kuvo\/computers\/(?:add(?:(\/events)|\/(add_[a-z0-9]{1,40})(\/answer)?)?|([A-Za-z0-9_-]{43})\/(setup|uninstall))$/;

/** Adding computers, as this box says it now. */
export function adding(): Adding {
  const can = canAdd();
  return { can: can.can, ...(can.can ? {} : { why: can.message }), hosts: can.can ? suggestedHosts(takenAddresses()) : [], tries: allTries() };
}

// What a try needs a process that stays: `kuvo-link.js box route` answers once and ends.
const ONCE = "Ask this through Kuvo Code’s running box on this computer: a single request can’t stay to do it.";

export async function* computersAddRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  const method = request.method.toUpperCase();
  const [, events, id, answering, key, action] = match;
  if (events) {
    if (method !== "GET") return yield* json(405, { message: "Only GET." });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(adding, tryFollowers, signal);
  }
  if (key) {
    if (method !== "POST") return yield* json(405, { message: "Only POST." });
    if (request.fromMac) return yield* json(409, { message: ONCE });
    const can = canAdd();
    if (!can.can) return yield* json(can.status, { message: can.message, ...(can.primary ? { primary: can.primary } : {}) });
    if (!sshAddressOf(key)) return yield* json(404, { message: "That isn’t a computer this one set up over SSH." });
    const begun = action === "setup" ? beginSetup(key) : beginUninstall(key);
    if (!begun) return yield* json(404, { message: "That isn’t a computer this one set up over SSH." });
    if ("busy" in begun) return yield* json(409, { message: begun.busy });
    return yield* json(202, { try: begun.seen });
  }
  if (id && answering) {
    if (method !== "POST") return yield* json(405, { message: "Only POST." });
    const given = parsed(request.body) as Body<"answer" | "at">;
    // (An answer names the ask it's for: one that doesn't is never taken.)
    const answered = typeof given?.answer === "string" && typeof given.at === "number" ? answerTry(id, given.answer, given.at) : null;
    if (!answered) return yield* json(409, { message: "It isn’t asking that now." });
    return yield* json(200, { try: answered });
  }
  if (id) {
    if (method !== "DELETE") return yield* json(405, { message: "Only DELETE." });
    if (!endTry(id)) return yield* json(404, { message: "There’s no such try." });
    return yield* json(200, {});
  }
  if (method === "GET") return yield* json(200, adding());
  if (method !== "POST") return yield* json(405, { message: "GET, or POST {destination}." });
  if (request.fromMac) return yield* json(409, { message: ONCE });
  const can = canAdd();
  if (!can.can) return yield* json(can.status, { message: can.message, ...(can.primary ? { primary: can.primary } : {}) });
  const destination = cleanAddress((parsed(request.body) as Body<"destination">)?.destination);
  const problem = addressProblem(destination);
  if (problem) return yield* json(400, { message: problem });
  const begun = beginAdd(destination);
  if ("busy" in begun) return yield* json(409, { message: begun.busy });
  return yield* json(202, { try: begun.seen });
}
