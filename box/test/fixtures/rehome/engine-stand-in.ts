// The engine's HTTP API for a move's check, as `opencode serve --stdio --port 0` answers it, read
// from the database it's started on (`$XDG_DATA_HOME/opencode/opencode.db`): its sessions' list
// and each session. It says where it listens as its first line and stops when its stdin closes.
// The move's tests start it in the engine's place; one test starts the real engine instead.
//
// Started on a database as Kuvo Code 0.7.2's engine (2.0.32) left it, it brings it up to date as
// the pinned engine does as it starts: here, by what the real one made of the same database,
// recorded (`engine-2.0.32-migrated.sql`, made by `make-engine-fixture.ts`), laid out where the
// test's world is (`STAND_IN_WORLD`, a file its launcher names: `{S, home, rootCommit}`). One
// test runs the real engine on it instead, and holds the recording to what that makes.
import { Database } from "bun:sqlite"
import { createHash } from "node:crypto"
import { readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

const file = `${process.env.XDG_DATA_HOME}/opencode/opencode.db`
{
  const before = new Database(file)
  const mark = before.query("SELECT count(*) || ' ' || max(id) AS mark FROM migration").get() as { mark: string }
  before.close()
  if (mark.mark === "50 20261010180456_project_time_archived") {
    const { S, home, rootCommit, cannotMigrate } = JSON.parse(readFileSync(process.env.STAND_IN_WORLD!, "utf8")) as { S: string; home: string; rootCommit: string; cannotMigrate?: boolean }
    // (One that can't, for a test: it stops as it starts, as an engine that failed to migrate would.)
    if (cannotMigrate) { console.error("migration failed"); process.exit(1) }
    const sql = readFileSync(join(import.meta.dir, "engine-2.0.32-migrated.sql"), "utf8")
      .replaceAll("@S_URL@", pathToFileURL(S).href.slice("file://".length)).replaceAll("@S@", S).replaceAll("@HOME@", home)
      .replaceAll("@ROOT6@", rootCommit.slice(0, 6)).replaceAll("@ROOT@", rootCommit)
      .replace(/@ID:([^@]+)@/g, (_, folder: string) => createHash("sha1").update(`directory:${folder}`).digest("hex"))
    for (const suffix of ["", "-wal", "-shm"]) rmSync(file + suffix, { force: true })
    const made = new Database(file)
    made.exec(sql)
    made.close()
  }
}
const db = new Database(file, { readonly: true })
const authorization = "Basic " + Buffer.from(`opencode:${process.env.OPENCODE_PASSWORD}`).toString("base64")
const session = (row: any) => ({ id: row.id, projectID: row.project_id, location: { directory: row.directory } })
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    if (request.headers.get("authorization") !== authorization) return new Response("", { status: 401 })
    const url = new URL(request.url)
    if (url.pathname === "/api/session") {
      const archived = url.searchParams.get("archived") === "true"
      const rows = db.query(`SELECT * FROM session_v2 WHERE time_archived IS ${archived ? "NOT " : ""}NULL ORDER BY time_updated DESC`).all()
      // (One that's lost a session, for a test of the check: as told by its launcher.)
      if (process.env.STAND_IN_LOSES_ONE && !archived) rows.pop()
      return Response.json({ data: rows.map(session), cursor: { previous: null, next: null } })
    }
    const one = /^\/api\/session\/([^/]+)$/.exec(url.pathname)
    if (one) {
      const row = db.query("SELECT * FROM session_v2 WHERE id = ?").get(decodeURIComponent(one[1]))
      return row ? Response.json({ data: session(row) }) : new Response("", { status: 404 })
    }
    return new Response("", { status: 404 })
  },
})
console.log(JSON.stringify({ url: `http://127.0.0.1:${server.port}` }))
process.stdin.on("end", () => process.exit(0))
process.stdin.resume()
