// The layout contract for a computer's Kuvo folder, independent of the formats carried inside it.
import fs from "node:fs";
import path from "node:path";
import { locked, writeDurablePrivate } from "./files";
import { UnsupportedFormatVersion } from "./format-version";
import { isObject } from "./json";

export const FOLDER_VERSION_FILE = "folder-version.json";

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
  upgradeFolder(folder, steps, importLegacy);
}

/** The same runner can be exercised with multiple steps before a second layout ships. */
export function upgradeFolder(folder: string, ordered: readonly Step[], importLegacy?: () => void): void {
  folder = path.resolve(folder);
  const file = path.join(folder, FOLDER_VERSION_FILE);
  const current = ordered.length;
  // A future, invalid, or already current marker requires no writes, including to the lock.
  if (checkpoint(file, current).version === current) return;
  const save = (state: Checkpoint) => writeDurablePrivate(file, JSON.stringify(state) + "\n", path.dirname(folder));
  locked(path.join(folder, ".folder-upgrade.lock"), () => {
    const start = checkpoint(file, current);
    for (let version = start.version; version < current; version++) {
      const upgradingTo = version + 1;
      if (start.upgradingTo !== upgradingTo) save({ version, upgradingTo });
      ordered[version](folder, importLegacy);
      save({ version: upgradingTo });
    }
  }, { exclusive: true, wait: 60_000 });
}
