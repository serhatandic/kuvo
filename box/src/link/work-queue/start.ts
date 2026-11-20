import type { Target } from "../../shared/records";
import { workSessions } from "../work/tree";
import { linkSession, Refusal, refuseHanding, refuseMoving } from "./ops";
import { references } from "./relations";
import { isClosed } from "./statuses";
import { allItems, changeQueues, matching } from "./store";
import { canChangeItem, overseenBy, refOf } from "./view";

/** An explicit item takes precedence over the first Work Queue reference in the task. */
export function startItem(args: Record<string, unknown>): string | number | undefined {
  if (args.item !== undefined && args.item !== null) {
    const item = args.item;
    return (typeof item === "string" || typeof item === "number") && refOf(item) ? item : undefined;
  }
  return typeof args.task === "string" ? references(args.task, { task: true })[0]?.number : undefined;
}

/** Link before the new session sees its task. The starter may only delegate work it can change. */
export async function linkStartedItem(given: unknown, starter: string, session: string, targets: Target[], signal?: AbortSignal): Promise<void> {
  const ref = refOf(given);
  if (!ref || !matching(allItems(), ref).some(item => !item.removed && !isClosed(item.status))) return;
  const overseen = overseenBy(starter, await workSessions(targets, signal, { archived: true }).catch(() => []));
  changeQueues(book => {
    const item = matching(book.items(), ref).find(each => !each.removed && canChangeItem(each, starter, overseen));
    if (!item || isClosed(item.status)) return;
    try {
      refuseMoving([item.owner]);
      refuseHanding(item);
    } catch (error) {
      if (error instanceof Refusal) return;
      throw error;
    }
    linkSession(book, item, session, Date.now());
  });
}
