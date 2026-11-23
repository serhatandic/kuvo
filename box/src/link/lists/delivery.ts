// One queue on the client's box, including the drafts it hasn't handed to the owner yet.
import { StringDecoder } from "node:string_decoder";
import { isObject, parsed } from "../../shared/json";
import type { QueueShown } from "../../shared/routes/sessions";
import { followedLive } from "../follow";
import { changeDelivery, deliveryFollowers, deliveryItems, readMessageReceipt, saveDelivery } from "../ownership/messages";
import { submitterOf } from "../ownership/authority";
import { KEY, ownKey, peersOf } from "../peers/known";
import { peerIsAway, readReach } from "../peers/reach";
import type { AnswerPiece, RouteContext } from "../server/context";
import { readLink } from "../state";

export async function* deliveryRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, redispatch } = context;
  const [, session, resource, mode] = match;
  const method = request.method.toUpperCase();
  const computer = new URLSearchParams(request.path.split("?")[1]).get("computer") ?? "here";
  const link = readLink();
  if (!submitterOf(context, link, true)) return yield* json(403, { message: "Open this from a paired Kuvo Code client." });
  if (computer !== "here" && !KEY.test(computer)) return yield* json(400, { message: "Name the conversation’s computer." });
  if (!resource && method === "POST") return yield* await saveDelivery(context, session!, computer);
  if (resource && resource !== "events" && (mode ? method === "POST" : method === "DELETE"))
    return yield* await changeDelivery(context, session!, computer, resource, mode as "steer" | "now" | "ack" | undefined);
  if ((resource !== "events" && resource !== undefined) || mode || method !== "GET")
    return yield* json(405, { message: "Read the queue or submit a draft." });

  const local = computer === "here" || Boolean(link && ownKey(link) === computer);
  const peer = local ? undefined : peersOf(link).find(each => each.key === computer);
  if (!local && !peer) return yield* json(404, { message: "There’s no such computer in this setup." });
  const snapshot = async (owner?: QueueShown) => {
    const outgoing = await deliveryItems(context, session!, computer);
    const ids = new Set(owner?.items?.map(item => item.id));
    const waiting = outgoing.filter(item => !ids.has(item.id));
    const deliveryLine = waiting.length > 0 && peer && peerIsAway(peer.key, undefined, readReach())
      ? `${peer.name} isn’t answering · your messages will go when it’s back` : undefined;
    return { paused: false, held: [], ...owner, items: [...(owner?.items ?? []), ...waiting], ...(deliveryLine ? { deliveryLine } : {}) };
  };
  if (!resource) return yield* json(200, await snapshot());

  const path = local ? `kuvo/sessions/${session}/queue/events` : `kuvo/peer-session/${computer}/${session}/queue/events`;
  const source = new AbortController();
  const stop = signal ? AbortSignal.any([signal, source.signal]) : source.signal;
  let owner: QueueShown | undefined;
  let opening = false;
  const follow = (tell: () => void) => {
    if (!redispatch || opening || stop.aborted) return;
    opening = true;
    void (async () => {
      const decoder = new StringDecoder("utf8");
      let pending = "";
      for await (const part of redispatch({ ...request, path }, stop)) {
        if ("head" in part) {
          if (part.head.status !== 200) return;
          continue;
        }
        pending += decoder.write(part.body);
        let end: number;
        while ((end = pending.indexOf("\n")) >= 0) {
          const value = parsed(Buffer.from(pending.slice(0, end)));
          pending = pending.slice(end + 1);
          if (isObject(value) && typeof value.paused === "boolean") {
            const next = value as QueueShown;
            owner = { ...next, items: next.items ?? owner?.items ?? null };
            tell();
          }
        }
        if (pending.length > 4 * 1024 * 1024) return;
      }
    })().catch(() => {}).finally(() => { opening = false; });
  };
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  try {
    yield* followedLive(async () => snapshot(owner), tell => {
      deliveryFollowers.add(tell);
      follow(tell);
      const again = setInterval(() => follow(tell), 5000);
      return () => { clearInterval(again); deliveryFollowers.delete(tell); };
    }, stop, { settle: 0, every: 3000 });
  } finally { source.abort(); }
}


export const DELIVERY_RECEIPT_EVENTS = /^kuvo\/data\/messages\/([A-Za-z0-9_-]{43})\/events$/;

export async function* deliveryReceiptEvents(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json, signal } = context;
  const operation = match[1]!;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  if (!await readMessageReceipt(context, operation)) return yield* json(404, { message: "There’s no such message operation here." });
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(() => readMessageReceipt(context, operation), tell => {
    deliveryFollowers.add(tell);
    return () => { deliveryFollowers.delete(tell); };
  }, signal, { settle: 0 });
}
