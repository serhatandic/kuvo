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
import { watchFolders } from "../follow";
import { readPeerAgents, vouchedOf } from "../peers/directory";
import { type Peer, mailFor, peersOf } from "../peers/known";
import { setupView } from "../setup/view";
import type { LinkState } from "../state";
import type { LinkHost } from "../relay/host";
import { readLink } from "../state";
import {
  type BoxRelease, UPDATE_KEYS, UPDATING_ALREADY, boxVersion, engineBehind, isUpdating, newerVersion, offerFailedTwice, offerHeld, offerRests, offerUpdate,
  offeredVersion, packName, startOfferedUpdate, updatesBlocked, verifyBoxes,
} from "./update";

/** Where the newest signed release this box holds is kept: `boxes.json` and its pack, by the name
 * its address ends in (a folder a release can be offered or taken from as it is). */
export const HELD = path.join(UPDATES, "release");
/** Where a release pulled from a peer is gathered: `<version>.json` (its boxes.json) and
 * `<version>.pack` (its pack, filled in part by part), and the folder it's offered from. */
export const INCOMING = path.join(UPDATES, "incoming");
/** Each part's size: as moves send theirs, well inside one sealed mail. */
export const PART = 30_000;
/** How many parts are asked for at once. */
const WINDOW = 16;
/** The largest pack a box takes (as `update` reads one). */
const MOST_PACK = 100_000_000;
/** A pull that heard nothing this long asks again for what it waits for; one that made no progress
 * for `GIVE_UP` leaves that peer for the next. */
const QUIET = 60_000;
const GIVE_UP = 5 * 60_000;
/** A peer that couldn't hand a version over isn't asked for it again for this long. */
const TRIED_FOR = 3600_000;

/** The newest signed release this box holds: its version, its signed manifest as it came
 * (`boxes.json`'s text), its pack's file and size. Held from the moment it's checked here (Kuvo Code
 * here handed it over, the box fetched it, or a peer did), before or while it's taken, and kept
 * whatever came of taking it here: each computer judges a release for itself, by its own health
 * rule, and goes back by itself, so none waits on another's trial. */
export interface Held {
  version: string;
  boxes: string;
  pack: string;
  size: number;
}

/** The pack held, checked against its signed SHA-256 once for each file there (its size and time). */
let packChecked: { stamp: string; sha: string; ok: boolean } | null = null;

export function heldRelease(keys: string[] = ownWays().keys ?? UPDATE_KEYS): Held | null {
  try {
    const boxes = fs.readFileSync(path.join(HELD, "boxes.json"), "utf8");
    const manifest = verifyBoxes(boxes, keys);
    // Only a release signed to go to your other computers (`spreads`): never a build made for
    // trying an update, which would take its version's place on every computer.
    if ((manifest as { spreads?: unknown }).spreads !== true) return null;
    const pack = path.join(HELD, packName(manifest));
    const stat = fs.statSync(pack);
    const stamp = `${stat.size} ${stat.mtimeMs}`;
    if (packChecked?.stamp !== stamp || packChecked.sha !== manifest.pack.sha256) {
      packChecked = { stamp, sha: manifest.pack.sha256, ok: crypto.createHash("sha256").update(fs.readFileSync(pack)).digest("hex") === manifest.pack.sha256 };
    }
    if (!packChecked.ok) return null;
    return { version: manifest.version, boxes, pack, size: stat.size };
  } catch { return null; }
}

/** A release checked here (its signed manifest and pack, checked already) kept as the one this box
 * holds, unless it holds a newer one: whole or not at all, the one before gone. */
export function keepRelease(boxes: Buffer | string, pack: Buffer): void {
  const partial = `${HELD}.partial`;
  let had = "";
  try { had = fs.readFileSync(path.join(HELD, "boxes.json"), "utf8"); } catch {}
  if (newerVersion(versionIn(had), versionIn(boxes.toString()))) return;
  try {
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
    fs.writeFileSync(path.join(partial, packName(JSON.parse(Buffer.from(JSON.parse(boxes.toString()).manifest, "base64").toString("utf8")))), pack, { mode: 0o600 });
    fs.rmSync(HELD, { recursive: true, force: true });
    fs.renameSync(partial, HELD);
  } catch {
    fs.rmSync(partial, { recursive: true, force: true });
  }
}

// The version a `boxes.json` says (checked elsewhere), or null.
function versionIn(boxes: string): string | null {
  try { return JSON.parse(Buffer.from(JSON.parse(boxes).manifest, "base64").toString("utf8"))?.version ?? null; } catch { return null; }
}

/** `changed` told as soon as the release this box holds changes (another process may have kept it:
 * Kuvo Code here hands its release to this box's server), so its peers hear at once, not at the
 * next minute's round. */
export function watchHeld(changed: () => void): { close: () => void } {
  let was = heldRelease()?.version ?? null;
  let soon: ReturnType<typeof setTimeout> | null = null;
  const look = () => {
    soon = null;
    const now = heldRelease()?.version ?? null;
    if (now === was) return;
    was = now;
    changed();
  };
  // (A moment's gather: the folder's renamed in, in more than one step.)
  const watch = watchFolders([UPDATES], () => { soon ??= setTimeout(look, 50); }, { recursive: false });
  return { close: () => { watch.close(); if (soon) clearTimeout(soon); } };
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
  /** Whole and checked, in `incoming/release`, waiting to be offered. */
  whole?: boolean;
}
let pull: Pull | null = null;
/** Each `<peer> <version>` that couldn't be had from that peer, with when. */
const tried = new Map<string, number>();
/** What the boxes a peer vouches for (another of your Macs, known through a machine both set up)
 * said they hold (`{holds}`), with when: they send no agents mail to this box. */
const heldBy = new Map<string, { version: string; at: number }>();
/** What this box last told each box a peer vouches for that it holds, with when. */
const toldHeld = new Map<string, { version: string; at: number }>();
/** How long a vouched box's word on what it holds counts, and how often it's said again. */
const HELD_WORD = 30 * 60_000;
const SAY_HELD_AGAIN = 10 * 60_000;

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

/** Your own computers this box can mail, beside its peers: the members of the setup it follows, and
 * your other Macs a peer of it names (a Mac one of your machines names as its own set it up, and
 * every Mac that set up one of your computers is yours, #783). Never a box only vouched for
 * otherwise: `release` mail, and the packs, go only between your own computers. */
export function yours(link: LinkState | null | undefined = readLink(), told = readPeerAgents()): Peer[] {
  const peers = peersOf(link);
  const members = new Set(setupView(link).computers.map(each => each.key));
  const others = vouchedOf(link, told).filter(box => members.has(box.key) || (box.kind === "mac" && peers.some(peer => peer.kind === "machine" && told[peer.key]?.boxes?.some(each => each.key === box.key && each.kind === "mac"))));
  return [...peers, ...others.filter(box => !peers.some(peer => peer.key === box.key))];
}
const isYours = (key: string): boolean => yours().some(each => each.key === key);

function tell(peer: string, release: Record<string, unknown>, options: FleetOptions): boolean {
  if (options.send) return options.send(peer, release);
  const link = readLink();
  // (A release's trust is its signature, never who hands it over; but it goes only between your
  // own computers.)
  const to = yours(link).find(each => each.key === peer);
  const bytes = link && to && mailFor(to, { release, at: Date.now() }, link);
  if (!bytes) return false;
  try { return hostOf()?.send(bytes) === true; } catch { return false; }
}

const log = (words: string) => console.error(`Kuvo updates: ${words}`);

/** Whether this box takes no release from a peer now: blocked, an update under way (the release
 * check's too) or on trial here, its engine stopped on purpose. */
function busyHere(blocked: string | null): boolean {
  return !!blocked || isUpdating() || !!offerHeld();
}

/** Whether version `version` isn't to be taken here now: one as new or newer is offered already, or
 * it rests after failing here, or it failed here twice (not again until a newer one, or Update Now). */
function notThis(version: string, now: number): boolean {
  const offered = offeredVersion();
  if (offered && !newerVersion(version, offered)) return true;
  return offerRests(version, now) || offerFailedTwice(version);
}

/** The newest release its peers hold that's newer than this box, and who holds it: the newest
 * version first, then the lowest key, each peer that couldn't hand it over lately left out. */
export function newestHeld(now = Date.now()): { peer: string; version: string } | null {
  return heldAround(now)[0] ?? null;
}

// What your other computers hold that's newer than this box, in the order it's taken: the newest
// version first, then the lowest key, each that couldn't hand it over lately left out.
function heldAround(now: number): { peer: string; version: string }[] {
  const mine = boxVersion();
  if (!mine) return [];
  const told = readPeerAgents();
  const link = readLink();
  const peers = new Set(peersOf(link).map(peer => peer.key));
  const others = yours(link, told).map(box => box.key).filter(key => !peers.has(key) && now - (heldBy.get(key)?.at ?? -Infinity) < HELD_WORD);
  return [
    ...[...peers].map(key => ({ peer: key, version: told[key]?.computer?.release })),
    ...others.map(key => ({ peer: key, version: heldBy.get(key)?.version })),
  ]
    .filter((each): each is { peer: string; version: string } => typeof each.version === "string" && newerVersion(each.version, mine))
    .filter(each => now - (tried.get(`${each.peer} ${each.version}`) ?? -Infinity) >= TRIED_FOR)
    .sort((a, b) => (newerVersion(a.version, b.version) ? -1 : newerVersion(b.version, a.version) ? 1 : a.peer < b.peer ? -1 : a.peer > b.peer ? 1 : 0));
}

/** A look: a peer holds a newer release than this box, and nothing's in the way: it's pulled from
 * that peer (or the pull under way goes on, asking again for what it waits for). As peers' agents
 * mail comes, and each minute. */
export function followPeers(asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  const { clock = Date.now, blocked = updatesBlocked() } = options;
  if (!(options.keys ?? UPDATE_KEYS).length) return;
  const now = clock();
  // (The newest that can be taken here: one resting after it failed here, or that failed twice,
  // gives way to an older one that's still newer than this box.)
  const best = heldAround(now).find(each => !notThis(each.version, now)) ?? null;
  if (pull) {
    // (Caught up meanwhile: it goes.)
    if (!newerVersion(pull.version, boxVersion())) return endPull(null);
    // (An update under way here, the release check's say: the pull waits, kept, until it's over.)
    if (busyHere(blocked)) return;
    // (Whole and checked, waiting to be offered: offered now.)
    if (pull.whole) return finish(pull, options, now);
    // (A computer that couldn't hand it over in time: the next that holds it, or the next look.)
    if (now - pull.progressed >= GIVE_UP) return endPull(`${pull.version} couldn't be had from that computer in time.`, now);
    // (Something newer elsewhere: this one goes, and that one's pulled now.)
    if (!best || !newerVersion(best.version, pull.version)) {
      if (now - pull.heard >= QUIET) ask(pull, options, now, true);
      return;
    }
    endPull(null);
  }
  if (!best || busyHere(blocked)) return;
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
  // Only between your own computers (a peer, or another of your Macs; never a box only vouched for).
  if (!isYours(sender.key)) return;
  // Told what another of your Macs holds: followed as a peer's word is.
  if (typeof given.holds === "string") {
    if (!/^\d+\.\d+\.\d+(-beta\.\d+)?$/.test(given.holds)) return;
    heldBy.set(sender.key, { version: given.holds, at: clock() });
    return followPeers(asked);
  }
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
    // (Never an older engine than the one running: refused before any part is asked.)
    const behind = engineBehind(manifest.engine);
    if (behind) return endPull(`${current.version} isn't taken here: ${behind}`, now);
    if (typeof size !== "number" || !Number.isInteger(size) || size <= 0 || size > MOST_PACK) return endPull(`its ${current.version} isn't a size a release can be.`, now);
    if (notThis(manifest.version, now)) return endPull(null);
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
  // Whole: checked against its signed SHA-256, and moved (never copied) into a folder a release is
  // offered from.
  const whole = path.join(INCOMING, `${current.version}.pack`);
  if (crypto.createHash("sha256").update(fs.readFileSync(whole)).digest("hex") !== current.manifest.pack.sha256) return endPull(`the ${current.version} it sent isn't what its release says.`, now);
  const folder = path.join(INCOMING, "release");
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  writePrivate(path.join(folder, "boxes.json"), current.boxes!);
  fs.renameSync(whole, path.join(folder, packName(current.manifest)));
  current.whole = true;
  finish(current, options, now);
}

// The pack whole and checked: offered to this box as Kuvo Code on a Mac offers the release it
// carries, from a folder of its own (checked again there), and taken. An update under way here (the
// release check's, say) holds it, kept, to be offered at the next look; refused for what it is, it
// goes, and that computer isn't asked for it again for a while.
function finish(current: Pull, options: FleetOptions, now: number): void {
  const folder = path.join(INCOMING, "release");
  const offered = offerUpdate(folder, { keys: options.keys ?? UPDATE_KEYS, blocked: options.blocked === undefined ? updatesBlocked() : options.blocked, clock: options.clock });
  if (offered.status !== 202) {
    if (offered.message === UPDATING_ALREADY || isUpdating()) return;
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

/** What this box holds told to each box a peer vouches for (another of your Macs, which hears no
 * agents mail from it): when it changes, and every ten minutes. Each minute. */
export function tellHeld(asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  const now = (options.clock ?? Date.now)();
  const held = heldRelease(options.keys ?? UPDATE_KEYS);
  if (!held) return;
  const link = readLink();
  const peers = new Set(peersOf(link).map(peer => peer.key));
  for (const box of yours(link).filter(each => !peers.has(each.key))) {
    const last = toldHeld.get(box.key);
    if (last?.version === held.version && now - last.at < SAY_HELD_AGAIN) continue;
    if (tell(box.key, { holds: held.version }, options)) toldHeld.set(box.key, { version: held.version, at: now });
  }
}

/** Forgets what's pulled and tried, for a test. */
export function forgetFleet(): void {
  pull = null;
  tried.clear();
  heldBy.clear();
  toldHeld.clear();
  fs.rmSync(INCOMING, { recursive: true, force: true });
}

// (Nothing else kept in `updates/incoming` outlives a pull: a box that starts again begins anew.)
export function clearIncoming(): void {
  if (listOf(INCOMING).length) fs.rmSync(INCOMING, { recursive: true, force: true });
}
