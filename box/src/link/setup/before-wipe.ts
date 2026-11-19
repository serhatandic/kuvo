import type { LocalWipe } from "./removal-record";

type WipePreparation =
  | { phase: "before-unlink"; setup: string; operation: string; key: string }
  | ({ phase: "before-wipe" } & LocalWipe);

/** #905 can arrange a move before revocation and complete an export before local deletion.
 * Throw to pause at either point. Retries use the same operation. */
export async function beforeMembershipWipe(_wipe: Readonly<WipePreparation>): Promise<void> {}
