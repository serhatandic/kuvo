// The layout contract for a computer's Kuvo folder, independent of the formats carried inside it.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { locked, writeDurablePrivate } from "./files";
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
];

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

/** Check a move's source or destination without changing it. */
export function checkFolderVersion(folder: string): void {
  checkpoint(path.resolve(folder, FOLDER_VERSION_FILE), steps.length);
}

export function upgradeKuvoFolder(folder: string, importLegacy?: () => void): void {
  upgradeFolder(folder, steps, importLegacy, start => checkTrialRecovery(folder, start));
}

/** Layout-5 writers remain fenced until the folder runner has committed the last step. */
export function setupLayoutDeferred(folder: string): boolean {
  try { return checkpoint(path.join(folder, FOLDER_VERSION_FILE), steps.length).version === 4; }
  catch { return true; }
}

/** A trial can advance only as far as the saved reader can open. Before layout markers,
 * readers ignored the probe completely and can recover only the additive layout 1. */
function checkTrialRecovery(folder: string, start: Checkpoint): void | false {
  if (!fs.existsSync(path.join(folder, "run", "update-trial"))) return;
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
    const accepts = (version: number) => {
      fs.writeFileSync(path.join(probe, FOLDER_VERSION_FILE), JSON.stringify({ version }));
      const flags = runtime === oldRuntime || process.versions.bun ? ["--config=/dev/null", "--no-env-file"] : [];
      return spawnSync(runtime, [...flags, saved, "here", ".kuvo-machine-probe"], {
        env: { ...process.env, HOME: scratch, KUVO_HOME: probe, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
        stdio: "ignore", timeout: 15_000,
      }).status === 0;
    };
    if (!accepts(2_147_483_647)) {
      if (accepts(steps.length)) return;
      // The only deferred step is additive. The trial's setup writers stay on hold
      // until confirmation, and rollback can reopen the original marker as-is.
      if (start.version === 4 && start.upgradingTo === undefined && accepts(start.version)) return false;
    }
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
  locked(path.join(folder, ".folder-upgrade.lock"), () => {
    const start = checkpoint(file, current);
    if (start.version < current && beforeSteps?.(start) === false) return;
    for (let version = start.version; version < current; version++) {
      const upgradingTo = version + 1;
      if (start.upgradingTo !== upgradingTo) save({ version, upgradingTo });
      ordered[version](folder, importLegacy);
      save({ version: upgradingTo });
    }
  }, { exclusive: true, wait: 60_000 });
}
