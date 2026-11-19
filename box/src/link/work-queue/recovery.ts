import { isObject } from "../../shared/json";
import { queueCopyDecision } from "../ownership/resolve";
import { itemElsewhere } from "./across";
import { deletionFor, departureFor } from "./store";

type Authority = Parameters<typeof queueCopyDecision>[1];

export function recoveredQueueDecision(item: Record<string, unknown> & { id: string }, queue: string,
  authority: Authority, saved?: { deletion: unknown; departure: unknown }) {
  const claim = itemElsewhere({ id: item.id }, authority?.link, { asTold: true, deleted: true });
  return queueCopyDecision({ kind: "queueItems", id: item.id, queue }, authority, {
    deleted: Boolean(item.removed || deletionFor(item.id) || saved?.deletion || claim?.removed),
    handing: isObject(item.handing),
    departed: Boolean(departureFor(item.id) || saved?.departure),
    claimed: Boolean(claim),
  });
}
