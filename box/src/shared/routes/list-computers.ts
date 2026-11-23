import { list, object, string } from "../shape";

export const LIST_COMPUTER = object({ id: string, name: string });
export const PENDING_COMPUTERS = { "pendingComputers?": list(LIST_COMPUTER) } as const;
