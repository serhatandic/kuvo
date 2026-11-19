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
