// One fleet, one version (PROTOCOL.md, "Updates", Your computers follow each other): every
// computer of yours runs the newest signed release any of them holds. A box keeps the signed release
// it runs (`updates/release`), says in its agents mail which it holds (`computer.release`) and which
// Kuvo it runs (`computer.kuvo`), and a box that's behind pulls the newer one from the peer that
// holds it, part by part over peer mail (end to end encrypted, through the relay), checks it as any
// release (the pinned key, newer, the pack's SHA-256), and offers it to itself: taken at once by the
// offered path, which stops nothing. Never older, never unsigned, never from a box that isn't one of
// your own computers.
// Platform: the same on macOS and Linux.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { listOf, writePrivate } from "../files";
import { REAL_BOX, UPDATES } from "../home";
import { readPeerAgents } from "../peers/directory";
import { mailFor, peersOf } from "../peers/known";
import type { LinkHost } from "../relay/host";
import { readLink } from "../state";
import {
  type BoxRelease, UPDATE_KEYS, boxVersion, isUpdating, newerVersion, offerRests, offerUpdate, offeredVersion, sameVersion,
  startOfferedUpdate, updateOnTrial, updatesBlocked, verifyBoxes,
} from "./update";

/** Where the signed release this box runs is kept: `boxes.json` and `pack.tgz`. */
export const HELD = path.join(UPDATES, "release");
/** Where a release pulled from a peer is gathered: `<version>.json` (its boxes.json) and
 * `<version>.pack` (its pack, filled in part by part), and the folder it's offered from. */
export const INCOMING = path.join(UPDATES, "incoming");
/** Each part's size: as moves send theirs, well inside one sealed mail. */
export const PART = 30_000;
/** How many parts are asked for at once. */
const WINDOW = 8;
/** The largest pack a box takes (as `update` reads one). */
const MOST_PACK = 100_000_000;
/** A pull that heard nothing this long asks again for what it waits for; one that made no progress
 * for `GIVE_UP` leaves that peer for the next. */
const QUIET = 60_000;
const GIVE_UP = 5 * 60_000;
/** A peer that couldn't hand a version over isn't asked for it again for this long. */
const TRIED_FOR = 3600_000;

/** The signed release this box holds and runs: its version, its signed manifest as it came
 * (`boxes.json`'s text), its pack's file and size. None while it isn't the version running (an
 * update taken back), or while an update is on trial here: a release goes on from a computer only
 * once it's judged well there. */
export interface Held {
  version: string;
  boxes: string;
  pack: string;
  size: number;
}

export function heldRelease(keys: string[] = ownWays().keys ?? UPDATE_KEYS): Held | null {
  if (updateOnTrial() || isUpdating()) return null;
  try {
    const boxes = fs.readFileSync(path.join(HELD, "boxes.json"), "utf8");
    const manifest = verifyBoxes(boxes, keys);
    if (!sameVersion(manifest.version, boxVersion())) return null;
    const pack = path.join(HELD, "pack.tgz");
    const size = fs.statSync(pack).size;
    return { version: manifest.version, boxes, pack, size };
  } catch { return null; }
}

/** The release just put in place (its signed manifest and pack, checked already) kept as the one
 * this box holds: whole or not at all, the one before gone. */
export function keepRelease(boxes: Buffer | string, pack: Buffer): void {
  const partial = `${HELD}.partial`;
  try {
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
    fs.writeFileSync(path.join(partial, "pack.tgz"), pack, { mode: 0o600 });
    fs.rmSync(HELD, { recursive: true, force: true });
    fs.renameSync(partial, HELD);
  } catch {
    fs.rmSync(partial, { recursive: true, force: true });
  }
}

/** What this box tells its peers of its Kuvo (`computer.kuvo`, `computer.release`). */
export function fleetFacts(): { kuvo?: string; release?: string } {
  const kuvo = boxVersion(), held = heldRelease();
  return { ...(kuvo ? { kuvo } : {}), ...(held ? { release: held.version } : {}) };
}

// ---- Pulling a newer one ----

/** The pull under way (one at a time, in the process with the relay connection). */
interface Pull {
  peer: string;
  version: string;
  manifest?: BoxRelease;
  boxes?: string;
  parts?: number;
  got: Set<number>;
  asked: Map<number, number>;
  heard: number;
  progressed: number;
}
let pull: Pull | null = null;
/** Each `<peer> <version>` that couldn't be had from that peer, with when. */
const tried = new Map<string, number>();

/** How a box follows its peers: for a test, its keys, its block, its clock, where its mail goes and
 * what's done with a release once it's offered. */
export interface FleetOptions {
  keys?: string[];
  blocked?: string | null;
  clock?: () => number;
  send?: (peer: string, release: Record<string, unknown>) => boolean;
  offered?: () => void;
}

let hostOf: () => Pick<LinkHost, "send"> | null | undefined = () => null;
// A test's own ways (its keys only in a test's folder), as the mail it takes uses them.
let ways: FleetOptions = {};
const ownWays = (): FleetOptions => ways;
/** For a test: where its mail goes, its keys, its block and what's done with an offered release. */
export function fleetWith(options: FleetOptions): void {
  ways = REAL_BOX ? { ...options, keys: undefined, blocked: undefined } : options;
}
/** Mail goes by the relay connection `host` gives (the one that's up). */
export function followOver(host: () => Pick<LinkHost, "send"> | null | undefined): void {
  hostOf = host;
}

function tell(peer: string, release: Record<string, unknown>, options: FleetOptions): boolean {
  if (options.send) return options.send(peer, release);
  const link = readLink();
  const to = peersOf(link).find(each => each.key === peer);
  const bytes = link && to && mailFor(to, { release, at: Date.now() }, link);
  if (!bytes) return false;
  try { return hostOf()?.send(bytes) === true; } catch { return false; }
}

const log = (words: string) => console.error(`Kuvo updates: ${words}`);

/** Why this box takes no release from a peer now, or null. */
function inTheWay(version: string, blocked: string | null, now: number): boolean {
  if (blocked || isUpdating() || updateOnTrial()) return true;
  // (One as new or newer offered already: that one's taken first.)
  const offered = offeredVersion();
  if (offered && !newerVersion(version, offered)) return true;
  return offerRests(version, now);
}

/** The newest release its peers hold that's newer than this box, and who holds it: the newest
 * version first, then the lowest key, each peer that couldn't hand it over lately left out. */
export function newestHeld(now = Date.now()): { peer: string; version: string } | null {
  const mine = boxVersion();
  if (!mine) return null;
  const told = readPeerAgents();
  const holding = peersOf(readLink())
    .map(peer => ({ peer: peer.key, version: told[peer.key]?.computer?.release }))
    .filter((each): each is { peer: string; version: string } => typeof each.version === "string" && newerVersion(each.version, mine))
    .filter(each => now - (tried.get(`${each.peer} ${each.version}`) ?? -Infinity) >= TRIED_FOR)
    .sort((a, b) => (newerVersion(a.version, b.version) ? -1 : newerVersion(b.version, a.version) ? 1 : a.peer < b.peer ? -1 : a.peer > b.peer ? 1 : 0));
  return holding[0] ?? null;
}

/** A look: a peer holds a newer release than this box, and nothing's in the way: it's pulled from
 * that peer (or the pull under way goes on, asking again for what it waits for). As peers' agents
 * mail comes, and each minute. */
export function followPeers(asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  const { clock = Date.now, blocked = updatesBlocked() } = options;
  if (!(options.keys ?? UPDATE_KEYS).length) return;
  const now = clock();
  const best = newestHeld(now);
  if (pull) {
    // (Caught up meanwhile, or an update began here: looked at again once it's over.)
    if (!newerVersion(pull.version, boxVersion()) || isUpdating() || updateOnTrial()) return endPull(null);
    // (A computer that couldn't hand it over in time: the next that holds it, or the next look.)
    if (now - pull.progressed >= GIVE_UP) return endPull(`${pull.version} couldn't be had from that computer in time.`, now);
    // (Something newer elsewhere: this one goes, and that one's pulled now.)
    if (!best || !newerVersion(best.version, pull.version)) {
      if (now - pull.heard >= QUIET) ask(pull, options, now, true);
      return;
    }
    endPull(null);
  }
  if (!best || inTheWay(best.version, blocked, now)) return;
  pull = { peer: best.peer, version: best.version, got: new Set(), asked: new Map(), heard: now, progressed: now };
  fs.rmSync(INCOMING, { recursive: true, force: true });
  ask(pull, options, now, false);
}

// What the pull asks for next: the release's manifest, then its parts, `WINDOW` at once; `again`:
// what was asked and not answered is asked again.
function ask(current: Pull, options: FleetOptions, now: number, again: boolean): void {
  if (again) current.heard = now;
  if (!current.manifest) {
    tell(current.peer, { want: current.version }, options);
    return;
  }
  if (again) current.asked.clear();
  for (let i = 0; i < current.parts! && current.asked.size < WINDOW; i++) {
    if (current.got.has(i) || current.asked.has(i)) continue;
    current.asked.set(i, now);
    tell(current.peer, { version: current.version, part: i }, options);
  }
}

function endPull(why: string | null, now = Date.now()): void {
  if (why && pull) {
    tried.set(`${pull.peer} ${pull.version}`, now);
    log(why);
  }
  pull = null;
  fs.rmSync(INCOMING, { recursive: true, force: true });
}

/** Release mail from a peer (`sender`, one of this box's own computers): asked for the release it
 * holds, or for a part of it, it answers; told the manifest or a part of the one it pulls, it takes
 * it in, and once the pack is whole and what it says, offers it to itself. */
export function takeReleaseMail(sender: { key: string }, given: Record<string, unknown>, asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  const { keys = UPDATE_KEYS, clock = Date.now, blocked = updatesBlocked() } = options;
  // Asked: what this box holds, if it's that version.
  if (typeof given.want === "string") {
    const held = heldRelease(keys);
    tell(sender.key, held && held.version === given.want ? { version: held.version, boxes: held.boxes, size: held.size } : { version: given.want, none: true }, options);
    return;
  }
  if (typeof given.version !== "string") return;
  if (Number.isInteger(given.part) && given.d === undefined) {
    const held = heldRelease(keys);
    const i = given.part as number;
    if (!held || held.version !== given.version || i < 0 || i * PART >= held.size) return;
    const bytes = Buffer.alloc(Math.min(PART, held.size - i * PART));
    const handle = fs.openSync(held.pack, "r");
    try { fs.readSync(handle, bytes, 0, bytes.length, i * PART); } finally { fs.closeSync(handle); }
    tell(sender.key, { version: held.version, i, d: bytes.toString("base64") }, options);
    return;
  }
  // Answers: only from the peer pulled from, for the version pulled.
  const current = pull;
  if (!current || current.peer !== sender.key || current.version !== given.version) return;
  const now = clock();
  current.heard = now;
  if (given.none === true) return endPull(`that computer no longer holds ${current.version}.`, now);
  if (typeof given.boxes === "string" && !current.manifest) {
    let manifest: BoxRelease;
    try {
      manifest = verifyBoxes(given.boxes, keys);
    } catch (error) {
      return endPull(`what it sent for ${current.version} isn't a release this computer takes: ${(error as Error).message}`, now);
    }
    const size = given.size;
    // (Exactly what was asked for, newer than this box, and a pack's size a box takes.)
    if (manifest.version !== current.version || !newerVersion(manifest.version, boxVersion())) return endPull(`what it sent isn't ${current.version}.`, now);
    if (typeof size !== "number" || !Number.isInteger(size) || size <= 0 || size > MOST_PACK) return endPull(`its ${current.version} isn't a size a release can be.`, now);
    if (inTheWay(manifest.version, blocked, now)) return endPull(null);
    fs.mkdirSync(INCOMING, { recursive: true, mode: 0o700 });
    writePrivate(path.join(INCOMING, `${current.version}.json`), given.boxes);
    fs.writeFileSync(path.join(INCOMING, `${current.version}.pack`), Buffer.alloc(0), { mode: 0o600 });
    fs.truncateSync(path.join(INCOMING, `${current.version}.pack`), size);
    current.manifest = manifest;
    current.boxes = given.boxes;
    current.parts = Math.ceil(size / PART);
    current.progressed = now;
    return ask(current, options, now, false);
  }
  if (!current.manifest || !Number.isInteger(given.i) || typeof given.d !== "string") return;
  const i = given.i as number;
  if (i < 0 || i >= current.parts! || current.got.has(i)) return;
  const bytes = Buffer.from(given.d, "base64");
  const size = fs.statSync(path.join(INCOMING, `${current.version}.pack`)).size;
  if (bytes.length !== Math.min(PART, size - i * PART)) return endPull(`a part of ${current.version} it sent isn't the size it said.`, now);
  const handle = fs.openSync(path.join(INCOMING, `${current.version}.pack`), "r+");
  try { fs.writeSync(handle, bytes, 0, bytes.length, i * PART); } finally { fs.closeSync(handle); }
  current.got.add(i);
  current.asked.delete(i);
  current.progressed = now;
  if (current.got.size < current.parts!) return ask(current, options, now, false);
  finish(current, options, now);
}

// The pack whole: checked against its signed SHA-256, then offered to this box as Kuvo Code on a
// Mac offers the release it carries, from a folder of its own (checked again there), and taken.
function finish(current: Pull, options: FleetOptions, now: number): void {
  const pack = fs.readFileSync(path.join(INCOMING, `${current.version}.pack`));
  if (crypto.createHash("sha256").update(pack).digest("hex") !== current.manifest!.pack.sha256) return endPull(`the ${current.version} it sent isn't what its release says.`, now);
  let name = "";
  try { name = decodeURIComponent(path.posix.basename(new URL(current.manifest!.pack.url).pathname)); } catch {}
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) return endPull(`its ${current.version} names no pack.`, now);
  const folder = path.join(INCOMING, "release");
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  writePrivate(path.join(folder, "boxes.json"), current.boxes!);
  fs.writeFileSync(path.join(folder, name), pack, { mode: 0o600 });
  const offered = offerUpdate(folder, { keys: options.keys ?? UPDATE_KEYS, blocked: options.blocked === undefined ? updatesBlocked() : options.blocked, clock: options.clock });
  if (offered.status !== 202) {
    endPull(`${current.version} couldn't be offered here: ${offered.message ?? offered.status}`, now);
    return;
  }
  log(`${current.version} came from another of your computers; it's taken now.`);
  pull = null;
  fs.rmSync(INCOMING, { recursive: true, force: true });
  (options.offered ?? (() => { startOfferedUpdate().catch(() => {}); }))();
}

/** The pull under way, for a test: its peer, version and how many parts it has of how many. */
export function pullUnderWay(): { peer: string; version: string; got: number; parts?: number } | null {
  return pull ? { peer: pull.peer, version: pull.version, got: pull.got.size, ...(pull.parts !== undefined ? { parts: pull.parts } : {}) } : null;
}

/** Forgets what's pulled and tried, for a test. */
export function forgetFleet(): void {
  pull = null;
  tried.clear();
  fs.rmSync(INCOMING, { recursive: true, force: true });
}

// (Nothing else kept in `updates/incoming` outlives a pull: a box that starts again begins anew.)
export function clearIncoming(): void {
  if (listOf(INCOMING).length) fs.rmSync(INCOMING, { recursive: true, force: true });
}
