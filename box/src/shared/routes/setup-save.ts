import { literal, number, object, string, type Infer, words } from "../shape";

export const SAVE_RECEIPT = object({
  setup: string, epoch: number, operation: string, proof: object({ d: string, s: string }),
  status: words("pending", "committed", "interrupted"), "pending?": literal(true), "message?": string,
});

export type SetupSaveReceipt = Infer<typeof SAVE_RECEIPT>;
