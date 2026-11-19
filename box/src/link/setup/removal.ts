// Delivery is separate from authorization: the accepted roster has already revoked the peer.
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { removalLayoutReady } from "../../shared/folder-version";
import { ownSign, readDoc, signDoc } from "../documents/format";
import { docMails, type DocsState } from "../documents/work";
import { noteDocsChanged } from "../documents/changed";
import { K } from "../home";
import { mailFor, ownKey, peersOf, type Peer } from "../peers/known";
import { writeAgents } from "../peers/agents";
import { assemble, type PendingParts } from "../documents/parts";
import type { LinkHost } from "../relay/host";
import { liveLink, readLink, updateLink, type LinkState } from "../state";
import { updateAccess } from "./access";
import { writeCard } from "./card";
import { cacheCheckpoint, leadershipDocuments, checkpointHash } from "./checkpoint";
import { primaryFenced } from "./lease";
import { takeLeadership } from "./leadership";
import { scheduleWipe } from "./wipe";
import { followedSetup, newSetupID } from "./record";
import { REMOVAL_ID, removalsOf, type MemberRemoval } from "./removal-record";

type Host = Pick<LinkHost, "welcomed" | "send" | "keepOnly">;
const challengeLifetime = 30_000;
let challenge: { setup: string; epoch: number; primary: string; operation: string; nonce: string; expires: number } | undefined;
const parts = new Map<string, PendingParts>();
const checkpoints: NonNullable<DocsState["checkpoints"]> = new Map();

export function sendRemoval(host: Host | null | undefined, peer: Pick<Peer, "key" | "address">, link: LinkState, message: object): void {
  if (!host?.welcomed || !link.key) return;
  const mail = mailFor(peer, message, link, true);
  if (mail) try { host.send(mail); } catch {}
}

function primary(link: LinkState | null) {
  const setup = followedSetup(link);
  return setup?.role === "primary" && link?.sign && !primaryFenced(link) && !link.localWipe ? setup : null;
}

function saveDelivery(key: string, operation: string): void {
  const result = updateAccess(draft => {
    const record = draft?.memberRemovals?.[key];
    if (!draft || !record || record.operation !== operation || record.delivered) return null;
    draft.memberRemovals = { ...draft.memberRemovals, [key]: { ...record, delivered: true } };
    return draft;
  });
  if (result.save) noteDocsChanged();
}

export function sendRemovalUpdates(host = liveLink()): void {
  if (!removalLayoutReady(K) || !host?.welcomed) return;
  const link = readLink();
  const setup = primary(link);
  if (!link?.sign || !setup) return;
  const removals = removalsOf(link);
  for (const [key, operation] of Object.entries(link.removalAcks ?? {})) {
    if (removals[key]?.operation === operation && !removals[key]?.delivered) saveDelivery(key, operation);
  }
  for (const record of Object.values(removals)) {
    if (record.delivered || link.removalAcks?.[record.peer.key] === record.operation) continue;
    const { peer, operation } = record;
    const offer = signDoc(link.sign, "setup-shared", setup.epoch, {
      removal: "offer", setup: setup.id, primary: setup.primary, key: peer.key, operation,
    });
    sendRemoval(host, peer, link, { offer });
  }
}

function receiveAuthority(sender: string, message: Record<string, unknown>, link: LinkState) {
  const setup = followedSetup(link);
  if (parts.size >= 16) parts.delete(parts.keys().next().value!);
  const doc = readDoc(message.authority ?? assemble(parts, sender, message.authorityPart));
  if (setup?.role !== "member" || !doc || (doc.kind !== "setup-checkpoint" && doc.kind !== "leadership")) return;
  if (doc.n !== setup.epoch + 1 && !(doc.kind === "setup-checkpoint" && doc.body.archive === true)) return;
  const changed = updateLink(current => {
    if (!current || current.setup?.id !== setup.id || current.setup.epoch !== setup.epoch || current.localWipe) return null;
    if (doc.kind === "setup-checkpoint") {
      cacheCheckpoint(current, checkpoints, doc);
      return null;
    }
    const hash = doc.body.checkpoint;
    const checkpoint = typeof hash === "string" ? checkpoints.get(hash) : undefined;
    if (!takeLeadership(current, doc, checkpoint)) return null;
    if (checkpoint) checkpoints.delete(checkpointHash(checkpoint));
    return current;
  }, { durable: true });
  if (changed) writeCard();
}

function takeInstruction(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState) {
  const setup = followedSetup(link);
  const proof = readDoc(message.instruction);
  const own = ownKey(link);
  const asked = challenge;
  const their = proof?.body;
  if (!own || setup?.role !== "member" || !proof || proof.kind !== "setup-shared" || proof.origin !== setup.sign
    || proof.n !== setup.epoch || sender !== setup.primary || !asked || Date.now() > asked.expires
    || asked.setup !== setup.id || asked.epoch !== setup.epoch || asked.primary !== sender
    || their?.removal !== "instruction" || their.setup !== setup.id || their.primary !== sender || their.key !== own
    || their.operation !== asked.operation || their.nonce !== asked.nonce || typeof their.wipe !== "boolean") return;
  const peer = peersOf(link).find(peer => peer.key === sender);
  if (!peer?.sign) return;
  challenge = undefined;
  const operation = asked.operation;
  const changed = updateLink(current => {
    const position = followedSetup(current);
    if (!current || position?.id !== setup.id || position.epoch !== setup.epoch || position.primary !== sender
      || position.sign !== proof.origin || ownKey(current) !== own || current.localWipe) return null;
    if (their.wipe) {
      current.localWipe = { setup: setup.id, operation, primary: peer };
      current.phones = [];
      delete current.pairing;
      delete current.appPairing;
      return current;
    }
    // Drop the old setup's authority and grants. The box's identity and on-disk work stay put.
    const { key, secret, sign, name, relay, startsFromOthers } = current;
    return { key, secret, sign, name, relay, startsFromOthers, phones: [],
      setup: { id: newSetupID(), epoch: 1, primary: own, sign: ownSign(current)!, role: "primary", managed: true } };
  }, { durable: true });
  if (!changed) return;
  host?.keepOnly(new Set());
  if (their.wipe) { scheduleWipe(); return; }
  sendRemoval(host, peer, link, { done: operation, setup: setup.id });
  writeAgents(changed);
  writeCard();
  noteDocsChanged();
}

function takeOffer(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState, from?: Buffer) {
  const setup = followedSetup(link);
  const doc = readDoc(message.offer);
  const offer = doc?.body;
  if (setup?.role !== "member" || link.localWipe || doc?.kind !== "setup-shared" || offer?.removal !== "offer"
    || offer.setup !== setup.id || offer.primary !== sender || offer.key !== ownKey(link)
    || typeof offer.operation !== "string" || !REMOVAL_ID.test(offer.operation) || doc.n < setup.epoch) return;
  if (doc.n > setup.epoch) {
    // A source address can carry a request for the chain. Only valid certificates advance trust.
    const peer = peersOf(link).find(peer => peer.key === sender);
    if (peer) sendRemoval(host, peer, link, { sync: setup.epoch, setup: setup.id });
    else if (from?.length === 16) sendRemoval(host, { key: sender, address: from.toString("base64url") }, link, { sync: setup.epoch, setup: setup.id });
    return;
  }
  if (sender !== setup.primary || doc.origin !== setup.sign) return;
  const peer = peersOf(link).find(peer => peer.key === sender);
  if (!peer) return;
  if (!challenge || challenge.setup !== setup.id || challenge.epoch !== setup.epoch || challenge.operation !== offer.operation
    || Date.now() > challenge.expires) {
    challenge = { setup: setup.id, epoch: setup.epoch, primary: sender, operation: offer.operation,
      nonce: crypto.randomBytes(16).toString("base64url"), expires: Date.now() + challengeLifetime };
  }
  sendRemoval(host, peer, link, { request: challenge.operation, setup: setup.id, nonce: challenge.nonce });
}

function deliverToRemoved(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState, record: MemberRemoval) {
  const setup = primary(link);
  if (!setup || message.setup !== setup.id || !link.sign) return;
  if (message.done === record.operation) {
    updateLink(current => {
      if (!current || primary(current)?.id !== setup.id || removalsOf(current)[sender]?.operation !== record.operation
        || current.removalAcks?.[sender] === record.operation) return null;
      current.removalAcks = { ...current.removalAcks, [sender]: record.operation };
      return current;
    }, { durable: true });
    sendRemoval(host, record.peer, link, { recorded: record.operation });
    saveDelivery(sender, record.operation);
  } else if (message.request === record.operation && typeof message.nonce === "string" && REMOVAL_ID.test(message.nonce)
    && !record.delivered && link.removalAcks?.[sender] !== record.operation) {
    const instruction = signDoc(link.sign, "setup-shared", setup.epoch, {
      removal: "instruction", setup: setup.id, primary: setup.primary, key: sender,
      operation: record.operation, wipe: record.wipe, nonce: message.nonce,
    });
    sendRemoval(host, record.peer, link, { instruction });
  } else if (typeof message.sync === "number" && Number.isSafeInteger(message.sync) && message.sync >= 1 && message.sync < setup.epoch) {
    const certificate = link.leadership?.[message.sync + 1];
    if (certificate) for (const authority of leadershipDocuments(certificate)) {
      for (const piece of docMails({ give: [authority] })) {
        sendRemoval(host, record.peer, link, piece.part ? { authorityPart: piece.part } : { authority });
      }
    }
  }
}

export function receiveRemovalMail(sender: string, message: Record<string, unknown>, host: Host | null | undefined, from?: Buffer): void {
  try {
    if (!removalLayoutReady(K) || !isObject(message)) return;
    const link = readLink();
    if (!link) return;
    const removal = removalsOf(link)[sender];
    if (removal && primary(link)) return deliverToRemoved(sender, message, host, link, removal);
    if (link.localWipe) return;
    if (message.offer !== undefined) return takeOffer(sender, message, host, link, from);
    if (message.instruction !== undefined) return takeInstruction(sender, message, host, link);
    if (message.authority !== undefined || message.authorityPart !== undefined) receiveAuthority(sender, message, link);
  } catch (error) { console.error("Computer removal delivery:", error); }
}
