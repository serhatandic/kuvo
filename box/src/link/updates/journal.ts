// Bounded diagnostics for transfer and installer attempts. The current transfer checkpoint is
// separate from this log so a restart can report the interrupted step.
import fs from "node:fs";
import path from "node:path";
import { RUN } from "../home";

const file = path.join(RUN, "fleet-update.log");

export function appendUpdateEvent(version: string, step: string, message: string, at = Date.now()): void {
  try {
    if ((fs.statSync(file, { throwIfNoEntry: false })?.size ?? 0) >= 512_000) fs.renameSync(file, `${file}.1`);
    fs.appendFileSync(file, JSON.stringify({ at, version, step, message }) + "\n", { mode: 0o600 });
  } catch (error) {
    console.error(`Kuvo updates: ${step}: ${message} (${(error as Error).message})`);
  }
}
