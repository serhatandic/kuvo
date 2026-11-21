import { fingerprint, observe, type Fields, type Passages } from "../ownership/merge";
import type { Item } from "./store";

const scalarFields = ["title", "body", "status", "priority", "labels", "assignee", "due"] as const;
export const queueField = (key: string) => key === "order" || scalarFields.some(field => field === key) || /^note:[\w-]{1,80}$/.test(key);
export const queuePassage = (key: string) => key === "body" || key.startsWith("note:");

export function observeQueueItem(item: Item, passages?: Passages) {
  const fields: Fields = Object.fromEntries(scalarFields.map(key => [key, item[key] ?? null]));
  fields.order = { position: item.order, status: item.status };
  for (const [index, note] of item.notes.entries()) {
    note.id ??= fingerprint([item.id, index, note.at, note.by, note.text]);
    fields[`note:${note.id}`] = note.text;
  }
  return item.offlineEdits = observe(item.offlineEdits, item.id, fields, Boolean(item.removed), undefined, queuePassage, passages);
}
