// Kuvo's own programs on this box, repaired from the signed release it runs (PROTOCOL.md, "Updates",
// Repaired from the release it runs). Every box keeps that release (`updates/release`, the fleet's
// held release): its signed manifest (`boxes.json`) names its pack's SHA-256, and the pack's
// `box.json` names each of its files' SHA-256. A file in place that isn't what the release says (an
// older Kuvo Code on a Mac put its own over it, a disk's damage, anything) is put back from the pack,
// checked against both. Only the release this box runs (`bin/box-version`): never another version,
// up or down. A held release whose pack isn't what its manifest says is never used, and goes.
// Not while an update or a rollback runs, nor while a Mac puts its files in place.
// Platform: the same on macOS and Linux.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { writePrivate } from "../files";
import { IS_MAC, K, OWN_HOME_MAC, RUN } from "../home";
import { HELD, heldKeys } from "./fleet";
import { type BoxFiles, MAC_INSTALLING, boxVersion, hexSHA256, isUpdating, packName, packedBox, verifyBoxes } from "./update";

/** What a look came to: all as the release says (`sound`), files put back (`repaired`), nothing it
 * could do (`none`: no release held here, or another version than the one running, or not now), or
 * the held release itself not what it says (`damaged`: not used, and gone). */
export type Repair =
  | { result: "sound"; version: string }
  | { result: "repaired"; version: string; files: string[] }
  | { result: "none"; why: "noRelease" | "otherVersion" | "busy" }
  | { result: "damaged"; version: string };

/** Where the last look is noted, for the box to say (`agents-down.ts`) and for logs. */
export const REPAIR_FILE = path.join(RUN, "repair.json");
/** What an older Kuvo kept of its tools (`bin/.own`), which the retired Mac app puts back over a
 * release's: gone at each look, so nothing does. */
const OWN_LEFT = path.join(K, "bin", ".own");

export function repairFromRelease(options: { keys?: string[]; now?: number } = {}): Repair {
  const now = options.now ?? Date.now();
  const said = (repair: Repair): Repair => {
    try { writePrivate(REPAIR_FILE, JSON.stringify({ at: now, ...repair })); } catch {}
    if (repair.result === "repaired") console.error(`repaired from the release it runs (${repair.version}): ${repair.files.join(", ")}`);
    if (repair.result === "damaged") console.error(`the release held (${repair.version}) isn't what its manifest says: not used, and removed`);
    return repair;
  };
  // (A retired Mac app's own box: that app puts its files in place.)
  if (IS_MAC && !OWN_HOME_MAC) return { result: "none", why: "busy" };
  if (isUpdating() || fs.existsSync(path.join(RUN, ".rolling-back"))) return said({ result: "none", why: "busy" });
  let installing = 0;
  try { installing = fs.statSync(path.join(RUN, "mac-installing")).mtimeMs; } catch {}
  if (installing && now - installing < MAC_INSTALLING) return said({ result: "none", why: "busy" });
  fs.rmSync(OWN_LEFT, { recursive: true, force: true });

  let text: string;
  try {
    text = fs.readFileSync(path.join(HELD, "boxes.json"), "utf8");
  } catch {
    return said({ result: "none", why: "noRelease" });
  }
  let manifest;
  try {
    manifest = verifyBoxes(text, options.keys ?? heldKeys());
  } catch {
    // (Not signed with a key this box knows: never used, but left, as it's not its to judge.)
    return said({ result: "none", why: "noRelease" });
  }
  const running = boxVersion();
  if (!running || manifest.version !== running) return said({ result: "none", why: "otherVersion" });
  let pack: Buffer;
  try {
    pack = fs.readFileSync(path.join(HELD, packName(manifest)));
  } catch {
    return said({ result: "none", why: "noRelease" });
  }
  if (hexSHA256(pack) !== manifest.pack.sha256) {
    fs.rmSync(HELD, { recursive: true, force: true });
    return said({ result: "damaged", version: manifest.version });
  }
  let box: BoxFiles;
  try {
    box = packedBox(pack, manifest.version);
  } catch {
    fs.rmSync(HELD, { recursive: true, force: true });
    return said({ result: "damaged", version: manifest.version });
  }
  const wrong = Object.entries(box.files).filter(([file, sha]) => {
    try {
      return !fs.lstatSync(path.join(K, file)).isFile() || hexSHA256(fs.readFileSync(path.join(K, file))) !== sha;
    } catch {
      return true;
    }
  });
  if (!wrong.length) return said({ result: "sound", version: manifest.version });

  // Those files alone, unpacked into a folder of its own, each checked, and each put in place whole.
  const stage = path.join(path.dirname(HELD), "repair");
  fs.rmSync(stage, { recursive: true, force: true });
  fs.mkdirSync(stage, { recursive: true, mode: 0o700 });
  try {
    const put: string[] = [];
    for (const [file, sha] of wrong) {
      // (Packed from a folder, `./<file>`; or as it's named.)
      const read = ["./" + file, file].map(name => spawnSync("tar", ["-xzOf", "-", name], { input: pack, maxBuffer: 200_000_000, timeout: 120_000 })).find(each => each.status === 0 && each.stdout?.length);
      const mode = modeIn(pack, file);
      if (!read || hexSHA256(read.stdout) !== sha) continue;
      const to = path.join(K, file);
      fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
      fs.writeFileSync(`${to}.repair`, read.stdout, { mode: 0o600 });
      fs.chmodSync(`${to}.repair`, mode);
      fs.renameSync(`${to}.repair`, to);
      put.push(file);
    }
    return said({ result: "repaired", version: manifest.version, files: put });
  } finally {
    fs.rmSync(stage, { recursive: true, force: true });
  }
}

/** A file's mode as the pack has it (tar's listing), else a program's (bin's scripts) or a file's. */
function modeIn(pack: Buffer, file: string): number {
  const listed = spawnSync("tar", ["-tvzf", "-"], { input: pack, maxBuffer: 10_000_000, timeout: 120_000 }).stdout?.toString("utf8") ?? "";
  const line = listed.split("\n").find(each => each.endsWith(` ${file}`) || each.endsWith(` ./${file}`));
  const bits = line?.slice(1, 10);
  if (bits && /^[r-][w-][xsS-][r-][w-][xsS-][r-][w-][xtT-]$/.test(bits)) {
    // (s and t carry the bit beneath them; S and T don't.)
    return [...bits].reduce((mode, bit, index) => mode | (bit !== "-" && bit !== "S" && bit !== "T" ? 1 << (8 - index) : 0), 0);
  }
  return 0o644;
}

/** The last look, as noted (null: none yet). */
export function lastRepair(): (Repair & { at: number }) | null {
  try {
    return JSON.parse(fs.readFileSync(REPAIR_FILE, "utf8"));
  } catch {
    return null;
  }
}
