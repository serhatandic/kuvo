// The layout contract for a computer's Kuvo folder, independent of the formats carried inside it.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { locked, textOf, writeDurablePrivate, writePrivate } from "./files";
import { UnsupportedFormatVersion } from "./format-version";
import { isObject } from "./json";

export const FOLDER_VERSION_FILE = "folder-version.json";

// Advance the layout for coordinated changes across files or an incompatible on-disk
// contract. A family encoding that still opens in older builds needs no folder step.
type Step = (folder: string, importLegacy?: () => void) => void;

/** Entry i takes layout i to i+1. A step must tolerate replay until its checkpoint commits.
 * Before returning, persist its changes so they survive any restart that sees the checkpoint. */
const steps: readonly Step[] = [
  // 1. Adopt the deployed layout in place. A legacy Mac move supplies its staged import here.
  (_folder, importLegacy) => importLegacy?.(),
  // 2. Link state can now carry a signed leadership history and demote its old signer.
  // Existing setups need no rewrite. An older box must never reopen a transferred setup.
  () => {},
  // 3. Reserve the standby delegation and fencing state before using them.
  () => {},
  // 4. Link state can carry an epoch's accepted shared values and pending saves.
  // Layout 3 setups keep their existing authority and files until their first save.
  () => {},
  // 5. Reserve immutable setup checkpoints and operation records. Layout 4 records remain readable.
  () => {},
  // 6. Reserve owner snapshots, message operations and ownership receipts. Existing data stays in place.
  () => {},
  // 7. Reserve offline edits, their receipts and compact deletion markers.
  () => {},
  // 8. Reserve portable computer removals and the local wipe journal.
  () => {},
  // 9. Reserve wipe preparation intents, source journals and verified export references.
  () => {},
  // 10. Reserve setup-scoped computer history and permanent enrollment revocations.
  () => {},
  // 11. Drop voucher ownership from stored device grants. Signed records are immutable;
  // the setup membership reader projects those grants without the legacy fields.
  migrateDeviceGrants,
  // 12. Authorize prepared removals through accepted membership and retain wipe receipts
  // outside the deleted installation. Existing work and legacy removal records stay in place.
  () => {},
  // 13. Reserve durable credential cleanup and signed final-standby retirement.
  () => {},
  // 14. Reserve signed successor order, branch ancestry and merged access revocations.
  () => {},
  // 15. Reserve accepted rank revisions and the persisted standby pin.
  () => {},
  // 16. Reserve bounded rank history and standby assignment intents.
  () => {},
  // 17. Reserve first-standby assignment revisions. Strip rank history from the active replica.
  migrateRankHistory,
  // 18. Reserve bridge handover and program/engine drain recovery records.
  // Existing session files stay in place; the additive stamp is written with the next save.
  () => {},
  // 19. Reserve installer invitations and the joining computer’s durable claim.
  () => {},
];

export const KUVO_FOLDER_LAYOUT = steps.length;

function migrateDeviceGrants(folder: string): void {
  const file = path.join(folder, "run", "link.json");
  if (!fs.existsSync(file)) return;
  locked(path.join(folder, "run", ".link.lock"), () => {
    let text: string;
    try { text = fs.readFileSync(file, "utf8"); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
      throw error;
    }
    const link: unknown = JSON.parse(text);
    if (!isObject(link)) throw new Error("The link state must be an object to migrate device grants.");
    let changed = Object.hasOwn(link, "vouchBlocked");
    delete link.vouchBlocked;
    for (const phone of Array.isArray(link.phones) ? link.phones : []) {
      if (!isObject(phone) || !Object.hasOwn(phone, "by")) continue;
      delete phone.by;
      changed = true;
    }
    if (changed) writeDurablePrivate(file, JSON.stringify(link) + "\n", path.dirname(folder));
  }, { exclusive: true, wait: 60_000 });
}

function migrateRankHistory(folder: string): void {
  const file = path.join(folder, "run", "link.json");
  if (!fs.existsSync(file)) return;
  locked(path.join(folder, "run", ".link.lock"), () => {
    let text: string;
    try { text = fs.readFileSync(file, "utf8"); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
      throw error;
    }
    const link: unknown = JSON.parse(text);
    if (!isObject(link) || !isObject(link.shared) || !isObject(link.shared.membershipAndAccess)) return;
    const value = link.shared.membershipAndAccess.value;
    if (!isObject(value) || !Object.hasOwn(value, "successorRevisions")) return;
    delete value.successorRevisions;
    writeDurablePrivate(file, JSON.stringify(link) + "\n", path.dirname(folder));
  }, { exclusive: true, wait: 60_000 });
}

interface Checkpoint {
  version: number;
  upgradingTo?: number;
}

function checkpoint(file: string, current: number): Checkpoint {
  const format = `Kuvo folder at ${path.dirname(file)}`;
  let text: string;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    // A dangling link is an explicit marker we could not read.
    if (fs.lstatSync(file, { throwIfNoEntry: false })) throw error;
    return { version: 0 };
  }
  let value: unknown;
  try { value = JSON.parse(text); } catch {}
  const number = (found: unknown): found is number => typeof found === "number" && Number.isSafeInteger(found) && found >= 0;
  if (!isObject(value) || !number(value.version) || value.version > current) {
    throw new UnsupportedFormatVersion(format, isObject(value) ? value.version : null, current);
  }
  const { version, upgradingTo } = value;
  if (upgradingTo !== undefined && (!number(upgradingTo) || upgradingTo !== version + 1 || upgradingTo > current)) {
    throw new UnsupportedFormatVersion(format, upgradingTo, current);
  }
  return { version, upgradingTo };
}

/** A backup can only describe a committed layout. */
export function backupFolderVersion(folder: string): number {
  const state = checkpoint(path.resolve(folder, FOLDER_VERSION_FILE), steps.length);
  if (state.upgradingTo !== undefined) throw new Error("Wait for the folder upgrade before backing up.");
  return state.version;
}

/** Check a move's source or destination without changing it. */
export function checkFolderVersion(folder: string): void {
  checkpoint(path.resolve(folder, FOLDER_VERSION_FILE), steps.length);
}

export function upgradeKuvoFolder(folder: string, importLegacy?: () => void, checkWriter?: () => void): void {
  upgradeFolder(folder, steps, importLegacy, start => {
    requireNoRollback(folder);
    const compatible = checkTrialRecovery(folder, start, importLegacy !== undefined);
    requireNoRollback(folder);
    checkWriter?.();
    return compatible;
  });
}

const trialOpen = (folder: string) => fs.existsSync(path.join(folder, "run", "update-trial"));

/** The installed bundle's build and maximum layout, also read by the keeper before starting Node. */
export function installedLinkBuild(folder: string): { layout: number; build: string } | null {
  let file: number | undefined;
  try {
    file = fs.openSync(path.join(folder, "bin", "kuvo-link.js"), "r");
    const header = Buffer.alloc(96);
    const count = fs.readSync(file, header, 0, header.length, 0);
    const match = /^\/\/ Kuvo folder layout ([1-9][0-9]*) ([0-9a-f]{12})\n/.exec(header.toString("utf8", 0, count));
    return match ? { layout: Number(match[1]), build: match[2] } : null;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return null;
  } finally {
    if (file !== undefined) fs.closeSync(file);
  }
}

/** Also recognize the installed path when the Box lives behind a symlink. */
export function isInstalledBundle(folder: string, file: string, name: string): boolean {
  if (path.basename(file) !== name) return false;
  const bin = path.resolve(folder, "bin");
  if (path.dirname(path.resolve(file)) === bin) return true;
  try { return fs.realpathSync(path.dirname(file)) === fs.realpathSync(bin); }
  catch { return false; }
}

function requireNoRollback(folder: string): void {
  const run = path.join(folder, "run");
  const restore = path.join(run, ".rolling-back");
  let active = false;
  if (fs.existsSync(restore)) {
    const pid = Number(textOf(path.join(restore, "pid")));
    if (Number.isSafeInteger(pid) && pid > 1) {
      try { process.kill(pid, 0); active = true; }
      catch (error) { active = (error as NodeJS.ErrnoException).code !== "ESRCH"; }
    } else {
      // The keeper creates the directory before it records its PID.
      const started = fs.statSync(restore, { throwIfNoEntry: false });
      active = !!started && Date.now() - started.mtimeMs < 60_000;
    }
  }
  if (active || (trialOpen(folder) && fs.existsSync(path.join(run, "update-restore")))) {
    throw new Error("Kuvo Code is rolling back this Box. Try again after the rollback.");
  }
}

/** A failed updater can call its saved restore script directly. Publish its rollback intent
 * after any folder writer finishes, before the script checks the layout and restores the code. */
export function beginFolderRollback(folder: string): void {
  withFolderLock(folder, () => writePrivate(path.join(folder, "run", "update-restore"), "0"));
}

function withFolderLock(folder: string, work: () => void): void {
  locked(path.join(folder, ".folder-upgrade.lock"), work, { exclusive: true, wait: 60_000 });
}

/** A trial on the deployed layout or its additive checkpoint uses the old data contracts. */
export function legacyFolderTrial(folder: string): boolean {
  if (!trialOpen(folder)) return false;
  const marker = path.join(folder, FOLDER_VERSION_FILE);
  if (!fs.lstatSync(marker, { throwIfNoEntry: false })) return true;
  try {
    const { version, upgradingTo } = checkpoint(marker, steps.length);
    return version === 1 && upgradingTo === undefined;
  } catch { return false; }
}

/** Setup writers with newer on-disk contracts remain fenced while the upgrade is deferred. */
export function setupLayoutDeferred(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version === 4 || legacyFolderTrial(folder); }
  catch { return true; }
}

/** Owner records are created only after the folder runner commits layout 6. */
export function ownershipLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 6; }
  catch { return false; }
}

/** Offline edits are writable after their layout step. */
export function offlineEditLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 7; }
  catch { return false; }
}

export function removalLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 8; }
  catch { return false; }
}

export function wipePreparationLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 9; }
  catch { return false; }
}

export function removalOperationsLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 12; }
  catch { return false; }
}

export function unlinkCleanupLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 13; }
  catch { return false; }
}

export function computerHistoryLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 10; }
  catch { return false; }
}

/** Keep the former device grant contract until a deferred layout-11 migration can run. */
export function deviceGrantsLayoutDeferred(folder: string): boolean {
  return trialOpen(folder) && checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version < 11;
}

/** Reliability ordering is written after the layout step commits. */
export function successorRevisionLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 15; }
  catch { return false; }
}

export function standbyIntentLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 16; }
  catch { return false; }
}

export function standbyRevisionLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 17; }
  catch { return false; }
}

export function successorRankLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 14; }
  catch { return false; }
}

/** A trial can advance only as far as the saved reader can open. Before layout markers,
 * readers ignored the probe completely and can recover the unmarked layout and additive layout 1. */
function checkTrialRecovery(folder: string, start: Checkpoint, importing: boolean): void | false {
  if (!trialOpen(folder)) return;
  const bin = path.join(folder, "updates", "rollback", "files", "bin");
  const saved = path.join(bin, "kuvo-link.js");
  let engine: string | undefined;
  try { engine = /^V=([\w.-]+)$/m.exec(fs.readFileSync(path.join(bin, "kuvo-start"), "utf8"))?.[1]; } catch {}
  const oldRuntime = engine && path.join(folder, "engine", engine, "opencode");
  let runtime = process.execPath;
  if (oldRuntime) try { fs.accessSync(oldRuntime, fs.constants.X_OK); runtime = oldRuntime; } catch {}
  const refuse = () => { throw new Error(`The saved Kuvo Code can't recover folder layout ${steps.length}. Roll back the trial before updating the folder.`); };
  if (!fs.existsSync(saved)) return refuse();
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-layout-probe-"));
  try {
    const probe = path.join(scratch, ".kuvo-code");
    fs.mkdirSync(probe, { mode: 0o700 });
    const marker = path.join(probe, FOLDER_VERSION_FILE);
    // The saved bridge is probing a disposable machine folder, even when this
    // process is a Mac's app-owned box with different paths and launch settings.
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: scratch, KUVO_HOME: probe, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" };
    for (const key of Object.keys(process.env)) if (key.startsWith("KUVO_") && key !== "KUVO_HOME") delete env[key];
    const accepts = (version?: number) => {
      if (version === undefined) fs.rmSync(marker, { force: true });
      else fs.writeFileSync(marker, JSON.stringify({ version }));
      const flags = runtime === oldRuntime || process.versions.bun ? ["--config=/dev/null", "--no-env-file"] : [];
      const result = spawnSync(runtime, [...flags, saved, "here", ".kuvo-machine-probe"], {
        env,
        encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 15_000,
      });
      if (result.status !== 0) return false;
      try { return isObject(JSON.parse(result.stdout)); } catch { return false; }
    };
    if (!accepts(2_147_483_647)) {
      if (accepts(steps.length)) return;
      // The remaining steps are additive. The trial's new writers stay on hold
      // until confirmation, and rollback can reopen the original marker as-is.
      if ((start.version >= 4 && start.version < steps.length) && start.upgradingTo === undefined && accepts(start.version)) return false;
    }
    // Leave an unmarked or additive layout in place until confirmation, so neither a
    // crash nor a rollback has to undo a step.
    if (!importing && legacyFolderTrial(folder) && accepts(start.version || undefined)) return false;
    refuse();
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

/** The same runner can be exercised with multiple steps before a second layout ships. */
export function upgradeFolder(folder: string, ordered: readonly Step[], importLegacy?: () => void, beforeSteps?: (start: Checkpoint) => void | false): void {
  folder = path.resolve(folder);
  const file = path.join(folder, FOLDER_VERSION_FILE);
  const current = ordered.length;
  // A future, invalid, or already current marker requires no writes, including to the lock.
  if (checkpoint(file, current).version === current) return;
  const save = (state: Checkpoint) => writeDurablePrivate(file, JSON.stringify(state) + "\n", path.dirname(folder));
  withFolderLock(folder, () => {
    const start = checkpoint(file, current);
    if (start.version < current && beforeSteps?.(start) === false) return;
    for (let version = start.version; version < current; version++) {
      const upgradingTo = version + 1;
      if (start.upgradingTo !== upgradingTo) save({ version, upgradingTo });
      ordered[version](folder, importLegacy);
      save({ version: upgradingTo });
    }
  });
}

export function addComputersLayoutReady(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version >= 19; }
  catch { return false; }
}
