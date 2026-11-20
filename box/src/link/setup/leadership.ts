// A signed certificate binds an epoch to its predecessor and accepted checkpoint.
// Install the certificate and local role together before replying.

import { setupLayoutDeferred, standbyRevisionLayoutReady, successorRankLayoutReady, unlinkCleanupLayoutReady } from "../../shared/folder-version";
import crypto from "node:crypto";
import type { Doc, SignedDoc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { docBytes, docKey, held, ownSign, readDoc, signDoc } from "../documents/format";
import { cleanMoved, cleanPeer, fitsPeer, ownKey, KEY } from "../peers/known";
import { fits, foldPhones } from "../phones";
import { K } from "../home";
import type { LinkState, Phone } from "../state";
import { checksSeal, leadershipDocuments, preserveCheckpoint, resolveCheckpoint, successorPredecessor } from "./checkpoint";
import { acceptedMembership, membershipOf, membershipRevocations, mergeRevocations, portableDevice, projectMembership } from "./membership";
import { committedOperations, installShared, sharedValues } from "./shared";
import { ledSetup, requiresStandby, setupDoc, setupOf } from "./record";

export const NUMBER_RESERVE = 2_000;
export const STANDBY_UNLINK = "Choose another standby first.";
export const PRIMARY_UNLINK = "Choose another main computer first.";

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

export interface ReplacementEnrollment {
  id: string;
  epoch: number;
  key: string;
  proposalHash: string;
}

export const leadershipProposalHash = (signature: string) => crypto.createHash("sha256").update(signature).digest("base64url");

export function replacementOutcome(link: LinkState | null, enrollment: ReplacementEnrollment) {
  const { id, epoch, key, proposalHash } = enrollment;
  const setup = link?.setup;
  if (!link || setup?.id !== id) return "interrupted";
  const certificate = held(link.leadership?.[epoch]);
  const approval = certificate?.body.approval;
  if (certificate?.body.recovery === true && certificate.body.standby === key && setupOf(certificate.body)?.id === id
    && typeof approval === "string" && leadershipProposalHash(approval) === proposalHash) return "committed";
  const pending = held(link.leadershipPending);
  if (setup.epoch === epoch - 1 && pending?.n === epoch && pending.body.standby === key
    && leadershipProposalHash(pending.envelope.s) === proposalHash) return "pending";
  return "interrupted";
}

/** The replacement is a mail peer only for the receipt to this signed recovery proposal. */
export function pendingStandby(link: LinkState) {
  const proposal = held(link.leadershipPending);
  if (proposal?.body.standbyAcceptance !== true || proposal.body.proposal !== true) return undefined;
  return resolveCheckpoint(proposal)?.membership.boxes.find(peer => peer.key === proposal.body.standby);
}

export function canSupersedeStandby(link: LinkState) {
  const setup = link.setup;
  const proposal = held(link.leadershipPending);
  return Boolean(setup && proposal && (setup.successorChain ||
    !setup.standby && proposal.body.successorChain === true && proposal.body.standbyAcceptance === true));
}

/** Turn a fenced primary's proposal into a certificate, on the current standby only. */
export function approveLeadership(link: LinkState, proposal: Doc, checkpoint?: Doc): Doc | null {
  const current = link.setup;
  if (setupLayoutDeferred(K) || !current || proposal.n !== current.epoch + 1 || current.sign !== proposal.origin || proposal.body.proposal !== true || !link.sign) return null;
  const resolved = resolveCheckpoint(proposal, checkpoint);
  if (resolved && !checksSeal(link, resolved.checkpoint)) return null;
  if (proposal.body.standbyAcceptance === true) {
    const replacement = proposal.body.replacesStandby;
    if ((current.standby ? !current.successorChain || replacement !== current.standby.key : replacement !== undefined) ||
      !resolved || (proposal.body.recovery === true && !requiresStandby(current))) return null;
    const successor = resolved.membership.boxes.find(peer => peer.key === proposal.body.standby);
    if (!successor || successor.key !== ownKey(link) || successor.sign !== ownSign(link)) return null;
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
    const proposed = values[kind as keyof typeof values];
    if (resolved && kind === "membershipAndAccess" && proposed) {
      // Older signed checkpoints may still carry rank history. Compare the accepted membership.
      if (JSON.stringify({ ...proposed, value: membershipOf(proposed.value) }) !==
        JSON.stringify({ ...entry, value: membershipOf(entry.value) })) return null;
      continue;
    }
    if (JSON.stringify(proposed) !== JSON.stringify(entry)) return null;
  }
  if (resolved) {
    for (const [operation, outcome] of Object.entries(committedOperations(link))) {
      if (JSON.stringify(resolved.outcomes[operation]) !== JSON.stringify(outcome)) return null;
    }
  }
  return readDoc(signDoc(link.sign, "leadership", proposal.n, { ...proposal.body, approval: proposal.envelope.s }));
}

export function successorHistory(link: LinkState): Doc[] {
  const epoch = link.setup?.epoch;
  if (!epoch || !link.setup?.successorChain) return [];
  const epochs = Object.keys(link.leadership ?? {}).map(Number).sort((a, b) => b - a);
  const recent = epochs.slice(0, 15);
  const activation = epochs.findLast(n => held(link.leadership?.[n])?.body.successorChain === true);
  if (activation && !recent.includes(activation)) recent.push(activation);
  return recent.flatMap(n => { const doc = held(link.leadership?.[n]); return doc ? [doc] : []; });
}

export function successorCertificates(link: LinkState, remote: Record<number, string>): SignedDoc[] {
  const local = link.setup?.epoch ?? 0;
  const epochs = Object.keys(remote).map(Number).filter(n => Number.isSafeInteger(n) && n > 0);
  const latest = Math.max(0, ...epochs);
  if (!link.setup?.successorChain || !latest || local < latest || leadershipOf(link)?.h === remote[local]) return [];
  const agreed = Math.max(0, ...epochs.filter(n => held(link.leadership?.[n])?.h === remote[n]));
  const from = agreed || Math.max(1, latest - 1);
  const docs: SignedDoc[] = [];
  for (let n = from + 1; n <= local && n <= from + 16; n++) {
    const envelope = link.leadership?.[n];
    if (envelope) docs.push(...leadershipDocuments(envelope));
  }
  return docs;
}

export type SuccessorForks = Map<string, { source: LinkState; rank: string[] }>;

function stagedFork(link: LinkState, doc: Doc, checkpoint: Doc | undefined, forks: SuccessorForks) {
  const next = setupOf(doc.body);
  const parent = doc.body.parent;
  if (!next || !link.setup || next.id !== link.setup.id || typeof parent !== "string") return null;
  let prior = forks.get(parent);
  if (!prior) {
    const base = successorPredecessor(link, doc.n - 1);
    if (!base || parent !== base.parent.h) return null;
    const { position, primary, standby, ceiling } = base;
    const previous = structuredClone(link);
    previous.setup = { ...position, sign: primary.sign!, role: ownKey(link) === primary.key ? "primary" : "member",
      managed: true, checkpointed: true, successorChain: true, sharedSaves: true,
      ...(standby?.sign && ceiling ? { standby: { key: standby.key, sign: standby.sign }, numberCeiling: ceiling } : {}) };
    previous.shared = base.shared;
    delete previous.sharedOutcomes;
    delete previous.setupRevocations;
    previous.leadership = Object.fromEntries(Object.entries(previous.leadership ?? {}).filter(([n]) => Number(n) <= position.epoch));
    prior = { source: previous, rank: base.membership.successorOrder! };
  }
  // The replay cache supplies only setup authority. Keep local changes made while this fork was arriving.
  const candidate = structuredClone(link);
  candidate.setup = prior.source.setup;
  candidate.leadership = prior.source.leadership;
  candidate.sharedSetup = prior.source.sharedSetup;
  candidate.shared = prior.source.shared;
  candidate.sharedOutcomes = prior.source.sharedOutcomes;
  delete candidate.setupRevocations;
  if (!installLeadership(candidate, doc, checkpoint, true)) return null;
  const staged = { source: candidate, rank: prior.rank };
  if (!forks.has(doc.h) && forks.size >= 16) forks.delete(forks.keys().next().value!);
  forks.set(doc.h, staged);
  return staged;
}

/** An SSH enrollment pins the current certificate in the signed roster. Fetch its
 * history without rolling back membership or settings accepted after that certificate. */
function fillEnrollmentCertificate(link: LinkState, doc: Doc, checkpoint?: Doc): boolean {
  const current = link.setup;
  if (!current?.leadershipHash || current.leadershipHash !== doc.h || doc.kind !== "leadership" ||
    current.epoch !== doc.n || held(link.leadership?.[doc.n]) || doc.body.successorChain !== true) return false;
  if (!acceptedMembership(link)?.boxes.some(peer => peer.key === ownKey(link) && peer.sign === ownSign(link))) return false;
  const position = setupOf(doc.body);
  const resolved = resolveCheckpoint(doc, checkpoint);
  if (!position || position.id !== current.id || position.primary !== current.primary || position.epoch !== current.epoch || !resolved) return false;
  preserveCheckpoint(resolved.checkpoint);
  link.leadership = { ...link.leadership, [doc.n]: doc.envelope };
  return true;
}

const signature = (value: unknown): value is string => typeof value === "string" && /^[\w-]{86}$/.test(value);

/** First assignments carry the primary's approval of an exact proposal. The same
 * primary can replace it without changing the epoch or the predecessor. */
export function firstStandbyRevision(doc: Doc | null) {
  if (doc?.kind !== "leadership" || doc.body.proposal !== true || doc.body.standbyAcceptance !== true ||
    doc.body.successorChain !== true || doc.body.replacesStandby !== undefined || doc.body.takeover === true) return undefined;
  const revision = doc.body.assignmentRevision;
  if (revision === undefined) return signature(doc.body.supersedes) ? 1 : 0;
  return typeof revision === "number" && Number.isSafeInteger(revision) && revision > 0 &&
    signature(doc.body.supersedes) ? revision : undefined;
}

function supersededFirstStandby(link: LinkState, active: Doc, doc: Doc, checkpoint?: Doc): LinkState | null {
  if (!standbyRevisionLayoutReady(K) || active.n !== doc.n || active.body.parent !== doc.body.parent ||
    !signature(active.body.approval) || !signature(doc.body.approval)) return null;
  const priorRevision = firstStandbyRevision(active);
  const revision = firstStandbyRevision(doc);
  if (priorRevision === undefined || revision === undefined ||
    (revision <= priorRevision && doc.body.supersedes !== active.body.approval)) return null;
  const current = link.setup!;
  const next = setupOf(doc.body);
  if (!next || next.id !== current.id || next.primary !== current.primary || doc.body.previous !== current.primary) return null;

  // The locally accepted first assignment pins the primary's signing key even
  // when the previous epoch had no checkpoint. Reuse the normal signature and
  // checkpoint verification from that position.
  const candidate = structuredClone(link);
  candidate.setup = { id: current.id, primary: current.primary, epoch: doc.n - 1, sign: current.sign,
    role: current.role, managed: true, checkpointed: true, sharedSaves: true,
    ...(typeof active.body.parent === "string" ? { successorChain: true, leadershipHash: active.body.parent } : {}) };
  candidate.leadership = Object.fromEntries(Object.entries(candidate.leadership ?? {}).filter(([n]) => Number(n) < doc.n));
  delete candidate.sharedOutcomes;
  delete candidate.setupRevocations;
  return installLeadership(candidate, doc, checkpoint, true) ? candidate : null;
}

/** Only a verified competing chain can replace the accepted tip. */
export function takeLeadership(link: LinkState, doc: Doc, checkpoint?: Doc, forks: SuccessorForks = new Map()): boolean {
  if (fillEnrollmentCertificate(link, doc, checkpoint)) return true;
  const old = link.setup?.successorChain ? acceptedMembership(link) : null;
  if (installLeadership(link, doc, checkpoint)) {
    const merged = acceptedMembership(link);
    if (old && merged && link.setup && mergeRevocations(link, membershipRevocations(link.setup.id, old))) {
      const membership = acceptedMembership(link);
      if (membership) projectMembership(link, membership);
    }
    return true;
  }
  const active = leadershipOf(link);
  if (!active || !link.setup?.successorChain || doc.body.successorChain !== true ||
    held(link.leadership?.[doc.n])?.h === doc.h) return false;
  const next = setupOf(doc.body);
  if (!next) return false;
  let source: LinkState;
  if (next.epoch === link.setup.epoch && next.primary === link.setup.primary) {
    // A standby acceptance does not create a competing primary. Only a newer
    // assignment signed by this primary can replace the accepted certificate.
    const replacement = supersededFirstStandby(link, active, doc, checkpoint);
    if (!replacement) return false;
    source = replacement;
  } else {
    const staged = stagedFork(link, doc, checkpoint, forks);
    if (!staged || next.epoch < link.setup.epoch) return false;
    if (next.epoch === link.setup.epoch) {
      const rank = (key: string) => {
        const index = staged.rank.indexOf(key);
        return index < 0 ? staged.rank.length : index;
      };
      const difference = rank(next.primary) - rank(link.setup.primary);
      if (difference > 0 || (difference === 0 && next.primary > link.setup.primary)) return false;
    }
    source = staged.source;
  }
  const losing = acceptedMembership(link);
  if (losing) mergeRevocations(source, membershipRevocations(next.id, losing));
  const merged = acceptedMembership(source);
  if (merged) projectMembership(source, merged);
  for (const key of Object.keys(link) as (keyof LinkState)[]) if (!Object.hasOwn(source, key)) delete link[key];
  Object.assign(link, source);
  return true;
}

function installLeadership(link: LinkState, doc: Doc, checkpoint?: Doc, discardingFork = false): boolean {
  const current = link.setup;
  const next = setupOf(doc.body);
  const acceptance = doc.body.standbyAcceptance === true;
  if (doc.body.assignmentRevision !== undefined && (current?.standby || firstStandbyRevision(doc) === undefined)) return false;
  const takeover = doc.body.takeover === true;
  const chained = doc.body.successorChain === true;
  if (current?.successorChain && !chained || chained && !successorRankLayoutReady(K)) return false;
  const prior = leadershipOf(link)?.h ?? current?.leadershipHash;
  if (current?.successorChain && prior && doc.body.parent !== prior) return false;
  const ranked = takeover && current?.successorChain && acceptedMembership(link)?.boxes.find(peer =>
    peer.key === next?.primary && peer.sign === doc.origin && peer.key !== current.primary);
  if (doc.body.recovery === true && !acceptance) return false;
  if (setupLayoutDeferred(K) || doc.kind !== "leadership" || !current || current.aside || !next || next.id !== current.id || doc.n !== next.epoch
    || next.epoch !== current.epoch + 1 || (!acceptance && !ranked && doc.origin !== (current.standby?.sign ?? current.sign))
    || doc.body.previous !== current.primary) return false;
  const resolved = resolveCheckpoint(doc, checkpoint);
  if (resolved && !checksSeal(link, resolved.checkpoint)) return false;
  if ((chained || current.checkpointed || Object.hasOwn(doc.body, "checkpoint")) && !resolved) return false;
  if (chained && !resolved?.membership.successorOrder) return false;
  const body = resolved?.membership ?? doc.body;
  if (!Array.isArray(body.boxes) || !body.boxes.every(fitsPeer)) return false;
  const shared = resolved?.shared ?? sharedValues(doc.body.shared);
  if (doc.body.shared !== undefined && !shared) return false;
  if (takeover) {
    if (doc.body.proposal || (current.successorChain ? !ranked || resolved?.checkpoint.origin !== doc.origin :
      next.primary !== current.standby?.key)) return false;
  } else if (current.standby || acceptance) {
    const { approval, ...proposal } = doc.body;
    if (proposal.proposal !== true || !readDoc({
      d: docBytes(current.sign, "leadership", doc.n, proposal).toString("base64url"), s: approval,
    })) return false;
  } else if (doc.body.proposal) return false;
  const peers = body.boxes.map(cleanPeer);
  if (new Set(peers.map(peer => peer.key)).size !== peers.length) return false;
  const retired = doc.body.retiredStandby;
  if (retired !== undefined && (!unlinkCleanupLayoutReady(K) || takeover || acceptance || !resolved ||
    !current.standby || retired !== current.standby.key || next.primary !== current.primary || doc.body.standby !== undefined
    || peers.length !== 2 || !peers.some(peer => peer.key === retired && peer.sign === current.standby!.sign))) return false;
  const primary = peers.find(peer => peer.key === next.primary);
  const standby = peers.find(peer => peer.key === doc.body.standby && peer.key !== next.primary && peer.sign !== primary?.sign);
  const retiring = doc.body.retiringStandby;
  if (retiring !== undefined && (takeover || acceptance || retiring !== current.standby?.key ||
    next.primary !== current.primary || !standby || standby.key === retiring)) return false;
  if (doc.body.pinAfterLease !== undefined && (doc.body.pinAfterLease !== true || !acceptance || current.standby || !standby)) return false;
  const replacement = doc.body.replacesStandby;
  if (replacement !== undefined && (!acceptance || !current.successorChain || replacement !== current.standby?.key || standby?.key === replacement)) return false;
  if (acceptance && (takeover || !resolved || current.standby && replacement === undefined || (doc.body.recovery === true && !requiresStandby(current))
    || next.primary !== current.primary || standby?.sign !== doc.origin
    || (!discardingFork && current.role === "primary" && held(link.leadershipPending)?.envelope.s !== doc.body.approval))) return false;
  if (!primary?.sign || !peers.some(peer => peer.key === current.primary && peer.sign === current.sign) && !(ranked && resolved?.membership.left.includes(current.primary))
    || (takeover && primary.sign !== doc.origin)
    || (doc.body.standby != null && !standby?.sign)
    || (next.primary === current.primary && (doc.body.standby ?? null) === (current.standby?.key ?? null)
      && !(resolved && !current.checkpointed)
      && !(chained && !current.successorChain)
      && typeof doc.body.supersedes !== "string"
      && !(typeof doc.body.numberCeiling === "number" && doc.body.numberCeiling > (current.numberCeiling ?? 0)))) return false;
  const ceiling = doc.body.numberCeiling;
  if (standby && (typeof ceiling !== "number" || !Number.isSafeInteger(ceiling) || ceiling < 1)) return false;
  const own = ownKey(link);
  if (!own) return false;
  const left = Array.isArray(body.left) ? body.left.filter((key): key is string => typeof key === "string" && KEY.test(key)) : [];
  const removed = !peers.some(peer => peer.key === own);
  const deliveringRemoval = removed && Boolean(resolved?.membership.removals?.[own]);
  if (removed && !deliveringRemoval) {
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
  const stillHere = here && current.role === "primary" && !discardingFork;
  if (resolved) preserveCheckpoint(resolved.checkpoint);
  link.setup = { ...next, ...((resolved || current.checkpointed) ? { checkpointed: true as const } : {}), sign: primary.sign, role: here ? "primary" : "member", managed: true,
    ...(chained ? { successorChain: true as const } : {}),
    ...(retired === undefined && (requiresStandby(current) || standby || doc.body.sharedSaves === true) ? { sharedSaves: true as const } : {}),
    ...(standby?.sign ? { standby: { key: standby.key, sign: standby.sign }, numberCeiling: ceiling as number } : {}),
  };
  link.leadership = { ...link.leadership, [next.epoch]: doc.envelope };
  delete link.leadershipPending;
  installShared(link, shared ?? undefined, resolved?.outcomes);
  const membership = acceptedMembership(link);
  if (!current.checkpointed && membership && link.peerNames) link.seedNames = { setup: current.id, names: link.peerNames };
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
  if (takeover || discardingFork) {
    // Clear transient losing state. Accepted removals are restored by the checkpoint and revocation union.
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
  } else if (!here) {
    // A former primary that is later removed must not start a new setup with an old roster.
    delete link.macPeers;
    delete link.macMoved;
    delete link.left;
    delete link.removedHere;
    delete link.peerNames;
  }
  foldPhones(link);
  return true;
}

// COMPAT(layout.4 setup): devices were embedded beside the roster in the certificate. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
function projectLegacyDevices(link: LinkState, value: unknown, primary: string, here: boolean, wasMember: boolean) {
  const snapshot = isObject(value) ? value : {};
  const local = (link.phones ?? []).filter(phone => phone.from === "pair" && (wasMember || phone.local))
    .map((phone): Phone => ({ ...phone, local: true }));
  const transferred = (Array.isArray(snapshot.phones) ? snapshot.phones : []).filter(phone => fits(phone, [primary]))
    .map((phone): Phone => ({ ...portableDevice(phone), from: here ? "pair" : "mac", ...(here ? {} : { mac: primary }) }));
  const transferredKeys = new Set(transferred.map(phone => phone.key));
  link.phones = [...transferred, ...local.filter(phone => !transferredKeys.has(phone.key))];
  for (const entry of Array.isArray(snapshot.removed) ? snapshot.removed : []) {
    if (fits(entry, []) && typeof entry.pairing === "string" && !link.removed?.some(each => each.key === entry.key && each.pairing === entry.pairing)) {
      (link.removed ??= []).push({ key: entry.key, pairing: entry.pairing });
    }
  }
}
