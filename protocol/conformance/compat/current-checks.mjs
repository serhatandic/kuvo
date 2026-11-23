// Keep the executable sample list tied to the architecture inventory.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

const names = table => table.split("\n").filter(line => line.startsWith("| ")).slice(2).map(line => line.split("|")[1].trim());
const inventory = readFileSync(new URL("../../../docs/architecture/stored-formats.md", import.meta.url), "utf8").split("\n## Message formats")[0];
const table = readFileSync(new URL("./current-families.md", import.meta.url), "utf8");
const rows = table.split("\n").filter(line => line.startsWith("| ")).slice(2).map(line => {
  const [, family, , glob, result] = line.split("|").map(field => field.trim());
  return { family, glob: glob.slice(1, -1), result };
});

export function currentChecks(root) {
  assert.deepEqual(names(table), names(inventory));
  const files = readdirSync(root, { recursive: true, withFileTypes: true }).filter(entry => entry.isFile())
    .map(entry => path.relative(root, path.join(entry.parentPath, entry.name)).split(path.sep).join("/")).sort();
  return rows.map(({ family, glob, result }) => {
    const pattern = new RegExp("^" + glob.split("*").map(part => RegExp.escape(part)).join("[^/]+") + "$");
    const entries = files.filter(file => pattern.test(file)).map(file => ({ file, bytes: readFileSync(path.join(root, file)) }));
    assert.ok(entries.length, `${family} has no fixture matching ${glob}`);
    return { family, result, entries };
  });
}
