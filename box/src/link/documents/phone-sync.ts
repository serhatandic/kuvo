// A fresh phones document for a member's relay connection, and receipts for a desktop
// removal the primary needs to confirm before it shuts down. Checks are scoped to the
// authenticated sending box and this connection's random challenge.

import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { writePrivate } from "../../shared/files";
import { IS_MAC, OWN_HOME_MAC, RUN } from "../home";
import { runningBridge } from "../keeper";
import { runningServe } from "../serve-kept";
import { macSign, readLink, updateLink } from "../state";
import type { LinkState } from "../state";
import { macsOf, mailFor, ownKey, peersOf } from "../peers/known";
import type { MailSender, Peer } from "../peers/known";
import { authorizePhonePairing, currentPhonePairings, ownPhones, pairedPhone } from "../phones";
import type { LinkHost, PhoneHello } from "../relay/host";
import { mainElsewhere } from "../setup/main-only";
import { acceptedMembership } from "../setup/membership";
import { publishedShared } from "../setup/shared";
import { removeDevicesHere } from "../peers/devices";
import { followedSetup } from "../setup/record";
import { docKey, held, ownSign, readDoc, SIGN } from "./format";
import { digestOf, docMails, primaryPeersBody, ownDoc } from "./work";
import type { DocSender } from "./work";
import type { SignedDoc } from "../../shared/documents";

const challenge = (): string => randomBytes(16).toString("base64url");
interface ListCheck {
  id: string;
  ready: boolean;
  legacyPhones?: string;
}
interface PhoneTrust {
  lists: Map<string, ListCheck>;
  asked: number;
  probed: Set<string>;
  offline: Set<string>;
  saved?: string;
}
const freshness = new WeakMap<Pick<LinkHost, "send">, PhoneTrust>();
const TRUST_FILE = path.join(RUN, "phone-trust.json");
interface KnownTrust { ready: string[]; offline: string[] }
interface Receipt { pending: Set<string>; fingerprint: string }
const receipts = new WeakMap<LinkHost, Map<string, Receipt>>();

function send(host: LinkHost, link: LinkState, to: Peer | MailSender, message: Record<string, unknown>): void {
  const bytes = mailFor(to, { ...message, at: Date.now() }, link);
  if (bytes) host.send(bytes);
}

function giveDoc(host: LinkHost, link: LinkState, peer: Peer | MailSender, envelope: SignedDoc): void {
  for (const docs of docMails({ give: [envelope] })) send(host, link, peer, { docs });
}

const fingerprint = (doc: { n: number; h: string }): string => `${doc.n}:${doc.h}`;

function authoritySign(link: LinkState, key: string): string | undefined {
  return macSign(link, key) ?? (link.setup?.primary === key ? link.setup.sign : undefined);
}

const knownTrust = (pending: PhoneTrust): KnownTrust => ({
  ready: [...pending.lists].filter(([, each]) => each.ready).map(([origin]) => origin),
  offline: [...pending.offline],
});

// Addressed SSH is answered in a separate process. Only trust what its running Box recorded.
function keepTrust(pending: PhoneTrust): void {
  if (IS_MAC && !OWN_HOME_MAC) return;
  const text = JSON.stringify({ pid: process.pid, ...knownTrust(pending) });
  if (pending.saved === text) return;
  try {
    writePrivate(TRUST_FILE, text);
    pending.saved = text;
  } catch {}
}

export function clearPhoneTrust(host: LinkHost): void {
  if (freshness.delete(host) && (!IS_MAC || OWN_HOME_MAC)) try { fs.rmSync(TRUST_FILE, { force: true }); } catch {}
}

function savedTrust(): KnownTrust | undefined {
  try {
    const record = JSON.parse(fs.readFileSync(TRUST_FILE, "utf8"));
    const owner = IS_MAC ? OWN_HOME_MAC ? runningServe()?.pid : undefined : runningBridge();
    if (owner && record.pid === owner && Array.isArray(record.ready) && Array.isArray(record.offline)) return record;
  } catch {}
  return undefined;
}

/** Request each signing Mac's list from all known peers, including other members that hold it. */
export function refreshPhoneTrust(host: LinkHost): void {
  if (!host.welcomed) return;
  const link = readLink();
  const main = mainElsewhere(link);
  if (!link || !main) return;
  const pending: PhoneTrust = freshness.get(host) ?? { lists: new Map(), asked: 0, probed: new Set(), offline: new Set() };
  freshness.set(host, pending);
  if (Date.now() - pending.asked < 2_000) return;
  pending.asked = Date.now();
  const peers = peersOf(link);
  const origins = new Set((link.setup?.managed ? [main.key] : [main.key, ...macsOf(link)]).map(key => authoritySign(link, key)).filter((sign): sign is string => Boolean(sign)));
  for (const origin of origins) {
    const check = pending.lists.get(origin) ?? { id: challenge(), ready: false };
    if (check.ready) continue;
    pending.lists.set(origin, check);
    for (const peer of peers) {
      pending.probed.add(peer.address);
      send(host, link, peer, { phoneSync: { id: check.id, origin } });
    }
  }
  keepTrust(pending);
}

/** Relay delivery and unreachable notices describe reachability on this connection only. */
export function notePhoneTrustReach(host: LinkHost, address: Buffer, reachable: boolean): void {
  const pending = freshness.get(host);
  const at = address.toString("base64url");
  if (!pending?.probed.has(at)) return;
  if (reachable) pending.offline.delete(at);
  else pending.offline.add(at);
  keepTrust(pending);
}

const checking = { reason: "pairing-wait", message: "Checking this app with your other computers…" } as const;
const readd = {
  reason: "pairing-readd",
  title: "This app needs to be paired again",
  message: "The computer that added this app was removed. Pair this app again from one of your computers.",
  action: { label: "Pair Again…", route: "kuvo/devices/pair" },
} as const;

// The signed compatibility document and the accepted grant snapshot can arrive separately.
function sameGrants(one: unknown, other: unknown): boolean {
  const entries = (value: unknown) => Array.isArray(value)
    ? value.filter(isObject).map(entry => JSON.stringify([entry.key, entry.pairing ?? ""])).sort()
    : [];
  return JSON.stringify(entries(one)) === JSON.stringify(entries(other));
}

/** Refresh relay access before accepting a cached pairing from this setup. */

export function phoneTrustWait(link: LinkState | null, key: string, host?: Pick<LinkHost, "send"> | null, { queued = false } = {}): typeof checking | typeof readd | undefined {
  const main = mainElsewhere(link);
  if (!link || !main) return;
  const phone = pairedPhone(link, key);
  if (!phone) return;
  const setup = link.setup;
  if (setup?.aside) return readd;
  if (!queued && !setup?.checkpointed && (phone.from !== "mac" || (!setup?.managed && phone.kind !== "desktop"))) return;

  const origin = setup?.checkpointed || queued && setup?.managed ? setup?.sign
    : (phone.mac && authoritySign(link, phone.mac)) || (queued && authoritySign(link, main.key));
  if (!origin) return queued ? checking : readd;
  const pending = host && freshness.get(host);
  const checked = pending ? knownTrust(pending) : host === undefined ? savedTrust() : undefined;
  if (setup?.checkpointed) {
    const membership = acceptedMembership(link);
    const phones = macsOf(link).includes(setup.primary) && held(link.docs?.[docKey(origin, "phones")]);
    if (checked?.ready.includes(origin) && membership?.boxes.some(peer => peer.key === ownKey(link))
      && phones && phones.body.epoch === setup.epoch
      && sameGrants(phones.body.phones, membership.devices.phones)
      && sameGrants(phones.body.removed, membership.devices.removed)) return;
  } else if (checked && (checked.ready.includes(origin) || (!queued && !setup?.managed && peersOf(link).every(peer => checked.offline.includes(peer.address))))) return;
  return checking;
}

export function authorizeDeviceConnection(host: LinkHost, key: Buffer, hello: PhoneHello, address: Buffer, signal: AbortSignal) {
  const link = readLink();
  const encoded = key.toString("base64url");
  const waiting = phoneTrustWait(link, encoded, host);
  if (waiting) return { ...waiting, waiting: waiting.message };
  return authorizePhonePairing(key, hello, address, signal);
}

/** A signing Mac with an older phoneSync sends its membership in the normal document exchange.
 * It must give the signed document itself, even when it is the same version we already had. */
export function confirmPhoneMembership(host: LinkHost, sender: DocSender, link: LinkState, given: readonly SignedDoc[]): void {
  const pending = freshness.get(host);
  const origin = authoritySign(link, sender.key);
  const check = origin && pending?.lists.get(origin);
  if (link.setup?.checkpointed || !origin || !pending || !check || check.ready || !check.legacyPhones) return;
  const peers = readDoc(link.docs?.[docKey(origin, "peers")]);
  if (peers?.origin !== origin || peers.kind !== "peers" || !given.some(each => each?.d === peers.envelope.d && each.s === peers.envelope.s)) return;
  const phones = readDoc(link.docs?.[docKey(origin, "phones")]);
  if (phones?.origin !== origin || phones.kind !== "phones" || fingerprint(phones) !== check.legacyPhones) return;
  check.ready = true;
  keepTrust(pending);
}

/** The caller has already been authenticated by peer mail. A proof is accepted only after the
 * signed document preceding it was installed. A receipt confirms the installed list. */
export function takePhoneSync(sender: MailSender, given: unknown, host: LinkHost | null | undefined): void {
  if (!host?.welcomed || !isObject(given) || typeof given.id !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(given.id)) return;
  const link = readLink();
  if (!link) return;
  const { id } = given;
  if (given.ack === true && typeof given.list === "string") {
    const requested = receipts.get(host)?.get(id);
    if (requested?.fingerprint === given.list) requested.pending.delete(sender.key);
    if (!peersOf(link).some(peer => peer.key === sender.key)) return;
    const pending = freshness.get(host);
    if (!pending) return;
    for (const [origin, check] of pending.lists) {
      if (check.id !== id) continue;
      if (link.setup?.managed && sender.key !== link.setup.primary) return;
      if (check.ready) return;
      // Both signed documents must match the ones installed here. An older list can't replace
      // the one this box holds, and a phones-only answer can't confirm who's still a peer.
      const phones = readDoc(link.docs?.[docKey(origin, "phones")]);
      if (phones?.origin !== origin || phones.kind !== "phones" || fingerprint(phones) !== given.list) return;
      if (typeof given.peers === "string") {
        const peers = readDoc(link.docs?.[docKey(origin, "peers")]);
        if (peers?.origin !== origin || peers.kind !== "peers" || fingerprint(peers) !== given.peers) return;
        check.ready = true;
        keepTrust(pending);
      } else if (given.peers === undefined && authoritySign(link, sender.key) === origin) {
        check.legacyPhones = given.list;
        const have = digestOf(link).filter(([key, kind]) => key !== origin || kind !== "peers");
        send(host, link, sender, { docs: { have, reply: true } });
      }
      return;
    }
    return;
  }
  if (typeof given.list === "string") {
    const sign = authoritySign(link, sender.key);
    const doc = sign && held(link.docs?.[docKey(sign, "phones")]);
    if (doc && fingerprint(doc) === given.list) {
      host.keepOnly(currentPhonePairings(link));
      send(host, link, sender, { phoneSync: { id, list: given.list, ack: true } });
    }
    return;
  }
  if (!peersOf(link).some(peer => peer.key === sender.key)) return;
  const self = mainElsewhere(link) ? null : followedSetup(link)?.role === "primary" || IS_MAC ? ownSign(link) : null;
  const origin = given.origin === undefined ? self : given.origin;
  if (typeof origin !== "string" || !SIGN.test(origin)) return;
  let envelope = link.docs?.[docKey(origin, "phones")];
  let membership = link.docs?.[docKey(origin, "peers")];
  let latest = link;
  if (origin === self) {
    envelope = undefined;
    const changed = updateLink(current => {
      if (!current || mainElsewhere(current) || ownSign(current) !== origin || !peersOf(current).some(peer => peer.key === sender.key)) return null;
      envelope = ownDoc(current, "phones", ownPhones(current))?.envelope;
      membership = ownDoc(current, "peers", primaryPeersBody(current))?.envelope;
      return envelope ? current : null;
    });
    if (!changed) return;
    latest = changed;
  }
  const doc = envelope && readDoc(envelope);
  if (envelope && doc?.origin === origin && doc.kind === "phones") {
    const word = membership && readDoc(membership);
    if (membership && word?.kind === "peers" && word.origin === origin) giveDoc(host, latest, sender, membership);
    giveDoc(host, latest, sender, envelope);
    if (origin === self) for (const shared of publishedShared(latest)) giveDoc(host, latest, sender, shared);
    send(host, latest, sender, { phoneSync: { id, list: fingerprint(doc), ...(word ? { peers: fingerprint(word) } : {}), ack: true } });
  }
}

/** On the live primary, revoke and close before sending the signed list. An unconfirmed peer
 * (including one that is offline) keeps the uninstall from reporting complete spreading. */
export async function publishDesktopRemoval(host: LinkHost | null, key: string, signal?: AbortSignal): Promise<boolean> {
  const removed = await removeDevicesHere([{ key }], null);
  if (removed.status !== 200) return false;
  let envelope: SignedDoc | undefined;
  const link = updateLink(current => {
    if (!current || mainElsewhere(current)) return null;
    envelope = ownDoc(current, "phones", ownPhones(current))?.envelope;
    return current;
  });
  if (!link) return false;
  host?.keepOnly(currentPhonePairings(link));
  const peers = peersOf(link);
  if (!peers.length) return true;
  const doc = envelope && readDoc(envelope);
  if (!host?.welcomed || !doc || !envelope) return false;
  const id = challenge();
  const receipt = { fingerprint: fingerprint(doc), pending: new Set(peers.map(peer => peer.key)) };
  const requests = receipts.get(host) ?? new Map<string, Receipt>();
  receipts.set(host, requests);
  requests.set(id, receipt);
  const deadline = Date.now() + 5_000;
  try {
    for (const peer of peers) {
      giveDoc(host, link, peer, envelope);
      send(host, link, peer, { phoneSync: { id, list: receipt.fingerprint } });
    }
    while (receipt.pending.size && host.welcomed && !signal?.aborted && Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    return !receipt.pending.size;
  } finally {
    requests.delete(id);
  }
}
