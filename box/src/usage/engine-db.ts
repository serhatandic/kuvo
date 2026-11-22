// The engine's databases on this computer (`opencode*.db` in its data folder, one for each release
// channel), only ever read: listed, stamped (to read them again only when one changed), and read
// in a worker of its own (`bun:sqlite`, read-only), so the bridge never waits on SQLite. What each
// usage source asks of them is its own SQL (engine-hosted.ts).

import { readdirSync, realpathSync, statSync, type Dirent } from "node:fs"
import { isAbsolute, join, relative, sep } from "node:path"

/**
 * Every channel's database in the engine's data folder, path-sorted; none when the folder isn't
 * there (no engine has run). A folder that is there but can't be listed throws: not known now.
 */
export function engineDatabases(folder: string): string[] {
  let files: Dirent[]
  let root: string
  try {
    root = realpathSync(folder)
    files = readdirSync(root, { withFileTypes: true })
  } catch (error) {
    if ((error as NodeJS.ErrnoException)?.code === "ENOENT") return []
    throw error
  }
  const paths = new Set<string>()
  for (const file of files) {
    if (!file.name.startsWith("opencode") || !file.name.endsWith(".db")) continue
    const path = join(root, file.name)
    // Plain entries already live under the resolved root. Keep unreadable files in
    // the list so the worker can report their errors without losing readable channels.
    if (file.isFile()) paths.add(path)
    if (!file.isSymbolicLink()) continue
    try {
      const resolved = realpathSync(path)
      const inside = relative(root, resolved)
      if (inside && inside !== ".." && !inside.startsWith(".." + sep) && !isAbsolute(inside)) paths.add(resolved)
    } catch (error) {
      if ((error as NodeJS.ErrnoException)?.code !== "ENOENT") throw error
    }
  }
  return [...paths].sort()
}

/** What the databases are now (each with its write-ahead log), to read them again only when one changed; throws as `engineDatabases` does. */
export function databasesStamp(folder: string): string {
  const parts: string[] = []
  for (const path of engineDatabases(folder).flatMap((db) => [db, db + "-wal"])) {
    try {
      const info = statSync(path)
      parts.push(`${path}:${info.size}:${info.mtimeMs}`)
    } catch {
      parts.push(`${path}:-`)
    }
  }
  return parts.join("\n")
}

/** What's asked of each database: which message tables it has, then the query for those it has. */
export interface EngineSQL {
  tables: string
  v1: string
  v2: string
  both: string
  /** The Insights query also uses session and part tables where present. */
  usage?: true
  after?: Record<string, EngineWatermarks>
}

/** What one database answered: its JSON, or why it couldn't be read. One without message tables answers nothing. */
export type EngineWatermarks = { message?: number; session_message?: number }
export type EngineAnswer = { path: string; watermarks?: EngineWatermarks; json?: string; sessions?: [id: string, parent: string | null, metadata: string | null][]; failed?: string }

/**
 * The worker: each database read-only. A database in WAL mode whose side files are gone (Linux's
 * SQLite removes them as the engine's last connection closes: a channel not running) can't be
 * opened read-only, so one with no write-ahead log to lose is read as it is (immutable). A database
 * without either message table isn't counted, read or failed.
 */
const READER = `
import { Database } from "bun:sqlite"
import { realpathSync, statSync } from "node:fs"
import { isAbsolute, relative, sep } from "node:path"
function owned(root, path) {
  const inside = relative(root, path)
  return inside && inside !== ".." && !inside.startsWith(".." + sep) && !isAbsolute(inside)
}
// (A database the engine is recovering or checkpointing is waited for a little, from the first query on.)
function opened(db) {
  db.exec("PRAGMA busy_timeout = 2000")
  return db
}
function open(path) {
  let db
  try {
    db = opened(new Database(path, { readonly: true }))
    db.query("SELECT count(*) FROM sqlite_master").values()
    return db
  } catch (error) {
    db?.close()
    let wal = 0
    try {
      wal = statSync(path + "-wal").size
    } catch {}
    if (wal > 0 || !/unable to open/i.test(error instanceof Error ? error.message : String(error))) throw error
    return opened(new Database("file:" + path.replace(/[%?#]/g, (character) => "%" + character.charCodeAt(0).toString(16)) + "?immutable=1", { readonly: true }))
  }
}
let resume
function page(value) {
  return new Promise(resolve => {
    resume = resolve
    postMessage({ page: value })
  })
}
self.onmessage = async (event) => {
  if (event.data === "next") { resume?.(); return }
  const { paths, root, sql } = event.data
  const answers = []
  for (const path of paths) {
    let db
    try {
      const resolved = realpathSync(path)
      if (!owned(root, resolved)) continue
      for (const suffix of ["-wal", "-shm", "-journal"]) {
        try {
          if (!owned(root, realpathSync(resolved + suffix))) throw new Error("The engine database has a side file outside its data folder.")
        } catch (error) {
          if (error?.code !== "ENOENT") throw error
        }
      }
      db = open(resolved)
      const names = String(db.query(sql.tables).values()[0]?.[0] ?? "").split(",").map((name) => name.trim())
      const v1 = names.includes("message")
      const v2 = names.includes("session_message")
      if (!v1 && !v2) continue
      let query = v1 && v2 ? sql.both : v1 ? sql.v1 : sql.v2
      const sessions = []
      const watermarks = {}
      if (sql.usage) {
        db.exec("BEGIN")
        for (const table of ["message", "session_message"]) {
          const columns = db.query("SELECT name FROM pragma_table_info(?)").values(table).flat()
          const updated = columns.includes("time_updated")
          if (updated) {
            const last = db.query("SELECT MAX(time_updated) FROM " + table).values()[0]?.[0]
            if (Number.isSafeInteger(last)) watermarks[table] = last
          }
          const after = sql.after?.[path]?.[table]
          // Include the boundary and an overlap for equal timestamps and recent in-place edits.
          const changed = updated && Number.isSafeInteger(after) && watermarks[table] >= after
            ? "(time_updated IS NULL OR time_updated >= " + (after - 3600000) + ")" : "1"
          query = query.replaceAll("@" + table + "_changed@", changed)
        }
        for (const [table, message] of [["session", "message"], ["session_v2", "session_message"]]) {
          const columns = db.query("SELECT name FROM pragma_table_info(?)").values(table).flat()
          const fact = (column) => columns.includes(column) ? "(SELECT " + column + " FROM " + table + " WHERE id = " + message + ".session_id)" : "NULL"
          query = query.replaceAll("@" + table + "_title@", fact("title"))
          if (columns.includes("id")) {
            const fields = ["parent_id", "metadata"].map((column) => columns.includes(column) ? column : "NULL")
            for (const session of db.query("SELECT id, " + fields.join(", ") + " FROM " + table).values()) sessions.push(session)
          }
        }
        const legacyParts = db.query("SELECT 1 FROM pragma_table_info('part') WHERE name='data'").get()
        query = query.replaceAll("@legacy_parts@", legacyParts ? "(SELECT json_group_array(json(p.data)) FROM part p WHERE p.message_id=message.id AND json_valid(p.data) AND json_extract(p.data, '$.type')='tool')" : "NULL")
      }
      if (sql.usage) {
        await page({ path, sessions, watermarks })
        let rows = [], bytes = 0
        for (const { row } of db.query(query).iterate()) {
          rows.push(row)
          bytes += row.length
          if (rows.length < 512 && bytes < 1024 * 1024) continue
          await page({ path, json: "[" + rows.join(",") + "]" })
          rows = []
          bytes = 0
        }
        if (rows.length) await page({ path, json: "[" + rows.join(",") + "]" })
      } else {
        const json = db.query(query).values()[0]?.[0]
        answers.push({ path, ...(typeof json === "string" ? { json } : {}) })
      }
    } catch (error) {
      answers.push({ path, failed: error instanceof Error ? error.message : String(error) })
    } finally {
      db?.close()
    }
  }
  postMessage(answers)
}
`
let readerURL: string | undefined

/** How long a read may take before it's given up (and tried again next round). */
const READ_FOR = 120_000

/** The tables query every reader starts with. */
export const TABLES_SQL = "SELECT group_concat(name) FROM sqlite_master WHERE type='table' AND name IN ('message','session_message');"

/**
 * What each database in `folder` with message tables answers `sql`, by path; undefined when
 * there's no database there. Fails when the folder can't be listed.
 */
export function readEngineDatabases(folder: string, sql: EngineSQL, selected?: ReadonlySet<string>, onPage?: (page: EngineAnswer) => void): Promise<EngineAnswer[] | undefined> {
  let paths: string[]
  let root: string
  try {
    paths = engineDatabases(folder).filter(path => !selected || selected.has(path))
    if (!paths.length) return Promise.resolve(undefined)
    root = realpathSync(folder)
  } catch (error) {
    return Promise.reject(error)
  }
  readerURL ??= URL.createObjectURL(new Blob([READER], { type: "application/javascript" }))
  const worker = new Worker(readerURL)
  // (It never keeps the bridge from ending.)
  worker.unref?.()
  let timer: ReturnType<typeof setTimeout> | undefined
  return new Promise<EngineAnswer[]>((resolve, reject) => {
    timer = setTimeout(() => reject(new Error("The engine's database took too long to read.")), READ_FOR)
    timer.unref?.()
    worker.onmessage = (event: MessageEvent<EngineAnswer[] | { page: EngineAnswer }>) => {
      if (Array.isArray(event.data)) { resolve(event.data); return }
      try {
        onPage?.(event.data.page)
        worker.postMessage("next")
      } catch (error) { reject(error) }
    }
    worker.onerror = (event) => reject(new Error(event.message))
    worker.postMessage({ paths, root, sql })
  }).finally(() => {
    clearTimeout(timer)
    worker.terminate()
  })
}

/** Usage metadata from the Kuvo engine only. Use the same read-only worker as its plan summaries. */
export function engineUsageAnswers(folder: string, since: number, after: Record<string, EngineWatermarks> | undefined, selected: ReadonlySet<string> | undefined, onPage: (page: EngineAnswer) => void) {
  const rows = (table: string, role: string, sessionTable: string, parts: string) => `
    SELECT json_array(session_id, id, ${role}, time_created,
      json_extract(data, '$.time.completed'),
      COALESCE(json_extract(data, '$.model.providerID'), json_extract(data, '$.providerID')),
      COALESCE(json_extract(data, '$.model.id'), json_extract(data, '$.modelID')),
      json_extract(data, '$.agent'), json_extract(data, '$.tokens'),
      json_extract(data, '$.metadata.source'),
      json_extract(data, '$.time.origin'), json_extract(data, '$.time.usageAccount'),
      @${sessionTable}_title@,
      (SELECT json_group_array(json_array(COALESCE(json_extract(part.value,'$.name'),json_extract(part.value,'$.tool')),
          json_extract(part.value,'$.state.input'))) FROM json_each(${parts}) AS part
        WHERE json_extract(part.value,'$.type') = 'tool' AND json_extract(part.value,'$.state.status') = 'completed'
          AND lower(COALESCE(json_extract(part.value,'$.name'),json_extract(part.value,'$.tool'))) IN ('skill','read','read_file'))) AS row
    FROM ${table} WHERE time_created >= ${Math.trunc(since)} AND @${table}_changed@ AND json_valid(data)
      AND (${role} IN ('user','assistant') OR (${role} = 'compaction' AND json_extract(data, '$.status') = 'completed'))`
  const v1 = rows("message", "json_extract(data, '$.role')", "session", "@legacy_parts@")
  const v2 = rows("session_message", "type", "session_v2", "json_extract(data, '$.content')")
  return readEngineDatabases(folder, { tables: TABLES_SQL, v1, v2, both: v1 + " UNION ALL " + v2, usage: true, after }, selected, onPage)
}
