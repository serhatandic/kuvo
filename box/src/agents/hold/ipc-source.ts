// Embed the shared IPC module in the standalone holder. The bridge also imports the module as
// code, so a text import would ask Bun's bundler to load the same file with two loaders.
import { readFileSync } from "node:fs";

export function ipcSource(): string {
  return readFileSync(new URL("../../shared/local-ipc.js", import.meta.url), "utf8");
}
