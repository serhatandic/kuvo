import { spawnSync } from "node:child_process";
import { OWN_RUNTIME } from "../home";

const scrub = `
const SQLite = process.versions.bun ? require("bun:sqlite").Database : require("node:sqlite").DatabaseSync;
const db = new SQLite(process.argv[1]);
const moved = JSON.parse(process.argv[2]);
const restoring = process.argv[3] === "true";
const sensitive = /(?:^|_)(?:auth|credentials?|oauth|tokens?|permissions?|grants?)(?:_|$)/i;
const tables = db.prepare("SELECT name FROM sqlite_schema WHERE type='table'").all().map(row => row.name);
const quote = name => '"' + name.replaceAll('"','""') + '"';
db.exec("PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON; BEGIN;");
for (const name of tables) {
  if (sensitive.test(name) || (restoring && /(?:^|_)inbox(?:_|$)/i.test(name))) db.exec("DELETE FROM " + quote(name));
}
if (moved.length && tables.includes("session")) {
  const drop = db.prepare("DELETE FROM session WHERE id = ?");
  for (const id of moved) drop.run(id);
}
db.exec("COMMIT; VACUUM;");
db.close();`;

/** Strip the engine's credential and grant tables before a SQLite page can enter an archive.
 * VACUUM also drops freed pages which may have held credentials. Restoring clears inboxes. */
export function scrubEngine(file: string, moved: string[], restoring: boolean): void {
  const result = spawnSync(process.execPath, [...OWN_RUNTIME, "-e", scrub, file, JSON.stringify(moved), String(restoring)],
    { env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 60_000 });
  if (result.status !== 0) throw new Error("Could not reconcile the engine database.");
}
