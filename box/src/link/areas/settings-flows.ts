// The flows `kuvo/settings` leads (PROTOCOL.md, "A box's settings", `flow`): a sign-in in the
// browser for a row, kept in `run/settings-flows.json` so whichever of the box's processes reads the
// page states it (the long-running one starts it; one answering over SSH may read it). What each is
// for (a server's sign-in, its attempt with the engine) is its section's; this keeps them, one a row.

import fs from "node:fs";
import path from "node:path";
import { RUN } from "../home";

export const FLOWS_FILE = path.join(RUN, "settings-flows.json");

/** How long a flow that ended is still shown, so you see it went through (or why not) (ms). */
export const ENDED_SHOWN = 120_000;

/** A flow as kept: its section and row, its id, how it stands, the box's words once it ended, and
 * its section's own facts (`about`). */
export interface FlowRecord {
  section: string;
  row: string;
  id: string;
  state: "waiting" | "done" | "failed";
  line?: string;
  started: number;
  ended?: number;
  about: Record<string, string>;
}

/** The time flows go by (a test's own). */
export const flowClock = { now: () => Date.now() };

function readAll(file = FLOWS_FILE): FlowRecord[] {
  try {
    const kept = JSON.parse(fs.readFileSync(file, "utf8"));
    return Array.isArray(kept?.flows) ? kept.flows.filter((each: FlowRecord) => typeof each?.section === "string" && typeof each.row === "string" && typeof each.id === "string") : [];
  } catch {
    return [];
  }
}

function writeAll(flows: FlowRecord[], file = FLOWS_FILE): void {
  if (!flows.length) {
    fs.rmSync(file, { force: true });
    return;
  }
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const temporary = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, JSON.stringify({ flows }), { mode: 0o600 });
  fs.renameSync(temporary, file);
}

/** A section's flows now: one that ended a while ago is gone. */
export function flowsOf(section: string, file = FLOWS_FILE): FlowRecord[] {
  const now = flowClock.now();
  const all = readAll(file);
  const kept = all.filter(each => each.state === "waiting" || now - (each.ended ?? now) < ENDED_SHOWN);
  if (kept.length !== all.length) writeAll(kept, file);
  return kept.filter(each => each.section === section);
}

/** A row's flow started, in place of any it had. */
export function keepFlow(flow: Omit<FlowRecord, "started" | "state">, file = FLOWS_FILE): FlowRecord {
  const kept: FlowRecord = { ...flow, state: "waiting", started: flowClock.now() };
  writeAll([...readAll(file).filter(each => each.section !== flow.section || each.row !== flow.row), kept], file);
  return kept;
}

/** A waiting flow ended (`done`, or `failed` with why): true where it was waiting (so what follows
 * its end is done once). */
export function endFlow(section: string, id: string, state: "done" | "failed", line?: string, file = FLOWS_FILE): boolean {
  const all = readAll(file);
  const flow = all.find(each => each.section === section && each.id === id);
  if (!flow || flow.state !== "waiting") return false;
  Object.assign(flow, { state, ended: flowClock.now() }, line ? { line } : {});
  writeAll(all, file);
  return true;
}

/** A row's flow put away (cancelled, or seen once it ended). */
export function dropFlow(section: string, row: string, file = FLOWS_FILE): void {
  const all = readAll(file);
  const kept = all.filter(each => each.section !== section || each.row !== row);
  if (kept.length !== all.length) writeAll(kept, file);
}
