// Disposable copies of your other boxes' lists, received through authenticated peer mail.

import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { writePrivateLater } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { fitsShape, list, map, number, object, type Infer } from "../../shared/shape";
import { BOT_ACTIVITY } from "../../shared/routes/bots";
import { SETUP_PROJECT, SETUP_QUEUE, SETUP_SESSION, SETUP_WAIT, START_CATALOG } from "../../shared/routes/sessions";
import { DOC_PART } from "../documents/format";
import { assemble, splitParts, type PendingParts } from "../documents/parts";
import { RUN } from "../home";
import { readLink } from "../state";
import { readPeerAgents } from "./directory";
import { KEY, mailFor, peersOf, removedBoxes, type MailSender } from "./known";
import { awaySince, readReach } from "./reach";
import { SWITCH_GRACE } from "../updates/progress";
import { forgetConversations } from "./kept-conversations";
import type { LinkHost } from "../relay/host";

export const SETUP_LIST_FEATURE = "line.setup-lists";
export const PEER_BOT_ACTIVITY = "bots.peer-activity";
export const PEER_LIST_FOLDER = path.join(RUN, "peers");

const CONTENT = {
  sessions: list(SETUP_SESSION), projects: list(SETUP_PROJECT),
  waiting: list(SETUP_WAIT), queues: list(SETUP_QUEUE), catalog: START_CATALOG, "bots?": map(BOT_ACTIVITY),
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

/** Whether a computer that's away is only switching to a new release: it last said it was ready
 * for one, or switching to it, and went quiet within the time a slow computer takes to restart
 * into it. A client shows its rows as updating then, and as last seen after. */
function switchingRelease(key: string, since: number, told: ReturnType<typeof readPeerAgents>, now: number): boolean {
  const state = told[key]?.computer?.fleetUpdate?.state;
  if (state !== "updating" && state !== "ready") return false;
  return now - since < SWITCH_GRACE;
}

/** Under this long since a computer stopped answering, it's just live: a restart or a blip greys
 * nothing, and no row says "seen now". */
export const LIVE_FOR = 60_000;

/** How a computer stands for the rows that show it, given whether it's `quiet` (the relay found it
 * not there, or nothing came from it a while) and when it was last heard from. Switching to a new
 * release isn't away: `updating`, its rows as they are. Away, with when it was last seen, only once
 * it has been quiet a minute. */
export function standingOf(key: string, quiet: boolean, seen: number | undefined, reach: Record<string, number>, told: ReturnType<typeof readPeerAgents>, now: number): { away: boolean; lastSeen?: number; updating?: true } {
  if (!quiet) return { away: false };
  // (Since its last sign of life: heard from, or answering until the relay first found it not there.)
  const since = Math.max(seen ?? 0, awaySince(key, reach) ?? 0) || now;
  if (switchingRelease(key, since, told, now)) return { away: false, updating: true };
  if (now - since < LIVE_FOR) return { away: false };
  return { away: true, ...(seen !== undefined ? { lastSeen: seen } : {}) };
}

/** Whether a computer answers, as its setup rows say it: a relay refusal is immediate, and silence
 * also ages out reach if this box missed a refusal. */
function standing(key: string, saved: Kept | null, reach: Record<string, number>, told: ReturnType<typeof readPeerAgents>, now: number) {
  const quiet = reach[key] !== undefined || (saved !== null && now - saved.seen > 3 * AGAIN);
  return standingOf(key, quiet, saved?.seen, reach, told, now);
}

/** How the computer `key` stands, as its setup rows say it (`peerSetupLists`). */
export async function peerStanding(key: string, now = Date.now()) {
  return standing(key, await readSaved(key), readReach(), readPeerAgents(), now);
}

export async function peerSetupLists(link = readLink(), now = Date.now()) {
  const reach = readReach();
  const told = readPeerAgents();
  return (await Promise.all(peersOf(link).map(async peer => {
    const saved = await readSaved(peer.key);
    if (!saved) return [];
    return [{ saved, origin: { computer: { id: peer.key, name: peer.name }, ...standing(peer.key, saved, reach, told, now) } }];
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
      await forgetConversations(key);
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
  const { bots: _, ...withoutBots } = lists;
  const variants = new Map<boolean, { digest: string; snapshot: Told; parts?: ReturnType<typeof splitParts> }>();
  for (const peer of ready) {
    if (!currentPeers.has(peer.key)) continue;
    const knowsBots = told[peer.key]?.features?.includes(PEER_BOT_ACTIVITY) ?? false;
    let variant = variants.get(knowsBots);
    if (!variant) {
      const content = knowsBots ? lists : withoutBots;
      variant = { digest: createHash("sha256").update(JSON.stringify(content)).digest("base64url"), snapshot: { at: now, ...content } };
      variants.set(knowsBots, variant);
    }
    const before = sent.get(peer.key);
    const requested = refresh.delete(peer.key);
    if (!requested && before?.digest === variant.digest && now - before.sent < AGAIN) continue;
    const single = mailFor(peer, { setupLists: variant.snapshot, at: now }, link);
    if (!single && !variant.parts) {
      const encoded = Buffer.from(JSON.stringify(variant.snapshot)).toString("base64url");
      if (Math.ceil(encoded.length / DOC_PART) > MOST_PARTS) {
        console.error("Setup lists are too large to share with peers.");
        continue;
      }
      variant.parts = splitParts(encoded);
    }
    const mails = single ? [single] : variant.parts!.map(part => mailFor(peer, { setupLists: { part }, at: now }, link));
    if (mails.every(bytes => bytes && host.send(bytes) !== false)) sent.set(peer.key, { digest: variant.digest, sent: now });
  }
}
