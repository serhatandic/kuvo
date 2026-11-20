// An accepted setup release moves over the existing authenticated peer mail in signed parts.
// Older setups retain their release announcements until they adopt accepted setup saves.

import { releaseChoice, releaseSetup } from "./choice";
import { checkpointTransfer, STUCK_AFTER, transferAttempt, type TransferStep } from "./attempt";
import { appendUpdateEvent } from "./journal";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { listOf, writePrivate } from "../files";
import { REAL_BOX, UPDATES } from "../home";
import { watchFolders } from "../follow";
import { readPeerAgents } from "../peers/directory";
import { mailFor, peersOf } from "../peers/known";
import { followedSetup } from "../setup/record";
import type { LinkState } from "../state";
import type { LinkHost } from "../relay/host";
import { readLink } from "../state";
import {
  type BoxRelease, UPDATE_KEYS, UPDATING_ALREADY, boxVersion, engineBehind, installedPack, needsRelease, isUpdating, newerVersion, offerFailedTwice, offerHeld, offerRests, offerUpdate, offeredPack,
  noteUpdate, offeredVersion, packName, updateState, startOfferedUpdate, updateOnTrial, updatesBlocked, verifyBoxes,
} from "./update";

/** Where the newest signed release this box holds is kept: `boxes.json` and its pack, by the name
 * its address ends in (a folder a release can be offered or taken from as it is). */
export const HELD = path.join(UPDATES, "release");
/** The signed release this box runs, kept apart once a newer one is held in its place (`HELD`), for
 * repairing its files from (`repair.ts`): whichever of the two is its version. */
export const RUNNING = path.join(UPDATES, "running");
export const PREVIOUS_CHOICE = path.join(UPDATES, "previous-choice");
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
const GIVE_UP = STUCK_AFTER;
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
  sha256: string;
  size: number;
}

/** The pack held, checked against its signed SHA-256 once for each file there (its size and time). */
const packChecked = new Map<string, { stamp: string; sha: string; ok: boolean }>();

export function heldRelease(keys: string[] = ownWays().keys ?? UPDATE_KEYS, folder = HELD): Held | null {
  try {
    const boxes = fs.readFileSync(path.join(folder, "boxes.json"), "utf8");
    const manifest = verifyBoxes(boxes, keys);
    // Only a release signed to go to your other computers (`spreads`): never a build made for
    // trying an update, which would take its version's place on every computer.
    if ((manifest as { spreads?: unknown }).spreads !== true) return null;
    const pack = path.join(folder, packName(manifest));
    const stat = fs.statSync(pack);
    const stamp = `${pack} ${stat.ino} ${stat.size} ${stat.mtimeMs}`;
    let checked = packChecked.get(folder);
    if (checked?.stamp !== stamp || checked.sha !== manifest.pack.sha256) {
      checked = { stamp, sha: manifest.pack.sha256, ok: crypto.createHash("sha256").update(fs.readFileSync(pack)).digest("hex") === manifest.pack.sha256 };
      packChecked.set(folder, checked);
    }
    if (!checked.ok) return null;
    return { version: manifest.version, boxes, pack, sha256: manifest.pack.sha256, size: stat.size };
  } catch { return null; }
}

/** A known pack is repaired by hash. A managed box without a receipt lets the updater establish
 * the identity of the installed build before accepting repairs. Older standalone boxes can use
 * their signed release by version. */
export const repairPackSha = (): string | null | undefined => installedPack() ?? (releaseSetup() ? null : undefined);

/** A sound release of the version this box runs, kept here (held, or apart): one to repair its
 * files from, and to hand to a computer of yours that has none. */
export function runningKept(keys: string[] = ownWays().keys ?? UPDATE_KEYS): Held | null {
  const running = boxVersion(), sha256 = repairPackSha();
  if (!running || sha256 === null) return null;
  for (const folder of [HELD, RUNNING, PREVIOUS_CHOICE]) {
    const kept = heldRelease(keys, folder);
    if (kept?.version === running && (!sha256 || kept.sha256 === sha256)) return kept;
  }
  return null;
}

/** A release checked here (its signed manifest and pack, checked already) kept as the one this box
 * holds, unless it holds a newer one: whole or not at all, the one before gone. */
export function keepRelease(boxes: Buffer | string, pack: Buffer): boolean {
  const partial = `${HELD}.partial`;
  let had = "";
  try { had = fs.readFileSync(path.join(HELD, "boxes.json"), "utf8"); } catch {}
  if (newerVersion(versionIn(had), versionIn(boxes.toString()))) return false;
  try {
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
    fs.writeFileSync(path.join(partial, packName(JSON.parse(Buffer.from(JSON.parse(boxes.toString()).manifest, "base64").toString("utf8")))), pack, { mode: 0o600 });
    const choice = releaseChoice();
    const previous = manifestIn(had);
    const installed = installedPack();
    const replacing = previous?.pack?.sha256 !== manifestIn(boxes.toString())?.pack?.sha256;
    if (previous && previous.version === boxVersion() && (!installed || installed === previous.pack?.sha256) && replacing) {
      fs.rmSync(RUNNING, { recursive: true, force: true });
      fs.renameSync(HELD, RUNNING);
    } else if (choice && previous?.pack?.sha256 === choice.sha256 && replacing) {
      fs.rmSync(PREVIOUS_CHOICE, { recursive: true, force: true });
      fs.renameSync(HELD, PREVIOUS_CHOICE);
    } else fs.rmSync(HELD, { recursive: true, force: true });
    fs.renameSync(partial, HELD);
    return true;
  } catch (error) {
    fs.rmSync(partial, { recursive: true, force: true });
    log(`Couldn't keep the signed release: ${(error as Error).message}`);
    return false;
  }
}

// The manifest a `boxes.json` says. The envelope is verified when taken or handed over.
function manifestIn(boxes: string): BoxRelease | null {
  try { return JSON.parse(Buffer.from(JSON.parse(boxes).manifest, "base64").toString("utf8")); } catch { return null; }
}

const versionIn = (boxes: string) => manifestIn(boxes)?.version;

/** `changed` told as soon as the release this box holds changes (another process may have kept it:
 * Kuvo Code here hands its release to this box's server), so its peers hear at once, not at the
 * next minute's round. */
export function watchHeld(changed: () => void): { close: () => void } {
  let was = heldRelease()?.sha256 ?? null;
  let soon: ReturnType<typeof setTimeout> | null = null;
  const look = () => {
    soon = null;
    const now = heldRelease()?.sha256 ?? null;
    if (now === was) return;
    was = now;
    changed();
  };
  // (A moment's gather: the folder's renamed in, in more than one step.)
  const watch = watchFolders([UPDATES], () => { soon ??= setTimeout(look, 50); }, { recursive: false });
  return { close: () => { watch.close(); if (soon) clearTimeout(soon); } };
}

/** What this box tells its peers of its Kuvo (`computer.kuvo`, `computer.release`). */
export function fleetFacts(): { kuvo?: string; release?: string; installedSha256?: string } {
  const kuvo = boxVersion(), held = heldRelease();
  const setup = releaseSetup();
  const choice = releaseChoice();
  const selected = choice && [HELD, RUNNING, PREVIOUS_CHOICE].some(folder => heldRelease(undefined, folder)?.sha256 === choice.sha256);
  const sha256 = updateOnTrial() ? null : installedPack();
  const published = selected ? choice.version : held && (!setup || held.sha256 === sha256) ? held.version : undefined;
  return { ...(kuvo ? { kuvo } : {}), ...(sha256 ? { installedSha256: sha256 } : {}), ...(published ? { release: published } : {}) };
}

// ---- Pulling a newer one ----

/** The pull under way (one at a time, in the process with the relay connection). */
interface Pull {
  peer: string;
  version: string;
  sha256?: string;
  manifest?: BoxRelease;
  boxes?: string;
  parts?: number;
  got: Set<number>;
  asked: Map<number, number>;
  heard: number;
  progressed: number;
  recorded?: number;
  /** Whole and checked, in `incoming/release`, waiting to be offered. */
  whole?: boolean;
  /** The version this box runs, pulled to repair its files from (`repair.ts`), never offered. */
  forRepair?: boolean;
  step: TransferStep;
}
let pull: Pull | null = null;
/** Each `<peer> <version>` that couldn't be had from that peer, with when. */
const tried = new Map<string, number>();
const tryKey = (peer: string, version: string, sha256?: string) => `${peer} ${sha256 ?? ""} ${version}`;
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
/** The keys a held release is checked with: Kuvo Code's own (a test's in a test's folder). */
export const heldKeys = (): string[] => ownWays().keys ?? UPDATE_KEYS;
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
  // (A release's trust is its signature, never who hands it over; but it goes only between your
  // own computers.)
  const to = peersOf(link).find(each => each.key === peer);
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
  const choice = releaseChoice();
  if (choice ? offered === version && offeredPack() === choice.sha256 : offered && !newerVersion(version, offered)) return true;
  return offerRests(version, now, choice?.sha256) || (!releaseSetup() && offerFailedTwice(version));
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
  const peers = peersOf(link);
  const choice = releaseChoice();
  const previous = transferAttempt();
  return peers.filter(peer => releaseAuthority(peer.key, link)).map(({ key }) => ({
    peer: key,
    version: choice?.version ?? told[key]?.computer?.release,
    holds: choice?.version === told[key]?.computer?.release,
  }))
    .filter((each): each is { peer: string; version: string; holds: boolean } => typeof each.version === "string" && (choice ? needsRelease(choice, mine) : newerVersion(each.version, mine)))
    .filter(each => now >= (tried.get(tryKey(each.peer, each.version, choice?.sha256)) ?? 0)
      && !(previous?.version === each.version && previous.sha256 === choice?.sha256 && previous.peer === each.peer && now < (previous.retryAt ?? 0)))
    .sort((a, b) => Number(b.holds) - Number(a.holds) || (newerVersion(a.version, b.version) ? -1 : newerVersion(b.version, a.version) ? 1 : (a.peer < b.peer ? -1 : a.peer > b.peer ? 1 : 0)));
}

function releaseAuthority(peer: string, link = readLink()): boolean {
  const setup = followedSetup(link);
  if (releaseSetup() && !releaseChoice()) return false;
  return !setup?.managed || peer === setup.primary || peer === setup.standby?.key;
}

function stillChosen(current: Pull): boolean {
  if (current.forRepair) return true;
  const choice = releaseChoice();
  return releaseAuthority(current.peer) && (!choice || (choice.version === current.version && current.sha256 === choice.sha256 &&
    (!current.manifest || current.manifest.pack.sha256 === choice.sha256)));
}

function transferStep(current: Pull, step: TransferStep, now: number): void {
  current.step = step;
  if (!current.forRepair) checkpointTransfer(current.version, current.peer, step, now, undefined, current.sha256);
}

/** No sound release of the version this box runs kept here (an older updater kept none, or it was
 * damaged, or a newer one is held in its place): pulled from a computer of yours that holds it, to
 * repair its files from, never offered. Whatever blocks updates here doesn't block this; an update
 * under way does. */
function repairPull(options: FleetOptions, now: number): void {
  const mine = boxVersion(), sha256 = repairPackSha();
  if (pull || !mine || sha256 === null || isUpdating() || runningKept(options.keys)) return;
  const told = readPeerAgents();
  const peers = peersOf(readLink());
  const holder = peers.map(({ key }) => ({ peer: key, version: told[key]?.computer?.release }))
    .filter(each => each.version === mine && now >= (tried.get(tryKey(each.peer, mine, sha256)) ?? 0))
    .sort((a, b) => (a.peer < b.peer ? -1 : a.peer > b.peer ? 1 : 0))[0];
  if (!holder) return;
  pull = { peer: holder.peer, version: mine, sha256, got: new Set(), asked: new Map(), heard: now, progressed: now, step: "requesting manifest", forRepair: true };
  fs.rmSync(INCOMING, { recursive: true, force: true });
  ask(pull, options, now, false);
}

/** A look: a peer holds a newer release than this box, and nothing's in the way: it's pulled from
 * that peer (or the pull under way goes on, asking again for what it waits for). As peers' agents
 * mail comes, and each minute. */
export function followPeers(asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  try { followRelease(options); } catch (error) {
    const reason = `Couldn't prepare the release transfer: ${(error as Error).message}`;
    if (pull) endPull(reason, (options.clock ?? Date.now)());
    else log(reason);
  }
}

function followRelease(options: FleetOptions): void {
  const { clock = Date.now, blocked = updatesBlocked() } = options;
  if (!(options.keys ?? UPDATE_KEYS).length) return;
  const now = clock();
  // (The newest that can be taken here: one resting after it failed here, or that failed twice,
  // gives way to an older one that's still newer than this box.)
  const best = heldAround(now).find(each => !notThis(each.version, now)) ?? null;
  if (pull?.forRepair) {
    // (Another version here meanwhile, or a sound one of its own: it goes.)
    if (pull.version !== boxVersion() || repairPackSha() !== pull.sha256 || (!pull.whole && runningKept(options.keys))) return endPull(null);
    if (pull.whole) return finish(pull, options, now);
    if (now - pull.progressed >= GIVE_UP) return endPull(timeoutReason(pull), now);
    // (A newer one to take after all: that goes first.)
    if (!best || busyHere(blocked)) {
      if (now - pull.heard >= QUIET) ask(pull, options, now, true);
      return;
    }
    endPull(null);
  }
  if (pull && !stillChosen(pull)) endPull("The setup changed its selected release during the transfer.", now);
  if (pull) {
    // (Caught up meanwhile: it goes.)
    if (!(pull.sha256 ? needsRelease({ version: pull.version, sha256: pull.sha256 }) : newerVersion(pull.version, boxVersion()))) return endPull(null);
    // (An update under way here, the release check's say: the pull waits, kept, until it's over.)
    if (busyHere(blocked)) return;
    // (Whole and checked, waiting to be offered: offered now.)
    if (pull.whole) return finish(pull, options, now);
    // (A computer that couldn't hand it over in time: the next that holds it, or the next look.)
    if (now - pull.progressed >= GIVE_UP) return endPull(timeoutReason(pull), now);
    // (Something newer elsewhere: this one goes, and that one's pulled now.)
    if (!best || !newerVersion(best.version, pull.version)) {
      if (now - pull.heard >= QUIET) ask(pull, options, now, true);
      return;
    }
    endPull(null);
  }
  const choice = releaseChoice();
  if (busyHere(blocked)) return repairPull(options, now);
  if (choice && needsRelease(choice) && !notThis(choice.version, now)) {
    const folder = [HELD, RUNNING, PREVIOUS_CHOICE].find(folder => heldRelease(options.keys, folder)?.sha256 === choice.sha256);
    if (folder) {
      offerHeldRelease(folder, choice.version, choice.sha256, options, now);
      return;
    }
  }
  if (!best) return repairPull(options, now);
  pull = { peer: best.peer, version: best.version, sha256: choice?.sha256, got: new Set(), asked: new Map(), heard: now, progressed: now, step: "requesting manifest" };
  transferStep(pull, "requesting manifest", now);
  fs.rmSync(INCOMING, { recursive: true, force: true });
  ask(pull, options, now, false);
}

// What the pull asks for next: the release's manifest, then its parts, `WINDOW` at once; `again`:
// what was asked and not answered is asked again.
function ask(current: Pull, options: FleetOptions, now: number, again: boolean): void {
  if (again) current.heard = now;
  if (!current.manifest) {
    if (!tell(current.peer, { want: current.version, ...(current.sha256 ? { sha256: current.sha256 } : {}) }, options)) endPull("Couldn’t send the release request over the relay.", now);
    return;
  }
  if (again) current.asked.clear();
  for (let i = 0; i < current.parts! && current.asked.size < WINDOW; i++) {
    if (current.got.has(i) || current.asked.has(i)) continue;
    if (!tell(current.peer, { version: current.version, part: i, ...(current.sha256 ? { sha256: current.sha256 } : {}) }, options))
      return endPull("The relay connection closed while requesting release parts.", now);
    current.asked.set(i, now);
  }
}

function timeoutReason(current: Pull): string {
  return current.manifest ? "The other computer stopped sending release parts."
    : "The other computer didn’t answer the request for the signed release.";
}

function endPull(why: string | null, now = Date.now()): void {
  if (why && pull) {
    const retry = pull.forRepair ? now + TRIED_FOR : checkpointTransfer(pull.version, pull.peer, pull.step, now, why, pull.sha256).retryAt!;
    tried.set(tryKey(pull.peer, pull.version, pull.sha256), retry);
    log(`${pull.version}: ${why}`);
  }
  pull = null;
  try { fs.rmSync(INCOMING, { recursive: true, force: true }); } catch (error) {
    log(`Couldn't clear the incoming release: ${(error as Error).message}`);
  }
}

/** Release mail from a peer (`sender`, one of this box's own computers): asked for the release it
 * holds, or for a part of it, it answers; told the manifest or a part of the one it pulls, it takes
 * it in, and once the pack is whole and what it says, offers it to itself. */
export function takeReleaseMail(sender: { key: string }, given: Record<string, unknown>, asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  try { exchangeRelease(sender, given, options); } catch (error) {
    const reason = `Couldn't handle release mail: ${(error as Error).message}`;
    if (pull?.peer === sender.key && pull.version === given.version) endPull(reason, (options.clock ?? Date.now)());
    else log(reason);
  }
}

function exchangeRelease(sender: { key: string }, given: Record<string, unknown>, options: FleetOptions): void {
  const { keys = UPDATE_KEYS, clock = Date.now } = options;
  if (!peersOf(readLink()).some(peer => peer.key === sender.key)) return;
  // Asked: what this box holds, if it's that version.
  // (The one it holds, or the one it runs, kept apart: a computer with none of its own repairs from it.)
  const holding = (version: unknown): Held | null => {
    const selected = releaseChoice();
    const sha256 = typeof given.sha256 === "string" ? given.sha256 : selected?.version === version ? selected?.sha256 : releaseSetup() ? installedPack() : undefined;
    if (sha256 === null) return null;
    for (const folder of [HELD, RUNNING, PREVIOUS_CHOICE]) {
      const held = heldRelease(keys, folder);
      if (held?.version === version && (!sha256 || held?.sha256 === sha256)) return held;
    }
    return null;
  };
  if (typeof given.want === "string") {
    const held = holding(given.want);
    tell(sender.key, held && held.version === given.want ? { version: held.version, boxes: held.boxes, size: held.size } : { version: given.want, none: true }, options);
    return;
  }
  if (typeof given.version !== "string") return;
  if (Number.isInteger(given.part) && given.d === undefined) {
    const held = holding(given.version);
    const i = given.part as number;
    if (!held || held.version !== given.version || i < 0 || i * PART >= held.size) return;
    const bytes = Buffer.alloc(Math.min(PART, held.size - i * PART));
    const handle = fs.openSync(held.pack, "r");
    try { fs.readSync(handle, bytes, 0, bytes.length, i * PART); } finally { fs.closeSync(handle); }
    tell(sender.key, { version: held.version, i, d: bytes.toString("base64"), ...(given.sha256 ? { sha256: held.sha256 } : {}) }, options);
    return;
  }
  // Answers: only from the peer pulled from, for the version pulled.
  const current = pull;
  if (!current || current.peer !== sender.key || current.version !== given.version) return;
  const now = clock();
  if (!stillChosen(current)) return endPull("The setup changed its selected release during the transfer.", now);
  if (given.none === true) return endPull(`that computer no longer holds ${current.version}.`, now);
  if (typeof given.boxes === "string" && !current.manifest) {
    let manifest: BoxRelease;
    try {
      manifest = verifyBoxes(given.boxes, keys);
    } catch (error) {
      return endPull(`what it sent for ${current.version} isn't a release this computer takes: ${(error as Error).message}`, now);
    }
    const choice = releaseChoice();
    if ((current.sha256 && current.sha256 !== manifest.pack.sha256)
      || (!current.forRepair && (manifest.spreads !== true || (choice && choice.sha256 !== manifest.pack.sha256))))
      return endPull("That computer sent a different release from the one selected.", now);
    const size = given.size;
    // (Exactly what was asked for, newer than this box (or, to repair from, the version it runs),
    // and a pack's size a box takes.)
    const fits = current.forRepair ? manifest.version === boxVersion() : current.sha256 ? needsRelease({ version: manifest.version, sha256: current.sha256 }) : newerVersion(manifest.version, boxVersion());
    if (manifest.version !== current.version || !fits) return endPull(`what it sent isn't ${current.version}.`, now);
    // (Never an older engine than the one running: refused before any part is asked. One to repair
    // from is never taken, so its engine is never started.)
    const behind = current.forRepair ? null : engineBehind(manifest.engine);
    if (behind) return endPull(`${current.version} isn't taken here: ${behind}`, now);
    if (typeof size !== "number" || !Number.isInteger(size) || size <= 0 || size > MOST_PACK) return endPull(`its ${current.version} isn't a size a release can be.`, now);
    if (!current.forRepair && notThis(manifest.version, now)) return endPull(null);
    fs.mkdirSync(INCOMING, { recursive: true, mode: 0o700 });
    writePrivate(path.join(INCOMING, `${current.version}.json`), given.boxes);
    fs.writeFileSync(path.join(INCOMING, `${current.version}.pack`), Buffer.alloc(0), { mode: 0o600 });
    fs.truncateSync(path.join(INCOMING, `${current.version}.pack`), size);
    current.manifest = manifest;
    current.boxes = given.boxes;
    current.parts = Math.ceil(size / PART);
    current.heard = current.progressed = now;
    transferStep(current, "receiving pack", now);
    return ask(current, options, now, false);
  }
  if (!current.manifest || !Number.isInteger(given.i) || typeof given.d !== "string"
    || (typeof given.sha256 === "string" && given.sha256 !== current.manifest.pack.sha256)) return;
  const i = given.i as number;
  if (i < 0 || i >= current.parts! || current.got.has(i)) return;
  const bytes = Buffer.from(given.d, "base64");
  const size = fs.statSync(path.join(INCOMING, `${current.version}.pack`)).size;
  if (bytes.length !== Math.min(PART, size - i * PART)) return endPull(`a part of ${current.version} it sent isn't the size it said.`, now);
  const handle = fs.openSync(path.join(INCOMING, `${current.version}.pack`), "r+");
  try { fs.writeSync(handle, bytes, 0, bytes.length, i * PART); } finally { fs.closeSync(handle); }
  current.got.add(i);
  current.asked.delete(i);
  current.heard = current.progressed = now;
  if (current.got.size % WINDOW === 0 || now - (current.recorded ?? 0) >= 5_000) {
    current.recorded = now;
    transferStep(current, "receiving pack", now);
  }
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
  // One to repair from: kept apart as the release this box runs (whole and checked already), never
  // offered; its files are put back at the keeper's next look (`repair.ts`).
  if (current.forRepair) {
    if (current.version === boxVersion() && repairPackSha() === current.sha256) {
      fs.rmSync(RUNNING, { recursive: true, force: true });
      fs.renameSync(folder, RUNNING);
      log(`${current.version}, the version here, came from another of your computers, to repair from.`);
    }
    pull = null;
    fs.rmSync(INCOMING, { recursive: true, force: true });
    return;
  }
  if (!stillChosen(current)) return endPull("The setup changed its selected release before the pack was offered.", now);
  transferStep(current, "offering pack", now);
  const offered = offerUpdate(folder, { keys: options.keys ?? UPDATE_KEYS, blocked: options.blocked === undefined ? updatesBlocked() : options.blocked, clock: options.clock });
  if (offered.status !== 202) {
    if (offered.message === UPDATING_ALREADY || isUpdating()) return;
    endPull(`${current.version} couldn't be offered here: ${offered.message ?? offered.status}`, now);
    return;
  }
  transferStep(current, "offer accepted", now);
  log(`${current.version} came from another of your computers; it’s ready to install.`);
  pull = null;
  fs.rmSync(INCOMING, { recursive: true, force: true });
  startOffer(options, current.version);
}

function offerHeldRelease(folder: string, version: string, sha256: string, options: FleetOptions, now: number): void {
  const previous = transferAttempt();
  if (previous?.version === version && previous.sha256 === sha256 && previous.peer === "here" && now < (previous.retryAt ?? 0)) return;
  checkpointTransfer(version, "here", "offering pack", now, undefined, sha256);
  const offered = offerUpdate(folder, { keys: options.keys, blocked: options.blocked, clock: options.clock });
  if (offered.status !== 202) {
    checkpointTransfer(version, "here", "offering pack", now, offered.message ?? "The updater refused the local release.", sha256);
    return;
  }
  checkpointTransfer(version, "here", "offer accepted", now, undefined, sha256);
  startOffer(options, version);
}

function startOffer(options: FleetOptions, version: string, manual = false): void {
  if (options.offered) return options.offered();
  startOfferedUpdate(Date.now, manual).catch(error => appendUpdateEvent(version, "staging", `Couldn't start the updater: ${(error as Error).message}`));
}

export function retryTransfer(version: string): void {
  const failed = updateState().offerFailed;
  const sha256 = releaseChoice()?.sha256;
  if (failed?.version === version && (!failed.sha256 || failed.sha256 === sha256)) noteUpdate({ offerFailed: { ...failed, retryNow: true } });
  const now = (ownWays().clock ?? Date.now)();
  if (pull && !pull.forRepair) endPull("The client requested another attempt.", now);
  for (const key of tried.keys()) if (key.endsWith(` ${version}`)) tried.delete(key);
  const previous = transferAttempt();
  if (previous?.version === version && previous.sha256 === sha256) checkpointTransfer(version, previous.peer, previous.step, now, undefined, sha256);
  followPeers();
  if (offeredVersion() === version) startOffer(ownWays(), version, true);
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
  const attempt = transferAttempt();
  if (attempt && !attempt.reason && attempt.step !== "offer accepted")
    checkpointTransfer(attempt.version, attempt.peer, attempt.step, Date.now(), `Kuvo restarted while ${attempt.step}.`, attempt.sha256);
  if (listOf(INCOMING).length) fs.rmSync(INCOMING, { recursive: true, force: true });
}
