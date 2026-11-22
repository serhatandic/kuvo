// Disposable copies of your other boxes' lists, received through authenticated peer mail.

import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { writePrivateLater } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { fitsShape, list, number, object, type Infer } from "../../shared/shape";
import { SETUP_PROJECT, SETUP_QUEUE, SETUP_SESSION, SETUP_WAIT, START_CATALOG } from "../../shared/routes/sessions";
import { DOC_PART } from "../documents/format";
import { assemble, splitParts, type PendingParts } from "../documents/parts";
import { RUN } from "../home";
import { readLink } from "../state";
import { readPeerAgents } from "./directory";
import { KEY, mailFor, peersOf, removedBoxes, type MailSender } from "./known";
import { readReach } from "./reach";
import type { LinkHost } from "../relay/host";

export const SETUP_LIST_FEATURE = "line.setup-lists";
export const PEER_LIST_FOLDER = path.join(RUN, "peers");

const CONTENT = {
  sessions: list(SETUP_SESSION), projects: list(SETUP_PROJECT),
  waiting: list(SETUP_WAIT), queues: list(SETUP_QUEUE), catalog: START_CATALOG,
};
const LISTS = object(CONTENT);
const TOLD = object({ at: number, ...CONTENT });
const KEPT = object({ seen: number, ...TOLD.object });
export type SetupLists = Infer<typeof LISTS>;
type Told = Infer<typeof TOLD>;
type Kept = Infer<typeof KEPT>;

const fileOf = (key: string) => path.join(PEER_LIST_FOLDER, key, "lists-v1.json");
const cached = new Map<string, { stamp: string; value: Kept }>();
async function readSaved(key: string): Promise<Kept | null> {
  const file = fileOf(key);
  try {
    const stat = await fs.stat(file);
    const stamp = `${stat.ino}:${stat.mtimeMs}`;
    const held = cached.get(key);
    if (held?.stamp === stamp) return held.value;
    const saved = parsed(await fs.readFile(file));
    if (fitsShape(KEPT, saved)) {
      const value = saved as Kept;
      cached.set(key, { stamp, value });
      return value;
    }
  } catch {}
  cached.delete(key);
  return null;
}

export const peerListFollowers = new Set<() => void>();
const pending = new Map<string, PendingParts>();
const writing = new Map<string, Promise<void>>();
const MOST_PARTS = 200;
const AGAIN = 60_000;
const SPACING = 15_000;

export async function takeSetupLists(sender: MailSender, mail: Record<string, unknown>, seen = Date.now()): Promise<void> {
  const { key } = sender;
  if (!KEY.test(key) || !peersOf(readLink()).some(peer => peer.key === key)) return;
  let told: unknown = mail;
  if ("part" in mail) {
    const whole = assemble(pending, key, mail.part, MOST_PARTS);
    if (!whole) return;
    told = parsed(Buffer.from(whole.d, "base64url"));
  }
  if (!fitsShape(TOLD, told)) return;
  const snapshot = told as Told;
  // Wait for a write already in progress, so an older snapshot can never replace a newer one.
  const save = (writing.get(key) ?? Promise.resolve()).catch(() => {}).then(async () => {
    if (removedBoxes(readLink()).has(key)) return;
    const before = await readSaved(key);
    if (before && snapshot.at <= before.at) return;
    await writePrivateLater(fileOf(key), JSON.stringify({ ...snapshot, seen }));
    for (const changed of peerListFollowers) changed();
  });
  writing.set(key, save);
  try { await save; }
  finally { if (writing.get(key) === save) writing.delete(key); }
}

export async function peerSetupLists(link = readLink(), now = Date.now()) {
  const reach = readReach();
  return (await Promise.all(peersOf(link).map(async peer => {
    const saved = await readSaved(peer.key);
    if (!saved) return [];
    // A relay refusal is immediate. Silence also ages out reach if this box missed a refusal.
    const away = reach[peer.key] !== undefined || now - saved.seen > 3 * AGAIN;
    return [{ saved, origin: { computer: { id: peer.key, name: peer.name }, away, ...(away ? { lastSeen: saved.seen } : {}) } }];
  }))).flat();
}

const pushed = new WeakMap<Pick<LinkHost, "send">, Map<string, { digest: string; sent: number }>>();
const refresh = new Set<string>();
const cleared = new Set<string>();
export const tellListsAgain = (key: string) => { refresh.add(key); };

/** Share changes on the box's normal peer round. A periodic full copy repairs missed mail and
 * probes for an away peer. Older boxes are left to their existing per-computer routes. */
export async function pushSetupLists(host: Pick<LinkHost, "send">, read: () => Promise<SetupLists | null>, now = Date.now()): Promise<void> {
  const link = readLink();
  if (!link?.key) return;
  const peers = peersOf(link);
  const removed = removedBoxes(link);
  const told = readPeerAgents();
  const reach = readReach();
  let sent = pushed.get(host);
  if (!sent) pushed.set(host, sent = new Map());
  for (const key of sent.keys()) if (!peers.some(peer => peer.key === key)) sent.delete(key);
  for (const key of removed) {
    if (!KEY.test(key) || cleared.has(key)) continue;
    cached.delete(key);
    try {
      await fs.rm(fileOf(key), { force: true });
      cleared.add(key);
    } catch {}
  }
  const ready = peers.filter(peer => {
    const before = sent.get(peer.key);
    return peer.address && told[peer.key]?.features?.includes(SETUP_LIST_FEATURE) &&
      (!before || refresh.has(peer.key) || now - before.sent >= (reach[peer.key] === undefined ? SPACING : AGAIN));
  });
  for (const key of refresh) if (!peers.some(peer => peer.key === key)) refresh.delete(key);
  if (!ready.length) return;
  const lists = await read();
  if (!lists) return;
  const current = readLink();
  if (!current || current.key !== link.key || current.setup?.id !== link.setup?.id) return;
  const currentPeers = new Set(peersOf(current).map(peer => peer.key));
  const digest = createHash("sha256").update(JSON.stringify(lists)).digest("base64url");
  const snapshot: Told = { at: now, ...lists };
  let parts: ReturnType<typeof splitParts> | undefined;
  for (const peer of ready) {
    if (!currentPeers.has(peer.key)) continue;
    const before = sent.get(peer.key);
    const requested = refresh.delete(peer.key);
    if (!requested && before?.digest === digest && now - before.sent < AGAIN) continue;
    const single = mailFor(peer, { setupLists: snapshot, at: now }, link);
    const encoded = single || parts ? null : Buffer.from(JSON.stringify(snapshot)).toString("base64url");
    if (encoded && Math.ceil(encoded.length / DOC_PART) > MOST_PARTS) {
      console.error("Setup lists are too large to share with peers.");
      return;
    }
    const mails = single ? [single] : (parts ??= splitParts(encoded!)).map(part => mailFor(peer, { setupLists: { part }, at: now }, link));
    if (mails.every(bytes => bytes && host.send(bytes) !== false)) sent.set(peer.key, { digest, sent: now });
  }
}
