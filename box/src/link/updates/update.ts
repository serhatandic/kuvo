import { releaseDisk } from "../server/disk-worker";
import { additiveObjectFormat } from "../../shared/format-version";
import { readVersionedFile } from "../../shared/versioned-file";
import { boxFilesFormat, installedFormat, readInstalled } from "../../shared/release-formats";
// Updates (PROTOCOL.md, "Updates")
// A box checks signed releases from the feed, an app on the same computer, or a setup peer.
// It verifies the pack, prepares the version beside the current one, and switches bin once.
// The keeper checks that the selected version starts and can restore the previous target.

import { installedLinkBuild } from "../../shared/folder-version";
import { syncDurableFile, writeDurablePrivate } from "../../shared/files";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject, parsed } from "../../shared/json";
import { updateUnderWay } from "../../shared/updating";
import type { Body } from "../../shared/json";
import { commandLineOf, engineHome, engineRuns, processRuns as runs } from "../../shared/platform";
import type { BridgeRecord, EngineRecord, Target } from "../../shared/records";
import type { LastUpdate, UpdateStatus, UpdateStep } from "../../shared/routes/box";
import { asked, call, fetchedPatiently, readBridgeFile, readEngineFile } from "../engine/client";
import type { UncheckedAnswer } from "../engine/client";
import { listOf, locked, textOf, writePrivate } from "../files";
import { BRIDGE_FILE, K, OWN_RUNTIME, REAL_BOX, RUN, SCRIPT, UPDATES, homeFolder, upgradeLinkFolder } from "../home";
import type { AnswerPiece, RouteContext } from "../server/context";
import type { LinkState } from "../state";
import { engineBusy, keeperEngine } from "./engine-swap";
import { engineSwapFormat, installHoldFile, swapLine, swapUnderWay, type EngineSwap } from "../../shared/engine-swap";
import { waitForPromptWrites } from "../moves/admission";
import { releaseWaiting } from "../engine/queue";
import { activationAllowed, askMainToCheck, isUpdateMain, keepRelease } from "./fleet";
import { recordCannot, recordReady, sameRelease, type ReleaseRef } from "./facts";
import { appendUpdateEvent } from "./journal";
import { activeBin, convertBin, previousBin, pruneVersions, stageVersion, versionBin } from "./versions";
import { rememberCutTurns } from "./interrupted";
import { isEngineModel } from "../../shared/subagents";
import { sleep } from "../../shared/wait";

// The releases' public keys (scripts/set-box-key.sh, scripts/sign-box.mjs), base64: with none, no
// box updates itself.
export const UPDATE_KEYS = ["iLSFoygG8wg+r2yj+/qnoMHpZIhPKRNLlf9vpEKmx0A="];
const UPDATE_URL = "https://github.com/serhatandic/kuvo-code/releases/latest/download/boxes.json";
const UPDATE_STATE = path.join(RUN, "update.json");
const UPDATE_TRIAL = path.join(RUN, "update-trial");
/** An update is here on trial: until it's confirmed, the keeper may yet put the version before back. */
export function checkRollbackRecords(): string | null {
  updateState();
  readInstalled(UPDATES);
  if (fs.existsSync(BRIDGE_FILE)) readBridgeFile();
  let version: string;
  try { version = fs.readFileSync(UPDATE_TRIAL, "utf8").trim(); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw error;
  }
  if (!/^[0-9][0-9A-Za-z.+-]*$/.test(version)) throw new Error("The update trial marker is unreadable.");
  const restore = path.join(RUN, "update-restore");
  if (fs.existsSync(restore) && !/^\d+$/.test(fs.readFileSync(restore, "utf8").trim())) {
    throw new Error("The update rollback counter is unreadable.");
  }
  return version;
}
export const updateOnTrial = (): boolean => fs.existsSync(UPDATE_TRIAL);
export const switchPending = (): boolean => {
  const previous = textOf(previousBin(K))?.trim();
  return !!previous && previous !== activeBin(K);
};
export const UPDATE_LOCK = path.join(RUN, ".updating");
const ROLLBACK = path.join(UPDATES, "rollback");
const LAUNCHERS = path.join(RUN, "launchers.json");
const ED25519_SPKI = Buffer.from("302a300506032b6570032100", "hex");
// A release this old before a box takes it by itself (most Macs have updated by then, and a bad
// one doesn't reach every box the day it's out). Only its own release check waits so: one an app
// offers, or another of your computers hands it, is taken at once.
const RELEASE_AGE = 3 * 86_400_000;
const MAC_MEMORY = 30 * 86_400_000;
export const BOX_FILE = /^(bin\/[A-Za-z0-9._-]+|bin\/runtime\/[A-Za-z0-9._-]+|config\/opencode\/plugins\/[A-Za-z0-9._-]+)$/;
// A Mac bundle a release carries (the box's helper, `bin/Kuvo Helper.app`), named in `box.json`'s
// `bundles` (which a box from before them passes over), each of its files by its path in it:
// none a link, no part starting with a dot (never `..`), and the bundle swapped whole.
const BUNDLE = /^bin\/[A-Za-z0-9][A-Za-z0-9._ -]*\.app$/;
const BUNDLE_PART = /^(?:[A-Za-z0-9_][A-Za-z0-9._+ -]*\/)*[A-Za-z0-9_][A-Za-z0-9._+ -]*$/;
const SWITCH_KEEPER = "# Kuvo release switch v1";
const RELEASE_VERSION = /^\d+\.\d+\.\d+(-beta\.\d+)?$/;

// A version as versions are ordered (a beta before its release), or null for one that isn't.
export function versionOrder(version: string | null | undefined): [number, number, number, number] | null {
  const found = /^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$/.exec(version ?? "");
  return found ? [+found[1], +found[2], +found[3], found[4] == null ? Infinity : +found[4]] : null;
}

// Whether `version` is newer than `than` (none: any is).
export function newerVersion(version: string | null | undefined, than: string | null | undefined): boolean {
  const a = versionOrder(version), b = versionOrder(than);
  if (!a) return false;
  if (!b) return true;
  for (let i = 0; i < 4; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

/** A release's manifest (in `boxes.json`, signed): its version, its engine's, when it came out,
 * its files' pack, and its engine for each kind of machine: Linux's and Macs', each by `uname -m`'s
 * kinds. */
export interface BoxRelease {
  kuvo: "box/1";
  version: string;
  engine: string;
  spreads?: boolean;
  layout?: number;
  clientProtocol?: number;
  released?: string;
  launch?: "rust";
  packs?: Record<string, BoxRelease["pack"]>;
  pack: { url: string; sha256: string };
  linux?: Record<string, { url?: string; integrity?: string } | undefined>;
  darwin?: Record<string, { url?: string; integrity?: string } | undefined>;
}

// A release's manifest, from boxes.json as the release workflow signs it: only with a key that's
// pinned, its signature over "kuvo-box v1\n" and its bytes. It throws for anything else.
export function verifyBoxes(text: string, keys: string[] = UPDATE_KEYS): BoxRelease {
  // (Each checked below.)
  let given: { manifest: string; signature: string; key: string } | null;
  try { given = JSON.parse(text); } catch { throw new Error("That isn't a release."); }
  if (typeof given?.manifest !== "string" || typeof given.signature !== "string" || typeof given.key !== "string") throw new Error("That isn't a release.");
  const key = keys.map(each => Buffer.from(each, "base64")).find(each => each.length === 32 && each.equals(Buffer.from(given!.key, "base64")));
  if (!key) throw new Error("It isn't signed with a key this machine trusts.");
  const bytes = Buffer.from(given.manifest, "base64");
  const publicKey = crypto.createPublicKey({ key: Buffer.concat([ED25519_SPKI, key]), format: "der", type: "spki" });
  if (!crypto.verify(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), bytes]), publicKey, Buffer.from(given.signature, "base64"))) throw new Error("Its signature doesn't match.");
  const manifest: BoxRelease = JSON.parse(bytes.toString("utf8"));
  if (manifest?.kuvo !== "box/1" || !RELEASE_VERSION.test(manifest.version ?? "") || typeof manifest.engine !== "string" || !/^[0-9][0-9A-Za-z.-]*$/.test(manifest.engine)
    || (manifest.layout !== undefined && (!Number.isSafeInteger(manifest.layout) || manifest.layout < 0))
    || (manifest.clientProtocol !== undefined && (!Number.isSafeInteger(manifest.clientProtocol) || manifest.clientProtocol < 1))
    || !isObject(manifest.pack) || typeof manifest.pack.url !== "string" || !/^[0-9a-f]{64}$/.test(manifest.pack.sha256 ?? "")) throw new Error("That isn't a release.");
  return localPack(manifest);
}

export function localPack(manifest: BoxRelease): BoxRelease {
  if (manifest.launch !== "rust") return manifest;
  const arch = { x64: "x86_64", arm64: "aarch64" }[os.arch() as "x64" | "arm64"];
  const system = process.platform === "darwin" ? "apple-darwin" :
    process.platform === "linux" ? "unknown-linux-musl" : undefined;
  const pack = system && arch && manifest.packs?.[`${arch}-${system}`];
  if (!isObject(pack) || typeof pack.url !== "string" || !/^[0-9a-f]{64}$/.test(pack.sha256 ?? ""))
    throw new Error("The release has no pack for this machine.");
  return { ...manifest, pack };
}

export function namesPack(manifest: BoxRelease, hash: string): boolean {
  return manifest.pack.sha256 === hash ||
    (manifest.launch === "rust" && Object.values(manifest.packs ?? {}).some(pack => pack?.sha256 === hash));
}

// This box's version, as the Kuvo Code that put its files there marked it (`bin/box-version`).
export const boxVersion = (): string | null => {
  const version = textOf(path.join(K, "bin", "box-version"))?.trim();
  return versionOrder(version) ? version! : null;
};

/** What an update came to (`update`'s `result`), as `kuvo/update`'s `last` says it. */
export type UpdateResult = LastUpdate["result"];

/** What `update` says happened: its result, the version it's at or found, and why, in words. */
interface UpdateResultBody {
  result: UpdateResult;
  version?: string;
  message?: string;
}
export type UpdateOutcome = UpdateResultBody;

/** An update being tried (`trial`): from and to which version, since when, when its files were
 * put in place (`swapped`), the features the box said before (what it must still say), how its
 * judging stands (`updates/health.ts`), and whether it was judged well. */
export interface UpdateTrial {
  from: string;
  to: string;
  sha256?: string;
  at: number;
  swapped?: number;
  /** The rollback is ready, and installation has not yet finished copying the release. */
  installing?: true;
  features?: string[];
  judged?: import("./health").Judged;
  /** How the engine's data was as a newer engine took it (`keepData`), to tell later whether that
   * engine wrote to it. */
  dataKept?: string;
  /** Whether the updater stopped the engine for a new one and hasn't started it yet. */
  engineStopped?: boolean;
  healthy?: boolean;
}

/** `run/update.json`: the newest release heard of (and when it was looked for), an update being
 * tried, the step under way, how the last went, and the release check's round it last looked in. */
export interface UpdateState {
  latest?: string;
  engineBusy?: { version: string; message: string };
  checked?: number;
  trial?: UpdateTrial;
  /** The step under way (shared/routes/box.ts, `STEP`). */
  running?: UpdateStep;
  /** How the last update went, when (shared/routes/box.ts, `LAST_UPDATE`). */
  last?: LastUpdate;
  /** The release check's round it last looked in (`releaseRound`). */
  round?: string;
  /** How the last change of engine went (`updates/engine-swap.ts`). */
  engine?: import("./engine-swap").EngineChangeLast;
}

/** A change to `run/update.json`: each field given, null to take it away. */
export type UpdateChange = { [Key in keyof UpdateState]?: UpdateState[Key] | null };

const updateStateFormat = additiveObjectFormat("Box update state");
export function updateState(): UpdateState {
  return readVersionedFile<UpdateState>(UPDATE_STATE, updateStateFormat) ?? {};
}
export function noteUpdate(change: UpdateChange): UpdateState {
  const state = { ...updateState(), ...change };
  for (const [key, value] of Object.entries(change)) if (value == null) delete state[key as keyof UpdateState];
  try { writePrivate(UPDATE_STATE, JSON.stringify(updateStateFormat.write(state))); } catch {}
  return state as UpdateState;
}

// Who started this box's engine lately: each Mac by its ID (in its launches' notes), with when.
export function noteLauncher(readEngine: () => EngineRecord = readEngineFile, now = Date.now()): void {
  let mac: unknown = null;
  try { mac = readEngine()?.notes?.mac; } catch {}
  if (typeof mac !== "string" || !mac || mac.length > 100) return;
  let all: Record<string, number> = {};
  try { all = JSON.parse(fs.readFileSync(LAUNCHERS, "utf8")) ?? {}; } catch {}
  if (now - (all[mac] ?? 0) < 3600_000) return;
  all[mac] = now;
  for (const [key, at] of Object.entries(all)) if (now - at > MAC_MEMORY) delete all[key];
  try { writePrivate(LAUNCHERS, JSON.stringify(all)); } catch {}
}

/** Why this folder cannot update itself. */
export function updatesBlocked(_link?: LinkState | null, _now?: number, root = K): string | null {
  return path.resolve(root) === path.join(homeFolder(), ".kuvo-code") ? null : "Only a machine's own Kuvo Code folder updates itself.";
}

/** Whether a conversation is working on the box now: the engine's, or the coding agents'; null when
 * that can't be told (one that's recorded as running didn't answer, a moment's restart). */
export async function agentsWorking(readEngine: () => EngineRecord = readEngineFile, readBridge: () => BridgeRecord = readBridgeFile): Promise<boolean | null> {
  let unknown = false;
  for (const [read, program] of [[readEngine, "opencode"], [readBridge, "kuvo-agents"]] as const) {
    let target: Target | null = null;
    try { target = read(); } catch (error) {
      if (error instanceof UnsupportedFormatVersion) unknown = true;
    }
    if (!target?.port || !target?.password) continue;
    // (A record its process left as it went, killed: nothing works there.)
    const pid = (target as { pid?: unknown }).pid;
    if (pid !== undefined && !runs(pid, program)) continue;
    const answer = await asked(target, "GET", "api/session/active").catch(() => null) as UncheckedAnswer | null;
    const active = answer?.data;
    if (Array.isArray(active) ? active.length : isObject(active) && Object.keys(active).length) return true;
    if (!Array.isArray(active) && !isObject(active)) unknown = true;
  }
  return unknown ? null : false;
}


const RELEASE_DRAIN_MOST = 5 * 60_000;
const ENGINE_TURNS_WAIT = "The release is ready. Kuvo will try again after the engine has had time to finish.";
const RELEASE_DRAIN_RETRY = 60_000;

export function releaseDrainDeferred(version: string, now: number): boolean {
  const last = updateState().last;
  return last?.result === "waiting" && last.version === version && last.message === ENGINE_TURNS_WAIT &&
    now - last.at < RELEASE_DRAIN_RETRY;
}

async function bridgeEngineWorking(signal: AbortSignal): Promise<boolean> {
  let bridge: BridgeRecord;
  try { bridge = readBridgeFile(); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  if (!runs(bridge.pid, "kuvo-agents")) return false;
  const reply = await asked(bridge, "GET", "api/session/active?release-switch", undefined, 3000);
  if (!isObject(reply) || !isObject(reply.data)) return true;
  if (Array.isArray(reply.engineTurns)) return reply.engineTurns.length !== 0;
  // An installed bridge may predate the query. Its session model includes Auto, which can fall
  // back to an engine model. The bridge and its loopback listener stay up until the switch.
  const active = await Promise.all(Object.keys(reply.data).map(id =>
    call(bridge, "GET", `/api/session/${encodeURIComponent(id)}`, undefined, signal)));
  return active.some(session => !isObject(session) || !isObject(session.model) ||
    typeof session.model.providerID !== "string" || isEngineModel({ providerID: session.model.providerID }));
}

async function nativeEngineWorking(): Promise<boolean> {
  let engine: EngineRecord;
  try { engine = readEngineFile(); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  if (!engineRuns(engine.pid, K)) return false;
  return engineBusy(engine);
}

/** The release hold was published before probing. Unknown work defers the switch too. */
export async function releaseSwitchQuiet(keepEngine: boolean): Promise<boolean> {
  const signal = AbortSignal.timeout(3000);
  const busy = await Promise.all([
    bridgeEngineWorking(signal).catch(() => true),
    keepEngine ? false : nativeEngineWorking().catch(() => true),
  ]);
  return !busy.some(Boolean);
}

async function drainEngineTurns(keepEngine: boolean, deadline: number, clock: () => number): Promise<boolean> {
  while (true) {
    if (await releaseSwitchQuiet(keepEngine)) return true;
    const left = deadline - clock();
    if (left <= 0) return false;
    await sleep(Math.min(1000, left));
  }
}

/** Release messages held through the keeper's health check, under the same lock as activation. */
export async function finishReleaseHold(): Promise<void> {
  const hold = installHoldFile(RUN);
  if (!fs.existsSync(hold) || fs.existsSync(previousBin(K)) || !takeUpdateLock()) return;
  try {
    if (fs.existsSync(previousBin(K))) return;
    await releaseWaiting(readEngineFile());
    fs.rmSync(hold, { force: true });
  } finally {
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
}

export const hexSHA256 = (data: Buffer): string => crypto.createHash("sha256").update(data).digest("hex");

// `command` with `args`, in the box's folder, its environment the bridge's: its exit status and
// output, within `timeout` ms.
function ran(command: string, args: string[], env: Record<string, string> = {}, timeout = 120_000): { status: number | null; output: string } {
  const result = spawnSync(command, args, { cwd: K, env: { ...process.env, ...env }, encoding: "utf8", timeout });
  return { status: result.status, output: `${result.stdout ?? ""}${result.stderr ?? ""}`.trim() };
}


export const UPDATING_ALREADY = "It's updating already.";
// An update under way, by its process (`run/.updating/pid`): one at a time, and one whose process
// went isn't one.
export function isUpdating(): boolean {
  return updateUnderWay(RUN);
}

export function takeUpdateLock(): boolean {
  return locked(path.join(RUN, ".update-acquire.lock"), () => {
    if (isUpdating()) return false;
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
    fs.mkdirSync(UPDATE_LOCK);
    fs.writeFileSync(path.join(UPDATE_LOCK, "pid"), String(process.pid), { mode: 0o600 });
    return true;
  }, { required: true });
}

// A trial left by a pre-switch updater can still be settled during the first boot.
export function newEngineUnused(trial: UpdateTrial): void {
  if (trial.dataKept !== undefined && trial.dataKept === dataNow()) fs.rmSync(path.join(ROLLBACK, "engine-started"), { force: true });
}

/** Where a copy of the engine's data is kept as an update changes its engine (`updates/backups`):
 * never put back by anything, a last resort to restore by hand; only the latest, for 30 days. */
export const BACKUPS = path.join(UPDATES, "backups");
const BACKUP_DAYS = 30 * 86_400_000;

/** Copies of the engine's data kept longer than 30 days (since they were made) taken away. */
export function dropOldBackups(now = Date.now()): void {
  for (const name of listOf(BACKUPS)) {
    let at = 0;
    try { at = fs.statSync(path.join(BACKUPS, name)).mtimeMs; } catch {}
    if (now - at > BACKUP_DAYS) fs.rmSync(path.join(BACKUPS, name), { recursive: true, force: true });
  }
}

// What the copies kept take, in bytes.
function backupsSize(): number {
  let size = 0;
  for (const name of listOf(BACKUPS)) for (const each of listOf(path.join(BACKUPS, name))) try { size += fs.statSync(path.join(BACKUPS, name, each)).size; } catch {}
  return size;
}

// The engine's data as it is, the engine stopped, before another engine reads it: kept whole or not
// at all in `updates/backups/<date>-<engine>-to-<engine>`, and the one before it goes. What it says:
// how the data was as it was copied (`dataNow`), to tell later whether an engine wrote to it since.
export function keepData(name: string): string {
  const folder = path.join(K, "data", "opencode"), partial = path.join(BACKUPS, `.${name}.partial`);
  fs.mkdirSync(BACKUPS, { recursive: true, mode: 0o700 });
  // (The one before goes first: its room is what this one takes, `roomForData`.)
  for (const other of listOf(BACKUPS)) fs.rmSync(path.join(BACKUPS, other), { recursive: true, force: true });
  fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
  for (const each of ["opencode.db", "opencode.db-wal", "opencode.db-shm"]) {
    if (fs.existsSync(path.join(folder, each))) fs.copyFileSync(path.join(folder, each), path.join(partial, each));
  }
  fs.renameSync(partial, path.join(BACKUPS, name));
  return dataNow();
}

// The engine's data, each file's size and when it was last written (none: "-"): what changes as
// any engine writes to it.
export function dataNow(): string {
  return ["opencode.db", "opencode.db-wal"].map(each => {
    try { const stat = fs.statSync(path.join(K, "data", "opencode", each)); return `${stat.size}:${stat.mtimeMs}`; } catch { return "-"; }
  }).join(" ");
}

// Whether there's room to keep the engine's data beside it, with some to spare.
export function roomForData(): boolean {
  let size = 0;
  for (const name of ["opencode.db", "opencode.db-wal"]) try { size += fs.statSync(path.join(K, "data", "opencode", name)).size; } catch {}
  const free = ran("df", ["-Pk", K]).output.split("\n").pop()?.trim().split(/\s+/)[3];
  // (The copy kept before goes as the new one's made: its room counts.)
  return !free || Number(free) * 1024 + backupsSize() > size * 1.1 + 200_000_000;
}

// COMPAT(0.7 files): `bin/.own`, the box's own copies of the tools an older Mac puts its own over. Goes on the user's word, once no Mac from before 1.0 connects.
// The box's tools that an older Kuvo Code on a Mac (0.7 and before) puts its own over as it
// connects: the box keeps copies of its own (`bin/.own`), which its keeper puts back, until a
// repair from the release it runs (`repair.ts`) has found its files sound, and removed them.
const OWN_TOOLS = ["kuvo-agents.js", "kuvo-browser.js", "kuvo-wake", "kuvo-npx"];

// The order files go in: its version first (a Mac connecting meanwhile leaves the box as it is),
// the keeper's mark last (the keeper starts its new script only once the script is there).
export const installOrder = (files: string[]): string[] => [...files].sort((a, b) => (a === "bin/box-version" ? -1 : b === "bin/box-version" ? 1 : a === "bin/.kuvo-boot" ? 1 : b === "bin/.kuvo-boot" ? -1 : a < b ? -1 : 1));

/** Check the feed, prepare a verified release folder, or activate a staged release. */
export interface UpdateOptions {
  now?: boolean;
  from?: string;
  phase?: "prepare" | "activate";
  expected?: ReleaseRef;
  keys?: string[];
  url?: string;
  blocked?: string | null;
  clock?: () => number;
  drainClock?: () => number;
  step?: (running: UpdateStep) => void;

}

/** A release's `box.json`, in its pack: its version, its engine's, each file's SHA-256, and its
 * Mac bundles' (`bin/<name>.app`), each file by its path in the bundle. */
export interface BoxFiles {
  launch?: "rust";
  version: string;
  engine: string;
  files: Record<string, string>;
  bundles?: Record<string, Record<string, string>>;
  /** The engines this release was tested with, starting with its own. */
  engines?: string[];
  /** Whether the engine before (`engines[1]`) opens what this one wrote, as its release checked:
   * a new engine that doesn't start may go back to it. */
  downgradeSafe?: boolean;
}

export function packedBox(pack: Buffer, version: string): BoxFiles {
  // (Only `box.json`, read from tar's output: nothing unpacked. Packed from a folder, `./box.json`.)
  for (const name of ["./box.json", "box.json"]) {
    const read = spawnSync("tar", ["-xzOf", "-", name], { input: pack, maxBuffer: 10_000_000, timeout: 120_000 });
    if (read.status !== 0 || !read.stdout?.length) continue;
    const box = boxFilesFormat.read(JSON.parse(read.stdout.toString("utf8"))) as unknown as BoxFiles;
    if (box?.version !== version || !isObject(box.files)) throw new Error("not this release");
    return box;
  }
  throw new Error("no box.json");
}

// Whether the last `update` failed in a way that's passing, before anything changed: a download
// that broke off (the release, its files or its engine), or its engine that couldn't be stopped
// (a start of it under way): what the installer's update tries once more by itself (`main.ts`),
// unlike a check that would fail again (what it got isn't what it says, no room, a self-test).
export async function update({ now = false, from, phase, expected, keys = UPDATE_KEYS, url = UPDATE_URL,
  blocked = updatesBlocked(), clock = Date.now, drainClock = () => performance.now(), step = () => {} }: UpdateOptions = {}): Promise<UpdateOutcome> {
  if (!keys.length) return { result: "off", message: "Updates aren't set up for this machine yet." };
  if (blocked) return { result: "blocked", message: blocked };
  if (switchPending() || updateOnTrial() || !takeUpdateLock()) return { result: "busy", message: UPDATING_ALREADY };
  const before = boxVersion();
  let attempt = expected;
  let engineHold = false;
  let stage: string | undefined;
  try {
    if (!before) return { result: "blocked", message: "This computer’s box version is unknown." };
    if (["stopped", "paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark))))
      return { result: "blocked", message: "This computer’s box is stopped." };
    if (phase === "activate") {
      if (!expected || !activationAllowed(expected, clock())) return { result: "waiting" };
      const target = versionBin(expected.version, expected.sha256);
      const previous = activeBin(K)!;
      const keeper = path.join(K, target, "kuvo-start");
      const enginePid = (() => { try { return readEngineFile()?.pid; } catch { return undefined; } })();
      const home = enginePid ? engineHome(enginePid, K) : undefined;
      if (path.basename(K) !== ".kuvo-code" || path.resolve(K) !== path.resolve(home ?? path.dirname(K), ".kuvo-code")) {
        throw new Error("This keeper doesn’t run from Kuvo’s home folder.");
      }
      const selectedHome = home ?? path.dirname(K);
      const keepEngine = ran("sh", [keeper, "switch-engine", target], { HOME: selectedHome }).output === "keep";
      const deadline = drainClock() + RELEASE_DRAIN_MOST;
      const holding = Date.now();
      const to = /^V=(.+)$/m.exec(textOf(keeper) ?? "")?.[1] ?? "";
      const hold: EngineSwap = { from: runningEngineVersion() ?? "", to, since: holding, stage: "holding", holding, pid: process.pid };
      engineHold = true;
      step("installing");
      writePrivate(installHoldFile(RUN), JSON.stringify(engineSwapFormat.write(hold)));
      await waitForPromptWrites();
      if (!await drainEngineTurns(keepEngine, deadline, drainClock)) {
        return { result: "waiting", version: expected.version, message: ENGINE_TURNS_WAIT };
      }
      if (!activationAllowed(expected, clock()) || ["stopped", "paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark)))) {
        return { result: "waiting", version: expected.version };
      }
      await rememberCutTurns(K, !keepEngine);
      writeDurablePrivate(previousBin(K), previous + "\n", K);
      const changed = ran("sh", [keeper, "stop", "--switch", target, keepEngine ? "keep" : "restart"], { HOME: selectedHome });
      if (activeBin(K) !== target) {
        fs.rmSync(previousBin(K), { force: true });
        throw new Error(changed.output || "Kuvo couldn’t stop its workers for the release.");
      }
      return { result: "updated", version: expected.version };
    }

    step("checking");
    if (!from && !isUpdateMain()) return { result: "waiting", message: "This computer follows its main computer." };
    let released: Buffer;
    try {
      released = from ? readRelease(path.join(from, "boxes.json"), 1_000_000) : await fetchedPatiently(url, 1_000_000, 30_000);
    } catch (error) {
      if (!from && (error as { status?: number }).status === 404) return { result: "current", version: before };
      throw error;
    }
    const manifest = verifyBoxes(released.toString("utf8"), keys);
    const ref = { version: manifest.version, sha256: manifest.pack.sha256 };
    if (expected && !sameRelease(ref, expected)) return { result: "waiting" };
    if (!from) noteUpdate({ latest: manifest.version, checked: clock() });
    if (!newerVersion(manifest.version, before)) return { result: "current", version: before };
    if (!from && !now && clock() - (Date.parse(manifest.released ?? "") || clock()) < RELEASE_AGE) {
      return { result: "waiting", version: manifest.version, message: "Automatic checks wait a few days after publication." };
    }
    if (manifest.spreads !== true) throw new Error("This release cannot be shared with your setup.");
    if (from) attempt = ref;
    step("downloading");
    const pack = from ? readRelease(path.join(from, packName(manifest)), 100_000_000) : await fetchedPatiently(manifest.pack.url, 100_000_000, 300_000);
    if (!from) {
      if (isUpdateMain() && !await releaseDisk.run("keepDownloadedRelease", released, pack, manifest.pack.sha256))
        throw new Error("Couldn’t keep the signed release.");
      return { result: "waiting", version: manifest.version };
    }
    if (hexSHA256(pack) !== manifest.pack.sha256) throw new Error("The release's files aren't what it says.");
    const behind = engineBehind(manifest.engine);
    if (behind) throw new Error(behind);
    const running = runningEngineVersion();
    const engineChanges = running !== manifest.engine;
    const box = packedBox(pack, manifest.version);
    stage = path.join(UPDATES, manifest.version);
    fs.rmSync(stage, { recursive: true, force: true });
    fs.mkdirSync(path.join(stage, "files"), { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(stage, "pack.tgz"), pack, { mode: 0o600 });
    if (ran("tar", ["xzf", path.join(stage, "pack.tgz"), "-C", path.join(stage, "files")]).status !== 0) throw new Error("The release's files couldn't be unpacked.");
    if (box.version !== manifest.version || box.engine !== manifest.engine || !isObject(box.files)) throw new Error("The release's files aren't what it says.");
    const rust = manifest.launch === "rust";
    if (rust !== (box.launch === "rust")) throw new Error("The release's files aren't what it says.");
    const layout = installedLinkBuild(path.join(stage, "files"))?.layout ?? 0;
    if (manifest.layout !== undefined && manifest.layout !== layout) throw new Error("The release’s folder layout doesn’t match its manifest.");
    const stagedBin = path.join(stage, "files", "bin");
    const keeper = path.join(stagedBin, "kuvo-start");
    if (!(textOf(keeper) ?? "").includes(SWITCH_KEEPER)) throw new Error("That release doesn’t support switching versions together.");
    const files = installOrder(Object.keys(box.files));
    // (Each a file, never a link to one elsewhere.)
    const matches = (file: string, sha: unknown): boolean => fs.lstatSync(file, { throwIfNoEntry: false })?.isFile() === true && hexSHA256(fs.readFileSync(file)) === sha;
    for (const file of files) {
      if (!BOX_FILE.test(file) || file.split("/").includes("..")) throw new Error("The release names a file outside Kuvo’s folder.");
      if (!matches(path.join(stage, "files", file), box.files[file])) throw new Error("The release's files aren't what it says.");
    }
    // Its bundles, each swapped whole, so nothing in one that `box.json` doesn't name. A Mac's
    // alone: on Linux they're left out.
    if (box.bundles !== undefined && !isObject(box.bundles)) throw new Error("The release's files aren't what it says.");
    const bundles = Object.keys(box.bundles ?? {}).sort();
    for (const bundle of bundles) {
      const listed = box.bundles![bundle];
      if (!BUNDLE.test(bundle) || files.includes(bundle) || !isObject(listed)) throw new Error("The release names a file outside Kuvo’s folder.");
      for (const [inner, sha] of Object.entries(listed)) {
        if (!BUNDLE_PART.test(inner)) throw new Error("The release names a file outside Kuvo’s folder.");
        if (!matches(path.join(stage, "files", bundle, inner), sha)) throw new Error("The release's files aren't what it says.");
      }
      for (const entry of fs.readdirSync(path.join(stage, "files", bundle), { recursive: true, encoding: "utf8" })) {
        const stat = fs.lstatSync(path.join(stage, "files", bundle, entry));
        if (!stat.isDirectory() && !(stat.isFile() && Object.hasOwn(listed, entry))) throw new Error("The release's files aren't what it says.");
      }
    }
    if (process.platform !== "darwin") bundles.length = 0;
    if (!files.includes("bin/box-version") || fs.readFileSync(path.join(stage, "files", "bin", "box-version"), "utf8").trim() !== manifest.version) throw new Error("The release's files aren't what it says.");
    // COMPAT(0.7 files): the release's tools kept in `bin/.own`. Goes on the user's word, once no Mac from before 1.0 connects.
    // Its tools kept as the box's own too (and put back with the rest, if it goes back), while it
    // keeps them (a box that has repaired from a release keeps none).
    if (fs.existsSync(path.join(K, "bin", ".own"))) for (const tool of OWN_TOOLS) {
      if (!files.includes(`bin/${tool}`)) continue;
      const from = path.join(stage, "files", "bin", tool), own = path.join(stage, "files", "bin", ".own", tool);
      fs.mkdirSync(path.dirname(own), { recursive: true, mode: 0o700 });
      fs.copyFileSync(from, own);
      fs.chmodSync(own, fs.statSync(from).mode & 0o777);
      files.push(`bin/.own/${tool}`);
    }
    // Each bundle in among the files, as one (before the keeper's mark, last).
    files.splice(0, files.length, ...installOrder([...files, ...bundles]));

    // Its engine, unless it's here already.
    const engineFolder = path.join(K, "engine", manifest.engine);
    let staged: string | null = null;
    if (!fs.existsSync(path.join(engineFolder, "opencode"))) {
      // (Another kind of machine has none: no kind, and no build.)
      const kind = { x64: "x86_64", arm64: "aarch64" }[os.arch() as "x64" | "arm64"];
      const build = (process.platform === "darwin" ? manifest.darwin : manifest.linux)?.[kind];
      if (!build?.url || !/^sha512-/.test(build.integrity ?? "")) throw new Error("The release has no engine for this machine.");
      step("downloading the engine");
      const engineURL = build.url;
      const tarball = await fetchedPatiently(engineURL, 400_000_000, 900_000);
      if (crypto.createHash("sha512").update(tarball).digest("base64") !== build.integrity!.slice(7)) throw new Error("The engine isn't what the release says.");
      fs.writeFileSync(path.join(stage, "engine.tgz"), tarball, { mode: 0o600 });
      fs.mkdirSync(path.join(stage, "engine"), { mode: 0o700 });
      if (ran("tar", ["xzf", path.join(stage, "engine.tgz"), "-C", path.join(stage, "engine"), "package/bin/opencode"]).status !== 0) throw new Error("The engine couldn't be unpacked.");
      staged = path.join(stage, "engine", "package", "bin", "opencode");
      fs.chmodSync(staged, 0o755);
    }

    // Tried before anything's swapped: the engine runs, the new bridge works with it, the keeper reads.
    step("checking it");
    const engine = staged ?? path.join(engineFolder, "opencode");
    if (ran(engine, ["--version"]).status !== 0) throw new Error("The new engine doesn't run here.");
    if (rust) {
      const program = path.join(stagedBin, "kuvo-box");
      if (!files.includes("bin/kuvo-box") ||
        ran(program, ["selftest", "--engine", manifest.engine], {}, 10_000).status !== 0) {
        throw new Error("The new box doesn't run with its engine here.");
      }
    } else {
      const tried = ran(engine, ["--config=/dev/null", "--no-env-file", path.join(stage, "files", "bin", "kuvo-link.js"), "selftest"], { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K });
      if (tried.status !== 0 || !tried.output.includes("ok")) throw new Error("The new link doesn't work with it here.");
      // Check the staged link against the running engine too, when it differs.
      if (running && engineChanges) {
        const before = path.join(K, "engine", running, "opencode");
        const triedBefore = fs.existsSync(before) ? ran(before, ["--config=/dev/null", "--no-env-file", path.join(stage, "files", "bin", "kuvo-link.js"), "selftest"], { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K }) : null;
        if (triedBefore && (triedBefore.status !== 0 || !triedBefore.output.includes("ok"))) throw new Error("The new link doesn't work with the engine at work here.");
      }
      if (ran("sh", ["-n", path.join(stage, "files", "bin", "kuvo-start")]).status !== 0) throw new Error("The new keeper doesn't read.");

      if (/^V=(.+)$/m.exec(textOf(keeper) ?? "")?.[1] !== manifest.engine) throw new Error("The new keeper names a different engine.");
      const env = { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K };
      const bridgeCheck = ran(engine, ["--config=/dev/null", "--no-env-file", path.join(stagedBin, "kuvo-agents.js"), "selftest"], env);
      if (bridgeCheck.status !== 0 || !/^ok(?:\s|$)/.test(bridgeCheck.output)) throw new Error("The new bridge doesn't work with its engine here.");
      const probeHome = path.join(stage, "probe");
      fs.mkdirSync(probeHome, { mode: 0o700 });
      const info = ran(engine, ["--config=/dev/null", "--no-env-file", path.join(stagedBin, "kuvo-link.js"), "box", "route", "--internal", "kuvo/info"],
        { ...env, KUVO_HOME: probeHome }, 10_000);
      if (info.status !== 0 || !/^\{"status":200,"internal":true,/.test(info.output)) throw new Error("The new link couldn't answer its internal kuvo/info call.");
    }
    if (rust && (ran("sh", ["-n", keeper]).status !== 0 ||
      /^V=(.+)$/m.exec(textOf(keeper) ?? "")?.[1] !== manifest.engine)) throw new Error("The new keeper doesn't read.");
    if (!roomForData()) throw new Error("There isn't room on it to keep its data while it updates.");
    // Rollback selects the old engine along with its code. Require release evidence that the
    // specific engine it would return to can read the new engine's data.
    const oldEngine = running ?? keeperEngine();
    if (oldEngine && oldEngine !== manifest.engine && !(box.downgradeSafe && box.engines?.includes(oldEngine))) {
      throw new Error("The new engine hasn't been checked for going back to the engine running here.");
    }

    if (staged) {
      fs.mkdirSync(engineFolder, { recursive: true, mode: 0o700 });
      syncDurableFile(staged);
      fs.renameSync(staged, path.join(engineFolder, "opencode"));
      syncDurableFile(path.join(engineFolder, "opencode"), K);
    }
    const previousTarget = await convertBin(K, before, installedPack() ?? undefined);
    const target = versionBin(manifest.version, manifest.pack.sha256);
    const receipt = JSON.stringify(installedFormat.write({ version: box.version, sha256: manifest.pack.sha256, files: box.files,
      engine: box.engine, engines: box.engines, downgradeSafe: box.downgradeSafe }));
    pruneVersions(K, [previousTarget, target]);
    const verified = path.join(stage, "verified");
    for (const file of files) {
      const to = path.join(verified, file);
      fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
      fs.renameSync(path.join(stage, "files", file), to);
    }
    if (rust) {
      fs.renameSync(path.join(stage, "files", "box.json"), path.join(verified, "box.json"));
      writeDurablePrivate(path.join(verified, "boxes.json"), released, verified);
    }
    await stageVersion(K, verified, target, receipt, rust);

    recordReady(ref);
    return { result: "waiting", version: manifest.version };
  } catch (error) {
    const reason = String((error as Error | undefined)?.message ?? error).slice(0, 300);
    if (attempt) {
      const fixable = /ENOSPC|EDQUOT|EACCES|EPERM|couldn.t stop its workers|isn.t room|download|answered|fetch|timed out|can.t be read/i.test(reason);
      try { recordCannot(attempt, reason, fixable); } catch {}
      appendUpdateEvent(attempt.version, phase ?? "prepare", reason, clock());
    }
    return { result: "failed", message: reason, version: attempt?.version };
  } finally {
    if (engineHold && !fs.existsSync(previousBin(K))) {
      fs.rmSync(installHoldFile(RUN), { force: true });
      try { await releaseWaiting(readEngineFile()); } catch {}
    }
    try { if (stage) fs.rmSync(stage, { recursive: true, force: true }); }
    finally { fs.rmSync(UPDATE_LOCK, { recursive: true, force: true }); }
  }
}

/** Validate and save a downloaded pack off the resident request loop. */
export function keepDownloadedRelease(released: Uint8Array, pack: Uint8Array, sha256: string): boolean {
  const bytes = Buffer.from(pack.buffer, pack.byteOffset, pack.byteLength);
  if (hexSHA256(bytes) !== sha256) throw new Error("The release's files aren't what it says.");
  return keepRelease(Buffer.from(released), bytes);
}

// A release's file in a folder here: a file, not too big, read whole; or why it can't be.
function readRelease(file: string, limit: number): Buffer {
  try {
    const stat = fs.statSync(file);
    if (stat.isFile() && stat.size <= limit) return fs.readFileSync(file);
  } catch {}
  throw new Error("That release can't be read.");
}

// The engine version the coding agents' bridge runs on now (its runtime's folder), or null.
function bridgeEngine(): string | null {
  try {
    const bridge = readBridgeFile();
    if (!bridge?.pid || !runs(bridge.pid, "kuvo-agents")) return null;
    // (Its runtime, `<box>/engine/<version>/…`: the engine's own program.)
    return /\/engine\/([^/\s]+)\/[^/\s]+(\s|$)/.exec(commandLineOf(bridge.pid) ?? "")?.[1] ?? null;
  } catch { return null; }
}

// The engines the release in place names (`updates/installed.json`'s `engines`).
function installedEngines(): string[] {
  try {
    const engines = readInstalled(UPDATES)?.engines;
    return Array.isArray(engines) ? engines.filter((each): each is string => typeof each === "string" && /^[0-9][0-9A-Za-z.-]*$/.test(each)) : [];
  } catch { return []; }
}

// The version of the engine running here, as its record says (null: none noted).
function runningEngineVersion(): string | null {
  try { return readEngineFile()?.version ?? null; } catch { return null; }
}

// Why a release with engine `engine` isn't taken: the engine running here is newer (it may have
// written what an older one can't read); null when it isn't, or that can't be told.
export function engineBehind(engine: string): string | null {
  let running: string | null | undefined = null;
  try { running = readEngineFile()?.version; } catch {}
  return running && newerVersion(running, engine) ? `Its engine (${running}) is newer than the release's (${engine}), and it never goes back to an older engine.` : null;
}

// The name of a release's pack, as its address ends: in a folder here, it's beside `boxes.json`.
export function packName(manifest: Pick<BoxRelease, "pack">): string {
  let name = "";
  try { name = decodeURIComponent(path.posix.basename(new URL(manifest.pack.url).pathname)); } catch {}
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) throw new Error("That isn't a release.");
  return name;
}

// The new bridge, once an update put it there: well, the update's done. What was kept goes, and
// engines but this one.
export function confirmUpdate(clock = Date.now): boolean {
  const state = updateState();
  readInstalled(UPDATES);
  if (!state.trial?.healthy || boxVersion() !== state.trial.to || isUpdating()
    || fs.existsSync(path.join(RUN, "update-restore"))
    || runs(Number(textOf(path.join(RUN, ".rolling-back", "pid"))), "kuvo-start")) return false;
  let engine: string | null | undefined = null;
  try { engine = readEngineFile()?.version; } catch {}
  // (How it went noted first: whoever sees the trial over finds it said.)
  noteUpdate({ trial: null, last: { at: clock(), result: "updated", from: state.trial.from, to: state.trial.to } });
  fs.rmSync(UPDATE_TRIAL, { force: true });
  syncDurableFile(RUN);
  upgradeLinkFolder();
  fs.rmSync(ROLLBACK, { recursive: true, force: true });
  // (Not the one the coding agents' bridge still runs on, until it moves to the new: its agents'
  // tools run from it.)
  const bridgeOn = bridgeEngine();
  // (Nor the keeper's own, which may wait to take the running one's place, nor those the release
  // names: the one before is kept to go back to, should a new one not start.)
  const kept = new Set([engine, bridgeOn, keeperEngine(), ...installedEngines()].filter((name): name is string => !!name));
  if (engine) for (const name of listOf(path.join(K, "engine"))) if (!kept.has(name) && !name.startsWith(".")) fs.rmSync(path.join(K, "engine", name), { recursive: true, force: true });
  dropOldBackups(clock());
  return true;
}

/** The recorded pack belongs to the version in place and is provisional during a trial. */
export function installedPack(): string | null {
  const installed = readInstalled(UPDATES);
  return isObject(installed) && installed.version === boxVersion() && typeof installed.sha256 === "string" ? installed.sha256 : null;
}

/** What an update put there (`updates/installed.json`): its version, and each file's SHA-256. */
interface Installed {
  version: string;
  files: Record<string, string>;
}

// COMPAT(0.7 files): how long a Mac's `run/mac-installing` counts. Goes on the user's word, once no Mac from before 1.0 connects.
export const MAC_INSTALLING = 3600_000;

// COMPAT(0.7 visits): drift, an older Mac's files put back over an update's. Goes on the user's word, once no Mac from before 1.0 connects.
// Whether a Mac that doesn't leave a newer box as it is (an older Kuvo Code) put its own files back
// since the box updated itself (seen twice, ten minutes apart, and not while a Mac says it puts its
// files in place): the box records what happened. Its version mark stays, so no other older Mac takes it further back.
export function checkDrift(now = Date.now()): boolean {
  const record = path.join(UPDATES, "installed.json"), pending = path.join(RUN, "update-drift");
  // A Mac's note that it puts its files in place (`run/mac-installing`) counts for an hour. One
  // whose connect was cut off before it marked the box (Disconnect, Remove, the network) is left
  // behind: past the hour, it goes.
  const note = path.join(RUN, "mac-installing");
  let installing = 0;
  try { installing = fs.statSync(note).mtimeMs; } catch {}
  if (installing && now - installing >= MAC_INSTALLING) {
    fs.rmSync(note, { force: true });
    installing = 0;
  }
  // Not while the box updates itself, or tries what it updated to: its files and version change then.
  if (isUpdating() || fs.existsSync(UPDATE_TRIAL)) return false;
  let installed: Installed | null = null;
  try { installed = readInstalled(UPDATES) as Installed | null; } catch { return false; }
  // A Mac marked it since (its own files, as they should be).
  if (installed?.version !== boxVersion() || !isObject(installed.files)) {
    fs.rmSync(record, { force: true });
    fs.rmSync(pending, { force: true });
    return false;
  }
  const changed = Object.entries(installed.files).some(([file, sha]) => {
    try { return hexSHA256(fs.readFileSync(path.join(K, file))) !== sha; } catch { return true; }
  });
  if (!changed || isUpdating() || now - installing < MAC_INSTALLING) {
    fs.rmSync(pending, { force: true });
    return false;
  }
  if (textOf(pending)?.trim() !== installed.version) {
    try { writePrivate(pending, installed.version); } catch {}
    return false;
  }
  noteUpdate({ last: { at: now, result: "failed", message: "An older Kuvo Code on a Mac put its own files back here: update Kuvo Code on your Macs." } });
  return true;
}

/** How often a box looks for a newer release by itself: needed only where no app opens and no peer
 * brings one (a phone-only fleet, Linux machines alone), so security fixes still come. */
export const RELEASE_CHECK = 6 * 3600_000;

/** The release check's round `now` falls in: one look in each, at any hour. */
export function releaseRound(now = Date.now(), offset = ROUND_OFFSET): string {
  return String(Math.floor((now + offset) / RELEASE_CHECK));
}

/** Where in its six hours each box's round falls: from its folder and its computer's name, so your
 * computers, and everyone's, don't all ask GitHub at once. */
const ROUND_OFFSET = parseInt(crypto.createHash("sha256").update(`${os.hostname()} ${K}`).digest("hex").slice(0, 8), 16) % RELEASE_CHECK;

/** A trial that ended without being said well (put back, or a rollback that gave up), noted as it
 * went, once: all put back (`updates/rollback` gone with the trial), or some of each version's files
 * left, which the next update puts right. `reason`: why, in words, where known. */
export function noteTrialEnded(reason = ""): void {
  const { trial } = updateState();
  if (!trial || updateOnTrial() || isUpdating()) return;
  const back = !fs.existsSync(ROLLBACK);
  const from = trial.from || "what it had";
  const message = back
    ? `${trial.to} didn't run well here${reason}, so it went back to ${from}. Your conversations stay as they are.`
    : `${trial.to} didn't run well here${reason}, and not all of it could go back to ${from}: Kuvo updates again later. Your conversations stay as they are.`;
  const at = Date.now();
  noteUpdate({ trial: null, last: { at, result: "failed", message, from: trial.from, to: trial.to } });
  appendUpdateEvent(trial.to, "rollback", message, at);
}

export function switchFailure(): { version: string; sha12: string; at: number; message: string } | null {
  const [seconds, version, sha12] = textOf(path.join(RUN, "update-cannot"))?.trim().split(" ") ?? [];
  if (!seconds || !/^\d+$/.test(seconds) || !version || !versionOrder(version) || !sha12 || !/^[a-f0-9]{12}$/.test(sha12)) return null;
  return { version, sha12, at: Number(seconds) * 1000, message: `${version} didn't start here` };
}

export function updateStatus(): UpdateStatus {
  // (A trial something else ended meanwhile, a keeper giving up say: how it went noted.)
  try { noteTrialEnded(); } catch {}
  const state = updateState();
  const cannot = switchFailure();
  if (cannot && (!state.last || cannot.at + 1_000 >= state.last.at))
    state.last = { result: "failed", at: cannot.at, to: cannot.version, message: cannot.message };
  let engine: string | null = null;
  try { engine = readEngineFile()?.version ?? null; } catch {}
  const swap = swapUnderWay(RUN);
  return {
    setUp: UPDATE_KEYS.length > 0,
    version: boxVersion(),
    engine,
    latest: state.latest ?? null,
    last: state.last ?? null,
    updating: isUpdating() ? state.running ?? "checking" : null,
    blocked: updatesBlocked(),
    ...(swap ? { engineChange: { stage: swap.stage, from: swap.from, to: swap.to, line: swapLine(swap)! } } : {}),
    ...(state.engine ? { engineLast: state.engine } : {}),
  };
}

// A client's request or the release check starts a detached updater. It carries on while
// the box's workers stop and the keeper takes over. Only one updater runs at a time.
let updateStarted: ChildProcess | null = null;
export function startUpdate({ now = false, round, keys, url, from, phase, expected, executable }: {
  now?: boolean; round?: string; keys?: string[]; url?: string; from?: string;
  phase?: UpdateOptions["phase"]; expected?: ReleaseRef; executable?: string;
} = {}): boolean {
  if (updateStarted || isUpdating() || switchPending()) return false;
  const test = !REAL_BOX && keys?.length ? ["--key", keys[0], ...(url ? ["--url", url] : [])] : [];
  const args = [...OWN_RUNTIME, SCRIPT, "update", "--switch", ...(now ? ["--now"] : []),
    ...(round ? ["--round", round] : []), ...(from ? ["--from", from] : []),
    ...(phase ? ["--phase", phase] : []), ...(expected ? ["--version", expected.version, "--sha256", expected.sha256] : []), ...test];
  const version = expected?.version ?? boxVersion() ?? "unknown";
  const started = Date.now();
  const failed = (message: string) => {
    const at = Date.now();
    appendUpdateEvent(version, "starting updater", message, at);
    if (expected) try { recordCannot(expected, message, true); } catch {}
    noteUpdate({ running: null, last: { at, result: "failed", message, to: version } });
  };
  let child: ChildProcess;
  let errors: number | undefined;
  try {
    // The updater keeps this descriptor if the caller exits during a live handover.
    const file = path.join(RUN, `.update-stderr-${crypto.randomUUID()}`);
    errors = fs.openSync(file, "wx+", 0o600);
    fs.unlinkSync(file);
    child = spawn(!REAL_BOX && executable ? executable : process.execPath, args, { detached: true, stdio: ["ignore", "ignore", errors], cwd: K, env: process.env });
  } catch (error) {
    if (errors !== undefined) fs.closeSync(errors);
    failed(`Couldn’t start the updater: ${(error as Error).message}`);
    return false;
  }
  updateStarted = child;
  const stderr = errors;
  let spawnFailed = false;
  child.once("error", error => {
    spawnFailed = true;
    failed(`Couldn’t start the updater: ${error.message}`);
  });
  child.once("close", (code, signal) => {
    if (updateStarted === child) updateStarted = null;
    try {
      if (spawnFailed || (!signal && code === 0)) return;
      const last = updateState().last;
      if (last?.at !== undefined && last.at >= started) return;
      const tail = Buffer.alloc(4_096);
      let error = "";
      try {
        const offset = Math.max(0, fs.fstatSync(stderr).size - tail.length);
        error = tail.toString("utf8", 0, fs.readSync(stderr, tail, 0, tail.length, offset)).trim();
      } catch {}
      const detail = error || (signal ? `stopped with ${signal}` : `exited with status ${code}`);
      failed(`The updater couldn’t finish: ${detail}`);
    } finally { fs.closeSync(stderr); }
  });
  child.unref();
  return true;
}

/** Check for Updates: the main looks at the feed now; a member asks its main to
 * (`askMainToCheck`), saying in words what happens. */
export function requestUpdateNow(options: { keys?: string[]; url?: string; executable?: string } = {}): { status: 202; message?: string } | { status: 409; message: string } {
  const blocked = updatesBlocked();
  if (blocked && (REAL_BOX || !options.keys)) return { status: 409, message: blocked };
  if (!isUpdateMain()) return { status: 202, ...askMainToCheck() };
  return startUpdate({ ...options, now: true }) ? { status: 202 } : { status: 409, message: UPDATING_ALREADY };
}

export async function* updateRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, updateStatus());
  if (method === "POST") {
    if ((parsed(request.body) as Body<"now">)?.now !== true) return yield* json(400, { message: "Say to update now (now)." });
    const started = requestUpdateNow();
    if (started.status === 409) return yield* json(409, { message: started.message });
    // (A member's: its main checks, said in `message`; this one's own status stays as it is.)
    if (started.message) return yield* json(202, { ...updateStatus(), message: started.message });
    return yield* json(202, { ...updateStatus(), updating: "checking" });
  }
  return yield* json(405, {});
}

/** The app's signed release is kept for the fleet scheduler. */
export function receiveRelease(from: unknown, keys = UPDATE_KEYS): { status: number; message?: string } {
  if (typeof from !== "string" || !path.isAbsolute(from)) return { status: 400, message: "Send the full path to the release folder." };
  try {
    const boxes = readRelease(path.join(from, "boxes.json"), 1_000_000);
    const manifest = verifyBoxes(boxes.toString("utf8"), keys);
    if (manifest.spreads !== true) throw new Error("This release cannot be shared with your setup.");
    const version = boxVersion(), installed = installedPack();
    if (newerVersion(version, manifest.version) || manifest.version === version && installed && installed !== manifest.pack.sha256)
      return { status: 202 };
    const pack = readRelease(path.join(from, packName(manifest)), 100_000_000);
    if (hexSHA256(pack) !== manifest.pack.sha256) throw new Error("The release's files aren't what it says.");
    if (!keepRelease(boxes, pack)) throw new Error("Couldn’t keep the signed release.");
    return { status: 202 };
  } catch (error) {
    return { status: 422, message: (error as Error).message };
  }
}

// Only the app on this computer may hand over its bundled release.
export async function* boxUpdateRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  if (!request.fromApp && !request.fromMac) return yield* json(403, { message: "Only Kuvo Code offers Kuvo on this computer a release." });
  const given = parsed(request.body);
  const kept = await releaseDisk.run("receiveRelease", isObject(given) ? given.from : undefined);
  if (kept.status !== 202) return yield* json(kept.status, { message: kept.message });
  return yield* json(202, updateStatus());
}
