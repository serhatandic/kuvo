// Whether you've had Kuvo Code's first run with this box (PROTOCOL.md, "Your setup", `welcomed`):
// kept on the box, so every client of it agrees, and a client installed again doesn't welcome you
// again. `run/welcomed.json`, beside link.json: when it was done.

import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { writePrivate } from "../files";
import { RUN } from "../home";

export const welcomeFormat = additiveObjectFormat("Setup welcome record");
const FILE = () => path.join(RUN, "welcomed.json");

/** Whether the first run is done with this box. */
export function isWelcomed(): boolean {
  if (!fs.existsSync(FILE())) return false;
  try {
    const value: unknown = JSON.parse(fs.readFileSync(FILE(), "utf8"));
    if (isObject(value)) welcomeFormat.read(value);
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
  return true;
}

/** The first run done (`true`, kept from the first time), or to be had again (`false`). */
export function setWelcomed(welcomed: boolean): void {
  const before = isWelcomed();
  if (!welcomed) return fs.rmSync(FILE(), { force: true });
  if (before) return;
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  writePrivate(FILE(), JSON.stringify(welcomeFormat.write({ at: Date.now() })));
}
