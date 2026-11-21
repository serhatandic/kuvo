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

/** These are the engine's v1 file/text parts and v2 inline message content. */
export async function restoreEngineAttachmentPaths(file: string, rewrite: (value: unknown) => unknown) {
  const SQLite = process.versions.bun
    ? (await import(["bun", "sqlite"].join(":"))).Database as typeof import("node:sqlite").DatabaseSync
    : (await import("node:sqlite")).DatabaseSync;
  const db = new SQLite(file);
  try {
    db.exec("PRAGMA synchronous=FULL; BEGIN IMMEDIATE");
    for (const table of ["part", "session_message"]) {
      const columns = db.prepare("SELECT name FROM pragma_table_info(?)").all(table);
      if (!["id", "data"].every(name => columns.some(column => column.name === name))) continue;
      const read = db.prepare(`SELECT id, data FROM ${table} WHERE id > ? ORDER BY id LIMIT 128`);
      const update = db.prepare(`UPDATE ${table} SET data = ? WHERE id = ?`);
      let after = "";
      for (;;) {
        const page = read.all(after);
        if (!page.length) break;
        for (const row of page) {
          if (typeof row.id !== "string" || typeof row.data !== "string") throw new Error("The engine transcript has an invalid record.");
          after = row.id;
          let value: unknown;
          try { value = JSON.parse(row.data); } catch { continue; }
          const mapped = rewrite(value);
          if (mapped !== value) update.run(JSON.stringify(mapped), row.id);
        }
      }
    }
    db.exec("COMMIT");
  } finally { db.close(); }
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
const [target, archive, origins, own] = process.argv.slice(1);
const db = new sqlite(target || archive);
function recover() {
  db.exec("PRAGMA busy_timeout=5000; PRAGMA synchronous=FULL;");
  if (target) db.prepare("ATTACH DATABASE ? AS archived").run(archive);
  for (const schema of target ? ["main", "archived"] : ["main"]) {
    if (![0, 1].includes(db.prepare("PRAGMA " + schema + ".user_version").get().user_version)) throw Error("Unsupported Insights database");
  }
  db.exec("BEGIN IMMEDIATE");
  let seq = JSON.parse(db.prepare("SELECT body FROM main.state WHERE key = 'sequence'").get()?.body ?? "0");
  if (!Number.isSafeInteger(seq) || seq < 0) throw Error("Invalid Insights sequence");
  // Even on the same hardware, an old pre-enrollment alias does not prove membership.
  if (!target) {
    db.prepare("DELETE FROM event WHERE computer NOT IN (SELECT value FROM json_each(?))").run(origins);
    db.exec("COMMIT");
    return;
  }
  const before = seq;
  const result = db.prepare(\`
    WITH recovered AS MATERIALIZED (
      SELECT saved.*, ? + ROW_NUMBER() OVER (ORDER BY saved.at, saved.key) AS next,
        kept.computer = saved.computer AND json_extract(kept.body, '$.accountObserved') = 1 AS keep_account,
        kept.account AS prior_account
      FROM archived.event AS saved LEFT JOIN main.event AS kept ON kept.key = saved.key
      WHERE saved.computer IN (SELECT value FROM json_each(?))
        AND (kept.key IS NULL
          OR (saved.computer = kept.computer AND saved.partial < kept.partial)
          OR (saved.computer <> kept.computer AND (
            (kept.computer <> ? AND kept.computer NOT IN (SELECT value FROM json_each(?)))
            OR COALESCE(json_extract(saved.body, '$.attributed'), 0) > COALESCE(json_extract(kept.body, '$.attributed'), 0)
            OR (COALESCE(json_extract(saved.body, '$.attributed'), 0) = COALESCE(json_extract(kept.body, '$.attributed'), 0)
              AND (saved.received, saved.computer) < (kept.received, kept.computer))
          )))
    )
    INSERT OR REPLACE INTO main.event (
      key, seq, sender, received, at, session, computer, bot, bot_name, title,
      provider, account, model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output,
      skills, body, supersedes, processed
    )
    SELECT key, next, sender, received, at, session, computer, bot, bot_name, title,
      provider, CASE WHEN keep_account THEN prior_account ELSE account END,
      model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output, skills,
      CASE WHEN keep_account THEN json_set(body, '$.account', prior_account, '$.accountObserved', json('true')) ELSE body END,
      supersedes, processed FROM recovered
  \`).run(seq, origins, own, origins);
  seq += Number(result.changes);
  const enriched = db.prepare(\`
    WITH accounts AS MATERIALIZED (
      SELECT kept.key, saved.account, ? + ROW_NUMBER() OVER (ORDER BY kept.at, kept.key) AS next
      FROM main.event AS kept JOIN archived.event AS saved USING (key)
      WHERE kept.computer = saved.computer AND saved.computer IN (SELECT value FROM json_each(?))
        AND saved.account IS NOT NULL AND json_extract(saved.body, '$.accountObserved') = 1
        AND COALESCE(json_extract(kept.body, '$.accountObserved'), 0) = 0
    )
    UPDATE main.event SET account = accounts.account, seq = accounts.next,
      body = json_set(event.body, '$.account', accounts.account, '$.accountObserved', json('true'))
    FROM accounts WHERE event.key = accounts.key
  \`).run(seq, origins);
  seq += Number(enriched.changes);
  if (seq !== before) db.prepare(
    "INSERT INTO main.state VALUES ('sequence', ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body"
  ).run(JSON.stringify(seq));
  db.exec("COMMIT");
}
try { recover(); } finally { db.close(); }
`;

/** Insights observations have their own immutable IDs. Preserve the local index and
 * its cursors while adding accepted observations from an encrypted recovery point. */
export function mergeInsightsDatabase(target: string | null, archived: string, origins: string[], own: string | null): boolean {
  if (target && (!fs.lstatSync(target).isFile() ||
      ["-wal", "-shm", "-journal"].some(suffix => fs.lstatSync(target + suffix, { throwIfNoEntry: false })?.isFile() === false))) return false;
  return spawnSync(process.execPath, [...OWN_RUNTIME, "-e", mergeInsights, target ?? "", archived, JSON.stringify(origins), own ?? ""],
    { env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 60_000 }).status === 0;
}
