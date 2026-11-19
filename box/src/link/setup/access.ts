// Stage the portable roster and grants. The holder's local work runs only after the receipt.
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { bodyHash } from "../documents/format";
import { primaryPeersBody } from "../documents/work";
import type { Peer } from "../peers/known";
import { ownPhones } from "../phones";
import { updateLink, type LinkState } from "../state";
import { acceptedMembership, membershipOf, projectMembership } from "./membership";
import { requiresStandby } from "./record";
import { pendingShared, stageShared, type Receipt } from "./shared";

const localEffects = DATA_DECLARATIONS.membershipAndAccess.shared.localEffects;
// COMPAT(layout.4 setup): an older standby expects the full projection in its proposal. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
const legacyFields = ["macPeers", "macMoved", "peerNames", "left", "removedHere", "removalRevisions",
  "phones", "removed", "vouchBlocked", ...localEffects,
] as const satisfies readonly (keyof LinkState)[];

export function captureMembership(link: LinkState, self?: Peer) {
  const { boxes, left, moved } = primaryPeersBody(link);
  // A draft starts with the accepted base names. Shared name overrides belong to setupNames.
  const projected = Array.isArray(link.macPeers) ? link.macPeers : [];
  const roster = self ? [self, ...projected] : boxes;
  return membershipOf({
    boxes: roster, left, moved, devices: ownPhones(link), removalRevisions: link.removalRevisions, removedHere: link.removedHere,
    vouchBlocked: link.vouchBlocked,
  });
}

export function updateAccess(change: (link: LinkState | null) => LinkState | null | undefined | void, { localOnly = false } = {}): { changed: LinkState | null; save: Receipt | null; busy: boolean } {
  let save: Receipt | null = null;
  let busy = false;
  const changed = updateLink(link => {
    if (localOnly || link?.setup?.role !== "primary") return change(link);
    if (link.leadershipPending) {
      busy = true;
      return null;
    }
    if (!link.setup.checkpointed && !requiresStandby(link.setup)) return change(link);
    if ((!link.setup.standby && requiresStandby(link.setup)) || pendingShared(link, "membershipAndAccess")) {
      busy = true;
      return null;
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
    if (!link.setup.checkpointed) {
      const fields = Object.fromEntries(legacyFields.map(key => [key, modified[key] ?? null]));
      const { boxes, left, moved } = primaryPeersBody(modified);
      save = stageShared(link, "membershipAndAccess", {
        epoch: link.setup.epoch, fields, pairing, boxes, left, moved,
        devices: ownPhones(modified), vouchBlocked: modified.vouchBlocked ?? [],
      });
    } else {
      const membership = captureMembership(modified, accepted?.boxes.find(box => box.key === link.setup!.primary));
      const fields = Object.fromEntries(localEffects.filter(key => JSON.stringify(modified[key]) !== JSON.stringify(link[key])).map(key => [key, modified[key] ?? null]));
      if (membership) save = stageShared(link, "membershipAndAccess", membership, { fields, pairing });
    }
    busy = save === null;
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
