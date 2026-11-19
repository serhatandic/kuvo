import { isObject } from "../../shared/json";
import { mayActivate } from "../ownership/resolve";
import { itemElsewhere } from "./across";
import { deletionFor, departureFor } from "./store";

type Authority = NonNullable<Parameters<typeof mayActivate>[1]>;

/** A queue address controls the queue, but a handoff changes the owner of one item.
 * With the old computer gone, absence from peer directories cannot prove that it
 * kept the item. Keep that copy for review until item-level ownership is known. */
export function restoredQueueItem(item: Record<string, unknown> & { id: string }, queue: string, paired: boolean,
  authority: Authority, saved: { deletion: unknown; departure: unknown }): "active" | "deleted" | "held" {
  if (item.removed || deletionFor(item.id) || saved.deletion) return "deleted";
  const ref = { kind: "queueItems" as const, id: item.id, queue };
  const told = itemElsewhere({ id: item.id }, authority.link);
  if (told || isObject(item.handing) || departureFor(item.id) || saved.departure || !mayActivate(ref, authority)) return "held";
  return paired ? "held" : "active";
}
