// Updates (PROTOCOL.md, "Updates")
// A box updates itself to a newer Kuvo Code's release: one it finds itself (its release check, every
// six hours at any hour), one Kuvo Code on this Mac offers, one another of your computers hands it
// (`updates/fleet.ts`), or now (Update Now, from a Mac or a phone). None waits for night or for
// work to end: an update stops nothing (but see `engineApart`). Never to an older one, only what
// a key pinned here signed, only in a machine's own ~/.kuvo-code (not a test's or a demo's), and
// with layout compatibility checked against the setup’s computers. It checks what
// it got, tries the new engine and bridge, keeps what it had, swaps its files one by one and
// starts again; then it's judged by the one health rule (updates/health.ts), and goes back to the
// code it had (never the data) when that finds it unwell. A release can come from a folder on this
// computer too (`kuvo/box/update {from}`: the one an app carries, for it to offer as it updates),
// checked the same way and taken at the next quiet moment; and it can carry a signed Mac bundle
// (the box's helper), swapped whole.

import { beginFolderRollback, installedLinkBuild } from "../../shared/folder-version";
import { syncDurableFile, writeDurablePrivate } from "../../shared/files";
import type { SaveEnvelope } from "../../shared/routes/setup-save";
import { askSetupOutcome, askSetupSave, type SetupSaveReceipt } from "../../shared/setup-local";
import { INTERRUPTED_SAVE } from "../setup/shared";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { bundleBuild } from "../../shared/bundle";
import { UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject, parsed } from "../../shared/json";
import { updateUnderWay } from "../../shared/updating";
import type { Body } from "../../shared/json";
import { alive, commandLineOf, engineHome, engineRole, processRuns as runs, runsAs } from "../../shared/platform";
import type { BridgeRecord, EngineRecord, Target } from "../../shared/records";
import type { LastUpdate, UpdateStatus, UpdateStep } from "../../shared/routes/box";
import { quote } from "../../shared/text";
import { sleep } from "../../shared/wait";
import { readAsks } from "../areas/asks";
import { asked, fetchedPatiently, readBridgeFile, readEngineFile } from "../engine/client";
import type { UncheckedAnswer } from "../engine/client";
import { listOf, textOf, writePrivate } from "../files";
import { K, OWN_RUNTIME, REAL_BOX, RUN, SCRIPT, UPDATES, homeFolder, upgradeLinkFolder } from "../home";
import type { AnswerPiece, RouteContext } from "../server/context";
import { peersOf } from "../peers/known";
import { readLink, type LinkState } from "../state";
import { keeperEngine, SWAP_NOW } from "./engine-swap";
import { installHoldFile, swapLine, swapUnderWay, type EngineSwap } from "../../shared/engine-swap";
import { waitForPromptWrites } from "../moves/admission";
import { releaseWaiting } from "../engine/queue";
import { HELD, heldRelease, keepRelease } from "./fleet";
import { layoutHold, RELEASE_CHOICE, releaseChoice, releaseSetup } from "./choice";
import { appendUpdateEvent } from "./journal";
import { requestFleetRetry } from "./progress";

// The releases' public keys (scripts/set-box-key.sh, scripts/sign-box.mjs), base64: with none, no
// box updates itself.
export const UPDATE_KEYS = ["iLSFoygG8wg+r2yj+/qnoMHpZIhPKRNLlf9vpEKmx0A="];
const UPDATE_URL = "https://github.com/serhatandic/kuvo-code/releases/latest/download/boxes.json";
const UPDATE_STATE = path.join(RUN, "update.json");
const UPDATE_TRIAL = path.join(RUN, "update-trial");
/** An update is here on trial: until it's confirmed, the keeper may yet put the version before back. */
export const updateOnTrial = (): boolean => fs.existsSync(UPDATE_TRIAL);
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
// How long a release that was offered rests before it's taken again: one that was put in place and
// didn't come up (taken back), a day; one that failed before anything changed (the engine's
// download offline, say), an hour the first time, then a day (one whose own checks fail every
// time isn't downloaded again every hour).
const OFFER_REST = 86_400_000;
const OFFER_RETRY = 3_600_000;
// A release offered from a folder on this computer (`kuvo/box/update`), kept until it's taken.
const OFFERED = path.join(UPDATES, "offered");
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

// Whether `version` is the same version as `than` (both versions).
export function sameVersion(version: string | null | undefined, than: string | null | undefined): boolean {
  const a = versionOrder(version), b = versionOrder(than);
  return !!a && !!b && a.every((part, i) => part === b[i]);
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
  return manifest;
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
type ReleaseSaveOutcome = UpdateResultBody & SaveEnvelope;
export type UpdateOutcome = ReleaseSaveOutcome | (UpdateResultBody & { save?: never });

function withReleaseSave(outcome: UpdateResultBody, save: SetupSaveReceipt): ReleaseSaveOutcome {
  return { ...outcome, save };
}

function waitingForReleaseSave(version: string, save: SetupSaveReceipt): ReleaseSaveOutcome {
  return { result: "waiting", version, message: save.status === "interrupted" ? INTERRUPTED_SAVE : save.message, save };
}

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
  releaseSave?: { version: string; sha256: string; save: SetupSaveReceipt };
  /** The release check's round it last looked in (`releaseRound`). */
  round?: string;
  /** A release offered from a folder here that failed as it was taken: its version, when, whether
   * it was put in place and taken back, and how many times in a row it failed. */
  offerFailed?: { version: string; at: number; takenBack: boolean; tries?: number; message?: string; retryNow?: true; sha256?: string };
  fleetFailedAt?: { version: string; sha256?: string; at: number };
  /** How the last change of engine went (`updates/engine-swap.ts`). */
  engine?: import("./engine-swap").EngineChangeLast;
}

/** A change to `run/update.json`: each field given, null to take it away. */
export type UpdateChange = { [Key in keyof UpdateState]?: UpdateState[Key] | null };

export const updateState = (): UpdateState => { try { return JSON.parse(fs.readFileSync(UPDATE_STATE, "utf8")) ?? {}; } catch { return {}; } };
export function noteUpdate(change: UpdateChange): UpdateState {
  const state = { ...updateState(), ...change };
  for (const [key, value] of Object.entries(change)) if (value == null) delete state[key as keyof UpdateState];
  try { writePrivate(UPDATE_STATE, JSON.stringify(state)); } catch {}
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

/** Why this folder cannot update itself. Client versions are checked on connection. */
export function updatesBlocked(_link?: LinkState | null, _now?: number, root = K): string | null {
  return path.resolve(root) === path.join(homeFolder(), ".kuvo-code") ? null : "Only a machine's own Kuvo Code folder updates itself.";
}

// Whether something runs on the box: a conversation working (the engine's, the coding agents'),
// or an agent's browser waiting on you.
export async function boxBusy(readEngine: () => EngineRecord = readEngineFile, readBridge: () => BridgeRecord = readBridgeFile): Promise<boolean> {
  return await agentsWorking(readEngine, readBridge) === true || readAsks().length > 0;
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


export const hexSHA256 = (data: Buffer): string => crypto.createHash("sha256").update(data).digest("hex");

// `command` with `args`, in the box's folder, its environment the bridge's: its exit status and
// output, within `timeout` ms.
function ran(command: string, args: string[], env: Record<string, string> = {}, timeout = 120_000): { status: number | null; output: string } {
  const result = spawnSync(command, args, { cwd: K, env: { ...process.env, ...env }, encoding: "utf8", timeout });
  return { status: result.status, output: `${result.stdout ?? ""}${result.stderr ?? ""}`.trim() };
}


export const UPDATING_ALREADY = "It's updating already.";
/** Why no update goes now: the last one is still on trial (until it's said to be well, or taken back). */
export const ON_TRIAL = "Its last update is still being checked: try again in a few minutes.";

/** An update's outcome that says nothing of how updates go (something at work, or the last update
 * still on trial): it neither replaces how the last one went nor ends the round's look. */
export const passingOutcome = (outcome: UpdateOutcome | null | undefined): boolean => !!outcome && (outcome.result === "busy" || outcome.message === ON_TRIAL);

// An update under way, by its process (`run/.updating/pid`): one at a time, and one whose process
// went isn't one.
export function isUpdating(): boolean {
  return updateUnderWay(RUN);
}

export function takeUpdateLock(): boolean {
  try {
    fs.mkdirSync(UPDATE_LOCK);
  } catch {
    if (isUpdating()) return false;
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
    try { fs.mkdirSync(UPDATE_LOCK); } catch { return false; }
  }
  fs.writeFileSync(path.join(UPDATE_LOCK, "pid"), String(process.pid), { mode: 0o600 });
  return true;
}

// Keeps what's there now of each of a release's files, with `restore.sh`, which puts them back: run
// by the update when its swap fails midway, or once its judge finds it unwell (`kuvo-start
// rollback`; not while an update runs). Each step checked: one that fails leaves what was kept, for
// the next try. Code only: the engine's data is never put back (what was written since would be
// lost), and nothing at work is stopped: each piece takes the code put back as it takes new code.
// With a newer engine (`engine`, the release's), once that engine was put in place and started
// (`ENGINE_STARTED`, which the update notes as it starts it), the engine stays the new one, as the
// older engine may not read what the newer wrote: the keeper put back starts it (its `V=`). Before
// that, the older engine never ran on anything newer, and goes back with the rest: the newer one
// stopped first, the older started after. A keeper put back from before the keeper kept the coding
// agents' bridge itself (one without `bridge)`) stops the engine and its bridge and starts them
// again, as it always did: that bridge only ever starts with its engine.
function keepForRollback(files: string[], engine: string | null, bundles: ReadonlySet<string>, keeper: string): void {
  fs.rmSync(ROLLBACK, { recursive: true, force: true });
  fs.mkdirSync(path.join(ROLLBACK, "files"), { recursive: true, mode: 0o700 });
  const guard = /^if \[ "\$\{1:-\}" = check-rollback-layout \]/m.test(fs.readFileSync(keeper, "utf8"));
  let checkLayout = 'test ! -e "$K/folder-version.json" && test ! -L "$K/folder-version.json"';
  if (guard) {
    fs.copyFileSync(keeper, path.join(ROLLBACK, "layout-guard.sh"));
    checkLayout = 'sh "$B/layout-guard.sh" check-rollback-layout "$K"';
  } else if (fs.lstatSync(path.join(K, "folder-version.json"), { throwIfNoEntry: false })) {
    // A release with an older keeper can restore only the layout that existed before the trial.
    fs.copyFileSync(path.join(K, "folder-version.json"), path.join(ROLLBACK, "folder-version.json"));
    checkLayout = 'cmp -s "$K/folder-version.json" "$B/folder-version.json"';
  }
  const lines = [
    "#!/bin/sh",
    `K=${quote(K)}; B=${quote(ROLLBACK)}`,
    `p=$(cat "$K/run/.updating/pid" 2>/dev/null)`,
    `if [ -n "$p" ] && [ "$p" != "\${KUVO_UPDATER:-}" ] && kill -0 "$p" 2>/dev/null && { if [ -d /proc ]; then grep -q kuvo-link "/proc/$p/cmdline" 2>/dev/null; else ps -ww -o command= -p "$p" 2>/dev/null | grep -q kuvo-link; fi; }; then exit 0; fi`,
    `{ ${checkLayout}; } || exit 1`,
    `halt=; if ! grep -q '^bridge)' "$B/files/bin/kuvo-start" 2>/dev/null; then halt=stop; elif ${engine ? `[ ! -f "$B/engine-started" ]` : "false"}; then halt=engine-stop; grep -q '^engine-stop)' "$K/bin/kuvo-start" 2>/dev/null || halt=stop; fi`,
    `if [ -n "$halt" ]; then sh "$K/bin/kuvo-start" "$halt" >/dev/null 2>&1 || exit 1; fi`,
  ];
  for (const file of files) {
    const from = path.join(K, file);
    if (bundles.has(file)) {
      // A bundle, whole: the one there put back aside the new, which goes (none there: it goes).
      const [here, fresh, old] = [`"$K/"${quote(file)}`, `"$K/"${quote(`${file}.new`)}`, `"$K/"${quote(`${file}.old`)}`];
      if (fs.existsSync(from)) {
        fs.mkdirSync(path.dirname(path.join(ROLLBACK, "files", file)), { recursive: true, mode: 0o700 });
        fs.cpSync(from, path.join(ROLLBACK, "files", file), { recursive: true, verbatimSymlinks: true });
        lines.push(`rm -rf ${fresh} ${old} && cp -Rp "$B/files/"${quote(file)} ${fresh} && { [ ! -e ${here} ] || mv ${here} ${old}; } && mv ${fresh} ${here} && rm -rf ${old} || exit 1`);
      } else {
        lines.push(`rm -rf ${here} ${fresh} || exit 1`);
      }
    } else if (fs.existsSync(from)) {
      fs.mkdirSync(path.dirname(path.join(ROLLBACK, "files", file)), { recursive: true, mode: 0o700 });
      fs.copyFileSync(from, path.join(ROLLBACK, "files", file));
      const mode = fs.statSync(from).mode & 0o777;
      fs.chmodSync(path.join(ROLLBACK, "files", file), mode);
      const put = `cp -p "$B/files/"${quote(file)} "$K/"${quote(file)}.new && mv -f "$K/"${quote(file)}.new "$K/"${quote(file)} || exit 1`;
      if (engine && file === "bin/kuvo-start") {
        lines.push(`if [ -f "$B/engine-started" ]; then sed 's/^V=.*/V=${engine}/' "$B/files/bin/kuvo-start" >"$K/bin/kuvo-start.new" && chmod ${mode.toString(8)} "$K/bin/kuvo-start.new" && mv -f "$K/bin/kuvo-start.new" "$K/bin/kuvo-start" || exit 1; else ${put}; fi`);
      } else {
        lines.push(put);
      }
    } else {
      lines.push(`rm -f "$K/"${quote(file)} || exit 1`);
    }
  }
  const installed = path.join(UPDATES, "installed.json");
  if (fs.existsSync(installed)) {
    fs.copyFileSync(installed, path.join(ROLLBACK, "installed.json"));
    lines.push('cp -p "$B/installed.json" "$K/updates/installed.json.new" && mv -f "$K/updates/installed.json.new" "$K/updates/installed.json" || exit 1');
  } else lines.push('rm -f "$K/updates/installed.json" || exit 1');
  lines.push(`if [ -n "$halt" ]; then sh "$K/bin/kuvo-start" start >/dev/null 2>&1; else sh "$K/bin/kuvo-start" box >/dev/null 2>&1; fi`, `rm -f "$K/run/update-trial" "$K/run/update-restore"`, `rm -rf "$B"`);
  writePrivate(path.join(ROLLBACK, "restore.sh"), lines.join("\n") + "\n");
}

// Noted in what an update kept as it starts a newer engine (put in place): from then on, a rollback
// keeps that engine (`keepForRollback`).
const ENGINE_STARTED = path.join(ROLLBACK, "engine-started");

/** Before the update on trial is put back: a newer engine it started that never answered and wrote
 * nothing to the data since it was copied (`trial.dataKept`) goes back with the rest, the old engine
 * reading only what it wrote itself. */
export function newEngineUnused(trial: UpdateTrial): void {
  if (trial.dataKept !== undefined && dataNow() === trial.dataKept) fs.rmSync(ENGINE_STARTED, { force: true });
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

/** How `update` runs: now (Update Now) or by its release check, from a folder on this computer (`from`, with
 * `boxes.json` and the pack it names) rather than GitHub, and for a test, its keys, address,
 * block, clock, steps and how long the new version has to come up. */
export interface UpdateOptions {
  now?: boolean;
  from?: string;
  keys?: string[];
  url?: string;
  blocked?: string | null;
  clock?: () => number;
  step?: (running: UpdateStep) => void;
  /** The features the box says now (`kuvo/info`): what the new files must still say. */
  features?: string[];
  /** Told as it starts putting the new version in place (from there, a failure is taken back). */
  swapping?: () => void;
  /** Told the version of the release it takes, once it's checked its signature. */
  taking?: (version: string) => void;
  /** Goes ahead whatever is at work (the default with `now`, Update Now, which a client asks
   * first); false: `now` only for a release's age. */
  whileBusy?: boolean;
}

/** A release's `box.json`, in its pack: its version, its engine's, each file's SHA-256, and its
 * Mac bundles' (`bin/<name>.app`), each file by its path in the bundle. */
export interface BoxFiles {
  version: string;
  engine: string;
  files: Record<string, string>;
  bundles?: Record<string, Record<string, string>>;
  /** The engines its link and bridge were checked with as it was released: its own, then the one
   * the release before ran (N-1). A box running one of them takes the code first and its engine
   * later, at a quiet moment (`engine-swap.ts`). */
  engines?: string[];
  /** Whether the engine before (`engines[1]`) opens what this one wrote, as its release checked:
   * a new engine that doesn't start may go back to it. */
  downgradeSafe?: boolean;
}

/** Whether release `box` changes the engine apart from its code: another engine than the one running,
 * which its release checked its link and bridge with (`engines`). */
const engineApart = (box: Pick<BoxFiles, "engine" | "engines">, running: string | null): boolean =>
  !!running && running !== box.engine && Array.isArray(box.engines) && box.engines.includes(running);

// Whether a release's files (its `box.json`) aren't all what's in place here: one of them missing,
// or another, by its SHA-256 (a Mac's bundles too, on a Mac). A release of the box's own version
// whose files are the same is the one it has; one whose files differ (an app rebuilt under the same
// version) is taken.
function differsHere(box: BoxFiles, root = K): boolean {
  const here = (file: string, sha: unknown): boolean => {
    try { return fs.lstatSync(file).isFile() && hexSHA256(fs.readFileSync(file)) === sha; } catch { return false; }
  };
  if (Object.entries(box.files ?? {}).some(([file, sha]) => !here(path.join(root, file), sha))) return true;
  if (process.platform !== "darwin" || !isObject(box.bundles)) return false;
  return Object.entries(box.bundles).some(([bundle, listed]) => isObject(listed) && Object.entries(listed).some(([inner, sha]) => !here(path.join(root, bundle, inner), sha)));
}

// The `box.json` in a release's `pack` (checked against its manifest already), unpacked in a folder
// of its own that goes after; it throws for a pack that has none, or not for `version`.
function packedLinkLayout(pack: Buffer): number {
  for (const name of ["./bin/kuvo-link.js", "bin/kuvo-link.js"]) {
    const read = spawnSync("sh", ["-c", 'tar -xzOf - "$1" 2>/dev/null | head -c 96', "sh", name], { input: pack, timeout: 120_000 });
    if (read.stdout?.length) return Number(/^\/\/ Kuvo folder layout ([1-9][0-9]*) [0-9a-f]{12}\n/.exec(read.stdout.toString("utf8", 0, 96))?.[1] ?? 0);
  }
  return 0;
}

export function packedBox(pack: Buffer, version: string): BoxFiles {
  // (Only `box.json`, read from tar's output: nothing unpacked. Packed from a folder, `./box.json`.)
  for (const name of ["./box.json", "box.json"]) {
    const read = spawnSync("tar", ["-xzOf", "-", name], { input: pack, maxBuffer: 10_000_000, timeout: 120_000 });
    if (read.status !== 0 || !read.stdout?.length) continue;
    const box: BoxFiles = JSON.parse(read.stdout.toString("utf8"));
    if (box?.version !== version || !isObject(box.files)) throw new Error("not this release");
    return box;
  }
  throw new Error("no box.json");
}

// Whether the last `update` failed in a way that's passing, before anything changed: a download
// that broke off (the release, its files or its engine), or its engine that couldn't be stopped
// (a start of it under way): what the installer's update tries once more by itself (`main.ts`),
// unlike a check that would fail again (what it got isn't what it says, no room, a self-test).
let brokeOff = false, stopFailed = false;
export const failedInPassing = (): boolean => brokeOff || stopFailed;

// The engine `pid` names (its record, as the update found it), when its command line says it's this
// box's: ended, as the keeper's stop ends it (ten seconds to go, then killed), and waited for.
async function stopEngine(pid: number): Promise<void> {
  if (!runsAs(pid, engineRole(K))) return;
  const gone = async (within: number): Promise<boolean> => {
    for (const began = Date.now(); Date.now() - began < within; await sleep(100)) if (!alive(pid)) return true;
    return !alive(pid);
  };
  try { process.kill(pid, "SIGTERM"); } catch {}
  if (await gone(10_000)) return;
  // (Still the engine: not a process that took its number as it went.)
  if (runsAs(pid, engineRole(K))) try { process.kill(pid, "SIGKILL"); } catch {}
  await gone(5_000);
}

// Updates the box to the newest release, when it's newer: `now` (Update Now) takes it whatever its
// age. What happened: `{result: "updated" | "current" | "waiting" | "busy" | "off" | "blocked" |
// "failed", version?, message?}`; once "updated", the bridge goes for the keeper to start the new
// one. `from`: a release in a folder here, taken whatever its age (the Kuvo Code it came with is
// here already). `keys` and `url`: a test's (a test's folder only).
function supersedes(manifest: BoxRelease, choice: { version: string; releasedAt: number }): boolean {
  return newerVersion(manifest.version, choice.version) || (sameVersion(manifest.version, choice.version)
    && (Date.parse(manifest.released ?? "") || 0) > choice.releasedAt);
}

function offerSupersedes(choice: { version: string; releasedAt: number }, keys: string[]): boolean {
  try { return supersedes(verifyBoxes(fs.readFileSync(path.join(OFFERED, "boxes.json"), "utf8"), keys), choice); }
  catch { return false; }
}

export async function update({ now = false, from, keys = UPDATE_KEYS, url = UPDATE_URL, blocked = updatesBlocked(), clock = Date.now, step = () => {}, features, swapping = () => {}, taking = () => {}, whileBusy = now }: UpdateOptions = {}): Promise<UpdateOutcome> {
  if (!keys.length) return { result: "off", message: "Updates aren't set up for this machine yet." };
  if (blocked) return { result: "blocked", message: blocked };
  // COMPAT(0.7 engine-stop): no update while an older Mac's Stop holds the engine down. Goes on the user's word, once no Mac from before 1.0 connects.
  if (fs.existsSync(path.join(RUN, "stopped"))) return { result: "blocked", message: "Its engine is stopped: start it to update it." };
  // Never over an update still on trial: what it kept to go back to would be lost.
  if (updateOnTrial()) return { result: "blocked", message: ON_TRIAL };
  // Nor while its engine is being changed (the old one stopped, the new one starting): a moment.
  const changing = swapUnderWay(RUN);
  if (changing && (changing.stage === "swapping" || changing.stage === "back")) return { result: "busy", message: "Its engine is being updated: try again in a few minutes." };
  const before = boxVersion();
  if (!before) return { result: "blocked", message: "This machine's version isn't known: connect to it from Kuvo Code on your Mac once." };
  if (!takeUpdateLock()) return { result: "busy", message: UPDATING_ALREADY };
  let engineHold: EngineSwap | undefined;
  let trial = false, stopped = false, swapped = false, stage: string | null = null;
  let releaseSave: SetupSaveReceipt | undefined;
  let recoveredReleaseSave: SetupSaveReceipt | undefined;
  let attempt: BoxRelease | undefined;
  const releaseResult = (outcome: UpdateOutcome): UpdateOutcome => {
    if (attempt && from !== OFFERED && (outcome.result === "failed" || outcome.result === "blocked")) {
      const reason = outcome.message ?? "Kuvo couldn’t install the release.";
      const { version, pack } = attempt;
      appendUpdateEvent(version, outcome.result, reason);
      if (releaseChoice()?.sha256 === pack.sha256) {
        recordOfferFailure(version, reason, swapped && boxVersion() === before && !updateOnTrial() && !fs.existsSync(ROLLBACK), clock(), pack.sha256);
      }
    }
    const result = outcome.save || !releaseSave ? outcome : withReleaseSave(outcome, releaseSave);
    if (recoveredReleaseSave && result.save === recoveredReleaseSave) noteUpdate({ releaseSave: null });
    return result;
  };
  // The engine's data as it was copied before a newer engine ran (`keepData`): a newer engine that
  // wrote nothing to it by the time a failed swap is put back goes back with the rest.
  let dataKept: string | null = null;
  // The box's keeper, as the update runs it (its stop, its start, putting back what was there): with
  // HOME spelled as the running engine was started with, so a keeper of any age knows that engine
  // as its own, never starting a second beside it (Kuvo Helper gave its keeper "/Users/you/", and
  // Kuvo Code runs this with "/Users/you").
  let keeperEnv: Record<string, string> = {};
  // The new engine, downloaded and checked: kept in its place when the update fails before anything
  // changed, so the next try (a moment later, or the next quiet moment) doesn't fetch it again.
  let checkedEngine: { from: string; to: string } | null = null;
  let fetching = false;
  brokeOff = stopFailed = false;
  const download = async <T>(get: () => Promise<T>): Promise<T> => {
    fetching = true;
    const got = await get();
    fetching = false;
    return got;
  };
  try {
    step("checking");
    let setup = releaseSetup();
    if (setup) whileBusy = false;
    const kept = updateState().releaseSave;
    let previousSave: SetupSaveReceipt | undefined;
    if (kept && kept.save.setup === setup?.id) {
      previousSave = kept.save;
      if (previousSave.status === "pending") {
        const known = await askSetupOutcome(RUN, previousSave, RELEASE_CHOICE);
        if (known.receipt && known.receipt.status !== "pending") {
          previousSave = known.receipt;
          // A retry can fail before it has checked which release it is taking.
          noteUpdate({ releaseSave: previousSave.status === "committed" ? { ...kept, save: previousSave } : null });
        }
      }
      if (!from && previousSave.status !== "committed") return waitingForReleaseSave(kept.version, previousSave);
      if (previousSave.status === "committed") setup = releaseSetup();
    }
    const choice = releaseChoice();
    let released: Buffer;
    if (!from && setup && !setup.primary) return { result: "waiting", message: "This computer follows the release chosen by the main computer." };
    if (from) released = readRelease(path.join(from, "boxes.json"), 1_000_000);
    else try {
      released = await download(() => fetchedPatiently(url, 1_000_000, 30_000));
    } catch (error) {
      // COMPAT(0.7 no-boxes-json): a newest release without `boxes.json` (0.7.2) is "current". Goes once a release with one is the newest.
      // The newest release has none for machines (one from before them): nothing newer for it.
      if (/answered 404/.test(String((error as Error | undefined)?.message))) return { result: "current", version: before };
      throw error;
    }
    const manifest = attempt = verifyBoxes(released.toString("utf8"), keys);
    const alreadyChosen = choice?.version === manifest.version && choice?.sha256 === manifest.pack.sha256;
    if (previousSave && kept?.version === manifest.version && kept.sha256 === manifest.pack.sha256) {
      if (previousSave.status === "committed") recoveredReleaseSave = previousSave;
      if (previousSave.status !== "committed" || !alreadyChosen) return releaseResult(waitingForReleaseSave(manifest.version, previousSave));
      releaseSave = previousSave;
    }
    if (setup && choice && !alreadyChosen && !supersedes(manifest, choice))
      return { result: "waiting", version: manifest.version, message: "The setup already selected a release at least as new." };
    if (setup && manifest.spreads !== true) return { result: "blocked", version: manifest.version, message: "This release is for one computer and cannot be selected for a setup." };
    if (setup && !setup.primary && !alreadyChosen) return { result: "waiting", message: "Your main computer has selected a different release." };
    taking(manifest.version);
    // (Only GitHub's says what the newest release is.)
    if (!from) noteUpdate({ latest: manifest.version, checked: clock() });
    // Never an older version. The same one from a folder here (the release an app carries, rebuilt
    // under its version) is taken when its files aren't the ones in place (below); GitHub's is the
    // one this version came from.
    const again = (!!from || !!setup) && sameVersion(manifest.version, before);
    if (!from && alreadyChosen && !needsRelease({ version: manifest.version, sha256: manifest.pack.sha256 }, before))
      return releaseResult({ result: "current", version: before });
    if (!from && alreadyChosen && offerRests(manifest.version, clock(), manifest.pack.sha256))
      return releaseResult({ result: "waiting", version: manifest.version, message: "The selected release is waiting to retry." });
    if (!newerVersion(manifest.version, before) && !again) return releaseResult({ result: "current", version: before });
    // (Its files read from the folder's pack, before anything's said to be under way.)
    const unchanged = again && !!from && !folderDiffers(from, manifest);
    if (unchanged && (!setup || (alreadyChosen && installedPack() === manifest.pack.sha256))) return releaseResult({ result: "current", version: before });
    // Never an older engine than the one running: it may not read what the newer one wrote.
    const behind = engineBehind(manifest.engine);
    if (behind) return releaseResult({ result: "blocked", message: behind });
    if (!now && !from && clock() - (Date.parse(manifest.released!) || clock()) < RELEASE_AGE) return releaseResult({ result: "waiting", version: manifest.version, message: "It takes a release a few days after it's out." });
    // Only a release with another engine that its code wasn't checked with waits for nothing to be at
    // work (that engine is started in the one running's place with its code); or now, on your word:
    // a client asks you first, saying what stops. One whose code was checked with the engine running
    // stops nothing: its engine changes later, at a quiet moment (`engine-swap.ts`). (Which, its
    // `box.json` says: looked at once it's here.)
    const running = runningEngineVersion();
    const engineChanges = running !== manifest.engine;

    step("downloading");
    stage = path.join(UPDATES, manifest.version);
    fs.rmSync(stage, { recursive: true, force: true });
    fs.mkdirSync(path.join(stage, "files"), { recursive: true, mode: 0o700 });
    const pack = from ? readRelease(path.join(from, packName(manifest)), 100_000_000) : await download(() => fetchedPatiently(manifest.pack.url, 100_000_000, 300_000));
    if (hexSHA256(pack) !== manifest.pack.sha256) throw new Error("The release's files aren't what it says.");
    fs.writeFileSync(path.join(stage, "pack.tgz"), pack, { mode: 0o600 });
    if (ran("tar", ["xzf", path.join(stage, "pack.tgz"), "-C", path.join(stage, "files")]).status !== 0) throw new Error("The release's files couldn't be unpacked.");
    const box: BoxFiles = JSON.parse(fs.readFileSync(path.join(stage, "files", "box.json"), "utf8"));
    if (box?.version !== manifest.version || box.engine !== manifest.engine || !isObject(box.files)) throw new Error("The release's files aren't what it says.");
    const layout = installedLinkBuild(path.join(stage, "files"))?.layout ?? 0;
    if (manifest.layout !== undefined && manifest.layout !== layout) throw new Error("The release’s folder layout doesn’t match its manifest.");
    const checkLayout = () => layoutHold(layout);
    const beforePublishing = checkLayout();
    if (beforePublishing) return releaseResult({ result: "blocked", version: manifest.version, message: beforePublishing });
    const authority = releaseSetup();
    if (authority?.id !== setup?.id || authority?.epoch !== setup?.epoch || authority?.sequence !== setup?.sequence) {
      return releaseResult({ result: "waiting", version: manifest.version, message: "The setup changed while this release was being checked." });
    }
    // A candidate pack can wait on disk. The fleet advertises only the accepted choice.
    const keptPack = keepRelease(released, pack);
    if (setup && !alreadyChosen) {
      if (!keptPack || heldRelease(keys)?.sha256 !== manifest.pack.sha256) throw new Error("Couldn’t retain the signed release for the other computers.");
      const response = await askSetupSave(RUN, {
        kind: RELEASE_CHOICE, id: setup.id, epoch: setup.epoch, sequence: setup.sequence,
        value: { version: manifest.version, sha256: manifest.pack.sha256, signed: released.toString("base64"), layout, clientProtocol: manifest.clientProtocol },
      });
      const save = releaseSave = response.receipt;
      if (save?.status === "pending") noteUpdate({ releaseSave: { version: manifest.version, sha256: manifest.pack.sha256, save } });
      if (response.status !== 200) {
        if (save) return waitingForReleaseSave(manifest.version, save);
        return releaseResult({ result: "waiting", version: manifest.version, message: response.message });
      }
      if (from !== OFFERED && offeredPack() !== manifest.pack.sha256
        && !offerSupersedes({ version: manifest.version, releasedAt: Date.parse(manifest.released ?? "") || 0 }, keys)) dropOffer();
    }
    const apart = engineChanges && engineApart(box, running);
    const busy = (): UpdateOutcome => {
      const message = "Something is running there now.";
      noteUpdate({ engineBusy: { version: manifest.version, message } });
      return releaseResult({ result: "busy", version: manifest.version, message });
    };
    if (!whileBusy && engineChanges && !apart && (await agentsWorking() !== false || readAsks().length > 0)) return busy();
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
      const tarball = await download(() => fetchedPatiently(engineURL, 400_000_000, 900_000));
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
    if (staged) checkedEngine = { from: staged, to: path.join(engineFolder, "opencode") };
    const tried = ran(engine, ["--config=/dev/null", "--no-env-file", path.join(stage, "files", "bin", "kuvo-link.js"), "selftest"], { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K });
    if (tried.status !== 0 || !tried.output.includes("ok")) throw new Error("The new link doesn't work with it here.");
    // Its engine changing later, the new link runs on the engine at work meanwhile: tried with it too.
    if (apart) {
      const before = path.join(K, "engine", running!, "opencode");
      const triedBefore = fs.existsSync(before) ? ran(before, ["--config=/dev/null", "--no-env-file", path.join(stage, "files", "bin", "kuvo-link.js"), "selftest"], { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K }) : null;
      if (triedBefore && (triedBefore.status !== 0 || !triedBefore.output.includes("ok"))) throw new Error("The new link doesn't work with the engine at work here.");
    }
    if (ran("sh", ["-n", path.join(stage, "files", "bin", "kuvo-start")]).status !== 0) throw new Error("The new keeper doesn't read.");

    if (unchanged) {
      writePrivate(path.join(UPDATES, "installed.json"), JSON.stringify({ version: box.version, files: box.files, engine: box.engine, engines: box.engines, downgradeSafe: box.downgradeSafe, sha256: manifest.pack.sha256 }));
      return releaseResult({ result: "current", version: before });
    }

    // What's there kept; then the new in its place, one file at a time.
    step("installing");
    // Finish any remote backup before reading the engine and fencing new turns.
    await (await import("../backup/engine")).backupBefore("update", { insideUpdate: true });
    let previous: Partial<EngineRecord> = {};
    try { previous = readEngineFile(); } catch {}
    const home = previous?.pid ? engineHome(previous.pid, K) : undefined;
    if (home) keeperEnv = { HOME: home };
    // (An engine that changes apart from the code isn't stopped here: the swapper changes it later.)
    const otherEngine = previous?.version !== manifest.engine && !apart;
    if (otherEngine && !roomForData()) throw new Error("There isn't room on it to keep its data while it updates.");
    if (otherEngine) {
      const holding = Date.now();
      engineHold = { from: previous.version ?? running ?? "", to: manifest.engine, since: holding, stage: "holding", holding, pid: process.pid };
      writePrivate(installHoldFile(RUN), JSON.stringify(engineHold));
      await waitForPromptWrites();
      if (!whileBusy && (await agentsWorking() !== false || readAsks().length > 0)) return busy();
    }
    // What's there kept, and how to put it back, before anything changes: a swap the update doesn't
    // get past (the machine going down) is put back too.
    const latestSetup = releaseSetup();
    const latestChoice = releaseChoice();
    if (latestSetup?.id !== setup?.id || latestSetup?.epoch !== setup?.epoch
      || (latestSetup && (latestChoice?.version !== manifest.version || latestChoice.sha256 !== manifest.pack.sha256))) {
      return releaseResult({ result: "waiting", version: manifest.version, message: "The main computer changed the release while this update was preparing." });
    }
    const heldLayout = checkLayout();
    if (heldLayout) return releaseResult({ result: "blocked", version: manifest.version, message: heldLayout });
    const newerEngine = otherEngine && newerVersion(manifest.engine, previous?.version);
    keepForRollback(files, newerEngine ? manifest.engine : null, new Set(bundles), path.join(stage, "files", "bin", "kuvo-start"));
    noteUpdate({ trial: { from: before, to: manifest.version, sha256: manifest.pack.sha256, at: clock(), installing: true }, running: null });
    // (A rollback's count from a trial an older Kuvo ended: never this one's.)
    fs.rmSync(path.join(RUN, "update-restore"), { force: true });
    writeDurablePrivate(UPDATE_TRIAL, manifest.version);
    trial = true;
    // Nothing at work is stopped: each piece takes the new code as it takes any (PROTOCOL.md,
    // "Updates"). Only another engine than the one running is: that one alone, its coding agents'
    // bridge running on, and only once nothing works there or on your word (above).
    if (engineHold) {
      writePrivate(installHoldFile(RUN), JSON.stringify({ ...engineHold, stage: "swapping" }));
      stopped = true;
      noteUpdate({ trial: { from: before, to: manifest.version, sha256: manifest.pack.sha256, at: clock(), installing: true, engineStopped: true } });
      const halt = /^engine-stop\)/m.test(textOf(path.join(K, "bin", "kuvo-start")) ?? "") ? "engine-stop" : "stop";
      const stop = ran("sh", [path.join(K, "bin", "kuvo-start"), halt], keeperEnv, 300_000);
      // COMPAT(beta.30 engine-record): an engine an older keeper's stop missed, stopped by its record. Goes once no box is from before beta.31.
      // (Its HOME not told, its command line unread: stopped here by that record, once its command
      // line says it's this box's engine.)
      if (stop.status === 0 && previous?.pid) await stopEngine(previous.pid);
      if (stop.status !== 0 || (previous?.pid && alive(previous.pid))) {
        stopFailed = true;
        throw new Error(`Kuvo's engine on ${HERE} couldn't be stopped for the update, so nothing changed. Try again in a few minutes.`);
      }
      // (A copy of its data, never put back by anything: for you to restore by hand, as a last resort.)
      dataKept = keepData(`${new Date(clock()).toISOString().slice(0, 10)}-${previous?.version ?? "unknown"}-to-${manifest.engine}`);
    }
    // (Its engine changing later: the one running noted as the one that ran, for the keeper to start
    // it again should it end before the swapper changes it, never the new one on its own.)
    if (apart && running) writePrivate(path.join(RUN, "engine-ran"), `${running}\n`);
    swapped = true;
    const swappedAt = Date.now();
    swapping();
    if (staged) {
      fs.mkdirSync(engineFolder, { recursive: true, mode: 0o700 });
      fs.renameSync(staged, path.join(engineFolder, "opencode"));
    }
    for (const file of files) {
      const from = path.join(stage, "files", file), to = path.join(K, file);
      fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
      if (bundles.includes(file)) {
        swapBundle(from, to);
        continue;
      }
      fs.copyFileSync(from, `${to}.new`);
      fs.chmodSync(`${to}.new`, fs.statSync(from).mode & 0o777);
      fs.renameSync(`${to}.new`, to);
    }
    // (A newer engine about to run on the data: from here, a rollback keeps it.)
    if (newerEngine) writePrivate(ENGINE_STARTED, manifest.engine);
    // (Its engine started with its code: the keeper starts its own, not the one that ran before.)
    if (otherEngine) fs.rmSync(path.join(RUN, "engine-ran"), { force: true });
    // The engine started again where it was stopped; otherwise only its keeper, should it have gone
    // (it starts itself again on the new code, as `serve`, `run` and the bridge each do).
    ran("sh", [path.join(K, "bin", "kuvo-start"), otherEngine ? "start" : "box"], keeperEnv);
    step("starting");
    // What it put there, for telling later whether an older Mac put its own back (`checkDrift`).
    // (Not its bundles: the helper's own keeper may put its own back, which is never drift.)
    // (With the engines its code was checked with, and whether the one before opens what the new
    // writes: what the swapper goes back by, and what a confirm keeps.)
    writePrivate(path.join(UPDATES, "installed.json"), JSON.stringify({ version: box.version, sha256: manifest.pack.sha256, files: box.files, engine: box.engine, ...(Array.isArray(box.engines) ? { engines: box.engines.filter(each => typeof each === "string") } : {}), ...(box.downgradeSafe === true ? { downgradeSafe: true } : {}) }));
    // Update Now with an engine apart: it's changed as soon as the turns under way end, once the
    // code is judged well (the swapper holds at once, rather than waiting for a quiet moment).
    if (apart && now) writePrivate(SWAP_NOW, String(clock()));
    // On trial from here: judged by the one health rule (`updates/health.ts`), by this process once
    // it's done here, or by the judge the keeper starts should this one go first.
    noteUpdate({ trial: { from: before, to: manifest.version, sha256: manifest.pack.sha256, at: clock(), swapped: swappedAt, ...(features ? { features } : {}), ...(dataKept !== null ? { dataKept } : {}) }, engineBusy: null });
    return releaseResult({ result: "updated", version: manifest.version });
  } catch (error) {
    brokeOff = fetching;
    const message = String((error as Error | undefined)?.message ?? error).slice(0, 300);
    // (An engine of another version beside the one running changes nothing: the keeper starts its own.)
    if (!swapped && checkedEngine && fs.existsSync(checkedEngine.from) && !fs.existsSync(checkedEngine.to)) {
      try {
        fs.mkdirSync(path.dirname(checkedEngine.to), { recursive: true, mode: 0o700 });
        fs.renameSync(checkedEngine.from, checkedEngine.to);
      } catch {}
    }
    // Room first: what was downloaded goes before anything's put back.
    if (stage) fs.rmSync(stage, { recursive: true, force: true });
    if (swapped) {
      // A newer engine that never answered and wrote nothing to the data: it goes back too, the old
      // engine reading only what it wrote itself. (The keeper's own rollback, later, keeps the new.)
      // (Looked at before restore.sh stops it: a commit it lands in that moment goes unseen. A narrow
      // window, taken as it is: an engine that hasn't answered in two minutes is still starting.)
      if (dataKept !== null && dataNow() === dataKept) fs.rmSync(ENGINE_STARTED, { force: true });
      try {
        beginFolderRollback(K);
        ran("sh", [path.join(ROLLBACK, "restore.sh")], { ...keeperEnv, KUVO_UPDATER: String(process.pid) }, 600_000);
      } catch (restoreError) {
        console.error("kuvo-link: the keeper still needs to roll back this update", restoreError);
      }
    } else {
      // Nothing changed: what was kept goes, and the engine starts again if it was stopped.
      if (trial) {
        fs.rmSync(UPDATE_TRIAL, { force: true });
        fs.rmSync(ROLLBACK, { recursive: true, force: true });
      }
      if (stopped) ran("sh", [path.join(K, "bin", "kuvo-start"), "start"], keeperEnv);
    }
    if (trial && !updateOnTrial()) noteUpdate({ trial: null });
    noteUpdate({ last: { at: clock(), result: "failed", message, from: before } });
    return releaseResult({ result: "failed", message });
  } finally {
    if (engineHold) {
      fs.rmSync(installHoldFile(RUN), { force: true });
      try { await releaseWaiting(readEngineFile()); } catch {}
    }
    clearUpdateStaging();
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
}

// A bundle in place whole, from where it's staged (on the same disk): the new beside the one
// there, that one put aside, the new renamed in, the old gone. A program running from the old one
// runs on (its keeper restarts it).
function swapBundle(from: string, to: string): void {
  const fresh = `${to}.new`, old = `${to}.old`;
  fs.rmSync(fresh, { recursive: true, force: true });
  fs.rmSync(old, { recursive: true, force: true });
  fs.renameSync(from, fresh);
  if (fs.existsSync(to)) fs.renameSync(to, old);
  fs.renameSync(fresh, to);
  fs.rmSync(old, { recursive: true, force: true });
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
    const engines = JSON.parse(fs.readFileSync(path.join(UPDATES, "installed.json"), "utf8"))?.engines;
    return Array.isArray(engines) ? engines.filter((each): each is string => typeof each === "string" && /^[0-9][0-9A-Za-z.-]*$/.test(each)) : [];
  } catch { return []; }
}

// The version of the engine running here, as its record says (null: none noted).
function runningEngineVersion(): string | null {
  try { return readEngineFile()?.version ?? null; } catch { return null; }
}

/** Whether taking the release offered from a folder here would start another engine than the one
 * running (the one kind of update that waits for nothing to be at work); false where it can't be told. */
let offeredBox: { stamp: string; box: Pick<BoxFiles, "engine" | "engines"> | null } | null = null;
export function offerChangesEngine(): boolean {
  try {
    const given = JSON.parse(fs.readFileSync(path.join(OFFERED, "boxes.json"), "utf8"));
    const manifest = JSON.parse(Buffer.from(given.manifest, "base64").toString("utf8"));
    const running = runningEngineVersion();
    if (typeof manifest?.engine !== "string" || manifest.engine === running) return false;
    // (One whose code was checked with the engine running changes its engine later, stopping nothing.
    // Its `box.json` read from the pack once for each pack there.)
    const pack = path.join(OFFERED, packName(manifest));
    let stamp = "";
    try { const stat = fs.statSync(pack); stamp = `${pack} ${stat.size} ${stat.mtimeMs}`; } catch { return true; }
    if (offeredBox?.stamp !== stamp) {
      let box: Pick<BoxFiles, "engine" | "engines"> | null = null;
      try { box = packedBox(readRelease(pack, 100_000_000), manifest.version); } catch {}
      offeredBox = { stamp, box };
    }
    return !offeredBox.box || !engineApart(offeredBox.box, running);
  } catch { return false; }
}

// Why a release with engine `engine` isn't taken: the engine running here is newer (it may have
// written what an older one can't read); null when it isn't, or that can't be told.
export function engineBehind(engine: string): string | null {
  let running: string | null | undefined = null;
  try { running = readEngineFile()?.version; } catch {}
  return running && newerVersion(running, engine) ? `Its engine (${running}) is newer than the release's (${engine}), and it never goes back to an older engine.` : null;
}

// Whether the release in folder `from` (its `manifest` checked) has other files than those in place;
// one whose pack can't be read or isn't what it says differs (the update then says why).
function folderDiffers(from: string, manifest: BoxRelease): boolean {
  try {
    const pack = readRelease(path.join(from, packName(manifest)), 100_000_000);
    return hexSHA256(pack) !== manifest.pack.sha256 || differsHere(packedBox(pack, manifest.version));
  } catch { return true; }
}

// The name of a release's pack, as its address ends: in a folder here, it's beside `boxes.json`.
export function packName(manifest: Pick<BoxRelease, "pack">): string {
  let name = "";
  try { name = decodeURIComponent(path.posix.basename(new URL(manifest.pack.url).pathname)); } catch {}
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) throw new Error("That isn't a release.");
  return name;
}

/** What an update staged in `updates` taken away as it ends, whatever came of it: never what's kept
 * there for good (its rollback, what it installed, a release offered from a folder here, the copy
 * of the engine's data its last engine change kept (`backups`), and a move of this Mac's box, `rehome`: its state and kept originals, which `rehome/state.ts` keeps
 * there, the signed release it runs, which it hands to your other computers (`release`), and one
 * being pulled from one of them (`incoming`, `updates/fleet.ts`). */
export function clearUpdateStaging(folder = UPDATES): void {
  const kept = new Set(["rollback", "installed.json", "rehome", "offered", "backups", "release", "running", "previous-choice", "incoming"]);
  for (const name of listOf(folder)) if (!kept.has(name)) fs.rmSync(path.join(folder, name), { recursive: true, force: true });
}

// The new bridge, once an update put it there: well, the update's done. What was kept goes, and
// engines but this one.
export function confirmUpdate(clock = Date.now): boolean {
  const state = updateState();
  if (!state.trial?.healthy || boxVersion() !== state.trial.to || isUpdating()
    || fs.existsSync(path.join(RUN, "update-restore"))
    || runs(Number(textOf(path.join(RUN, ".rolling-back", "pid"))), "kuvo-start")) return false;
  let engine: string | null | undefined = null;
  try { engine = readEngineFile()?.version; } catch {}
  // (How it went noted first: whoever sees the trial over finds it said.)
  noteUpdate({ trial: null, last: { at: clock(), result: "updated", from: state.trial.from, to: state.trial.to }, ...(state.offerFailed && !newerVersion(state.offerFailed.version, state.trial.to) ? { offerFailed: null } : {}) });
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
  const installed = parsed(textOf(path.join(UPDATES, "installed.json")));
  return isObject(installed) && installed.version === boxVersion() && typeof installed.sha256 === "string" ? installed.sha256 : null;
}

export function needsRelease(choice: { version: string; sha256: string }, current = boxVersion()): boolean {
  return newerVersion(choice.version, current) || (sameVersion(choice.version, current) && installedPack() !== choice.sha256);
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
  try { installed = JSON.parse(fs.readFileSync(record, "utf8")); } catch { return false; }
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

// What `kuvo/update` says: whether updates are set up, this version and the newest heard of, how
// the last went, whether one runs now, why not, when it can't, and a release offered from a folder
// here that waits for a quiet moment.
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
  // (Counted with the failures of that version before it: twice, and Update Now is offered.)
  const at = Date.now();
  recordOfferFailure(trial.to, message, back, at, trial.sha256);
  noteUpdate({ trial: null, last: { at, result: "failed", message, from: trial.from, to: trial.to } });
  appendUpdateEvent(trial.to, "rollback", message, at);
}

export function updateStatus(): UpdateStatus {
  // (A trial something else ended meanwhile, a keeper giving up say: how it went noted.)
  try { noteTrialEnded(); } catch {}
  const state = updateState();
  const offered = offeredVersion();
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
    ...(offered ? { offered } : {}),
    // How the release offered from a folder here last went, when taking it failed: its own outcome,
    // apart from the release check's (`last`), which Kuvo Code here goes by (#679).
    ...(state.offerFailed ? { offerFailed: { version: state.offerFailed.version, at: state.offerFailed.at, takenBack: state.offerFailed.takenBack, ...(state.offerFailed.message ? { message: state.offerFailed.message } : {}) } } : {}),
    ...(swap ? { engineChange: { stage: swap.stage, from: swap.from, to: swap.to, line: swapLine(swap)! } } : {}),
    ...(state.engine ? { engineLast: state.engine } : {}),
  };
}

/** Waiting facts are read on the target computer, including sessions awaiting an approval. */
export async function updateStatusHere(readEngine = readEngineFile, readBridge = readBridgeFile): Promise<UpdateStatus> {
  const status = updateStatus();
  const state = updateState();
  const available = [status.offered, heldVersion(), status.latest, state.engineBusy?.version]
    .filter((version): version is string => newerVersion(version, status.version) || (version === status.offered && offerWaits()))
    .sort((a, b) => newerVersion(a, b) ? -1 : newerVersion(b, a) ? 1 : 0)[0];
  let bridge: BridgeRecord | null = null;
  try { bridge = readBridge(); } catch {}
  const to = bundleBuild(path.join(K, "bin", "kuvo-agents.js"));
  const bridgeBehind = bridge?.build && to && bridge.build !== to && (!bridge.pid || runs(bridge.pid, "kuvo-agents"));
  const switching = fs.existsSync(path.join(RUN, "agents", "handover.json"));
  const version = available ?? ((switching || bridgeBehind || status.engineChange || updateOnTrial()) ? status.version : null);
  if (!version) return status;

  type Waiting = NonNullable<UpdateStatus["waiting"]>;
  const facts = (sessionsAtWork: number | null): Pick<Waiting, "reason" | "detail"> => {
    if (status.blocked) return { reason: "blocked", detail: status.blocked };
    if (["stopped", "paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark)))) return { reason: "stopped", detail: "Start this computer’s box to update it." };
    if (status.updating) return { reason: "installing", detail: "Kuvo is installing the update." };
    if (available && state.offerFailed?.version === available && (offerFailedTwice(available) || offerRests(available, Date.now()))) {
      return { reason: "retry", detail: state.offerFailed.message ?? "The last attempt to install this release failed." };
    }
    if (state.engineBusy && available === state.engineBusy.version && (sessionsAtWork !== 0 || readAsks().length > 0)) {
      return { reason: "engine", detail: state.engineBusy.message };
    }
    if (available && offerChangesEngine() && status.offered === available) {
      return { reason: "engine", detail: "This release needs an engine change before it can install." };
    }
    if (switching || bridge?.renewal?.waits === "handover") return { reason: "handover", detail: "Kuvo is transferring running sessions." };
    if (bridgeBehind) {
      if (bridge?.renewal?.waits === "restart") return { reason: "agents", detail: "The coding agents’ bridge could not start the new build." };
      return { reason: "agents", detail: "Some work is still using the previous coding agents’ bridge." };
    }
    if (status.engineChange) return { reason: "engine", detail: status.engineChange.line };
    if (updateOnTrial()) return { reason: "trial", detail: "Kuvo is checking that the installed release is healthy." };
    if (available === state.latest && state.last?.result === "waiting" && state.last.version === available) {
      return { reason: "release-age", detail: state.last.message ?? "Automatic release checks wait a few days after publication." };
    }
    return { reason: "ready", detail: "The update is ready to install." };
  };
  const counts = await Promise.all(([[readEngine, "opencode"], [readBridge, "kuvo-agents"]] as const).map(async ([read, program]) => {
    let target: EngineRecord | BridgeRecord;
    const missing = program === "kuvo-agents" && switching ? null : 0;
    try { target = read(); } catch (error) {
      return error instanceof UnsupportedFormatVersion ? null : missing;
    }
    if (!target?.port || !target.password || (target.pid !== undefined && !runs(target.pid, program))) return missing;
    const active = (await asked(target, "GET", "api/session/active", undefined, 3_000).catch(() => null) as UncheckedAnswer | null)?.data;
    return Array.isArray(active) ? active.length : isObject(active) ? Object.keys(active).length : null;
  }));
  const sessionsAtWork = counts.includes(null) ? null : counts.reduce<number>((sum, count) => sum + (count ?? 0), 0);
  return { ...status, waiting: { version, ...facts(sessionsAtWork), sessionsAtWork } };
}

// An update in a process of its own (`kuvo-link.js update [--now] [--offered] [--round <round>]`,
// as the offered release's is): a phone's or a Mac's Update Now, or the release check's. Never in the
// process that asks (a client's server, a machine's bridge), which the update restarts, or which may
// be stopped meanwhile: the update outlives it, and says how it went in `run/update.json`. Each
// piece takes the new files in its own time. One at a time: false when one runs, or this process just started
// one. `round`: the release check's round it's for, noted by it unless something was at work. `keys` and `url`: a
// test's (a test's folder only).
let updateStarted: ChildProcess | null = null;
export function startUpdate({ now = false, offered = false, round, keys, url, from, executable }: {
  now?: boolean; offered?: boolean; round?: string; keys?: string[]; url?: string; from?: string; executable?: string;
} = {}): boolean {
  if (updateStarted || isUpdating()) return false;
  const test = !REAL_BOX && keys?.length ? ["--key", keys[0], ...(url ? ["--url", url] : [])] : [];
  const args = [...OWN_RUNTIME, SCRIPT, "update", ...(now ? ["--now", "--free"] : []), ...(offered ? ["--offered"] : []), ...(round ? ["--round", round] : []), ...(from ? ["--from", from] : []), ...test];
  const choice = releaseChoice();
  const version = (offered ? offeredVersion() : choice?.version) ?? updateState().latest ?? boxVersion() ?? "unknown";
  const sha256 = offered ? offeredPack() ?? undefined : choice?.sha256;
  const started = Date.now();
  const failed = (message: string) => {
    const at = Date.now();
    appendUpdateEvent(version, "starting updater", message, at);
    if (offered || (sha256 && releaseChoice()?.sha256 === sha256 && needsRelease({ version, sha256 }))) {
      recordOfferFailure(version, message, false, at, sha256);
    }
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

/** Check for a signed release immediately, through the same no-cut path as an automatic update. */
export function requestUpdateNow(): { status: 202 } | { status: 409; message: string } {
  const choice = releaseChoice();
  if (choice) return requestFleetRetry(choice.version);
  if (releaseSetup()) return { status: 409, message: "The setup doesn’t have an accepted release yet." };
  const blocked = !UPDATE_KEYS.length ? "Updates aren't set up for this machine yet." : updatesBlocked();
  if (blocked) return { status: 409, message: blocked };
  if (isUpdating()) return { status: 409, message: UPDATING_ALREADY };
  const held = offerHeld() ?? (["paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark))) ? "Start this computer’s box to update it." : null);
  if (held) return { status: 409, message: held };
  const offered = offerWaits();
  const kept = !offered && newerVersion(heldVersion(), boxVersion()) ? HELD : undefined;
  return startUpdate({ now: true, offered, from: kept }) ? { status: 202 } : { status: 409, message: UPDATING_ALREADY };
}

export async function* updateRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json, readEngine, readBridge } = context;
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, await updateStatusHere(readEngine, readBridge));
  if (method === "POST") {
    if ((parsed(request.body) as Body<"now">)?.now !== true) return yield* json(400, { message: "Say to update now (now)." });
    const started = requestUpdateNow();
    if (started.status === 409) return yield* json(409, { message: started.message });
    return yield* json(202, { ...updateStatus(), updating: "checking" });
  }
  return yield* json(405, {});
}

// The version of the release offered from a folder here (`updates/offered`), or null: checked as
// it was offered, and again as it's taken.
export function offeredVersion(): string | null {
  try {
    const given = JSON.parse(fs.readFileSync(path.join(OFFERED, "boxes.json"), "utf8"));
    const version = JSON.parse(Buffer.from(given.manifest, "base64").toString("utf8"))?.version;
    return versionOrder(version) ? version : null;
  } catch { return null; }
}

const dropOffer = (): void => fs.rmSync(OFFERED, { recursive: true, force: true });

// The version of the newest release held here to hand to your other computers (`updates/release`),
// checked as it was kept, or null.
function heldVersion(): string | null {
  try {
    const given = JSON.parse(fs.readFileSync(path.join(HELD, "boxes.json"), "utf8"));
    const version = JSON.parse(Buffer.from(given.manifest, "base64").toString("utf8"))?.version;
    return versionOrder(version) ? version : null;
  } catch { return null; }
}

/** Since when the release offered here has waited (its folder's time), or now for none. */
export function offeredSince(): number {
  try { return fs.statSync(path.join(OFFERED, "boxes.json")).mtimeMs; } catch { return Date.now(); }
}

// The SHA-256 of the offered release's pack, as its manifest says (checked as it was offered), or null.
export function offeredPack(): string | null {
  try {
    const given = JSON.parse(fs.readFileSync(path.join(OFFERED, "boxes.json"), "utf8"));
    const sha = JSON.parse(Buffer.from(given.manifest, "base64").toString("utf8"))?.pack?.sha256;
    return typeof sha === "string" ? sha : null;
  } catch { return null; }
}

// An offered release of the box's own version whose files differ from those in place, marked so as
// it was offered (`updates/offered/.differs`): what's offered is ahead of the box then too.
const OFFER_DIFFERS = ".differs";

// Whether the offered release `offered` is still ahead of the box at `version`: newer, or the same
// version with other files.
const offerAhead = (offered: string, version: string): boolean =>
  newerVersion(offered, version) || (sameVersion(offered, version) && fs.existsSync(path.join(OFFERED, OFFER_DIFFERS)));

/** Why the release Kuvo Code here offered can't be taken even on your word now, in words to show
 * as they are: its engine stopped on purpose, or its last update not yet said to be well. */
export function offerHeld(): string | null {
  if (fs.existsSync(path.join(RUN, "stopped"))) return "Its engine is stopped: start it to update it.";
  if (updateOnTrial()) return ON_TRIAL;
  return null;
}

/** Whether a release Kuvo Code here offered waits to be taken: newer than this box, or its own
 * version with other files. */
export function offerWaits(): boolean {
  const offered = offeredVersion(), version = boxVersion();
  return !!offered && !!version && offerAhead(offered, version);
}

/** What offering a release says: 202 and its version, or why not (400 not asked right, 409 not
 * now or not newer, 422 not a release this box takes). */
export interface Offered {
  status: 202 | 400 | 409 | 422;
  version?: string;
  message?: string;
}

// Whether a release offered from a folder here rests now, `version` at `now`: it failed as it was
// taken lately (a day once it was taken back, an hour when it failed before anything changed), or
// the update to it was taken back by the keeper (its new bridge never said it was well: the trial
// says it's on its way to it, and the box is back where it was).
/** Whether the offered release `version` failed more than once in a row as it was taken, or was put
 * in place and taken back: then it rests a day; one failure that changed nothing is tried again in
 * an hour. */
export function offerFailedAgain(version: string | null | undefined, failed: UpdateState["offerFailed"] = updateState().offerFailed): boolean {
  return !!failed && failed.version === version && (failed.takenBack || (failed.tries ?? 1) > 1);
}

/** Whether the offered release `version` failed here twice in a row (put in place and taken back, or
 * before anything changed): what a box can't get past by itself, so Update Now is offered. */
export function offerFailedTwice(version: string | null | undefined, failed: UpdateState["offerFailed"] = updateState().offerFailed): boolean {
  return !!failed && failed.version === version && (failed.tries ?? 1) > 1;
}

export function offerRetryAt(failed: NonNullable<UpdateState["offerFailed"]>): number {
  return failed.at + (offerFailedAgain(failed.version, failed) ? OFFER_REST : OFFER_RETRY);
}

export function offerRests(version: string, now: number, sha256?: string): boolean {
  const { offerFailed: failed, trial } = updateState();
  if (!releaseSetup()) sha256 = undefined; // Unmanaged releases retain their version-based retry policy.
  if (failed?.version === version && (!sha256 || !failed.sha256 || sha256 === failed.sha256) && !failed.retryNow && now < offerRetryAt(failed)) return true;
  return !failed?.retryNow && trial?.to === version && (!sha256 || !trial.sha256 || sha256 === trial.sha256) && boxVersion() !== version && !updateOnTrial() && now - trial.at < OFFER_REST;
}

/** A release offered from a folder on this computer (`kuvo/box/update {from}`: the one Kuvo Code
 * carries, as it updates): `boxes.json` and the pack it names, beside it. Checked as a downloaded
 * one is (the signature, a newer version, the pack's SHA-256), then kept in `updates/offered` (the
 * folder may go, the app updated again) for the next quiet moment (`takeOffer`), under the update
 * lock, so a take never reads it half replaced. Not one that was put in place lately and taken
 * back. `keys`, `blocked` and `clock`: a test's. */
export function offerUpdate(from: unknown, { keys = UPDATE_KEYS, blocked = updatesBlocked(), clock = Date.now }: Pick<UpdateOptions, "keys" | "blocked" | "clock"> = {}): Offered {
  if (typeof from !== "string" || !path.isAbsolute(from)) return { status: 400, message: "Say which folder the release is in (from), as a full path." };
  if (!keys.length) return { status: 409, message: "Updates aren't set up for this machine yet." };
  if (blocked) return { status: 409, message: blocked };
  const before = boxVersion();
  if (!before) return { status: 409, message: "This machine's version isn't known: connect to it from Kuvo Code on your Mac once." };
  if (updateStarted || isUpdating()) return { status: 409, message: UPDATING_ALREADY };
  let boxes: Buffer, manifest: BoxRelease, name: string;
  try {
    boxes = readRelease(path.join(from, "boxes.json"), 1_000_000);
    manifest = verifyBoxes(boxes.toString("utf8"), keys);
    name = packName(manifest);
  } catch (error) {
    return { status: 422, message: String((error as Error | undefined)?.message ?? error).slice(0, 300) };
  }
  // Never an older version; its own version only with other files than those in place (below).
  const again = sameVersion(manifest.version, before);
  if (!newerVersion(manifest.version, before) && !again) return { status: 409, message: `It has ${before} already, and takes only a newer version.` };
  const behind = engineBehind(manifest.engine);
  if (behind) return { status: 409, message: behind };
  const offered = offeredVersion();
  const setup = releaseSetup();
  const choice = releaseChoice();
  if (setup && manifest.spreads !== true) return { status: 409, message: "This release cannot be shared with your setup." };
  if (setup?.primary && choice && choice.sha256 !== manifest.pack.sha256 && !supersedes(manifest, choice))
    return { status: 409, message: "The setup already selected a release at least as new." };
  if (setup && !setup.primary && (choice?.version !== manifest.version || choice.sha256 !== manifest.pack.sha256))
    return { status: 409, message: "Only the release chosen by the main computer can update this computer." };
  if (offered && newerVersion(offered, manifest.version) && (!setup || setup.primary || offered === choice?.version)) return { status: 409, message: `A newer release (${offered}) waits to be taken already.` };
  const { offerFailed: failed } = updateState();
  if (offerRests(manifest.version, clock(), manifest.pack.sha256) && (failed?.version !== manifest.version || failed.takenBack)) return { status: 409, message: `It tried ${manifest.version} lately and went back to what it had: it takes it again tomorrow.` };
  // The same, offered again (as Kuvo Code reconnects): it's there already. (The same version built
  // again, other files under it: it takes its place.)
  if (offered === manifest.version && offeredPack() === manifest.pack.sha256) return { status: 202, version: manifest.version };
  let pack: Buffer;
  try {
    pack = readRelease(path.join(from, name), 100_000_000);
  } catch (error) {
    return { status: 422, message: String((error as Error | undefined)?.message ?? error) };
  }
  if (hexSHA256(pack) !== manifest.pack.sha256) return { status: 422, message: "The release's files aren't what it says." };
  if ((!setup || (setup.primary && choice?.sha256 !== manifest.pack.sha256)) && peersOf(readLink()).length) {
    const layout = packedLinkLayout(pack);
    if (manifest.layout !== undefined && manifest.layout !== layout) return { status: 422, message: "The release’s folder layout doesn’t match its manifest." };
    const hold = layoutHold(layout);
    if (hold) return { status: 409, message: hold };
  }
  // Kept whole or not at all, and never while a take could be reading it.
  if (!takeUpdateLock()) return { status: 409, message: UPDATING_ALREADY };
  const partial = `${OFFERED}.partial`;
  try {
    // Its own version: taken only when its files aren't those in place (an app rebuilt under the
    // same version); the same files are what it has.
    if (again) {
      let differs: boolean;
      try {
        differs = differsHere(packedBox(pack, manifest.version));
      } catch {
        return { status: 422, message: "The release's files aren't what it says." };
      }
      if (!differs && (!setup || installedPack() === manifest.pack.sha256)) {
        // (The release it runs, signed: kept to hand to your other computers, where it holds none
        // as new yet, as a box an app installed does.)
        keepRelease(boxes, pack);
        return { status: 409, message: `It has ${before} already, with these same files.` };
      }
    }
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
    fs.writeFileSync(path.join(partial, name), pack, { mode: 0o600 });
    if (again) fs.writeFileSync(path.join(partial, OFFER_DIFFERS), "", { mode: 0o600 });
    dropOffer();
    fs.renameSync(partial, OFFERED);
    // Signed and checked: held from now, so your other computers take it at once, in parallel with
    // this one (`updates/fleet.ts`).
    keepRelease(boxes, pack);
  } catch (error) {
    fs.rmSync(partial, { recursive: true, force: true });
    return { status: 409, message: `It couldn't keep the release: ${String((error as Error | undefined)?.message ?? error).slice(0, 200)}` };
  } finally {
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
  return { status: 202, version: manifest.version };
}

/** Whether the release offered from a folder here is to be taken now: newer than the box, not
 * resting after a failure, and nothing in the way (an update under way or on trial, a block, a
 * stopped engine, something running). `now` (Update Now, on your word): whatever is at work, and
 * though it rests. One the box has caught up with goes. `blocked` and `clock`: a test's. */
export async function offerDue({ blocked = updatesBlocked(), clock = Date.now, now = false, keys = UPDATE_KEYS }: Pick<UpdateOptions, "blocked" | "clock" | "now" | "keys"> = {}): Promise<boolean> {
  const offered = offeredVersion(), before = boxVersion();
  // (None, or one being written: nothing to do yet.)
  if (!offered) return false;
  if (before && !offerAhead(offered, before)) {
    // (Not while an update could be writing a new one in its place.)
    if (!isUpdating()) dropOffer();
    return false;
  }
  const setup = releaseSetup();
  const choice = releaseChoice();
  if (setup && (offered !== choice?.version || offeredPack() !== choice.sha256)) {
    if (!setup.primary) return false;
    if (choice && !offerSupersedes(choice, keys)) return false;
  }
  if (blocked || !before || isUpdating() || updateOnTrial() || fs.existsSync(path.join(RUN, "stopped"))) return false;
  if (now) return true;
  // (One that failed here twice waits for your word, Update Now, or a newer one.)
  if (!setup && offerFailedTwice(offered)) return false;
  // (Only one with another engine waits for nothing to be at work.)
  return !updateStarted && !offerRests(offered, clock(), offeredPack() ?? undefined) && !(offerChangesEngine() && await boxBusy());
}

function recordOfferFailure(version: string, message: string, takenBack: boolean, at = Date.now(), sha256 = offeredPack() ?? releaseChoice()?.sha256): void {
  const previous = updateState().offerFailed;
  if (!releaseSetup()) sha256 = undefined;
  const tries = previous?.version === version && (!sha256 || !previous.sha256 || previous.sha256 === sha256) ? (previous.tries ?? 1) + 1 : 1;
  noteUpdate({ offerFailed: { version, at, takenBack, tries, message, sha256 }, fleetFailedAt: { version, sha256, at } });
}

export function noteInterruptedOffer(): void {
  const version = offeredVersion();
  if (!version || !updateState().running || isUpdating() || updateOnTrial()) return;
  const message = "Kuvo restarted while it was installing the offered release.";
  recordOfferFailure(version, message, false);
  noteUpdate({ running: null });
  appendUpdateEvent(version, "staging", message);
}

/** The release offered from a folder here, taken when it's due (`offerDue`), as `update --offered`
 * does in a process of its own. Taken, or no longer newer, it goes. One that fails once it was put
 * in place is taken back, goes, and rests a day (offered again sooner, it's refused); one that
 * fails before anything changed stays, and is tried again in an hour, then a day after each next
 * failure. Busy, it waits. Nothing due: null. */
export async function takeOffer(options: UpdateOptions = {}): Promise<UpdateOutcome | null> {
  if (!(await offerDue(options))) return null;
  let version = offeredVersion()!, swapped = false;
  const before = boxVersion();
  appendUpdateEvent(version, "staging", "Preparing the accepted offer.");
  const outcome = await update({ ...options, from: OFFERED, swapping: () => { swapped = true; }, taking: taken => { version = taken; },
    step: step => { appendUpdateEvent(version, step, "Installing the offered release."); options.step?.(step); } });
  appendUpdateEvent(version, outcome.result, outcome.message ?? "The installer finished this attempt.");
  const takenBack = outcome.result === "failed" && swapped && !!before && boxVersion() === before &&
    !updateOnTrial() && !fs.existsSync(ROLLBACK);
  if (outcome.result === "failed" || (outcome.result === "blocked" && releaseSetup()))
    recordOfferFailure(version, outcome.message ?? "Kuvo couldn’t install the release.", takenBack, (options.clock ?? Date.now)());
  // (Taken: forgotten once it's judged well (`confirmUpdate`), so one taken back after its trial
  // counts with the failures before it. Here already: forgotten now.)
  if (outcome.result === "current") noteUpdate({ offerFailed: null });
  if (outcome.result === "updated" || outcome.result === "current" || takenBack) dropOffer();
  return outcome;
}

// COMPAT(beta.30 installer): `installerSteps` and `untilFree`, for `update --installer`. Goes once no box is from before beta.31.
/** What the installer Kuvo Code carries says as it brings this box up to its release
 * (`kuvo-install update`, `update --installer`), for the app to show as they are. */
const HERE = process.platform === "darwin" ? "this Mac" : "this computer";
export const installerSteps = {
  waiting: {
    step: "waiting", title: `Kuvo on ${HERE} updates once your sessions finish.`,
    confirm: { title: `Update Kuvo on ${HERE} now?`, message: `What your sessions on ${HERE} are doing stops as Kuvo restarts. It’s back in a moment.`, label: "Update Now" },
  },
  updating: { step: "updating", title: `Updating Kuvo on ${HERE}…` },
} as const;

/** Waits until nothing is at work here before the release in `from` is taken (`update --installer`
 * without `--now`) when it starts another engine, saying once that it waits. True to go on: nothing at work, or nothing to wait
 * for (this box has that version already, or can't update, which the update then says). False once
 * the app that asked is gone (this process left to another): it's taken by the box itself then. */
export async function untilFree(from: string, { keys = UPDATE_KEYS, blocked }: Pick<UpdateOptions, "keys" | "blocked"> = {}): Promise<boolean> {
  const every = !REAL_BOX && Number(process.env.KUVO_TEST_UNTIL_FREE_EVERY) > 0 ? Number(process.env.KUVO_TEST_UNTIL_FREE_EVERY) : 15_000;
  let manifest: BoxRelease;
  try { manifest = verifyBoxes(readRelease(path.join(from, "boxes.json"), 1_000_000).toString("utf8"), keys); } catch { return true; }
  const { version } = manifest;
  const parent = process.ppid;
  let said = false;
  // (Its own version: something to wait for only with other files than those in place, read once
  // for each version the box has: they change only as it updates.)
  let differs = null as { at: string; differs: boolean } | null;
  for (;;) {
    const before = boxVersion();
    if (before && sameVersion(version, before) && differs?.at !== before) differs = { at: before, differs: folderDiffers(from, manifest) };
    const ahead = !!before && (newerVersion(version, before) || (sameVersion(version, before) && differs!.differs));
    if (!before || !ahead || (blocked === undefined ? updatesBlocked() : blocked)) return true;
    // (The box taking it itself meanwhile: waited for, and then it's there.)
    // (Only a release with another engine waits for nothing to be at work: the rest stop nothing.)
    if (!isUpdating() && (manifest.engine === runningEngineVersion() || !(await boxBusy()))) return true;
    if (!said) { console.log(JSON.stringify(installerSteps.waiting)); said = true; }
    await sleep(every);
    if (process.ppid !== parent) return false;
  }
}

/** The offered release taken in a process of its own (`kuvo-link.js update --offered`), as Update
 * Now's is, when it's due: never in the process that asks, whose answer it would outlive, and
 * which isn't the bridge that goes for the keeper to start the new one. */
let offerSpawned = 0;
export async function startOfferedUpdate(clock: () => number = Date.now, manual = false): Promise<boolean> {
  // (Asked again every few seconds while one waits: not a second process while the first starts.)
  if ((!manual && clock() - offerSpawned < 60_000) || !(await offerDue())) return false;
  offerSpawned = clock();
  return startUpdate({ offered: true, now: manual });
}

// A release from a folder on this computer (PROTOCOL.md, "Updates"): offered by Kuvo Code here
// (never a phone), checked now, taken at the next quiet moment.
export async function* boxUpdateRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  if (!request.fromApp && !request.fromMac) return yield* json(403, { message: "Only Kuvo Code offers Kuvo on this computer a release." });
  const given = parsed(request.body);
  const offered = offerUpdate(isObject(given) ? given.from : undefined);
  if (offered.status !== 202) return yield* json(offered.status, { message: offered.message });
  startOfferedUpdate().catch(() => {});
  return yield* json(202, updateStatus());
}
