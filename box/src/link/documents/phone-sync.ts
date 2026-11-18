// A fresh phones document for a member's relay connection, and receipts for a desktop
// removal the primary needs to confirm before it shuts down. Checks are scoped to the
// authenticated sending box and this connection's random challenge.

import { randomBytes } from "node:crypto";
import { isObject } from "../../shared/json";
import { IS_MAC } from "../home";
import { macSign, readLink, updateLink } from "../state";
import type { LinkState } from "../state";
import { macsOf, mailFor, peersOf } from "../peers/known";
import type { MailSender, Peer } from "../peers/known";
import { ownPhones, pairedPhone, removeAsMac } from "../phones";
import type { LinkHost } from "../relay/host";
import { mainElsewhere } from "../setup/main-only";
import { docKey, held, readDoc } from "./format";
import { docMails, ownDoc } from "./work";
import type { SignedDoc } from "../../shared/documents";

const challenge = (): string => randomBytes(16).toString("base64url");
interface Freshness { id: string; sign: string; asked: number; ready: boolean }
const freshness = new WeakMap<LinkHost, Map<string, Freshness>>();
interface Receipt { pending: Set<string>; fingerprint: string }
const receipts = new WeakMap<LinkHost, Map<string, Receipt>>();

function send(host: LinkHost, link: LinkState, to: Peer | MailSender, message: Record<string, unknown>): void {
  const bytes = mailFor(to, { ...message, at: Date.now() }, link);
  if (bytes) host.send(bytes);
}

function givePhones(host: LinkHost, link: LinkState, peer: Peer | MailSender, envelope: SignedDoc): void {
  for (const docs of docMails({ give: [envelope] })) send(host, link, peer, { docs });
}

const fingerprint = (doc: { n: number; h: string }): string => `${doc.n}:${doc.h}`;

function authoritySign(link: LinkState, key: string): string | undefined {
  return macSign(link, key) ?? (link.setup?.primary === key ? link.setup.sign : undefined);
}

/** Request each of the member's authorities' current signed lists on this connection. */
export function refreshPhoneTrust(host: LinkHost): void {
  if (!host.welcomed) return;
  const link = readLink();
  const main = mainElsewhere(link);
  if (!link || !main) return;
  const pending = freshness.get(host) ?? new Map<string, Freshness>();
  freshness.set(host, pending);
  for (const key of new Set([main.key, ...macsOf(link)])) {
    const sign = authoritySign(link, key);
    const peer = peersOf(link).find(each => each.key === key);
    if (!peer || !sign) continue;
    const known = pending.get(key);
    if (known?.sign === sign && (known.ready || Date.now() - known.asked < 2_000)) continue;
    const next = { id: known?.sign === sign ? known.id : challenge(), sign, asked: Date.now(), ready: false };
    pending.set(key, next);
    send(host, link, peer, { phoneSync: { id: next.id } });
  }
}

/** Cached desktop pairings (and devices descended from a voucher) don't open on reconnect until
 * their authority answers a fresh challenge with a signed list. Independent phones still can. */
export function freshDesktopTrust(host: LinkHost, key: Buffer): boolean {
  const link = readLink();
  if (!link || !mainElsewhere(link)) return true;
  const phone = pairedPhone(link, key.toString("base64url"));
  if (phone?.from !== "mac" || (phone.kind !== "desktop" && !phone.by)) return true;
  const known = phone.mac && freshness.get(host)?.get(phone.mac);
  return Boolean(known && known.ready && phone.mac && known.sign === authoritySign(link, phone.mac));
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
    const known = freshness.get(host)?.get(sender.key);
    if (known?.id === id && known.sign === authoritySign(link, sender.key)) {
      const doc = held(link.docs?.[docKey(known.sign, "phones")]);
      if (doc && fingerprint(doc) === given.list) known.ready = true;
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
  if (!IS_MAC || !peersOf(link).some(peer => peer.key === sender.key)) return;
  let envelope: SignedDoc | undefined;
  const latest = updateLink(current => {
    if (!current || !peersOf(current).some(peer => peer.key === sender.key)) return null;
    envelope = ownDoc(current, "phones", ownPhones(current))?.envelope;
    return envelope ? current : null;
  });
  const doc = envelope && readDoc(envelope);
  if (latest && envelope && doc) {
    givePhones(host, latest, sender, envelope);
    send(host, latest, sender, { phoneSync: { id, list: fingerprint(doc), ack: true } });
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
      givePhones(host, link, peer, envelope);
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
