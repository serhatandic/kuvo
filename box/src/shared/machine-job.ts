import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat } from "./format-version";

export const machineJobFormat = additiveObjectFormat("Machine helper job");
export const machineJobPath = (run: string) => path.join(run, "machine-helper-job.json");

const phases = ["installing", "waiting", "workers", "restoring"] as const;
export type MachineJob = {
  phase: typeof phases[number];
  owner: number;
  plist: string | null;
  loaded: boolean;
  legacy: boolean;
  version?: string;
  helper?: number;
};

/** Refuse unreadable work instead of treating it as permission to start a new job. */
export function readMachineJob(run: string): MachineJob | null {
  let text: string;
  try { text = fs.readFileSync(machineJobPath(run), "utf8"); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw error;
  }
  const job = machineJobFormat.read(JSON.parse(text));
  if (!phases.some(phase => phase === job.phase)
      || typeof job.owner !== "number" || !Number.isSafeInteger(job.owner) || job.owner <= 1
      || (job.plist !== null && typeof job.plist !== "string")
      || typeof job.loaded !== "boolean" || typeof job.legacy !== "boolean"
      || (job.version !== undefined && typeof job.version !== "string")
      || (job.helper !== undefined && (typeof job.helper !== "number" || !Number.isSafeInteger(job.helper) || job.helper <= 1))) {
    throw new Error("The machine helper job is unreadable. Update Kuvo before resuming it.");
  }
  return job as MachineJob;
}
