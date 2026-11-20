import { linkSession, noteEvent, Refusal, refuseHanding, refuseMoving, refuseQueueOwner } from "./ops";
import { references } from "./relations";
import { isClosed } from "./statuses";
import { allItems, changeQueues, matching, type Who } from "./store";
import { refOf, visibleItem } from "./view";

/** An explicit item takes precedence over the first Work Queue reference in the task. */
export function startItem(args: Record<string, unknown>): string | number | undefined {
  if (args.item !== undefined && args.item !== null) {
    const item = args.item;
    return (typeof item === "string" || typeof item === "number") && refOf(item) ? item : undefined;
  }
  return typeof args.task === "string" ? references(args.task, { task: true })[0]?.number : undefined;
}

/** Link before the new session sees its task. */
export function linkStartedItem(given: unknown, starter: Who, session: string): void {
  const ref = refOf(given);
  if (!ref || !matching(allItems(), ref).some(item => !item.removed && !isClosed(item.status))) return;
  changeQueues(book => {
    const item = matching(book.items(), ref).find(each => visibleItem(each, { owner: "all" }));
    if (!item || isClosed(item.status)) return;
    try {
      refuseQueueOwner(item.owner, item.id);
      refuseMoving([item.owner]);
      refuseHanding(item);
    } catch (error) {
      if (error instanceof Refusal) return;
      throw error;
    }
    if (item.sessions.some(each => each.session === session)) return;
    const at = Date.now();
    linkSession(book, item, session, at, starter);
    noteEvent(item, { at, by: starter, kind: "linked", to: session });
    item.updated = { at, by: starter };
  });
}
