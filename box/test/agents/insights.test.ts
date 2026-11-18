import "../safety"
import { afterAll, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import type { Agent } from "../../src/agents/agent"
import { LinkDirectory, PeerPost } from "../../src/agents/groups/post"
import { Store } from "../../src/agents/store"
import { Insights, insightsRoutes } from "../../src/agents/usage/insights"
import { dayBounds, InsightsDB } from "../../src/agents/usage/insights-db"
import { InsightsScan } from "../../src/agents/usage/insights-scan"
import { insightKey, type InsightRecord } from "../../src/agents/usage/insight-records"
import { generationEvent } from "../../src/agents/usage/antigravity-history"
import { Catalog, Pricing, PricingStore, supplementFrom } from "../../src/agents/usage/pricing"
import type { Message } from "../../src/agents/wire"

const dir = mkdtempSync(join(tmpdir(), "kuvo-insights-"))
afterAll(() => rmSync(dir, { recursive: true, force: true }))
const now = Date.parse("2026-11-18T09:00:00Z")
const day = 86_400_000
const tokens = { input: 100, output: 50, cacheRead: 500, cacheWrite5m: 2, cacheWrite1h: 0 }
const catalog = (input = 1) => new Pricing(supplementFrom({ pricing: {}, alias_rules: [] }), new Catalog(new Map([
  ["claude-test", { input, output: 1, cacheWrite: 1, cacheRead: 0.1, cacheReadIsExplicit: true, fast: 1, longContextThreshold: 200_000 }],
])), new Catalog())
const priced = catalog()
const event = (call: string, computer = "first", patch: Partial<InsightRecord> = {}): InsightRecord => ({
  key: insightKey("claude", "session", call), at: now - 300, session: "session", provider: "claude",
  account: "claude#same-account", attributed: true, model: "claude-test", call, computer, tokens, ...patch,
})
const show = (db: InsightsDB, filter = {}, computers = ["first", "second"], period = 30) => {
  return db.view(now, "America/New_York", period, filter, computers, "overview", 0)
}

test("copies merge before filters, resumed sessions count once, and moved bot calls retain their origin", () => {
  const db = new InsightsDB(":memory:")
  const one = event("previous", "first", { bot: "bot_demo", botName: "Demo", skills: ["research", "writing"] })
  const two = event("new", "second", { bot: "bot_demo", botName: "Demo" })
  const another = event("third", "second", { session: "another", model: "unpriced-model" })
  db.take([one, two, another, event("previous", "second", { attributed: undefined }), event("unknown", "second", { tokens: undefined })], priced, now)
  db.take([one, another], priced, now, "second", 100)
  expect(db.read<number>("cursor/second")).toBe(100)
  expect(show(db).totals).toMatchObject({ tokens: 456, processed: 1956, sessions: 2, unpriced: 2, partial: 1 })
  expect(show(db, { bot: "bot_demo" }).breakdowns.skill.map((row) => row.costShare)).toEqual([0.5, 0.5])
  expect(show(db, { bot: "bot_demo" }, ["second"]).totals).toMatchObject({ tokens: 152, sessions: 1 })
  expect(show(db, { account: "claude#same-account" }).totals?.tokens).toBe(456)
  const calls = db.view(now, "America/New_York", 30, { session: "session" }, ["first", "second"], "detail", 0).calls
  expect(calls?.map((call) => call.computer).sort()).toEqual(["first", "second", "second"])
  const cost = show(db).totals!.cost!
  db.price(catalog(10), now)
  expect(show(db).totals?.cost).toBeGreaterThan(cost)
  expect(show(db).totals?.tokens).toBe(456)
  db.close()
})

test("a newly available provider call replaces its saved placeholder across filters", () => {
  const db = new InsightsDB(":memory:")
  const saved = event("missing", "first", { tokens: undefined })
  db.take([saved], priced, now)
  expect(show(db).totals?.partial).toBe(1)
  db.take([event("native", "first", { supersedes: saved.key })], priced, now)
  expect(show(db).totals).toMatchObject({ sessions: 1, tokens: 152, partial: 0, unpriced: 0 })
  db.close()
})

test("day bounds use the viewer's time zone, including DST transitions", () => {
  const at = Date.parse("2026-03-09T12:00:00Z")
  const bounds = dayBounds(at, "America/New_York", 7)
  expect(bounds.at(-2)).toEqual(["2026-03-08", Date.parse("2026-03-08T05:00:00Z"), Date.parse("2026-03-09T04:00:00Z")])
  const midnight = Date.parse("2026-11-18T05:00:00Z")
  const db = new InsightsDB(":memory:")
  db.take([event("yesterday", "first", { at: midnight - 1 }), event("today", "first", { at: midnight })], priced, now)
  expect(show(db).daily?.slice(-2).map(({ tokens }) => tokens)).toEqual([152, 152])
  expect(db.view(now, "Asia/Kathmandu", 7, {}, ["first"], "overview", 0).daily?.at(-1)?.tokens).toBe(304)
  db.close()
})

test("a year of daily readings uses indexed ranges and reuses unchanged days", () => {
  const db = new InsightsDB(":memory:")
  db.take(Array.from({ length: 40_000 }, (_, i) => event(String(i), "first", {
    session: "session-" + i % 177, at: now - 2_000 - i % 365 * day,
  })), priced, now)
  const begin = performance.now()
  const year = show(db, {}, ["first"], 365)
  expect(year.daily?.length).toBe(365)
  expect(year.totals).toMatchObject({ sessions: 177, tokens: 40_000 * 152 })
  expect(performance.now() - begin).toBeLessThan(8000)
  const warm = performance.now()
  expect(show(db, {}, ["first"], 365).daily).toEqual(year.daily!)
  expect(performance.now() - warm).toBeLessThan(500)
  db.close()
}, 60_000)

test("resume peer pages, reject vouched computers, and price incoming records locally", () => {
  const db = new InsightsDB(":memory:")
  const post = new PeerPost()
  const directory = new LinkDirectory(post)
  const mail = new Insights(db, {} as InsightsScan, new PricingStore(undefined, null), directory, post, "first", () => now)
  directory.set({ self: { key: "first", name: "First" }, boxes: [{ key: "second", name: "Second" }], also: [{ key: "shared", name: "Shared" }], bots: [] })
  const page = { kind: "insights" as const, action: "page" as const, after: 0, next: 100, more: true, epoch: "first-generation", scanned: now, records: [event("remote", "second")] }
  mail.take("shared", page)
  expect(db.read("cursor/shared")).toBeUndefined()
  mail.take("second", page)
  mail.take("second", page)
  mail.take("second", { ...page, after: 100, next: 101, more: false, records: [event("next", "second")] })
  expect(db.read<number>("cursor/second")).toBe(101)
  mail.take("second", { ...page, after: 101, next: 2, more: false, epoch: "new-generation", reset: true, records: [event("next", "second")] })
  expect(db.read<number>("cursor/second")).toBe(2)
  expect(db.read<string>("peer-epoch/second")).toBe("new-generation")
  expect(show(db, {}, ["second"]).totals).toMatchObject({ sessions: 1, calls: 2, tokens: 304 })
  const asked = new URLSearchParams("period=7&zone=UTC")
  expect(insightsRoutes(mail, "GET", [], asked).status).toBe(200)
  expect(insightsRoutes(mail, "GET", ["sessions", "session"], asked).status).toBe(200)
  expect(insightsRoutes(mail, "GET", [], new URLSearchParams("period=10")).status).toBe(400)
  db.close()
})

const writeLog = (path: string, ...rows: object[]) => {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, rows.map((row) => JSON.stringify(row)).join("\n") + "\n")
}

test("isolated Claude, Codex, Grok and Antigravity records join Kuvo steps, and the Kuvo engine uses its SQLite reader", async () => {
  const home = join(dir, "isolated")
  const store = new Store(join(home, "saved"))
  const agents: Agent[] = []
  const assistant = (provider: string, model: string, at: number): Message => ({
    type: "assistant", id: "message", time: { created: at - 1500, completed: at + 500, origin: "first" },
    model: { providerID: provider, id: model }, agent: "build",
    content: [{ type: "tool", id: "read", name: "Read", state: { status: "completed", input: { path: ".agents/skills/fixture/SKILL.md" }, content: [] } }],
  })
  const saved = (provider: string, native: string, field: string, at: number) => {
    const id = "kuvo-" + provider
    const step = assistant(provider, "session-model", at)
    store.addWhole({
      session: { id, projectID: "p", title: id, cost: 999, time: { created: at - 3000, updated: at + 500 }, location: { directory: home },
        tokens: { input: 999, output: 999, reasoning: 0, cache: { read: 0, write: 0 } } },
      messages: [step],
      agent: { threads: { [provider]: { state: { [field]: native } } } },
    })
    return step
  }
  const agent = (providerID: string, instanceHome: string | undefined, roots: string[], extra = {}) =>
    agents.push({ providerID, instance: { id: "default", home: instanceHome }, conversationRoots: () => roots, ...extra } as unknown as Agent)
  const at = now - 10_000
  const timestamp = new Date(at).toISOString()
  const claude = join(home, "claude", "home", "projects")
  agent("claude-code", dirname(claude), [claude, join(claude, "shared")])
  saved("claude-code", "native-claude", "claudeSession", at)
  const claudeEntry = { timestamp, sessionId: "native-claude", requestId: "req", message: { model: "claude-test", id: "one", usage: { input_tokens: 100, output_tokens: 50 } } }
  writeLog(join(claude, "project", "call.jsonl"), claudeEntry, claudeEntry, { ...claudeEntry, sessionId: "outside", message: { ...claudeEntry.message, id: "outside" } })
  const external = join(dir, "outside", "projects")
  writeLog(join(external, "external.jsonl"), { ...claudeEntry, message: { ...claudeEntry.message, id: "outside" } })
  symlinkSync(external, join(claude, "shared"), "dir")
  const codex = join(home, "codex", "home", "sessions")
  agent("codex-cli", dirname(codex), [codex])
  saved("codex-cli", "native-codex", "codexThread", at)
  const total = { input_tokens: 100, cached_input_tokens: 20, output_tokens: 25, total_tokens: 125 }
  const count = { timestamp, type: "event_msg", payload: { type: "token_count", info: { total_token_usage: total } } }
  writeLog(join(codex, "rollout.jsonl"), { timestamp, type: "session_meta", payload: { id: "native-codex" } },
    { timestamp, type: "turn_context", payload: { model: "gpt-5" } }, count, count)
  const grok = join(home, "grok", "home", "sessions")
  agent("grok-cli", dirname(grok), [grok])
  saved("grok-cli", "native-grok", "acpSession", at)
  writeLog(join(grok, "native-grok", "updates.jsonl"),
    { params: { sessionId: "native-grok", _meta: { agentTimestampMs: at, eventId: "grok-event" },
      update: { sessionUpdate: "turn_completed", usage: { modelUsage: { "grok-test": { inputTokens: 20, outputTokens: 5 } } } } } })
  const profile = join(home, "antigravity", "profile")
  const agPath = join(profile, "antigravity", "conversations", "native-ag.db")
  const { cases } = JSON.parse(readFileSync(join(import.meta.dir, "../fixtures/usage/antigravity-blobs.json"), "utf8"))
  const fixture = Buffer.from(cases[0].hex, "hex")
  const agAt = generationEvent(fixture)!.timestampSeconds * 1000
  agent("antigravity", undefined, [], { profile })
  saved("antigravity", "native-ag", "acpSession", agAt)
  mkdirSync(dirname(agPath), { recursive: true })
  const sql = new Database(agPath)
  sql.exec("CREATE TABLE gen_metadata(idx INTEGER PRIMARY KEY, data BLOB)")
  sql.query("INSERT INTO gen_metadata VALUES (1, ?)").run(fixture)
  sql.close()
  const cursor = saved("cursor", "sdk-context", "cursorAgent", at)
  if (cursor.type === "assistant") cursor.tokens = { input: 100_000, output: 500, reasoning: 30, cache: { read: 50_000, write: 10_000 } }
  const engine = join(home, "engine")
  mkdirSync(engine)
  const db = new Database(join(engine, "opencode.db"))
  db.exec(readFileSync(join(import.meta.dir, "../fixtures/usage/opencode-hosted.sql"), "utf8"))
  db.close()
  const scanner = new InsightsScan({ root: home, cache: join(home, "cache"), agents: () => agents, store, bots: () => [], computer: () => "first", account: () => "account", engine })
  const { records, issues } = await scanner.scan(priced, now - 365 * day)
  expect(issues).toEqual([])
  for (const provider of ["claude", "codex", "grok", "antigravity"]) {
    const found = records.filter((record) => record.provider === provider)
    expect(found, provider).toHaveLength(1)
    expect(found[0]?.tokens, provider).toBeDefined()
    expect(found[0]?.skills, provider).toEqual(["fixture"])
  }
  expect(records.find((r) => r.provider === "cursor")).toMatchObject({ partial: true })
  expect(records.find((r) => r.provider === "cursor")?.tokens).toBeUndefined()
  expect(records.filter((r) => r.provider === "opencode-go").some((r) => r.human)).toBe(true)
  expect(records.find((r) => r.call === "m-go-2")?.reportedTotal).toBe(750)
  expect(records.find((r) => r.call === "m-go-4")).toBeUndefined()
})
