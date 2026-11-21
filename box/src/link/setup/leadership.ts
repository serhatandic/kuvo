// Who leads a setup: the relay's lease names one computer at a term, and only that computer signs
// the next certificate (PROTOCOL.md, "Your setup"). A certificate binds an epoch to its checkpoint;
// install the certificate and the local role together before replying.

import { setupLayoutDeferred } from "../../shared/folder-version";
import type { Doc } from "../../shared/documents";
import { docKey, held, ownSign } from "../documents/format";
import { cleanPeer, ownKey } from "../peers/known";
import { foldPhones } from "../phones";
import { K } from "../home";
import type { LinkState } from "../state";
import { archiveDeviceRevocations, rememberDeviceRevocations } from "./device-revocations";
import { preserveCheckpoint, resolveCheckpoint } from "./checkpoint";
import { acceptedMembership, projectMembership } from "./membership";
import { installShared } from "./shared";
import { relayLease } from "./lease";
import { ledSetup, setupDoc, setupOf } from "./record";

export const PRIMARY_UNLINK = "Add another computer to this setup before unlinking its main computer.";

export function leadershipOf(link: LinkState | null | undefined): Doc | null {
  const setup = link?.setup;
  return setup?.managed ? held(link?.leadership?.[setup.epoch]) : null;
}

/** A fresh member starts at the position named by the primary's signed SSH roster. */
export function leadershipPosition(link: LinkState) {
  const tip = leadershipOf(link);
  if (tip) return { origin: tip.origin, epoch: tip.n, h: tip.h };
  const setup = link.setup;
  if (!setup?.managed || setup.aside || setup.epoch === 1) return null;
  const roster = setupDoc(link);
  const position = ledSetup(roster);
  if (!roster || roster.origin !== setup.sign || position?.id !== setup.id
    || position.epoch !== setup.epoch || position.primary !== setup.primary) return null;
  return { origin: roster.origin, epoch: position.epoch, h: roster.h };
}

/** An SSH enrollment pins the current certificate in the signed roster. Fetch it without rolling
 * back membership or settings accepted after it. */
function fillEnrollmentCertificate(link: LinkState, doc: Doc, checkpoint?: Doc): boolean {
  const current = link.setup;
  if (!current?.leadershipHash || current.leadershipHash !== doc.h || doc.kind !== "leadership" ||
    current.epoch !== doc.n || held(link.leadership?.[doc.n])) return false;
  if (!acceptedMembership(link)?.boxes.some(peer => peer.key === ownKey(link) && peer.sign === ownSign(link))) return false;
  const position = setupOf(doc.body);
  const resolved = resolveCheckpoint(doc, checkpoint);
  if (!position || position.id !== current.id || position.primary !== current.primary || position.epoch !== current.epoch || !resolved) return false;
  preserveCheckpoint(resolved.checkpoint);
  link.leadership = { ...link.leadership, [doc.n]: doc.envelope };
  return true;
}

/** Take a certificate: the one an SSH roster pins, or a newer one signed by the relay lease's holder. */
export function takeLeadership(link: LinkState, doc: Doc, checkpoint?: Doc): boolean {
  return fillEnrollmentCertificate(link, doc, checkpoint) || installLeadership(link, doc, checkpoint);
}

function installLeadership(link: LinkState, doc: Doc, checkpoint?: Doc): boolean {
  const current = link.setup;
  const next = setupOf(doc.body);
  if (setupLayoutDeferred(K) || doc.kind !== "leadership" || !current || current.aside || !next || next.id !== current.id
    || doc.n !== next.epoch || next.epoch <= current.epoch) return false;
  // The relay's lease is the one authority: its holder, at its term, signs the next certificate.
  const lease = relayLease(link);
  if (!lease || doc.body.relayTerm !== lease.term || next.primary !== lease.holder || doc.origin !== lease.sign) return false;
  const resolved = resolveCheckpoint(doc, checkpoint);
  if (!resolved || resolved.checkpoint.origin !== doc.origin) return false;
  const { membership: body } = resolved;
  const peers = body.boxes.map(cleanPeer);
  if (new Set(peers.map(peer => peer.key)).size !== peers.length) return false;
  const primary = peers.find(peer => peer.key === next.primary);
  const own = ownKey(link);
  if (!primary?.sign || primary.sign !== doc.origin || !own) return false;
  const removed = !peers.some(peer => peer.key === own);
  if (removed && !body.removals?.[own]) {
    // A member offline since before its removal may first hear of it in this certificate.
    if (!body.left.includes(own)) return false;
    archiveDeviceRevocations(link);
    if (body.devices.removed.length) rememberDeviceRevocations(link, current.id, body.devices.removed);
    link.departedKey = own;
    delete link.pairing;
    delete link.appPairing;
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
  preserveCheckpoint(resolved.checkpoint);
  // A member can retain an installer journal for retries after joining. That
  // journal was terminal even on versions that didn't record adoption explicitly.
  if (here && current.role === "member" && link.installerClaim?.setup === current.id) link.installerClaim.adopted = true;
  link.setup = { ...next, sign: primary.sign, role: here ? "primary" : "member", managed: true, checkpointed: true };
  link.leadership = { ...link.leadership, [next.epoch]: doc.envelope };
  delete link.leadershipPending;
  installShared(link, resolved.shared, resolved.outcomes);
  const membership = acceptedMembership(link)!;
  if (!current.checkpointed && link.peerNames) link.seedNames = { setup: current.id, names: link.peerNames };
  delete link.peerNames;
  if (here && current.role === "primary") {
    projectMembership(link, membership);
    return true;
  }
  link.macs = here ? [] : [primary.key];
  link.macSigns = here ? {} : { [primary.key]: primary.sign };
  link.macNames = here ? {} : { [primary.key]: primary.name };
  link.macSeen = here ? {} : { [primary.key]: Date.now() };
  if (here) delete link.mac;
  else link.mac = primary.key;
  if (link.docs) delete link.docs[docKey(primary.sign, "phones")];
  if (next.primary !== current.primary) {
    // Clear the old main's transient state. Accepted removals come back with the checkpoint.
    link.removed = [];
    delete link.untold;
    delete link.untoldMembership;
    delete link.pendingMachineCleanup;
  }
  delete link.pairing;
  delete link.appPairing;
  projectMembership(link, membership, current.role === "member");
  if (!here) {
    // A former primary that is later removed must not start a new setup with an old roster.
    delete link.macPeers;
    delete link.macMoved;
    delete link.left;
    delete link.removedHere;
  }
  foldPhones(link);
  return true;
}
