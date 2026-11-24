// Delivery is separate from authorization: the accepted roster has already revoked the peer.
import { archiveDeviceRevocations } from "./device-revocations";
import { releasedWorkBaseline } from "./existing-work";
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { withCredentialCopyLock } from "../add/seed";
import { removalLayoutReady, removalOperationsLayoutReady, unlinkCleanupLayoutReady } from "../../shared/folder-version";
import { ownSign, readDoc, signDoc } from "../documents/format";
import { docMails, type DocsState } from "../documents/work";
import { noteDocsChanged } from "../documents/changed";
import { K, KEYS_ON_HOST } from "../home";
import { clearHostKeys } from "../keys";
import { clearUnlinkCredentials } from "./unlink-credentials";
import { KEY, mailFor, ownKey, peersOf, type Peer } from "../peers/known";
import { writeAgents } from "../peers/agents";
import { assemble, type PendingParts } from "../documents/parts";
import type { LinkHost } from "../relay/host";
import { liveLink, readLink, updateLink, type LinkState } from "../state";
import { updateAccess } from "./access";
import { writeCard } from "./card";
import { cacheCheckpoint, leadershipDocuments, checkpointHash } from "./checkpoint";
import { primaryFenced } from "./lease";
import { takeLeadership } from "./leadership";
import { holdMemberWipe, scheduleWipe, wipeSupportedHere } from "./wipe";
import { usesAutomaticRelayBackup } from "../backup/engine";
import { followedSetup, newSetupID } from "./record";
import { startSetup } from "./lead";
import { EXPORT_INVENTORY_VERSION, MOVE_INVENTORY_VERSION, generationOf, REMOVAL_EXPORT_REQUIRED, REMOVAL_ID, REMOVAL_LAYOUT_UNSUPPORTED, removalDeliveryPending, removalsOf, selectionOf, type MemberRemoval } from "./removal-record";

import { currentPreparation, forgetFinishedPreparations, removalChoice, retrySource } from "./before-wipe";
import { receiveRemovalMove } from "./removal-moves";
import { readMoves } from "../moves/records";

type Host = Pick<LinkHost, "welcomed" | "send" | "keepOnly">;
const challengeLifetime = 30_000;
let challenge: { setup: string; epoch: number; primary: string; operation: string; nonce: string; expires: number } | undefined;
const parts = new Map<string, PendingParts>();
const checkpoints: NonNullable<DocsState["checkpoints"]> = new Map();

export function sendRemoval(host: Pick<Host, "welcomed" | "send"> | null | undefined, peer: Pick<Peer, "key" | "address">, link: LinkState, message: object): void {
  if (!host?.welcomed || !link.key) return;
  const mail = mailFor(peer, message, link, true);
  if (mail) try { host.send(mail); } catch {}
}

function primary(link: LinkState | null) {
  const setup = followedSetup(link);
  return setup?.role === "primary" && link?.sign && !primaryFenced(link) && !link.localWipe ? setup : null;
}

function updateRemoval(record: MemberRemoval, change: (next: MemberRemoval, draft: LinkState) => void) {
  const key = record.peer.key;
  const result = updateAccess(draft => {
    const current = draft?.memberRemovals?.[key];
    if (!draft || !current || !primary(draft) || JSON.stringify(current) !== JSON.stringify(record)) return null;
    change(current, draft);
    return draft;
  }, { checkpoint: true });
  if (result.save) noteDocsChanged();
  return result.save;
}

function completedGeneration(record: MemberRemoval, generation: unknown): boolean {
  return (record.generation === generation && (!record.plan || Boolean(record.generation) || record.resolution === "cancelled"))
    || (!record.resolution && Boolean(record.withdrawnGeneration) && record.withdrawnGeneration === generation);
}

const recordCompletion = (record: MemberRemoval) => updateRemoval(record, next => {
  next.delivered = true;
  delete next.error;
  delete next.unresolved;
});

/** The SSH probe can relay an acknowledgment after the wiping process (or its primary) is gone. */
export async function takeWipeReceipt(envelope: unknown): Promise<void> {
  if (!removalOperationsLayoutReady(K)) return;
  const link = readLink();
  const setup = primary(link);
  const proof = readDoc(envelope);
  if (!setup || proof?.kind !== "setup-shared" || proof.n !== 1 || proof.body.removal !== "completed" ||
    proof.body.setup !== setup.id || typeof proof.body.source !== "string") return;
  const record = removalsOf(link)[proof.body.source];
  if (!record?.wipe || record.resolution || record.delivered || record.peer.sign !== proof.origin ||
    record.operation !== proof.body.operation || !completedGeneration(record, proof.body.generation)) return;
  recordCompletion(record);
}

let checkingLocalRemoval = false;
export function advanceLocalRemoval(link: LinkState): void {
  if (checkingLocalRemoval) return;
  checkingLocalRemoval = true;
  void checkLocalRemoval(link).catch(error => console.error("Computer removal preparation:", error))
    .finally(() => { checkingLocalRemoval = false; });
}

async function checkLocalRemoval(link: LinkState): Promise<void> {
  const setup = primary(link);
  if (!setup || !removalOperationsLayoutReady(K)) return;
  const record = removalsOf(link)[setup.primary];
  if (!record?.local || !record.plan || record.resolution) return;
  if (record.retry) retrySource(setup.id, setup.primary, record.operation, record.retry);
  const progress = await currentPreparation(setup.id, setup.primary, record, removalChoice(setup.id, setup.primary, record.operation));
  acceptProgress(record, progress, setup.id);
  scheduleWipe();
}

let unlinkCleanupRunning = false;

function resumeUnlink(host: Host | null | undefined) {
  if (unlinkCleanupRunning) return;
  unlinkCleanupRunning = true;
  void finishUnlink(host).catch(error => console.warn("Unlink credential cleanup:", (error as Error).message))
    .finally(() => { unlinkCleanupRunning = false; });
}

function finishUnlink(host: Host | null | undefined) {
  return withCredentialCopyLock(() => finishUnlinkLocked(host));
}

async function finishUnlinkLocked(host: Host | null | undefined) {
  const current = readLink();
  const pending = current?.localUnlink;
  if (!current || !pending || !unlinkCleanupLayoutReady(K)) return;
  const fenced = current.removalSource;
  const keysCleared = host && host !== liveLink() && fenced?.setup === pending.setup &&
    fenced.operation === pending.operation && fenced.hostKeysCleared;
  if (!pending.completed && KEYS_ON_HOST && !keysCleared) await clearHostKeys();
  const finished = pending.completed ? current : updateLink(link => {
    if (!link?.localUnlink || link.localUnlink.operation !== pending.operation || link.localUnlink.completed) return null;
    clearUnlinkCredentials();
    const own = ownKey(link);
    const origin = ownSign(link);
    if (!own || !origin) return null;
    const { key, secret, sign, name, relay, startsFromOthers, macIdentityVersion } = link;
    const releasedSetup = newSetupID();
    archiveDeviceRevocations(link);
    const released: LinkState = { key, secret, sign, name, relay, startsFromOthers, macIdentityVersion, phones: [], departedKey: own,
      formerDeviceRevocations: link.formerDeviceRevocations, formerPhoneIssuers: link.formerPhoneIssuers,
      ...releasedWorkBaseline(),
      installerCancellations: link.installerCancellations,
      localUnlink: { ...link.localUnlink, completed: true },
      releasedSetup };
    startSetup(released, releasedSetup);
    // (Below the checkpoint layout, the bare setup, as it always was: never one it follows.)
    released.setup = { ...released.setup!, managed: true };
    return released;
  }, { durable: true });
  if (!finished) return;
  if (!pending.completed) {
    writeAgents(finished);
    writeCard();
    noteDocsChanged();
  }
  sendRemoval(host, pending.primary, finished, { done: pending.operation, setup: pending.setup, generation: pending.generation });
}

export function sendRemovalUpdates(host = liveLink()): void {
  if (readLink()?.localUnlink) resumeUnlink(host);
  if (!removalLayoutReady(K)) return;
  const link = readLink();
  const setup = primary(link);
  if (!link?.sign || !setup) return;
  advanceLocalRemoval(link);
  if (!host?.welcomed) return;
  forgetFinishedPreparations(link);
  const removals = removalsOf(link);
  for (const [key, operation] of Object.entries(link.removalAcks ?? {})) {
    const record = removals[key];
    if (record?.operation === operation && !record.delivered && record.resolution !== "lost") updateRemoval(record, next => { next.delivered = true; });
  }
  for (const record of Object.values(removals)) {
    if (record.local || !removalDeliveryPending(record)) continue;
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

async function takeInstruction(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState) {
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
  const operation = asked.operation;
  if (their.wipe && their.plan === undefined && their.requiresPreparation !== true) {
    const held = link.removalSource;
    const prior = held?.setup === setup.id && held.operation === operation ? held.review : undefined;
    const review = prior === "choose-export" || (wipeSupportedHere() && usesAutomaticRelayBackup()) ? "choose-export"
      : prior || (!wipeSupportedHere() ? "unsupported-layout" : undefined);
    if (review) {
      if (held?.operation !== operation || held.review !== review) {
        if (!holdMemberWipe(setup.id, operation, review)) return;
        host?.keepOnly(new Set());
      }
      challenge = undefined;
      sendRemoval(host, peer, link, { review, setup: setup.id, operation, epoch: setup.epoch });
      return;
    }
  }
  if (their.plan !== undefined) {
    if (!removalOperationsLayoutReady(K) || !isObject(their.plan) || !generationOf(their.plan.choiceHash)) return;
    const plan = { choiceHash: their.plan.choiceHash,
      ...(typeof their.plan.moveTo === "string" && KEY.test(their.plan.moveTo) ? { moveTo: their.plan.moveTo } : {}),
      ...(their.plan.archive === true ? { archive: true as const } : {}) };
    const generation = generationOf(their.generation) ? their.generation : undefined;
    const record = { operation, plan, generation, selection: selectionOf(their.selection) };
    const relayBackup = usesAutomaticRelayBackup();
    if (link.removalSource?.operation !== operation || link.removalSource.moveTo !== plan.moveTo ||
      (relayBackup && link.removalSource.review !== "choose-export")) {
      const changed = updateLink(current => {
        if (!current || followedSetup(current)?.id !== setup.id || current.setup?.epoch !== setup.epoch || current.localWipe) return null;
        const prior = current.removalSource?.setup === setup.id ? current.removalSource : undefined;
        const continues = prior && (prior.operation === operation || their.supersedes === prior.operation);
        current.removalSource = { ...(continues && prior.stopped ? { stopped: true, hostKeysCleared: prior.hostKeysCleared } : {}),
          ...(relayBackup || (continues && prior.review === "choose-export") ? { review: "choose-export" } : {}),
          setup: setup.id, operation, moveTo: plan.moveTo };
        current.phones = [];
        delete current.pairing;
        delete current.appPairing;
        return current;
      }, { durable: true });
      if (!changed) return;
      host?.keepOnly(new Set());
    }
    if (typeof their.retry === "string" && REMOVAL_ID.test(their.retry)) retrySource(setup.id, own, operation, their.retry);
    const progress = await currentPreparation(setup.id, own, record, message.choice, host);
    const current = readLink();
    if (!current || current.setup?.id !== setup.id || current.setup.epoch !== setup.epoch
      || current.removalSource?.operation !== operation || current.localWipe || challenge !== asked) return;
    if (generation === undefined || progress.generation !== generation) {
      sendRemoval(host, peer, link, { progress, setup: setup.id, operation, epoch: setup.epoch });
      return;
    }
  }
  if (!their.wipe && link.removalSource && readMoves(true).some(move => move.role === "from" && move.removalOperation && (!move.failed || move.assigning))) {
    updateLink(current => {
      if (!current?.removalSource || !current.removalSource.moveTo) return null;
      delete current.removalSource.moveTo;
      return current;
    }, { durable: true });
    return;
  }
  if (!their.wipe && !unlinkCleanupLayoutReady(K)) return;
  challenge = undefined;
  const changed = updateLink(current => {
    const position = followedSetup(current);
    if (!current || position?.id !== setup.id || position.epoch !== setup.epoch || position.primary !== sender
      || position.sign !== proof.origin || ownKey(current) !== own || current.localWipe || current.localUnlink) return null;
    if (their.wipe) {
      const held = current.removalSource;
      current.localWipe = { setup: setup.id, operation, primary: peer, source: own,
        ...(generationOf(their.generation) ? { generation: their.generation } : {}), ...(their.requiresPreparation === true ? { requiresPreparation: true } : {}),
        ...(host && host !== liveLink() && held?.setup === setup.id && held.operation === operation &&
          held.hostKeysCleared ? { hostKeysCleared: true } : {}) };
      current.phones = [];
      delete current.pairing;
      delete current.appPairing;
      return current;
    }
    const prior = current.removalSource;
    if (prior?.setup === setup.id && prior.stopped && their.supersedes === prior.operation) {
      current.removalSource = { setup: setup.id, operation, stopped: true, hostKeysCleared: prior.hostKeysCleared };
    }
    current.localUnlink = { setup: setup.id, operation, primary: peer,
      ...(generationOf(their.generation) ? { generation: their.generation } : {}) };
    current.phones = [];
    delete current.pairing;
    delete current.appPairing;
    return current;
  }, { durable: true });
  if (!changed) return;
  host?.keepOnly(new Set());
  if (their.wipe) { scheduleWipe(); return; }
  resumeUnlink(host);
}

function takeOffer(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState, from?: Buffer) {
  const setup = followedSetup(link);
  const doc = readDoc(message.offer);
  const offer = doc?.body;
  if (setup?.role !== "member" || doc?.kind !== "setup-shared" || offer?.removal !== "offer"
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
  if (link.localWipe) {
    if (link.localWipe.operation !== offer.operation || link.localWipe.prepared || wipeSupportedHere() ||
      !holdMemberWipe(setup.id, offer.operation, "unsupported-layout", link.localWipe)) return;
    host?.keepOnly(new Set());
  }
  if (!challenge || challenge.setup !== setup.id || challenge.epoch !== setup.epoch || challenge.operation !== offer.operation
    || Date.now() > challenge.expires) {
    challenge = { setup: setup.id, epoch: setup.epoch, primary: sender, operation: offer.operation,
      nonce: crypto.randomBytes(16).toString("base64url"), expires: Date.now() + challengeLifetime };
  }
  sendRemoval(host, peer, link, { request: challenge.operation, setup: setup.id, nonce: challenge.nonce,
    prior: link.removalSource?.operation });
}

function acceptProgress(record: MemberRemoval, progress: Record<string, unknown>, setup: string) {
  const { generation, invalidated, error, unresolved, export: archive } = progress;
  const selection = selectionOf(progress.selection);
  if (generationOf(invalidated) && invalidated === record.generation) {
    updateRemoval(record, next => {
      delete next.generation;
      delete next.inventoryVersion;
      delete next.moveInventoryVersion;
      delete next.export;
      next.error = typeof error === "string" ? error.slice(0, 500) : "Source work changed after verification. Review or retry the removal plan.";
    });
  } else if (!record.generation && !record.error) {
    if (typeof error === "string") {
      updateRemoval(record, next => {
        next.error = error.slice(0, 500);
        if (Array.isArray(unresolved)) next.unresolved = unresolved.filter(id => typeof id === "string").slice(0, 100);
      });
    } else if (record.plan?.moveTo && !record.selection && selection) {
      updateRemoval(record, next => { next.selection = selection; });
    } else if (generationOf(generation)) {
      if (record.plan?.archive && progress.inventoryVersion !== EXPORT_INVENTORY_VERSION) {
        updateRemoval(record, next => { next.error = "Update the source computer to verify all attachments, then retry the export."; });
        return;
      }
      if (record.plan?.archive && (!isObject(archive) || typeof archive.source !== "string" ||
        typeof archive.id !== "string" || typeof archive.at !== "number" || !Number.isFinite(archive.at))) return;
      if (record.plan?.moveTo) {
        if (progress.moveInventoryVersion !== MOVE_INVENTORY_VERSION) {
          updateRemoval(record, next => { next.error = "Update the source and destination to verify moved files before wiping."; });
          return;
        }
        if (!record.selection) return;
      }
      updateRemoval(record, (next, draft) => {
        next.generation = generation;
        if (record.plan?.moveTo) next.moveInventoryVersion = MOVE_INVENTORY_VERSION;
        if (record.plan?.archive) {
          next.inventoryVersion = EXPORT_INVENTORY_VERSION;
          next.export = archive as NonNullable<MemberRemoval["export"]>;
        }
        if (record.local) draft.localWipe = { setup, operation: record.operation, generation, requiresPreparation: true };
      });
    }
  }
}

function deliverToRemoved(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState, record: MemberRemoval, from?: Buffer) {
  const setup = primary(link);
  if (!setup || message.setup !== setup.id || !link.sign) return;
  if (message.done === record.operation && record.resolution !== "lost" && completedGeneration(record, message.generation)) {
    if (!record.delivered) {
      if (!record.wipe || record.resolution === "cancelled") updateLink(current => {
        if (!current || removalsOf(current)[sender]?.operation !== record.operation) return null;
        current.removalAcks = { ...current.removalAcks, [sender]: record.operation };
        return current;
      }, { durable: true });
      // The wiped computer may only get one completion through.
      if (!recordCompletion(record) || !record.wipe || record.resolution) return;
    }
    const receipt = typeof message.receipt === "string" && KEY.test(message.receipt) && from?.length === 16
      ? { key: message.receipt, address: from.toString("base64url") } : record.peer;
    sendRemoval(host, receipt, link, { recorded: record.operation, setup: setup.id });
  } else if (message.request === record.operation && typeof message.nonce === "string" && REMOVAL_ID.test(message.nonce)
    && !record.delivered && record.resolution !== "lost") {
    const oldMove = record.plan?.moveTo && record.moveInventoryVersion !== MOVE_INVENTORY_VERSION;
    const oldExport = record.plan?.archive && record.inventoryVersion !== EXPORT_INVENTORY_VERSION;
    if (record.generation && (oldMove || oldExport) && !record.resolution) {
      updateRemoval(record, next => {
        next.withdrawnGeneration = record.generation;
        delete next.generation;
        delete next.export;
        next.error = oldMove ? "The previous wipe approval is withdrawn. Update both computers, then retry verification." : "Retry the export to verify all attachments before wiping.";
      });
      return;
    }
    const preparing = Boolean(record.plan && !record.resolution);
    const supersedes = typeof message.prior === "string" && REMOVAL_ID.test(message.prior) && record.superseded?.[message.prior]
      ? message.prior : undefined;
    const instruction = signDoc(link.sign, "setup-shared", setup.epoch, {
      removal: "instruction", setup: setup.id, primary: setup.primary, key: sender,
      operation: record.operation, wipe: record.wipe && !record.resolution, nonce: message.nonce,
      ...(supersedes ? { supersedes } : {}),
      ...(preparing ? { plan: record.plan, generation: record.generation, retry: record.retry, selection: record.selection } : {}),
      ...(record.requiresPreparation && !record.resolution ? { requiresPreparation: true } : {}),
    });
    const choice = preparing && !record.generation ? removalChoice(setup.id, sender, record.operation) : undefined;
    sendRemoval(host, record.peer, link, { instruction, ...(choice ? { choice } : {}) });
  } else if (record.plan && !record.resolution && !record.delivered && message.operation === record.operation &&
    message.epoch === setup.epoch && isObject(message.progress)) {
    acceptProgress(record, message.progress, setup.id);
  } else if (record.wipe && !record.plan && !record.resolution && !record.delivered &&
    message.operation === record.operation && message.epoch === setup.epoch &&
    ((!record.needsExport && message.review === "choose-export") || (!record.error && message.review === "unsupported-layout"))) {
    updateRemoval(record, next => {
      if (message.review === "choose-export") {
        next.needsExport = true;
        next.error = REMOVAL_EXPORT_REQUIRED;
      } else next.error = REMOVAL_LAYOUT_UNSUPPORTED;
    });
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
    const unlink = link.localUnlink;
    if (unlink?.completed && unlink.primary.key === sender && message.setup === unlink.setup && message.recorded === unlink.operation) {
      updateLink(current => {
        if (!current?.localUnlink?.completed || current.localUnlink.operation !== unlink.operation) return null;
        delete current.localUnlink;
        return current;
      }, { durable: true });
      return;
    }
    if (unlink && !unlink.completed) return;
    if (receiveRemovalMove(sender, message, host, link)) return;
    const removal = removalsOf(link)[sender];
    if (removal && primary(link)) return deliverToRemoved(sender, message, host, link, removal, from);
    if (message.offer !== undefined) return takeOffer(sender, message, host, link, from);
    if (link.localWipe) return;
    if (message.instruction !== undefined) {
      void takeInstruction(sender, message, host, link).catch(error => console.error("Computer removal delivery:", error));
      return;
    }
    if (message.authority !== undefined || message.authorityPart !== undefined) receiveAuthority(sender, message, link);
  } catch (error) { console.error("Computer removal delivery:", error); }
}
