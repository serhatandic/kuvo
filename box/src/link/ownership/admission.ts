import { dataOwner } from "../../shared/data-declarations";
import { MUTATION_DECLARATIONS, type MutationLane } from "../../shared/mutation-declarations";
import { ownSign } from "../documents/format";
import { followedSetup } from "../setup/record";
import { readLink, type LinkState } from "../state";
import { mayActivate, mayRecover, type DataReference } from "./resolve";

export function primaryMutationActor(link: LinkState | null, stage: "propose" | "accept" | "recover" = "propose",
  witness?: { key: string; sign: string }) {
  const actor = MUTATION_DECLARATIONS.sharedSave[stage];
  const setup = followedSetup(link);
  if (setup?.role !== "primary" || setup.sign !== ownSign(link)) return null;
  if (actor === "current primary with required standby receipt" && setup.standby &&
      (witness?.key !== setup.standby.key || witness.sign !== setup.standby.sign)) return null;
  return setup;
}

export function acceptsMutation(lane: Exclude<MutationLane, "sharedSave">, ref: DataReference, link = readLink(), verifyLegacy = false): boolean {
  const canUse = verifyLegacy ? mayRecover : mayActivate;
  return MUTATION_DECLARATIONS[lane].accept === "declared owner" && canUse(ref, { link });
}

export function recoversMutation(lane: MutationLane, ref: DataReference, authority: Parameters<typeof mayRecover>[1]): boolean {
  return MUTATION_DECLARATIONS[lane].recover === "declared owner" && mayRecover(ref, authority);
}

/** The same computer that accepts the mutation must run its native adapter. Protected
 * actions take an observation from that adapter's OS, such as the current Helper's grant. */
export function performsMutation(lane: "command" | "consent" | "queuedSessionWork" | "sandboxCommand" | "macProtectedAction", ref: DataReference,
  osGrant?: () => boolean, link = readLink()): boolean {
  const nativeSession = lane !== "macProtectedAction";
  if (!acceptsMutation(lane, ref, link, nativeSession)) return false;
  const actor = MUTATION_DECLARATIONS[lane].perform;
  return actor === "owner's native adapter" || (actor === "locally OS-granted helper"
    && MUTATION_DECLARATIONS.macProtectedAction.propose === "executing computer"
    && dataOwner(ref.kind, ref.queue) === "local computer" && osGrant?.() === true);
}
