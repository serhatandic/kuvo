// Peer announcements select a release. Signed manifests supply its URL and identity.
import { releaseChoice } from "./choice";
import { cannotFor, readyFact, recordCannot, runningVersion, sameRelease, type ReleaseRef } from "./facts";
import { fetchedPatiently } from "../engine/client";
import { peerIsAway, readReach } from "../peers/reach";
import { isObject, parsed } from "../../shared/json";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { locked, textOf, writePrivate } from "../files";
import { REAL_BOX, RUN, UPDATES } from "../home";
import { watchFolders } from "../follow";
import { releaseDisk } from "../server/disk-worker";
import { keepsList } from "../server/resident-state";
import { readPeerAgents } from "../peers/directory";
import { mailFor, peersOf } from "../peers/known";
import { requestFleetRetry } from "./progress";
import { followedSetup } from "../setup/record";
import type { LinkState } from "../state";
import type { LinkHost } from "../relay/host";
import { readLink } from "../state";
import {
  type BoxRelease, UPDATE_KEYS, boxVersion, installedPack, isUpdating, newerVersion,
  localPack, namesPack, packName, releaseDrainDeferred, startUpdate, switchPending, updateOnTrial, updatesBlocked, verifyBoxes,
} from "./update";

/** The newest shareable release held here. */
export const HELD = path.join(UPDATES, "release");
/** The signed release this box runs, kept apart once a newer one is held in its place (`HELD`), for
 * repairing its files from (`repair.ts`): whichever of the two is its version. */
export const RUNNING = path.join(UPDATES, "running");
export const PREVIOUS_CHOICE = path.join(UPDATES, "previous-choice");
/** Where a pack received in parts is assembled. */
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

/** The newest signed release this box holds: its version, its signed manifest as it came
 * (`boxes.json`'s text), its pack's file and size. Held from the moment it's checked here (Kuvo Code
 * here handed it over, the box fetched it, or a peer did), before or while it's taken, and kept
 * while the fleet prepares and switches. */
export interface Held {
  version: string;
  boxes: string;
  pack: string;
  sha256: string;
  size: number;
}

/** The pack held, checked against its signed SHA-256 once for each file there (its size and time). */
const packChecked = new Map<string, { stamp: string; sha: string; ok: boolean }>();

const packChecking = new Map<string, string>();
const packVerified = new Set<() => void>();
const packStamp = (pack: string, stat: fs.Stats) => `${pack} ${stat.ino} ${stat.size} ${stat.mtimeMs} ${stat.ctimeMs}`;

export async function verifyHeldPack(pack: string, stamp: string, sha: string): Promise<boolean | null> {
  const hash = crypto.createHash("sha256");
  try {
    const before = await fs.promises.stat(pack);
    if (packStamp(pack, before) !== stamp) return null;
    if (!before.isFile() || before.size > MOST_PACK) return false;
    for await (const chunk of fs.createReadStream(pack)) hash.update(chunk);
    return packStamp(pack, await fs.promises.stat(pack)) === stamp ? hash.digest("hex") === sha : null;
  } catch { return null; }
}

function verifyPackLater(folder: string, pack: string, stamp: string, sha: string): void {
  const key = JSON.stringify([stamp, sha]);
  if (packChecking.get(folder) === key) return;
  packChecking.set(folder, key);
  void releaseDisk.run("verifyHeldPack", pack, stamp, sha).then(ok => {
    if (ok === null || packChecking.get(folder) !== key) return;
    packChecked.set(folder, { stamp, sha, ok });
    for (const tell of packVerified) tell();
  }).catch(() => {}).finally(() => {
    if (packChecking.get(folder) === key) packChecking.delete(folder);
  });
}

export function heldRelease(keys: string[] = ownWays().keys ?? UPDATE_KEYS, folder = HELD): Held | null {
  try {
    const boxes = fs.readFileSync(path.join(folder, "boxes.json"), "utf8");
    const manifest = verifyBoxes(boxes, keys);
    // Only a release signed to go to your other computers (`spreads`): never a build made for
    // trying an update, which would take its version's place on every computer.
    if ((manifest as { spreads?: unknown }).spreads !== true) return null;
    const pack = path.join(folder, packName(manifest));
    const stat = fs.statSync(pack);
    const stamp = packStamp(pack, stat);
    let checked = packChecked.get(folder);
    if (checked?.stamp !== stamp || checked.sha !== manifest.pack.sha256) {
      if (keepsList()) {
        verifyPackLater(folder, pack, stamp, manifest.pack.sha256);
        return null;
      }
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
export const repairPackSha = (): string | null | undefined => installedPack() ?? (releaseChoice() ? null : undefined);

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

/** Keep the newest candidate and preserve the pack for the version in use. A member can
 * also receive its main's release while holding a newer release from its own app. */
export function keepRelease(boxes: Buffer | string, pack: Buffer): boolean {
  const manifest = manifestIn(boxes.toString())!;
  fs.mkdirSync(UPDATES, { recursive: true, mode: 0o700 });
  return locked(path.join(RUN, ".held-release.lock"), () => {
    const read = (folder: string) => manifestIn(textOf(path.join(folder, "boxes.json")) ?? "");
    const previous = read(HELD);
    const folder = newerVersion(previous?.version, manifest.version)
      ? (manifest.version === boxVersion() ? RUNNING : PREVIOUS_CHOICE) : HELD;
    if (heldRelease(heldKeys(), folder)?.sha256 === manifest.pack.sha256) return true;
    const partial = `${folder}.partial`;
    try {
      fs.rmSync(partial, { recursive: true, force: true });
      fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
      fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
      fs.writeFileSync(path.join(partial, packName(manifest)), pack, { mode: 0o600 });
      if (folder === HELD && previous?.version === boxVersion() && previous?.pack.sha256 === installedPack()) {
        fs.rmSync(RUNNING, { recursive: true, force: true });
        fs.renameSync(HELD, RUNNING);
      } else if (folder === HELD && previous && sameRelease(readyFact(), { version: previous.version, sha256: previous.pack.sha256 })) {
        fs.rmSync(PREVIOUS_CHOICE, { recursive: true, force: true });
        fs.renameSync(HELD, PREVIOUS_CHOICE);
      } else fs.rmSync(folder, { recursive: true, force: true });
      fs.renameSync(partial, folder);
      return true;
    } catch (error) {
      fs.rmSync(partial, { recursive: true, force: true });
      log(`Couldn't keep the signed release: ${(error as Error).message}`);
      return false;
    }
  }, { exclusive: true });
}

// The envelope is verified before use.
function manifestIn(boxes: string): BoxRelease | null {
  try { return localPack(JSON.parse(Buffer.from(JSON.parse(boxes).manifest, "base64").toString("utf8"))); } catch { return null; }
}

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
  const schedule = () => { soon ??= setTimeout(look, 50); };
  const watch = watchFolders([UPDATES], schedule, { recursive: false });
  packVerified.add(schedule);
  return { close: () => { packVerified.delete(schedule); watch.close(); if (soon) clearTimeout(soon); } };
}

export function isUpdateMain(): boolean {
  return followedSetup(readLink())?.role !== "member";
}

export interface FleetTarget { version: string; sha256?: string }
interface Candidate extends FleetTarget { peer?: string; boxes?: string }
const heldFolders = [HELD, RUNNING, PREVIOUS_CHOICE];

function matchingPack(target: FleetTarget, keys = heldKeys()): Held | undefined {
  return heldFolders.map(folder => heldRelease(keys, folder)).find(kept =>
    kept?.version === target.version && (!target.sha256 || kept.sha256 === target.sha256)) ?? undefined;
}

function signedCandidate(version: string, peer: string, boxes?: string, sha256?: string): Candidate {
  if (boxes) try {
    const manifest = verifyBoxes(boxes, heldKeys());
    if (manifest.spreads && manifest.version === version && (!sha256 || namesPack(manifest, sha256)))
      return { version, peer, boxes, sha256: manifest.pack.sha256 };
  } catch {}
  return { version, peer, sha256 };
}

function candidates(now: number): Candidate[] {
  const link = readLink(), setup = followedSetup(link);
  const told = readPeerAgents();
  if (setup?.role === "member") {
    const main = told[setup.primary]?.computer;
    const result: Candidate[] = [];
    if (main?.kuvo) {
      // Catch up to the committed version first, even if the main already has another candidate.
      const legacy = releaseChoice();
      result.push(signedCandidate(main.kuvo, setup.primary, main.runningManifest, main.installedSha256
        ?? (legacy?.version === main.kuvo ? legacy.sha256 : undefined)));
    }
    const announced = main?.candidate ?? main?.release;
    if (announced && newerVersion(announced, main?.kuvo))
      result.push(signedCandidate(announced, setup.primary, main?.releaseManifest));
    return result;
  }
  const held = heldRelease();
  const result: Candidate[] = held ? [{ version: held.version, sha256: held.sha256 }] : [];
  const reach = readReach();
  for (const peer of peersOf(link)) {
    const said = told[peer.key];
    const computer = said?.computer;
    if (!computer || peerIsAway(peer.key, said.heard, reach, now)) continue;
    if (computer.kuvo) result.push(signedCandidate(computer.kuvo, peer.key, computer.runningManifest, computer.installedSha256));
    const announced = computer.candidate ?? computer.release;
    if (announced && announced !== computer.kuvo)
      result.push(signedCandidate(announced, peer.key, computer.releaseManifest));
  }
  const newestFirst = (a: FleetTarget, b: FleetTarget) => newerVersion(a.version, b.version) ? -1 : newerVersion(b.version, a.version) ? 1 : 0;
  return result.sort(newestFirst);
}

function nextCandidate(now: number): Candidate | undefined {
  return candidates(now).find(candidate => newerVersion(candidate.version, runningVersion(boxVersion())));
}

export function fleetTarget(now = Date.now()): FleetTarget | undefined {
  const next = nextCandidate(now);
  if (next) {
    const kept = matchingPack(next);
    return { version: next.version, sha256: next.sha256 ?? kept?.sha256 };
  }
  const setup = followedSetup(readLink());
  if (setup?.role === "member") {
    const primary = readPeerAgents()[setup.primary]?.computer;
    return primary?.kuvo ? { version: primary.kuvo, sha256: primary.installedSha256 } : undefined;
  }
  const version = runningVersion(boxVersion());
  return version ? { version, sha256: switchPending() ? undefined : installedPack() ?? undefined } : undefined;
}

export function fleetFacts(): Pick<import("../../shared/records").ToldComputer,
  "kuvo" | "release" | "candidate" | "installedSha256" | "releaseManifest" | "runningManifest" | "staged" | "cannot"> {
  const pending = switchPending() || updateOnTrial();
  const kuvo = runningVersion(boxVersion()) ?? undefined;
  const running = pending ? undefined : runningKept();
  const held = heldRelease();
  const target = fleetTarget();
  const ref = target?.sha256 ? { version: target.version, sha256: target.sha256 } : undefined;
  const cannot = ref && cannotFor(ref);
  const staged = readyFact();
  const published = isUpdateMain()
    ? target && newerVersion(target.version, kuvo) ? matchingPack(target) : undefined
    : held && newerVersion(held.version, kuvo) ? held : undefined;
  return {
    kuvo, installedSha256: pending ? undefined : installedPack() ?? undefined,
    release: running?.version, candidate: published?.version, releaseManifest: published?.boxes,
    runningManifest: running?.boxes,
    cannot, staged: !cannot && sameRelease(staged, ref) ? staged : undefined,
  };
}

/** Count from the first ready computer. Off computers are excluded, and a slow one
 * never extends the deadline. The main must have a checked bin before it can switch. */
export const STAGE_WAIT = 5 * 60_000;
export const STAGE_CLOCK = path.join(RUN, "fleet-stage-clock.json");

function waitingSince(ref: ReleaseRef, now: number): number {
  const kept = parsed(textOf(STAGE_CLOCK));
  if (isObject(kept) && kept.version === ref.version && kept.sha256 === ref.sha256 && typeof kept.at === "number") return kept.at;
  writePrivate(STAGE_CLOCK, JSON.stringify({ ...ref, at: now }));
  return now;
}

function mainCanSwitch(ref: ReleaseRef, now: number): boolean {
  const link = readLink(), told = readPeerAgents(), reach = readReach();
  // A detached Mac updater reads the saved roster without the relay process's Keychain overlay.
  const self = link?.setup?.role === "primary" ? link.setup.primary : undefined;
  const peers = peersOf(link).filter(peer => peer.key !== self && !peerIsAway(peer.key, told[peer.key]?.heard, reach, now));
  const ready = peers.map(peer => {
    const facts = told[peer.key]?.computer;
    return !!facts?.kuvo && !newerVersion(ref.version, facts.kuvo) || sameRelease(facts?.staged, ref);
  });
  const local = sameRelease(readyFact(), ref) && !cannotFor(ref);
  if (!local && !ready.some(Boolean)) return false;
  const since = waitingSince(ref, now);
  return local && (ready.every(Boolean) || now - since >= STAGE_WAIT);
}

export function activationAllowed(ref: ReleaseRef, now = Date.now()): boolean {
  const candidate = nextCandidate(now);
  if (!candidate || candidate.version !== ref.version || candidate.sha256 && candidate.sha256 !== ref.sha256
    || !sameRelease(readyFact(), ref) || cannotFor(ref)) return false;
  const setup = followedSetup(readLink());
  if (setup?.role !== "member") return mainCanSwitch(ref, now);
  const main = readPeerAgents()[setup.primary]?.computer;
  if (main?.kuvo !== ref.version) return false;
  return !main.installedSha256 || signedCandidate(main.kuvo, setup.primary, main.runningManifest, main.installedSha256).sha256 === ref.sha256;
}

// ---- Acquiring a pack ----

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
  downloading?: boolean;
  urlTried?: boolean;
  /** The version this box runs, pulled to repair its files from (`repair.ts`), never offered. */
  forRepair?: boolean;
}
let pull: Pull | null = null;
/** How far this box got pulling a release, by its version, 0..1 (`got / parts`): only ever up, so
 * a pull started again (from another computer, after Try Again) never steps its row back. In the
 * process that pulls; a new process starts without it. */
let pulled: { version: string; fill: number } | null = null;

function notePulled(current: Pull): void {
  if (current.forRepair || !current.parts) return;
  const fill = Math.min(1, current.got.size / current.parts);
  if (pulled?.version !== current.version) pulled = { version: current.version, fill };
  else pulled.fill = Math.max(pulled.fill, fill);
}

/** How far this box is in getting `version` ready, 0..1, where it pulls it part by part from
 * another of your computers; none where it isn't known (a download, or nothing pulled yet). */
export function pullFill(version: string): number | undefined {
  if (pull?.version === version) notePulled(pull);
  return pulled?.version === version ? pulled.fill : undefined;
}
// Fail over to another holder of the same release before reporting a transfer failure.
const tried = new Set<string>();
const tryKey = (peer: string, version: string, sha256?: string) => `${peer} ${sha256 ?? ""} ${version}`;
export interface FleetOptions {
  keys?: string[];
  blocked?: string | null;
  clock?: () => number;
  send?: (peer: string, release: Record<string, unknown>) => boolean;
  start?: typeof startUpdate;
}

let hostOf: () => Pick<LinkHost, "send"> | null | undefined = () => null;
// A test's own ways (its keys only in a test's folder), as the mail it takes uses them.
const fixtureKey = !REAL_BOX ? process.env.KUVO_TEST_RELEASE_KEY : undefined;
let ways: FleetOptions = fixtureKey ? { keys: [fixtureKey] } : {};
const ownWays = (): FleetOptions => ways;
/** The keys a held release is checked with: Kuvo Code's own (a test's in a test's folder). */
export const heldKeys = (): string[] => ownWays().keys ?? UPDATE_KEYS;
/** Allows disposable test homes to use their own signing keys and transport. */
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

export function newestHeld(now = Date.now()): { peer: string; version: string } | null {
  const next = nextCandidate(now);
  return next?.peer ? { peer: next.peer, version: next.version } : null;
}

function stillChosen(current: Pull, now: number): boolean {
  if (current.forRepair) return current.version === boxVersion() && current.sha256 === repairPackSha();
  const next = nextCandidate(now);
  return next?.version === current.version && (!next.sha256 || !current.sha256 || next.sha256 === current.sha256);
}

function repairPull(options: FleetOptions, now: number): void {
  const mine = boxVersion(), sha256 = repairPackSha();
  if (pull || !mine || sha256 === null || runningKept(options.keys)) return;
  const told = readPeerAgents();
  const peer = peersOf(readLink()).find(({ key }) =>
    told[key]?.computer?.release === mine && !tried.has(tryKey(key, mine, sha256)));
  if (peer) beginPull({ peer: peer.key, version: mine, sha256 }, options, now, true);
}

export function followPeers(asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  try { followRelease(options); } catch (error) { log((error as Error).message); }
}

function followRelease(options: FleetOptions): void {
  const { clock = Date.now, blocked = updatesBlocked(), start = startUpdate, keys = UPDATE_KEYS } = options;
  if (!keys.length || blocked || isUpdating() || switchPending() || updateOnTrial()) return;
  const now = clock();
  const next = nextCandidate(now);
  if (pull && (!stillChosen(pull, now) || pull.forRepair && next)) endPull();
  if (next) {
    const kept = matchingPack(next, keys);
    const expected = kept ?? (next.sha256 ? { version: next.version, sha256: next.sha256 } : undefined);
    if (expected && isUpdateMain()) mainCanSwitch(expected, now);
    if (expected && cannotFor(expected)) return;
    if (expected && sameRelease(readyFact(), expected)) {
      if (pull) endPull();
      if (!releaseDrainDeferred(expected.version, now) && activationAllowed(expected, now)) start({ phase: "activate", expected, keys });
      return;
    }
    if (kept) {
      if (pull) endPull();
      start({ phase: "prepare", from: path.dirname(kept.pack), expected: kept, keys });
      return;
    }
  }
  if (pull) {
    if (pull.downloading) return;
    if (now - pull.progressed >= GIVE_UP) return endPull();
    if (now - pull.heard >= QUIET) ask(pull, options, now, true);
    return;
  }
  if (!next) return repairPull(options, now);
  const source = sourcesFor(next, now)[0];
  if (source) beginPull(source, options, now);
}

function sourcesFor(target: Candidate, now: number): (Candidate & { peer: string })[] {
  const told = readPeerAgents(), reach = readReach();
  return peersOf(readLink())
    .filter(peer => [told[peer.key]?.computer?.candidate, told[peer.key]?.computer?.release, told[peer.key]?.computer?.kuvo].includes(target.version))
    .map(peer => ({ version: target.version, sha256: target.sha256,
      peer: peer.key, boxes: peer.key === target.peer ? target.boxes : undefined }))
    .filter(source => (source.boxes || !peerIsAway(source.peer, told[source.peer]?.heard, reach, now))
      && !tried.has(tryKey(source.peer, source.version, source.sha256)))
    .sort((a, b) => Number(b.peer === target.peer) - Number(a.peer === target.peer));
}

function beginPull(source: Candidate & { peer: string }, options: FleetOptions, now: number, forRepair = false): void {
  const current = pull = { peer: source.peer, version: source.version, sha256: source.sha256,
    got: new Set(), asked: new Map(), heard: now, progressed: now, forRepair };
  if (source.boxes) {
    downloadPack(current, source.boxes, options, () => ask(current, options, now, true));
  } else ask(current, options, now, false);
}

function downloadPack(current: Pull, boxes: string, options: FleetOptions, fallback: () => void): void {
  const manifest = verifyBoxes(boxes, options.keys ?? UPDATE_KEYS);
  current.downloading = true;
  current.urlTried = true;
  const fetch = fetchedPatiently(manifest.pack.url, MOST_PACK, 90_000, []);
  void fetch.then(pack => {
    if (pull !== current) return;
    if (crypto.createHash("sha256").update(pack).digest("hex") !== manifest.pack.sha256) throw new Error("Pack checksum mismatch.");
    savePack(current, boxes, pack, options);
  }).catch(() => {
    if (pull !== current) return;
    current.downloading = false;
    fallback();
  });
}

// What the pull asks for next: the release's manifest, then its parts, `WINDOW` at once; `again`:
// what was asked and not answered is asked again.
function ask(current: Pull, options: FleetOptions, now: number, again: boolean): void {
  if (again) current.heard = now;
  if (!current.manifest) {
    if (!tell(current.peer, { want: current.version, ...(current.sha256 ? { sha256: current.sha256 } : {}) }, options)) endPull();
    return;
  }
  if (again) current.asked.clear();
  for (let i = 0; i < current.parts! && current.asked.size < WINDOW; i++) {
    if (current.got.has(i) || current.asked.has(i)) continue;
    if (!tell(current.peer, { version: current.version, part: i, ...(current.sha256 ? { sha256: current.sha256 } : {}) }, options))
      return endPull();
    current.asked.set(i, now);
  }
}

function endPull(): void {
  pull = null;
  fs.rmSync(INCOMING, { recursive: true, force: true });
}

function failPull(current: Pull, options: FleetOptions, reason: string): void {
  if (pull !== current) return;
  tried.add(tryKey(current.peer, current.version, current.sha256));
  log(`${current.version}: ${reason}`);
  endPull();
  const now = (options.clock ?? Date.now)();
  const target = nextCandidate(now);
  if (!current.forRepair && target && current.sha256 && !sourcesFor(target, now).length) {
    try { recordCannot({ version: current.version, sha256: current.sha256 }, reason, true); } catch {}
  }
  followPeers(options);
}

/** Release mail from a peer (`sender`, one of this box's own computers): asked for the release it
 * holds, or for a part of it, it answers; told the manifest or a part of the one it pulls, it takes
 * it in, and keeps it after verifying the pack. */
export function takeReleaseMail(sender: { key: string }, given: Record<string, unknown>, asked: FleetOptions = {}): void {
  const options = { ...ownWays(), ...asked };
  try { exchangeRelease(sender, given, options); } catch (error) {
    const reason = `Couldn't handle release mail: ${(error as Error).message}`;
    if (pull?.peer === sender.key && pull.version === given.version) failPull(pull, options, reason);
    else log(reason);
  }
}

function exchangeRelease(sender: { key: string }, given: Record<string, unknown>, options: FleetOptions): void {
  const { keys = UPDATE_KEYS, clock = Date.now } = options;
  if (!peersOf(readLink()).some(peer => peer.key === sender.key)) return;
  // Try Again for this computer, passed on by the box it was asked of.
  if (isObject(given.retry)) {
    const { version, sha256 } = given.retry;
    if (typeof version === "string" && typeof sha256 === "string") requestFleetRetry(version, sha256);
    return;
  }
  // Asked by a member's Check for Updates: the main looks at the feed now (one look at a time).
  if (given.check === true) {
    const { blocked = updatesBlocked(), start = startUpdate } = options;
    if (!blocked && isUpdateMain()) start({ now: true });
    return;
  }
  // Asked: what this box holds, if it's that version.
  // (The one it holds, or the one it runs, kept apart: a computer with none of its own repairs from it.)
  const holding = (version: unknown): Held | null => {
    const sha256 = typeof given.sha256 === "string" ? given.sha256 : undefined;
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
  if (!stillChosen(current, now)) return endPull();
  if (current.downloading) return;
  const failed = (reason: string) => failPull(current, options, reason);
  if (given.none === true) return failed(`That computer no longer holds ${current.version}.`);
  if (typeof given.boxes === "string" && !current.manifest) {
    let manifest: BoxRelease;
    try { manifest = verifyBoxes(given.boxes, keys); }
    catch { return failed("The other computer did not send a trusted manifest."); }
    if (manifest.version !== current.version || current.sha256 && current.sha256 !== manifest.pack.sha256 || manifest.spreads !== true)
      return failed("The other computer sent a different release.");
    const size = given.size;
    if (typeof size !== "number" || !Number.isInteger(size) || size <= 0 || size > MOST_PACK)
      return failed("The other computer reported an invalid pack size.");
    const boxes = given.boxes;
    const receiveParts = () => {
      fs.mkdirSync(INCOMING, { recursive: true, mode: 0o700 });
      const file = path.join(INCOMING, "pack");
      fs.writeFileSync(file, Buffer.alloc(0), { mode: 0o600 });
      fs.truncateSync(file, size);
      current.manifest = manifest;
      current.boxes = boxes;
      current.parts = Math.ceil(size / PART);
      notePulled(current);
      current.heard = current.progressed = clock();
      ask(current, options, current.heard, false);
    };
    if (current.forRepair || current.urlTried) receiveParts();
    else downloadPack(current, boxes, options, receiveParts);
    return;
  }
  if (!current.manifest || !Number.isInteger(given.i) || typeof given.d !== "string"
    || (typeof given.sha256 === "string" && given.sha256 !== current.manifest.pack.sha256)) return;
  const i = given.i as number;
  if (i < 0 || i >= current.parts! || current.got.has(i)) return;
  const bytes = Buffer.from(given.d, "base64");
  const size = fs.statSync(path.join(INCOMING, "pack")).size;
  if (bytes.length !== Math.min(PART, size - i * PART)) return failed("The other computer sent an invalid pack part.");
  const handle = fs.openSync(path.join(INCOMING, "pack"), "r+");
  try { fs.writeSync(handle, bytes, 0, bytes.length, i * PART); } finally { fs.closeSync(handle); }
  current.got.add(i);
  notePulled(current);
  current.asked.delete(i);
  current.heard = current.progressed = now;
  if (current.got.size < current.parts!) return ask(current, options, now, false);
  const pack = fs.readFileSync(path.join(INCOMING, "pack"));
  if (crypto.createHash("sha256").update(pack).digest("hex") !== current.manifest.pack.sha256)
    return failed("The pack from the other computer did not match its signed manifest.");
  savePack(current, current.boxes!, pack, options);
}

function savePack(current: Pull, boxes: string, pack: Buffer, options: FleetOptions): void {
  if (!stillChosen(current, (options.clock ?? Date.now)())) return endPull();
  if (!keepRelease(boxes, pack)) return failPull(current, options, "Couldn’t keep the signed release.");
  tried.clear();
  endPull();
  followPeers(options);
}

// ---- Check for Updates on a member: the main looks at the feed ----

const CHECK_FILE = path.join(RUN, "fleet-check.json");
const CHECK_KEPT = 24 * 60 * 60_000;
const shortName = (version: string) => version.replace(/^.*-(beta\.\d+)$/, "$1");

/** A member's Check for Updates: kept until the relay owner tells the main (`consumeFleetCheck`),
 * at once where the main is reached, else once it's back. In words, what happens. */
export function askMainToCheck(now = Date.now()): { message: string } {
  const link = readLink(), setup = followedSetup(link);
  const main = setup && peersOf(link).find(peer => peer.key === setup.primary);
  const told = setup ? readPeerAgents()[setup.primary] : undefined;
  const name = main?.name || "your main computer";
  writePrivate(CHECK_FILE, JSON.stringify({ at: now }));
  if (!setup || !peerIsAway(setup.primary, told?.heard, readReach(), now)) return { message: `${name} is checking for updates.` };
  const held = heldRelease();
  const where = main?.name ? `${main.name} (your main computer)` : name;
  return { message: held && newerVersion(held.version, runningVersion(boxVersion()))
    ? `${shortName(held.version)} is ready. It installs on all your computers when ${where} is back.`
    : `Kuvo checks for updates when ${where} is back.` };
}

/** In the relay owner: a kept Check for Updates goes to the main once it's reached. */
export function consumeFleetCheck(asked: FleetOptions = {}): void {
  const kept = parsed(textOf(CHECK_FILE));
  if (!isObject(kept)) return;
  const options = { ...ownWays(), ...asked };
  const now = (options.clock ?? Date.now)();
  const link = readLink(), setup = followedSetup(link);
  if (typeof kept.at !== "number" || now - kept.at > CHECK_KEPT || setup?.role !== "member") return void fs.rmSync(CHECK_FILE, { force: true });
  if (peerIsAway(setup.primary, readPeerAgents()[setup.primary]?.heard, readReach(), now)) return;
  if (tell(setup.primary, { check: true }, options)) fs.rmSync(CHECK_FILE, { force: true });
}

/** Try Again for another of your computers (`key`), asked of any box: passed on to that computer,
 * which retries as its own Try Again does (`requestFleetRetry`). False where it can't be reached. */
export function askPeerToRetry(key: string, ref: { version: string; sha256: string }, asked: FleetOptions = {}): boolean {
  return tell(key, { retry: ref }, { ...ownWays(), ...asked });
}

export function retryTransfer(version: string): void {
  if (pull?.version === version) endPull();
  for (const key of tried) if (key.endsWith(` ${version}`)) tried.delete(key);
  followPeers();
}

/** The pull under way, for a test: its peer, version and how many parts it has of how many. */
export function pullUnderWay(): { peer: string; version: string; got: number; parts?: number } | null {
  return pull ? { peer: pull.peer, version: pull.version, got: pull.got.size, ...(pull.parts !== undefined ? { parts: pull.parts } : {}) } : null;
}

/** Forgets the transfer for a test or a new link process. */
export function forgetFleet(): void {
  endPull();
  tried.clear();
}

export function clearIncoming(): void {
  endPull();
}
