// Accepted membership is portable. The link fields used by old callers are projections;
// SSH configuration, the removal delivery queue and local pairing tokens stay on their box.
import { isObject } from "../../shared/json";
import type { LinkState, Phone } from "../state";
import type { Peer } from "../peers/known";
import { KEY, cleanMoved, cleanPeer, fitsPeer, ownKey, type MovedSession } from "../peers/known";
import { fits } from "../phones";

import { removalRecords, type MemberRemoval } from "./removal-record";

export interface Membership {
  boxes: Peer[];
  left: string[];
  moved: MovedSession[];
  devices: { phones: Phone[]; removed: { key: string; pairing: string }[] };
  removalRevisions: Record<string, number>;
  vouchBlocked: NonNullable<LinkState["vouchBlocked"]>;
  removals?: Record<string, MemberRemoval>;
}

export function membershipOf(value: unknown): Membership | null {
  if (!isObject(value) || !Array.isArray(value.boxes) || !value.boxes.every(fitsPeer)) return null;
  const boxes = value.boxes.map(box => {
    const { id: _localDestination, ...portable } = cleanPeer(box);
    return portable;
  });
  if (new Set(boxes.map(box => box.key)).size !== boxes.length) return null;
  const devices = isObject(value.devices) ? value.devices : {};
  const revisions = value.removalRevisions;
  const left = Array.isArray(value.left) ? value.left : [];
  const removed = value.removedHere;
  const tombstones = [...new Set([...(Array.isArray(removed) ? removed : []), ...left]
    .filter((key): key is string => typeof key === "string" && KEY.test(key)))];
  const removals = removalRecords(value.removals, new Set(tombstones), new Set(boxes.map(box => box.key)));
  return {
    boxes,
    ...(Object.keys(removals).length ? { removals } : {}),
    left: tombstones,
    moved: cleanMoved(value.moved),
    devices: {
      phones: (Array.isArray(devices.phones) ? devices.phones : []).filter(phone => fits(phone, [])).map(phone => ({ ...phone, pairing: phone.pairing ?? undefined })),
      removed: (Array.isArray(devices.removed) ? devices.removed : []).filter(entry => fits(entry, []) && typeof entry.pairing === "string"),
    },
    removalRevisions: Object.fromEntries(Object.entries(isObject(revisions) ? revisions : {})
      .filter(([key, n]) => KEY.test(key) && Number.isSafeInteger(n) && Number(n) > 0)) as Record<string, number>,
    vouchBlocked: Array.isArray(value.vouchBlocked) ? value.vouchBlocked : [],
  };
}

export function acceptedNames(link: LinkState): Record<string, unknown> {
  const value = link.sharedSetup === link.setup?.id ? link.shared?.setupNames?.value : undefined;
  return isObject(value) ? value : {};
}

export function acceptedName(link: LinkState, key: string | null, fallback: string): string {
  const name = key && (acceptedNames(link)[key] ?? (link.setup?.checkpointed ? undefined : link.peerNames?.[key]));
  return typeof name === "string" && name ? name : fallback;
}

export function acceptedMembership(link: LinkState | null | undefined): Membership | null {
  if (!link?.setup || link.setup.aside || link.sharedSetup !== link.setup.id) return null;
  const value = link.shared?.membershipAndAccess?.value;
  // Before checkpoint negotiation, older peers can still write or receive layout 4 data.
  return isObject(value) && !Object.hasOwn(value, "fields") && link.setup.checkpointed ? membershipOf(value) : null;
}

export function namedRoster(link: LinkState, boxes: Peer[]): Peer[] {
  return boxes.map(box => ({ ...box, name: acceptedName(link, box.key, box.name) }));
}

/** Refresh the compatibility fields from an accepted membership, retaining local metadata. */
export function projectMembership(link: LinkState, membership: Membership, keepLocalPairings = false) {
  const setup = link.setup!;
  const here = setup.role === "primary";
  if (!here && membership.removals?.[ownKey(link) ?? ""]) {
    link.phones = [];
    delete link.pairing;
    delete link.appPairing;
    return;
  }
  const old = new Map((link.phones ?? []).map(phone => [phone.key, phone]));
  const local = (link.phones ?? []).filter(phone => phone.from === "pair" && (phone.local || keepLocalPairings))
    .map((phone): Phone => ({ ...phone, local: true }));
  const transferred = membership.devices.phones.map((phone): Phone => ({
    ...phone, from: here ? "pair" : "mac",
    ...(here ? {} : { mac: setup.primary }),
    added: old.get(phone.key)?.added,
    address: old.get(phone.key)?.address ?? phone.address,
  }));
  link.phones = [...transferred, ...local.filter(phone => !transferred.some(each => each.key === phone.key))];
  link.removed = membership.devices.removed;
  if (here) {
    link.macPeers = membership.boxes.filter(peer => peer.key !== ownKey(link));
    link.macMoved = membership.moved;
    link.left = membership.left.slice(-64);
    link.removedHere = membership.left;
    link.removalRevisions = membership.removalRevisions;
    link.memberRemovals = membership.removals;
    link.vouchBlocked = membership.vouchBlocked;
  }
}
