// Kuvo's own programs on this box, repaired from the signed release it runs (PROTOCOL.md, "Updates",
// Repaired from the release it runs). A box keeps that release: as the one it holds for your other
// computers (`updates/release`), or, once a newer one is held there, apart (`updates/running`). Its
// signed manifest (`boxes.json`) names its pack's SHA-256, and the pack's `box.json` names each of
// its files' SHA-256. A file in place that isn't what the release says (an older Kuvo Code on a Mac
// put its own over it, a disk's damage, anything) is put back from the pack, checked against both.
// Only the release this box runs (`bin/box-version`): never another version, up or down. A kept
// release whose pack isn't what its manifest says is never used, and goes. Not while an update, its
// trial or a rollback runs, nor while a Mac puts its files in place; and, as it writes, under the
// update's lock, so no update (nor a second repair) runs meanwhile.
// Platform: the same on macOS and Linux.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { writePrivate } from "../files";
import { fetchedPatiently } from "../engine/client";
import { IS_MAC, K, OWN_HOME_MAC, RUN } from "../home";
import { HELD, RUNNING, PREVIOUS_CHOICE, heldKeys, repairPackSha } from "./fleet";
import { BOX_FILE, type BoxFiles, MAC_INSTALLING, UPDATE_LOCK, boxVersion, hexSHA256, installOrder, isUpdating, packName, packedBox, takeUpdateLock, updateOnTrial, verifyBoxes } from "./update";

/** What a look came to: all as the release says (`sound`), files put back (`repaired`), nothing it
 * could do (`none`: no release of the version it runs kept here, or not now, or the files it found
 * wrong couldn't be put back), or the kept release itself not what it says (`damaged`: not used,
 * and gone). */
export type Repair =
  | { result: "sound"; version: string }
  | { result: "repaired"; version: string; files: string[] }
  | { result: "none"; why: "noRelease" | "otherVersion" | "busy" | "failed" }
  | { result: "damaged"; version: string };

/** Where the last look is noted, for the box to say (`agents-down.ts`) and for logs. */
export const REPAIR_FILE = path.join(RUN, "repair.json");
/** What an older Kuvo kept of its tools (`bin/.own`), which the keeper (and the retired Mac app) put
 * back over a release's: gone once a repair finds the files sound or puts them back, so nothing
 * stale ever goes back. */
const OWN_LEFT = path.join(K, "bin", ".own");
/** The keeper's own script: its engine line (`V=`) is the box's to change (a rollback that keeps a
 * newer engine writes it), so it's compared, and put back, with the line it has here. */
const KEEPER = "bin/kuvo-start";
const ENGINE_LINE = /^V=.*$/m;

export function repairFromRelease(options: { keys?: string[]; now?: number } = {}): Repair {
  const now = options.now ?? Date.now();
  const said = (repair: Repair): Repair => {
    try { writePrivate(REPAIR_FILE, JSON.stringify({ at: now, ...repair })); } catch {}
    // COMPAT(0.7 files): `bin/.own`, an older Kuvo's copies of its tools, which its keeper puts back after an older Mac's visit. Goes on the user's word, once no Mac from before 1.0 connects.
    // Only once this box has a release to repair from, and its files are as it says: until then
    // it's what the keeper puts back after an older Mac's visit.
    if (repair.result === "sound" || repair.result === "repaired") fs.rmSync(OWN_LEFT, { recursive: true, force: true });
    if (repair.result === "repaired") console.error(`repaired from the release it runs (${repair.version}): ${repair.files.join(", ")}`);
    if (repair.result === "damaged") console.error(`the release kept (${repair.version}) isn't what its manifest says: not used, and removed`);
    return repair;
  };
  // (A retired Mac app's own box: that app puts its files in place.)
  if (IS_MAC && !OWN_HOME_MAC) return { result: "none", why: "busy" };
  if (busy(now)) return said({ result: "none", why: "busy" });

  const running = boxVersion(), sha256 = repairPackSha();
  const found = keptRelease(running, sha256, options.keys ?? heldKeys());
  if ("result" in found) return said(found);
  const { manifest, pack, folder } = found;
  if (hexSHA256(pack) !== manifest.pack.sha256) {
    fs.rmSync(folder, { recursive: true, force: true });
    return said({ result: "damaged", version: manifest.version });
  }
  // (The pack is what its signed manifest says from here: tar failing says nothing against it.)
  let box: BoxFiles;
  try {
    box = packedBox(pack, manifest.version);
  } catch {
    return said({ result: "none", why: "failed" });
  }
  const files = Object.entries(box.files).filter(([file]) => BOX_FILE.test(file) && !file.split("/").includes(".."));
  // (The keeper's script, where it differs, read from the pack: one whose only difference is its
  // engine line is as the release says.)
  const keeper = files.find(([file, sha]) => file === KEEPER && !matches(file, sha, null));
  if (keeper) unpacked(pack, KEEPER);
  const wrong = files.filter(([file, sha]) => !matches(file, sha, null)).map(([file]) => file);
  if (!wrong.length) return said({ result: "sound", version: manifest.version });

  // Written under the update's lock: no update, rollback or other repair meanwhile.
  if (!takeUpdateLock()) return said({ result: "none", why: "busy" });
  try {
    if (updateOnTrial() || fs.existsSync(path.join(RUN, ".rolling-back")) || boxVersion() !== running || repairPackSha() !== sha256) return said({ result: "none", why: "busy" });
    const listing = spawnSync("tar", ["-tvzf", "-"], { input: pack, maxBuffer: 10_000_000, timeout: 120_000 }).stdout?.toString("utf8") ?? "";
    const put: string[] = [];
    for (const file of installOrder(wrong)) {
      const to = path.join(K, file), next = `${to}.repair-${process.pid}`;
      try {
        let data = unpacked(pack, file);
        if (!data || !matches(file, box.files[file]!, data)) continue;
        // (The keeper's script with the engine line it has here.)
        if (file === KEEPER) data = withEngineLine(data, to);
        // (Already so: nothing to put back.)
        try { if (fs.readFileSync(to).equals(data)) continue; } catch {}
        fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
        fs.writeFileSync(next, data, { mode: 0o600 });
        fs.chmodSync(next, modeIn(listing, file));
        fs.renameSync(next, to);
        put.push(file);
      } catch (error) {
        fs.rmSync(next, { force: true });
        console.error(`couldn't put ${file} back:`, error);
      }
    }
    return said(put.length ? { result: "repaired", version: manifest.version, files: put } : { result: "none", why: "failed" });
  } finally {
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
}

/** Not now: an update, its trial or a rollback runs, or a Mac puts its files in place. */
function busy(now: number): boolean {
  if (isUpdating() || updateOnTrial() || fs.existsSync(path.join(RUN, ".rolling-back"))) return true;
  let installing = 0;
  try { installing = fs.statSync(path.join(RUN, "mac-installing")).mtimeMs; } catch {}
  return installing > 0 && now - installing < MAC_INSTALLING;
}

/** The release this box runs, as it's kept (held, or apart once a newer one is held), signed with
 * `keys`; or why none is. One of another version kept apart goes. */
function keptRelease(running: string | null, sha256: string | null | undefined, keys: string[]): { manifest: ReturnType<typeof verifyBoxes>; pack: Buffer; folder: string } | Repair {
  let other = false;
  if (sha256 === null) return { result: "none", why: "noRelease" };
  for (const folder of [HELD, RUNNING, PREVIOUS_CHOICE]) {
    let manifest;
    try {
      manifest = verifyBoxes(fs.readFileSync(path.join(folder, "boxes.json"), "utf8"), keys);
    } catch {
      // (None there, or not signed with a key this box knows: never used, and left.)
      continue;
    }
    if (!running || manifest.version !== running) {
      other = true;
      // (The one kept apart, once this box runs another: no longer its.)
      if (folder === RUNNING) fs.rmSync(RUNNING, { recursive: true, force: true });
      continue;
    }
    if (sha256 && manifest.pack.sha256 !== sha256) continue;
    try {
      return { manifest, pack: fs.readFileSync(path.join(folder, packName(manifest))), folder };
    } catch {
      continue;
    }
  }
  return { result: "none", why: other ? "otherVersion" : "noRelease" };
}

/** Whether `file` here (or `data`, what it would be) is what the release says, by `sha`; the keeper's
 * script whatever its engine line. */
function matches(file: string, sha: string, data: Buffer | null): boolean {
  let here = data;
  if (!here) {
    try {
      if (!fs.lstatSync(path.join(K, file)).isFile()) return false;
      here = fs.readFileSync(path.join(K, file));
    } catch {
      return false;
    }
  }
  if (hexSHA256(here) === sha) return true;
  if (file !== KEEPER || data) return false;
  // (The release's own with this engine line: the same.)
  const packed = released.get(sha);
  return packed !== undefined && withEngineLine(packed, path.join(K, file)).equals(here);
}

/** The keeper's script as the release has it, by its SHA-256, once unpacked (for `matches`). */
const released = new Map<string, Buffer>();

/** `file` as the pack has it (`./<file>`, as packed from a folder, or as named); null if it has none. */
function unpacked(pack: Buffer, file: string): Buffer | null {
  for (const name of ["./" + file, file]) {
    const read = spawnSync("tar", ["-xzOf", "-", name], { input: pack, maxBuffer: 200_000_000, timeout: 120_000 });
    if (read.status === 0 && read.stdout?.length) {
      released.set(hexSHA256(read.stdout), read.stdout);
      return read.stdout;
    }
  }
  return null;
}

/** The keeper's script `data` with the engine line `to` has now (as it is where it has none). */
function withEngineLine(data: Buffer, to: string): Buffer {
  let line: string | undefined;
  try { line = fs.readFileSync(to, "utf8").match(ENGINE_LINE)?.[0]; } catch {}
  return line ? Buffer.from(data.toString("utf8").replace(ENGINE_LINE, () => line!)) : data;
}

/** A file's mode as the pack's listing has it, else a plain file's. */
function modeIn(listing: string, file: string): number {
  const line = listing.split("\n").find(each => each.endsWith(` ${file}`) || each.endsWith(` ./${file}`));
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

/** Where a release of a version is, by its version (the release feed: `KUVO_TEST_RELEASE_FEED` in a
 * test's folder). */
const FEED = "https://github.com/serhatandic/kuvo-code/releases/download";

/** `repair`, as the keeper starts it: a look, and where there's no sound release of the version this
 * box runs kept here (`noRelease`, `otherVersion`, `damaged`), that version fetched from the release
 * feed (its signed manifest and pack, checked as an update checks them), kept apart
 * (`updates/running`), and looked at again. A version the feed doesn't have (a beta from Kuvo
 * Code's own feed) comes from a computer of yours that holds it, through the fleet (`fleet.ts`,
 * `repairPull`), and is used at the next look. */
export async function repairWithFetch(options: { keys?: string[]; feed?: string; now?: number } = {}): Promise<Repair> {
  const first = repairFromRelease(options);
  const missing = first.result === "damaged" || (first.result === "none" && (first.why === "noRelease" || first.why === "otherVersion"));
  const running = boxVersion();
  const sha256 = repairPackSha();
  if (!missing || !running || sha256 === null) return first;
  const keys = options.keys ?? heldKeys();
  try {
    const base = options.feed ?? FEED;
    const boxes = await fetchedPatiently(`${base}/v${running}/boxes.json`, 1_000_000, 30_000);
    const manifest = verifyBoxes(boxes.toString("utf8"), keys);
    if (manifest.version !== running) throw new Error(`the feed's ${running} says ${manifest.version}`);
    if (sha256 && manifest.pack.sha256 !== sha256) throw new Error(`the feed's ${running} is a different build`);
    // (Its pack beside its manifest in the feed, by its name: where its own address points, on
    // the feed; checked by the manifest's SHA-256 whatever it came from.)
    const pack = await fetchedPatiently(`${base}/v${running}/${packName(manifest)}`, 100_000_000, 300_000);
    if (hexSHA256(pack) !== manifest.pack.sha256) throw new Error(`the feed's ${running} isn't what its manifest says`);
    if (boxVersion() !== running || repairPackSha() !== sha256) return first;
    const partial = `${RUNNING}.partial`;
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(partial, "boxes.json"), boxes, { mode: 0o600 });
    fs.writeFileSync(path.join(partial, packName(manifest)), pack, { mode: 0o600 });
    fs.rmSync(RUNNING, { recursive: true, force: true });
    fs.renameSync(partial, RUNNING);
    console.error(`${running}, the version here, fetched from the release feed to repair from`);
  } catch (error) {
    console.error(`${running} couldn't be fetched to repair from:`, (error as Error).message ?? error);
    return first;
  }
  return repairFromRelease(options);
}
