// A new engine (PROTOCOL.md, "A new engine")
// An update that brings another engine swaps the box's code at once and leaves the engine at work:
// the new one waits in `engine/<version>` (the keeper's `V`). This swapper (`kuvo-link.js
// engine-swap`, which the keeper starts while an older engine than its own runs) changes it without
// cutting a turn: at the first quiet moment (nothing at work on the engine for half a minute), or,
// failing one, an hour on in the night's window or a day on whatever the hour, it holds what's sent
// to the engine on the box (shown waiting), lets the turns under way end, moves what the engine's
// inboxes held onto the box, stops the old engine, keeps a copy of its data, and starts the new.
// Once that answers, what was held goes to it, each once, in order. A hold that turns don't end
// within half an hour is let go of (what it held goes to the engine running), and it holds again an
// hour on. A new engine that never answers goes back to the one before where that can read what the
// new one wrote (its release's check said so, or it wrote nothing), and what was held goes there;
// that engine rests a day before it's tried again. The coding agents' bridge, which follows the
// engine by its record, runs on throughout: its turns are never touched.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { HOLD_MOST, readSwap, SWAP_MOST, swapFile, swapUnderWay, type EngineSwap, type SwapStage } from "../../shared/engine-swap";
import { nightOf } from "../../shared/night";
import { engineRuns } from "../../shared/platform";
import type { EngineRecord, Target } from "../../shared/records";
import { sleep } from "../../shared/wait";
import { readAsks } from "../areas/asks";
import { asked, readEngineFile } from "../engine/client";
import { moveInboxToBox, releaseWaiting } from "../engine/queue";
import { listOf, textOf, writePrivate } from "../files";
import { K, REAL_BOX, RUN, UPDATES } from "../home";
import { dataNow, isUpdating, keepData, newerVersion, noteUpdate, roomForData, updateOnTrial, versionOrder } from "./update";

/** How the swapper goes: how often it looks, how long nothing must be at work to be quiet, how long
 * it waits for that before it holds at night or whatever the hour, how long a hold lasts before
 * it's let go of, how long a new engine may fail to answer (three looks in a row, over this long at
 * least) before it's taken for one that won't start, and how long an engine that didn't rests. A
 * test's folder may set its own (`KUVO_TEST_SWAP_*`). */
export const SWAP_TIMING = {
  every: 10_000, quiet: 30_000, patience: 3_600_000, longest: 86_400_000, holdMost: HOLD_MOST,
  failingFor: 60_000, startMost: SWAP_MOST, rest: 86_400_000,
};
if (!REAL_BOX) for (const [key, name] of [["every", "EVERY"], ["quiet", "QUIET"], ["patience", "PATIENCE"], ["longest", "LONGEST"], ["holdMost", "HOLD_MOST"], ["failingFor", "FAILING_FOR"], ["startMost", "START_MOST"]] as const) {
  const given = Number(process.env[`KUVO_TEST_SWAP_${name}`]);
  if (process.env[`KUVO_TEST_SWAP_${name}`] && given >= 0) SWAP_TIMING[key] = given;
}

/** An engine that didn't start, and the one gone back to: `run/engine-back`, `<that one> <the one
 * before> <when, s>`, which the keeper reads to start the one before meanwhile (its `V`). */
export const BACK_FILE = path.join(RUN, "engine-back");
/** Held by the swapper while it has the engine stopped: the keeper starts none meanwhile. */
const SWAPPING_MARK = path.join(RUN, ".engine-swapping");
/** Update Now said for another engine: the swapper holds at once, without waiting for a quiet
 * moment (the turns under way still end first). */
export const SWAP_NOW = path.join(RUN, "engine-swap.now");

/** How the last engine change went (`run/update.json`'s `engine`). */
export interface EngineChangeLast {
  at: number;
  result: "changed" | "back" | "failed";
  from: string;
  to: string;
  message?: string;
}

const engineFolder = (version: string) => path.join(K, "engine", version);
const engineThere = (version: string) => fs.existsSync(path.join(engineFolder(version), "opencode"));

/** The engine the keeper in place starts (its `V`, while that's there, else the newest there), as
 * `bin/kuvo-start` says it, an engine that didn't start aside. */
export function keeperEngine(): string | null {
  const said = /^V=([0-9][0-9A-Za-z.-]*)$/m.exec(textOf(path.join(K, "bin", "kuvo-start")) ?? "")?.[1];
  if (said && engineThere(said)) return said;
  const there = listOf(path.join(K, "engine")).filter(name => !name.startsWith(".") && engineThere(name) && versionOrder(name));
  return there.sort((a, b) => (newerVersion(a, b) ? 1 : newerVersion(b, a) ? -1 : 0)).at(-1) ?? null;
}

/** An engine that didn't start, while it rests: which, the one gone back to, and when (ms). */
export function readBack(): { to: string; from: string; at: number } | null {
  const [to, from, at] = (textOf(BACK_FILE) ?? "").trim().split(/\s+/);
  return to && from && Number(at) ? { to, from, at: Number(at) * 1000 } : null;
}

/** The engine running, as its record says, while its process runs. */
function runningEngine(): EngineRecord | null {
  try {
    const record = readEngineFile();
    return record?.pid && engineRuns(record.pid, K) && record.port && record.password ? record : null;
  } catch {
    return null;
  }
}

const stoppedOnPurpose = () => ["stopped", "paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark)));

/** The change to make now, from the engine running to the keeper's, newer; null for none (the same,
 * older, not there, stopped on purpose, an update under way or on trial, or one that rests). */
export function swapDue(now = Date.now()): { from: string; to: string } | null {
  if (stoppedOnPurpose() || updateOnTrial() || isUpdating()) return null;
  const from = runningEngine()?.version;
  const to = keeperEngine();
  if (!from || !to || from === to || !newerVersion(to, from) || !engineThere(to)) return null;
  const back = readBack();
  if (back?.to === to && now - back.at < SWAP_TIMING.rest) return null;
  return { from, to };
}

/** Whether anything is at work on the engine: a conversation (a turn, a subagent's, one retrying, an
 * ask waiting on you), or the agent's browser waiting on you. Unknown counts as at work. */
async function engineBusy(engine: Target): Promise<boolean> {
  if (readAsks().length > 0) return true;
  const active = await asked(engine, "GET", "api/session/active").then(answer => (answer as { data?: unknown } | null)?.data, () => undefined);
  if (Array.isArray(active)) return active.length > 0;
  if (active && typeof active === "object") return Object.keys(active).length > 0;
  return true;
}

// The keeper's command `command` (its stop of the engine alone, or its start), with HOME as it runs.
function keeper(command: string): boolean {
  return spawnSync("sh", [path.join(K, "bin", "kuvo-start"), command], { cwd: K, env: process.env, stdio: "ignore", timeout: 300_000 }).status === 0;
}

/** Whether the new engine said it reads what it writes when the one before opens it again (its
 * release's check, `box.json`'s `downgradeSafe`, as the update noted it in `updates/installed.json`). */
function downgradeSafe(to: string, from: string): boolean {
  try {
    const installed = JSON.parse(fs.readFileSync(path.join(UPDATES, "installed.json"), "utf8"));
    return installed?.engine === to && installed.downgradeSafe === true && Array.isArray(installed.engines) && installed.engines.includes(from);
  } catch {
    return false;
  }
}

/** How a swapper goes (a test's own): its clock, how it waits. */
export interface SwapOptions {
  clock?: () => number;
  wait?: (ms: number) => Promise<unknown>;
}

/** What a swap came to: the new engine runs; it didn't start and the one before runs again; it
 * didn't start and couldn't go back; a hold let go of to look again later is still `waiting`
 * underneath; or nothing to do. */
export type Swapped = "changed" | "back" | "failed" | "none";

/** Changes the engine as above, staying until it's done (or there's nothing to do). */
export async function swapEngine({ clock = Date.now, wait = sleep }: SwapOptions = {}): Promise<Swapped> {
  const due = swapDue(clock());
  if (!due) return "none";
  // One at a time: another swapper at it, this one goes.
  const other = swapUnderWay(RUN);
  if (other && other.pid !== process.pid) return "none";
  const state: EngineSwap = { from: due.from, to: due.to, since: readSwap(RUN)?.to === due.to ? readSwap(RUN)!.since : clock(), stage: "waiting", pid: process.pid };
  const say = (stage: SwapStage, change: Partial<EngineSwap> = {}) => {
    Object.assign(state, { stage }, change);
    if (stage === "waiting") delete state.holding;
    writePrivate(swapFile(RUN), JSON.stringify(state));
  };
  say("waiting");
  let quietSince: number | null = null;
  try {
    for (;;) {
      await wait(SWAP_TIMING.every);
      const now = clock();
      const still = swapDue(now);
      const engine = runningEngine();
      if (!still || still.to !== state.to || !engine) {
        // (Changed by something else meanwhile, a crash or a stop, or an update came: nothing held
        // stays held.)
        if (state.stage === "holding" && engine) await releaseWaiting(engine).catch(() => 0);
        return "none";
      }
      const busy = await engineBusy(engine);
      if (state.stage === "waiting") {
        quietSince = busy ? null : quietSince ?? now;
        const waited = now - state.since;
        const rested = state.letGo === undefined || now - state.letGo >= SWAP_TIMING.patience;
        const nowAsked = fs.existsSync(SWAP_NOW);
        if ((quietSince !== null && now - quietSince >= SWAP_TIMING.quiet) || (rested && (nowAsked || waited >= SWAP_TIMING.longest || (waited >= SWAP_TIMING.patience && nightOf(new Date(now)) !== null)))) {
          say("holding", { holding: now });
          continue;
        }
      } else if (state.stage === "holding") {
        if (!busy) {
          // (Looked at again a moment on: a send just before the hold may have started one.)
          await wait(Math.min(1000, SWAP_TIMING.every));
          if (!(await engineBusy(engine))) {
            const done = await swapNow(engine, state, say, clock, wait);
            if (done !== null) return done;
            continue;
          }
        }
        if (now - state.holding! >= SWAP_TIMING.holdMost) {
          // Turns that don't end (one waiting on you overnight): let go of, what was held going to
          // the engine at work; held again an hour on.
          say("waiting", { letGo: now });
          quietSince = null;
          fs.rmSync(SWAP_NOW, { force: true });
          await releaseWaiting(engine).catch(() => 0);
        }
      }
    }
  } finally {
    fs.rmSync(SWAPPING_MARK, { recursive: true, force: true });
    // (Whatever came of it, nothing is held once it's gone: what waits goes with the next look.)
    if (readSwap(RUN)?.pid === process.pid) fs.rmSync(swapFile(RUN), { force: true });
  }
}

// The swap itself, nothing at work: what the engine's inboxes hold onto the box, the old engine
// stopped, its data kept, the new one started and waited for; what was held to it once it answers,
// else back to the one before where it can be. Null: it didn't go (something came up), and it goes
// on holding.
async function swapNow(engine: EngineRecord, state: EngineSwap, say: (stage: SwapStage, change?: Partial<EngineSwap>) => void, clock: () => number, wait: (ms: number) => Promise<unknown>): Promise<Swapped | null> {
  say("swapping");
  // What each conversation's inbox holds, onto the box (an engine stopped loses what it held).
  try {
    const page = await asked(engine, "GET", "api/session?parentID=null&limit=100&order=desc") as { data?: { id?: string; time?: { updated?: number } }[] } | null;
    const week = clock() - 7 * 86_400_000;
    for (const session of page?.data ?? []) {
      if (typeof session.id === "string" && (session.time?.updated ?? clock()) >= week) await moveInboxToBox(engine, session.id);
    }
  } catch {
    say("holding");
    return null;
  }
  if (await engineBusy(engine)) {
    say("holding");
    return null;
  }
  if (!roomForData()) {
    // (Rests a day, as one that didn't start: the keeper's engine meanwhile is the one at work.)
    writePrivate(BACK_FILE, `${state.to} ${state.from} ${Math.floor(clock() / 1000)}\n`);
    return ended(state, "failed", "There isn’t room to keep a copy of the engine’s data, so the engine update waits a day.", engine, clock);
  }
  fs.mkdirSync(SWAPPING_MARK, { recursive: true, mode: 0o700 });
  writePrivate(path.join(SWAPPING_MARK, "pid"), String(process.pid));
  if (!keeper("engine-halt")) {
    fs.rmSync(SWAPPING_MARK, { recursive: true, force: true });
    say("holding");
    return null;
  }
  // (A copy of its data, never put back by anything: for you to restore by hand, as a last resort.)
  const kept = keepData(`${new Date(clock()).toISOString().slice(0, 10)}-${state.from}-to-${state.to}`);
  keeper("engine");
  fs.rmSync(SWAPPING_MARK, { recursive: true, force: true });
  const fresh = await answering(state.to, clock, wait);
  if (fresh) return ended(state, "changed", undefined, fresh, clock);
  // Didn't start: back to the one before, where it can read what this one wrote.
  if (!downgradeSafe(state.to, state.from) && dataNow() !== kept) {
    return ended(state, "failed", `The new engine (${state.to}) didn’t start, and the one before can’t open what it wrote, so it keeps trying.`, null, clock);
  }
  say("back");
  writePrivate(BACK_FILE, `${state.to} ${state.from} ${Math.floor(clock() / 1000)}\n`);
  fs.mkdirSync(SWAPPING_MARK, { recursive: true, mode: 0o700 });
  writePrivate(path.join(SWAPPING_MARK, "pid"), String(process.pid));
  keeper("engine-halt");
  keeper("engine");
  fs.rmSync(SWAPPING_MARK, { recursive: true, force: true });
  const before = await answering(state.from, clock, wait);
  return ended(state, before ? "back" : "failed", before ? `The new engine (${state.to}) didn’t start, so Kuvo went back to the one before. It tries again in a day.` : `Neither the new engine (${state.to}) nor the one before started.`, before, clock);
}

// The engine `version` once it answers, judged as the update's health rule judges one starting: it
// runs (the keeper starts it again should it end) and answers; one whose log grows is starting
// still. Null once it fails three looks in a row over `failingFor` at least, or after `startMost`.
async function answering(version: string, clock: () => number, wait: (ms: number) => Promise<unknown>): Promise<EngineRecord | null> {
  const began = clock();
  let failing: number | null = null, looks = 0, log = -1;
  for (;;) {
    const record = runningEngine();
    if (record?.version === version && (await asked(record, "GET", "api/session?limit=1").then(() => true, () => false))) return record;
    let size = -1;
    try { size = fs.statSync(path.join(RUN, "engine.log")).size; } catch {}
    const starting = record?.version === version && size > log;
    log = size;
    if (starting) { failing = null; looks = 0; }
    else { failing ??= clock(); looks++; }
    if ((looks >= 3 && clock() - failing! >= SWAP_TIMING.failingFor) || clock() - began >= SWAP_TIMING.startMost) return null;
    await wait(Math.min(SWAP_TIMING.every, 2000));
  }
}

// How it ended, noted (`run/update.json`'s `engine`), and what was held sent to the engine that
// answers (none: it waits on the box, for the next engine that does).
async function ended(state: EngineSwap, result: EngineChangeLast["result"], message: string | undefined, engine: Target | null, clock: () => number): Promise<Swapped> {
  noteUpdate({ engine: { at: clock(), result, from: state.from, to: state.to, ...(message ? { message } : {}) } });
  fs.rmSync(SWAP_NOW, { force: true });
  if (result === "changed") fs.rmSync(BACK_FILE, { force: true });
  if (engine) {
    await releaseWaiting(engine).catch(() => 0);
    // (Sent while that went: once more, now that nothing holds.)
    fs.rmSync(swapFile(RUN), { force: true });
    await releaseWaiting(engine).catch(() => 0);
  }
  return result;
}
