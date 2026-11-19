import { literal, number, object, string, type Infer, words } from "../shape";

export const SAVE_RECEIPT = object({
  setup: string, epoch: number, operation: string, proof: object({ d: string, s: string }),
  status: words("pending", "committed", "interrupted"), "pending?": literal(true), "message?": string,
});

export const PENDING_SAVE = "Pending until the standby is back.";
export const INTERRUPTED_SAVE = "The primary changed before this save committed.";

type Reference = Omit<Infer<typeof SAVE_RECEIPT>, "status" | "pending" | "message">;
export type Receipt = Reference & (
  | { status: "committed"; pending?: never; message?: string }
  | { status: "pending"; pending: true; message: string }
  | { status: "interrupted"; pending?: never; message: string }
);
export type SetupSaveReceipt = Receipt;
export type CommittedReceipt = Extract<Receipt, { status: "committed" }>;
export type PendingReceipt = Extract<Receipt, { status: "pending" }>;
export type InterruptedReceipt = Extract<Receipt, { status: "interrupted" }>;

export type SavedReply<T extends object = object> = { status: 200; value: T & CommittedReceipt };
export type PendingReply = { status: 202; value: PendingReceipt };
export type InterruptedReply = { status: 409; value: InterruptedReceipt };
export type SaveReply<T extends object = object> = SavedReply<T> | PendingReply | InterruptedReply;

// Local, legacy and refused changes can reply without a shared operation.
export type UnsavedReply<S extends number, T extends object = object> = {
  status: S;
  value: T & { status?: never; operation?: never; proof?: never };
};

export type SaveEnvelope<R extends Receipt = Receipt> = { save: R };
