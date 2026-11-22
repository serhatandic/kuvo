// The resident server owns demand and idle admission. The keeper still launches the
// unmodified engine and recovers an unexpected exit using its existing record.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { swapUnderWay } from "../../shared/engine-swap";
import { engineRuns } from "../../shared/platform";
import type { EngineRecord } from "../../shared/records";
import { updateUnderWay } from "../../shared/updating";
import { APP_FOLDER, K, RUN } from "../home";
import { residentWorkersInstalled } from "../server/resident";
import { updateState } from "../updates/update";
import { engineBusy } from "../updates/engine-swap";
import { readEngineFile } from "./client";

const keeper = path.join(K, "bin", "kuvo-start");
const stopped = () => ["off", "paused", "stopped"].some(name => fs.existsSync(path.join(RUN, name)));
const switching = () => Boolean(swapUnderWay(RUN)) || fs.existsSync(path.join(RUN, "previous"));
const startingFenced = () => switching() || updateUnderWay(RUN) &&
  !["checking", "downloading", "downloading the engine", "checking it"].includes(updateState().running ?? "");
const parkingFenced = () => switching() || updateUnderWay(RUN);

function liveEngine(): EngineRecord | null {
  try {
    const record = readEngineFile();
    return record.port && record.password && engineRuns(record.pid, K) ? record : null;
  } catch { return null; }
}

function command(action: string, ...args: string[]): Promise<void> {
  const env: NodeJS.ProcessEnv = { ...process.env, HOME: path.dirname(K) };
  delete env.BUN_BE_BUN;
  return new Promise(resolve => {
    const child = spawn("sh", [keeper, action, ...args], { cwd: K, env, stdio: "ignore" });
    child.once("exit", () => resolve());
    child.once("error", () => resolve());
  });
}

let active: EngineLifetime | undefined;

export class EngineLifetime {
  private used: number;
  private requests = 0;
  private starting?: Promise<void>;
  private parking?: Promise<void>;

  constructor(private readonly clock = () => performance.now(), private readonly idle = 10 * 60_000) {
    this.used = clock();
  }

  get state() {
    return stopped() ? "stopped" : this.starting ? "starting" : liveEngine() ? "running" : "idle";
  }

  /** Pin the engine until the request has handed over its work. A cancellation
   * releases its pin when the caller finishes, independent of a shared cold start. */
  async acquire(): Promise<() => void> {
    this.requests++;
    this.used = this.clock();
    try {
      await this.parking;
      if (!liveEngine() && !stopped() && !startingFenced()) {
        this.starting ??= command("engine").finally(() => { this.starting = undefined; });
        await this.starting;
      }
    } catch (error) {
      this.requests--;
      throw error;
    }
    return () => {
      this.requests--;
      this.used = this.clock();
    };
  }

  /** An unconfirmed worker, an update or an overlapping acquisition all defer sleep. */
  async look(): Promise<void> {
    if (this.requests || this.parking || this.clock() - this.used < this.idle || stopped() || parkingFenced()) return;
    const engine = liveEngine();
    if (!engine) return;
    if (await engineBusy(engine)) {
      this.used = this.clock();
      return;
    }
    if (this.requests || this.parking || this.clock() - this.used < this.idle || stopped() || parkingFenced()) return;
    this.parking = command("engine-idle", String(engine.pid)).finally(() => { this.parking = undefined; });
    await this.parking;
  }
}

export function keepEngineOnDemand(clock?: () => number): EngineLifetime | undefined {
  if (APP_FOLDER || !residentWorkersInstalled() || !fs.existsSync(keeper)) return;
  active = new EngineLifetime(clock);
  setInterval(() => { void active?.look().catch(error => console.error("Engine idle check:", error)); }, 30_000).unref();
  return active;
}

export const engineState = () => active?.state;
export const engineManaged = () => active !== undefined;
export const acquireEngine = async (): Promise<() => void> => active ? active.acquire() : () => {};

/** Coding agents keep their own sessions, including while their bridge makes way. */
export const acquireSessionEngine = (session: string) =>
  /^[A-Za-z0-9_-]+$/.test(session) && fs.existsSync(path.join(APP_FOLDER ?? K, "agents", "sessions", session, "session.json"))
    ? Promise.resolve(() => {})
    : acquireEngine();
