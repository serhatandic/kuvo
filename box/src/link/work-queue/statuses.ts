// Where a Work Queue item stands (PROTOCOL.md, "The Work Queue", "Statuses"): one status each, the
// same set on every box, never changed by anyone; and how much it matters (its priority). The box
// says both lists, in the order clients show them, and which statuses are closed.

export interface Status { id: string; name: string; closed: boolean }
export interface Priority { id: string; name: string }

/** Every status, in the order clients show them: the open ones, then the closed. */
export const STATUSES: readonly Status[] = [
  { id: "needs-you", name: "Needs you", closed: false },
  { id: "in-review", name: "In review", closed: false },
  { id: "in-progress", name: "In progress", closed: false },
  { id: "waiting", name: "Waiting", closed: false },
  { id: "todo", name: "Todo", closed: false },
  { id: "backlog", name: "Backlog", closed: false },
  { id: "done", name: "Done", closed: true },
  { id: "canceled", name: "Canceled", closed: true },
];

/** Every priority, the most pressing first. An item with none kept is Normal. */
export const PRIORITIES: readonly Priority[] = [
  { id: "urgent", name: "Urgent" },
  { id: "high", name: "High" },
  { id: "normal", name: "Normal" },
  { id: "low", name: "Low" },
];

export const STATUS_ID = /^(?:needs-you|in-review|in-progress|waiting|todo|backlog|done|canceled)$/;
export const PRIORITY_ID = /^(?:urgent|high|normal|low)$/;

/** Whether a status is a closed one (Done or Canceled). */
export const isClosed = (status: string): boolean => status === "done" || status === "canceled";

/** A status's name, as it's said (its id for one this box doesn't know). */
export const statusName = (id: string): string => STATUSES.find(each => each.id === id)?.name ?? id;
export const priorityName = (id: string | undefined): string => PRIORITIES.find(each => each.id === (id ?? "normal"))?.name ?? "Normal";

// What people and agents write for a status that isn't its name: the old categories' names, and
// other spellings.
const ALSO: Record<string, string> = {
  next: "todo", "to do": "todo", later: "backlog", cancelled: "canceled", "waiting on you": "needs-you", "needs user": "needs-you", review: "in-review",
  "in-review": "in-review", doing: "in-progress", blocked: "waiting",
};

/** The status `given` names (its id or its name, any case, with spaces, dashes or underscores), or
 * null for none. */
export function statusNamed(given: unknown): Status | null {
  if (typeof given !== "string" || !given.trim()) return null;
  const wanted = given.trim().toLowerCase().replace(/[\s_-]+/g, " ");
  return STATUSES.find(each => each.id.replace(/-/g, " ") === wanted || each.name.toLowerCase() === wanted) ?? STATUSES.find(each => each.id === ALSO[wanted]) ?? null;
}

/** The priority `given` names (its id or its name, any case), or null for none. */
export function priorityNamed(given: unknown): Priority | null {
  if (typeof given !== "string" || !given.trim()) return null;
  const wanted = given.trim().toLowerCase();
  return PRIORITIES.find(each => each.id === wanted) ?? (wanted === "medium" || wanted === "none" ? PRIORITIES[2] : null);
}

/** A custom category's colour as the apps drew it before labels (by its id), in a label's colours.
 * (Here, not in labels.ts: `migrate.ts` reads it as the store loads.) */
export const colorBefore = (id: string): string =>
  ["orange", "pink", "indigo", "mint", "coral", "amber"][[...id].reduce((sum, each) => (sum * 31 + (each.codePointAt(0) ?? 0)) & 0xFFFF, 0) % 6];

/** The label that says an item was Parked, before statuses (`migrate.ts`). */
export const PARKED_LABEL = "lbl_parked";

/** The category a box from before statuses reads an item under (one of its built-in four), so it
 * never takes an item kept, handed or moved with a status for no item at all: Done for a closed
 * one, Later for Backlog (Parked with that label), else Next. */
export const categoryBefore = (item: { status: string; labels?: string[] }): string =>
  isClosed(item.status) ? "done" : item.status === "backlog" ? (item.labels?.includes(PARKED_LABEL) ? "parked" : "later") : "next";

/** Where an item stands as it's read, by what it says: its `status` and labels, unless a box from
 * before statuses moved it since. Such a box changes only `category` (it keeps `status` as a field
 * it doesn't know), and this one always writes the category its status gives (`categoryBefore`):
 * so a category that isn't that one is the later word, and the item stands where it puts it. One
 * with no status at all is from before statuses. `name`: a custom category's name, where known. */
export function standing(given: { status?: unknown; category?: unknown; labels?: unknown }, name?: (category: string) => string | undefined): { status: string; labels: string[] } {
  const kept = (Array.isArray(given.labels) ? given.labels : []).filter((each): each is string => typeof each === "string" && /^(?:lbl|cat)_[a-z0-9-]{1,40}$/.test(each));
  const named = typeof given.category === "string" ? name?.(given.category) : undefined;
  if (typeof given.status !== "string" || !given.status) {
    const before = fromCategory(given.category, named);
    return { status: before.status, labels: before.label ? [before.label] : [] };
  }
  const labels = [...new Set(kept)];
  // (A status this box doesn't know is a newer box's, with a category of its own choosing: as it came.)
  // (Nor is a category such a box never writes its move.)
  if (typeof given.category !== "string" || !/^(?:next|later|parked|done|cat_[a-z0-9-]{1,40})$/.test(given.category) || !STATUS_ID.test(given.status) || given.category === categoryBefore({ status: given.status, labels })) return { status: given.status, labels };
  const moved = fromCategory(given.category, named);
  // (Out of Parked: its label goes with the move.)
  const rest = moved.label === PARKED_LABEL ? labels : labels.filter(each => each !== PARKED_LABEL);
  return { status: moved.status, labels: moved.label && !rest.includes(moved.label) ? [...rest, moved.label] : rest };
}

/** Where an item kept under a category (before statuses) stands now: its status, and the label its
 * category became. `name` is its custom category's name, where that's known. Next is Todo, Later
 * Backlog, Done Done; Parked is Backlog with the label Parked; "Waiting on you" is Needs you; any
 * other custom one is a label, on a Todo item (Backlog for "Correctness sweep" and "Edge cases"). */
export function fromCategory(category: unknown, name?: string): { status: string; label?: string } {
  if (category === "next") return { status: "todo" };
  if (category === "later") return { status: "backlog" };
  if (category === "done") return { status: "done" };
  if (category === "parked") return { status: "backlog", label: PARKED_LABEL };
  if (typeof category !== "string" || !/^cat_[a-z0-9-]{1,40}$/.test(category)) return { status: "todo" };
  const said = (name ?? category.slice(4).replace(/-/g, " ")).trim().toLowerCase().replace(/[\s_-]+/g, " ");
  if (said === "waiting on you") return { status: "needs-you" };
  if (said === "correctness sweep" || said === "edge cases") return { status: "backlog", label: category };
  return { status: "todo", label: category };
}
