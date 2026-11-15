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
import { databaseCounts, databasesIn, tableCounts } from "./database";
import { migrateDatabases } from "./migrate";
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
function tool(program: string, args: string[]): { status: number | null; said: Record<string, unknown> | null; errors: string } {
  const ran = spawnSync(program, args, { encoding: "utf8", timeout: 180_000, stdio: ["ignore", "pipe", "pipe"] });
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

/** This Mac's keys copied for the helper (`kuvo-keys copy`), and read back by it (`keys check`):
 * what an earlier box or an earlier try left under the helper's names is taken away first, once.
 * Throws in words to show as they are. Nothing where the app brings no `kuvo-keys` (a copy of the
 * app, which has no keys of Kuvo Code's own to hand over). */
export function handOverKeys(run: RunPlan, home: string): void {
  if (!run.keys) return;
  const helper = helperOf(run, home);
  for (let tries = 0; ; tries++) {
    const copied = tool(run.keys, ["copy", "--home", home, "--helper", helper]);
    if (copied.status === 0) break;
    const reason = typeof copied.said?.reason === "string" ? copied.said.reason : "";
    // (An earlier box's items, or an earlier try's copies: taken away by the helper, then once more.)
    if (tries === 0 && (reason === "leftovers" || reason === "journal")) {
      const cleared = tool(helperProgram(helper), ["keys", reason === "leftovers" ? "clear" : "uncopy", "--home", home]);
      if (cleared.status === 0) continue;
    }
    if (reason === "refused") throw new Error(LOCKED);
    throw new Error(`Kuvo couldn't take this Mac's keys along (${typeof copied.said?.error === "string" ? copied.said.error : copied.errors || "kuvo-keys said nothing"}). Try again.`);
  }
  const checked = tool(helperProgram(helper), ["keys", "check", "--home", home]);
  if (checked.status !== 0) {
    const unreadable = Array.isArray(checked.said?.unreadable) ? checked.said.unreadable.length : 0;
    throw new Error(unreadable ? `Kuvo couldn't read ${unreadable === 1 ? "one of this Mac's keys" : `${unreadable} of this Mac's keys`} back after copying ${unreadable === 1 ? "it" : "them"}. Try again.` : `Kuvo couldn't check this Mac's keys after copying them (${checked.errors || "its helper said nothing"}). Try again.`);
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

/** The programs that have an engine database in `folder` open now (its log and index too), by
 * name: an earlier Kuvo Code still open, or its engine left running. Asked of the system
 * (`lsof`), never by opening the database: that could rewrite the log's index beside it, and
 * the app's folder is never changed. Where it can't be asked, none (what changes meanwhile is
 * still caught before the move goes forward). */
export function heldOpen(folder: string): string[] {
  const files = databasesIn(folder).flatMap(file => [file, `${file}-wal`, `${file}-shm`]).filter(exists);
  if (!files.length || process.platform !== "darwin") return [];
  const asked = spawnSync("/usr/sbin/lsof", ["-F", "pc", "--", ...files], { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "ignore"] });
  const names = new Set<string>();
  let pid = 0;
  for (const line of (asked.stdout ?? "").split("\n")) {
    if (line.startsWith("p")) pid = Number(line.slice(1));
    else if (line.startsWith("c") && pid !== process.pid) names.add(line.slice(1));
  }
  return [...names];
}

/** Why a move can't go on while something has the earlier Kuvo Code's database open, or null. */
export function heldRefusal(appFolder: string): string | null {
  const held = heldOpen(path.join(appFolder, "opencode", "data", "opencode"));
  return held.length ? `An earlier Kuvo Code is still running on this Mac (${held.join(", ")}). Quit Kuvo Code wherever it's open, then try again.` : null;
}

/** Looked at once more before the move goes forward: nothing has its database open. */
export function beforeForward(_run: RunPlan, appFolder: string): void {
  const held = heldRefusal(appFolder);
  if (held) throw new Error(held);
}

// The engine's database

/** The staged engine database brought up to date by the release's engine, staged beside it. */
export async function migrateStaged(_run: RunPlan, move: MoveContext): Promise<void> {
  const data = placementOf(move, "data");
  if (!data || !databasesIn(path.join(data.staged, "opencode")).length) return;
  const engine = stagedEngine(move);
  if (!engine) throw new Error("Kuvo's engine isn't here to bring your sessions up to date with. Check that this Mac is online, then try again.");
  await migrateDatabases({ engine, data: data.staged, scratch: path.join(move.work, "migrate") });
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
  return counts;
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
    throw new Error("What your sessions and settings held wasn't counted before they were copied.");
  }
  const after = stagedCounts(move);
  const fewer = Object.keys(before).filter(name => (after[name] ?? 0) < before[name]);
  if (fewer.length) throw new Error(`Something went missing as your sessions and settings were copied (${fewer.map(name => `${name}: ${after[name] ?? 0} of ${before[name]}`).join(", ")}), so nothing was changed.`);
  return Object.fromEntries(Object.keys(after).flatMap(name => [[`${name}Before`, before[name] ?? 0], [name, after[name]]]));
}
