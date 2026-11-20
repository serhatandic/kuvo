import fs from "node:fs";
import path from "node:path";
import { artifactIndexFormat } from "../../shared/artifact-format";
import { isObject } from "../../shared/json";

/** Latest entry per artifact version, in the same journal order the store replays. */
export function retainedArtifactCopies(root: string): Map<string, { expectedSize: number; expectedSHA: string }> {
  const log = path.join(root, "log");
  const copies = new Map<string, { expectedSize: number; expectedSHA: string }>();
  if (!fs.lstatSync(log, { throwIfNoEntry: false })?.isDirectory()) return copies;
  const versions = new Map<string, Record<string, unknown>>();
  for (const name of fs.readdirSync(log).sort()) {
    const file = path.join(log, name);
    if (!name.endsWith(".jsonl") || name.startsWith(".") || !fs.lstatSync(file, { throwIfNoEntry: false })?.isFile()) continue;
    for (const line of fs.readFileSync(file, "utf8").split("\n")) {
      let saved: unknown;
      try { saved = JSON.parse(line); } catch { continue; } // A torn last line has no version.
      if (!isObject(saved)) continue;
      const { a, v } = artifactIndexFormat.read(saved);
      if (isObject(a) && typeof a.id === "string" && isObject(v) && typeof v.n === "number") versions.set(JSON.stringify([a.id, v.n]), v);
    }
  }
  for (const version of versions.values()) {
    if (version.kept !== true) continue;
    const hex = typeof version.hash === "string" ? /^sha256:([a-f0-9]{64})$/.exec(version.hash)?.[1] : undefined;
    if (!hex || !Number.isSafeInteger(version.size) || (version.size as number) < 0) throw new Error("A retained artifact version has an invalid inventory.");
    const name = `artifacts/copies/${hex.slice(0, 2)}/${hex}`;
    const expectedSize = version.size as number;
    if (copies.has(name) && copies.get(name)!.expectedSize !== expectedSize) throw new Error("Retained artifact versions disagree about their byte count.");
    copies.set(name, { expectedSize, expectedSHA: Buffer.from(hex, "hex").toString("base64url") });
  }
  return copies;
}
