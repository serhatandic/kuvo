// Disposable copies of your other boxes' lists, received through authenticated peer mail.

import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { writePrivateLater } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { fitsShape, list, map, number, object, type Infer, type Shape } from "../../shared/shape";
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
export type SetupLists = Infer<typeof LISTS>;
const MOST_PARTS = 200;
const AGAIN = 60_000;
const SPACING = 15_000;

type PeerSnapshot<T> = T & { at: number; seen: number };

/** Independent streams share the same peer round, ordering, disk and multipart rules. A large
 * snapshot in one stream does not delay or replace another stream's kept copy. */
export function peerSnapshots<Value extends object>(
  name: string, file: string, feature: string, fields: Record<string, Shape>,
  variant: (value: Value, features: readonly string[]) => Value = value => value,
  spacing = SPACING,
) {
  const told = object({ at: number, ...fields });
  const kept = object({ seen: number, ...told.object });
  const fileOf = (key: string) => path.join(PEER_LIST_FOLDER, key, file);
  const cached = new Map<string, { stamp: string; value: PeerSnapshot<Value> }>();
  const followers = new Set<() => void>();
  const pending = new Map<string, PendingParts>();
  const writing = new Map<string, Promise<void>>();
  const pushed = new WeakMap<Pick<LinkHost, "send">, Map<string, { digest: string; sent: number }>>();
  const refresh = new Set<string>();
  const cleared = new Set<string>();

  async function read(key: string): Promise<PeerSnapshot<Value> | null> {
    if (!KEY.test(key)) return null;
    const file = fileOf(key);
    try {
      const stat = await fs.stat(file);
      const stamp = `${stat.ino}:${stat.mtimeMs}`;
      const held = cached.get(key);
      if (held?.stamp === stamp) return held.value;
      const saved = parsed(await fs.readFile(file));
      if (fitsShape(kept, saved)) {
        const value = saved as unknown as PeerSnapshot<Value>;
        cached.set(key, { stamp, value });
        return value;
      }
    } catch {}
    cached.delete(key);
    return null;
  }

  async function take(sender: MailSender, mail: Record<string, unknown>, seen = Date.now()): Promise<void> {
    const { key } = sender;
    if (!KEY.test(key) || !peersOf(readLink()).some(peer => peer.key === key)) return;
    let given: unknown = mail;
    if ("part" in mail) {
      const whole = assemble(pending, key, mail.part, MOST_PARTS);
      if (!whole) return;
      given = parsed(Buffer.from(whole.d, "base64url"));
    }
    if (!fitsShape(told, given)) return;
    const snapshot = given as Value & { at: number };
    const save = (writing.get(key) ?? Promise.resolve()).catch(() => {}).then(async () => {
      if (removedBoxes(readLink()).has(key)) return;
      const before = await read(key);
      if (before && snapshot.at <= before.at) return;
      await writePrivateLater(fileOf(key), JSON.stringify({ ...snapshot, seen }));
      for (const changed of followers) changed();
    });
    writing.set(key, save);
    try { await save; }
    finally { if (writing.get(key) === save) writing.delete(key); }
  }

  /** Changes go on the normal peer round. A periodic full copy repairs missed mail. */
  async function push(host: Pick<LinkHost, "send">, currentValue: () => Promise<Value | null>, now = Date.now()): Promise<void> {
    const link = readLink();
    if (!link?.key) return;
    const peers = peersOf(link);
    const agents = readPeerAgents();
    const reach = readReach();
    let sent = pushed.get(host);
    if (!sent) pushed.set(host, sent = new Map());
    const keys = new Set(peers.map(peer => peer.key));
    for (const key of sent.keys()) if (!keys.has(key)) sent.delete(key);
    for (const key of refresh) if (!keys.has(key)) refresh.delete(key);
    for (const key of removedBoxes(link)) {
      if (!KEY.test(key) || cleared.has(key)) continue;
      cached.delete(key);
      try {
        await fs.rm(fileOf(key), { force: true });
        // (With the conversations kept from it: `kept-conversations.ts`.)
        await forgetConversations(key);
        cleared.add(key);
      } catch {}
    }
    const ready = peers.filter(peer => {
      const before = sent.get(peer.key);
      return peer.address && agents[peer.key]?.features?.includes(feature) &&
        (!before || refresh.has(peer.key) || now - before.sent >= (reach[peer.key] === undefined ? spacing : AGAIN));
    });
    if (!ready.length) return;
    const value = await currentValue();
    if (!value) return;
    const current = readLink();
    if (!current || current.key !== link.key || current.setup?.id !== link.setup?.id) return;
    const currentPeers = new Set(peersOf(current).map(peer => peer.key));
    const variants = new Map<Value, { digest: string; snapshot: Value & { at: number }; parts?: ReturnType<typeof splitParts> }>();
    for (const peer of ready) {
      if (!currentPeers.has(peer.key)) continue;
      const content = variant(value, agents[peer.key]?.features ?? []);
      let prepared = variants.get(content);
      if (!prepared) {
        prepared = { digest: createHash("sha256").update(JSON.stringify(content)).digest("base64url"), snapshot: { at: now, ...content } };
        variants.set(content, prepared);
      }
      const before = sent.get(peer.key);
      const requested = refresh.delete(peer.key);
      if (!requested && before?.digest === prepared.digest && now - before.sent < AGAIN) continue;
      const single = mailFor(peer, { [name]: prepared.snapshot, at: now }, link);
      if (!single && !prepared.parts) {
        const encoded = Buffer.from(JSON.stringify(prepared.snapshot)).toString("base64url");
        if (Math.ceil(encoded.length / DOC_PART) > MOST_PARTS) {
          console.error(`${name} is too large to share with peers.`);
          continue;
        }
        prepared.parts = splitParts(encoded);
      }
      const mails = single ? [single] : prepared.parts!.map(part => mailFor(peer, { [name]: { part }, at: now }, link));
      if (mails.every(bytes => bytes && host.send(bytes) !== false)) sent.set(peer.key, { digest: prepared.digest, sent: now });
    }
  }

  return { read, take, push, followers, refresh: (key: string) => { refresh.add(key); } };
}

const setupLists = peerSnapshots<SetupLists>("setupLists", "lists-v1.json", SETUP_LIST_FEATURE, CONTENT, (value, features) => {
  if (features.includes(PEER_BOT_ACTIVITY)) return value;
  const { bots: _, ...withoutBots } = value;
  return withoutBots;
});
export const { take: takeSetupLists, push: pushSetupLists, followers: peerListFollowers, refresh: tellListsAgain } = setupLists;

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

/** How the computer `key` stands for rows drawn from a copy it sent, kept since `seen` (ms): quiet
 * once the relay found it not there, or the copy is older than `quietAfter`; then `standingOf`. */
export function copyStanding(key: string, seen: number, reach = readReach(), told = readPeerAgents(), now = Date.now(), quietAfter = 180_000) {
  return standingOf(key, reach[key] !== undefined || now - seen > quietAfter, seen, reach, told, now);
}

/** Whether a computer answers, as its setup rows say it: a relay refusal is immediate, and silence
 * also ages out reach if this box missed a refusal. */
function standing(key: string, saved: PeerSnapshot<SetupLists> | null, reach: Record<string, number>, told: ReturnType<typeof readPeerAgents>, now: number) {
  const quiet = reach[key] !== undefined || (saved !== null && now - saved.seen > 3 * AGAIN);
  return standingOf(key, quiet, saved?.seen, reach, told, now);
}

/** How the computer `key` stands, as its setup rows say it (`peerSetupLists`). */
export async function peerStanding(key: string, now = Date.now()) {
  return standing(key, await setupLists.read(key), readReach(), readPeerAgents(), now);
}

export async function peerSetupLists(link = readLink(), now = Date.now()) {
  const reach = readReach();
  const told = readPeerAgents();
  return (await Promise.all(peersOf(link).map(async peer => {
    const saved = await setupLists.read(peer.key);
    if (!saved) return [];
    return [{ saved, origin: { computer: { id: peer.key, name: peer.name }, ...standing(peer.key, saved, reach, told, now) } }];
  }))).flat();
}
