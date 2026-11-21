// Save the portable roster and grants with their local effects.
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import { setupMergeLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { bodyHash } from "../documents/format";
import { K } from "../home";
import { primaryPeersBody } from "../documents/work";
import type { Peer } from "../peers/known";
import { ownPhones } from "../phones";
import { updateLink, type LinkState } from "../state";
import { computerHistory } from "./computer-history";
import { acceptedMembership, installerRecoveriesOf, INSTALLER_RECOVERIES, membershipOf, projectMembership, type Membership } from "./membership";
import { primaryFenced } from "./lease";
import { SharedCapacity, stageShared, type CompanionChanges, type Receipt } from "./shared";

const localEffects = DATA_DECLARATIONS.membershipAndAccess.shared.localEffects;
// COMPAT(layout.4 setup): a layout 4 setup saves the full projection until its main converts it. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
const legacyFields = ["macPeers", "macMoved", "peerNames", "left", "removedHere", "removalRevisions",
  "phones", "removed", "computerHistory", ...localEffects.filter(key => key !== "localWipe"),
] as const satisfies readonly (keyof LinkState)[];

export type MembershipCompanions = (draft: LinkState) => (roster: Membership, link: LinkState) => CompanionChanges;

export function captureMembership(link: LinkState, self?: Peer, previous?: Membership) {
  const { boxes, left, moved } = primaryPeersBody(link);
  // A draft starts with the accepted base names. Shared name overrides belong to setupNames.
  const projected = Array.isArray(link.macPeers) ? link.macPeers : [];
  const roster = self ? [self, ...projected] : boxes;
  return membershipOf({
    boxes: roster, left, moved, computerHistory: computerHistory(link), devices: ownPhones(link), removalRevisions: link.removalRevisions, removedHere: link.removedHere,
    removals: link.memberRemovals, wipePreparations: link.wipePreparations, mergedComputers: link.mergedComputers ?? previous?.mergedComputers,
    installerRecoveries: link.installerRecoveries,
  });
}

export function updateAccess(change: (link: LinkState | null) => LinkState | null | undefined | void,
  { localOnly = false, operation, checkpoint = false, together, beforeWrite }: { localOnly?: boolean; operation?: string; checkpoint?: boolean; together?: MembershipCompanions; beforeWrite?: (link: LinkState) => void } = {}): { changed: LinkState | null; save: Receipt | null; busy: boolean } {
  let save: Receipt | null = null;
  let busy = false;
  const changed = updateLink(link => {
    if (link?.macRotation) {
      busy = true;
      return null;
    }
    if (localOnly || link?.setup?.role !== "primary") {
      const modified = change(link);
      if (modified) beforeWrite?.(modified);
      return modified;
    }
    if (primaryFenced(link, true) || link.localWipe) {
      busy = true;
      return null;
    }
    const bootstrap = checkpoint && !link.setup.checkpointed;
    if (!bootstrap && !link.setup.checkpointed) {
      const modified = change(link);
      if (modified) beforeWrite?.(modified);
      return modified;
    }
    const draft = structuredClone(link);
    const accepted = acceptedMembership(link);
    if (accepted) {
      projectMembership(draft, accepted);
      // Legacy mutators edit this projection. Readers of the live link use accepted membership.
      delete draft.setup!.checkpointed;
    }
    const modified = change(draft);
    if (!modified) return null;
    const pairing = Object.fromEntries((["pairing", "appPairing"] as const).flatMap(slot =>
      link[slot] && !modified[slot] ? [[slot, bodyHash(link[slot].token)]] : []));
    if (bootstrap) link.setup = { ...link.setup, managed: true, checkpointed: true };
    if (!link.setup.checkpointed) {
      const fields = Object.fromEntries(legacyFields.map(key => [key, modified[key] ?? null]));
      const { boxes, left, moved } = primaryPeersBody(modified);
      save = stageShared(link, "membershipAndAccess", {
        epoch: link.setup.epoch, fields, pairing, boxes, left, moved,
        devices: ownPhones(modified),
      }, undefined, operation, undefined);
    } else {
      const companions = together?.(modified);
      const membership = captureMembership(modified, accepted?.boxes.find(box => box.key === link.setup!.primary), accepted ?? undefined);
      const fields = Object.fromEntries(localEffects.filter(key => JSON.stringify(modified[key]) !== JSON.stringify(link[key])).map(key => [key, modified[key] ?? null]));
      if (membership) {
        const changes = companions?.(membership, link);
        const value = changes?.ownershipAssignments?.value ?? (link.sharedSetup === link.setup.id ? link.shared?.ownershipAssignments?.value : undefined);
        const assignments = isObject(value) ? value : {};
        const recoveries = membership.installerRecoveries ?? {};
        const kept = assignments[INSTALLER_RECOVERIES];
        const canCarry = setupMergeLayoutReady(K);
        let withRecoveries = changes;
        if (canCarry) {
          delete membership.installerRecoveries;
          if (JSON.stringify(kept ?? {}) !== JSON.stringify(recoveries)) {
            withRecoveries = { ...changes, ownershipAssignments: { value: { ...assignments, [INSTALLER_RECOVERIES]: recoveries } } };
          }
        } else {
          // Below layout 23 the standby can still accept flat commitments. A
          // previously carried entry must be cleared with a companion save.
          const carried = installerRecoveriesOf(kept, membership.boxes);
          if (Object.entries(carried).some(([key, entry]) => JSON.stringify(entry) !== JSON.stringify(recoveries[key]))) {
            busy = true;
            return null;
          }
          for (const key of Object.keys(carried)) delete membership.installerRecoveries?.[key];
          if (!Object.keys(recoveries).length) delete membership.installerRecoveries;
        }
        save = stageShared(link, "membershipAndAccess", membership, { fields, pairing }, operation, undefined,
          { together: withRecoveries, reportCapacity: !!companions });
      } else if (companions) throw new SharedCapacity("The two setups’ combined membership cannot be accepted.");
    }
    busy = save === null;
    if (save) beforeWrite?.(link);
    return busy ? null : link;
  }, { durable: true });
  return { changed, save: changed ? save : null, busy };
}

export function applyAccess(link: LinkState, value: unknown, effects: unknown): void {
  const membership = acceptedMembership(link);
  if (membership) projectMembership(link, membership);
  const update = membership ? effects : value;
  if (!isObject(update)) return;
  const fields = update.fields;
  if (isObject(fields)) for (const key of membership ? localEffects : legacyFields) {
    if (membership && !Object.hasOwn(fields, key)) continue;
    if (fields[key] == null) delete link[key];
    else Object.assign(link, { [key]: fields[key] });
  }
  for (const slot of ["pairing", "appPairing"] as const) {
    if (isObject(update.pairing) && link[slot] && bodyHash(link[slot].token) === update.pairing[slot]) delete link[slot];
  }
}
