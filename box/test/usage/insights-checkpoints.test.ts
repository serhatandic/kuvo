import "../safety"
import { expect, test } from "bun:test"
import { closeSync, mkdirSync, mkdtempSync, openSync, rmSync, writeFileSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { InsightsDB } from "../../src/usage/insights-db"
import { usageReader } from "../../src/usage/service"
import { bundledPricing } from "../../src/usage/pricing"

// A real Insights worker and index read the same saved files after a restart.
test("Insights checkpoints local history and counts only changed transcripts after restart", async () => {
  const home = mkdtempSync(join(tmpdir(), "insights-index-"))
  const database = join(home, "insights.sqlite")
  const now = Date.UTC(2026, 10, 23, 10)
  const folder = join(home, "sessions", "ses_one")
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "session.json"), JSON.stringify({ id: "ses_one", title: "History", time: { updated: now } }))
  const call = (id: string) => ({
    type: "assistant", id, model: { providerID: "claude-code", id: "claude-test" }, content: [],
    time: { created: now - 10, completed: now }, tokens: { input: 10, output: 1 },
  })
  const save = (...ids: string[]) => writeFileSync(join(folder, "messages.json"), JSON.stringify(ids.map(call)))
  const scan = (db: InsightsDB) => db.scan({ home, cache: join(home, "cache"), bots: [], computer: "box" },
    bundledPricing(), now - 86_400_000, now, { eligible: ["box"], historical: [] })
  let db = new InsightsDB(database)
  try {
    save("first")
    expect(await scan(db)).toMatchObject({ transcripts: 1, observations: 1 })
    await db.close()
    db = new InsightsDB(database)
    expect(await scan(db)).toMatchObject({ transcripts: 0, observations: 0 })
    save("first", "second")
    expect(await scan(db)).toMatchObject({ transcripts: 1, observations: 2 })
    expect((await db.batch(0)).records.map(row => row.call)).toEqual(["first", "second"])
  } finally {
    await db.close()
    rmSync(home, { recursive: true, force: true })
  }
})

test("Insights adds engine updates without replaying the older calls in the same database", async () => {
  const home = mkdtempSync(join(tmpdir(), "insights-engine-"))
  const engine = join(home, "data")
  const now = Date.UTC(2026, 10, 23, 10)
  const hour = 3_600_000
  mkdirSync(engine)
  const { Database } = await import("bun:sqlite")
  const source = new Database(join(engine, "opencode.db"))
  const pricing = bundledPricing()
  let db = new InsightsDB(join(home, "insights.sqlite"))
  const scan = () => db.scan({ home, engine, cache: join(home, "cache"), bots: [], computer: "box" },
    pricing, now - 86_400_000, now, { eligible: ["box"], historical: [] })
  try {
    source.exec("CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)")
    const insert = source.query("INSERT INTO session_message VALUES (?, 'ses_engine', 'assistant', ?, ?, ?)")
    const call = (id: string, at: number, output = 1) => insert.run(id, at, at,
      JSON.stringify({ time: { completed: at }, modelID: "test", tokens: { input: 10, output } }))
    call("old", now - 5 * hour)
    call("recent", now - 2 * hour)
    expect(await scan()).toMatchObject({ observations: 2 })
    await db.close()
    db = new InsightsDB(join(home, "insights.sqlite"))
    expect(await scan()).toMatchObject({ observations: 0 })
    call("new", now - 100)
    expect(await scan()).toMatchObject({ observations: 2 })
    expect((await db.batch(0)).records.map(row => row.call)).toEqual(["old", "recent", "new"])
  } finally {
    await db.close()
    source.close()
    rmSync(home, { recursive: true, force: true })
  }
})

test("learning the box's peer name leaves its unchanged local history checkpoint in place", async () => {
  const home = mkdtempSync(join(tmpdir(), "insights-roster-"))
  const folder = join(home, "sessions", "ses_local")
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "session.json"), JSON.stringify({ id: "ses_local", title: "Local", time: { updated: Number.MAX_SAFE_INTEGER } }))
  writeFileSync(join(folder, "messages.json"), JSON.stringify([{
    type: "user", id: "prompt", text: "Hello", time: { created: 0 },
  }]))
  const run = join(home, "run")
  mkdirSync(run)
  const bundle = join(import.meta.dir, "../../dist/kuvo-link.js")
  const reader = usageReader({ home, run, box: home, bots: join(home, "bots"), bundle })
  const { Database } = await import("bun:sqlite")
  const refresh = () => reader.handle(new Request("http://box/api/kuvo/insights/summary?period=7&zone=UTC&refresh=1"))
  const checkpoint = () => {
    const db = new Database(join(home, "usage", "insights.sqlite"), { readonly: true })
    try {
      return db.query<{ body: string }, []>("SELECT body FROM state WHERE key='scan/transcript-v1/ses_local'").get()?.body
    } finally { db.close() }
  }
  try {
    expect((await refresh()).status).toBe(200)
    const before = checkpoint()
    expect(before).toBeDefined()
    const roster = await reader.handle(new Request("http://box/api/kuvo/peers", {
      method: "PUT", body: JSON.stringify({ self: { key: "box-public-key", name: "Box" }, boxes: [], bots: [] }),
    }))
    expect(roster.status).toBe(200)
    expect((await refresh()).status).toBe(200)
    expect(checkpoint()).toBe(before)
  } finally {
    await reader.closeInsights()
    rmSync(home, { recursive: true, force: true })
  }
})


test("a large backfill keeps all usage as it releases batches and stays finished after a restart", async () => {
  const home = mkdtempSync(join(tmpdir(), "insights-batches-"))
  const now = Date.UTC(2026, 10, 23, 10)
  const file = join(home, "insights.sqlite")
  const args: Parameters<InsightsDB["scan"]> = [
    { home, cache: join(home, "cache"), bots: [], computer: "box" },
    bundledPricing(), now - 86_400_000, now, { eligible: ["box"], historical: [] },
  ]
  const block = "X".repeat(1024 * 1024)
  let db = new InsightsDB(file)
  try {
    for (const [n, providerID] of ["claude-code", "codex", "antigravity", "grok", "cursor"].entries()) {
      const id = "ses_batch_" + n
      const folder = join(home, "sessions", id)
      mkdirSync(folder, { recursive: true })
      writeFileSync(join(folder, "session.json"), JSON.stringify({ id, title: "Batch " + n, time: { updated: now } }))
      const transcript = openSync(join(folder, "messages.json"), "w")
      try {
        const reply = { type: "assistant", id: "call_" + n, model: { id: "test", providerID }, content: [],
          time: { created: now - 10, completed: now }, tokens: { input: n + 10, output: 1 } }
        writeSync(transcript, "[" + JSON.stringify(reply).slice(0, -1) + ',"unused":"')
        for (let i = 0; i < 12; i++) writeSync(transcript, block)
        writeSync(transcript, '"}]')
      } finally { closeSync(transcript) }
    }
    expect(await db.scan(...args)).toMatchObject({ transcripts: 5, observations: 5 })
    expect((await db.batch(0)).records.map(row => row.call).sort()).toEqual(["call_0", "call_1", "call_2", "call_3", "call_4"])
    await db.close()
    db = new InsightsDB(file)
    expect(await db.scan(...args)).toMatchObject({ transcripts: 0, observations: 0 })
    expect((await db.batch(0)).records).toHaveLength(5)
  } finally {
    await db.close()
    rmSync(home, { recursive: true, force: true })
  }
})
