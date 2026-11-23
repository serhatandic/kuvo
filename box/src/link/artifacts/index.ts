// A disposable index of the artifact journal. Bodies and old versions stay on disk.
import type { Database as SQLite } from "bun:sqlite"
import { closeSync, openSync, readdirSync, statSync } from "node:fs"
import { join } from "node:path"
import type { Artifact, Scope } from "./store"

const Database = typeof Bun === "undefined" ? undefined : (await import("bun:sqlite")).Database

export class ArtifactIndex {
  private db: SQLite
  constructor(private readonly log: string, file: string) {
    closeSync(openSync(file, "a", 0o600))
    this.db = new Database!(file)
    this.db.exec(`
      PRAGMA busy_timeout = 3000;
      PRAGMA synchronous = NORMAL;
      PRAGMA cache_size = -1024;
      PRAGMA temp_store = FILE;
      CREATE TABLE IF NOT EXISTS state (stamp TEXT NOT NULL, lines INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS artifact (id TEXT PRIMARY KEY, pinned INTEGER NOT NULL, updated TEXT NOT NULL, body TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS artifact_order ON artifact(pinned DESC, updated DESC, id DESC);
      CREATE TABLE IF NOT EXISTS scope (kind TEXT, name TEXT, id TEXT, PRIMARY KEY (kind, name, id)) WITHOUT ROWID;
    `)
  }

  close() { this.db.close() }

  update(work: () => void) { this.db.transaction(work).immediate() }

  get(id: string): Artifact | undefined {
    const row = this.db.query<{ body: string }, [string]>("SELECT body FROM artifact WHERE id = ?").get(id)
    return row ? JSON.parse(row.body) : undefined
  }

  put(artifact: Artifact) {
    this.db.query("INSERT OR REPLACE INTO artifact VALUES (?, ?, ?, ?)").run(artifact.id, +Boolean(artifact.pinned), artifact.updated, JSON.stringify(artifact))
    const add = this.db.query("INSERT OR IGNORE INTO scope VALUES (?, ?, ?)")
    for (const session of artifact.where.sessions) add.run("session", session, artifact.id)
    for (const bot of artifact.where.bots) add.run("bot", bot, artifact.id)
  }

  drop(id: string) {
    this.db.query("DELETE FROM artifact WHERE id = ?").run(id)
    this.db.query("DELETE FROM scope WHERE id = ?").run(id)
  }

  *values(scope: Scope = {}, ordered = false): IterableIterator<Artifact> {
    const sql = ordered ? " ORDER BY pinned DESC, updated DESC, artifact.id DESC" : ""
    const args = scope.session ? ["session", scope.session] : scope.bot ? ["bot", scope.bot] : []
    const where = args.length ? " WHERE artifact.id IN (SELECT id FROM scope WHERE kind = ? AND name = ?)" : ""
    const statement = this.db.prepare<{ body: string }, string[]>("SELECT body FROM artifact" + where + sql)
    try {
      for (const { body } of statement.iterate(...args)) yield JSON.parse(body)
    } finally { statement.finalize() }
  }

  page(after: string) {
    return this.db.query<{ id: string; body: string }, [string]>(
      "SELECT id, body FROM artifact WHERE id > ? ORDER BY id LIMIT 64"
    ).all(after)
  }

  paths(after: string) {
    return this.db.query<{ id: string; path: string }, [string]>(
      "SELECT id, json_extract(body, '$.path') AS path FROM artifact WHERE id > ? ORDER BY id LIMIT 64"
    ).all(after)
  }

  months() {
    return readdirSync(this.log).filter(name => name.endsWith(".jsonl")).sort()
  }

  private stamp() {
    return JSON.stringify(this.months().map(name => {
      const { ino, size, mtimeNs } = statSync(join(this.log, name), { bigint: true })
      return [name, String(ino), String(size), String(mtimeNs)]
    }))
  }

  *load(replay: () => Generator<void, number>): Generator<void, number> {
    const stamp = this.stamp()
    const previous = this.db.query<{ stamp: string; lines: number }, []>("SELECT stamp, lines FROM state").get()
    if (previous?.stamp === stamp) return previous.lines
    this.db.exec("BEGIN IMMEDIATE")
    try {
      this.db.exec("DELETE FROM state; DELETE FROM scope; DELETE FROM artifact")
      const lines = yield* replay()
      this.db.query("INSERT INTO state VALUES (?, ?)").run(stamp, lines)
      this.db.exec("COMMIT")
      return lines
    } finally {
      if (this.db.inTransaction) this.db.exec("ROLLBACK")
    }
  }

  /** The journal is committed first. After an interruption an old stamp always forces replay. */
  appended(artifact: Artifact) {
    const stamp = this.stamp()
    this.db.transaction(() => {
      this.put(artifact)
      this.db.query("UPDATE state SET stamp = ?, lines = lines + 1").run(stamp)
    }).immediate()
  }

  compacted(lines: number) {
    this.db.query("UPDATE state SET stamp = ?, lines = ?").run(this.stamp(), lines)
  }
}
