// Paired phones (PROTOCOL.md, "Paired phones")
// A pairing adds a phone; a removal, from a phone on the list, takes one off for good (`removed`,
// with the pairing it ended): only a pairing of its own, with a new pairing id, lets it back in,
// and nobody removes a Mac. The phones this box pairs and removes are its own phones document; a
// Mac's come in that Mac's, and a removal in any document this box takes is noted here too
// (PROTOCOL.md, "Documents"). The Mac's are the same rules (PairedPhones in KuvoLink), and both
// are held to protocol/vectors/link.json.

import { updateAccess } from "./setup/access";
import { primaryFenced } from "./setup/lease";
import crypto from "node:crypto";
import { isObject } from "../shared/json";
import { addressOf, publicOf } from "./crypto/keys";
import { noteDocsChanged } from "./documents/changed";
import { mainElsewhere } from "./setup/main-only";
import { held } from "./documents/format";
import { ADDRESS, KEY, macDoc, macsOf } from "./peers/known";
import { readLink, updateLink } from "./state";
import type { Doc, SignedDoc } from "../shared/documents";
import type { PhoneHello } from "./relay/host";
import type { LinkState, Pairing, Phone } from "./state";

/** The current pairing for a relay session or token issuer. Older pairings use the empty id. */
export function pairedPhone(link: LinkState | null | undefined, key: string, pairing?: string): Phone | undefined {
  return (link?.phones ?? []).find(phone => phone.key === key && (pairing === undefined || (phone.pairing ?? "") === pairing));
}

/** A device whose voucher is kept by another box must get its pairing links there. For old
 * documents that lacked voucher metadata, an inherited desktop pairing is treated the same way. */
export const canIssuePairing = (phone: Phone): boolean => phone.from !== "mac" || (!phone.by && phone.kind !== "desktop");

// Lets in phones in link.json, and a new one showing the pairing token (once, in time), noting
// where it is on the relay, for its notifications. Where a paired one is changes once its session
// proves its own (confirmPhone): the first handshake message can be replayed from elsewhere.
export function authorizePhonePairing(key: Buffer, hello: PhoneHello | null | undefined, address: Buffer | undefined): string | false {
  const encoded = key.toString("base64url");
  const current = () => {
    const phone = pairedPhone(readLink(), encoded);
    return phone ? phone.pairing ?? "" : false;
  };
  if (!hello?.pair) return current();
  // Showing the token: a pairing of its own, paired before or not (one removed comes back).
  let paired: string | undefined;
  const { save, changed } = updateAccess(link => {
    // A phone's (`kuvo/phones/pair`), or, on a Mac, the one its Settings › Phone shows: each apart,
    // so neither ends the other.
    const given = Buffer.from(String(hello!.pair), "base64url");
    const slot = (["pairing", "appPairing"] as const).find(name => {
      const pairing = link?.[name];
      // Older tokens in the shared slot could have been issued over the relay without an issuer.
      if (!pairing || Date.now() > pairing.expires || (name === "pairing" && pairing.issuer === undefined)) return false;
      const expected = Buffer.from(pairing.token, "base64url");
      return given.length === expected.length && crypto.timingSafeEqual(given, expected);
    });
    if (!slot) return null;
    const issuer = link![slot]!.issuer;
    const issuedBy = issuer && pairedPhone(link, issuer.key, issuer.pairing);
    if (mainElsewhere(link) || (issuer && (!issuedBy || !canIssuePairing(issuedBy)))) {
      delete link![slot];
      return link;
    }
    const by = issuedBy ? issuedBy.by : undefined;
    if (by && !hasVoucherSpace(link!, encoded)) return null;
    paired = newPairing();
    pairPhone(link!, { key: encoded, name: String(hello!.name ?? "").slice(0, 100), added: Math.floor(Date.now() / 1000), from: "pair", address: address?.toString("base64url"), pairing: paired, ...(hello!.client === "desktop" ? { kind: "desktop" as const } : {}), ...(by ? { by } : {}) });
    delete link![slot];
    return link;
  });
  if (save || !changed) return current();
  if (paired) noteDocsChanged();
  return paired ?? current();
}

export const authorizePhone = (...args: Parameters<typeof authorizePhonePairing>): boolean => authorizePhonePairing(...args) !== false;

const PAIRING = /^[A-Za-z0-9_-]{0,32}$/;

/** A phone named in a removal or a list, once `fits` has checked its key and pairing. */
export interface PhoneEntry {
  key: string;
  pairing?: string | null;
  [more: string]: unknown;
}

// At most so many in one removal, and so many removals kept (the oldest go first: a computer
// removed adds one for each device its box vouched for, 16 at most).
const MOST_AT_ONCE = 16;
const MOST_KEPT = 256;

const removedAlready = (link: LinkState, key: string, pairing: string) => (link.removed ?? []).some(r => r.key === key && r.pairing === pairing);

function noteRemoved(link: LinkState, key: string, pairing: string): void {
  link.removed ??= [];
  if (!removedAlready(link, key, pairing)) link.removed = [...link.removed, { key, pairing }].slice(-MOST_KEPT);
}

/** The most boxes kept from vouching at once (the oldest go first past it). */
const MOST_BLOCKED = 64;

// A device taken off for good that a box vouched for (`by`, PROTOCOL.md "Paired phones",
// `kuvo/devices/pair`): every other device that box vouched for goes with it, by the pairing each
// has now, noted among the removals as any removal is (so they're out on every computer), and
// that box vouches for no other from now on, until you allow it again (`allowVouching`). Whoever
// sits at that computer can't have its app make a new key and come back, nor keep one it had its
// box vouch for before, which could ask for the computer to be allowed again.
function blockVoucher(link: LinkState, phone: Phone, now: number): void {
  if (!phone.by) return;
  revokeVoucher(link, phone.by, phone.name ?? "", now);
}

export function revokeVoucher(link: LinkState, by: string, device = "", now = Date.now()): void {
  takeOffVouched(link, by);
  link.vouchBlocked = [...(link.vouchBlocked ?? []).filter(each => each.key !== by), { key: by, device, at: now }].slice(-MOST_BLOCKED);
}

/** Every device the box `by` vouched for taken off (in place), each by the pairing it has now and
 * noted among the removals as any removal is, so this box's phones document carries them and
 * they're out on every computer: whether any was. Other vouchers and independent pairings stay.
 * Also what removing that computer itself does (`setMacPeers`,
 * mac/routes.ts): Kuvo Code's app there no longer reaches the others with the key it had. That
 * alone keeps no box from vouching: one that's removed is no machine of this box's, so its vouch
 * isn't taken (`deviceAct`), and added again it vouches as a new one does. */
export function takeOffVouched(link: LinkState, by: string): boolean {
  const theirs = (link.phones ?? []).filter(each => each.by === by);
  for (const each of theirs) noteRemoved(link, each.key, each.pairing ?? "");
  if (theirs.length) link.phones = link.phones!.filter(each => each.by !== by);
  return theirs.length > 0;
}

/** A computer set up again with keys made anew (`from`, its box's key before; `to`, the one it has
 * now), in place: what's kept by its box's key goes by the new one, the devices it vouched for
 * and its being kept from vouching, so removing the computer still takes its app off your
 * devices, and new keys don't let it vouch again. */
export function vouchMoved(link: LinkState, from: string, to: string): void {
  if (from === to) return;
  for (const phone of link.phones ?? []) if (phone.by === from) phone.by = to;
  const blocked = vouchBlockOf(link, from);
  if (blocked) link.vouchBlocked = [...link.vouchBlocked!.filter(each => each.key !== from && each.key !== to), { ...blocked, key: to }];
}

/** What a confirm adds, before a computer is removed or uninstalled, where its box vouched for a
 * device this box lets in (the main computer alone knows): that Kuvo Code's app there goes too. */
export const vouchedWords = (link: LinkState | null | undefined, key: string | undefined, name: string): string =>
  (key && (link?.phones ?? []).some(each => each.by === key) ? ` Kuvo Code’s app on ${name} is taken off your devices.` : "");

/** The removal that keeps the box `key` from vouching now, if any. */
export const vouchBlockOf = (link: LinkState | null | undefined, key: string | undefined) => (key ? (link?.vouchBlocked ?? []).find(each => each.key === key) : undefined);

/** The box `key` vouches again (in place): whether anything changed. What was removed stays removed. */
export function allowVouching(link: LinkState, key: string, staging = false): boolean {
  if ((!staging && primaryFenced(link)) || !vouchBlockOf(link, key)) return false;
  link.vouchBlocked = link.vouchBlocked!.filter(each => each.key !== key);
  if (!link.vouchBlocked.length) delete link.vouchBlocked;
  return true;
}

// A phone's key, and its pairing if given, as a removal or a list names it: never a Mac's.
// (`any`: a value off the wire, which it checks itself.)
export const fits = (entry: any, macs: string[]): entry is PhoneEntry => typeof entry?.key === "string" && KEY.test(entry.key) && !macs.includes(entry.key)
  && (entry.pairing == null || (typeof entry.pairing === "string" && PAIRING.test(entry.pairing)));

// `key`'s pairing `pairing` (the one it has here, when not given) taken off: one paired again
// since, with another pairing, stays. The box that vouched for the one taken off vouches for no
// other (`blockVoucher`).
function takeOff(link: LinkState, key: string, pairing?: string | null, now = Date.now()): void {
  const current = (link.phones ?? []).find(phone => phone.key === key);
  const which = pairing == null ? (current?.pairing ?? "") : String(pairing);
  noteRemoved(link, key, which);
  if (!current || (current.pairing ?? "") !== which) return;
  link.phones = link.phones!.filter(phone => phone.key !== key);
  blockVoucher(link, current, now);
}

// A new pairing id, made where a phone pairs.
export const newPairing = () => crypto.randomBytes(12).toString("base64url");

// A phone paired here (its pairing token): in, with a pairing of its own, which nothing removed.
// What removed its others still keeps them out.
export function pairPhone(link: LinkState, phone: Phone): void {
  link.phones = [...(link.phones ?? []).filter(each => each.key !== phone.key), phone];
}

/** What `kuvo/devices/pair` came to here: the pairing the device has now, or that it was removed. */
export type DevicePaired = { pairing: string } | { removed: true } | { full: true } | { blocked: { device: string } };

/** The most desktop apps and voucher-bound devices paired this way at once. The limit keeps a
 * voucher from filling a phones document by issuing pairing links. */
export const MOST_DESKTOPS = 16;

const hasVoucherSpace = (link: LinkState, key: string): boolean =>
  (link.phones ?? []).filter(phone => phone.key !== key && (phone.kind === "desktop" || phone.by)).length < MOST_DESKTOPS;

// Kuvo Code's desktop app paired here with no token (`kuvo/devices/pair`: asked by the app beside
// this box, or vouched for by a member's box, PROTOCOL.md "Paired phones"), in place, under
// link.json's lock: one let in already keeps the pairing it has (asked again, nothing new is
// made); one removed, by any pairing, stays out, as only a pairing by token brings a removed
// device back; one vouched for (`by`, the vouching box's key) by a box kept from vouching
// (`vouchBlocked`) isn't paired, said with the device whose removal did that; else it's in, with a
// pairing of its own, and who vouched kept on it. Never touches a pairing link that waits
// (`pairing`, `appPairing`).
export function pairDevice(link: LinkState, device: { key: string; name: string; by?: string }, now = Date.now()): DevicePaired {
  const known = (link.phones ?? []).find(phone => phone.key === device.key);
  if (known) return { pairing: known.pairing ?? "" };
  if ((link.removed ?? []).some(each => each.key === device.key)) return { removed: true };
  const blocked = vouchBlockOf(link, device.by);
  if (blocked) return { blocked: { device: blocked.device } };
  if (!hasVoucherSpace(link, device.key)) return { full: true };
  const pairing = newPairing();
  pairPhone(link, { key: device.key, name: device.name, added: Math.floor(now / 1000), from: "pair", pairing, kind: "desktop", ...(device.by ? { by: device.by } : {}) });
  return { pairing };
}

// Phones this Mac removes (its own box, from its app): Settings › Phone, or the removals a
// machine's phones document brought; and those Kuvo Code on any computer removes on the box beside
// it. Only phones; false, changing nothing, for anything else.
export function removeAsMac(link: LinkState, entries: unknown, staging = false): boolean {
  if ((!staging && primaryFenced(link))) return false;
  // (On a machine, as Kuvo Code beside its box removes: never one of the Macs that set it up.)
  const macs = macsOf(link);
  if (!Array.isArray(entries) || !entries.length || entries.length > MOST_KEPT || !entries.every(entry => fits(entry, macs))) return false;
  for (const entry of entries) takeOff(link, entry.key, entry.pairing);
  return true;
}

// Phones `by` removes ({key, pairing}): only a phone on the list, or a Mac, removes, and only
// phones, a few at a time. False, changing nothing, otherwise.
export function removePhones(link: LinkState, by: string | undefined, entries: unknown, staging = false): boolean {
  if ((!staging && primaryFenced(link))) return false;
  const macs = macsOf(link);
  const allowed = new Set([...(link.phones ?? []).map(phone => phone.key), ...macs]);
  if (!allowed.has(by!) || !Array.isArray(entries) || !entries.length || entries.length > MOST_AT_ONCE) return false;
  if (!entries.every(entry => fits(entry, macs))) return false;
  for (const entry of entries) takeOff(link, entry.key, entry.pairing);
  return true;
}

// The phones this box lets in, from its documents (`link.phones`, which the rest reads): the ones
// each of its Macs lists in its phones document (or, for a Mac that hasn't signed one yet, as it
// listed them before), and the ones paired here, less every pairing removed in a document this box
// takes, which it notes as its own removal too, so it stays removed whatever becomes of that
// document. Each keeps when it was added here and where it was last seen.
export function foldPhones(link: LinkState, now = Date.now()): void {
  const macs = macsOf(link);
  for (const doc of phoneDocs(link)) {
    for (const entry of Array.isArray(doc.body.removed) ? doc.body.removed.slice(0, MOST_KEPT) : []) {
      if (fits(entry, macs) && typeof entry.pairing === "string") noteRemoved(link, entry.key, entry.pairing);
    }
  }
  // One paired here on a box's word that a removal in a document just took off (removed on another
  // computer): the others that box vouched for go too, and it vouches for no other, as when it's
  // removed here (`takeOff`).
  for (const phone of [...(link.phones ?? [])]) {
    if (phone.from === "pair" && link.phones!.includes(phone) && removedAlready(link, phone.key, phone.pairing ?? "")) blockVoucher(link, phone, now);
  }
  const known = new Map((link.phones ?? []).map(phone => [phone.key, phone]));
  const signed = macs.filter(mac => macDoc(link, mac, "phones"));
  const listed: Phone[] = [];
  const add = (phone: Phone) => {
    if (!fits(phone, macs) || removedAlready(link, phone.key, phone.pairing ?? "") || listed.some(each => each.key === phone.key)) return;
    listed.push(phone);
  };
  for (const mac of macs) {
    const doc = macDoc(link, mac, "phones");
    if (!doc) {
      for (const phone of link.phones ?? []) if (phone.from === "mac" && phone.mac === mac) add(phone);
      continue;
    }
    for (const phone of Array.isArray(doc.body.phones) ? doc.body.phones.slice(0, 64) : []) {
      if (!isObject(phone)) continue;
      add({
        key: phone.key as string,
        name: String(phone.name ?? "").slice(0, 100),
        added: known.get(phone.key as string)?.added ?? Math.floor(now / 1000),
        from: "mac",
        mac,
        address: typeof phone.address === "string" && ADDRESS.test(phone.address) ? phone.address : known.get(phone.key as string)?.address,
        pairing: typeof phone.pairing === "string" ? phone.pairing : "",
        ...(phone.kind === "desktop" ? { kind: "desktop" as const } : {}),
        ...(typeof phone.by === "string" && KEY.test(phone.by) ? { by: phone.by } : {}),
      });
    }
  }
  // One from before Macs were named: kept until a Mac signs its phones.
  if (!signed.length) for (const phone of link.phones ?? []) if (phone.from === "mac" && phone.mac == null) add(phone);
  for (const phone of link.phones ?? []) if (phone.from === "pair") add(phone);
  link.phones = listed;
}

// This box's own phones document: the phones paired here, and every removal it knows of.
export const ownPhones = (link: LinkState) => ({
  ...(link.setup?.managed && link.setup.role === "primary" ? { epoch: link.setup.epoch } : {}),
  phones: (link.phones ?? []).filter(phone => phone.from === "pair" && !phone.local).map(phone => ({ key: phone.key, name: phone.name ?? "", ...(phone.address ? { address: phone.address } : {}), pairing: phone.pairing ?? "", ...(phone.kind === "desktop" ? { kind: "desktop" as const } : {}), ...(phone.by ? { by: phone.by } : {}) })),
  removed: (link.removed ?? []).map(({ key, pairing }) => ({ key, pairing })),
});

// A phone's session proved its own: its notifications go where it is now.
export function confirmPhone(phone: { key: Buffer; address?: Buffer }): void {
  const where = phone.address?.toString("base64url");
  const known = readLink()?.phones?.find(each => each.key === phone.key.toString("base64url"));
  if (!known || !where || known.address === where) return;
  updateLink(link => {
    const known = link?.phones?.find(each => each.key === phone.key.toString("base64url"));
    if (!known || !where || known.address === where) return null;
    known.address = where;
    return link;
  });
}

// How long a pairing waits for its phone.
export const PAIRING_MINUTES = 10;

// A pairing's token, good once, for `PAIRING_MINUTES`.
export const newPairingToken = (issuer: Pairing["issuer"] = null): Pairing => ({ token: crypto.randomBytes(16).toString("base64url"), expires: Date.now() + PAIRING_MINUTES * 60_000, issuer });

// The link a phone pairs with, for `link`'s pairing.
export function pairingLink(link: LinkState & { relay: string; secret: string; key: string; name: string; pairing: Pairing }): string {
  const query = new URLSearchParams({
    v: "1",
    relay: link.relay,
    host: addressOf(Buffer.from(link.secret, "base64url")).toString("base64url"),
    key: publicOf(Buffer.from(link.key, "base64url")).toString("base64url"),
    token: link.pairing.token,
    name: link.name,
  });
  return `kuvo-link://pair?${query}`;
}

// The phones documents this box takes now: its Macs' and its peers'.
export const phoneDocs = (link: LinkState | null | undefined): Doc[] => (Object.values(link?.docs ?? {}) as SignedDoc[]).map(held).filter(doc => doc?.kind === "phones") as Doc[];
