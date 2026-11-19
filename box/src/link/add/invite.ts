// Installer invitations authenticate the first exchange. Once saved, the computer uses ordinary
// signed setup documents and relay mail, with the same roster entry as a computer added over SSH.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { addComputersLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { addressOf, newKey, publicOf } from "../crypto/keys";
import { openOneWay, sealOneWay } from "../crypto/noise";
import { noteDocsChanged } from "../documents/changed";
import { macDocsChanged } from "../documents/exchange";
import { newSigningSeed, ownSign, readDoc, SIGN } from "../documents/format";
import { assemble, type PendingParts } from "../documents/parts";
import { docMails, docsWork, ownDoc, primaryPeersBody, type OwnDoc } from "../documents/work";
import { IS_MAC, K, REAL_BOX } from "../home";
import { runningBridge, startBridge } from "../keeper";
import { computerName, macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { ADD_RESERVATION } from "../marks";
import { ADDRESS, KEY, macsOf, ownKey, peersOf, type Peer } from "../peers/known";
import { readPersonalities } from "../personalities/library";
import { pairedPhone } from "../phones";
import { Frame, frame, readFrame, readRelay, relayHello, relaySend } from "../relay/frames";
import type { LinkHost } from "../relay/host";
import type { Asker } from "../server/context";
import { writeCard } from "../setup/card";
import { settleOwnSetup } from "../setup/lead";
import { LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import { followedSetup, ledSetup, requiresStandby, SETUP_ID } from "../setup/record";
import { localRemovalPending } from "../setup/removal-record";
import { operationOutcome, pendingShared } from "../setup/shared";
import { managedFields } from "../setup/settle";
import { macLink, machineLink, readLink, updateLink, type LinkState } from "../state";
import { allItems } from "../work-queue/store";
import type { Acted } from "../work-queue/forward";
import { type InstallerInvite } from "./invite-state";
import { stageInstallerEnrollment } from "./flow";
import { Failed } from "./tries";

class ClaimRefused extends Error {}

const INVITE_LIFETIME = 15 * 60_000;
const CLAIM_LIFETIME = 24 * 60 * 60_000;
const PREFIX = Buffer.from("kuvo installer v1\0");
const CODE = "kuvo-add-1.";
const TOKEN = /^[A-Za-z0-9_-]{32}$/;
const TOKEN_HASH = /^[A-Za-z0-9_-]{43}$/;
const hash = (value: string) => crypto.createHash("sha256").update(value).digest("base64url");
const bytes = (text: string) => Buffer.from(text, "base64url");
const relayAddress = (secret: string) => addressOf(bytes(secret)).toString("base64url");

interface Code { expires: number; setup: string; relay: string; address: string; primary: string; token: string }

const refusal = (status: number, message: string): Acted => ({ status, value: { message } });
const isLive = (invite: InstallerInvite, now: number) => (invite.claim?.until ?? invite.expires) > now;

function relayURL(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 512) return false;
  try {
    const url = new URL(value);
    return (url.protocol === "wss:" || (!REAL_BOX && url.protocol === "ws:")) && !url.username && !url.password && !url.hash;
  } catch { return false; }
}

function readCode(text: string): Code | null {
  if (text.length > 4096 || !text.startsWith(CODE)) return null;
  let value: unknown;
  try { value = JSON.parse(bytes(text.slice(CODE.length)).toString("utf8")); } catch { return null; }
  if (!isObject(value) || typeof value.expires !== "number" || !Number.isSafeInteger(value.expires) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup) || !relayURL(value.relay) || typeof value.address !== "string" || !ADDRESS.test(value.address)
    || typeof value.primary !== "string" || !KEY.test(value.primary) || typeof value.token !== "string" || !TOKEN.test(value.token)) return null;
  return { expires: value.expires, setup: value.setup, relay: value.relay, address: value.address, primary: value.primary, token: value.token };
}

/** The response can cross the durable request forwarder without exposing the bearer token. */
export async function installerCodeFrom(issue: (id: string) => Promise<Acted>): Promise<Acted> {
  const token = crypto.randomBytes(24).toString("base64url");
  const reply = await issue(hash(token));
  if (reply.status !== 201 || !isObject(reply.value)) return reply;
  const { expires } = reply.value;
  return { status: 201, value: { code: CODE + Buffer.from(JSON.stringify({ ...reply.value, token })).toString("base64url"), expires } };
}

export async function createInstallerInvite(id: unknown, asker?: Asker): Promise<Acted> {
  if (!addComputersLayoutReady(K)) return refusal(503, "Wait for the Kuvo Code update to finish before adding a computer.");
  if (IS_MAC) await macKeys({ needed: true });
  if (typeof id !== "string" || !TOKEN_HASH.test(id)) return refusal(400, "Send the installer invitation hash.");
  const actor = asker?.key && { key: asker.key.toString("base64url"), pairing: asker.pairing ?? "" };
  let reply = refusal(409, "Only the current primary can issue an installer code.");
  const changed = updateLink(link => {
    if (!link) return null;
    settleOwnSetup(link);
    const setup = followedSetup(link);
    if (setup?.role !== "primary") return null;
    if (requiresStandby(setup) && !setup.standby) {
      reply = refusal(503, "Your primary needs a standby before it can issue installer codes. You can add a replacement standby over SSH.");
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
    if (prior && (prior.actor?.key !== actor?.key || prior.actor?.pairing !== actor?.pairing || prior.setup !== setup.id || prior.epoch !== setup.epoch || !isLive(prior, now))) {
      reply = refusal(409, "This installer invitation is no longer available.");
      return null;
    }
    const expires = prior?.expires ?? now + INVITE_LIFETIME;
    const invites = Object.entries(link.installerInvites ?? {}).filter(([, invite]) => isLive(invite, now));
    if (!prior && invites.length >= 32) {
      reply = refusal(409, "There are too many installers waiting to join. Use an existing code or wait for it to expire.");
      return null;
    }
    link.installerInvites = Object.fromEntries(invites);
    link.installerInvites[id] = prior ?? { setup: setup.id, epoch: setup.epoch, expires, actor };
    const content: Omit<Code, "token"> = { expires, setup: setup.id, relay: link.relay, address: relayAddress(link.secret), primary: setup.primary };
    reply = { status: 201, value: content };
    return link;
  }, { durable: true });
  if (changed) {
    if (IS_MAC) macLink.wake?.();
    else if (!runningBridge() && !machineLink.host) startBridge();
  }
  return reply;
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

export function receiveInstallerClaim(body: Buffer, link: LinkState | null, host: LinkHost | null | undefined, from?: Buffer): boolean {
  if (!body.subarray(0, PREFIX.length).equals(PREFIX)) return false;
  if (!host || !from || !link?.key || !link.installerInvites || body.length > 2_000) return true;
  const message = opened(body, link.key);
  if (!message) return true;
  const { sender, value } = message;
  if (typeof value.token !== "string" || !TOKEN.test(value.token) || typeof value.sign !== "string" || !SIGN.test(value.sign)
    || typeof value.address !== "string" || !ADDRESS.test(value.address) || typeof value.nonce !== "string" || !TOKEN.test(value.nonce)
    || (value.kind !== "mac" && value.kind !== "machine") || typeof value.name !== "string" || value.name.length > 100) return true;
  const peer = { key: sender, sign: value.sign, address: value.address, kind: value.kind, name: plainName(value.name).slice(0, 100) || "Computer" } satisfies Peer;
  const id = hash(value.token);
  const nonce = value.nonce;
  const invite = link.installerInvites?.[id];
  if (!invite) return true;
  const send = (reply: object) => host.send(seal(link.key!, sender, from, { nonce, ...reply }));
  const position = followedSetup(link);
  const now = Date.now();
  if (position?.role !== "primary" || position.id !== invite.setup || position.epoch !== invite.epoch) {
    send({ message: "Your setup’s primary changed. Get a new installer code from the same setup.", retrySameSetup: true });
    return true;
  }
  const claimedBy = (entry: InstallerInvite | undefined) => entry?.claim?.key === peer.key && entry.claim.sign === peer.sign && entry.claim.address === peer.address;
  const allowed = (current: LinkState | null, entry: InstallerInvite | undefined, saved = false) => {
    const setup = followedSetup(current);
    return Boolean(current && entry && isLive(entry, Date.now()) && setup?.role === "primary" && setup.id === entry.setup
      && setup.epoch === entry.epoch && !current.localWipe && !current.localUnlink && !localRemovalPending(current)
      && (saved || !entry.actor || pairedPhone(current, entry.actor.key, entry.actor.pairing)) && (!entry.claim || claimedBy(entry)));
  };
  if (!allowed(link, invite, Boolean(invite.claim))) {
    send({ message: "This installer code is no longer available. Get a new code from your setup." });
    return true;
  }
  if (value.confirmed !== true) {
    if (primaryFenced(link)) return true;
    send({ ready: true });
    return true;
  }
  // Save the claim before starting the roster operation so a restart can take the same request again.
  if (!invite.claim) {
    const changed = updateLink(current => {
      const entry = current?.installerInvites?.[id];
      if (!current || !entry || !allowed(current, entry)) return null;
      entry.claim ??= { key: peer.key, sign: peer.sign, address: peer.address, until: now + CLAIM_LIFETIME };
      return current;
    }, { durable: true });
    if (!changed) return true;
  }
  const response: { doc?: OwnDoc } = {};
  const committed = updateLink(current => {
    if (!current || !allowed(current, current.installerInvites?.[id], true) || primaryFenced(current)) return null;
    const present = peersOf(current).find(each => each.key === peer.key && each.sign === peer.sign && each.address === peer.address);
    if (!present) return null;
    const doc = ownDoc(current, "peers", primaryPeersBody(current));
    if (doc) response.doc = doc;
    return doc?.changed ? current : null;
  });
  if (response.doc) {
    for (const docs of docMails({ give: [response.doc.envelope] })) send({ docs });
    if (committed) macDocsChanged();
    return true;
  }
  const pending = operationOutcome(readLink(), position.id, id);
  if (pending?.status === "pending") return true;
  if (!allowed(link, invite)) {
    send({ message: "This installer code is no longer available. Get a new code from your setup." });
    return true;
  }
  if (primaryFenced(link) || pendingShared(readLink(), "membershipAndAccess")) return true;
  try {
    const { save } = stageInstallerEnrollment(peer, position.id, id, current => {
      const entry = current?.installerInvites?.[id];
      if (!claimedBy(entry) || !allowed(current, entry)) throw new Error("This installer code is no longer available.");
    });
    if (save) macDocsChanged();
  } catch (error) {
    // Another write may have won the slot since the read. Leave that request pending.
    if (error instanceof Failed && !pendingShared(readLink(), "membershipAndAccess")) {
      send({ message: error.message, ...(error.again ? { retrySameSetup: true } : {}) });
    }
  }
  return true;
}

function exchange(
  code: Code, key: string, peer: { address: string; sign: string; name: string; kind: Peer["kind"] },
  reserve: () => void,
): Promise<ReturnType<typeof readDoc>> {
  return new Promise((resolve, reject) => {
    const nonce = crypto.randomBytes(24).toString("base64url");
    const secret = crypto.randomBytes(32);
    const address = bytes(code.address);
    const payload = { ...peer, nonce, token: code.token };
    let request = seal(key, code.primary, address, payload);
    const socket = new WebSocket(code.relay);
    const parts = new Map<string, PendingParts>();
    socket.binaryType = "arraybuffer";
    let again: ReturnType<typeof setInterval> | undefined;
    const deadline = setTimeout(() => finish(null, new Error("Your primary didn't finish adding this computer. Check that it’s online and run the installer again with the same code.")), 60_000);
    function finish(doc: ReturnType<typeof readDoc>, error?: Error) {
      clearTimeout(deadline);
      clearInterval(again);
      try { socket.close(); } catch {}
      if (error) reject(error);
      else resolve(doc);
    }
    const send = () => { if (socket.readyState === WebSocket.OPEN) socket.send(request); };
    let confirmed = false;
    socket.onopen = () => socket.send(relayHello(secret));
    socket.onerror = socket.onclose = () => finish(null, new Error("Couldn't connect to your setup's relay. Run the installer again with the same code."));
    socket.onmessage = event => {
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
      if (response.value.ready === true && !confirmed) {
        try {
          reserve();
          request = seal(key, code.primary, address, { ...payload, confirmed: true });
          confirmed = true;
          send();
        } catch (error) {
          finish(null, error instanceof Error ? error : new Error("This computer couldn't reserve its enrollment."));
        }
        return;
      }
      const { docs } = response.value;
      if (typeof response.value.message === "string") {
        return finish(null, response.value.retrySameSetup === true ? new Error(response.value.message) : new ClaimRefused(response.value.message));
      }
      if (!confirmed || !isObject(docs)) return;
      const doc = readDoc(Array.isArray(docs.give) ? docs.give[0] : assemble(parts, code.primary, docs.part));
      if (doc) finish(doc);
    };
  });
}

export async function joinWithInstallerCode(input: string): Promise<{ setup: string; primary: string }> {
  const code = readCode(input.trim());
  if (!code) throw new Error("Enter the installer code shown on your device.");
  if (!addComputersLayoutReady(K)) throw new Error("Wait for the Kuvo Code update to finish before adding a computer.");
  if (IS_MAC) {
    macLink.config ??= savedMacConfig() ?? ownMacConfig();
    if (macLink.config?.relay !== code.relay || !await macKeys({ needed: true })) throw new Error("This Mac cannot connect to the relay in that installer code.");
  }
  const claimKey = hash(code.token);
  const canJoin = (current: LinkState) => {
    if (!current.localUnlink && !current.localWipe && !current.removalSource && current.setup?.role === "member"
      && current.setup.primary === code.primary && current.installerClaim?.invite === claimKey) return;
    if (current.localUnlink || current.localWipe || current.removalSource || current.setup?.role === "member" || macsOf(current).length
      || peersOf(current).length || current.phones?.length || current.preJoinPairings?.length) {
      throw new Error("This computer already belongs to a setup or has paired devices. Unlink it before adding it to another setup.");
    }
    if (current.sharedSetup === current.setup?.id && Object.keys(current.shared ?? {}).length || allItems().length) {
      throw new Error("This computer has setup data. Use a setup merge to bring that data along.");
    }
    if (fs.lstatSync(path.join(path.dirname(K), ADD_RESERVATION), { throwIfNoEntry: false })) throw new Error("An SSH setup is already in progress on this computer.");
    if (current.installerClaim && current.installerClaim.until > Date.now() && current.installerClaim.setup !== code.setup) {
      throw new Error("An installer enrollment is already in progress for another setup. Retry with its code.");
    }
  };
  const local = updateLink(current => {
    const link = current ?? { phones: [] };
    canJoin(link);
    if (Date.now() >= code.expires && link.installerClaim?.invite !== claimKey) {
      throw new Error("This installer code has expired. Get a new code from your setup.");
    }
    if (IS_MAC && (!link.key || !link.sign || !link.secret)) throw new Error("Kuvo couldn’t read this Mac's identity.");
    link.key ??= newKey().toString("base64url");
    link.sign ??= newSigningSeed();
    link.secret ??= crypto.randomBytes(32).toString("base64url");
    link.name ??= computerName();
    if (ownKey(link) === code.primary) throw new Error("This computer is already your primary.");
    return link;
  }, { durable: true })!;
  if (local.setup?.role === "member") return { setup: local.setup.id, primary: local.setup.primary };
  const key = ownKey(local)!;
  const sign = ownSign(local)!;
  const address = relayAddress(local.secret!);
  let doc;
  try {
    doc = await exchange(code, local.key!, { address, sign, name: local.name!, kind: IS_MAC ? "mac" : "machine" }, () => {
      const reserved = updateLink(current => {
        if (!current || ownKey(current) !== key || ownSign(current) !== sign || relayAddress(current.secret ?? "") !== address) return null;
        canJoin(current);
        current.installerClaim = { setup: code.setup, invite: claimKey, until: Date.now() + CLAIM_LIFETIME };
        return current;
      }, { durable: true });
      if (!reserved) throw new Error("This computer’s identity changed. Run the installer again.");
    });
  } catch (error) {
    if (error instanceof ClaimRefused) updateLink(current => {
      if (!current || current.setup?.role === "member" || current.installerClaim?.invite !== claimKey) return null;
      delete current.installerClaim;
      return current;
    }, { durable: true });
    throw error;
  }
  const setup = ledSetup(doc);
  if (!doc || !setup || setup.id !== code.setup || setup.primary !== code.primary || !Array.isArray(doc.body.boxes)
    || !doc.body.boxes.some(peer => isObject(peer) && peer.key === key && peer.sign === sign && peer.address === address)) {
    throw new Error("The primary didn’t send this computer's accepted membership.");
  }
  const changed = updateLink(current => {
    if (!current || current.installerClaim?.invite !== claimKey || ownKey(current) !== key || ownSign(current) !== sign) return null;
    canJoin(current);
    current.mac = setup.primary;
    current.macs = [setup.primary];
    current.macSigns = { [setup.primary]: doc.origin };
    current.setup = { ...setup, sign: doc.origin, role: "member", ...managedFields(doc), managed: true };
    current.relay = code.relay;
    delete current.pairing;
    delete current.appPairing;
    docsWork({ link: current, library: readPersonalities() }, { key: setup.primary, sign: doc.origin }, { give: [doc.envelope] });
    return current;
  }, { durable: true });
  if (!changed) throw new Error("The installer's enrollment changed. Run it again with the same code.");
  writeCard();
  noteDocsChanged();
  if (!IS_MAC && !runningBridge()) startBridge();
  return { setup: setup.id, primary: setup.primary };
}
