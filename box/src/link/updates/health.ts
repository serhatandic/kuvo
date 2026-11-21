import { BOX_SERVICES_ROLE, readServices } from "../../shared/box-services";
// One health rule for every box (PROTOCOL.md, "Updates"): whether an update that put its files in
// place is well, judged the same way on a Mac and on Linux, from what the pieces that took the new
// code do, never from a timer that runs out when nothing said so. The judge is a process of its own
// (`kuvo-link.js update …`, detached: the updater itself once it swapped the files, or `update
// --settle`, which the keeper starts when an update on trial has no judge, so a judge cut off
// midway is taken up again where it was). It looks every ten seconds:
//   - the box answers `kuvo/info` from its new files, with every feature it had before;
//   - each client-facing process started since the swap (`serve`, `serve --kept`, the link `run`)
//     runs, and a server answers (one still on the code from before is waiting, not unwell);
//   - the engine answers (or is starting: its log grows), or is stopped on purpose;
//   - the coding agents' bridge answers (on its new build, or on the old one while it makes way);
//   - none of them starts again and again (more than three times while it's judged).
// Well, once all of them hold through five minutes of the judge's own running time (a Mac asleep
// doesn't count it down): what was kept to go back to goes. Unwell, once a piece fails three looks
// in a row over a minute at least, or starts again and again: its code is put back (never its
// data), each piece taking it as it takes any new code.

import { setupLayoutDeferred } from "../../shared/folder-version";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { keeperEngine } from "../../shared/engines";
import { FEATURES } from "../../shared/features";
import { engineRuns, processRuns as runs, runsAs } from "../../shared/platform";
import { readBridgeFile } from "../engine/client";
import type { EngineRecord, Target } from "../../shared/records";
import { macFeatures } from "../../shared/routes/mac";
import { sleep } from "../../shared/wait";
import { asked } from "../engine/client";
import { writePrivate } from "../files";
import { ENGINE_FILE, IS_MAC, K, NEEDED_FILE, OWN_HOME_MAC, PID_FILE, REAL_BOX, RUN } from "../home";
import { boxVersion, checkRollbackRecords, confirmUpdate, isUpdating, newEngineUnused, noteTrialEnded, noteUpdate, updateOnTrial, updateState, type UpdateTrial } from "./update";

/** The pieces judged. */
export type Piece = "info" | "serve" | "run" | "engine" | "bridge";
export const PIECES: Piece[] = ["info", "serve", "run", "engine", "bridge"];

/** One look at a piece: whether it holds now (`ok`), and the process it runs as (to tell a piece
 * that starts again and again). */
export interface PieceLook {
  ok: boolean;
  pid?: number | null;
}

/** How the judging of the update on trial stands (`trial.judged` in `run/update.json`), so a judge
 * started again goes on from it: how long all held (ms of a judge's running time), each piece's
 * failed looks in a row and since when (that judge's time), and the processes seen for each. */
export interface Judged {
  well: number;
  /** Whether the engine ever answered while it was judged. */
  engineAnswered?: boolean;
  fails: Partial<Record<Piece, number>>;
  failing: Partial<Record<Piece, number>>;
  pids: Partial<Record<Piece, number[]>>;
}

/** How long all must hold, how often it looks, how many failed looks in a row (over how long at
 * least) make a piece unwell, and how many times one may start again while it's judged. */
export const HEALTH = { window: 5 * 60_000, every: 10_000, fails: 3, failingFor: 60_000, restarts: 3 };
// (A test's box judges as quickly as it says.)
if (!REAL_BOX) {
  if (Number(process.env.KUVO_TEST_HEALTH_WINDOW) > 0) HEALTH.window = Number(process.env.KUVO_TEST_HEALTH_WINDOW);
  if (Number(process.env.KUVO_TEST_HEALTH_EVERY) > 0) HEALTH.every = Number(process.env.KUVO_TEST_HEALTH_EVERY);
}

/** The judge's mark (`run/update-judge.pid`): the judging process, while it judges. */
export const JUDGE_FILE = path.join(RUN, "update-judge.pid");
/** How many judges the keeper started that never looked once (`run/update-settle.tries`). */
export const SETTLE_TRIES = path.join(RUN, "update-settle.tries");

/** Whether a judge other than this process judges the update on trial now. */
export function judging(): boolean {
  let pid = 0;
  try { pid = Number(fs.readFileSync(JUDGE_FILE, "utf8")); } catch {}
  return pid > 0 && pid !== process.pid && runs(pid, "kuvo-link");
}

/** The features the box says it has (`kuvo/info`), as this code says them: what an update's new
 * files must still say. */
export const featuresNow = (): string[] => (IS_MAC ? macFeatures() : [...FEATURES]);

const readJSON = <T>(file: string): T | null => { try { return JSON.parse(fs.readFileSync(file, "utf8")) as T; } catch { return null; } };
const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };
const size = (file: string): number => { try { return fs.statSync(file).size; } catch { return -1; } };

// `kuvo/info` asked of the box's files in place, in a process of their own, as a client asks it
// (`box route`): its status and features, or null where it didn't answer.
function infoFromFiles(): Promise<{ status: number; features: string[] } | null> {
  const version = keeperEngine(K);
  const runtime = version ? path.join(K, "engine", version, "opencode") : null;
  const script = path.join(K, "bin", "kuvo-link.js");
  if (!runtime || !fs.existsSync(runtime) || !fs.existsSync(script)) return Promise.resolve(null);
  return new Promise(resolve => {
    let out = "", done = false;
    const finish = (value: { status: number; features: string[] } | null) => { if (!done) { done = true; resolve(value); } };
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(runtime, ["--config=/dev/null", "--no-env-file", script, "box", "route", "--internal"], { cwd: K, env: { ...process.env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K }, stdio: ["pipe", "pipe", "ignore"] });
    } catch { return finish(null); }
    const timer = setTimeout(() => { try { child.kill("SIGKILL"); } catch {} finish(null); }, 60_000);
    child.stdout!.on("data", chunk => { out += chunk; });
    child.on("error", () => { clearTimeout(timer); finish(null); });
    child.on("close", () => {
      clearTimeout(timer);
      try {
        const said = JSON.parse(out.trim().split("\n").pop()!);
        const body = typeof said.body === "string" ? JSON.parse(said.body) : said.body;
        finish({ status: Number(said.status), features: Array.isArray(body?.features) ? body.features : [] });
      } catch { finish(null); }
    });
    child.stdin!.end(JSON.stringify({ method: "GET", path: "kuvo/info" }));
  });
}

const answers = (target: Target, route: string): Promise<boolean> => asked(target, "GET", route).then(() => true, () => false);

/** One look at each piece, for the update on trial (`trial`, swapped at `since`, wall ms). `last`:
 * the engine's log's size at the look before, to tell an engine that starts from one that hangs. */
export async function lookAtPieces(trial: UpdateTrial, since: number, last: { engineLog?: number } = {}): Promise<Record<Piece, PieceLook>> {
  // The box from its new files, with every feature it had.
  const info = await infoFromFiles();
  const had = trial.features ?? [];
  const infoOK = info !== null && info.status === 200 && had.every(feature => info.features.includes(feature));

  // Its server for clients: one started since the swap answers; one from before is waiting.
  const servedRecord = readJSON<{ pid?: number; port?: number; password?: string; kept?: boolean }>(path.join(RUN, "serve.json"));
  const servePid = servedRecord?.pid && runsAs(servedRecord.pid, servedRecord.kept ? /kuvo-link\.js serve --kept(\s|$)/ : /kuvo-link\.js serve(\s|$)/) ? servedRecord.pid : null;
  const serveWanted = OWN_HOME_MAC || fs.existsSync(path.join(RUN, "serve.needed"));
  let serveOK = !serveWanted;
  if (servePid) {
    const fresh = mtime(path.join(RUN, "serve.json")) >= since;
    serveOK = !fresh || (!!servedRecord!.port && !!servedRecord!.password && (await answers({ port: servedRecord!.port!, password: servedRecord!.password! }, "kuvo/info")));
  }

  // Its link (`run`): running while it's wanted.
  let linkPid: number | null = null;
  try { linkPid = Number(fs.readFileSync(PID_FILE, "utf8")) || null; } catch {}
  if (linkPid && !runs(linkPid, "kuvo-link")) linkPid = null;
  const runOK = linkPid !== null || !fs.existsSync(NEEDED_FILE);

  // The engine: answers, or starts (its log grows), or is stopped on purpose.
  const engine = readJSON<EngineRecord>(ENGINE_FILE);
  const enginePid = engine?.pid && engineRuns(engine.pid, K) ? engine.pid : null;
  const log = size(path.join(RUN, "engine.log"));
  let engineOK: boolean;
  if (enginePid) engineOK = (await answers(engine!, "api/session?limit=1")) || (last.engineLog !== undefined && log > last.engineLog);
  else engineOK = ["stopped", "paused", "off"].some(mark => fs.existsSync(path.join(RUN, mark)));
  last.engineLog = log;

  // The coding agents' bridge: answers while the engine runs, where its keeper keeps it (or its
  // engine's launch started it beside it, before the keeper did): whichever build, one making way
  // for the new is waiting.
  let bridge: ReturnType<typeof readBridgeFile> | null = null;
  try { bridge = readBridgeFile(); } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const bridgePid = bridge?.pid && runs(bridge.pid, "kuvo-agents") ? bridge.pid : null;
  let keeps = false;
  try { keeps = /^bridge\)/m.test(fs.readFileSync(path.join(K, "bin", "kuvo-start"), "utf8")); } catch {}
  const bridgeWanted = fs.existsSync(path.join(K, "bin", "kuvo-agents.js")) && enginePid !== null && (keeps || !!engine?.agentsPort);
  let bridgeOK = !bridgeWanted;
  // (One whose new build doesn't run here, or that nothing would start again, isn't taking it.)
  if (bridgePid && bridge!.renewal?.waits === "restart") bridgeOK = false;
  else if (bridgePid) bridgeOK = !!bridge!.port && !!bridge!.password && (await answers(bridge!, "api/session/active"));

  // Only after the new bridge has taken ownership. A live pre-split bridge still owns these routes.
  const services = bridgePid && bridge?.boxServices ? readServices(RUN) : undefined;
  if (bridgePid && bridge?.boxServices && !["off", "paused", "stopped"].some(mark => fs.existsSync(path.join(RUN, mark)))) {
    serveOK &&= !!services && runsAs(services.pid, BOX_SERVICES_ROLE) && await answers(services, "api/kuvo/limits");
  }

  return {
    info: { ok: infoOK },
    serve: { ok: serveOK, pid: servePid },
    run: { ok: runOK, pid: linkPid },
    engine: { ok: engineOK, pid: enginePid },
    bridge: { ok: bridgeOK, pid: bridgePid },
  };
}

/** How a judge goes (a test's own): its running-time clock (ms; the default stops while the
 * computer sleeps), how often it looks and how long all must hold, how it waits, looks, puts the
 * code back, and what's done once the update is well. */
export interface SettleOptions {
  clock?: () => number;
  every?: number;
  window?: number;
  /** How long a piece must have failed (with three looks in a row) to be unwell. */
  failingFor?: number;
  wait?: (ms: number) => Promise<unknown>;
  look?: (trial: UpdateTrial, since: number) => Promise<Record<Piece, PieceLook>>;
  rollBack?: () => boolean;
  well?: () => Promise<void> | void;
}

/** What judging came to: well (kept), unwell (its code put back), or nothing to judge (none on
 * trial, another judge at it, an update under way, or the trial ended by something else). */
export type Settled = "well" | "unwell" | "none";

/** The code from before put back (`kuvo-start rollback`): its data never. True once done. */
export function rollBackCode(): boolean {
  checkRollbackRecords();
  const start = path.join(K, "bin", "kuvo-start");
  if (!fs.existsSync(start)) return false;
  const ran = spawnSync("sh", [start, "rollback"], { cwd: K, env: { ...process.env, KUVO_UPDATER: String(process.pid) }, stdio: "ignore", timeout: 600_000 });
  return ran.status === 0 && !updateOnTrial();
}

/** Judges the update on trial (one judge at a time) until it's well or unwell, by the one rule. */
export async function settleUpdate({ clock = () => performance.now(), every = HEALTH.every, window = HEALTH.window, failingFor = HEALTH.failingFor, wait = sleep, look, rollBack = rollBackCode, well = () => {} }: SettleOptions = {}): Promise<Settled> {
  // (A trial ended meanwhile by something else, a keeper giving up say: how it went noted first.)
  const version = checkRollbackRecords();
  noteTrialEnded();
  if (!updateOnTrial() || isUpdating() || judging() || stoppedOnPurpose()) return "none";
  // The trial, as the update that opened it noted it (one from before this rule notes no features,
  // nor when it swapped: its start then).
  const opened = updateState().trial;
  const to = opened?.to ?? version;
  if (!to) return "none";
  const trial: UpdateTrial = opened ?? { from: "", to, at: mtime(path.join(RUN, "update-trial")) };
  const since = trial.swapped ?? trial.at;
  // The judge's mark, taken whole or not at all: never two judges (nor two rollbacks) at once.
  if (!takeJudgeMark()) return "none";
  // A rollback begun that didn't get it all back: carried on, not judged again (`kuvo-start
  // rollback` gives up after three tries, ending the trial and starting the box as it is).
  if (fs.existsSync(path.join(RUN, "update-restore"))) {
    try { return putBack(trial, to, null, rollBack); } finally { dropJudgeMark(); }
  }
  // A cut during installation leaves a mix of releases. Restore the old code before starting
  // any more of the update, including an engine that was stopped for the swap.
  if (trial.installing || trial.engineStopped || (opened && setupLayoutDeferred(K) && !trial.swapped)) {
    try {
      return putBack(trial, to, null, rollBack);
    } finally {
      // (Only once it's all put back: a new engine never starts on files half restored.)
      if (trial.engineStopped && !updateOnTrial()) spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "start"], { cwd: K, env: process.env, stdio: "ignore", timeout: 600_000 });
      dropJudgeMark();
    }
  }
  const lastLook: { engineLog?: number } = {};
  const looking = look ?? ((open: UpdateTrial, swapped: number) => lookAtPieces(open, swapped, lastLook));
  const judged: Judged = { well: 0, fails: {}, failing: {}, pids: {}, ...(trial.judged ?? {}) };
  // (A judge taken up again: what failed counts from its own time again.)
  judged.failing = {};
  const stillOpen = () => updateOnTrial() && boxVersion() === to && !isUpdating() && !stoppedOnPurpose();
  try {
    let before = clock();
    for (;;) {
      if (!stillOpen()) return "none";
      const looked = await looking(trial, since);
      // (A judge that looked once is one that runs: the keeper's count of those that didn't goes.)
      fs.rmSync(SETTLE_TRIES, { force: true });
      if (!stillOpen()) return "none";
      const now = clock();
      // (What passed since the look before, never more than two looks' worth: a computer that slept
      // or a process held up doesn't count as time all held.)
      const passed = Math.max(0, Math.min(now - before, 2 * every));
      before = now;
      let unwell: Piece | null = null;
      for (const piece of PIECES) {
        const { ok, pid } = looked[piece];
        if (ok) {
          judged.fails[piece] = 0;
          delete judged.failing[piece];
        } else {
          judged.fails[piece] = (judged.fails[piece] ?? 0) + 1;
          judged.failing[piece] ??= now;
          if (judged.fails[piece]! >= HEALTH.fails && now - judged.failing[piece]! >= failingFor) unwell = piece;
        }
        if (pid) {
          const seen = judged.pids[piece] ?? [];
          if (!seen.includes(pid)) seen.push(pid);
          judged.pids[piece] = seen.slice(-(HEALTH.restarts + 2));
          if (seen.length > HEALTH.restarts + 1) unwell = piece;
        }
      }
      if (looked.engine.ok && looked.engine.pid) judged.engineAnswered = true;
      if (unwell) {
        noteUpdate({ trial: { ...trial, judged } });
        if (!judged.engineAnswered) newEngineUnused(trial);
        return putBack({ ...trial, judged }, to, unwell, rollBack);
      }
      judged.well = PIECES.every(piece => looked[piece].ok) ? judged.well + passed : 0;
      noteUpdate({ trial: { ...trial, judged } });
      if (judged.well >= window) {
        noteUpdate({ trial: { ...trial, judged, healthy: true } });
        if (!confirmUpdate()) return "none";
        await Promise.resolve(well()).catch(() => {});
        return "well";
      }
      await wait(every);
    }
  } finally {
    dropJudgeMark();
  }
}

/** What each piece is, in words. */
const PIECE_WORDS: Record<Piece, string> = { info: "Kuvo didn't answer", serve: "its server for your devices didn't run", run: "its link to your phone and computers didn't run", engine: "its engine didn't run", bridge: "its coding agents didn't run" };

// The code from before put back (`rollBack`), and how that went noted: unwell for `why` (none: a
// rollback begun before, carried on, or an update cut off).
function putBack(trial: UpdateTrial, to: string, why: Piece | null, rollBack: () => boolean): Settled {
  rollBack();
  const reason = why ? ` (${PIECE_WORDS[why]})` : "";
  if (updateOnTrial()) {
    const message = `${to} didn't run well here${reason}, and going back to ${trial.from || "what it had"} didn't finish: it tries again in a moment.`;
    noteUpdate({ trial, last: { at: Date.now(), result: "failed", message, from: trial.from, to } });
  } else noteTrialEnded(reason);
  return "unwell";
}

/** Whether Kuvo here is stopped until it starts again, or off: nothing to judge meanwhile (the
 * next start's keeper starts a judge again). */
const stoppedOnPurpose = (): boolean => fs.existsSync(path.join(RUN, "paused")) || fs.existsSync(path.join(RUN, "off"));

// The judge's mark, made only where there's none (one whose judge went is taken over).
function takeJudgeMark(): boolean {
  for (let tries = 0; tries < 2; tries++) {
    try {
      fs.writeFileSync(JUDGE_FILE, String(process.pid), { flag: "wx", mode: 0o600 });
      return true;
    } catch {
      if (judging()) return false;
      try { fs.rmSync(JUDGE_FILE, { force: true }); } catch {}
    }
  }
  return false;
}

function dropJudgeMark(): void {
  try { if (Number(fs.readFileSync(JUDGE_FILE, "utf8")) === process.pid) fs.rmSync(JUDGE_FILE, { force: true }); } catch {}
}

/** A judge in a process of its own (`kuvo-link.js update --settle`), outliving the one that starts
 * it: for an update whose own process can't stay to judge (the installer's, which an app waits on). */
export function startSettling(): void {
  // (From the box's own files, on the engine its keeper starts, as the keeper starts one: never
  // the installer's, whose staged files are gone once the update ends.)
  const version = keeperEngine(K);
  const runtime = version ? path.join(K, "engine", version, "opencode") : null;
  if (!runtime || !fs.existsSync(runtime)) return;
  try {
    const child = spawn(runtime, ["--config=/dev/null", "--no-env-file", path.join(K, "bin", "kuvo-link.js"), "update", "--settle"], { detached: true, stdio: "ignore", cwd: K, env: { ...process.env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: K } });
    child.on("error", () => {});
    child.unref();
  } catch {}
}
