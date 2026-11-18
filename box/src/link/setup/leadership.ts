// A change is authorized by the preceding epoch's sole successor signer. The installed
// certificate and the local role are written together, before anyone can send the certificate.

import type { Doc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { docBytes, docKey, held, ownSign, readDoc, signDoc } from "../documents/format";
import { cleanMoved, cleanPeer, fitsPeer, ownKey, KEY } from "../peers/known";
import { fits, foldPhones } from "../phones";
import type { LinkState, Phone } from "../state";
import { setupOf } from "./record";

export const NUMBER_RESERVE = 2_000;
export const STANDBY_UNLINK = "Choose another standby first.";
export const PRIMARY_UNLINK = "Choose another primary first.";

export function leadershipOf(link: LinkState | null | undefined): Doc | null {
  const setup = link?.setup;
  return setup?.managed ? held(link?.leadership?.[setup.epoch]) : null;
}

/** Turn a fenced primary's proposal into a certificate, on the current standby only. */
export function approveLeadership(link: LinkState, proposal: Doc): Doc | null {
  const current = link.setup;
  if (!current?.standby || current.standby.key !== ownKey(link) || current.standby.sign !== ownSign(link)
    || current.sign !== proposal.origin || proposal.body.proposal !== true || !link.sign) return null;
  return readDoc(signDoc(link.sign, "leadership", proposal.n, { ...proposal.body, approval: proposal.envelope.s }));
}

/** Install handover, standby assignment/retirement, or takeover under the same epoch check. */
export function takeLeadership(link: LinkState, doc: Doc): boolean {
  const current = link.setup;
  const next = setupOf(doc.body);
  if (doc.kind !== "leadership" || !current || current.aside || !next || next.id !== current.id || doc.n !== next.epoch
    || next.epoch !== current.epoch + 1 || doc.origin !== (current.standby?.sign ?? current.sign)
    || doc.body.previous !== current.primary || !Array.isArray(doc.body.boxes) || !doc.body.boxes.every(fitsPeer)) return false;
  const takeover = doc.body.takeover === true;
  if (takeover) {
    if (!current.standby || next.primary !== current.standby.key || doc.body.proposal) return false;
  } else if (current.standby) {
    const { approval, ...proposal } = doc.body;
    if (proposal.proposal !== true || !readDoc({
      d: docBytes(current.sign, "leadership", doc.n, proposal).toString("base64url"), s: approval,
    })) return false;
  } else if (doc.body.proposal) return false;
  const peers = doc.body.boxes.map(cleanPeer);
  if (new Set(peers.map(peer => peer.key)).size !== peers.length) return false;
  const primary = peers.find(peer => peer.key === next.primary);
  const standby = peers.find(peer => peer.key === doc.body.standby && peer.key !== next.primary && peer.sign !== primary?.sign);
  if (!primary?.sign || !peers.some(peer => peer.key === current.primary && peer.sign === current.sign)
    || (takeover && primary.sign !== current.standby?.sign)
    || (doc.body.standby != null && !standby?.sign)
    || (next.primary === current.primary && (doc.body.standby ?? null) === (current.standby?.key ?? null)
      && !(typeof doc.body.numberCeiling === "number" && doc.body.numberCeiling > (current.numberCeiling ?? 0)))) return false;
  const ceiling = doc.body.numberCeiling;
  if (standby && (typeof ceiling !== "number" || !Number.isSafeInteger(ceiling) || ceiling < 1)) return false;
  const own = ownKey(link);
  if (!own) return false;
  const left = Array.isArray(doc.body.left) ? doc.body.left.filter((key): key is string => typeof key === "string" && KEY.test(key)) : [];
  if (!peers.some(peer => peer.key === own)) {
    // A member offline since before its removal may first hear of it in this certificate.
    if (!left.includes(own)) return false;
    delete link.setup;
    delete link.leadership;
    delete link.leadershipPending;
    link.macs = [];
    delete link.mac;
    delete link.macSigns;
    link.phones = (link.phones ?? []).filter(phone => phone.from === "pair");
    return true;
  }

  const here = own === primary.key;
  if (here && ownSign(link) !== primary.sign) return false;
  // Same-primary transitions preserve local-only state. The certificate already contains
  // the shared state from this primary and supplies it to peers as they catch up.
  const stillHere = here && current.role === "primary";
  link.setup = { ...next, sign: primary.sign, role: here ? "primary" : "member", managed: true,
    ...(standby?.sign ? { standby: { key: standby.key, sign: standby.sign }, numberCeiling: ceiling as number } : {}),
  };
  link.leadership = { ...link.leadership, [next.epoch]: doc.envelope };
  delete link.leadershipPending;
  if (stillHere) return true;
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

  // Keep the installed device grants available while the successor publishes its own list.
  // A former primary must stop treating these pairings as grants it owns.
  const snapshot = isObject(doc.body.devices) ? doc.body.devices : {};
  const local = (link.phones ?? []).filter(phone => phone.from === "pair" && (current.role === "member" || phone.local))
    .map((phone): Phone => ({ ...phone, local: true }));
  const transferred = (Array.isArray(snapshot.phones) ? snapshot.phones : []).filter(phone => fits(phone, [primary.key]))
    .map((phone): Phone => ({ ...phone, pairing: phone.pairing ?? undefined, from: here ? "pair" : "mac", ...(here ? {} : { mac: primary.key }) }));
  const transferredKeys = new Set(transferred.map(phone => phone.key));
  link.phones = [...transferred, ...local.filter(phone => !transferredKeys.has(phone.key))];
  for (const entry of Array.isArray(snapshot.removed) ? snapshot.removed : []) {
    if (fits(entry, []) && typeof entry.pairing === "string" && !link.removed?.some(each => each.key === entry.key && each.pairing === entry.pairing)) {
      (link.removed ??= []).push({ key: entry.key, pairing: entry.pairing });
    }
  }
  if (here) {
    link.macPeers = peers.filter(peer => peer.key !== own);
    link.macMoved = cleanMoved(doc.body.moved);
    link.left = left;
    link.removedHere = left;
    delete link.peerNames;
    link.vouchBlocked = Array.isArray(doc.body.vouchBlocked) ? doc.body.vouchBlocked : [];
  } else {
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
