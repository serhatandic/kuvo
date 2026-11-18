// A change of primary is authorized by the primary of the previous epoch. The installed
// certificate and the local role are written together, before anyone can send the certificate.

import type { Doc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { docKey, held, ownSign } from "../documents/format";
import { cleanMoved, cleanPeer, fitsPeer, ownKey, KEY } from "../peers/known";
import { fits, foldPhones } from "../phones";
import type { LinkState, Phone } from "../state";
import { setupOf } from "./record";

export const PRIMARY_UNLINK = "Choose another primary first.";

export function leadershipOf(link: LinkState | null | undefined): Doc | null {
  const setup = link?.setup;
  return setup?.managed ? held(link?.leadership?.[setup.epoch]) : null;
}

/** Install a verified transition. A future standby can supply another authorization rule
 * here, before using the same epoch checks and state installation. */
export function takeLeadership(link: LinkState, doc: Doc): boolean {
  const current = link.setup;
  const next = setupOf(doc.body);
  if (doc.kind !== "leadership" || !current || !next || next.id !== current.id || doc.n !== next.epoch
    || next.epoch !== current.epoch + 1 || doc.origin !== current.sign || doc.body.previous !== current.primary
    || next.primary === current.primary || !Array.isArray(doc.body.boxes) || !doc.body.boxes.every(fitsPeer)) return false;
  const peers = doc.body.boxes.map(cleanPeer);
  if (new Set(peers.map(peer => peer.key)).size !== peers.length) return false;
  const primary = peers.find(peer => peer.key === next.primary);
  if (!primary?.sign || !peers.some(peer => peer.key === current.primary && peer.sign === current.sign)) return false;
  const own = ownKey(link);
  if (!own) return false;
  const left = Array.isArray(doc.body.left) ? doc.body.left.filter((key): key is string => typeof key === "string" && KEY.test(key)) : [];
  if (!peers.some(peer => peer.key === own)) {
    // A member offline since before its removal may first hear of it in this certificate.
    if (!left.includes(own)) return false;
    delete link.setup;
    delete link.leadership;
    link.macs = [];
    delete link.mac;
    delete link.macSigns;
    link.phones = (link.phones ?? []).filter(phone => phone.from === "pair");
    return true;
  }

  const here = own === primary.key;
  if (here && ownSign(link) !== primary.sign) return false;
  link.setup = { ...next, sign: primary.sign, role: here ? "primary" : "member", managed: true };
  link.leadership = { ...link.leadership, [next.epoch]: doc.envelope };
  link.macs = here ? [] : [primary.key];
  link.macSigns = here ? {} : { [primary.key]: primary.sign };
  link.macNames = here ? {} : { [primary.key]: primary.name };
  link.macSeen = here ? {} : { [primary.key]: Date.now() };
  if (here) delete link.mac;
  else link.mac = primary.key;
  if (link.docs) delete link.docs[docKey(primary.sign, "phones")];
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
