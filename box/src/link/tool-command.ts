// A command approved by a tool runs in a child that can finish while the resident is restarted.
// Its inputs (including vault values) travel over stdin. Only the masked result is kept.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { syncDurableFile, writeDurablePrivate } from "../shared/files";
import { processRuns } from "../shared/platform";
import { pause } from "../shared/wait";
import { OWN_RUNTIME, RUN, SCRIPT } from "./home";
import { runCommand } from "./run-command";
import { ToolCallPending } from "./server/tool-call";
import { maskValues, type Hidden } from "./vault/mask";

type Result = Awaited<ReturnType<typeof runCommand>>;
type Spec = { command: string; folder: string; env: Record<string, string>; timeout: number; hidden?: Hidden[]; scratch?: string; files?: string[]; head?: string; args?: string[] };
type Receipt = { pid: number; result?: Result; head?: string };
const valid = (key: string) => /^[A-Za-z0-9_-]{43}$/.test(key);
const file = (key: string) => path.join(RUN, "tool-commands", key + ".json");

function receipt(key: string): Receipt | undefined {
  try { return JSON.parse(fs.readFileSync(file(key), "utf8")); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

export async function commandReceipt(key: string) {
  let kept = receipt(key);
  if (!kept) return undefined;
  while (!kept.result) {
    if (!processRuns(kept.pid, "tool-command-worker")) {
      kept = receipt(key) ?? kept;
      if (kept.result) break;
      return { result: { output: "The command worker was interrupted after accepting the command.", code: null, timedOut: false }, head: kept.head };
    }
    await pause(200);
    kept = receipt(key) ?? kept;
  }
  return { result: kept.result, head: kept.head };
}

export async function runToolCommand(key: string, spec: Spec) {
  const kept = await commandReceipt(key);
  if (kept) return kept;
  const child = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "box", "tool-command-worker", key], {
    env: { ...process.env, ...(process.versions.bun ? { BUN_BE_BUN: "1" } : {}) },
    stdio: ["pipe", "ignore", "ignore"], detached: true,
  });
  child.on("error", () => {});
  child.stdin?.on("error", () => {});
  child.stdin?.end(JSON.stringify(spec));
  child.unref();
  for (let attempt = 0; attempt < 100; attempt++) {
    const answer = await commandReceipt(key);
    if (answer) return answer;
    if (child.exitCode !== null || child.signalCode !== null) break;
    await pause(100);
  }
  throw new ToolCallPending("Waiting for the command worker to accept the tool call.");
}

export async function toolCommandWorker(key: string, input: () => Promise<string>) {
  if (!valid(key)) throw new Error("Invalid tool command id.");
  const spec: Spec = JSON.parse(await input());
  const target = file(key);
  fs.mkdirSync(path.dirname(target), { recursive: true, mode: 0o700 });
  // Publish a whole, durable claim using an exclusive link. A second worker can see the
  // first owner's PID or result even when the resident that spawned it has gone away.
  const offer = target + "." + process.pid;
  const claim = { pid: process.pid, head: spec.head };
  writeDurablePrivate(offer, JSON.stringify(claim), RUN);
  try {
    try { fs.linkSync(offer, target); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST") return;
      throw error;
    } finally { fs.rmSync(offer, { force: true }); }
    syncDurableFile(path.dirname(target), RUN);
    const result = await runCommand(spec.command, spec.folder, spec.env, spec.timeout, undefined, key, undefined, spec.args);
    const output = spec.hidden ? maskValues(result.output, spec.hidden) : result.output;
    writeDurablePrivate(target, JSON.stringify({ ...claim, result: { ...result, output } }), RUN);
  } finally {
    for (const file of spec.files ?? []) fs.rmSync(file, { force: true });
    if (spec.scratch) fs.rmSync(spec.scratch, { recursive: true, force: true });
  }
}
