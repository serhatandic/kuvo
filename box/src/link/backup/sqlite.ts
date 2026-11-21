import fs from "node:fs";
import { spawnSync } from "node:child_process";
import { OWN_RUNTIME } from "../home";

const snapshot = `
const bun = Boolean(process.versions.bun);
const SQLite = bun ? require("bun:sqlite").Database : require("node:sqlite").DatabaseSync;
const db = new SQLite(process.argv[1], bun ? { readonly: true } : { readOnly: true });
try {
  db.exec("PRAGMA busy_timeout=5000");
  db.exec(process.argv[2]);
} finally { db.close(); }
`;

export function snapshotDatabase(file: string, copy: string): void {
  const query = "VACUUM INTO '" + copy.replaceAll("'", "''") + "'";
  const result = spawnSync(process.execPath, [...OWN_RUNTIME, "-e", snapshot, file, query], {
    env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 60_000,
  });
  if (result.status !== 0) throw new Error("Could not take a consistent database snapshot.");
}

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
for (const table of ["session", "session_v2"]) {
  if (moved.length && tables.includes(table)) {
    const drop = db.prepare("DELETE FROM " + quote(table) + " WHERE id = ?");
    for (const id of moved) drop.run(id);
  }
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

const bootstrap = `
const db = process.versions.bun
  ? new (require("bun:sqlite").Database)(process.argv[1], { readwrite: true })
  : new (require("node:sqlite").DatabaseSync)(process.argv[1]);
const tables = db.prepare("SELECT name FROM sqlite_schema WHERE type='table'").all().map(row => row.name);
if (Object.values(db.prepare("PRAGMA quick_check").all()[0])[0] !== "ok" || !(tables.includes("session") || tables.includes("session_v2"))) process.exit(1);
for (const name of tables) {
  if (name === "migration" || name === "__drizzle_migrations" || name === "sqlite_sequence") continue;
  const quote = '"' + name.replaceAll('"', '""') + '"';
  if (db.prepare("SELECT 1 FROM " + quote + " LIMIT 1").all().length) process.exit(1);
}
if (Object.values(db.prepare("PRAGMA journal_mode=DELETE").all()[0])[0] !== "delete") process.exit(1);
db.close();`;

/** A stopped engine's schema and migration bookkeeping alone are disposable. Retain
 * unknown or populated databases and let the user decide how to recover their history. */
export function prepareEngineBootstrap(file: string): boolean {
  if (!fs.lstatSync(file).isFile() || ["-wal", "-shm", "-journal"].some(suffix => fs.lstatSync(file + suffix, { throwIfNoEntry: false })?.isFile() === false)) return false;
  return spawnSync(process.execPath, [...OWN_RUNTIME, "-e", bootstrap, file], {
    env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 10_000,
  }).status === 0;
}

const mergeInsights = `
const sqlite = process.versions.bun ? require("bun:sqlite").Database : require("node:sqlite").DatabaseSync;
const db = new sqlite(process.argv[1]);
const origins = JSON.parse(process.argv[3]);
try {
  db.exec("PRAGMA busy_timeout=5000; PRAGMA synchronous=FULL;");
  db.prepare("ATTACH DATABASE ? AS archived").run(process.argv[2]);
  for (const schema of ["main", "archived"]) {
    if (![0, 1].includes(db.prepare("PRAGMA " + schema + ".user_version").get().user_version)) process.exit(1);
  }
  db.exec("BEGIN IMMEDIATE");
  const seq = JSON.parse(db.prepare("SELECT body FROM main.state WHERE key = 'sequence'").get()?.body ?? "0");
  if (!Number.isSafeInteger(seq) || seq < 0) process.exit(1);
  const result = db.prepare(\`
    WITH recovered AS MATERIALIZED (
      SELECT saved.*, ? + ROW_NUMBER() OVER (ORDER BY saved.at, saved.key) AS next
      FROM archived.event AS saved LEFT JOIN main.event AS kept ON kept.key = saved.key
      WHERE saved.computer IN (SELECT value FROM json_each(?))
        AND (kept.key IS NULL OR (
          saved.computer <> kept.computer AND json_extract(saved.body, '$.attributed') = 1
          AND (COALESCE(json_extract(kept.body, '$.attributed'), 0) = 0 OR saved.received < kept.received)
        ))
    )
    INSERT OR REPLACE INTO main.event (
      key, seq, sender, received, at, session, computer, bot, bot_name, title,
      provider, account, model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output,
      skills, body, supersedes, processed
    )
    SELECT key, next, sender, received, at, session, computer, bot, bot_name, title,
      provider, account, model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output,
      skills, body, supersedes, processed FROM recovered
  \`).run(seq, origins);
  if (result.changes) db.prepare(
    "INSERT INTO main.state VALUES ('sequence', ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body"
  ).run(JSON.stringify(seq + Number(result.changes)));
  db.exec("COMMIT");
} finally { db.close(); }
`;

/** Insights observations have their own immutable IDs. Preserve the local index and
 * its cursors while adding accepted observations from an encrypted recovery point. */
export function mergeInsightsDatabase(target: string, archived: string, origins: string[]): boolean {
  if (!origins.length || !fs.lstatSync(target).isFile() ||
      ["-wal", "-shm", "-journal"].some(suffix => fs.lstatSync(target + suffix, { throwIfNoEntry: false })?.isFile() === false)) return false;
  return spawnSync(process.execPath, [...OWN_RUNTIME, "-e", mergeInsights, target, archived, JSON.stringify(origins)],
    { env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 60_000 }).status === 0;
}
