// Each computer spends its local number blocks, including while offline, and
// asks the main for more before running out. The relay records the limit the
// main can allocate through. An item can wait for a number and keeps its stable ID.
//
// An import keeps its items' numbers by claiming them: the primary grants a claimed number only at
// or above its counter (below it, it may have handed it out already), and moves its counter past.
// Kept in `run/work-queue/numbers.json`, each box's own.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { IS_MAC, RUN } from "../home";
import { objectFormat } from "../../shared/format-version";
import { liveMoves } from "../moves/records";
import { mayActivate } from "../ownership/resolve";
import { locked, writeDurablePrivate } from "../files";
import { KEY, macsOf, mailFor, ownKey, peersOf, removedBoxes } from "../peers/known";
import { readTold, toldKeys } from "./told";
import { awaySince, lastFoundAway, PEER_CHECKED, readReach } from "../peers/reach";
import { followedSetup } from "../setup/record";
import { primaryFenced, relayLease } from "../setup/lease";
import { leadershipOf } from "../setup/leadership";
import { readLink, updateLink } from "../state";
import { allItems, changeQueues, noteQueuesChanged, OWNER, QUEUES, WORK_QUEUE } from "./store";
import { numberDisk, numberUpkeepDisk } from "../server/disk-worker";
import type { Book, Item } from "./store";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";
import type { MailSender, Peer } from "../peers/known";

export const NUMBERS_FILE = path.join(WORK_QUEUE, "numbers.json");

/** How many numbers a block holds, and how few left start an ask for the next. */
export const BLOCK = 50;
export const LOW = 15;
/** How long an ask waits for its answer before it's sent again. */
export const ASK_AGAIN = 30_000;
const MOST_GIVEN = 500;
const MOST_RANGES = 400;
const ASK_ID = /^wqn_[A-Za-z0-9_-]{8,40}$/;

/** Numbers as runs, `[from, to]` each, sorted and joined. */
export type Ranges = [number, number][];

/** What a box keeps of its numbers: its primary (once it's used: "self" or the Mac's key); on the
 * primary, its counter (`next`), the blocks and claims it gave by ask, and the numbers it granted
 * each box (`granted`, by box: "self" for its own); elsewhere, its leased blocks and the ask it
 * waits on, with the numbers it claims. */
export interface Numbers {
  primary?: string;
  /** When this box was set up, as it ranks for numbering (`ownSince`). */
  since?: number;
  /** Since when the primary it keeps hasn't been heard of (`notePrimaryNow`). */
  lost?: number;
  /** The peer it reaches its primary through, when that isn't one of its own. */
  through?: string;
  next?: number;
  leadership?: string;
  allocation?: number;
  reserveThrough?: number;
  /** By ask: the block given, the claims granted with it, and the box it went to (`by`; missing in
   * what an older primary kept). */
  given?: Record<string, { from: number; to: number; granted: Ranges; at: number; by?: string }>;
  granted?: Record<string, Ranges>;
  leases: Ranges;
  /** Elsewhere, the numbers of its leases it gave items here: an import here never claims one that
   * no item here has now (it left with a move, or was deleted). */
  used?: Ranges;
  /** The ask under way: how many, the numbers claimed, and of those, the ones asked back for items
   * an import renumbered (`restores`). */
  asking?: { id: string; count: number; claims?: Ranges; restores?: Ranges; since: number; sent?: number };
}

/** Who hands out this box's numbers: itself, another computer (reached through a peer, `via`, when
 * it isn't one of this box's own), or one it can't write to yet. */
export type Primary = { self: true } | { peer: Peer; via?: Peer } | { away: true };

// (A number has six digits at most: what a chip reads (`#243`), and far past what's ever handed out.)
export const MOST_NUMBER = 1_000_000;
/** How far past the counter a claim is granted: an import's numbers, not one stray far number that
 * would move the counter past every number there is. Beyond it, an item is numbered afresh. */
export const CLAIM_REACH = 10_000;
const isRange = (each: unknown): each is [number, number] => Array.isArray(each) && each.length === 2 && each.every(n => Number.isInteger(n) && n > 0 && n < MOST_NUMBER) && each[0] <= each[1];

/** Runs sorted, and those that touch or overlap joined. */
const joined = (...all: Ranges[]): Ranges => {
  const sorted = all.flat().filter(isRange).map(([from, to]): [number, number] => [from, to]).sort((a, b) => a[0] - b[0]);
  const ranges: Ranges = [];
  for (const [from, to] of sorted) {
    const last = ranges[ranges.length - 1];
    if (last && from <= last[1] + 1) last[1] = Math.max(last[1], to);
    else ranges.push([from, to]);
  }
  return ranges;
};
/** `numbers` as runs. */
export const rangesOf = (numbers: Iterable<number>): Ranges => joined([...numbers].filter(n => Number.isInteger(n) && n > 0).map((n): [number, number] => [n, n]));
/** What of `ranges` is also in `within`. */
const overlap = (ranges: Ranges, within: Ranges): Ranges => joined(ranges.flatMap(([from, to]) => within.flatMap(([a, b]): Ranges => (Math.max(from, a) <= Math.min(to, b) ? [[Math.max(from, a), Math.min(to, b)]] : []))));
export const inRanges = (ranges: Ranges, n: number) => ranges.some(([from, to]) => n >= from && n <= to);
const cleanRanges = (value: unknown): Ranges => (Array.isArray(value) ? value.filter(isRange).slice(-MOST_RANGES) : []);

const numbersFormat = objectFormat("Work Queue number ledger", 1);

function openNumbers(file: string): { fields: Record<string, unknown>; upgraded: boolean } | null {
  let text: string;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw error;
  }
  const raw = JSON.parse(text);
  const fields = numbersFormat.read(raw);
  return { fields, upgraded: raw.formatVersion !== numbersFormat.version };
}

// Read the last pre-lease allocation once as a floor when an existing setup upgrades.
export function legacyNumberHigh(setup: string): number {
  const quorum = openNumbers(NUMBERS_FILE)?.fields.quorum;
  const votes = isObject(quorum) && isObject(quorum.votes) ? quorum.votes[setup] : undefined;
  const observed = isObject(quorum) && isObject(quorum.observed) ? quorum.observed[setup] : undefined;
  return Math.max(isObject(votes) && typeof votes.to === "number" ? votes.to : 0, typeof observed === "number" ? observed : 0);
}

export function readNumbers(file = NUMBERS_FILE): Numbers {
  return numbersOf(openNumbers(file)?.fields ?? {});
}

function numbersOf(kept: Record<string, unknown>): Numbers {
  const asking = isObject(kept.asking) && typeof kept.asking.id === "string" && typeof kept.asking.count === "number" && typeof kept.asking.since === "number"
    ? { id: kept.asking.id, count: kept.asking.count, since: kept.asking.since, ...(typeof kept.asking.sent === "number" ? { sent: kept.asking.sent } : {}), ...(Array.isArray(kept.asking.claims) ? { claims: cleanRanges(kept.asking.claims) } : {}), ...(Array.isArray(kept.asking.restores) ? { restores: cleanRanges(kept.asking.restores) } : {}) }
    : undefined;
  return {
    ...(typeof kept.primary === "string" ? { primary: kept.primary } : {}),
    ...(typeof kept.since === "number" && Number.isFinite(kept.since) && kept.since > 0 ? { since: kept.since } : {}),
    ...(typeof kept.lost === "number" && Number.isFinite(kept.lost) ? { lost: kept.lost } : {}),
    ...(typeof kept.through === "string" ? { through: kept.through } : {}),
    ...(typeof kept.next === "number" && Number.isInteger(kept.next) && kept.next > 0 ? { next: kept.next } : {}),
    ...(typeof kept.leadership === "string" ? { leadership: kept.leadership } : {}),
    ...(typeof kept.allocation === "number" ? { allocation: kept.allocation } : {}),
    ...(typeof kept.reserveThrough === "number" ? { reserveThrough: kept.reserveThrough } : {}),
    ...(isObject(kept.given) ? { given: kept.given as Numbers["given"] } : {}),
    ...(isObject(kept.granted) ? { granted: Object.fromEntries(Object.entries(kept.granted).map(([box, ranges]) => [box, cleanRanges(ranges)])) } : {}),
    leases: cleanRanges(kept.leases),
    ...(Array.isArray(kept.used) ? { used: cleanRanges(kept.used) } : {}),
    ...(asking ? { asking } : {}),
  };
}

/** This box's primary: its setup's main computer, itself when it's that (PROTOCOL.md, "Your setup");
 * one it can't write to yet (its document isn't here, or names no address) is waited for, never
 * passed over. An independent computer keeps its own counter. */
export function primaryOf(link: LinkState | null | undefined = readLink()): Primary {
  const setup = followedSetup(link);
  if (!setup) return link?.setup || peersOf(link).length || macsOf(link).length ? { away: true } : { self: true };
  if (setup.role === "primary") return { self: true };
  const peer = peersOf(link).find(each => each.key === setup.primary);
  return peer?.address ? { peer } : { away: true };
}

/** For a box in no setup, kept until every box knows setups and shared groups are joined (#183).
 * The first computer you set up, as far as this box can tell (PROTOCOL.md,
 * "The Work Queue", "Numbers"): of this box, its peers and the primaries they say they number from,
 * a Mac before a machine (a machine is always set up from a Mac), then the one set up first, then by
 * key. The one it numbers from now is kept while it's still one of your computers and none set up
 * earlier is known; a box that hears of an earlier one moves to it (never back), and one whose
 * primary was removed or forgotten finds the next. A machine with a Mac is never its own. */
export function rankedPrimary(link: LinkState | null | undefined = readLink(), file = NUMBERS_FILE, now = Date.now()): Primary {
  const kept = readNumbers(file);
  const own = link ? ownKey(link) : null;
  const gone = goneOf(link, now);
  const all = candidatesOf(link, kept, own, file, gone);
  const macs = IS_MAC ? [] : macsOf(link);
  // (A machine set up from a Mac never numbers on its own.)
  const choices = macs.length ? all.filter(each => !each.self) : all;
  const best = choices.sort(rank)[0];
  const keptKey = kept.primary === "self" || (own && kept.primary === own) ? "self" : kept.primary;
  let current: Candidate | null | "away" = null;
  if (keptKey === "self") current = all.find(each => each.self) ?? null;
  else if (keptKey && !gone.has(keptKey)) {
    current = all.find(each => each.key === keptKey) ?? null;
    // Not heard of now (one of its Macs whose peers document isn't here yet, one a peer stopped
    // naming for a while, no link yet): it waits, so a box never goes back and forth between two;
    // only one you removed, or one not heard of for days (gone), makes way for the next; and one
    // the peer it was reached through now says it numbers from another instead of (it found it
    // gone, or one set up earlier).
    const removed = removedBoxes(link);
    const through = kept.through ? readTold(kept.through)?.primary : undefined;
    const movedOn = Boolean(kept.through && through && through.key !== keptKey && peersOf(link).some(peer => peer.key === kept.through) && !gone.has(kept.through));
    if (!current && !movedOn && !removed.has(keptKey) && !(kept.through && removed.has(kept.through)) && !(kept.lost !== undefined && now - kept.lost > LOST_FOR)) current = "away";
  }
  if (current === "away") return { away: true };
  // A machine kept itself (before it was set up from a Mac): its Mac now.
  if (current?.self && macs.length) current = null;
  const chosen = current && (!best || !earlier(best, current)) ? current : best;
  if (!chosen) return (keptKey && keptKey !== "self") || macs.length ? { away: true } : { self: true };
  if (chosen.self) return { self: true };
  if (chosen.peer?.address) return { peer: chosen.peer };
  if (chosen.via?.address) return { peer: { key: chosen.key, address: "", kind: chosen.kind, name: chosen.name ?? "" }, via: chosen.via };
  return { away: true };
}

/** How long a box's primary may go unheard of (no peer names it), or not answer the box that reaches
 * it, before it's taken to be gone. */
export const LOST_FOR = 3 * 86_400_000;

/** The computers taken to be gone, for numbering: a peer that hasn't answered this box for days,
 * and still didn't lately (a Mac put away, or set up again with new keys, that its machines still
 * list a while; not one that was away when this box itself went off the relay). A box that reaches
 * that one through this one hears of it no more once this box numbers from another. Heard from
 * again, it's back, and the earliest again: a box moves back to it, and it moves past what was
 * numbered meanwhile (`top`). */
export function goneOf(link: LinkState | null | undefined, now = Date.now()): Set<string> {
  const gone = new Set<string>();
  const reach = readReach();
  const checked = readReach(PEER_CHECKED);
  for (const peer of peersOf(link)) {
    const away = awaySince(peer.key, reach);
    const last = lastFoundAway(peer.key, checked);
    if (away !== null && now - away > LOST_FOR && last !== null && now - last < FOUND_AWAY_WITHIN) gone.add(peer.key);
  }
  return gone;
}

/** How lately a peer has to have been found away for it to count as gone. */
const FOUND_AWAY_WITHIN = 3_600_000;

/** A computer that may be the primary: this box, a peer, or a primary a peer named (reached
 * through that peer, `via`). */
interface Candidate {
  key: string;
  kind: "mac" | "machine";
  since: number;
  self?: true;
  peer?: Peer;
  via?: Peer;
  name?: string;
}

// Whether `a` was set up before `b`, as far as either has said: a Mac before a machine, then the one
// that says it was set up first (one that hasn't said is never earlier).
// (Never over one that hasn't said yet, an older box in the middle of an update: it may well be
// the first.)
const earlier = (a: Candidate, b: Candidate): boolean => (a.kind !== b.kind ? a.kind === "mac" : b.since !== Number.MAX_SAFE_INTEGER && a.since < b.since);
// A Mac first, then the one set up first (one that hasn't said is last), then by key.
const rank = (a: Candidate, b: Candidate): number => (a.kind === b.kind ? 0 : a.kind === "mac" ? -1 : 1) || a.since - b.since || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0);

function candidatesOf(link: LinkState | null | undefined, kept: Numbers, own: string | null, file: string, gone: Set<string> = goneOf(link, Date.now())): Candidate[] {
  const all: Candidate[] = [{ key: own ?? "self", kind: IS_MAC ? "mac" : "machine", since: kept.since ?? ownSince(Date.now(), file), self: true }];
  const peers = peersOf(link).filter(peer => !gone.has(peer.key));
  for (const peer of peers) all.push({ key: peer.key, kind: peer.kind, since: readTold(peer.key)?.since ?? Number.MAX_SAFE_INTEGER, peer, name: peer.name });
  for (const peer of peers) {
    const named = readTold(peer.key)?.primary;
    if (!named || named.key === own || gone.has(named.key)) continue;
    const known = all.find(each => each.key === named.key);
    if (known) {
      if (known.since === Number.MAX_SAFE_INTEGER && named.since) known.since = named.since;
      continue;
    }
    all.push({ key: named.key, kind: named.kind, since: named.since ?? Number.MAX_SAFE_INTEGER, via: peer });
  }
  return all;
}

/** When this box was set up, as it ranks among your computers for numbering: kept once worked out
 * (the earliest of its folder's making, where the system keeps that, and its oldest item). */
export function ownSince(now = Date.now(), file = NUMBERS_FILE): number {
  const kept = readNumbers(file).since;
  if (kept) return kept;
  let since = now;
  try {
    const made = fs.statSync(RUN).birthtimeMs;
    if (made > 0 && made < since) since = made;
  } catch {}
  for (const item of allItems()) if (item.created.at > 0 && item.created.at < since) since = item.created.at;
  since = Math.floor(since);
  changeNumbers(numbers => { numbers.since ??= since; }, file);
  return readNumbers(file).since ?? since;
}

/** The highest number this box used, holds or handed out: a primary moves its counter past it. */
export function topOf(kept: Numbers = readNumbers()): number {
  // (Not a number an import was refused, `importedAs`: that one was never used.)
  return Math.max(0, (kept.next ?? 1) - 1, ...kept.leases.map(([, to]) => to), ...(kept.used ?? []).map(([, to]) => to), ...allItems().map(item => item.number ?? 0));
}

const keyOf = (primary: Primary, link: LinkState | null | undefined) => ("self" in primary ? "self" : "peer" in primary ? primary.peer.key : (readNumbers().primary ?? (link?.mac || undefined)));
const askID = () => `wqn_${crypto.randomBytes(12).toString("base64url")}`;
const leftIn = (leases: Ranges) => leases.reduce((sum, [from, to]) => sum + to - from + 1, 0);

/** `work` on the numbers, one process at a time. */
export function changeNumbers<T>(work: (kept: Numbers) => T, file = NUMBERS_FILE): T {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  return locked(`${path.join(path.dirname(file), "." + path.basename(file))}.lock`, () => {
    const stored = openNumbers(file);
    const kept = numbersOf(stored?.fields ?? {});
    const before = JSON.parse(JSON.stringify(kept)) as Record<string, unknown>;
    const value = work(kept);
    const after: Record<string, unknown> = { ...kept };
    const fields = { ...stored?.fields };
    let changed = stored?.upgraded ?? false;
    for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
      if (JSON.stringify(before[key]) === JSON.stringify(after[key])) continue;
      if (after[key] === undefined) delete fields[key];
      else fields[key] = after[key];
      changed = true;
    }
    if (changed) writeDurablePrivate(file, JSON.stringify(numbersFormat.write(fields)), file === NUMBERS_FILE ? RUN : path.dirname(file));
    return value;
  }, { exclusive: true });
}

/** The primary's counter: as kept, or, lost with its file, past every number here (never #1 again). */
function counterOf(kept: Numbers): number {
  return kept.next ?? Math.max(0, ...allItems().map(item => item.number ?? 0)) + 1;
}

/** Install the certified ledger once, inside the same lock as the first allocation. */
export function installNumberAuthority(kept: Numbers, link: LinkState | null | undefined): void {
  const setup = followedSetup(link);
  const allocation = link?.sharedSetup === setup?.id ? link?.shared?.queueNumberAllocation : undefined;
  const numbers = setup?.checkpointed ? allocation?.value : leadershipOf(link)?.body.numbers;
  if (setup?.role !== "primary" || !isObject(numbers)) return;
  const stamp = `${setup.id}:${setup.epoch}`;
  if (kept.leadership === stamp && kept.allocation === allocation?.sequence) return;
  const incoming = numbersOf(isObject(allocation?.value) ? allocation.value : numbers);
  kept.next = Math.max(counterOf(kept), incoming.next ?? 1, Number(numbers.next) || 1, topOf(kept) + 1);
  kept.given = incoming.given;
  kept.granted = incoming.granted;
  const me = setup.primary;
  if (kept.granted?.[me]) {
    kept.granted.self = kept.granted[me]!;
    delete kept.granted[me];
  }
  kept.leadership = stamp;
  kept.allocation = allocation?.sequence;
}

// Check once more after taking the ledger lock, in case a different process handed over.
function currentNumberAuthority(link: LinkState | null | undefined, file: string): LinkState | null | undefined {
  if (file !== NUMBERS_FILE) return link;
  const current = readLink();
  return current;
}

/** The primary `primary` noted as the one this box numbers from now. One that leaves numbering on its
 * own asks its new primary at once, so that one moves past what it handed out (`top`); one that
 * becomes the primary starts past every number it knows of. */
function notePrimary(kept: Numbers, primary: Primary, link: LinkState | null | undefined, now: number): void {
  if ("away" in primary) {
    kept.primary ??= keyOf(primary, link);
    return;
  }

  const key = keyOf(primary, link)!;
  const through = "peer" in primary ? primary.via?.key : undefined;
  if (through) kept.through = through;
  else delete kept.through;
  if (kept.primary === key) return;
  const was = kept.primary;
  kept.primary = key;
  if (was === "self") kept.asking ??= { id: askID(), count: BLOCK, since: now };

}

/** The highest number your other computers told this one of. */
export function toldTop(): number {
  return Math.max(0, ...toldKeys().map(key => {
    const told = readTold(key);
    return Math.max(told?.top ?? 0, ...(told?.items ?? []).map(item => item.number ?? 0));
  }));
}

/** Where a `top` another box tells can move the counter at most: room left for blocks below six
 * digits, so one far number never takes every box past what a chip reads. */
const TOP_MOST = MOST_NUMBER - 1 - 4 * BLOCK;
const cappedTop = (top: number) => Math.min(top, TOP_MOST);

/** The primary moved past `top`, a number another of your computers used (on the primary only). */
export function movePast(top: number, { file = NUMBERS_FILE }: { file?: string } = {}): void {
  if (!Number.isInteger(top) || top <= 0 || top >= MOST_NUMBER) return;
  top = cappedTop(top);
  if ((readNumbers(file).next ?? 0) > top) return;
  changeNumbers(kept => {
    if (kept.primary !== "self" && kept.primary !== undefined) return;
    kept.next = Math.max(counterOf(kept), top + 1);
  }, file);
}

type NumberOptions = { file?: string; now?: number; link?: LinkState | null };

/** Take the next local number, or leave the item waiting for a block. */
export function takeNumber(primary: Primary, options?: NumberOptions): number | null {
  return allocateNumbers(primary, 1, [], options)[0] ?? null;
}

/** Spend a block in one ledger write, with the same authority checks as taking one number. */
export function allocateNumbers(primary: Primary, count: number, used: number[], { file = NUMBERS_FILE, now = Date.now(), link = readLink() }: NumberOptions = {}): number[] {
  if (!count && !used.length) return [];
  return changeNumbers(kept => {
    if (used.length) kept.used = joined(kept.used ?? [], rangesOf(used)).slice(-MOST_RANGES);
    const allocated: number[] = [];
    for (let at = 0; at < count; at++) {
      const number = takeNumberFrom(kept, primary, file, now, link);
      if (number === null) break;
      allocated.push(number);
    }
    return allocated;
  }, file);
}

function takeNumberFrom(kept: Numbers, primary: Primary, file: string, now: number, link: LinkState | null): number | null {
  const current = currentNumberAuthority(link, file);
  if (current?.setupMerge) return null;
  if ("self" in primary && current?.setup?.managed && current.setup.role !== "primary") return null;
  notePrimary(kept, primary, current, now);
  if ("self" in primary && !kept.leases.length) {
    const ceiling = numberCeiling(kept, current, file);
    if (ceiling !== null) {
      const from = counterOf(kept);
      const to = Math.min(from + BLOCK - 1, ceiling);
      if (from <= to) {
        kept.next = to + 1;
        kept.leases = [[from, to]];
      }
    }
  }
  const lease = kept.leases[0];
  let number: number | null = null;
  if (lease) {
    number = lease[0];
    if (lease[0] === lease[1]) kept.leases.shift();
    else lease[0]++;
    kept.used = joined(kept.used ?? [], [[number, number]]).slice(-MOST_RANGES);
  }
  if (number === null || !("self" in primary) && leftIn(kept.leases) <= LOW) kept.asking ??= { id: askID(), count: BLOCK, since: now };
  return number;
}

export async function takeNumberLater(primary: Primary): Promise<number | null> {
  const [number] = await numberDisk.run("allocateNumbers", primary, 1, []);
  return number ?? null;
}

/** The highest number the computer `key` told it used or holds: its `top`, or its items'. */
const toldTopOf = (key: string): number => {
  const told = readTold(key);
  return Math.max(told?.top ?? 0, ...(told?.items ?? []).map(item => item.number ?? 0));
};

/** What an ask says of its claims: that the asking box never claims a number it gave an item (`own`,
 * from a box that keeps `used`; an older one doesn't say it), and which are asked back for items an
 * import renumbered (`restores`). */
interface ClaimsSaid { own?: boolean; restores?: Ranges }

// On the primary: the blocks it gave the box `by`, whose numbers are that box's own (an import there
// that claims one it never used keeps it).
// Until the blocks an older primary kept without whom they went to (`by`) are gone from `given`
// (MOST_GIVEN asks on): one of those counts for numbers asked back (`restores`) when it can only be
// that box's: every other computer this one knows told it of no number as high as its first (as one
// holding it would, its `top`), and the asking box told of one as high as its last.
function blocksOf(kept: Numbers, by: string, restores: Ranges): Ranges {
  const given = Object.values(kept.given ?? {});
  const own: Ranges = given.filter(each => each.by === by).map(each => [each.from, each.to]);
  const unsaid = restores.length ? given.filter(each => each.by === undefined) : [];
  if (!unsaid.length) return joined(own);
  const told = new Set(toldKeys());
  const known = new Set([...peersOf(readLink()).map(peer => peer.key), ...Object.keys(kept.granted ?? {}), ...told]);
  known.delete(by);
  known.delete("self");
  // (One that never told this one anything could hold any of them.)
  if ([...known].some(key => !told.has(key))) return joined(own);
  const others = Math.max(0, ...[...known].map(toldTopOf));
  const theirs = toldTopOf(by);
  const old = unsaid.filter(each => each.from > others && each.to <= theirs).map((each): [number, number] => [each.from, each.to]);
  return joined(own, overlap(joined(old), restores));
}

// On the primary: which of `claims` the box `by` may have (at or above the counter and within reach
// of it, granted it before, or, from a box that never claims one it used, in a block it gave that
// box), the counter moved past them.
function grantOn(kept: Numbers, by: string, claims: Ranges, said: ClaimsSaid = {}, ceiling = MOST_NUMBER - 1): Ranges {
  const before = kept.granted?.[by] ?? [];
  const next = counterOf(kept);
  const blocks = by !== "self" && said.own ? blocksOf(kept, by, overlap(said.restores ?? [], claims)) : [];
  const ranges = joined(overlap(claims, [[next, Math.min(next + CLAIM_REACH, ceiling)]]), overlap(claims, before), overlap(claims, blocks));
  if (ranges.length) {
    kept.next = Math.max(next, ranges[ranges.length - 1][1] + 1);
    kept.granted = { ...(kept.granted ?? {}), [by]: joined(before, ranges).slice(-MOST_RANGES) };
  }
  return ranges;
}

/** Numbers an import here wants to keep. On the primary, granted at once: what it may have
 * (`granted`). Elsewhere, claimed from the primary with the next ask (leases at or below them
 * dropped): `granted` is null, and the answer comes by mail (`takeBlock`). */
export function claimNumbers(primary: Primary, numbers: number[], { file = NUMBERS_FILE, now = Date.now(), link = readLink(), restores = [] }: { file?: string; now?: number; link?: LinkState | null; restores?: number[] } = {}): { granted: Ranges | null } {
  const claims = rangesOf(numbers);
  if (!claims.length) return { granted: [] };
  return changeNumbers(kept => {
    const current = currentNumberAuthority(link, file);
    if ("self" in primary && current?.setup?.managed && current.setup.role !== "primary") return { granted: null };
    notePrimary(kept, primary, current, now);
    if ("self" in primary) {
      const ceiling = numberCeiling(kept, current, file);
      if (ceiling !== null && hasClaimRoom(kept, claims, 0, ceiling)) return { granted: grantOn(kept, "self", claims, {}, ceiling) };
    }
    const highest = claims[claims.length - 1][1];
    kept.leases = kept.leases.map(([from, to]): [number, number] => [Math.max(from, highest + 1), to]).filter(([from, to]) => from <= to);
    // (A new ask, so the claims go with it even if one without them was sent already.)
    const back = joined(kept.asking?.restores ?? [], overlap(rangesOf(restores), claims)).slice(-MOST_RANGES);
    kept.asking = { id: askID(), count: BLOCK, claims: joined(kept.asking?.claims ?? [], claims).slice(-MOST_RANGES), ...(back.length ? { restores: back } : {}), since: now };
    return { granted: null };
  }, file);
}

/** What goes to the primary now: the ask under way, if it wasn't sent in the last half minute
 * (kept as sent before it goes), with the highest number this box used or holds (`top`). */
export function askToSend({ file = NUMBERS_FILE, now = Date.now(), retry = ASK_AGAIN }: { file?: string; now?: number; retry?: number } = {}): { id: string; count: number; claims?: Ranges; restores?: Ranges; own?: true; top?: number } | null {
  const asked = changeNumbers(kept => {
    const asking = kept.asking;
    if (!asking || (asking.sent !== undefined && now - asking.sent < retry)) return null;
    asking.sent = now;
    return { id: asking.id, count: asking.count, ...(asking.claims?.length ? { claims: asking.claims, own: true as const } : {}), ...(asking.restores?.length ? { restores: asking.restores } : {}) };
  }, file);
  if (!asked) return null;
  const top = topOf(readNumbers(file));
  return { ...asked, ...(top ? { top } : {}) };
}

/** Whether this box's primary changed (one set up earlier heard of, or the one it had gone): noted
 * now, so a box that numbered on its own asks its new primary at once. */
export function notePrimaryNow(link: LinkState | null | undefined = readLink(), { file = NUMBERS_FILE, now = Date.now() }: { file?: string; now?: number } = {}): void {
  const primary = primaryOf(link);
  const kept = readNumbers(file);
  if (kept.primary === undefined || "away" in primary || (kept.primary === keyOf(primary, link) && kept.through === ("peer" in primary ? primary.via?.key : undefined))) return;
  changeNumbers(numbers => notePrimary(numbers, primary, link, now), file);
}

/** Apply the relay's floor once per term, then only allocate through its confirmed high mark. */
function numberCeiling(kept: Numbers, link: LinkState | null | undefined, file: string): number | null {
  if (file !== NUMBERS_FILE) return MOST_NUMBER - 1;
  if (!("self" in primaryOf(link)) || primaryFenced(link)) return null;
  if (!link?.setup) return MOST_NUMBER - 1;
  const lease = relayLease(link);
  if (!lease || lease.numberFloor === undefined) return null;
  installNumberAuthority(kept, link);
  kept.next = Math.max(counterOf(kept), lease.numberFloor + 1);
  return Math.min(MOST_NUMBER - 1, lease.high);
}

export function reserveMergeNumbers(setup: string, operation: string, count: number): { from: number; to: number } | { exhausted: true } | null {
  return changeNumbers(kept => {
    const link = readLink();
    if (link?.setup?.id !== setup) return null;
    const ceiling = numberCeiling(kept, link, NUMBERS_FILE);
    if (ceiling === null) return null;
    const from = counterOf(kept);
    const to = from + count - 1;
    const id = `merge:${operation}`;
    const prior = kept.given?.[id];
    if (prior) return { from: prior.from, to: prior.to };
    if (to >= MOST_NUMBER) return { exhausted: true };
    if (to > ceiling) {
      kept.reserveThrough = Math.max(kept.reserveThrough ?? 0, to);
      return null;
    }
    delete kept.reserveThrough;
    keepBlock(kept, link.setup.primary, id, from, to, [], Date.now());
    return { from, to };
  });
}

function keepBlock(kept: Numbers, by: string, id: string, from: number, to: number, granted: Ranges, at: number) {
  kept.next = to + 1;
  const all = Object.entries({ ...kept.given, [id]: { from, to, granted, at, by } });
  kept.given = Object.fromEntries(all.sort((a, b) => a[1].at - b[1].at).slice(-MOST_GIVEN));
  return { id, from, to, granted };
}

function hasClaimRoom(kept: Numbers, claims: Ranges, count: number, ceiling: number) {
  const next = counterOf(kept);
  const claimLimit = Math.min(next + CLAIM_REACH, MOST_NUMBER - count - 1);
  const last = Math.max(next - 1, ...overlap(claims, [[next, claimLimit]]).map(([, to]) => to));
  if (last + count <= ceiling) return true;
  kept.reserveThrough = Math.max(kept.reserveThrough ?? 0, last + count);
  return false;
}

/** The main records a block before returning it, and replies to retries with the same block. */
export function giveBlock(by: string, ask: unknown, { file = NUMBERS_FILE, now = Date.now() }: { file?: string; now?: number } = {}): { id: string; from: number; to: number; granted: Ranges } | null {
  if (!isObject(ask) || typeof ask.id !== "string" || !ASK_ID.test(ask.id)) return null;
  const id = ask.id;
  const count = typeof ask.count === "number" && Number.isInteger(ask.count) ? Math.min(Math.max(ask.count, 1), 200) : BLOCK;
  const claims = cleanRanges(ask.claims);
  const top = typeof ask.top === "number" && Number.isInteger(ask.top) && ask.top > 0 && ask.top < MOST_NUMBER ? cappedTop(ask.top) : 0;
  const said = { own: ask.own === true, restores: cleanRanges(ask.restores) };
  return changeNumbers(kept => {
    const link = file === NUMBERS_FILE ? readLink() : null;
    const ceiling = numberCeiling(kept, link, file);
    if (ceiling === null || (link?.setup && by !== link.setup.primary && !peersOf(link).some(peer => peer.key === by))) return null;
    const given = kept.given?.[id];
    if (given) return given.by && given.by !== by ? null : { id, from: given.from, to: given.to, granted: given.granted ?? [] };
    kept.next = Math.max(counterOf(kept), top + 1);
    if (!hasClaimRoom(kept, claims, count, ceiling)) return null;
    const granted = grantOn(kept, by, claims, said, ceiling - count);
    const from = counterOf(kept);
    return keepBlock(kept, by, id, from, from + count - 1, granted, now);
  }, file);
}

/** What the primary said to claims: those it granted, of those it was asked (`claimed`). */
export interface Granting { granted: Ranges; claimed: Ranges }

/** A block the primary gave, kept, if it answers the ask under way: the claims that ask carried and
 * those granted, or null for an answer to no ask under way. */
export function takeBlock(give: unknown, { file = NUMBERS_FILE }: { file?: string } = {}): Granting | null {
  if (!isObject(give) || typeof give.id !== "string" || !Number.isInteger(give.from) || !Number.isInteger(give.to)) return null;
  const from = give.from as number;
  const to = give.to as number;
  if (from < 1 || to < from || to - from > 1000) return null;
  const granted = cleanRanges(give.granted);
  return changeNumbers(kept => {
    if (kept.asking?.id !== give.id) return null;
    const claimed = kept.asking?.claims ?? [];
    kept.asking = undefined;
    kept.leases = joined(kept.leases, [[from, to]]);
    return { granted, claimed };
  }, file);
}

/** Items here that wait: those imported take the number they wanted where it was granted (one
 * refused is noted and numbered afresh), and those an import renumbered move to their own where it
 * was given back (`restoreNumbers`); only the items whose numbers that answer was asked (`claimed`)
 * are decided, the others wait for theirs. Then every one without a number takes the next, oldest
 * first. The numbers items took are noted as used here. */
export function numberWaiting(book: Book, primary: Primary, answer: Granting | null, options: { file?: string; now?: number } = {}): void {
  const { waiting, used } = waitingForNumbers(book, answer, options.now ?? Date.now());
  const allocated = allocateNumbers(primary, waiting.length, used, options);
  allocated.forEach((number, at) => { waiting[at]!.number = number; });
}

export async function numberWaitingLater(book: Book, primary: Primary, answer: Granting | null, options: { file?: string; now?: number } = {}): Promise<void> {
  const { waiting, used } = waitingForNumbers(book, answer, options.now ?? Date.now());
  if (!waiting.length && !used.length) return;
  const allocated = await numberDisk.run("allocateNumbers", primary, waiting.length, used, options);
  allocated.forEach((number, at) => { waiting[at]!.number = number; });
}

function waitingForNumbers(book: Book, answer: Granting | null, now: number) {
  const used: number[] = [];
  const link = readLink();
  const moves = liveMoves();
  const mayNumber = (item: Item) => !item.handing && !item.source && OWNER.test(item.owner)
    && mayActivate({ kind: "queueItems", id: item.id, queue: item.owner }, { link, moves });
  if (answer) {
    const held = new Set(book.items().flatMap(item => item.number === undefined ? [] : [item.number]));
    for (const item of book.items()) {
      if (item.wanted === undefined || item.backGranted || !inRanges(answer.claimed, item.wanted) || !mayNumber(item)) continue;
      const kept = inRanges(answer.granted, item.wanted);
      if (item.number !== undefined) {
        if (kept) item.backGranted = true;
        else stays(item, `Its own number, #${item.wanted}, couldn't be given back (another of your computers was given it already, or it's far past the numbers in use), so it stays #${item.number}.`, now);
        continue;
      }
      if (kept && !held.has(item.wanted)) {
        item.number = item.wanted;
        held.add(item.number);
        used.push(item.number);
      } else {
        item.notes.push({ at: now, by: { user: true }, text: `Imported as #${item.wanted}, a number that couldn't be kept (another of your computers was given it already, or it's far past the numbers in use), so it has a new one.` });
        item.importedAs = item.wanted;
      }
      delete item.wanted;
    }
  }
  used.push(...restoreNumbers(book, now));
  const waiting = book.items().filter(item => item.number === undefined && item.wanted === undefined && mayNumber(item)).sort((a, b) => a.created.at - b.created.at);
  return { waiting, used };
}

// An item asking for its own number back that keeps the one it has, for good (`why`, its note).
function stays(item: Item, why: string, now: number): void {
  item.notes.push({ at: now, by: { user: true }, text: why });
  delete item.wanted;
  delete item.backGranted;
  item.stays = true;
}

/** Items an import renumbered whose own numbers were given back (`backGranted`), moved to them all
 * at once, so two can trade. One whose number another item here has waits while that one is asking
 * for its own (not asked again: settled when that one is); else it stays as it is, with a note, as
 * does a second one asking for the same number. What it used: the numbers they moved to (those
 * they left are no item's now, and an import here may give one back to the item it names). */
function restoreNumbers(book: Book, now: number): number[] {
  const back = book.items().filter(item => item.backGranted && item.wanted !== undefined && item.number !== undefined);
  if (!back.length) return [];
  const first = new Map<number, Item>();
  for (const item of back) if (!first.has(item.wanted!)) first.set(item.wanted!, item);
  const going = new Set(first.values());
  const holders = new Map<number, Item[]>();
  for (const item of book.items()) {
    const held = item.number ?? item.wanted;
    if (held !== undefined) holders.set(held, [...(holders.get(held) ?? []), item]);
  }
  const blocker = (item: Item): Item | undefined => (first.get(item.wanted!) !== item ? first.get(item.wanted!) : undefined)
    ?? (holders.get(item.wanted!) ?? []).find(each => each !== item && !going.has(each));
  for (let changed = true; changed;) {
    changed = false;
    for (const item of going) {
      if (!blocker(item)) continue;
      going.delete(item);
      changed = true;
    }
  }
  for (const item of back) {
    if (going.has(item)) continue;
    const holder = blocker(item)!;
    // (One that has it and is asking for its own: this one moves once that one has.)
    if (!holder.removed && !holder.stays && holder.number === item.wanted && holder.wanted !== undefined) continue;
    stays(item, `Its own number, #${item.wanted}, is another item's here, so it stays #${item.number}.`, now);
  }
  const used: number[] = [];
  for (const item of going) {
    item.notes.push({ at: now, by: { user: true }, text: `Has its own number, #${item.wanted}, again (it was #${item.number}).` });
    used.push(item.wanted!);
    item.number = item.wanted;
    delete item.wanted;
    delete item.importedAs;
    delete item.backGranted;
  }
  return used;
}

/** Items here still waiting for a number with nothing under way to give them one (a crash between
 * a block's coming and its use, or items a move brought), or asking for their own number back:
 * their claims asked again (on the primary, granted at once), and a block asked for those with
 * none; those given theirs back and waiting for another item to move are settled. What it did:
 * whether anything waited. */
export function reclaimWaiting(primary: Primary = primaryOf(), options: { file?: string; now?: number } = {}): boolean {
  const mine = allItems().filter(item => !item.source && !item.removed && !item.handing && OWNER.test(item.owner));
  const waiting = mine.filter(item => item.number === undefined || (item.wanted !== undefined && !item.backGranted));
  const settling = mine.some(item => item.backGranted && item.wanted !== undefined);
  if (!waiting.length && !settling) return false;
  const kept = readNumbers(options.file);
  const asked = kept.asking?.claims ?? [];
  const wanted = waiting.flatMap(item => (item.wanted !== undefined && !inRanges(asked, item.wanted) ? [item.wanted] : []));
  const restores = waiting.flatMap(item => (item.number !== undefined && item.wanted !== undefined && !inRanges(asked, item.wanted) ? [item.wanted] : []));
  if ("self" in primary) {
    const granted = wanted.length ? claimNumbers(primary, wanted, { ...options, restores }).granted : [];
    changeQueues(book => numberWaiting(book, primary, granted && { granted, claimed: rangesOf(wanted) }, options), undefined, { onItsOwn: true });
    return true;
  }
  if (wanted.length) claimNumbers(primary, wanted, { ...options, restores });
  else if (!kept.asking && !leftIn(kept.leases) && waiting.some(item => item.number === undefined)) changeNumbers(numbers => { numbers.asking ??= { id: askID(), count: BLOCK, since: options.now ?? Date.now() }; }, options.file);
  // (A lease left: numbered now.)
  if (leftIn(readNumbers(options.file).leases) || settling) changeQueues(book => numberWaiting(book, primary, null, options), undefined, { onItsOwn: true });
  return true;
}

// Run these ledger and queue writes together on the upkeep worker. It can wait for the
// queue writer lock while the separate ledger worker still serves that writer.
let lookedForWaiting = 0;
export function numbersToSend(link: LinkState | null, welcomed: boolean) {
  let changed = false;
  if (Date.now() - lookedForWaiting > 60_000) {
    lookedForWaiting = Date.now();
    try { notePrimaryNow(link); } catch {}
    try { changed = reclaimWaiting(primaryOf(link)); } catch {}
  }
  const primary = primaryOf(link);
  const kept = readNumbers();
  if ("self" in primary) {
    if (link?.setup && kept.asking) {
      const block = giveBlock(link.setup.primary, { ...kept.asking, own: true });
      if (block) changed = blockArrived(block, primary) || changed;
    }
    return { changed };
  }
  if (!welcomed || !link || !kept.asking || !("peer" in primary)) return { changed };
  const ask = askToSend({ retry: 2_000 });
  return { changed, ...(ask ? { peer: primary.peer, ask } : {}) };
}

let pushingNumbers = false;
let checkedNumbers = 0;
/** Send a waiting ask to the main, or fulfill it locally when this box holds the lease. */
export async function pushNumbers(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null = readLink()): Promise<void> {
  if (pushingNumbers) return;
  const kept = readNumbers();
  const now = Date.now();
  if (!kept.asking && (now - checkedNumbers < 60_000 || !kept.primary && !fs.existsSync(QUEUES))) return;
  pushingNumbers = true;
  try {
    const { changed, peer, ask } = await numberUpkeepDisk.run("numbersToSend", link, Boolean(host?.welcomed));
    if (now - checkedNumbers >= 60_000) checkedNumbers = now;
    if (changed) noteQueuesChanged();
    if (!host?.welcomed || !link || !peer || !ask) return;
    const bytes = mailFor(peer, { workQueue: { ask }, at: Date.now() }, link);
    if (bytes) try { host.send(bytes); } catch {}
  } finally { pushingNumbers = false; }
}

export function receiveNumbers(key: string, given: Record<string, unknown>) {
  const link = readLink();
  const primary = primaryOf(link);
  if (isObject(given.ask) && "self" in primary && given.to === undefined && given.for === undefined)
    return { link, block: giveBlock(key, given.ask) };
  if (isObject(given.give) && given.for === undefined && "peer" in primary && primary.peer.key === key)
    return { changed: blockArrived(given.give, primary) };
  return {};
}

/** Numbers' mail from another box: reply to the ask, or use the granted block. */
export async function takeNumbersMail(sender: MailSender, given: Record<string, unknown>, host: Pick<LinkHost, "send"> | null | undefined): Promise<void> {
  const { changed, block, link } = await numberUpkeepDisk.run("receiveNumbers", sender.key, given);
  if (changed) noteQueuesChanged();
  // Older ranked members still look for quorumGrant when accepting a grant.
  const bytes = block && link && "address" in sender && sender.address
    ? mailFor(sender, { workQueue: { give: block, quorumGrant: true }, at: Date.now() }, link) : null;
  if (bytes) try { host?.send(bytes); } catch {}
}

/** A block from the primary, kept, and what waits here numbered with it (imports' wanted numbers
 * as granted): whether it answered the ask under way. */
export function blockArrived(give: unknown, primary: Primary, options: { file?: string } = {}): boolean {
  const answer = takeBlock(give, options);
  if (!answer) return false;
  changeQueues(book => numberWaiting(book, primary, answer, options), undefined, { onItsOwn: true });
  return true;
}
