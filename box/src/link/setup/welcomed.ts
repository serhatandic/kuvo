// Whether you've had Kuvo Code's first run with this box (PROTOCOL.md, "Your setup", `welcomed`):
// kept on the box, so every client of it agrees, and a client installed again doesn't welcome you
// again. `run/welcomed.json`, beside link.json: when it was done.

import fs from "node:fs";
import path from "node:path";
import { writePrivate } from "../files";
import { RUN } from "../home";

const FILE = () => path.join(RUN, "welcomed.json");

/** Whether the first run is done with this box. */
export const isWelcomed = (): boolean => fs.existsSync(FILE());

/** The first run done (`true`, kept from the first time), or to be had again (`false`). */
export function setWelcomed(welcomed: boolean): void {
  if (!welcomed) return fs.rmSync(FILE(), { force: true });
  if (isWelcomed()) return;
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  writePrivate(FILE(), JSON.stringify({ at: Date.now() }));
}
