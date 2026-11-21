import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { setImmediate as nextTurn } from "node:timers/promises";
import { RUN } from "../home";
import { locked, privateFolder } from "../../shared/files";
import { isObject } from "../../shared/json";
import { readSubmissionID, submissionKey, submissionReference } from "./submission-address";
import { draftReplayFence, readForwardEvidence } from "./forward-evidence-address";
import { readSubmitter } from "./submitter";
import type { DatabaseSync } from "node:sqlite";

const VERSION = 1;
const root = path.join(RUN, "owned-data");
const folder = path.join(root, "receipt-index-v" + VERSION);
const file = path.join(folder, "addresses.sqlite");
const lock = path.join(root, ".receipt-index.lock");
export const RECEIPT_INDEX_BATCH = 128;
const BUILD_BYTES = 4 * 1024 * 1024;
export const RECEIPT_PAGE = 40;

export type IndexedFamily = "submissions" | "outbox" | "message-receipts" | "mutations";
const families: readonly IndexedFamily[] = ["submissions", "outbox", "message-receipts", "mutations"];
const validKey = /^[A-Za-z0-9_-]{43}\.json$/;
const absent = (error: unknown) => (error as NodeJS.ErrnoException).code === "ENOENT";

function stamp(family: IndexedFamily): string {
  try {
    const stat = fs.statSync(path.join(root, family), { bigint: true });
    return [stat.dev, stat.ino, stat.mtimeNs, stat.ctimeNs].join("/");
  } catch (error) {
    if (absent(error)) return "-";
    throw error;
  }
}

function database<T>(work: (db: DatabaseSync) => T): T {
  privateFolder(folder);
  const SQLite = process.versions.bun
    ? require(["bun", "sqlite"].join(":")).Database as typeof DatabaseSync
    : require("node:sqlite").DatabaseSync as typeof DatabaseSync;
  const db = new SQLite(file);
  try {
    db.exec(`
      PRAGMA busy_timeout=500;
      CREATE TABLE IF NOT EXISTS family (
        name TEXT PRIMARY KEY, stamp TEXT NOT NULL, generation TEXT NOT NULL, catalogued INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS retired (family TEXT NOT NULL, generation TEXT NOT NULL, PRIMARY KEY(family, generation));
      CREATE TABLE IF NOT EXISTS scan (
        family TEXT NOT NULL, generation TEXT NOT NULL, key TEXT NOT NULL, PRIMARY KEY(family, generation, key)
      );
      CREATE TABLE IF NOT EXISTS entry (
        family TEXT NOT NULL, generation TEXT NOT NULL, key TEXT NOT NULL, setup TEXT NOT NULL, topic TEXT NOT NULL,
        author TEXT NOT NULL, issuer TEXT NOT NULL, handle TEXT NOT NULL,
        PRIMARY KEY(family, generation, key)
      );
      CREATE INDEX IF NOT EXISTS receipt_page ON entry(family, generation, setup, topic, author, issuer, handle, key);
      CREATE INDEX IF NOT EXISTS receipt_scope ON entry(family, generation, setup, topic, key);
    `);
    db.exec("BEGIN IMMEDIATE");
    try {
      const result = work(db);
      db.exec("COMMIT");
      return result;
    } catch (error) {
      try { db.exec("ROLLBACK"); } catch {}
      throw error;
    }
  } finally {
    db.close();
  }
}

function indexed<T>(work: (db: DatabaseSync) => T): T {
  return locked(lock, () => database(work), { exclusive: true, wait: 500 });
}

export function receiptAuthor(value: unknown, through = ""): string | undefined {
  const author = readSubmitter(value);
  return author && (author.kind === "computer" ? "computer:" + through : author.kind + ":" + author.key);
}

function addresses(family: IndexedFamily, key: string, record: unknown): string[] | undefined {
  // A version this reader cannot place must leave the scope incomplete, especially for a move.
  const unreadable = ["", "unreadable", "", "", ""];
  if (!isObject(record) || (record.formatVersion !== undefined && record.formatVersion !== 1 && record.formatVersion !== 0))
    return unreadable;
  if (family === "submissions") {
    const saved = record.submission;
    const ref = submissionReference(saved, key);
    const handle = isObject(saved) && readSubmissionID(saved.id);
    if (!ref || !handle || !isObject(saved)) return saved === undefined ? undefined : unreadable;
    const author = receiptAuthor(saved.submitter, typeof saved.submittedThrough === "string" ? saved.submittedThrough : "");
    return author ? [handle.setup, "session:" + ref.id, author, handle.origin, handle.submission] : unreadable;
  }
  if (family !== "mutations") {
    if (record.scoped !== true) return;
    const handle = readSubmissionID(record.submission);
    const author = receiptAuthor(record.submitter);
    return handle && typeof record.record === "string" && record.setup === handle.setup
      && key === submissionKey(handle.setup, record.record, handle.submission) && author
      ? [handle.setup, "session:" + record.record, author, "", handle.submission] : unreadable;
  }
  try {
    const fence = draftReplayFence(record.draftReplayFence, key);
    if (fence) return [String(fence.setup), "session:" + fence.session, "", "", ""];
    const forward = readForwardEvidence(record.forward, key);
    const id = forward?.ref?.id;
    if (!id) return;
    if (forward.kind === "forwardedQueueDecisions") return [forward.setup, "item:" + id, "", "", ""];
    if (forward.kind === "forwardedSessionDecisions") return [forward.setup, "session:" + id, "", "", ""];
    if (forward.kind === "forwardedGroupDecisions") return [forward.setup, "group:" + id, "", "", ""];
  } catch { return unreadable; }
  return undefined;
}

function setEntry(db: DatabaseSync, family: IndexedFamily, generation: string, key: string, value: unknown) {
  const entry = value === undefined ? undefined : addresses(family, key, value);
  if (entry) db.prepare("INSERT OR REPLACE INTO entry VALUES (?, ?, ?, ?, ?, ?, ?, ?)").run(family, generation, key, ...entry);
  else db.prepare("DELETE FROM entry WHERE family=? AND generation=? AND key=?").run(family, generation, key);
}

type Generation = { stamp: string; generation: string; catalogued: number };
function known(db: DatabaseSync, family: IndexedFamily) {
  return db.prepare("SELECT stamp, generation, catalogued FROM family WHERE name=?").get(family) as Generation | undefined;
}

function start(db: DatabaseSync, family: IndexedFamily, at: string) {
  const prior = known(db, family);
  if (prior?.stamp === at) return prior;
  const generation = randomUUID();
  if (prior) db.prepare("INSERT OR IGNORE INTO retired VALUES (?, ?)").run(family, prior.generation);
  db.prepare("INSERT OR REPLACE INTO family VALUES (?, ?, ?, 0)").run(family, at, generation);
  return { stamp: at, generation, catalogued: 0 };
}

function remember(db: DatabaseSync, family: IndexedFamily, generation: string, keys: string[]) {
  const insert = db.prepare("INSERT OR IGNORE INTO scan VALUES (?, ?, ?)");
  for (const key of keys) insert.run(family, generation, key);
}

function publish(db: DatabaseSync, family: IndexedFamily, generation: string) {
  db.prepare("UPDATE family SET catalogued=1 WHERE name=? AND generation=?").run(family, generation);
}

function fewKeys(family: IndexedFamily): string[] | undefined {
  try {
    const directory = fs.opendirSync(path.join(root, family));
    const found: string[] = [];
    try {
      for (let visited = 0; visited <= RECEIPT_INDEX_BATCH; visited++) {
        const entry = directory.readSync();
        if (!entry) return found;
        if (validKey.test(entry.name)) found.push(entry.name.slice(0, -5));
      }
    } finally { directory.closeSync(); }
    return undefined; // The keeper inventories a large legacy directory off the request path.
  } catch (error) {
    if (absent(error)) return [];
    throw error;
  }
}

function prepare(db: DatabaseSync, family: IndexedFamily, at: string) {
  const prior = known(db, family);
  if (prior?.stamp === at) return prior.catalogued ? prior : undefined;
  const found = fewKeys(family);
  const staged = start(db, family, at);
  if (!found) return;
  remember(db, family, staged.generation, found);
  publish(db, family, staged.generation);
  return { ...staged, catalogued: 1 };
}

function refresh(db: DatabaseSync, selected: readonly IndexedFamily[]) {
  const before = selected.map(family => stamp(family));
  let remaining = RECEIPT_INDEX_BATCH;
  let bytes = BUILD_BYTES;
  scan: for (const [i, family] of selected.entries()) {
    const staged = prepare(db, family, before[i]!);
    if (!staged) continue;
    const batch = db.prepare("SELECT key FROM scan WHERE family=? AND generation=? ORDER BY key LIMIT ?")
      .all(family, staged.generation, remaining) as { key: string }[];
    for (const { key } of batch) {
      let value: unknown;
      try {
        const file = path.join(root, family, key + ".json");
        const size = fs.statSync(file).size;
        if (size > bytes && bytes !== BUILD_BYTES) break scan;
        value = JSON.parse(fs.readFileSync(file, "utf8"));
        bytes -= size;
      } catch (error) { if (!absent(error)) value = null; }
      setEntry(db, family, staged.generation, key, value);
      db.prepare("DELETE FROM scan WHERE family=? AND generation=? AND key=?").run(family, staged.generation, key);
      remaining--;
      if (bytes <= 0 || remaining === 0) break scan;
    }
  }
  return before;
}

function queryGeneration(db: DatabaseSync, family: IndexedFamily, at: string) {
  const staged = known(db, family);
  return staged?.stamp === at && staged.catalogued ? staged.generation : null;
}

function complete(db: DatabaseSync, selected: readonly IndexedFamily[], before: string[]) {
  return selected.every((family, i) => {
    const generation = queryGeneration(db, family, before[i]!);
    return generation && before[i] === stamp(family)
      && !db.prepare("SELECT 1 FROM scan WHERE family=? AND generation=? LIMIT 1").get(family, generation)
      && !db.prepare("SELECT 1 FROM entry WHERE family=? AND generation=? AND setup='' AND topic='unreadable' LIMIT 1").get(family, generation);
  });
}

/** A receipt remains canonical if an index write is interrupted. Directory changes after
 * the last committed stamp leave a rebuild pending. Raw legacy installs are discovered too. */
export function changeReceiptFile(family: string, key: string, value: unknown, change: () => void): void {
  if (!families.includes(family as IndexedFamily)) return change();
  const target = family as IndexedFamily;
  if (!fs.existsSync(file) && !addresses(target, key, value)) return change();
  let began = false;
  let changed = false;
  try {
    indexed(db => {
      const before = stamp(target);
      const staged = known(db, target) ?? prepare(db, target, before);
      began = true;
      change();
      changed = true;
      if (!staged?.catalogued || staged.stamp !== before) return;
      setEntry(db, target, staged.generation, key, value);
      db.prepare("DELETE FROM scan WHERE family=? AND generation=? AND key=?").run(target, staged.generation, key);
      db.prepare("UPDATE family SET stamp=? WHERE name=?").run(stamp(target), target);
    });
  } catch (error) {
    // The index can never turn an already persisted effect into a failed write.
    // If locking failed before the file change, the stale stamp detects the fallback write.
    if (!began) change();
    else if (!changed) throw error;
  }
}

type Address = { key: string; handle: string };
type PageScope = { setup: string; session: string; author: string; listingAuthor?: string; after: string; issuers: readonly string[] };
const byHandle = (a: Address, b: Address) => a.handle < b.handle ? -1 : a.handle > b.handle ? 1 : 0;

export function receiptPageKeys(scope: PageScope) {
  const selected: IndexedFamily[] = scope.listingAuthor ? ["submissions", "outbox", "message-receipts"] : ["submissions"];
  return indexed(db => {
    const before = refresh(db, selected);
    const read = db.prepare(`
      SELECT key, handle FROM entry WHERE family=? AND generation=? AND setup=? AND topic=? AND author=? AND issuer=? AND handle>?
      ORDER BY handle, key LIMIT ?`);
    const page = (family: IndexedFamily, author: string, issuers: readonly string[]) => {
      const generation = queryGeneration(db, family, before[selected.indexOf(family)]!);
      if (!generation) return [];
      const rows = issuers.flatMap(issuer => read.all(family, generation, scope.setup, "session:" + scope.session,
        author, issuer, scope.after, RECEIPT_PAGE + 1) as Address[]);
      return rows.sort(byHandle).slice(0, RECEIPT_PAGE + 1);
    };
    const listing = scope.listingAuthor
      ? [...page("outbox", scope.listingAuthor, [""]), ...page("message-receipts", scope.listingAuthor, [""])].sort(byHandle) : [];
    return { owner: page("submissions", scope.author, scope.issuers),
      listing: [...new Map(listing.map(row => [row.key, row])).values()].slice(0, RECEIPT_PAGE + 1),
      complete: complete(db, selected, before) };
  });
}

export function moveReceiptKeys(setup: string, sessions: readonly string[], items: ReadonlySet<string>, group?: string) {
  const selected: IndexedFamily[] = sessions.length ? ["mutations", "submissions"] : ["mutations"];
  const result = indexed(db => {
    const before = refresh(db, selected);
    if (!complete(db, selected, before)) return;
    const read = db.prepare("SELECT key, handle FROM entry WHERE family=? AND generation=? AND setup=? AND topic=? ORDER BY key");
    const sessionsOnce = [...new Set(sessions)];
    const topics = sessionsOnce.map(session => "session:" + session);
    const moved = [...topics, ...Array.from(items, item => "item:" + item), ...(group ? ["group:" + group] : [])];
    return { mutations: moved.flatMap(topic => (read.all("mutations", queryGeneration(db, "mutations", before[0]!), setup, topic) as Address[]).map(row => row.key)),
      submissions: sessionsOnce.flatMap(session => (read.all("submissions", queryGeneration(db, "submissions", before[1]!), setup, "session:" + session) as Address[]).map(row => ({ ...row, session }))) };
  });
  if (!result) throw new Error("Receipt evidence is still being indexed or cannot be read. Finish indexing or update the Box before moving this work.");
  return result;
}

async function catalogue(family: IndexedFamily) {
  const at = stamp(family);
  const staged = indexed(db => prepare(db, family, at) ?? known(db, family)!);
  if (staged.catalogued) return;
  let batch: string[] = [];
  const insert = (keys: string[], done = false) => indexed(db => {
    const current = known(db, family);
    if (at !== stamp(family) || current?.generation !== staged.generation || current.catalogued) return false;
    remember(db, family, staged.generation, keys);
    if (done) publish(db, family, staged.generation);
    return true;
  });
  const source = await fs.promises.opendir(path.join(root, family));
  for await (const entry of source) {
    if (validKey.test(entry.name)) batch.push(entry.name.slice(0, -5));
    if (batch.length < RECEIPT_INDEX_BATCH) continue;
    if (!insert(batch)) return;
    batch = [];
    await nextTurn();
  }
  insert(batch, true);
}

function dropRetired(db: DatabaseSync) {
  const old = db.prepare("SELECT family, generation FROM retired LIMIT 1").get() as { family: string; generation: string } | undefined;
  if (!old) return;
  for (const table of ["entry", "scan"]) {
    db.prepare(`DELETE FROM ${table} WHERE rowid IN (SELECT rowid FROM ${table} WHERE family=? AND generation=? LIMIT ?)`)
      .run(old.family, old.generation, RECEIPT_INDEX_BATCH);
    if (db.prepare(`SELECT 1 FROM ${table} WHERE family=? AND generation=? LIMIT 1`).get(old.family, old.generation)) return;
  }
  db.prepare("DELETE FROM retired WHERE family=? AND generation=?").run(old.family, old.generation);
}

let rebuilding: Promise<void> | undefined;

/** Keeper maintenance discovers large legacy catalogues in yielding batches. A killed
 * reader can re-enumerate the names safely. Only a full catalogue can publish addresses. */
export function refreshReceiptIndex(): Promise<void> {
  if (!fs.existsSync(file)) return Promise.resolve();
  return rebuilding ??= (async () => {
    for (const family of families) await catalogue(family);
    indexed(db => {
      refresh(db, families);
      dropRetired(db);
    });
  })().finally(() => { rebuilding = undefined; });
}
