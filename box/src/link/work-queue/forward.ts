// What you do to an item another of your computers keeps, done there (PROTOCOL.md, "The Work
// Queue", "Across your computers", Done where it's kept): a client sends it to any box, and that box,
// which knows which computer keeps the item (as that one told it), hands it to that computer's box
// by mail and answers with what it said. So no client works out where an item is.
//
// The asking box leaves it in `run/work-queue/forward/<id>.json` and waits for `<id>.answer.json`
// beside it: a machine's routes from a Mac over SSH are answered by a process of their own (`box
// route`) with no relay connection, so its link sends what's left there, and leaves the answer, as
// this Mac's box does in one process. The computer that keeps it does it as its own client's, once
// for each id.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { listOf, writePrivate } from "../files";
import { watchFolders } from "../follow";
import { ownName } from "../letters/start";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { awaySince } from "../peers/reach";
import { offRelay } from "../letters/leave";
import { IS_MAC } from "../home";
import { readLink } from "../state";
import { computerName } from "../peers/computer-name";
import { mailsFor } from "./parts";
import { refNames, WORK_QUEUE, type ItemRef } from "./store";
import { toldItemOf, type ToldItem } from "./told";
import type { MailSender } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";
import type { Computer } from "./view";

/** What a box's agents' mail says when it takes what you do to an item from your other computers'
 * boxes (`kuvo/info`'s `features` too). */
export const FORWARD_FEATURE = "work-queue.forward";
/** What a computer says as it keeps the files a client attaches to an item (PROTOCOL.md). */
export const ATTACHMENTS = "work-queue.attachments";

/** Whether the computer `key` takes what you do to its items from another box (`feature`: what it
 * says it takes, for another area's: renaming and removing your machines, `computers.manage`). */
export const takesForwarded = (key: string, feature = FORWARD_FEATURE): boolean => (readPeerAgents()[key]?.features ?? []).includes(feature);

/** What a box says it takes of each other area's asks. */
const AREA_FEATURES = { computers: "computers.manage", devices: "devices.pair", setup: "setup.handover" } as const;

export const FORWARDS = path.join(WORK_QUEUE, "forward");
// What a keeper did for the asks it took, by asker and id (changes only: a read is done again).
const forwardFormat = additiveObjectFormat("Work Queue forwarded request");
const answerFormat = additiveObjectFormat("Work Queue forwarded answer");
const doneFormat = additiveObjectFormat("Work Queue forwarded action receipts");
const DONE_FILE = path.join(WORK_QUEUE, "acted.json");
const ID = /^wqa_[A-Za-z0-9_-]{16,40}$/;
const LEFT = /^(wqa_[A-Za-z0-9_-]{16,40})\.json$/;

/** How long a box waits for the computer that keeps an item to answer; a file longer, as its
 * answer comes in parts (a test's shorter). */
export const forwardTiming = { wait: 20_000, file: 60_000 };
const waitFor = (act: { area?: unknown; method?: unknown; action?: unknown }) =>
  (act.action === "file" ? forwardTiming.file : act.area === "setup" && act.method === "POST" ? 2 * forwardTiming.wait + AGAIN_AFTER : forwardTiming.wait);
/** How often what's waiting goes again while it's waited for (the keeper takes each id once). */
const AGAIN_AFTER = 5_000;
/** How soon a keeper answers an ask sent again: a file's not while it's waited for (it can be big). */
const answerAgainAfter = (act: Act) => (act.action === "file" ? 60_000 : 4_000);
/** How long a keeper remembers what it did for each id (from when it was asked, or its `at` if
 * that's later), and how long ago an ask's `at` may be; and how far ahead of this clock, a minute. */
export const ACT_MEMORY = 600_000;
const AHEAD = 60_000;
/** The most asks a keeper remembers at once (the oldest go first past it). */
const MOST_REMEMBERED = 2_000;

/** What a client did, as it goes to the box that keeps the item: a whole read (`GET`) or one of its
 * files (`action` `file`, `name`), a delete, a change (`PATCH`, with what it said, `body`), or a
 * bringing back (`action` `restore`). `area` `computers`: one of your machines (`item`, its key)
 * renamed (`PATCH {name}`) or removed (`DELETE`) on the box that writes its record
 * (`peers/manage.ts`); `area` `devices`: Kuvo Code's desktop app on the asking box's computer
 * (`item`, its key; `body`, `{name, kind}`) paired on the setup's main computer, which the asking
 * box vouches for (`POST`), or the asking box revokes its own vouched devices (`DELETE`,
 * `item` is the sender's box key, `peers/devices.ts`); a Work Queue item's otherwise. */
export interface Act {
  id: string;
  area?: "computers" | "devices" | "setup";
  method: "GET" | "DELETE" | "POST" | "PATCH";
  item: string;
  action?: "restore" | "file";
  name?: string;
  body?: Record<string, unknown>;
  at: number;
}

/** What the box that keeps it answered: as its own route would have (a file: `{type, data}`, its
 * bytes in base64). */
export interface Acted { status: number; value: unknown }

/** How it went, for the box that asked: that box's answer, or why there's none. */
export type Forwarded = Acted | { status: 409 | 503; refused: string };

/** How what's left goes at once, where this process sends it (this Mac's box, a machine's link). */
export const forwardsSoon: { send: () => void } = { send: () => {} };

const left = (id: string) => path.join(FORWARDS, `${id}.json`);
const answerOf = (id: string) => path.join(FORWARDS, `${id}.answer.json`);
const read = (format: ReturnType<typeof additiveObjectFormat>, file: string): Record<string, unknown> | null => {
  try {
    const value: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(value) ? format.read(value) : null;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
};

/** `act` done on the computer `computer` (one of your other computers, by what it told of its
 * items), and what it answered: or, when it can't be asked, why, in words a client shows (`named`,
 * the item as they name it). */
export async function forwardAct(computer: Computer, named: string, act: Omit<Act, "id" | "at">, signal?: AbortSignal | null, link: LinkState | null = readLink(), now = Date.now(), { backgroundRelay = false } = {}): Promise<Forwarded> {
  const peer = peersOf(link).find(each => each.key === computer.key);
  // (One it doesn't know any more, or that doesn't take it yet: changed there, as before.)
  if (!peer || !takesForwarded(peer.key, act.area ? AREA_FEATURES[act.area] : FORWARD_FEATURE)) return { status: 409, refused: `${named} is on ${computer.name}: change it there.` };
  if (!act.area && Array.isArray(act.body?.files) && act.body.files.length && !takesForwarded(peer.key, ATTACHMENTS)) return { status: 409, refused: `${computer.name} needs an update before it can keep attached files. Update it and try again.` };
  const here = ownName();
  if (awaySince(peer.key) !== null) return { status: 503, refused: `${computer.name} isn't answering now (it may be asleep, off or offline), so nothing was changed there. Try again once it's back.` };
  if (!backgroundRelay && offRelay()) return { status: 503, refused: `${here} isn't connected to your other computers now, so nothing was changed on ${computer.name}. Try again in a moment.` };
  const id = `wqa_${crypto.randomBytes(16).toString("base64url")}`;
  try {
    writePrivate(left(id), JSON.stringify(forwardFormat.write({ to: peer.key, act: { ...act, id, at: now } })));
  } catch {
    return { status: 503, refused: `${here} couldn't send it to ${computer.name}. Try again in a moment.` };
  }
  madeHere.add(id);
  forwardsSoon.send();
  try {
    const answer = await answered(id, peer.key, waitFor(act), signal);
    // (It may have got there before that computer stopped answering, or its answer was lost.)
    if (answer === "away") return { status: 503, refused: `${computer.name} stopped answering, so ${here} can't tell whether it was done there. Look again once it's back.` };
    if (answer === "late") return { status: 503, refused: `${computer.name} didn't answer in time, so ${here} can't tell whether it was done there. Look again in a moment.` };
    return answer;
  } finally {
    madeHere.delete(id);
    read(forwardFormat, left(id));
    read(answerFormat, answerOf(id));
    fs.rmSync(left(id), { force: true });
    fs.rmSync(answerOf(id), { force: true });
  }
}

/** What asking your other computers for an item this box knows nothing of came to: the one that
 * keeps it, with the item whole as its box answered (`told`: as this box reads any item another
 * computer tells); that none has it (`missing`: every one answered); or that some couldn't say, so
 * it may be there (`unsure`, with why in words a client shows). */
export type Sought = { computer: Computer; item: Record<string, unknown>; told: ToldItem } | { missing: true } | { unsure: Computer[]; message: string };

const listed = (names: string[]) => (names.length < 3 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`);

/** An item no computer told this box of (an old Done one, a deleted one), named as a client named
 * it (`named`, which is `ref`: its number, id or issue), asked of each of your other computers'
 * boxes at once, for a client (PROTOCOL.md, "The Work Queue", "Across your computers", Found where
 * it's kept): so a client asks one box, whichever keeps the item. A live one answers at once; a
 * deleted one only once every computer answered, as another may keep it live (an issue is one item,
 * and of two the one changed last). */
export async function seekElsewhere(named: string, ref: ItemRef, signal?: AbortSignal | null, link: LinkState | null = readLink()): Promise<Sought> {
  const peers = peersOf(link);
  // (A name the box that keeps it wouldn't take, or an asker that's gone: no one is asked.)
  if (!peers.length || named.length > 300 || signal?.aborted) return { missing: true };
  const here = ownName();
  const computerOf = (peer: (typeof peers)[number]): Computer => ({ key: peer.key, name: computerName(peer), kind: peer.kind, ...(peer.id ? { id: peer.id } : {}) });
  if (offRelay()) return { unsure: peers.map(computerOf), message: `It isn't on ${here}, and ${here} isn't connected to your other computers now, so it can't tell whether it's on one of them. Try again in a moment.` };
  type Found = { computer: Computer; item: Record<string, unknown>; told: ToldItem };
  // (A live one answers for all: the others aren't waited for.)
  const rest = new AbortController();
  const gone = () => rest.abort();
  signal?.addEventListener("abort", gone, { once: true });
  // Why each that couldn't say couldn't: it isn't answering, it's a box from before this, or it
  // refused (in its own words).
  const away: Computer[] = [];
  const older: Computer[] = [];
  const refused: { computer: Computer; said: string }[] = [];
  let deleted: Found | null = null;
  try {
    const found = await new Promise<Found | null>(resolve => {
      let left = peers.length;
      const done = () => { if (--left === 0) resolve(deleted); };
      for (const peer of peers) {
        const computer = computerOf(peer);
        // (One that doesn't take it from another box, an older one: it can't be asked.)
        if (!takesForwarded(peer.key)) {
          older.push(computer);
          done();
          continue;
        }
        forwardAct(computer, "", { method: "GET", item: named }, rest.signal, link).then(there => {
          if ("refused" in there) away.push(computer);
          else if (there.status === 200) {
            const item = isObject(there.value) && isObject(there.value.item) ? there.value.item : null;
            const told = item ? toldItemOf(item) : null;
            // (One this box can't read, or that isn't the one asked for, is no answer.)
            if (!item || !told || !refNames(ref, told)) refused.push({ computer, said: "" });
            else if (!isObject(item.removed)) return resolve({ computer, item, told });
            else if (!deleted || told.updated.at > deleted.told.updated.at) deleted = { computer, item, told };
          } else if (there.status !== 404) {
            const said = isObject(there.value) && typeof there.value.message === "string" ? there.value.message.slice(0, 300) : "";
            refused.push({ computer, said });
          }
          done();
        }, () => {
          refused.push({ computer, said: "" });
          done();
        });
      }
    });
    // (A deleted one stands only when no computer that may keep it live couldn't say.)
    if (found && (!isObject(found.item.removed) || (!away.length && !older.length && !refused.length))) return found;
  } finally {
    rest.abort();
    signal?.removeEventListener("abort", gone);
  }
  if (!away.length && !older.length && !refused.length) return { missing: true };
  const names = (computers: Computer[]) => listed(computers.map(each => each.name));
  const why = [
    ...(away.length ? [`${names(away)} ${away.length === 1 ? "isn't" : "aren't"} answering now (${away.length === 1 ? "it" : "they"} may be asleep, off or offline)`] : []),
    ...(older.length ? [`${names(older)} ${older.length === 1 ? "needs" : "need"} a newer Kuvo Code to be asked from ${here}`] : []),
    ...refused.map(each => `${each.computer.name} couldn't say${each.said ? ` ("${each.said}")` : ""}`),
  ].join(", and ");
  // (Trying again helps only for one that isn't answering.)
  const again = away.length ? ` Try again once ${away.length === 1 ? `${away[0]!.name} answers` : "they answer"}.` : "";
  return { unsure: [...away, ...older, ...refused.map(each => each.computer)], message: `It isn't on ${here}, and ${why}, so it may be there.${again}` };
}

// The asks this process left. This Mac's box answers its routes in the process that sends, so it
// sends only these: what anything else on this Mac leaves there never goes out as you (on a machine,
// a Mac's `box route` leaves them, and anything there could read the box's keys anyway).
const madeHere = new Set<string>();

// The answer to `id` once it's left beside it; "away" once the relay finds that computer not there
// (after it was sent), "late" past the wait or once its asker went.
function answered(id: string, key: string, wait: number, signal?: AbortSignal | null): Promise<Acted | "away" | "late"> {
  return new Promise((resolve, reject) => {
    const until = Date.now() + wait;
    let done = false;
    const finish = (value: Acted | "away" | "late" | Error) => {
      if (done) return;
      done = true;
      watch.close();
      clearInterval(timer);
      signal?.removeEventListener("abort", gone);
      if (value instanceof Error) reject(value);
      else resolve(value);
    };
    const check = () => {
      try {
        const answer = read(answerFormat, answerOf(id));
        if (answer && typeof answer.status === "number") return finish({ status: answer.status, value: answer.value });
        if (awaySince(key) !== null) return finish("away");
        if (Date.now() >= until || signal?.aborted) finish("late");
      } catch (error) {
        finish(error instanceof Error ? error : new Error(String(error)));
      }
    };
    const gone = () => finish("late");
    // (The system's watch, and a look every half second for what one missed.)
    const watch = watchFolders([FORWARDS], check, { recursive: false });
    const timer = setInterval(check, 500);
    signal?.addEventListener("abort", gone, { once: true });
    check();
  });
}

// When each one left was last sent, in this process.
const sent = new Map<string, number>();

/** What's left to forward, sent by this process's relay connection (each round, and as it's left):
 * each again every few seconds while it's waited for. What's past its wait is cleared. `ownOnly`
 * (this Mac's box): only what this process left. */
export function sendForwards(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null = readLink(), now = Date.now(), { ownOnly = IS_MAC }: { ownOnly?: boolean } = {}): void {
  const names = listOf(FORWARDS);
  const here = new Set<string>();
  for (const name of names) {
    const id = LEFT.exec(name)?.[1];
    if (!id) continue;
    here.add(id);
    const kept = read(forwardFormat, path.join(FORWARDS, name));
    const act = isObject(kept?.act) ? kept.act : null;
    // (Its asker is gone: a process that stopped mid-wait leaves it.)
    if (!act || typeof act.at !== "number" || now - act.at > waitFor(act) + AGAIN_AFTER) {
      read(answerFormat, answerOf(id));
      fs.rmSync(path.join(FORWARDS, name), { force: true });
      fs.rmSync(answerOf(id), { force: true });
      here.delete(id);
      continue;
    }
    if ((ownOnly && !madeHere.has(id)) || !host?.welcomed || !link || fs.existsSync(answerOf(id)) || now - (sent.get(id) ?? -Infinity) < AGAIN_AFTER) continue;
    const peer = peersOf(link).find(each => each.key === kept!.to);
    const mails = peer ? mailsFor(peer, { act }, link, now) : null;
    if (!mails) continue;
    try {
      for (const bytes of mails) host.send(bytes);
      sent.set(id, now);
    } catch {}
  }
  for (const id of sent.keys()) if (!here.has(id)) sent.delete(id);
  // (An answer whose ask is gone.)
  for (const name of names) {
    const id = /^(wqa_[A-Za-z0-9_-]{16,40})\.answer\.json$/.exec(name)?.[1];
    if (id && !here.has(id)) {
      read(answerFormat, answerOf(id));
      fs.rmSync(answerOf(id), { force: true });
    }
  }
}

/** The answer from the box an ask went to, left for whoever waits for it; only from that box. */
export function takeActed(sender: MailSender, given: unknown): void {
  if (!isObject(given) || typeof given.id !== "string" || !ID.test(given.id)) return;
  const status = given.status;
  if (typeof status !== "number" || !Number.isInteger(status) || status < 200 || status > 599) return;
  const kept = read(forwardFormat, left(given.id));
  if (kept?.to !== sender.key) return;
  const file = answerOf(given.id);
  read(answerFormat, file);
  try { writePrivate(file, JSON.stringify(answerFormat.write({ status, value: given.value ?? null }))); } catch {}
}

// What this box did for each ask, by asker and id, and when it last answered it: so one sent again
// is answered the same (not again at once) and done once. A change's is kept on
// disk too, so it holds across a restart; one that was under way as the box stopped is answered as
// one that may have been done.
interface Done { until: number; answer: Acted | null; answered?: number }
const did = new Map<string, Done>();

// Those of `asks` past their time gone (in place), and past MOST_REMEMBERED, reads answered first
// (one asked again is read again), then the oldest.
function prune<T extends { until: number; answer: Acted | null; answered?: number }>(asks: Map<string, T>, now: number): void {
  for (const [key, each] of asks) if (each.until < now) asks.delete(key);
  for (const [key, each] of asks) {
    if (asks.size <= MOST_REMEMBERED) return;
    if (!each.answer && each.answered !== undefined) asks.delete(key);
  }
  for (const key of asks.keys()) {
    if (asks.size <= MOST_REMEMBERED) return;
    asks.delete(key);
  }
}

function readDone(now: number): Map<string, { until: number; answer: Acted | null }> {
  const kept = read(doneFormat, DONE_FILE) ?? {};
  const fresh = new Map<string, { until: number; answer: Acted | null }>();
  for (const [key, each] of Object.entries(kept)) {
    if (isObject(each) && typeof each.until === "number") fresh.set(key, { until: each.until, answer: isObject(each.answer) ? each.answer as unknown as Acted : null });
  }
  prune(fresh, now);
  return fresh;
}

function keepDone(key: string, done: Done, now: number): void {
  try {
    const kept = readDone(now);
    kept.delete(key);
    kept.set(key, { until: done.until, answer: done.answer });
    prune(kept, now);
    writePrivate(DONE_FILE, JSON.stringify(doneFormat.write(Object.fromEntries(kept))));
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
}

/** An ask from another of your computers' boxes: done here as this box's own client's (`run`), and
 * answered; one asked again is answered as it was, never done twice. */
export async function takeAct(sender: MailSender, given: unknown, reply: (payload: Record<string, unknown>) => void, run: (act: Act) => Promise<Acted>, now = Date.now()): Promise<void> {
  const act = actOf(given);
  if (!act) return;
  const link = readLink();
  // (Only from one of your computers that's still a peer.)
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!peer) return;
  const here = ownName();
  const answer = (value: Acted) => reply({ acted: { id: act.id, ...value } });
  prune(did, now);
  const key = `${sender.key} ${act.id}`;
  const changes = act.method !== "GET";
  const mine = did.get(key);
  // (Under way now, or answered a moment ago; a change as it went, a read done again.)
  if (mine && (mine.answered === undefined || now - mine.answered < answerAgainAfter(act))) return;
  if (mine?.answer) {
    mine.answered = now;
    return answer(mine.answer);
  }
  // A change this box took before it last started: as it went, or, cut short then, that it may
  // have been done.
  const kept = changes ? readDone(now).get(key) : undefined;
  // (Renaming or removing one of your machines, or pairing your app, is the same done twice: one
  // cut short is done again.)
  const redo = kept !== undefined && kept.answer === null && act.area !== undefined && act.area !== "setup";
  if (kept && !redo) {
    const value = kept.answer ?? { status: 409, value: { message: `${here} stopped while it was doing this, so it can't tell whether it was done. Look again in a moment.` } };
    did.set(key, { until: kept.until, answer: value, answered: now });
    return answer(value);
  }
  // (One from long ago, sent again, or from a computer whose clock is far off: not done. Each is
  // remembered until it would be too old to take, so none is ever taken twice.)
  if (!redo && (now - act.at > ACT_MEMORY || act.at - now > AHEAD)) return answer({ status: 409, value: { message: `${here}'s clock and ${computerName(peer)}'s are too far apart to take it: check both computers' time.` } });
  const entry: Done = { until: kept?.until ?? Math.max(now, act.at) + ACT_MEMORY, answer: null };
  did.set(key, entry);
  if (changes) keepDone(key, entry, now);
  try {
    entry.answer = await run(act);
  } catch {
    entry.answer = { status: 500, value: { message: `Something went wrong on ${here}. Try again in a moment.` } };
  }
  if (changes) keepDone(key, entry, now);
  entry.answered = Date.now();
  answer(entry.answer);
  // (A read's answer isn't kept: a file's can be big, and one asked again is read again.)
  if (!changes) entry.answer = null;
}

/** All this box did for others forgotten, in this process (a test's: what's on disk stays, as for a
 * box that started again). */
export function forgetActs(): void {
  did.clear();
  sent.clear();
}

// (Nothing else is an act: letting a computer add Kuvo Code's app to your devices again,
// `kuvo/computers/<key>/allow-devices`, is never one, so the box kept from it can't ask for itself.)
const actOf = (given: unknown): Act | null => {
  if (!isObject(given) || typeof given.id !== "string" || !ID.test(given.id) || typeof given.item !== "string" || !given.item || given.item.length > 300) return null;
  if (typeof given.at !== "number" || !Number.isFinite(given.at)) return null;
  const { method, action, name } = given;
  const fits = (method === "GET" && action === undefined) || (method === "DELETE" && action === undefined) || (method === "POST" && action === "restore") || (method === "PATCH" && action === undefined)
    || (method === "GET" && action === "file" && typeof name === "string" && name.length > 0 && name.length <= 300);
  const devices = given.area === "devices" && (method === "POST" || method === "DELETE") && action === undefined;
  const setup = given.area === "setup" && (method === "GET" || method === "POST") && action === undefined;
  if (!fits && !devices && !setup) return null;
  if (given.area !== undefined && !devices && !setup && !(given.area === "computers" && (method === "PATCH" || method === "DELETE") && action === undefined)) return null;
  return { id: given.id, ...(given.area === "computers" || devices || setup ? { area: given.area } : {}), method, item: given.item, ...(action ? { action } : {}), ...(action === "file" ? { name } : {}), ...(isObject(given.body) ? { body: given.body } : {}), at: given.at } as Act;
};
