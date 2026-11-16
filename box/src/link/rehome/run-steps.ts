// What a move the box runs whole itself does that the keeper's never did (`rehome --run`, #713
// §4.2; PROTOCOL.md, "Moving this Mac's box"): this Mac's keys handed over by `kuvo-keys`, what an
// earlier Kuvo Code kept brought into the box's own files, the engine's database brought up to
// date, what it holds counted before and after, and the look that nothing still has the earlier
// Kuvo Code's database open. The last three and the upgrades work in the staged copy alone: the
// app's folder is read, never written, and its database never opened.
// Platform: a Mac's alone (`lsof`, the Keychain, the helper); nothing here runs on Linux.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { databaseCounts, databasesIn, soundness, tableCounts } from "./database";
import { mapped, moves } from "./paths";
import { migrateDatabases } from "./migrate";
import { Said } from "./said";
import { noteRecord, readRecord } from "./state";
import { placementOf, stagedEngine, type MoveContext } from "./rewrite";
import type { RunPlan } from "./state";

/** The helper's name, as its release brings it (`helper.ts`'s, which this can't load: the move's
 * own files are read by tests that load nothing else of the box). */
const HELPER_NAME = "Kuvo Helper";
const exists = (file: string): boolean => { try { fs.lstatSync(file); return true; } catch { return false; } };
const listOf = (folder: string): string[] => { try { return fs.readdirSync(folder); } catch { return []; } };
const foldersIn = (folder: string): string[] => listOf(folder).filter(name => { try { return !name.startsWith(".") && fs.statSync(path.join(folder, name)).isDirectory(); } catch { return false; } });

// The Keychain

/** A short run of one of the Keychain's tools: its last line of JSON, and how it ended. Run by
 * this very process, with no shell between: `kuvo-keys` works only for the process that holds the
 * move's lock. */
function tool(program: string, args: string[], timeout = 180_000): { status: number | null; said: Record<string, unknown> | null; errors: string } {
  const ran = spawnSync(program, args, { encoding: "utf8", timeout, stdio: ["ignore", "pipe", "pipe"] });
  let said: unknown = null;
  try { said = JSON.parse((ran.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
  return { status: ran.error ? null : ran.status, said: isObject(said) ? said : null, errors: ran.error ? ran.error.message : (ran.stderr ?? "").trim().split("\n").pop() ?? "" };
}

/** The helper the release brings, which the copies are made to trust (the one put in the box's
 * `bin` is the same program: one signature), or the box's own once the release is gone. */
function helperOf(run: RunPlan, home: string): string {
  const inRelease = path.join(run.release, "bin", `${HELPER_NAME}.app`);
  return exists(inRelease) ? inRelease : path.join(home, "bin", `${HELPER_NAME}.app`);
}
const helperProgram = (bundle: string) => path.join(bundle, "Contents", "MacOS", HELPER_NAME);

const LOCKED = "Your Keychain is locked, so Kuvo can't take this Mac's keys along. Unlock it (log out and back in, or open Keychain Access), then try again.";
/** macOS kept an item from `kuvo-keys` (an earlier Kuvo Code signed differently made it, and it
 * asked, or would have): you allow it when it asks. */
const NOT_ALLOWED = "Kuvo needs your OK to take this Mac's keys along. Try again: macOS asks once for each key whether kuvo-keys may use it. Each time, enter your Mac's password and click Always Allow.";
const NOT_TAKEN = "Kuvo couldn't take this Mac's keys along, so nothing was moved. Try again.";
/** How long `kuvo-keys copy` may take: macOS may ask you about each item it reads. */
const ASKING = 1_800_000;

/** This Mac's keys copied for the helper (`kuvo-keys copy`), and read back by it (`keys check`):
 * what an earlier box or an earlier try left under the helper's names is taken away first, once.
 * Throws in words to show as they are. Nothing where the app brings no `kuvo-keys` (a copy of the
 * app, which has no keys of Kuvo Code's own to hand over). */
export function handOverKeys(run: RunPlan, home: string): void {
  if (!run.keys) return;
  const helper = helperOf(run, home);
  for (let tries = 0; ; tries++) {
    const copied = tool(run.keys, ["copy", "--home", home, "--helper", helper], ASKING);
    if (copied.status === 0) break;
    const reason = typeof copied.said?.reason === "string" ? copied.said.reason : "";
    // (An earlier box's items, or an earlier try's copies: taken away by the helper, then once more.)
    if (tries === 0 && (reason === "leftovers" || reason === "journal")) {
      const cleared = tool(helperProgram(helper), ["keys", reason === "leftovers" ? "clear" : "uncopy", "--home", home]);
      if (cleared.status === 0) continue;
    }
    // (What it said goes to the move's own log alone, for support: it names an item, and what the
    // Keychain said. What you're shown is in words, by its one word for why.)
    const status = typeof copied.said?.status === "number" ? ` (OSStatus ${copied.said.status})` : "";
    console.error(`kuvo-keys copy: ${reason || (copied.status === null ? "didn't finish" : `exit ${copied.status}`)}${status}: ${typeof copied.said?.error === "string" ? copied.said.error : copied.errors || "it said nothing"}`);
    if (reason === "locked") throw new Said(LOCKED);
    if (reason === "not-allowed") throw new Said(NOT_ALLOWED);
    throw new Said(NOT_TAKEN);
  }
  const checked = tool(helperProgram(helper), ["keys", "check", "--home", home]);
  if (checked.status !== 0) {
    const unreadable = Array.isArray(checked.said?.unreadable) ? checked.said.unreadable.length : 0;
    if (!unreadable) console.error(`Kuvo Helper keys check: ${checked.errors || "it said nothing"}`);
    throw new Said(unreadable ? `Kuvo couldn't read ${unreadable === 1 ? "one of this Mac's keys" : `${unreadable} of this Mac's keys`} back after copying ${unreadable === 1 ? "it" : "them"}. Try again.` : "Kuvo couldn't check this Mac's keys after copying them, so nothing was moved. Try again.");
  }
}

/** The copies taken away again (the helper's `keys uncopy`), as a move goes back: the app's own
 * were never touched. Throws when they couldn't be: they stay journaled, and go before the next copy. */
export function takeBackKeys(run: RunPlan, home: string): void {
  if (!run.keys || !exists(path.join(home, "updates", "rehome", "keychain.json"))) return;
  const helper = helperOf(run, home);
  const undone = tool(helperProgram(helper), ["keys", "uncopy", "--home", home]);
  if (undone.status !== 0) throw new Error(undone.errors || "its helper said nothing");
}

/** The app's own Keychain items taken away, once the move can't go back (`kuvo-keys
 * delete-originals`): whether it's done. */
export function deleteOriginalKeys(keys: string, home: string): { done: boolean; why?: string } {
  const deleted = tool(keys, ["delete-originals", "--home", home]);
  return deleted.status === 0 ? { done: true } : { done: false, why: typeof deleted.said?.error === "string" ? deleted.said.error : deleted.errors };
}

// Who has the earlier Kuvo Code's database open

/** The programs that have an engine database in `folder` open for writing now (its log and index
 * too), by name: an earlier Kuvo Code still open, or its engine left running. One that only reads
 * it (a backup, Spotlight) holds nothing back. Asked of the system (`lsof`), never by opening the
 * database: that could rewrite the log's index beside it, and the app's folder is never changed.
 * Where it can't be asked, none (what changes meanwhile is still caught before the move goes
 * forward). */
export function heldOpen(folder: string): string[] {
  const files = databasesIn(folder).flatMap(file => [file, `${file}-wal`, `${file}-shm`]).filter(exists);
  if (!files.length || process.platform !== "darwin") return [];
  // (Each process, its name, then each of its files there with how it's open: `r`, `w` or `u`, both.)
  const asked = spawnSync("/usr/sbin/lsof", ["-F", "pcfa", "--", ...files], { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "ignore"] });
  const names = new Set<string>();
  let pid = 0, name = "";
  for (const line of (asked.stdout ?? "").split("\n")) {
    if (line.startsWith("p")) { pid = Number(line.slice(1)); name = ""; }
    else if (line.startsWith("c")) name = line.slice(1);
    else if (line.startsWith("a") && /[wu]/.test(line.slice(1)) && pid !== process.pid && name) names.add(name);
  }
  return [...names];
}

/** Why a move can't go on while something has the earlier Kuvo Code's database open, or null. */
export function heldRefusal(appFolder: string): string | null {
  const held = heldOpen(path.join(appFolder, "opencode", "data", "opencode"));
  // (Which, for support, in the move's log alone.)
  if (held.length) console.error(`The earlier Kuvo Code's database is open for writing in: ${held.join(", ")}`);
  return held.length ? "Your sessions are still open in another program on this Mac. Quit Kuvo Code wherever it's open, then try again." : null;
}

/** Looked at once more before the move goes forward: nothing has its database open. */
export function beforeForward(_run: RunPlan, appFolder: string): void {
  const held = heldRefusal(appFolder);
  if (held) throw new Said(held);
}

// The engine's database

/** The staged engine database brought up to date by the release's engine, staged beside it. */
export async function migrateStaged(_run: RunPlan, move: MoveContext): Promise<void> {
  const data = placementOf(move, "data");
  if (!data || !databasesIn(path.join(data.staged, "opencode")).length) return;
  const engine = stagedEngine(move);
  if (!engine) throw new Said("Kuvo's engine isn't here to bring your sessions up to date with. Check that this Mac is online, then try again.");
  const migrated = await migrateDatabases({ engine, data: data.staged, scratch: path.join(move.work, "migrate") });
  // What the engine's own update let go of is said: messages that were waiting on a session when
  // the earlier Kuvo Code last quit (its engine kept them in a table the pinned one has no more).
  const waiting = migrated.reduce((sum, each) => sum + (each.gone.session_pending ?? 0), 0);
  if (waiting) noteRecord(move.home, { dropped: [...(readRecord(move.home).dropped ?? []), `${waiting === 1 ? "A message" : `${waiting} messages`} still waiting to reach an agent when the earlier Kuvo Code last quit ${waiting === 1 ? "wasn't" : "weren't"} kept: send ${waiting === 1 ? "it" : "them"} again.`] });
}

// The upgrades

/** What an earlier Kuvo Code kept in its own files and preferences, brought into the box's own in
 * the staged copy (`upgrades.ts`): what it didn't carry, in words to show once. */
export async function upgradeStaged(run: RunPlan, move: MoveContext): Promise<string[]> {
  // (Loaded as it's needed: it reads with the box's own readers, which the move's own files,
  // loaded alone by its tests, don't bring.)
  const { runUpgrades } = await import("./upgrades");
  return runUpgrades(run, move);
}

// Counts

/** Where a place in the box's home (`bots`, `box`, …) is staged, or null where it isn't moving. */
export function stagedPlace(move: MoveContext, name: string): string | null {
  const [top, ...rest] = name.split("/");
  const placement = placementOf(move, top);
  return placement ? path.join(placement.staged, ...rest) : null;
}

/** What the staged copy holds of yours, counted: sessions and projects (the engine's databases),
 * messages, bots, conversations' wake-ups, worktrees (the engine's, and the box's own records),
 * vault items, and the accounts added to coding agents. */
export function stagedCounts(move: MoveContext): Record<string, number> {
  const counts: Record<string, number> = { sessions: 0, projects: 0, messages: 0, worktrees: 0 };
  const data = stagedPlace(move, "data/opencode");
  for (const database of data ? databasesIn(data) : []) {
    const held = databaseCounts(database), rows = tableCounts(database);
    counts.sessions += held.sessions;
    counts.projects += held.projects;
    counts.messages += rows.session_message ?? 0;
    counts.worktrees += rows.worktree ?? 0;
  }
  const at = (name: string) => stagedPlace(move, name) ?? "";
  counts.bots = foldersIn(at("bots")).length;
  counts.wakeUps = foldersIn(at("wakes")).length + foldersIn(at("bots")).reduce((sum, bot) => sum + listOf(path.join(at("bots"), bot, "wakes")).filter(name => !name.startsWith(".")).length, 0);
  let records: unknown = null;
  try { records = JSON.parse(fs.readFileSync(path.join(at("box"), "worktrees.json"), "utf8")); } catch {}
  counts.worktreeRecords = isObject(records) && Array.isArray(records.worktrees) ? records.worktrees.length : 0;
  counts.vaultItems = listOf(path.join(at("box"), "vault", "items")).filter(name => name.endsWith(".json")).length;
  counts.accounts = foldersIn(at("agents")).reduce((sum, agent) => sum + foldersIn(path.join(at("agents"), agent, "accounts")).length, 0);
  counts.groupChats = foldersIn(at("groups")).length;
  // (Rows of the engine's databases that point at a row that isn't there: an engine's own
  // leftovers, which the move may not add to.)
  counts.brokenLinks = (data ? databasesIn(data) : []).reduce((sum, database) => sum + soundness(database).brokenLinks, 0);
  return counts;
}

/** Links in what's staged that point, by a whole path, into what moves (a link in a chat's folder
 * to a file in another, say): pointed at where that will be, as every other path is. A link
 * that names another by a path from its own place stays as it is. How many. */
export function relinkStaged(move: MoveContext): number {
  let changed = 0;
  const walk = (folder: string, depth: number) => {
    let entries: fs.Dirent[] = [];
    try { entries = fs.readdirSync(folder, { withFileTypes: true }); } catch {}
    for (const entry of entries) {
      const file = path.join(folder, entry.name);
      if (entry.isSymbolicLink()) {
        let target = "";
        try { target = fs.readlinkSync(file); } catch { continue; }
        if (!path.isAbsolute(target) || !moves(target, move.map)) continue;
        fs.rmSync(file, { force: true });
        fs.symlinkSync(mapped(target, move.map), file);
        changed++;
      } else if (entry.isDirectory() && depth < 40) walk(file, depth + 1);
    }
  };
  for (const placement of move.placements) if (!placement.installed) walk(placement.staged, 0);
  return changed;
}

const countsFile = (move: MoveContext) => path.join(move.work, "run-counts.json");

/** The counts as cloned, before anything's upgraded or rewritten, kept for the check. */
export function noteCounts(move: MoveContext, _when: "before"): void {
  fs.mkdirSync(move.work, { recursive: true, mode: 0o700 });
  fs.writeFileSync(countsFile(move), JSON.stringify(stagedCounts(move)), { mode: 0o600 });
}

/** The counts now held to those as cloned: none fewer (the upgrades may add: a group chat, a
 * worktree's record). Throws with what's missing; else both, for support. */
export function countsHold(move: MoveContext): Record<string, number> {
  let before: Record<string, number> = {};
  try { before = JSON.parse(fs.readFileSync(countsFile(move), "utf8")); } catch {
    throw new Error("the counts taken after the clone can't be read");
  }
  // The engine's databases read whole, with no more broken links than they came with.
  const data = stagedPlace(move, "data/opencode");
  for (const database of data ? databasesIn(data) : []) if (!soundness(database).whole) throw new Error(`${path.basename(database)} doesn't pass its integrity check once moved`);
  const after = stagedCounts(move);
  if ((after.brokenLinks ?? 0) > (before.brokenLinks ?? 0)) throw new Error(`the engine's database has ${after.brokenLinks} broken links once moved, ${before.brokenLinks ?? 0} before`);
  const fewer = Object.keys(before).filter(name => name !== "brokenLinks" && (after[name] ?? 0) < before[name]);
  if (fewer.length) throw new Said(`Something went missing as your sessions and settings were copied (${fewer.map(name => `${name}: ${after[name] ?? 0} of ${before[name]}`).join(", ")}), so nothing was changed.`);
  return Object.fromEntries(Object.keys(after).flatMap(name => [[`${name}Before`, before[name] ?? 0], [name, after[name]]]));
}
