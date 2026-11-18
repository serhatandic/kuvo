// A change is authorized by the preceding epoch's sole successor signer. The installed
// certificate and the local role are written together, before anyone can send the certificate.

import type { Doc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { docBytes, docKey, held, ownSign, readDoc, signDoc } from "../documents/format";
import { cleanMoved, cleanPeer, fitsPeer, ownKey, KEY } from "../peers/known";
import { fits, foldPhones } from "../phones";
import type { LinkState, Phone } from "../state";
import { preserveCheckpoint, resolveCheckpoint } from "./checkpoint";
import { acceptedMembership, projectMembership } from "./membership";
import { committedOperations, installShared, sharedValues } from "./shared";
import { requiresStandby, setupOf } from "./record";

export const NUMBER_RESERVE = 2_000;
export const STANDBY_UNLINK = "Choose another standby first.";
export const PRIMARY_UNLINK = "Choose another primary first.";

export function leadershipOf(link: LinkState | null | undefined): Doc | null {
  const setup = link?.setup;
  return setup?.managed ? held(link?.leadership?.[setup.epoch]) : null;
}

/** The replacement is a mail peer only for the receipt to this signed recovery proposal. */
export function pendingStandby(link: LinkState) {
  const proposal = held(link.leadershipPending);
  if (proposal?.body.standbyAcceptance !== true || proposal.body.proposal !== true) return undefined;
  return resolveCheckpoint(proposal)?.membership.boxes.find(peer => peer.key === proposal.body.standby);
}

/** Turn a fenced primary's proposal into a certificate, on the current standby only. */
export function approveLeadership(link: LinkState, proposal: Doc, checkpoint?: Doc): Doc | null {
  const current = link.setup;
  if (!current || current.sign !== proposal.origin || proposal.body.proposal !== true || !link.sign) return null;
  const resolved = resolveCheckpoint(proposal, checkpoint);
  if (proposal.body.standbyAcceptance === true) {
    if (current.standby || !resolved || (proposal.body.recovery === true && !requiresStandby(current))) return null;
    const replacement = resolved.membership.boxes.find(peer => peer.key === proposal.body.standby);
    if (!replacement || replacement.key !== ownKey(link) || replacement.sign !== ownSign(link)) return null;
    // The caller installs the verified checkpoint and epoch durably before sending this certificate.
    return readDoc(signDoc(link.sign, "leadership", proposal.n, { ...proposal.body, approval: proposal.envelope.s }));
  }
  if (current.standby?.key !== ownKey(link) || current.standby?.sign !== ownSign(link)) return null;
  if ((current.checkpointed || Object.hasOwn(proposal.body, "checkpoint")) && !resolved) return null;
  const values = resolved?.shared ?? sharedValues(proposal.body.shared) ?? {};
  for (const [kind, entry] of Object.entries(link.shared ?? {})) {
    if (resolved && kind === "membershipAndAccess" && !current.checkpointed) continue;
    if (resolved && kind === "queueNumberAllocation") {
      const ledger = entry.value;
      const next = values.queueNumberAllocation?.value;
      if (!isObject(ledger) || !isObject(next) || Number(next.next) < Number(ledger.next)
        || JSON.stringify(next.given ?? {}) !== JSON.stringify(ledger.given ?? {})
        || JSON.stringify(next.granted ?? {}) !== JSON.stringify(ledger.granted ?? {})) return null;
      continue;
    }
    if (JSON.stringify(values[kind as keyof typeof values]) !== JSON.stringify(entry)) return null;
  }
  if (resolved) {
    for (const [operation, outcome] of Object.entries(committedOperations(link))) {
      if (JSON.stringify(resolved.outcomes[operation]) !== JSON.stringify(outcome)) return null;
    }
  }
  return readDoc(signDoc(link.sign, "leadership", proposal.n, { ...proposal.body, approval: proposal.envelope.s }));
}

/** Install handover, standby assignment/retirement, or takeover under the same epoch check. */
export function takeLeadership(link: LinkState, doc: Doc, checkpoint?: Doc): boolean {
  const current = link.setup;
  const next = setupOf(doc.body);
  const acceptance = doc.body.standbyAcceptance === true;
  if (doc.body.recovery === true && !acceptance) return false;
  if (doc.kind !== "leadership" || !current || current.aside || !next || next.id !== current.id || doc.n !== next.epoch
    || next.epoch !== current.epoch + 1 || (!acceptance && doc.origin !== (current.standby?.sign ?? current.sign))
    || doc.body.previous !== current.primary) return false;
  const resolved = resolveCheckpoint(doc, checkpoint);
  if ((current.checkpointed || Object.hasOwn(doc.body, "checkpoint")) && !resolved) return false;
  const body = resolved?.membership ?? doc.body;
  if (!Array.isArray(body.boxes) || !body.boxes.every(fitsPeer)) return false;
  const shared = resolved?.shared ?? sharedValues(doc.body.shared);
  if (doc.body.shared !== undefined && !shared) return false;
  const takeover = doc.body.takeover === true;
  if (takeover) {
    if (!current.standby || next.primary !== current.standby.key || doc.body.proposal) return false;
  } else if (current.standby || acceptance) {
    const { approval, ...proposal } = doc.body;
    if (proposal.proposal !== true || !readDoc({
      d: docBytes(current.sign, "leadership", doc.n, proposal).toString("base64url"), s: approval,
    })) return false;
  } else if (doc.body.proposal) return false;
  const peers = body.boxes.map(cleanPeer);
  if (new Set(peers.map(peer => peer.key)).size !== peers.length) return false;
  const primary = peers.find(peer => peer.key === next.primary);
  const standby = peers.find(peer => peer.key === doc.body.standby && peer.key !== next.primary && peer.sign !== primary?.sign);
  if (acceptance && (takeover || !resolved || current.standby || (doc.body.recovery === true && !requiresStandby(current))
    || next.primary !== current.primary || standby?.sign !== doc.origin
    || (current.role === "primary" && held(link.leadershipPending)?.envelope.s !== doc.body.approval))) return false;
  if (!primary?.sign || !peers.some(peer => peer.key === current.primary && peer.sign === current.sign)
    || (takeover && primary.sign !== current.standby?.sign)
    || (doc.body.standby != null && !standby?.sign)
    || (next.primary === current.primary && (doc.body.standby ?? null) === (current.standby?.key ?? null)
      && !(resolved && !current.checkpointed)
      && !(typeof doc.body.numberCeiling === "number" && doc.body.numberCeiling > (current.numberCeiling ?? 0)))) return false;
  const ceiling = doc.body.numberCeiling;
  if (standby && (typeof ceiling !== "number" || !Number.isSafeInteger(ceiling) || ceiling < 1)) return false;
  const own = ownKey(link);
  if (!own) return false;
  const left = Array.isArray(body.left) ? body.left.filter((key): key is string => typeof key === "string" && KEY.test(key)) : [];
  if (!peers.some(peer => peer.key === own)) {
    // A member offline since before its removal may first hear of it in this certificate.
    if (!left.includes(own)) return false;
    delete link.setup;
    delete link.leadership;
    delete link.leadershipPending;
    installShared(link, undefined);
    link.macs = [];
    delete link.mac;
    delete link.macSigns;
    link.phones = (link.phones ?? []).filter(phone => phone.from === "pair");
    return true;
  }

  const here = own === primary.key;
  if (here && ownSign(link) !== primary.sign) return false;
  // Same-primary transitions leave local effects on the primary.
  const stillHere = here && current.role === "primary";
  if (resolved) preserveCheckpoint(resolved.checkpoint);
  link.setup = { ...next, ...((resolved || current.checkpointed) ? { checkpointed: true as const } : {}), sign: primary.sign, role: here ? "primary" : "member", managed: true,
    ...(requiresStandby(current) || standby || doc.body.sharedSaves === true ? { sharedSaves: true as const } : {}),
    ...(standby?.sign ? { standby: { key: standby.key, sign: standby.sign }, numberCeiling: ceiling as number } : {}),
  };
  link.leadership = { ...link.leadership, [next.epoch]: doc.envelope };
  delete link.leadershipPending;
  installShared(link, shared ?? undefined, resolved?.outcomes);
  const membership = acceptedMembership(link);
  if (membership) delete link.peerNames;
  if (stillHere) {
    if (membership) projectMembership(link, membership);
    return true;
  }
  link.macs = here ? [] : [primary.key];
  link.macSigns = here ? {} : { [primary.key]: primary.sign };
  link.macNames = here ? {} : { [primary.key]: primary.name };
  link.macSeen = here ? {} : { [primary.key]: Date.now() };
  if (here) delete link.mac;
  else link.mac = primary.key;
  if (link.docs) delete link.docs[docKey(primary.sign, "phones")];
  if (takeover) {
    // All shared device removals come from the winning snapshot. Any disconnected
    // primary's interim removals must not silently veto grants on the new primary.
    link.removed = [];
    delete link.untold;
    delete link.untoldMembership;
    delete link.pendingMachineCleanup;
  }
  delete link.pairing;
  delete link.appPairing;

  if (membership) projectMembership(link, membership, current.role === "member");
  else projectLegacyDevices(link, doc.body.devices, primary.key, here, current.role === "member");
  if (here && !membership) {
    link.macPeers = peers.filter(peer => peer.key !== own);
    link.macMoved = cleanMoved(doc.body.moved);
    link.left = left;
    link.removedHere = left;
    if (isObject(doc.body.removalRevisions)) {
      link.removalRevisions = Object.fromEntries(Object.entries(doc.body.removalRevisions)
        .filter(([key, n]) => KEY.test(key) && Number.isSafeInteger(n) && Number(n) > 0)) as Record<string, number>;
    }
    delete link.peerNames;
    link.vouchBlocked = Array.isArray(doc.body.vouchBlocked) ? doc.body.vouchBlocked : [];
  } else if (!here) {
    // A former primary that is later removed must not start a new setup with an old roster.
    delete link.macPeers;
    delete link.macMoved;
    delete link.left;
    delete link.removedHere;
    delete link.peerNames;
    delete link.vouchBlocked;
  }
  foldPhones(link);
  return true;
}

// COMPAT(layout 4): devices were embedded beside the roster in the certificate.
function projectLegacyDevices(link: LinkState, value: unknown, primary: string, here: boolean, wasMember: boolean) {
  const snapshot = isObject(value) ? value : {};
  const local = (link.phones ?? []).filter(phone => phone.from === "pair" && (wasMember || phone.local))
    .map((phone): Phone => ({ ...phone, local: true }));
  const transferred = (Array.isArray(snapshot.phones) ? snapshot.phones : []).filter(phone => fits(phone, [primary]))
    .map((phone): Phone => ({ ...phone, pairing: phone.pairing ?? undefined, from: here ? "pair" : "mac", ...(here ? {} : { mac: primary }) }));
  const transferredKeys = new Set(transferred.map(phone => phone.key));
  link.phones = [...transferred, ...local.filter(phone => !transferredKeys.has(phone.key))];
  for (const entry of Array.isArray(snapshot.removed) ? snapshot.removed : []) {
    if (fits(entry, []) && typeof entry.pairing === "string" && !link.removed?.some(each => each.key === entry.key && each.pairing === entry.pairing)) {
      (link.removed ??= []).push({ key: entry.key, pairing: entry.pairing });
    }
  }
}
