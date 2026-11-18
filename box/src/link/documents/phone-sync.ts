// A fresh phones document for a member's relay connection, and receipts for a desktop
// removal the primary needs to confirm before it shuts down. Checks are scoped to the
// authenticated sending box and this connection's random challenge.

import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { writePrivate } from "../../shared/files";
import { IS_MAC, RUN } from "../home";
import { runningBridge } from "../keeper";
import { macSign, readLink, updateLink } from "../state";
import type { LinkState } from "../state";
import { macsOf, mailFor, peersOf } from "../peers/known";
import type { MailSender, Peer } from "../peers/known";
import { authorizePhonePairing, ownPhones, pairedPhone, removeAsMac } from "../phones";
import type { LinkHost, PhoneHello } from "../relay/host";
import { mainElsewhere } from "../setup/main-only";
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
const freshness = new WeakMap<LinkHost, PhoneTrust>();
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

// The local server for a machine runs separately from the process that owns the relay.
function keepTrust(pending: PhoneTrust): void {
  if (IS_MAC) return;
  const text = JSON.stringify({ pid: process.pid, ...knownTrust(pending) });
  if (pending.saved === text) return;
  try {
    writePrivate(TRUST_FILE, text);
    pending.saved = text;
  } catch {}
}

export function clearPhoneTrust(host: LinkHost): void {
  if (!IS_MAC && freshness.has(host)) try { fs.rmSync(TRUST_FILE, { force: true }); } catch {}
}

function savedTrust(): KnownTrust | undefined {
  try {
    const record = JSON.parse(fs.readFileSync(TRUST_FILE, "utf8"));
    if (record.pid === runningBridge() && Array.isArray(record.ready) && Array.isArray(record.offline)) return record;
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
  const origins = new Set([main.key, ...macsOf(link)].map(key => authoritySign(link, key)).filter((sign): sign is string => Boolean(sign)));
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

/** Accept an inherited desktop pairing once a peer confirms the signed list or the relay
 * reports all peers unreachable. Keep asking while relying on the cached authorization. */
export function phoneTrustWait(link: LinkState | null, key: string, host?: LinkHost | null): typeof checking | typeof readd | undefined {
  if (!link || !mainElsewhere(link)) return;
  const phone = pairedPhone(link, key);
  if (phone?.from !== "mac" || (phone.kind !== "desktop" && !phone.by)) return;
  const origin = phone.mac && authoritySign(link, phone.mac);
  if (!origin) return readd;
  const pending = host && freshness.get(host);
  const checked = pending ? knownTrust(pending) : host === undefined && !IS_MAC ? savedTrust() : undefined;
  if (checked && (checked.ready.includes(origin) || peersOf(link).every(peer => checked.offline.includes(peer.address)))) return;
  return checking;
}

export function authorizeDesktopConnection(host: LinkHost, key: Buffer, hello: PhoneHello, address: Buffer) {
  const waiting = phoneTrustWait(readLink(), key.toString("base64url"), host);
  return waiting ? { ...waiting, waiting: waiting.message } : authorizePhonePairing(key, hello, address);
}

/** A signing Mac with an older phoneSync sends its membership in the normal document exchange.
 * It must give the signed document itself, even when it is the same version we already had. */
export function confirmPhoneMembership(host: LinkHost, sender: DocSender, link: LinkState, given: readonly SignedDoc[]): void {
  const pending = freshness.get(host);
  const origin = authoritySign(link, sender.key);
  const check = origin && pending?.lists.get(origin);
  if (!origin || !pending || !check || check.ready || !check.legacyPhones) return;
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
      host.keepOnly(new Set((link.phones ?? []).map(phone => phone.key)));
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
    send(host, latest, sender, { phoneSync: { id, list: fingerprint(doc), ...(word ? { peers: fingerprint(word) } : {}), ack: true } });
  }
}

/** On the live primary, revoke and close before sending the signed list. An unconfirmed peer
 * (including one that is offline) keeps the uninstall from reporting complete spreading. */
export async function publishDesktopRemoval(host: LinkHost | null, key: string, signal?: AbortSignal): Promise<boolean> {
  let envelope: SignedDoc | undefined;
  const link = updateLink(current => {
    if (!current || mainElsewhere(current) || !removeAsMac(current, [{ key }])) return null;
    envelope = ownDoc(current, "phones", ownPhones(current))?.envelope;
    return current;
  });
  if (!link) return false;
  host?.keepOnly(new Set((link.phones ?? []).map(phone => phone.key)));
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
