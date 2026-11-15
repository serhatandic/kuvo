// Shared by the store and activity without importing either.
import fs from "node:fs";
import path from "node:path";
import { locked } from "../files";
import { RUN } from "../home";

const LOCK = path.join(RUN, ".vault.lock");

/** One change at a time, whoever makes it. */
export const vaultWork = <T>(work: () => T): T => {
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  return locked(LOCK, work);
};
