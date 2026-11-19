import type { Decision, Edit, Fields } from "./merge";
import type { DataReference } from "./resolve";

export interface EditTarget extends DataReference {
  kind: "queueItems" | "botSettings" | "setupTimeZone" | "setupUsageDisplay" | "setupNames" | "setupPersonalities" | "queueLabels";
}

export interface OfflineEdit extends Edit {
  ref: EditTarget;
  create?: true;
  notes?: { id: string; text: string }[];
  attachments?: { id: string; file: Fields }[];
}
export type EditReceipt = Decision | { operation: string; status: "pending" | "interrupted" | "capacity"; message: string };
export const editStatus = (receipt: EditReceipt) => receipt.status === "pending" ? 202
  : receipt.status === "capacity" ? 413
  : receipt.status === "interrupted" || receipt.status === "refresh" || receipt.status === "deleted" ? 409 : 200;
