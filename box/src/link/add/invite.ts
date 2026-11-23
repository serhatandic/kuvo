// Installer invitations authenticate the first exchange. Once saved, the computer uses ordinary
// signed setup documents and relay mail, with the same roster entry as a computer added over SSH.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { addComputersLayoutReady, credentialSelectionLayoutReady, KUVO_FOLDER_LAYOUT, setupMergeLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { addressOf, newKey, publicOf } from "../crypto/keys";
import { openOneWay, sealOneWay } from "../crypto/noise";
import { noteDocsChanged } from "../documents/changed";
import { macDocsChanged } from "../documents/exchange";
import { DOC_PART, MOST_DOC, newSigningSeed, ownSign, readDoc, SIGN, signDoc } from "../documents/format";
import { assemble, splitParts, type PendingParts } from "../documents/parts";
import { docMails, docsWork, ownDoc, primaryPeersBody, type OwnDoc } from "../documents/work";
import { IS_MAC, K, REAL_BOX } from "../home";
import { runningBridge, startBridge } from "../keeper";
import { computerName, macKeys, macLinkCanRotate, ownMacConfig, rotateMacLink, savedMacConfig } from "../mac/link";
import { ADD_RESERVATION } from "../marks";
import { ADDRESS, fitsPeer, KEY, macsOf, ownKey, peersOf, type Peer } from "../peers/known";
import { readPersonalities } from "../personalities/library";
import { pairedPhone } from "../phones";
import { Frame, frame, readFrame, readRelay, relayHello, relaySend } from "../relay/frames";
import type { LinkHost } from "../relay/host";
import type { Asker } from "../server/context";
import { writeCard } from "../setup/card";
import { updateAccess } from "../setup/access";
import { setMacPeers } from "../mac/routes";
import { settleOwnSetup, startSetup } from "../setup/lead";
import { ensurePrimaryLease } from "../setup/relay-lease";
import { holdsRelayLease, LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import { hasBootstrapMembership, acceptedInstallerRecoveries, acceptedMembership, membershipOf } from "../setup/membership";
import { authorizeMerge, bindMergeAcceptance, bringMergeAccess, computersSupportMerge, MERGE_FEATURE, MergeRefusal, mergeAuthority, mergeInput, mergeSourceProblem, planMerge, sameMerge, sealMergeSource, settleMergedNumbers, type MergeJournal, type MergePlan } from "../setup/merge-data";
import { followedSetup, ledSetup, SETUP_ID, type SetupWord } from "../setup/record";
import { localRemovalPending } from "../setup/removal-record";
import { installShared, operationOutcome, publishedShared, SharedCapacity, sharedValue, sharedValues, stageShared, type Receipt } from "../setup/shared";
import { managedFields } from "../setup/settle";
import { liveLink, macLink, machineLink, readLink, updateLink, type LinkState } from "../state";
import { archiveDeviceRevocations } from "../setup/device-revocations";
import { hasSetupWork } from "../setup/existing-work";
import { reserveMergeNumbers } from "../work-queue/numbers";
import { takesForwarded, type Acted } from "../work-queue/forward";
import type { InstallerCancellation, InstallerInvite, MergeTarget } from "./invite-state";
import { stageInstallerEnrollment } from "./flow";
import { tokenPayload, UnsupportedFormatVersion } from "../../shared/format-version";
import { credentialSeedFormat } from "./command-formats";
import { applyInstallerSeed, credentialChecklist, gatherSeed, withCredentialCopyLock } from "./seed";
import { Failed } from "./tries";
import type { Doc, SignedDoc } from "../../shared/documents";

class ClaimRefused extends Error {}
class DeclinedMerge extends ClaimRefused {}

const INVITE_LIFETIME = 15 * 60_000;
const CLAIM_LIFETIME = 24 * 60 * 60_000;
const PREFIX = Buffer.from("kuvo installer v1\0");
const CONSENT = Buffer.from("kuvo installer credential consent v1\0");
const CODE = "kuvo-add-1.";
const RECOVERY = "kuvo-cancel-1.";
const TOKEN = /^[A-Za-z0-9_-]{32}$/;
const TOKEN_HASH = /^[A-Za-z0-9_-]{43}$/;
const hash = (value: string) => crypto.createHash("sha256").update(value).digest("base64url");
const bytes = (text: string) => Buffer.from(text, "base64url");
const relayAddress = (secret: string) => addressOf(bytes(secret)).toString("base64url");

interface Code { expires: number; setup: string; relay: string; address: string; primary: string; token: string; merge?: MergeTarget }

const refusal = (status: number, message: string): Acted => ({ status, value: { message } });
const isLive = (invite: InstallerInvite, now: number) => (invite.claim?.until ?? invite.expires) > now;

export function installerCancellationReceipts(): string[] {
  const receipts: InstallerCancellation[] = [];
  const link = updateLink(current => {
    receipts.push(...current?.installerCancellations ?? []);
    const claim = current?.installerClaim;
    if (!current || !claim?.recovery || claim.adopted || current.setup?.role === "member" || current.macRotation
      || !claim.cancelPending && claim.until > Date.now() && !hasSetupWork(current)) return null;
    claim.cancelPending = true;
    return current;
  }, { durable: true });
  const claim = link?.installerClaim;
  if (claim?.recovery) receipts.push({ setup: claim.setup, invite: claim.invite, ...claim.recovery });
  if (!receipts.length) throw new Error("This computer has no installer cancellation receipt.");
  return receipts.map(receipt => RECOVERY + Buffer.from(JSON.stringify(receipt)).toString("base64url"));
}

/** A later code gets fresh keys. Keep the old exact receipt available without holding a setup fence. */
export function archiveInstallerCancellation(link: LinkState) {
  const claim = link.installerClaim;
  if (claim?.recovery && !claim.adopted) {
    link.installerCancellations ??= [];
    link.installerCancellations.push({ setup: claim.setup, invite: claim.invite, ...claim.recovery });
  }
  delete link.installerClaim;
}

/** This local command works on the current primary, including after an authority change.
 * Possession of the receipt is scoped to the exact enrollment that committed its hash. */
export async function applyInstallerCancellation(input: string): Promise<string> {
  let receipt: unknown;
  const payload = input.length < 512 ? tokenPayload(input, "kuvo-cancel-", 1) : null;
  if (payload !== null) {
    try { receipt = JSON.parse(bytes(payload).toString("utf8")); } catch {}
  }
  if (!isObject(receipt) || typeof receipt.setup !== "string" || !SETUP_ID.test(receipt.setup)
    || typeof receipt.invite !== "string" || !TOKEN_HASH.test(receipt.invite)
    || typeof receipt.key !== "string" || !KEY.test(receipt.key)
    || typeof receipt.secret !== "string" || !TOKEN.test(receipt.secret)) throw new Error("Enter an installer cancellation receipt.");
  const { setup, invite, key, secret } = receipt;
  const commitment = hash(secret);
  const operationFor = (epoch: number) => hash(JSON.stringify(["installer-cancellation", setup, invite, key, secret, epoch]));
  const matches = (entry: NonNullable<LinkState["installerRecoveries"]>[string] | undefined) =>
    entry?.invite === invite && entry.commitment === commitment;
  const link = readLink();
  const position = followedSetup(link);
  if (!link || position?.role !== "primary" || position.id !== setup) {
    throw new Error("Run this command on the current main computer of the setup that issued the code.");
  }
  const accepted = acceptedMembership(link);
  if (accepted?.left.includes(key) && Object.entries(link.sharedOutcomes ?? {}).some(([id, outcome]) =>
    outcome.kind === "membershipAndAccess" && outcome.status === "committed" && id === operationFor(outcome.epoch))) {
    return "The provisional installer entry was already retired.";
  }
  if (!matches(acceptedInstallerRecoveries(link, accepted)[key])) {
    throw new Error("The accepted roster has no matching installer recovery entry. No computer was retired.");
  }
  const operation = operationFor(position.epoch);
  const { save } = updateAccess(draft => {
    const current = followedSetup(draft);
    const entry = draft?.installerRecoveries?.[key];
    if (!draft || !entry || current?.role !== "primary" || current.id !== setup || current.epoch !== position.epoch || current.primary === key
      || primaryFenced(draft) || localRemovalPending(draft) || draft.localUnlink
      || draft.installerInvites?.[invite]?.activated || draft.installerInvites?.[invite]?.credentialsCompleted
      || draft.wipePreparations?.[key] || !matches(entry)) return null;
    const roster = Array.isArray(draft.macPeers) ? draft.macPeers.filter(fitsPeer) : [];
    if (!roster.some(peer => peer.key === key && peer.sign === entry.sign && peer.address === entry.address)) return null;
    return setMacPeers(draft, { peers: roster.filter(peer => peer.key !== key), moved: draft.macMoved, removed: [key] }).link;
  }, { checkpoint: true, operation, beforeWrite: current => {
    const entry = current.installerInvites?.[invite];
    if (entry?.claim?.key !== key) return;
    entry.cancelled = true;
    delete entry.credentialSeed;
    delete entry.credentialSelectionDigest;
  } });
  if (!save) throw new Error("The main computer could not save this cancellation. Check its connection to Kuvo’s relay, then retry the receipt.");
  macDocsChanged();
  return "The provisional installer entry was retired.";
}

export function mergeTarget(value: unknown): MergeTarget | null {
  if (!isObject(value) || typeof value.source !== "string" || !SETUP_ID.test(value.source) || typeof value.computer !== "string" || !KEY.test(value.computer)
    || value.resume !== undefined && (typeof value.resume !== "string" || !TOKEN_HASH.test(value.resume))) return null;
  return { source: value.source, computer: value.computer, ...(value.resume ? { resume: value.resume } : {}) };
}

function relayURL(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 512) return false;
  try {
    const url = new URL(value);
    return (url.protocol === "wss:" || (!REAL_BOX && url.protocol === "ws:")) && !url.username && !url.password && !url.hash;
  } catch { return false; }
}

export function readInstallerCode(text: string): Code | null {
  text = text.trim();
  const payload = text.length <= 4096 ? tokenPayload(text, "kuvo-add-", 1) : null;
  if (payload === null) return null;
  let value: unknown;
  try { value = JSON.parse(bytes(payload).toString("utf8")); } catch { return null; }
  if (!isObject(value) || typeof value.expires !== "number" || !Number.isSafeInteger(value.expires) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup) || !relayURL(value.relay) || typeof value.address !== "string" || !ADDRESS.test(value.address)
    || typeof value.primary !== "string" || !KEY.test(value.primary) || typeof value.token !== "string" || !TOKEN.test(value.token)) return null;
  const merge = mergeTarget(value.merge);
  if (value.merge !== undefined && !merge) return null;
  return { expires: value.expires, setup: value.setup, relay: value.relay, address: value.address, primary: value.primary, token: value.token, ...(merge ? { merge } : {}) };
}

/** The response can cross the durable request forwarder without exposing the bearer token. */
export async function installerCodeFrom(issue: (id: string) => Promise<Acted>): Promise<Acted> {
  const token = crypto.randomBytes(24).toString("base64url");
  const reply = await issue(hash(token));
  if (reply.status !== 201 || !isObject(reply.value)) return reply;
  const { expires } = reply.value;
  return { status: 201, value: { code: CODE + Buffer.from(JSON.stringify({ ...reply.value, token })).toString("base64url"), expires } };
}

export async function createInstallerInvite(id: unknown, asker?: Asker, credentials: string[] = [], merge?: MergeTarget): Promise<Acted> {
  if (!addComputersLayoutReady(K)) return refusal(503, "Wait for the Kuvo Code update to finish before adding a computer.");
  if (merge && !setupMergeLayoutReady(K)) return refusal(503, "Wait for the Kuvo Code update to finish before merging setups.");
  if (typeof id !== "string" || !TOKEN_HASH.test(id)) return refusal(400, "Send the installer invitation hash.");
  if (IS_MAC) await macKeys({ needed: true });
  await ensurePrimaryLease();
  let seed: Buffer | undefined;
  const selectionDigest = credentials.length ? hash(JSON.stringify(credentials)) : undefined;
  if (selectionDigest && !readLink()?.installerInvites?.[id]) {
    const current = await credentialChecklist();
    const available = new Set(current.items.map(item => item.id));
    if (credentials.some(id => !available.has(id))) {
      return refusal(current.unavailable.length ? 503 : 409, "Some selected sign-ins or keys are no longer available. Open the checklist again.");
    }
    const { keys, credentials: captured, integrations, unavailable } = await gatherSeed(credentials);
    const missing = credentials.length - Object.keys(keys).length - captured.length;
    if (missing > 0) return refusal(unavailable ? 503 : 409, "Some selected sign-ins or keys are no longer available. Open the checklist again.");
    seed = Buffer.from(JSON.stringify(credentialSeedFormat.write({ keys, credentials: captured, integrations })));
    if (seed.length > MOST_DOC) return refusal(409, "The selected sign-ins and keys are too large to copy.");
  }
  const actor = asker?.key && { key: asker.key.toString("base64url"), pairing: asker.pairing ?? "" };
  let reply = refusal(409, "Only the main computer can issue an installer code.");
  const changed = updateLink(link => {
    if (!link) return null;
    settleOwnSetup(link);
    const setup = followedSetup(link);
    if (setup?.role !== "primary") return null;
    const merged = merge && acceptedMembership(link)?.mergedComputers?.[merge.computer];
    if (merge?.resume && merged && (merged.source !== merge.source || merged.operation !== merge.resume)) {
      reply = refusal(409, "That computer has a different accepted merge.");
      return null;
    }
    const recovery = merge && merged && merged.source === merge.source ? { ...merge, resume: merged.operation } : merge;
    if (merge && (merge.source === setup.id || merge.computer === setup.primary
      || peersOf(link).some(peer => peer.key === merge.computer) && !recovery?.resume)) {
      reply = refusal(409, "Choose the main computer of a different setup.");
      return null;
    }
    if (merge && !computersSupportMerge(link)) {
      reply = refusal(409, "Update the computers in the retained setup before merging.");
      return null;
    }
    if (primaryFenced(link) || localRemovalPending(link) || link.localWipe || link.localUnlink) {
      reply = refusal(503, LEADERSHIP_WAIT);
      return null;
    }
    if (actor && !pairedPhone(link, actor.key, actor.pairing)) {
      reply = refusal(403, "Pair this device before adding a computer.");
      return null;
    }
    if (!relayURL(link.relay) || !link.secret || !link.sign || !link.key) {
      reply = refusal(503, "This computer can't reach your relay yet.");
      return null;
    }
    const now = Date.now();
    const prior = link.installerInvites?.[id];
    if (prior && (prior.credentialsCompleted || prior.actor?.key !== actor?.key || prior.actor?.pairing !== actor?.pairing || prior.setup !== setup.id || prior.epoch !== setup.epoch || !isLive(prior, now) || prior.credentialSelectionDigest !== selectionDigest || prior.credentials?.length
      || JSON.stringify(prior.merge) !== JSON.stringify(recovery))) {
      reply = refusal(409, "This installer invitation is no longer available.");
      return null;
    }
    const expires = prior?.expires ?? now + INVITE_LIFETIME;
    const invites = Object.entries(link.installerInvites ?? {}).filter(([, invite]) => isLive(invite, now));
    if (!prior && invites.length >= 32) {
      reply = refusal(409, "There are too many installers waiting to join. Use an existing code or wait for it to expire.");
      return null;
    }
    if (credentials.length && !prior?.credentialSeed && !seed) {
      reply = refusal(409, "Open the checklist again to issue a code for these sign-ins and keys.");
      return null;
    }
    const key = bytes(link.key);
    const credentialSeed = seed ? sealOneWay(seed, key, publicOf(key), Buffer.concat([CONSENT, Buffer.from(id)])).message.toString("base64url") : undefined;
    link.installerInvites = Object.fromEntries(invites);
    link.installerInvites[id] = prior ?? { setup: setup.id, epoch: setup.epoch, expires, actor,
      ...(credentialSeed ? { credentialSelectionDigest: selectionDigest, credentialSeed } : {}), ...(recovery ? { merge: recovery } : {}) };
    const content: Omit<Code, "token"> = { expires, setup: setup.id, relay: link.relay, address: relayAddress(link.secret), primary: setup.primary, ...(recovery ? { merge: recovery } : {}) };
    reply = { status: 201, value: content };
    return link;
  }, { durable: true });
  if (changed) {
    if (IS_MAC) macLink.wake?.();
    else if (!runningBridge() && !machineLink.host) startBridge();
  }
  return reply;
}

function sendInstallerCredentials(id: string, seed: string, key: string, send: (value: object) => unknown) {
  try {
    const privateKey = bytes(key);
    const packed = openOneWay(bytes(seed), privateKey, publicOf(privateKey), Buffer.concat([CONSENT, Buffer.from(id)])).payload;
    if (packed.length < 34_000) {
      send({ seed: JSON.parse(packed.toString("utf8")) });
      return;
    }
    for (const seedPart of splitParts(packed.toString("base64url"))) send({ seedPart });
  } catch { send({ credentialProblem: "The saved copy is unavailable. Open the checklist again." }); }
}

function seal(key: string, to: string, address: Buffer, value: object): Buffer {
  const privateKey = bytes(key);
  const { message } = sealOneWay(Buffer.from(JSON.stringify(value)), privateKey, bytes(to), PREFIX);
  return relaySend(address, frame(Frame.mail, 0, Buffer.concat([PREFIX, publicOf(privateKey), message])));
}

function opened(body: Buffer, key: string): { sender: string; value: Record<string, unknown> } | null {
  const sender = body.subarray(PREFIX.length, PREFIX.length + 32);
  try {
    const value: unknown = JSON.parse(openOneWay(body.subarray(PREFIX.length + 32), bytes(key), sender, PREFIX).payload.toString("utf8"));
    return isObject(value) ? { sender: sender.toString("base64url"), value } : null;
  } catch { return null; }
}

const mergeParts = new Map<string, PendingParts>();
const adoptionParts = new Map<string, PendingParts>();
const adoptions = new Map<string, { authority?: Doc; roster?: Doc; replica?: Doc }>();
const adopted = new Set<string>();
let lastOffers = 0;

function deniedMerge(link: LinkState | null, operation: string) {
  const denied = sharedValue(link, "setupMergeDenials");
  return isObject(denied) ? readDoc(denied[operation]) : null;
}

/** An accepted negative outcome makes late confirmations incapable of committing. */
function declineMerge(position: SetupWord, source: MergeTarget, peer: Peer, operation: string, message: string) {
  const save = updateAccess(link => {
    if (!link?.sign || primaryFenced(link) || link.setup?.role !== "primary" || link.setup.id !== position.id
      || link.setup.epoch !== position.epoch || acceptedMembership(link)?.mergedComputers?.[peer.key]?.operation === operation
      || deniedMerge(link, operation)) return null;
    return link;
  }, {
    checkpoint: true,
    operation: hash("refused merge " + operation),
    together: () => (_roster, link) => {
      const prior = sharedValue(link, "setupMergeDenials");
      const decision = signDoc(link.sign!, "setup-shared", position.epoch, {
        mergeDenied: true, setup: position.id, source: source.source, computer: peer.key, sign: peer.sign, address: peer.address, operation, message,
      });
      return { setupMergeDenials: { value: { ...(isObject(prior) ? prior : {}), [operation]: decision } } };
    },
  }).save;
  if (save) macDocsChanged();
}

/** Every joining member receives the source primary's authorization and the target's accepted
 * membership. An offline computer receives the same proof when it comes back. */
export function sendMergeOffers(host = liveLink()) {
  if (!host?.welcomed || Date.now() - lastOffers < 6_000) return;
  lastOffers = Date.now();
  const link = readLink();
  const setup = followedSetup(link);
  const values = sharedValue(link, "setupMergeAuthorities");
  if (!link?.key || setup?.role !== "primary" || primaryFenced(link) || !isObject(values)) return;
  const membership = acceptedMembership(link);
  if (!membership) return;
  for (const [source, saved] of Object.entries(values)) {
    if (!isObject(saved)) continue;
    const authorityDoc = readDoc(saved.authority);
    const authority = mergeAuthority(authorityDoc);
    if (!authorityDoc || !authority) continue;
    const peers = membership.boxes.filter(peer => peer.key !== setup.primary && peer.address
      && authority.computers.includes(peer.key) && !adopted.has(authority.operation + peer.key)
      && !adopted.has((membership.mergedComputers?.[peer.key]?.operation ?? "") + peer.key));
    if (!peers.length) continue;
    const roster = readDoc(saved.roster), replica = readDoc(saved.replica);
    if (!roster || !replica) continue;
    const mails = docMails({ give: [authorityDoc.envelope, roster.envelope, replica.envelope] });
    for (const peer of peers) {
      for (const docs of mails) host.send(seal(link.key, peer.key, bytes(peer.address), { mergeOffer: source, docs }));
    }
  }
}

function receiveMergeOffer(sender: string, value: Record<string, unknown>, link: LinkState, host: LinkHost, from: Buffer) {
  if (!setupMergeLayoutReady(K) || !link.key || !link.setup) return;
  if (typeof value.mergeDone === "string") {
    const proofs = sharedValue(link, "setupMergeAuthorities");
    if (peersOf(link).some(peer => peer.key === sender) && isObject(proofs) && Object.values(proofs).some(saved => {
      const authority = isObject(saved) && mergeAuthority(readDoc(saved.authority));
      return authority && authority.operation === value.mergeDone && authority.computers.includes(sender);
    })) adopted.add(value.mergeDone + sender);
    return;
  }
  if (typeof value.mergeOffer !== "string" || !SETUP_ID.test(value.mergeOffer) || !isObject(value.docs)) return;
  const own = ownKey(link);
  if (!own || link.setup.id !== value.mergeOffer && link.lastSetupMerge?.setup !== link.setup.id) return;
  const scope = sender + value.mergeOffer;
  const pending = adoptions.get(scope) ?? {};
  if (!adoptions.has(scope) && adoptions.size >= 8) adoptions.delete(adoptions.keys().next().value!);
  adoptions.set(scope, pending);
  for (const envelope of Array.isArray(value.docs.give) ? value.docs.give : [assemble(adoptionParts, scope, value.docs.part)]) {
    const doc = readDoc(envelope);
    if (doc?.kind === "peers") pending.roster = doc;
    else if (doc?.kind === "setup-shared" && doc.body.replica) pending.replica = doc;
    else if (mergeAuthority(doc)) pending.authority = doc!;
  }
  const { authority: proof, roster, replica } = pending;
  if (!proof || !roster || !replica) return;
  adoptions.delete(scope);
  const authority = mergeAuthority(proof)!;
  const setup = ledSetup(roster);
  const membership = membershipOf(sharedValues(replica.body.replica)?.membershipAndAccess?.value);
  const plan = membership?.mergedComputers?.[own];
  if (!setup || !plan || !membership || authority.source !== value.mergeOffer || setup.primary !== authority.primary
    || authority.target !== setup.id || roster.origin !== authority.signer || replica.origin !== authority.signer
    || replica.body.id !== setup.id || replica.body.epoch !== setup.epoch || replica.n !== setup.epoch
    || plan.operation !== authority.operation || plan.source !== authority.source || !authority.computers.includes(own)
    || !membership.boxes.some(peer => peer.key === own && peer.sign === ownSign(link))) return;
  const ack = () => host.send(seal(link.key!, sender, from, { mergeDone: plan.operation }));
  if (link.lastSetupMerge?.operation === plan.operation && link.setup?.id === setup.id) {
    ack();
    return;
  }
  if (link.setup.id !== authority.source || link.setup.epoch > authority.sourceEpoch || link.setup.primary !== authority.sourcePrimary
    || link.setup.sign !== proof.origin || link.localUnlink || link.localWipe || link.removalSource || !link.relay) return;
  if (link.setup.role === "primary" && !link.setupMerge?.snapshot) return;
  const intention: MergeJournal = { source: authority.source, epoch: link.setup.epoch, target: authority.target,
    primary: setup.primary, operation: authority.operation, authority: proof.envelope };
  const reserved = updateLink(current => {
    if (!current || current.setup?.id !== authority.source || current.setup.epoch !== intention.epoch
      || current.setup.sign !== proof.origin || current.setupMerge && !sameMerge(current, intention)) return null;
    current.setupMerge ??= intention;
    return current;
  }, { durable: true });
  if (!reserved?.setupMerge) return;
  const finished = finishLocalMerge(reserved, reserved.setupMerge, setup, roster, replica, reserved.relay!, plan);
  if (finished) ack();
}

export function receiveInstallerClaim(body: Buffer, link: LinkState | null, host: LinkHost | null | undefined, from?: Buffer): boolean {
  if (!body.subarray(0, PREFIX.length).equals(PREFIX)) return false;
  if (!host || !from || !link?.key || body.length > 50_000) return true;
  const message = opened(body, link.key);
  if (!message) return true;
  const { sender, value } = message;
  if (value.mergeOffer !== undefined || value.mergeDone !== undefined) {
    try { receiveMergeOffer(sender, value, link, host, from); } catch (error) { console.warn("Setup merge delivery:", error); }
    return true;
  }
  if (typeof value.token !== "string" || !TOKEN.test(value.token) || typeof value.sign !== "string" || !SIGN.test(value.sign)
    || typeof value.address !== "string" || !ADDRESS.test(value.address) || typeof value.nonce !== "string" || !TOKEN.test(value.nonce)
    || (value.kind !== "mac" && value.kind !== "machine") || typeof value.name !== "string" || value.name.length > 100) return true;
  const peer = { key: sender, sign: value.sign, address: value.address, kind: value.kind, name: plainName(value.name).slice(0, 100) || "Computer" } satisfies Peer;
  const id = hash(value.token);
  const nonce = value.nonce;
  const findInvite = (current: LinkState | null): InstallerInvite | undefined => {
    const kept = current?.installerInvites?.[id];
    if (kept) return kept;
    const setup = followedSetup(current);
    const durable = acceptedMembership(current)?.mergedComputers?.[sender];
    const denial = deniedMerge(current, id);
    if (value.merging === true && setup?.role === "primary" && denial?.body.computer === sender
      && denial.body.sign === peer.sign && denial.body.address === peer.address && typeof denial.body.source === "string") {
      return { setup: setup.id, epoch: setup.epoch, expires: 0, merge: { source: denial.body.source, computer: sender } };
    }
    if (value.merging !== true || setup?.role !== "primary" || durable?.operation !== id
      || !peersOf(current).some(entry => entry.key === sender && entry.sign === peer.sign && entry.address === peer.address)) return;
    // Accepted membership can answer an exact original retry even after the invitation was pruned.
    return { setup: setup.id, epoch: setup.epoch, expires: 0, merge: { source: durable.source, computer: sender },
      claim: { key: sender, sign: peer.sign, address: peer.address, until: 0 } };
  };
  const invite = findInvite(link);
  if (!invite) return true;
  if (Boolean(invite.merge) !== (value.merging === true) || invite.merge && (invite.merge.computer !== sender || !setupMergeLayoutReady(K))) return true;
  const send = (reply: object) => host.send(seal(link.key!, sender, from, { nonce, ...reply }));
  const position = followedSetup(link);
  const now = Date.now();
  const enrolled = (current: LinkState | null) => peersOf(current).some(each => each.key === peer.key && each.sign === peer.sign && each.address === peer.address);
  const claimedBy = (entry: InstallerInvite | undefined) => entry?.claim?.key === peer.key && entry.claim.sign === peer.sign && entry.claim.address === peer.address;
  if (!invite.merge && value.cancel === true) {
    if (value.activationAckCapable !== true) return true;
    const marked = updateLink(current => {
      const entry = current?.installerInvites?.[id];
      const setup = followedSetup(current);
      if (!current || setup?.role !== "primary" || setup.id !== invite.setup || !entry || entry.merge || entry.activated || entry.credentialsCompleted
        || primaryFenced(current) || localRemovalPending(current) || !entry.cancelled && !isLive(entry, now)) return null;
      if (entry.claim ? !claimedBy(entry) : entry.actor && !pairedPhone(current, entry.actor.key, entry.actor.pairing)) return null;
      if (entry.claim?.recoveryBound && enrolled(current)
        && acceptedInstallerRecoveries(current)[peer.key]?.invite !== id) return null;
      // The applicant may have saved its claim and lost the connection before
      // the primary saw confirmation. In that case close the unused code too.
      entry.claim ??= { key: peer.key, sign: peer.sign, address: peer.address, until: now + CLAIM_LIFETIME };
      if (entry.cancelled) return current;
      entry.cancelled = true;
      delete entry.credentialSeed;
      delete entry.credentialSelectionDigest;
      return current;
    }, { durable: true });
    if (!marked) return true;
    if (!enrolled(marked)) {
      if (!peersOf(marked).some(each => each.key === peer.key)) send({ cancelled: true });
      return true;
    }
    // An invitation against an existing membership must never acquire
    // permission to retire it. Only this invitation's enrollment save qualifies.
    if (!marked.installerInvites?.[id]?.claim?.provisional) {
      send({ message: "The main computer could not verify that this invitation created the roster entry. Review that entry on the main computer." });
      return true;
    }
    const outcome = operationOutcome(marked, invite.setup, id);
    if (outcome?.kind !== "membershipAndAccess" || outcome.status !== "committed") return true;
    const { save } = updateAccess(draft => {
      const entry = draft?.installerInvites?.[id];
      const outcome = operationOutcome(draft, invite.setup, id);
      if (!draft || followedSetup(draft)?.id !== invite.setup || primaryFenced(draft) || draft.localUnlink || localRemovalPending(draft)
        || !entry?.cancelled || !entry.claim?.provisional || !claimedBy(entry) || !enrolled(draft)
        || draft.wipePreparations?.[peer.key] || entry.claim.recoveryBound && draft.installerRecoveries?.[peer.key]?.invite !== id
        || outcome?.kind !== "membershipAndAccess" || outcome.status !== "committed") return null;
      const roster = Array.isArray(draft.macPeers) ? draft.macPeers.filter(fitsPeer) : [];
      return setMacPeers(draft, { peers: roster.filter(each => each.key !== peer.key), moved: draft.macMoved, removed: [peer.key] }).link;
    }, { checkpoint: true });
    if (save) macDocsChanged();
    return true;
  }
  if (invite.cancelled) {
    send({ message: "This enrollment was cancelled. Get a new installer code from your setup." });
    return true;
  }
  const mergeOperation = invite.merge?.resume ?? id;
  const acceptedMerge = (current: LinkState | null) => invite.merge && acceptedMembership(current)?.mergedComputers?.[sender]?.operation === mergeOperation;
  const denial = invite.merge && deniedMerge(link, mergeOperation);
  if (denial?.body.computer === sender && denial.body.sign === peer.sign && denial.body.address === peer.address && position?.role === "primary") {
    send({ denial: denial.envelope });
    return true;
  }
  if (position?.role !== "primary" || position.id !== invite.setup || (position.epoch !== invite.epoch
    && !(claimedBy(invite) && (invite.merge || enrolled(link)) || acceptedMerge(link) && enrolled(link)))) {
    send({ message: "Your setup’s main computer changed. Get a new installer code from the same setup.", retrySameSetup: true });
    return true;
  }
  const allowed = (current: LinkState | null, entry: InstallerInvite | undefined, saved = false) => {
    const setup = followedSetup(current);
    const replay = acceptedMerge(current);
    return Boolean(current && entry && !entry.cancelled && (replay || isLive(entry, Date.now())) && setup?.role === "primary" && setup.id === entry.setup
      && (setup.epoch === entry.epoch || (replay || saved && claimedBy(entry)) && (entry.merge || enrolled(current))) && !current.localWipe && !current.localUnlink && !localRemovalPending(current)
      && (replay || saved || !entry.actor || pairedPhone(current, entry.actor.key, entry.actor.pairing)) && (!entry.claim || claimedBy(entry)));
  };
  if (!allowed(link, invite, Boolean(invite.claim))) {
    send({ message: "This installer code is no longer available. Get a new code from your setup." });
    return true;
  }
  const acknowledging = !invite.merge && (value.activationAck === true || value.credentialAck === true && invite.credentialSeed);
  if (acknowledging && acceptedInstallerRecoveries(link)[peer.key]?.invite === id) {
    // Seal successful adoption in portable membership before acknowledging it.
    const { save } = updateAccess(draft => {
      const entry = draft?.installerInvites?.[id];
      const recovery = draft?.installerRecoveries?.[peer.key];
      if (!draft || recovery?.invite !== id || !claimedBy(entry) || !enrolled(draft) || !allowed(draft, entry, true)
        || primaryFenced(draft)) return null;
      delete draft.installerRecoveries![peer.key];
      return draft;
    }, { checkpoint: true });
    if (save) macDocsChanged();
    return true;
  }
  if (value.activationAck === true && !invite.merge) {
    const marked = updateLink(current => {
      const entry = current?.installerInvites?.[id];
      if (!current || !entry || !claimedBy(entry) || !enrolled(current) || !allowed(current, entry, true) || primaryFenced(current)) return null;
      entry.activated = true;
      return current;
    }, { durable: true });
    if (marked) send({ activated: true, ...(marked.installerInvites?.[id]?.claim?.recoveryBound ? { portableActivation: true } : {}) });
    return true;
  }
  if (value.credentialAck === true && !invite.merge) {
    let acknowledged = false;
    updateLink(current => {
      const entry = current?.installerInvites?.[id];
      if (!current || !entry || !claimedBy(entry) || !enrolled(current) || !allowed(current, entry, true)
        || primaryFenced(current) || (!entry.credentialSeed && !entry.credentialsCompleted)) return null;
      acknowledged = true;
      if (entry.credentialsCompleted) return null;
      delete entry.credentialSeed;
      delete entry.credentialSelectionDigest;
      entry.credentialsCompleted = true;
      return current;
    }, { durable: true });
    if (acknowledged) send({ credentialComplete: true });
    return true;
  }
  if (invite.credentialSeed && value.credentialReceipt !== true) {
    send({ message: "Update this computer’s installer to copy the sign-ins and keys selected for this code.", retrySameSetup: true });
    return true;
  }
  if (invite.credentials?.length || invite.credentialSelectionDigest && !invite.credentialSeed) {
    send({ message: "Open the checklist again to issue a code for these sign-ins and keys." });
    return true;
  }
  if (value.confirmed !== true && !invite.merge) {
    if (primaryFenced(link)) return true;
    send({ ready: true, recoveryCapable: true });
    return true;
  }
  const merged = acceptedMerge(link);
  let importing: ReturnType<typeof mergeInput> = null;
  if (value.confirmed === true && invite.merge && !merged) {
    if (!value.mergeDoc && !value.mergePart) return true;
    const sourceDoc = readDoc(value.mergeDoc ?? assemble(mergeParts, id, value.mergePart));
    if (!sourceDoc) return true;
    importing = mergeInput(sourceDoc, peer, position, mergeOperation, Boolean(invite.merge.resume), readDoc(value.deliverAuthority));
    if (!importing || importing.source !== invite.merge.source) {
      send({ message: "This code was made for a different setup, or this setup’s saved data could not be read." });
      return true;
    }
  }
  // Save the claim before starting the roster operation so a restart can take the same request again.
  if (!invite.claim && !merged) {
    const changed = updateLink(current => {
      const entry = current?.installerInvites?.[id];
      if (!current || !entry || !allowed(current, entry)) return null;
      entry.claim ??= { key: peer.key, sign: peer.sign, address: peer.address, until: now + CLAIM_LIFETIME };
      return current;
    }, { durable: true });
    if (!changed) return true;
  }
  if (value.confirmed !== true) {
    if (!primaryFenced(readLink())) send({ ready: true, targetSigner: ownSign(link) });
    return true;
  }
  const response: { doc?: OwnDoc } = {};
  const committed = updateLink(current => {
    if (!current || !allowed(current, findInvite(current), true) || primaryFenced(current)) return null;
    if (!enrolled(current) || invite.merge && !acceptedMerge(current)) return null;
    const doc = ownDoc(current, "peers", primaryPeersBody(current));
    if (doc) response.doc = doc;
    return doc?.changed ? current : null;
  });
  if (response.doc) {
    const current = readLink();
    const fresh = findInvite(current);
    const seed = credentialSelectionLayoutReady(K) && allowed(current, fresh, true) && !primaryFenced(current) && enrolled(current) && fresh?.credentialSeed;
    for (const docs of docMails({ give: [response.doc.envelope] })) send({ docs, copyCredentials: !!seed,
      ...(!invite.merge && value.activationAckCapable === true ? { activationReceipt: true } : {}),
      ...(fresh?.claim?.recoveryBound ? { portableActivation: true } : {}),
      ...(seed && fresh?.claim ? { credentialUntil: fresh.claim.until } : {}) });
    if (invite.merge) {
      if (current) for (const docs of docMails({ give: publishedShared(current) })) send({ docs });
    }
    if (seed) sendInstallerCredentials(id, seed, link.key, send);
    if (committed) macDocsChanged();
    return true;
  }
  if (!allowed(link, invite, Boolean(invite.merge && invite.claim))) {
    send({ message: "This installer code is no longer available. Get a new code from your setup." });
    return true;
  }
  if (primaryFenced(link)) return true;
  try {
    const check = (current: LinkState | null) => {
      const entry = current?.installerInvites?.[id];
      if (!claimedBy(entry) || !allowed(current, entry, Boolean(entry?.merge))) throw new Error("This installer code is no longer available.");
    };
    const source = importing;
    const merge = invite.merge;
    const reservation = merge && source?.high ? reserveMergeNumbers(position.id, mergeOperation, source.high) : undefined;
    if (merge && source?.high && !reservation) return true;
    if (reservation && "exhausted" in reservation)
      throw new MergeRefusal("This setup needs enough Work Queue numbers to bring the other setup over.");
    const recovery = !merge && value.activationAckCapable === true && typeof value.recoveryCommitment === "string"
      && TOKEN_HASH.test(value.recoveryCommitment)
      ? { invite: id, commitment: value.recoveryCommitment, sign: peer.sign, address: peer.address } : undefined;
    const staged = source && merge
      ? stageInstallerEnrollment(peer, position.id, id, check, draft => {
        if (!computersSupportMerge(readLink() ?? draft)) throw new Error("Update the computers in the retained setup before merging.");
        const saved = planMerge(draft, source, mergeOperation, reservation ?? undefined);
        bringMergeAccess(draft, saved);
        return (roster, current) => bindMergeAcceptance(current, roster, saved);
      })
      : stageInstallerEnrollment(peer, position.id, id, current => {
        check(current);
        if (peersOf(current).some(each => each.key === peer.key || each.sign === peer.sign)
          || operationOutcome(current, position.id, id)) throw new Failed("This enrollment changed.");
      }, undefined, current => {
        const claim = current.installerInvites?.[id]?.claim;
        if (claim && value.activationAckCapable === true) claim.provisional = true;
        if (claim && recovery) claim.recoveryBound = true;
      }, recovery);
    const { save } = staged;
    if (save) macDocsChanged();
  } catch (error) {
    if (invite.merge && (error instanceof MergeRefusal || error instanceof SharedCapacity)) {
      try { declineMerge(position, invite.merge, peer, mergeOperation, error.message); } catch {}
      return true;
    }
    if (error instanceof Error && (error instanceof Failed || invite.merge)) {
      send({ message: error.message, ...(error instanceof Failed && error.again ? { retrySameSetup: true } : {}) });
    }
  }
  return true;
}

function exchange(
  code: Code, key: string, peer: { address: string; sign: string; name: string; kind: Peer["kind"] },
  reserve: (signer: string | undefined, recoveryCapable: boolean) => { snapshot?: SignedDoc; authority?: SignedDoc; recoveryCommitment?: string } | void
    | Promise<{ snapshot?: SignedDoc; authority?: SignedDoc; recoveryCommitment?: string } | void>,
  end?: "credentials" | "cancel" | "activate",
  requirePortableActivation = false,
): Promise<{ doc: Doc | null; replica?: Doc; seed?: unknown; credentialProblem?: string; credentialUntil?: number; activationReceipt?: true; portableActivation?: true }> {
  return new Promise((resolve, reject) => {
    const nonce = crypto.randomBytes(24).toString("base64url");
    const secret = crypto.randomBytes(32);
    const address = bytes(code.address);
    const payload = { ...peer, nonce, token: code.token, layout: KUVO_FOLDER_LAYOUT, credentialReceipt: credentialSelectionLayoutReady(K), ...(end === "credentials" ? { credentialAck: true } : end === "cancel" ? { cancel: true } : end === "activate" ? { activationAck: true } : {}), ...(code.merge ? { merging: true } : { activationAckCapable: true }) };
    let requests = [seal(key, code.primary, address, payload)];
    const socket = new WebSocket(code.relay);
    const parts = new Map<string, PendingParts>();
    const seedParts = new Map<string, PendingParts>();
    let doc: Doc | null = null;
    let replica: Doc | undefined;
    let seed: unknown;
    let copyCredentials = false;
    let credentialProblem: string | undefined;
    let credentialUntil: number | undefined;
    let activationReceipt: true | undefined;
    let portableActivation: true | undefined;
    socket.binaryType = "arraybuffer";
    let again: ReturnType<typeof setInterval> | undefined;
    const deadline = setTimeout(() => finish(new Error("Your main computer didn't finish adding this computer. Check that it’s online and run the installer again with the same code.")), end === "activate" ? 5_000 : 60_000);
    function finish(error?: Error) {
      clearTimeout(deadline);
      clearInterval(again);
      try { socket.close(); } catch {}
      if (error) reject(error);
      else if (doc || end) resolve({ doc, replica, seed, credentialProblem, credentialUntil, activationReceipt, portableActivation });
    }
    const send = () => { if (socket.readyState === WebSocket.OPEN) for (const request of requests) socket.send(request); };
    let confirmed = false;
    let reserving = false;
    socket.onopen = () => socket.send(relayHello(secret));
    socket.onerror = socket.onclose = () => finish(new Error("Couldn't connect to your setup's relay. Run the installer again with the same code."));
    socket.onmessage = async event => {
      const message = readRelay(Buffer.from(event.data));
      if (message?.kind === "welcome" && message.address.equals(addressOf(secret)) && !again) {
        again = setInterval(send, 1800);
        send();
        return;
      }
      if (message?.kind !== "deliver" || !message.address.equals(address)) return;
      const incoming = readFrame(message.payload);
      if (incoming?.kind !== Frame.mail || !incoming.body.subarray(0, PREFIX.length).equals(PREFIX)) return;
      const response = opened(incoming.body, key);
      if (response?.sender !== code.primary || response.value.nonce !== nonce) return;
      if (end === "credentials" && response.value.credentialComplete === true || end === "cancel" && response.value.cancelled === true
        || end === "activate" && response.value.activated === true
          && (!requirePortableActivation || response.value.portableActivation === true)) return finish();
      if (end === "cancel" && typeof response.value.message === "string") return finish(new ClaimRefused(response.value.message));
      if (end) return;
      if (response.value.ready === true && !confirmed && !reserving) {
        reserving = true;
        try {
          const saved = await reserve(typeof response.value.targetSigner === "string" ? response.value.targetSigner : undefined, response.value.recoveryCapable === true);
          const snapshot = saved?.snapshot;
          const next = { ...payload, confirmed: true, ...(saved?.authority ? { deliverAuthority: saved.authority } : {}),
            ...(saved?.recoveryCommitment ? { recoveryCommitment: saved.recoveryCommitment } : {}) };
          const given = snapshot ? snapshot.d.length <= DOC_PART ? [{ mergeDoc: snapshot }]
            : splitParts(snapshot.d, snapshot.s).map(mergePart => ({ mergePart })) : [{}];
          requests = given.map(part => seal(key, code.primary, address, { ...next, ...part }));
          confirmed = true;
          send();
        } catch (error) {
          finish(error instanceof Error ? error : new Error("This computer couldn't reserve its enrollment."));
        }
        return;
      }
      if (response.value.activationReceipt === true) activationReceipt = true;
      if (response.value.portableActivation === true) portableActivation = true;
      const { docs } = response.value;
      const denial = code.merge && readDoc(response.value.denial);
      if (denial?.body.mergeDenied === true && denial.body.setup === code.setup && denial.body.source === code.merge?.source
        && denial.body.computer === code.merge?.computer && denial.body.operation === (code.merge?.resume ?? hash(code.token))
        && typeof denial.body.message === "string") return finish(new DeclinedMerge(denial.body.message));
      if (typeof response.value.message === "string") {
        return finish(response.value.retrySameSetup === true ? new Error(response.value.message) : new ClaimRefused(response.value.message));
      }
      if (!confirmed) return;
      if (isObject(docs)) {
        for (const envelope of Array.isArray(docs.give) ? docs.give : [assemble(parts, code.primary, docs.part)]) {
          const received = readDoc(envelope);
          if (received?.kind === "peers") {
            doc = received;
            copyCredentials = response.value.copyCredentials === true;
            if (typeof response.value.credentialUntil === "number" && Number.isSafeInteger(response.value.credentialUntil)) {
              credentialUntil = response.value.credentialUntil;
            }
          }
          if (code.merge && received?.kind === "setup-shared") replica = received;
        }
      }
      if (isObject(response.value.seed)) seed = response.value.seed;
      if (typeof response.value.credentialProblem === "string") credentialProblem = response.value.credentialProblem;
      const assembled = assemble(seedParts, code.primary, response.value.seedPart);
      if (assembled) {
        try { seed = JSON.parse(bytes(assembled.d).toString("utf8")); } catch {}
      }
      if (doc && (!code.merge || replica) && (!copyCredentials || seed !== undefined || credentialProblem)) finish();
    };
  });
}

export interface InstallerMembership {
  setup: string;
  primary: string;
  credentials?: { missing: number };
}

function adoptMembership(link: LinkState, setup: SetupWord, doc: Doc, relay: string, replica?: Doc) {
  link.mac = setup.primary;
  link.macs = [setup.primary];
  link.macSigns = { [setup.primary]: doc.origin };
  link.setup = { ...setup, sign: doc.origin, role: "member", ...managedFields(doc), managed: true };
  link.relay = relay;
  delete link.pairing;
  delete link.appPairing;
  delete link.leadership;
  delete link.mergedComputers;
  installShared(link, undefined);
  const give = replica ? [doc.envelope, replica.envelope] : [doc.envelope];
  docsWork({ link, library: readPersonalities() }, { key: setup.primary, sign: doc.origin }, { give });
}

function memberInstalled() {
  writeCard();
  macDocsChanged();
  if (!IS_MAC && !machineLink.host && !runningBridge()) startBridge();
}

function finishLocalMerge(kept: LinkState, journal: MergeJournal, setup: SetupWord, doc: Doc, replica: Doc, relay: string, plan: MergePlan) {
  const key = ownKey(kept)!;
  settleMergedNumbers(journal, plan, kept, setup.primary);
  const changed = updateLink(current => {
    if (!current || !sameMerge(current, journal)) return null;
    adoptMembership(current, setup, doc, relay, replica);
    if (acceptedMembership(current)?.mergedComputers?.[key]?.operation !== journal.operation) return null;
    delete current.setupMerge;
    delete current.installerInvites;
    archiveInstallerCancellation(current);
    delete current.releasedSetup;
    delete current.releasedWork;
    delete current.releasedDatabaseWork;
    delete current.releasedScopedWork;
    delete current.legacy;
    delete current.macSeen;
    delete current.macNames;
    current.lastSetupMerge = { operation: journal.operation, setup: setup.id, primary: setup.primary };
    return current;
  }, { durable: true });
  if (changed?.lastSetupMerge) memberInstalled();
  const finished = changed?.lastSetupMerge ?? readLink()?.lastSetupMerge;
  return finished?.operation === journal.operation ? finished : undefined;
}

/** Record the departing setup’s fence before the retained setup accepts its computers. */
async function recordDeparture(journal: MergeJournal) {
  let save: Receipt | null = null;
  const kept = updateLink(link => {
    if (!link || !sameMerge(link, journal) || !link.setupMerge?.authority) return null;
    if (readDoc(sharedValue(link, "setupDeparture"))?.envelope.s === journal.authority?.s) return link;
    save = stageShared(link, "setupDeparture", link.setupMerge.authority, undefined, journal.operation);
    return save ? link : null;
  }, { durable: true });
  if (!kept) throw new Error("The joining setup could not save its departure. Retry with the same code.");
}

async function releaseDeparture(journal: MergeJournal): Promise<boolean> {
  let save: Receipt | null = null;
  const kept = updateLink(link => {
    if (!link || !sameMerge(link, journal)) return null;
    if (!sharedValue(link, "setupDeparture")) return link;
    save = stageShared(link, "setupDeparture", null, undefined, hash("release merge " + journal.operation));
    return save ? link : null;
  }, { durable: true });
  return !!kept;
}

export async function joinSetupMerge(input: string): Promise<Acted> {
  let code: Code | null;
  try { code = readInstallerCode(input); }
  catch (error) {
    if (!(error instanceof UnsupportedFormatVersion)) throw error;
    return refusal(400, error.message);
  }
  if (!code?.merge) return refusal(400, "Enter the code for the other setup’s merge.");
  if (!setupMergeLayoutReady(K)) return refusal(503, "Wait for the Kuvo Code update to finish before merging setups.");
  if (IS_MAC) {
    macLink.config ??= savedMacConfig() ?? ownMacConfig();
    if (!await macKeys({ needed: true })) return refusal(503, "Kuvo couldn’t verify this Mac’s identity.");
  }
  const local = readLink();
  const operation = code.merge.resume ?? hash(code.token);
  const key = local && ownKey(local);
  if (local?.lastSetupMerge?.operation === operation && local.setup?.id === code.setup && local.setup.role === "member") {
    return { status: 200, value: local.lastSetupMerge };
  }
  const problem = mergeSourceProblem(local);
  if (problem || !local?.setup || !key || code.merge.computer !== key || code.merge.source !== local.setup.id) {
    return refusal(409, problem ?? "This code was made for another setup.");
  }
  if (local.relay !== code.relay) return refusal(409, "Both setups must use the same relay so their paired devices can still connect.");
  const pending = local.setupMerge;
  if (code.merge.resume && (!pending?.authority || pending.target !== code.setup || pending.operation !== operation)) {
    return refusal(409, "This code resumes a different saved merge.");
  }
  const intention: MergeJournal = pending ?? { source: local.setup.id, epoch: local.setup.epoch, target: code.setup, primary: code.primary, operation };
  if (intention.operation !== operation || intention.target !== code.setup) {
    return refusal(409, "This setup already has a merge in progress. Use its existing code.");
  }
  if (!local.setupMerge && Date.now() >= code.expires) return refusal(409, "This merge code has expired. Get a new one from the other setup.");
  const sign = ownSign(local)!;
  const address = relayAddress(local.secret!);
  const clear = (unsentOnly = false) => updateLink(current => {
    if (!sameMerge(current, intention) || unsentOnly && current?.setupMerge?.snapshot) return null;
    delete current!.setupMerge;
    return current;
  }, { durable: true });
  try {
    const result = await exchange(code, local.key!, { address, sign, name: local.name ?? computerName(), kind: IS_MAC ? "mac" : "machine" }, async signer => {
      const reserved = updateLink(current => {
        if (!current || mergeSourceProblem(current) || current.setup?.id !== intention.source || current.setup.epoch !== intention.epoch
          || ownKey(current) !== key || ownSign(current) !== sign || current.setupMerge && !sameMerge(current, intention)) return null;
        if (!current.setupMerge) {
          if (!signer || !SIGN.test(signer)) return null;
          current.setupMerge = { ...intention, authority: authorizeMerge(current, intention, signer) };
        }
        return current;
      }, { durable: true });
      if (!reserved?.setupMerge) throw new Error("This setup changed before its merge started.");
      const deliveryAuthority = code.merge?.resume && signer && SIGN.test(signer)
        ? authorizeMerge(reserved, { ...reserved.setupMerge, primary: code.primary }, signer) : undefined;
      if (reserved.setupMerge.snapshot) return { snapshot: reserved.setupMerge.snapshot, authority: deliveryAuthority };
      try {
        await recordDeparture(reserved.setupMerge);
        const snapshot = sealMergeSource(readLink() ?? reserved, reserved.setupMerge);
        if (!readDoc(snapshot)) throw new Error("This setup has too much shared data to merge in one save.");
        const recorded = updateLink(current => {
          if (!current?.setupMerge || !sameMerge(current, intention)) return null;
          current.setupMerge.snapshot ??= snapshot;
          return current;
        }, { durable: true });
        if (!recorded?.setupMerge?.snapshot) throw new Error("This setup changed before its merge was saved.");
        return { snapshot: recorded.setupMerge.snapshot, authority: deliveryAuthority };
      } catch (error) {
        // A request already in flight may have saved the snapshot first.
        if (!sharedValue(readLink(), "setupDeparture")) clear(true);
        throw error;
      }
    });
    const { doc, replica } = result;
    const setup = ledSetup(doc);
    const values = replica && sharedValues(replica.body.replica);
    const member = values && membershipOf(values.membershipAndAccess?.value);
    const plan = member?.mergedComputers?.[key];
    if (!doc || !setup || setup.id !== code.setup || setup.primary !== code.primary || !replica || replica.origin !== doc.origin
      || replica.body.id !== setup.id || replica.n !== setup.epoch || replica.body.epoch !== setup.epoch
      || !plan || !member.boxes.some(peer => peer.key === key && peer.sign === sign && peer.address === address)) {
      return refusal(503, "The main computer hasn’t returned this setup’s accepted merge. Retry with the same code.");
    }
    const kept = readLink();
    if (kept?.lastSetupMerge?.operation === operation) return { status: 200, value: kept.lastSetupMerge };
    if (!kept?.setupMerge || !sameMerge(kept, intention)) return refusal(409, "This setup changed before the merge could finish.");
    const finished = finishLocalMerge(kept, kept.setupMerge, setup, doc, replica, code.relay, plan);
    return finished ? { status: 200, value: finished } : refusal(503, "Retry with the same code to finish this setup’s merge.");
  } catch (error) {
    if (error instanceof DeclinedMerge && readLink()?.setupMerge) {
      if (!await releaseDeparture(intention)) return refusal(503, "The retained setup declined the merge. Retry the code so this setup can resume work.");
      clear();
    }
    return refusal(error instanceof ClaimRefused ? 409 : 503, error instanceof Error ? error.message : "Retry with the same code to finish this setup’s merge.");
  }
}

export function requireJoinCode(input: string): Code {
  const code = readInstallerCode(input);
  if (!code) throw new Error("Enter the installer code shown on your device.");
  if (code.merge) throw new Error("Use Merge Setups on the setup this code was made for.");
  return code;
}

function replaceInstallerIdentity(link: LinkState, identity: Pick<LinkState, "key" | "secret" | "sign">) {
  const bootstrap = hasBootstrapMembership(link);
  Object.assign(link, identity);
  delete link.own;
  delete link.docs;
  if (bootstrap) {
    // The relay lease is tied to the old identity. Start a fresh standalone setup with the
    // replacement keys while preserving the work baseline for an explicitly unlinked computer.
    const released = link.releasedSetup === link.setup!.id;
    startSetup(link);
    if (released) link.releasedSetup = link.setup!.id;
  }
  else if (link.setup?.role === "primary") Object.assign(link.setup, { primary: ownKey(link), sign: ownSign(link) });
}

export async function joinWithInstallerCode(code: Code): Promise<InstallerMembership> {
  if (!addComputersLayoutReady(K)) throw new Error("Wait for the Kuvo Code update to finish before adding a computer.");
  if (IS_MAC) {
    macLink.config ??= savedMacConfig() ?? ownMacConfig();
    if (macLink.config?.relay !== code.relay || !await macKeys({ needed: true })) throw new Error("This Mac cannot connect to the relay in that installer code.");
  }
  const claimKey = hash(code.token);
  const canJoin = (current: LinkState) => {
    if (!current.localUnlink && !current.localWipe && !current.removalSource && current.setup?.role === "member"
      && current.setup.primary === code.primary && current.installerClaim?.invite === claimKey) return;
    if (current.localUnlink || current.localWipe || current.removalSource || current.setupMerge || current.setup?.role === "member" || macsOf(current).length
      || peersOf(current).length) {
      throw new Error("This computer already belongs to a setup or has paired devices. Unlink it before adding it to another setup.");
    }
    if (hasSetupWork(current)) {
      throw new Error("This computer has setup data. Use a setup merge to bring that data along.");
    }
    if (fs.lstatSync(path.join(path.dirname(K), ADD_RESERVATION), { throwIfNoEntry: false })) throw new Error("An SSH setup is already in progress on this computer.");
    if (current.macRotation && current.macRotation.attempt !== claimKey) throw new Error("Finish rotating this Mac’s identity with the original code.");
    if (current.installerClaim && !current.installerClaim.adopted && current.installerClaim.until > Date.now()
      && current.installerClaim.invite !== claimKey) {
      throw new Error("An installer enrollment is already in progress. Retry with its original code.");
    }
  };
  let joinProblem: string | undefined;
  let local = updateLink(current => {
    const link = current ?? { phones: [] };
    if (link.macRotation?.attempt === claimKey || link.installerClaim?.invite === claimKey && link.installerClaim.cancelPending) return link;
    if (link.setup?.role !== "member" && link.installerClaim?.invite === claimKey
      && link.installerClaim.until <= Date.now() && link.installerClaim.recovery) {
      link.installerClaim.cancelPending = true;
      return link;
    }
    try { canJoin(link); }
    catch (error) {
      if (link.setup?.role === "member" || link.installerClaim?.invite !== claimKey) throw error;
      joinProblem = error instanceof Error ? error.message : undefined;
      link.installerClaim.cancelPending = true;
      return link;
    }
    if (Date.now() >= code.expires && link.installerClaim?.invite !== claimKey) {
      throw new Error("This installer code has expired. Get a new code from your setup.");
    }
    if (IS_MAC && (!link.key || !link.sign || !link.secret)) throw new Error("Kuvo couldn’t read this Mac's identity.");
    const previous = ownKey(link);
    const abandoned = link.installerClaim && link.installerClaim.invite !== claimKey;
    if (previous === code.primary) throw new Error("This computer is already your main computer.");
    if (IS_MAC && previous && link.setup?.role !== "member" && link.codeIdentity !== claimKey && link.installerClaim?.invite !== claimKey
      && (macLinkCanRotate() || abandoned || link.departedKey === previous || !link.departedKey && link.releasedSetup === link.setup?.id && link.releasedSetup)) {
      if (!macLinkCanRotate()) throw new Error("Update Kuvo Helper before re-enrolling this Mac with an installer code.");
      link.macRotation = { attempt: claimKey, previous };
      if (abandoned) archiveInstallerCancellation(link);
      return link;
    }
    if (!IS_MAC && link.setup?.role !== "member"
      && link.codeIdentity !== claimKey && link.installerClaim?.invite !== claimKey) {
      archiveDeviceRevocations(link, "unattributed");
      link.codeIdentity = claimKey;
      replaceInstallerIdentity(link, {
        key: newKey().toString("base64url"), sign: newSigningSeed(), secret: crypto.randomBytes(32).toString("base64url"),
      });
      if (abandoned) archiveInstallerCancellation(link);
    }
    link.key ??= newKey().toString("base64url");
    link.sign ??= newSigningSeed();
    link.secret ??= crypto.randomBytes(32).toString("base64url");
    link.name ??= computerName();
    return link;
  }, { durable: true })!;
  if (local.macRotation?.attempt === claimKey) {
    local = await rotateMacLink(claimKey, local.macRotation.previous, replaceInstallerIdentity);
    canJoin(local);
  }
  const key = ownKey(local)!;
  const sign = ownSign(local)!;
  const address = relayAddress(local.secret!);
  const peer = { address, sign, name: local.name!, kind: IS_MAC ? "mac" as const : "machine" as const };
  const cancel = async (problem: string): Promise<never> => {
    const claim = readLink()?.installerClaim;
    const recovery = claim?.invite === claimKey && claim.recovery;
    const instructions = "Run installer-recovery issue on this computer and installer-recovery apply on the setup’s current main computer.";
    if (recovery && claim.until <= Date.now()) throw new Error(`${problem} The installer claim expired. ${instructions}`);
    await exchange(code, local.key!, peer, () => {}, "cancel").catch(error => {
      if (recovery) throw new Error(`${problem} ${instructions}`);
      if (error instanceof ClaimRefused) throw new Error(`${problem} ${error.message}`);
      throw new Error(`${problem} Run the installer again with the original code to finish cancelling the enrollment.`);
    });
    updateLink(current => {
      if (!current || current.setup?.role === "member" || current.installerClaim?.invite !== claimKey
        || !current.installerClaim.cancelPending || ownKey(current) !== key) return null;
      current.departedKey = key;
      delete current.installerClaim;
      return current;
    }, { durable: true });
    throw new Error(`${problem} The enrollment was cancelled. This computer kept its own setup.`);
  };
  if (local.installerClaim?.invite === claimKey && local.installerClaim.cancelPending) return cancel(joinProblem ?? "This computer could not join.");
  const acknowledgeActivation = async () => {
    const claim = readLink()?.installerClaim;
    if (!claim?.activationPending) return;
    try { await exchange(code, local.key!, peer, () => {}, "activate", claim.portableActivation === true); }
    catch { return; }
    updateLink(current => {
      if (current?.setup?.role !== "member" || current.installerClaim?.invite !== claimKey || ownKey(current) !== key) return null;
      delete current.installerClaim.activationPending;
      delete current.installerClaim.portableActivation;
      return current;
    }, { durable: true });
  };
  const acknowledge = async () => {
    if ((readLink()?.installerClaim?.until ?? 0) > Date.now()) {
      await exchange(code, local.key!, peer, () => {}, "credentials").catch(() => {
        throw new Error("The sign-ins and keys were saved. Run the installer again with the same code to confirm the copy.");
      });
    }
    updateLink(current => {
      if (!current?.installerClaim?.credentialsAckPending || current.installerClaim.invite !== claimKey || ownKey(current) !== key) return null;
      delete current.installerClaim.credentialsAckPending;
      return current;
    }, { durable: true });
  };
  if (local.setup?.role === "member" && !local.installerClaim?.credentialsPending) {
    await acknowledgeActivation();
    if (local.installerClaim?.credentialsAckPending) await acknowledge();
    const missing = local.installerClaim?.credentialsMissing ?? 0;
    return { setup: local.setup.id, primary: local.setup.primary, ...(missing > 0 ? { credentials: { missing } } : {}) };
  }
  const copyReady = credentialSelectionLayoutReady(K);
  let result;
  try {
    result = await exchange(code, local.key!, peer, (_signer, recoveryCapable) => {
      const reserved = updateLink(current => {
        if (!current || ownKey(current) !== key || ownSign(current) !== sign || relayAddress(current.secret ?? "") !== address) return null;
        if (current.installerClaim?.cancelPending) return null;
        canJoin(current);
        const previous = current.installerClaim?.invite === claimKey ? current.installerClaim : undefined;
        current.installerClaim = { setup: code.setup, invite: claimKey, until: previous?.until ?? Date.now() + CLAIM_LIFETIME,
          ...(!previous && recoveryCapable && current.setup?.role !== "member" ? { recovery: { key, secret: crypto.randomBytes(24).toString("base64url") } } : {}),
          ...(previous?.recovery ? { recovery: previous.recovery } : {}),
          ...(previous?.adopted ? { adopted: true } : {}),
          ...(previous?.portableActivation ? { portableActivation: true } : {}),
          ...(current.installerClaim?.invite === claimKey && current.installerClaim.credentialsPending ? { credentialsPending: true } : {}),
          ...(current.installerClaim?.invite === claimKey && current.installerClaim.activationPending ? { activationPending: true } : {}) };
        return current;
      }, { durable: true });
      if (!reserved) throw new Error("This computer’s identity changed. Run the installer again.");
      const secret = reserved.installerClaim?.recovery?.secret;
      return secret ? { recoveryCommitment: hash(secret) } : undefined;
    });
  } catch (error) {
    if (error instanceof ClaimRefused) updateLink(current => {
      if (!current || current.setup?.role === "member" || current.installerClaim?.invite !== claimKey
        || current.installerClaim.cancelPending) return null;
      if (current.installerClaim.recovery) current.installerClaim.cancelPending = true;
      else delete current.installerClaim;
      return current;
    }, { durable: true });
    throw error;
  }
  const { doc, seed, credentialProblem, credentialUntil, activationReceipt, portableActivation } = result;
  const setup = ledSetup(doc);
  if (!doc || !setup || setup.id !== code.setup || setup.primary !== code.primary || !Array.isArray(doc.body.boxes)
    || !doc.body.boxes.some(peer => isObject(peer) && peer.key === key && peer.sign === sign && peer.address === address)) {
    throw new Error("The main computer didn’t send this computer's accepted membership.");
  }
  const copyPending = copyReady && (seed !== undefined || credentialProblem !== undefined);
  let refused: string | undefined;
  const changed = updateLink(current => {
    if (!current || current.installerClaim?.invite !== claimKey || ownKey(current) !== key || ownSign(current) !== sign) return null;
    if (current.installerClaim.cancelPending) return current;
    try {
      if (current.setup?.role !== "member" && current.installerClaim.recovery && current.installerClaim.until <= Date.now()) {
        throw new Error("The installer did not finish before the claim expired.");
      }
      canJoin(current);
    }
    catch (error) {
      refused = error instanceof Error ? error.message : "This computer changed while it was joining.";
      current.installerClaim.cancelPending = true;
      return current;
    }
    if (copyPending && credentialUntil) current.installerClaim.until = credentialUntil;
    if (current.setup?.role === "member") {
      if (current.setup.id !== setup.id || current.setup.primary !== setup.primary || current.setup.sign !== doc.origin) return null;
      return current;
    }
    archiveDeviceRevocations(current, "unattributed");
    adoptMembership(current, setup, doc, code.relay);
    current.installerClaim.adopted = true;
    if (activationReceipt && portableActivation) current.installerClaim.portableActivation = true;
    delete current.installerClaim.recovery;
    delete current.releasedSetup;
    delete current.releasedWork;
    delete current.releasedDatabaseWork;
    delete current.releasedScopedWork;
    delete current.codeIdentity;
    if (copyPending) current.installerClaim.credentialsPending = true;
    if (activationReceipt) current.installerClaim.activationPending = true;
    return current;
  }, { durable: true });
  if (!changed) throw new Error("The installer's enrollment changed. Run it again with the same code.");
  if (changed.installerClaim?.cancelPending) return cancel(refused ?? "This computer could not join.");
  memberInstalled();
  await acknowledgeActivation();
  let missing = 0;
  if (copyPending) {
    if (credentialProblem) throw new Error(`This computer joined. ${credentialProblem}`);
    missing = isObject(seed) && typeof seed.missing === "number" ? seed.missing : 0;
    const sameMember = (current: LinkState | null) => current?.installerClaim?.invite === claimKey
      && current.setup?.role === "member" && current.setup.id === setup.id && ownKey(current) === key
      && !current.localWipe && !current.localUnlink && !current.removalSource;
    await withCredentialCopyLock(async () => {
      if (!sameMember(readLink())) throw new Error("This computer’s enrollment changed before the selected credentials could be saved.");
      if (!await applyInstallerSeed(seed).catch(() => false) || missing > 0 && isObject(seed) && seed.unavailable === true) {
        throw new Error("This computer joined. Run the installer again with the same code to complete the selected sign-ins and keys.");
      }
      const completed = updateLink(current => {
        if (!current?.installerClaim || !sameMember(current)) return null;
        delete current.installerClaim.credentialsPending;
        current.installerClaim.credentialsAckPending = true;
        if (missing > 0) current.installerClaim.credentialsMissing = missing;
        return current;
      }, { durable: true });
      if (!completed) throw new Error("This computer’s enrollment changed while the selected credentials were being saved.");
    });
    await acknowledge();
  }
  return { setup: setup.id, primary: setup.primary, ...(missing > 0 ? { credentials: { missing } } : {}) };
}
