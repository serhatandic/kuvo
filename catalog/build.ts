// The single downloadable snapshot and the box's embedded copy use the same bytes.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = import.meta.dir;
const digest = (bytes: Buffer) => createHash("sha256").update(bytes).digest("hex");

function snapshot(area: string, manifest: string) {
  return readdirSync(join(root, area), { withFileTypes: true })
    .filter(entry => entry.isDirectory()).map(entry => entry.name).sort()
    .map(id => {
      const folder = join(root, area, id);
      const entry = JSON.parse(readFileSync(join(folder, manifest), "utf8"));
      if (entry.id !== id) throw new Error(`The ID in ${area}/${id} must match its folder.`);
      if (area === "apps") return { ...entry, logo: readFileSync(join(folder, "logo.png")).toString("base64") };
      if (area === "agents") return entry;
      const bytes = readFileSync(join(folder, "SKILL.md"));
      return { ...entry, files: [{ path: "SKILL.md", data: bytes.toString("base64"), sha256: digest(bytes) }] };
    });
}
const text = JSON.stringify({
  format: 1,
  apps: snapshot("apps", "app.json"),
  agents: snapshot("agents", "agent.json"),
  skills: snapshot("skills", "skill.json"),
}, null, 2) + "\n";
const index = join(root, "index.json");
if (process.argv.includes("--check")) {
  if (readFileSync(index, "utf8") !== text) throw new Error("Run bun catalog/build.ts to update the snapshot.");
} else {
  writeFileSync(index, text);
}
