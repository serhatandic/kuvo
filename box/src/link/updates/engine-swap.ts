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
import { readInstalled } from "../../shared/release-formats";
import { engineSwapFormat, HOLD_MOST, readSwap, SWAP_MOST, swapFile, swapUnderWay, type EngineSwap, type SwapStage } from "../../shared/engine-swap";
import { nightOf } from "../../shared/night";
import { engineRuns, processRuns } from "../../shared/platform";
import type { EngineRecord, Target } from "../../shared/records";
import { sleep } from "../../shared/wait";
import { readAsks } from "../areas/asks";
import { asked, engineGet, readBridgeFile, readEngineFile } from "../engine/client";
import { atWork, openFolders } from "../engine/models";
import { moveInboxToBox, releaseWaiting } from "../engine/queue";
import { listOf, textOf, writePrivate } from "../files";
import { forgetMachineCron, machineHandoverDone, machineHelperAvailable, machineHelperInstalled, machineJob, machineJobFile, machineKeeperIdentity, machineKeeperRuns, machineRetryFile, previousMachineInstall, restoreMachineJob, saveMachineJob, startLegacyMachine, startMachineHelper, watchMachinePermissions } from "../helper";
import { machineFilesAllowed } from "../permissions";
import { K, REAL_BOX, RUN, UPDATES } from "../home";
import { agentsWorking, dataNow, isUpdating, keepData, newerVersion, noteUpdate, roomForData, takeUpdateLock, UPDATE_LOCK, updateOnTrial, versionOrder } from "./update";
import { performsMutation } from "../ownership/admission";

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

/** Whether anything is at work on the engine that stopping it would cut: a conversation (a turn, a
 * subagent's, one retrying), in any folder it has open a question or permission waiting on you, a
 * terminal or a command an agent left running (as closing a folder on it counts them,
 * `engine/models.ts`), or the agent's browser waiting on you. Unknown counts as at work. */
export async function engineBusy(engine: Target): Promise<boolean> {
  if (readAsks().length > 0) return true;
  const active = await asked(engine, "GET", "api/session/active").then(answer => (answer as { data?: unknown } | null)?.data, () => undefined);
  if (Array.isArray(active) ? active.length > 0 : active && typeof active === "object" ? Object.keys(active).length > 0 : true) return true;
  try {
    const folders = await openFolders(engine);
    return (await atWork(engine, folders)).size > 0;
  } catch {
    return true;
  }
}

/** What waits on you there and doesn't end by itself, which holding wouldn't outlast: an open
 * terminal (`api/pty`), a question or permission waiting on you, the agent's browser asking. A hold
 * isn't begun for the time that's come while one is there (only a quiet moment, or your word,
 * changes the engine then), and the line says so. Null: none (or it can't be told). */
async function waitsOnYou(engine: Target): Promise<"terminal" | "ask" | null> {
  if (readAsks().length > 0) return "ask";
  try {
    for (const folder of await openFolders(engine)) {
      const [forms, permissions, terminals] = await Promise.all(["api/form", "api/permission/request", "api/pty"].map(route => engineGet(engine, route, folder)));
      const list = (answer: unknown): unknown[] => (answer && typeof answer === "object" && Array.isArray((answer as { data?: unknown }).data) ? (answer as { data: unknown[] }).data : Array.isArray(answer) ? answer : []);
      if (list(terminals).some(each => !each || typeof each !== "object" || (each as { status?: unknown }).status === "running")) return "terminal";
      if (list(forms).length || list(permissions).length) return "ask";
    }
  } catch {}
  return null;
}

// The keeper's command `command` (its stop of the engine alone, or its start), with HOME as it runs.
function keeper(command: string): boolean {
  return spawnSync("sh", [path.join(K, "bin", "kuvo-start"), command], { cwd: K, env: process.env, stdio: "ignore", timeout: 300_000 }).status === 0;
}

/** Whether the new engine said it reads what it writes when the one before opens it again (its
 * release's check, `box.json`'s `downgradeSafe`, as the update noted it in `updates/installed.json`). */
function downgradeSafe(to: string, from: string): boolean {
  try {
    const installed = readInstalled(UPDATES);
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
  // A swapper that went while it held (the machine down, killed): what it held goes now, to the
  // engine that runs, and its state with it.
  const stale = readSwap(RUN);
  if (stale && swapUnderWay(RUN)) return "none";
  fs.rmSync(swapFile(RUN), { force: true });
  // Nothing holds now: whatever waits on the box for a conversation that isn't paused (one that went
  // while it held, an engine that ended meanwhile, a stop) goes to the engine that runs.
  const engine = runningEngine();
  if (engine) await releaseWaiting(engine).catch(() => 0);
  const due = swapDue(clock());
  if (!due) return "none";
  // One at a time: another swapper at it, this one goes.
  const other = swapUnderWay(RUN);
  if (other && other.pid !== process.pid) return "none";
  const state: EngineSwap = { from: due.from, to: due.to, since: stale?.to === due.to ? stale.since : clock(), stage: "waiting", pid: process.pid };
  const say = (stage: SwapStage, change: Partial<EngineSwap> = {}) => {
    Object.assign(state, { stage }, change);
    if (stage === "waiting") delete state.holding;
    writePrivate(swapFile(RUN), JSON.stringify(engineSwapFormat.write(state)));
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
        if (state.stage === "holding" && engine && !isUpdating()) await releaseWaiting(engine, true).catch(() => 0);
        return "none";
      }
      const busy = await engineBusy(engine);
      if (state.stage === "waiting") {
        quietSince = busy ? null : quietSince ?? now;
        const waited = now - state.since;
        const night = nightOf(new Date(now)) !== null;
        const rested = state.letGo === undefined || now - state.letGo >= SWAP_TIMING.patience;
        // (Let go of once, it holds again only in a night's window, or on your word.)
        const timeCame = state.letGo === undefined ? waited >= SWAP_TIMING.longest || (waited >= SWAP_TIMING.patience && night) : rested && night;
        const nowAsked = fs.existsSync(SWAP_NOW);
        // (What waits on you keeps the time that came from holding: a hold wouldn't outlast it.)
        const blocked = !busy || (rested && nowAsked) ? null : timeCame ? await waitsOnYou(engine) : state.blocked ?? null;
        if (blocked !== (state.blocked ?? null)) say("waiting", blocked ? { blocked } : { blocked: undefined });
        if ((quietSince !== null && now - quietSince >= SWAP_TIMING.quiet) || (rested && nowAsked) || (timeCame && !blocked)) {
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

/** Keeps pending engine messages on the box before either kind of restart. */
export async function keepInboxes(engine: Target): Promise<boolean> {
  try {
    const sessions = new Set<string>();
    for (const archived of [false, true]) {
      const seen = new Set<string>();
      let cursor = "";
      for (;;) {
        const page = await asked(engine, "GET", `api/session?limit=100&order=desc${archived ? "&archived=true" : ""}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`) as {
          data?: unknown; cursor?: { next?: unknown } | null;
        } | null;
        if (!Array.isArray(page?.data) || (page.cursor != null && (typeof page.cursor !== "object" || Array.isArray(page.cursor)))) return false;
        for (const entry of page.data) {
          if (typeof entry?.id !== "string" || !entry.id) return false;
          sessions.add(entry.id);
        }
        const next = page.cursor?.next;
        if (next == null) {
          if (page.data.length >= 100 && !Object.hasOwn(page, "cursor")) return false;
          break;
        }
        if (typeof next !== "string" || !next || !page.data.length || seen.has(next)) return false;
        seen.add(next);
        cursor = next;
      }
    }
    for (const session of sessions) await moveInboxToBox(engine, session);
    return true;
  } catch {
    return false;
  }
}

const restartWorkers = path.join(RUN, ".machine-handover.pid");
const requestEngine = () => writePrivate(path.join(RUN, "engine.now"), "");
const backOffMachine = () => writePrivate(machineRetryFile, String(Date.now() + SWAP_TIMING.patience));

function workerHelper(role: "engine" | "agents", pid: number) {
  const [worker, helper] = (textOf(path.join(RUN, `${role}.launch`)) ?? "").trim().split(/\s+/).map(Number);
  return worker === pid ? helper : null;
}

async function serviceReady(version: string | undefined, helper: number | null, old: { engine?: number; bridge?: number }, clock: () => number, wait: (ms: number) => Promise<unknown>): Promise<EngineRecord | null> {
  const until = clock() + Math.min(SWAP_TIMING.startMost, 120_000);
  do {
    if (stoppedOnPurpose()) return null;
    const engine = runningEngine();
    let bridge;
    try { bridge = readBridgeFile(); } catch { /* still starting */ }
    if (engine?.pid && engine.pid !== old.engine && (!version || engine.version === version) &&
        bridge?.pid && bridge.pid !== old.bridge && processRuns(bridge.pid, "kuvo-agents") &&
        (helper === null || (machineKeeperIdentity()?.helper === helper &&
          workerHelper("engine", engine.pid) === helper && workerHelper("agents", bridge.pid) === helper))) {
      const [engineOK, bridgeOK] = await Promise.all([
        asked(engine, "GET", "api/session?limit=1").then(() => true, () => false),
        asked(bridge, "GET", "api/session/active").then(() => true, () => false),
      ]);
      if (engineOK && bridgeOK) return engine;
    }
    await wait(Math.min(SWAP_TIMING.every, 1000));
  } while (clock() < until);
  return null;
}

function completeMachineJob() {
  writePrivate(machineHandoverDone, "");
  fs.rmSync(machineJobFile, { force: true });
  forgetMachineCron();
}

/** The installer's first start, or an explicit start of a machine already installed. */
export async function startMachineInstall({ clock = Date.now, wait = sleep }: SwapOptions = {}): Promise<boolean> {
  if (!machineHelperAvailable() || fs.existsSync(path.join(RUN, "off"))) {
    console.error("Kuvo Helper for machines is missing, or can’t start on this Mac.");
    return false;
  }
  fs.rmSync(path.join(RUN, "stopped"), { force: true });
  if (machineJob()) await recoverMachineJob({ clock, wait });
  if (machineJob()) return false;
  const adopted = fs.existsSync(machineHandoverDone) && machineHelperInstalled();
  if (!adopted) fs.rmSync(machineHandoverDone, { force: true });
  if (!adopted && previousMachineInstall()) {
    if (!runningEngine()) requestEngine();
    // The existing launch job may be stopped. Its plist still makes this a legacy start.
    return startLegacyMachine() && !!await serviceReady(undefined, null, {}, clock, wait);
  }
  if (!takeUpdateLock()) return false;
  try {
    // Installation verifies the engine and seeds credentials through it. The
    // resident keeper otherwise starts it only when it has work to do.
    if (!runningEngine()) requestEngine();
    const started = machineKeeperRuns() || startMachineHelper();
    const helper = started ? machineKeeperIdentity()?.helper : undefined;
    // Once adopted, detached workers can still serve across a restart of the same signed Helper.
    if (helper && await serviceReady(undefined, adopted ? null : helper, {}, clock, wait)) {
      if (!adopted) completeMachineJob();
      return true;
    }
    await recoverMachineJob({ clock, wait }, true);
    console.error("Kuvo Helper’s engine and coding agents didn’t finish starting. Check that Kuvo is allowed in Login Items.");
    return false;
  } finally {
    fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
}

/** Recovers a replacement interrupted before its workers were verified. The keeper, and a
 * start from cron or from a client, invoke this even when the engine is down. */
export async function recoverMachineJob({ clock = Date.now, wait = sleep }: SwapOptions = {}, owner = false): Promise<EngineRecord | null> {
  const job = machineJob();
  if (!job || stoppedOnPurpose()) return null;
  if (fs.existsSync(machineHandoverDone)) {
    fs.rmSync(machineJobFile, { force: true });
    forgetMachineCron();
    return null;
  }
  if (!owner && (processRuns(job.owner, "kuvo-link") || !takeUpdateLock())) return null;
  try {
    const helper = job.helper ?? machineKeeperIdentity()?.helper;
    if (!owner && !job.legacy && job.phase === "waiting" && helper) {
      saveMachineJob({ ...job, owner: process.pid });
      if (!runningEngine()) requestEngine();
      const target = await serviceReady(undefined, helper, {}, clock, wait);
      if (target) {
        completeMachineJob();
        return target;
      }
    }
    if (!restoreMachineJob(job)) return null;
    if (helper) {
      const engine = runningEngine();
      if (engine?.pid && workerHelper("engine", engine.pid) === helper &&
          (!job.legacy || !await asked(engine, "GET", "api/session?limit=1").then(() => true, () => false))) keeper("engine-halt");
      const pid = Number(textOf(path.join(RUN, "agents.pid")));
      if (processRuns(pid, "kuvo-agents") && workerHelper("agents", pid) === helper) {
        let answering = false;
        try {
          const bridge = readBridgeFile();
          answering = bridge.pid === pid && await asked(bridge, "GET", "api/session/active").then(() => true, () => false);
        } catch { /* the bridge did not finish starting */ }
        if ((!job.legacy || !answering) && processRuns(pid, "kuvo-agents")) process.kill(pid, "SIGTERM");
      }
    }
    if (job.legacy && !runningEngine()) requestEngine();
    fs.rmSync(restartWorkers, { force: true });
    const target = job.legacy ? await serviceReady(job.version, null, {}, clock, wait) : null;
    if (job.legacy && !target) return null;
    if (!job.legacy) fs.rmSync(path.join(RUN, "engine-ran"), { force: true });
    fs.rmSync(machineJobFile, { force: true });
    backOffMachine();
    if (target && !owner) await releaseAfterHold(target);
    return target;
  } finally {
    if (!owner) fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
  }
}

function hasHelperGrant(helper?: number): boolean {
  return performsMutation("macProtectedAction", { kind: "localSettingsAndPermissions" }, () => machineFilesAllowed(helper));
}

/** Keep the old launch job until the signed Helper sees the grant. Switch at a quiet point. */
export async function handOverMachine({ clock = Date.now, wait = sleep }: SwapOptions = {}): Promise<"deferred" | "handled"> {
  if (!machineHelperAvailable() || fs.existsSync(machineHandoverDone)) return "deferred";
  if (swapUnderWay(RUN)) return "handled";
  if (machineJob()) {
    await recoverMachineJob({ clock, wait });
    return machineJob() ? "handled" : "deferred";
  }
  if (Number(textOf(machineRetryFile)) > clock()) return "deferred";
  if (machineKeeperRuns() || !watchMachinePermissions() || !hasHelperGrant()) return "deferred";
  const engine = runningEngine();
  if (!engine?.version || !engine.pid) return "deferred";
  const blocked = () => stoppedOnPurpose() || isUpdating() || updateOnTrial() || fs.existsSync(path.join(RUN, ".rolling-back"));
  const busy = async () => await engineBusy(engine) || await agentsWorking() !== false;
  const valid = () => !blocked() && runningEngine()?.pid === engine.pid && !machineKeeperRuns() && hasHelperGrant();
  if (!valid()) return "deferred";

  const state: EngineSwap = { from: engine.version, to: engine.version, pid: process.pid, since: clock(), stage: "waiting", machineHelper: true };
  const say = () => writePrivate(swapFile(RUN), JSON.stringify(engineSwapFormat.write(state)));
  let locked = false;
  let held = false;
  let ready: EngineRecord | null = null;
  say();
  try {
    let quietSince: number | undefined;
    while (valid()) {
      quietSince = await busy() ? undefined : quietSince ?? clock();
      if (quietSince !== undefined && clock() - quietSince >= SWAP_TIMING.quiet) break;
      await wait(SWAP_TIMING.every);
    }
    if (!valid()) return "deferred";
    state.stage = "holding";
    state.holding = clock();
    say();
    held = true;
    await wait(Math.min(1000, SWAP_TIMING.every));
    if (await busy() || !valid()) return "deferred";

    const bridge = Number(textOf(path.join(RUN, "agents.pid")));
    if (processRuns(bridge, "kuvo-agents")) {
      try {
        const noted = readBridgeFile();
        if (noted.pid !== bridge || noted.machineHandover !== true) return "deferred";
      } catch { return "deferred"; }
    }
    if (!(await keepInboxes(engine)) || await busy() || !valid()) return "deferred";
    state.stage = "swapping";
    say();
    if (!takeUpdateLock()) return "handled";
    locked = true;
    if (stoppedOnPurpose() || updateOnTrial() || !hasHelperGrant()) return "deferred";
    if (!startMachineHelper(engine.version)) {
      console.error("Kuvo Helper’s machine launch agent couldn’t start.");
      backOffMachine();
      return "handled";
    }
    const job = machineJob();
    const helper = machineKeeperIdentity()?.helper;
    if (!job || !helper) return "handled";
    const until = clock() + Math.min(12_000, SWAP_TIMING.startMost);
    while (machineKeeperIdentity()?.helper === helper && !hasHelperGrant(helper) && !stoppedOnPurpose() && clock() < until) {
      await wait(Math.min(SWAP_TIMING.every, 500));
    }
    if (machineKeeperIdentity()?.helper !== helper || !hasHelperGrant(helper) || await busy()) return "handled";

    // Keep the Helper keeper in standby until the old bridge and engine have been retired.
    writePrivate(restartWorkers, String(process.pid));
    saveMachineJob({ ...job, phase: "workers", owner: process.pid, helper });
    for (let i = 0; i < 45 && processRuns(bridge, "kuvo-agents"); i++) await wait(1000);
    if (!processRuns(bridge, "kuvo-agents") && !stoppedOnPurpose() && machineKeeperIdentity()?.helper === helper &&
        !await engineBusy(engine) && machineKeeperIdentity()?.helper === helper && hasHelperGrant(helper) && keeper("engine-halt")) {
      requestEngine();
      fs.rmSync(restartWorkers, { force: true });
      ready = await serviceReady(engine.version, helper, { engine: engine.pid, bridge }, clock, wait);
      if (ready) completeMachineJob();
    }
    return "handled";
  } finally {
    try {
      if (locked && machineJob() && !ready) {
        state.stage = "back";
        say();
        ready = await recoverMachineJob({ clock, wait }, true);
      }
    } finally {
      if (locked) fs.rmSync(UPDATE_LOCK, { recursive: true, force: true });
      fs.rmSync(restartWorkers, { force: true });
      if (ready || !machineJob()) await releaseAfterHold(ready ?? (held ? runningEngine() : null));
    }
  }
}

// The swap itself, nothing at work: what the engine's inboxes hold onto the box, the old engine
// stopped, its data kept, the new one started and waited for; what was held to it once it answers,
// else back to the one before where it can be. Null: it didn't go (something came up), and it goes
// on holding.
async function swapNow(engine: EngineRecord, state: EngineSwap, say: (stage: SwapStage, change?: Partial<EngineSwap>) => void, clock: () => number, wait: (ms: number) => Promise<unknown>): Promise<Swapped | null> {
  say("swapping");
  if (!(await keepInboxes(engine))) {
    say("holding");
    return null;
  }
  // (Nor while an update began meanwhile: it goes first, and this looks again after.)
  if (isUpdating() || await engineBusy(engine)) {
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
  // Kuvo stopped meanwhile (paused, off, stopped): not the new engine's failing; tried again later.
  if (stoppedOnPurpose()) return ended(state, "failed", "Kuvo was stopped while its engine updated. It tries again once it starts.", null, clock);
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
  if (engine) await releaseAfterHold(engine);
  return result;
}

async function releaseAfterHold(engine: Target | null): Promise<void> {
  if (engine) await releaseWaiting(engine, true).catch(() => 0);
  // Send anything that arrived during the first drain once the hold is gone.
  const record = readSwap(RUN);
  if (record && (record.pid === process.pid || (record.machineHelper && !processRuns(record.pid, "kuvo-link")))) {
    fs.rmSync(swapFile(RUN), { force: true });
  }
  if (engine) await releaseWaiting(engine).catch(() => 0);
}
