// Accepted membership is portable. The link fields used by old callers are projections;
// SSH configuration, the removal delivery queue and local pairing tokens stay on their box.
import { historicallyRevoked, rememberDeviceRevocations } from "./device-revocations";
import { deviceGrantsLayoutDeferred, unlinkCleanupLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { K } from "../home";
import type { LinkState, Phone } from "../state";
import type { Peer } from "../peers/known";
import { KEY, cleanMoved, cleanPeer, fitsPeer, ownKey, type MovedSession } from "../peers/known";
import { readComputerHistory, type ComputerHistory } from "./computer-history";
import { fits, type PhoneEntry } from "../phones";

import { REMOVAL_ID, removalRecords, type MemberRemoval, type WipePreparationClaim } from "./removal-record";
import { SETUP_ID } from "./record";
import type { MergePlan } from "./merge-data";

export interface Membership {
  boxes: Peer[];
  left: string[];
  moved: MovedSession[];
  devices: { phones: Phone[]; removed: { key: string; pairing: string }[] };
  removalRevisions: Record<string, number>;
  computerHistory?: ComputerHistory;
  removals?: Record<string, MemberRemoval>;
  wipePreparations?: Record<string, WipePreparationClaim>;
  mergedComputers?: Record<string, MergePlan>;
  installerRecoveries?: Record<string, InstallerRecovery>;
}

/** The public commitment is scoped to one accepted installer enrollment. */
export interface InstallerRecovery {
  invite: string;
  commitment: string;
  sign: string;
  address: string;
}

// Older checkpoint writers normalize the roster and drop unknown fields. They retain
// the accepted assignment map, including entries they do not interpret.
export const INSTALLER_RECOVERIES = "installer recovery";

export function installerRecoveriesOf(value: unknown, boxes: Peer[]): Record<string, InstallerRecovery> {
  const recoveries = isObject(value) ? value : {};
  return Object.fromEntries(boxes.flatMap(({ key, sign, address }) => {
    const entry = recoveries[key];
    return isObject(entry) && typeof entry.invite === "string" && KEY.test(entry.invite)
      && typeof entry.commitment === "string" && KEY.test(entry.commitment)
      && sign && entry.sign === sign && address && entry.address === address
      ? [[key, { invite: entry.invite, commitment: entry.commitment, sign, address }]] : [];
  }));
}

export function acceptedInstallerRecoveries(link: LinkState | null, membership = acceptedMembership(link)) {
  if (!link || !membership || link.sharedSetup !== link.setup?.id) return {};
  const assignments = link.shared?.ownershipAssignments?.value;
  const stored = isObject(assignments) ? assignments[INSTALLER_RECOVERIES] : undefined;
  return { ...membership.installerRecoveries, ...installerRecoveriesOf(stored, membership.boxes) };
}

export function mergePlan(value: unknown): MergePlan | null {
  if (!isObject(value) || typeof value.operation !== "string" || !KEY.test(value.operation) || typeof value.source !== "string" || !SETUP_ID.test(value.source)
    || typeof value.digest !== "string" || !KEY.test(value.digest) || !Number.isSafeInteger(value.sourceHigh) || !Number.isSafeInteger(value.offset)
    || typeof value.sourceName !== "string" || value.sourceName.length > 100
    || Number(value.sourceHigh) < 0 || Number(value.offset) < 0 || Number(value.sourceHigh) + Number(value.offset) > 999_999) return null;
  return { operation: value.operation, source: value.source, digest: value.digest, sourceHigh: Number(value.sourceHigh), sourceName: value.sourceName, offset: Number(value.offset) };
}

// Signed snapshots from older boxes still contain the voucher and provenance fields.
export function portableDevice(phone: PhoneEntry): Phone {
  return {
    key: phone.key,
    name: typeof phone.name === "string" ? phone.name : "",
    pairing: phone.pairing ?? "",
    ...(typeof phone.address === "string" ? { address: phone.address } : {}),
    ...(phone.kind === "desktop" ? { kind: "desktop" as const } : {}),
    ...(typeof phone.by === "string" && KEY.test(phone.by) && deviceGrantsLayoutDeferred(K) ? { by: phone.by } : {}),
  };
}

export function membershipOf(value: unknown): Membership | null {
  if (!isObject(value) || !Array.isArray(value.boxes) || !value.boxes.every(fitsPeer)) return null;
  const boxes = value.boxes.map(box => {
    const { id: _localDestination, ...portable } = cleanPeer(box);
    return portable;
  });
  const boxKeys = new Set(boxes.map(box => box.key));
  if (boxKeys.size !== boxes.length) return null;
  const devices = isObject(value.devices) ? value.devices : {};
  const revisions = value.removalRevisions;
  const left = Array.isArray(value.left) ? value.left : [];
  const removed = value.removedHere;
  const tombstones = [...new Set([...(Array.isArray(removed) ? removed : []), ...left]
    .filter((key): key is string => typeof key === "string" && KEY.test(key)))];
  if (boxes.some(box => tombstones.includes(box.key))) return null;
  const computerHistory = readComputerHistory(value.computerHistory, new Set([...tombstones, ...boxes.map(box => box.key)]), new Set(tombstones));
  if (computerHistory && boxes.some(box => box.sign && computerHistory.retiredSigns.includes(box.sign))) return null;
  const removals = removalRecords(value.removals, new Set(tombstones), new Set(boxes.map(box => box.key)));
  const preparation = isObject(value.wipePreparations) ? value.wipePreparations : {};
  const wipePreparations = Object.fromEntries(boxes.flatMap(({ key }) => {
    const claim = preparation[key];
    return isObject(claim) && typeof claim.operation === "string" && REMOVAL_ID.test(claim.operation) &&
      typeof claim.choiceHash === "string" && /^[A-Za-z0-9_-]{43}$/.test(claim.choiceHash)
      ? [[key, { operation: claim.operation, choiceHash: claim.choiceHash }]] : [];
  }));
  const keys = [...boxKeys];
  const mergeClaims = isObject(value.mergedComputers) ? value.mergedComputers : {};
  const mergedComputers = Object.fromEntries(keys.flatMap(key => {
    const plan = mergePlan(mergeClaims[key]);
    return plan ? [[key, plan]] : [];
  }));
  const installerRecoveries = installerRecoveriesOf(value.installerRecoveries, boxes);
  const revoked = (Array.isArray(devices.removed) ? devices.removed : [])
    .filter(entry => fits(entry, keys) && typeof entry.pairing === "string")
    .map(({ key, pairing }) => ({ key, pairing: pairing as string }));
  const isRevoked = (phone: Phone) => revoked.some(each => each.key === phone.key && each.pairing === (phone.pairing ?? ""));
  return {
    boxes,
    ...(computerHistory ? { computerHistory } : {}),
    ...(Object.keys(wipePreparations).length ? { wipePreparations } : {}),
    ...(Object.keys(removals).length ? { removals } : {}),
    ...(Object.keys(mergedComputers).length ? { mergedComputers } : {}),
    ...(Object.keys(installerRecoveries).length ? { installerRecoveries } : {}),
    left: tombstones,
    moved: cleanMoved(value.moved),
    devices: {
      phones: (Array.isArray(devices.phones) ? devices.phones : []).filter(phone => fits(phone, keys)).map(portableDevice).filter(phone => !isRevoked(phone)),
      removed: revoked,
    },
    removalRevisions: Object.fromEntries(Object.entries(isObject(revisions) ? revisions : {})
      .filter(([key, n]) => KEY.test(key) && Number.isSafeInteger(n) && Number(n) > 0)) as Record<string, number>,
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
  if (!here && !membership.boxes.some(peer => peer.key === ownKey(link))) {
    if (membership.devices.removed.length) rememberDeviceRevocations(link, setup.id, membership.devices.removed);
    link.phones = [];
    delete link.preJoinPairings;
    delete link.pairing;
    delete link.appPairing;
    return;
  }
  const old = new Map((link.phones ?? []).map(phone => [phone.key, phone]));
  const quarantine = unlinkCleanupLayoutReady(K);
  const local = [...(link.preJoinPairings ?? []), ...(link.phones ?? [])]
    .filter(phone => phone.from === "pair" && (phone.local || keepLocalPairings)
    && !membership.devices.removed.some(each => each.key === phone.key && each.pairing === (phone.pairing ?? "")))
    .map((phone): Phone => ({ ...phone, local: true }));
  const transferred = membership.devices.phones.filter(phone => !historicallyRevoked(link, phone.key, phone.pairing ?? "")).map((phone): Phone => ({
    ...phone, from: here ? "pair" : "mac",
    ...(here ? {} : { mac: setup.primary }),
    added: old.get(phone.key)?.added ?? Math.floor(Date.now() / 1000),
    address: old.get(phone.key)?.address ?? phone.address,
  }));
  const unmatched = local.filter(phone => !transferred.some(each => each.key === phone.key
    && (!quarantine || each.pairing === (phone.pairing ?? ""))));
  if (quarantine) {
    link.phones = transferred;
    if (unmatched.length) link.preJoinPairings = unmatched;
    else delete link.preJoinPairings;
  } else {
    link.phones = [...transferred, ...unmatched];
  }
  link.removed = membership.devices.removed;
  if (membership.computerHistory?.setup === setup.id) link.computerHistory = membership.computerHistory;
  else delete link.computerHistory;
  if (here) {
    link.macPeers = membership.boxes.filter(peer => peer.key !== ownKey(link));
    link.macMoved = membership.moved;
    link.left = membership.left.slice(-64);
    link.removedHere = membership.left;
    link.removalRevisions = membership.removalRevisions;
    link.memberRemovals = membership.removals;
    link.wipePreparations = membership.wipePreparations;
    link.mergedComputers = membership.mergedComputers;
    const recoveries = acceptedInstallerRecoveries(link, membership);
    link.installerRecoveries = Object.keys(recoveries).length ? recoveries : undefined;
  }
}
