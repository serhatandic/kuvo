// How the coding agents stand on this box when their bridge can't be asked: coming back by itself
// (`restarting`: a bridge making way for a newer one, or for a new engine; started again after a
// crash; starting), or down, with why in one plain sentence and, where it helps, Try Again
// (`kuvo/agents/start`). Every route that needs the bridge answers with this one state, and a
// request that comes while the bridge restarts waits for it (`bridgeBack`), so a planned hand-over
// never shows as an error (PROTOCOL.md, "The coding agents' state").
//
// What it reads, all in `run/`: `bridge.json` (the bridge, while it runs), `agents-renewing` (a
// bridge that made way on purpose, as it went), `agents.pid` (the bridge its keeper started),
// `box.pid` (the keeper), `agents.retry` (when the keeper starts it next, after it kept stopping),
// and `engine.json`. A retired Mac app's own box keeps its `agents-stopped`.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { readRepairRecord } from "../updates/repair-record";
import { observeProcess, processRuns } from "../../shared/platform";
import { keepsList } from "../lists/sessions";
import { pause } from "../../shared/wait";
import type { Target } from "../../shared/records";
import { IS_MAC, K, OWN_HOME_MAC, RUN } from "../home";
import type { AnswerPiece, RouteContext } from "../server/context";

/** Why the coding agents are down, every reason a box says (for logs, peers and tests; people see
 * `line`). */
export const AGENTS_DOWN_WHY = ["engine", "notInstalled", "outdated", "keeper", "crashing", "stuck", "notStarting", "app"] as const;
export type AgentsDownWhy = (typeof AGENTS_DOWN_WHY)[number];

export type AgentsState =
  | { state: "running" }
  | { state: "restarting"; line: string }
  | { state: "down"; why: AgentsDownWhy; line: string; tryAgain: boolean };

/** What a route answers while the bridge can't be asked (503): the one sentence, the word clients
 * map (`agentsRestarting`, `agentsDown`), and Try Again where it helps. */
export interface AgentsAway {
  message: string;
  reason: "agentsRestarting" | "agentsDown";
  action?: { label: string; route: string };
}

export const AGENTS_START = "kuvo/agents/start";

/** A bridge that made way on purpose is back within this; a bridge noted as started has this long
 * to say where it listens; and with nothing to explain it, the keeper starts one within this. */
export const agentsTiming = { renewing: 90_000, starting: 60_000, keeper: 120_000 };

/** The files it reads, and the moment (a test's own). */
export interface AgentsPlace {
  run: string;
  bin: string;
  now: number;
  /** The bridge here, while it runs (as `bridgeTarget` reads it). */
  bridge: () => Target | null;
  /** Whether a pid runs a program (by a word of its command line). */
  runs: (pid: unknown, name: string) => boolean;
}

const readNumber = (file: string): number => {
  try {
    return Number(fs.readFileSync(file, "utf8").trim());
  } catch {
    return NaN;
  }
};

const readJSON = (file: string): Record<string, unknown> | null => {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return null;
  }
};

const mtime = (file: string): number | null => {
  try {
    return fs.statSync(file).mtimeMs;
  } catch {
    return null;
  }
};

/** Since when (ms) this process has found the bridge away with nothing that explains it: past
 * `agentsTiming.keeper`, the keeper isn't starting it after all. Forgotten whenever a bridge is
 * found (here, or by a request that reached one, `bridgeBack`), so each outage counts afresh. */
let unexplainedSince: number | null = null;
function seenRunning() {
  unexplainedSince = null;
  lastNoted = undefined;
}

const ENGINE_FIRST = "The coding agents start once the engine is running.";
export const AGENTS_RESTARTING = "The coding agents are restarting. This takes a few seconds.";

/** How the coding agents stand here now. */
export function agentsState(place: Partial<AgentsPlace> = {}): AgentsState {
  const run = place.run ?? RUN;
  const bin = place.bin ?? path.join(K, "bin");
  const now = place.now ?? Date.now();
  const runs = place.runs ?? (keepsList() ? (pid, name) => observeProcess(pid, name, true) : processRuns);
  const bridge = place.bridge ?? (() => bridgeAt(run, (pid, name) => runs(pid, name) === true));
  if (bridge()) {
    seenRunning();
    return { state: "running" };
  }
  // COMPAT(0.7 app-agents-stopped): a box in the retired Mac app's folder, whose app wrote why it gave up. Goes on the user's word, once no Mac from before 1.0 connects.
  if (IS_MAC && !OWN_HOME_MAC && place.run === undefined) {
    try {
      const { why, retrying } = JSON.parse(fs.readFileSync(path.join(run, "agents-stopped"), "utf8"));
      if (typeof why === "string" && why) return { state: "down", why: "app", line: `This Mac's coding agents aren't running. ${why}${retrying === true ? "" : " Restart them in Kuvo Code on the Mac."}`, tryAgain: false };
    } catch {}
    return { state: "restarting", line: AGENTS_RESTARTING };
  }
  const restarting = { state: "restarting", line: AGENTS_RESTARTING } as const;
  // The resident asks macOS for the process name asynchronously. Until that
  // observation comes back, a live PID is neither confirmed nor down.
  if (runs(readJSON(path.join(run, "bridge.json"))?.pid, "kuvo-agents") === undefined) return restarting;
  if (!fs.existsSync(path.join(bin, "kuvo-agents.js"))) return { state: "down", why: "notInstalled", line: "The coding agents aren't installed here yet. Kuvo's next update brings them.", tryAgain: false };
  // The engine first: the keeper starts the bridge only while it runs.
  const engine = readJSON(path.join(run, "engine.json"));
  const engineRunning = engine?.pid === undefined ? !!engine : runs(engine.pid, "opencode");
  if (engineRunning === undefined) return restarting;
  if (!engineRunning) {
    unexplainedSince = null;
    return { state: "down", why: "engine", line: ENGINE_FIRST, tryAgain: false };
  }
  // One making way on purpose (a newer build, a new engine): it started the next as it went.
  const renewing = readJSON(path.join(run, "agents-renewing"));
  if (typeof renewing?.at === "number" && now - renewing.at >= 0 && now - renewing.at < agentsTiming.renewing) return { state: "restarting", line: AGENTS_RESTARTING };
  // One its keeper started that hasn't said where it listens yet.
  const started = readNumber(path.join(run, "agents.pid"));
  if (runs(started, "kuvo-agents")) {
    const since = mtime(path.join(run, "agents.pid"));
    if (since !== null && now - since < agentsTiming.starting) return { state: "restarting", line: AGENTS_RESTARTING };
    return { state: "down", why: "stuck", line: "The coding agents didn't finish starting.", tryAgain: true };
  }
  // A bridge's program from before its keeper kept it apart from the engine (it asks for the engine's
  // port and pid as it starts, and ends at once): from an older Kuvo, put back over the release's.
  if (fs.existsSync(path.join(run, "agents.retry")) || unexplainedSince !== null) {
    let said = "";
    try {
      const fd = fs.openSync(path.join(run, "agents.log"), "r");
      try {
        const tail = Buffer.alloc(8000);
        const size = fs.fstatSync(fd).size;
        said = tail.subarray(0, fs.readSync(fd, tail, 0, tail.length, Math.max(0, size - tail.length))).toString("utf8").slice(-2000);
      } finally { fs.closeSync(fd); }
    } catch {}
    if (said.includes("KUVO_ENGINE_PORT and KUVO_ENGINE_PID are needed")) {
      // Repaired from the release this box runs, unless it has no sound one (updates/repair.ts).
      let repair: ReturnType<typeof readRepairRecord>;
      try { repair = readRepairRecord(run); }
      catch (error) {
        if (!(error instanceof Error)) throw error;
        return { state: "down", why: "outdated", line: error.message, tryAgain: false };
      }
      const cant = repair?.result === "damaged" || (repair?.result === "none" && (repair.why === "noRelease" || repair.why === "otherVersion"));
      return cant
        ? { state: "down", why: "outdated", line: "The coding agents here are from an older Kuvo and can't start, and there's no sound copy here to repair them from. Kuvo's next update brings them.", tryAgain: false }
        : { state: "down", why: "outdated", line: "The coding agents here are from an older Kuvo and can't start. Kuvo puts back the right ones by itself.", tryAgain: true };
    }
  }
  const keeping = runs(readNumber(path.join(run, "box.pid")), "kuvo-start");
  if (keeping === undefined) return restarting;
  if (!keeping) {
    return { state: "down", why: "keeper", line: "The coding agents stopped, and nothing is starting them again.", tryAgain: true };
  }
  // Stopping again and again: its keeper waits longer before each start (`run/agents.retry`, when, in
  // seconds since 1970). The keeper looks every fifteen seconds, and a start takes a few more.
  const retry = readNumber(path.join(run, "agents.retry"));
  if (Number.isFinite(retry) && retry * 1000 - now > 30_000) {
    const minutes = Math.ceil((retry * 1000 - now) / 60_000);
    return { state: "down", why: "crashing", line: `The coding agents keep stopping. Kuvo tries again in ${minutes === 1 ? "about a minute" : `about ${minutes} minutes`}.`, tryAgain: true };
  }
  // Ended a moment ago (a crash): the keeper starts it again at its next round, unless that never
  // comes about.
  unexplainedSince ??= now;
  if (now - unexplainedSince < agentsTiming.keeper) return { state: "restarting", line: AGENTS_RESTARTING };
  return { state: "down", why: "notStarting", line: "The coding agents haven't started.", tryAgain: true };
}

/** The bridge in `run`, while its process runs. */
function bridgeAt(run: string, runs: AgentsPlace["runs"]): Target | null {
  const bridge = readJSON(path.join(run, "bridge.json"));
  return bridge?.port && bridge?.password && runs(bridge.pid, "kuvo-agents") ? (bridge as unknown as Target) : null;
}

/** The state as a route answers it (503), or null while the bridge runs. */
export function agentsAway(place: Partial<AgentsPlace> = {}): AgentsAway | null {
  const state = agentsState(place);
  if (state.state === "running") return null;
  if (state.state === "restarting") return { message: state.line, reason: "agentsRestarting" };
  noteDown(state);
  return { message: state.line, reason: "agentsDown", ...(state.tryAgain ? { action: { label: "Try Again", route: AGENTS_START } } : {}) };
}

/** Why, to say when the bridge isn't running (the old `agentsDown`): the one sentence. */
export function agentsDown(place: Partial<AgentsPlace> = {}): string | null {
  return agentsAway(place)?.message ?? null;
}

/** The sentence, whatever the state (one that runs a moment later says it's restarting). */
export const agentsLine = (place: Partial<AgentsPlace> = {}): string => agentsAway(place)?.message ?? AGENTS_RESTARTING;

// Each reason, with the bridge's last words, once to the log as it comes about: codes and paths go
// there, never to people.
let lastNoted: string | undefined = undefined;
function noteDown(state: Extract<AgentsState, { state: "down" }>) {
  if (lastNoted === state.why) return;
  lastNoted = state.why;
  let tail = "";
  try {
    tail = fs.readFileSync(path.join(RUN, "agents.log"), "utf8").trim().split("\n").slice(-5).join(" | ").slice(0, 600);
  } catch {}
  console.error(`coding agents down (${state.why})${tail ? `; their log ends: ${tail}` : ""}`);
}

/** The bridge, once it's back, while it's restarting: up to `within` ms (30 s), looked for every
 * 200 ms. An explicit handover refusal proves it is transferring before any keeper mark changes. */
export async function bridgeBack(bridge: () => Target | null, signal?: AbortSignal, within = 30_000, place: Partial<AgentsPlace> & { transferring?: boolean } = {}): Promise<Target | null> {
  const from = Date.now();
  while (true) {
    const found = bridge();
    if (found) {
      seenRunning();
      return found;
    }
    // (A retired Mac app's own box: its app starts the bridge, whenever that is.)
    if (signal?.aborted || Date.now() - from >= within) return null;
    // (One that came up between the two looks: looked for again.)
    if (!place.transferring && ((IS_MAC && !OWN_HOME_MAC && place.run === undefined) || agentsState({ ...place, bridge }).state === "down")) return null;
    await pause(200, signal);
  }
}

/** For a test: what this process found unexplained, forgotten. */
export function forgetAgentsState() {
  unexplainedSince = null;
  lastNoted = undefined;
}

/** `POST kuvo/agents/start`, Try Again: the keeper's own command (`kuvo-start bridge-again`) starts
 * the bridge now (one that never finished starting ended first, the keeper started if it isn't
 * running, its wait after failures forgotten). Answered once that's done, 45 s at most: 200 once
 * it runs, else 503 with how they stand now. */
let starting: Promise<void> | null = null;
export async function* agentsStartRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  // (A retired Mac app's own box: its app starts them.)
  if (IS_MAC && !OWN_HOME_MAC) return yield* json(409, { message: "Restart them in Kuvo Code on the Mac." });
  const start = path.join(K, "bin", "kuvo-start");
  if (!fs.existsSync(start)) return yield* json(409, { message: "Kuvo isn't set up to start them here." });
  // (With what the keeper starts it with, HOME and PATH alone: never this process's own.)
  const environment = { HOME: path.basename(K) === ".kuvo-code" ? path.dirname(K) : process.env.HOME ?? "", PATH: process.env.PATH ?? "/usr/bin:/bin" };
  const workspace = path.join(K, "workspace");
  // (One at a time: a second Try Again, from a phone say, waits for the first.)
  starting ??= new Promise<void>(resolve => {
    try {
      const child = spawn("sh", [start, "bridge-again"], { cwd: fs.existsSync(workspace) ? workspace : K, env: environment, detached: true, stdio: "ignore" });
      const timer = setTimeout(resolve, 45_000);
      const done = () => (clearTimeout(timer), resolve());
      child.on("exit", done);
      child.on("error", done);
      child.unref();
    } catch {
      resolve();
    }
  }).finally(() => (starting = null));
  await starting;
  forgetAgentsState();
  const target = await bridgeBack(context.bridge, context.signal);
  const away = target ? null : agentsAway();
  return yield* json(away ? 503 : 200, away ?? {});
}
