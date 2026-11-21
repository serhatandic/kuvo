import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { LinkDirectory, PeerPost } from "../../src/agents/groups/post"
import type { Record_ } from "../../src/agents/store"
import type { Message } from "../../src/agents/wire"
import { Insights, insightsRoutes } from "../../src/usage/insights"
import { InsightsDB } from "../../src/usage/insights-db"
import { modelName } from "../../src/usage/insights-names"
import { InsightsScan } from "../../src/usage/insights-scan"
import { insightKey, type InsightRecord } from "../../src/usage/insight-records"
import { Catalog, Pricing, PricingStore, supplementFrom } from "../../src/usage/pricing"

const dir = mkdtempSync(join(tmpdir(), "kuvo-insights-summary-"))
afterAll(() => rmSync(dir, { recursive: true, force: true }))
const now = Date.parse("2026-11-18T09:00:00Z")
const day = 86_400_000
const priced = new Pricing(supplementFrom({ pricing: {}, alias_rules: [] }), new Catalog(new Map([
  ["claude-test", { input: 1, output: 1, cacheWrite: 1, cacheRead: 0.1, cacheReadIsExplicit: true, fast: 1, longContextThreshold: 200_000 }],
])), new Catalog())

/** One call: 100 uncached, `cached` cache reads and 2 cache writes in, 50 out. */
const call = (session: string, name: string, patch: Partial<InsightRecord> = {}, cached = 500): InsightRecord => ({
  key: insightKey("test", session, name), at: now - 3_600_000, session, call: name, provider: "claude", computer: "first",
  account: "claude#one", attributed: true, model: "claude-test", title: session,
  tokens: { input: 100, output: 50, cacheRead: cached, cacheWrite5m: 2, cacheWrite1h: 0 }, ...patch,
})
const processed = (cached = 500) => 100 + 50 + cached + 2

type Summary = {
  state: string
  totals: { cost: number; tokens: number; cacheRate: number | null }
  change: { cost: number | null; tokens: number | null; cacheRate: number | null } | null
  trend: { day: string; cost: number; tokens: number }[]
  lists: { key: string; name: string; rows: { kind: string; id: string; name: string; note?: string; provider?: string; look?: unknown; tokens: number; cost: number; cacheRate: number | null; sessions?: { id: string; name: string; note?: string; tokens: number }[] }[] }[]
  standouts: { id: string; name: string; reason: string; kind: string }[]
}

async function summaryOf(records: InsightRecord[], query = "", bots: { id: string; name: string; look?: unknown }[] = []) {
  const db = new InsightsDB(":memory:")
  const post = new PeerPost()
  const insights = new Insights(db, {} as InsightsScan, new PricingStore(undefined, null), new LinkDirectory(post), post, "first", () => now, undefined, () => bots)
  try {
    await db.take(records, priced, now)
    const answer = await insightsRoutes(insights, "GET", ["summary"], new URLSearchParams(`period=30&zone=UTC${query}`))
    expect(answer.status).toBe(200)
    return (await answer.json() as { data: Summary }).data
  } finally {
    await db.close()
  }
}
const list = (summary: Summary, key: string) => summary.lists.find((each) => each.key === key)!

test("a bot's row is its own chat and every session it oversees, subagents and older starts included", async () => {
  const step = (id: string): Message => ({
    id, type: "assistant", model: { providerID: "claude-code", id: "claude-test" }, agent: "build", content: [],
    time: { created: now - 2000, completed: now - 1000, origin: "first" },
    tokens: { input: 100, output: 50, reasoning: 0, cache: { read: 500, write: 2 } },
  } as Message)
  const session = (id: string, extra: { parentID?: string; metadata?: Record<string, unknown> } = {}): Record_ => ({
    session: { id, projectID: "p", title: `Title of ${id}`, cost: 0, time: { created: now - 3000, updated: now },
      location: { directory: dir }, tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } }, ...extra },
    messages: [step(`call-${id}`)], agent: {},
  } as Record_)
  const saved = [
    session("ses_lead"),
    session("ses_lead_sub", { parentID: "ses_lead" }),
    session("ses_task", { metadata: { kuvo: { startedBy: "ses_lead", by: { bot: "bot_lead", name: "Lead" } } } }),
    session("ses_task_sub", { parentID: "ses_task" }),
    session("ses_task_sub_sub", { parentID: "ses_task_sub" }),
    // (Started before starts were marked with their bot: found through its starter.)
    session("ses_older", { metadata: { kuvo: { startedBy: "ses_task" } } }),
    session("ses_seat", { metadata: { kuvo: { seat: { group: "grp_1", bot: "bot_lead" } } } }),
    session("ses_alone"),
    session("ses_other", { metadata: { kuvo: { by: { bot: "bot_other", name: "Other" } } } }),
  ]
  const scanner = new InsightsScan({ root: dir, cache: join(dir, "cache"), agents: () => [], store: () => saved,
    bots: () => [{ id: "bot_lead", name: "Lead", sessionID: "ses_lead" }], computer: () => "first" })
  const { records } = await scanner.scan(priced, now - 30 * day)
  const of = (session: string) => records.find((record) => record.session === session)!
  expect(of("ses_lead")).toMatchObject({ bot: "bot_lead", botName: "Lead", botOwn: true })
  expect(of("ses_lead").root).toBeUndefined()
  expect(of("ses_lead_sub")).toMatchObject({ bot: "bot_lead", root: "ses_lead", botOwn: true })
  expect(of("ses_task")).toMatchObject({ bot: "bot_lead", botName: "Lead" })
  expect(of("ses_task").botOwn).toBeUndefined()
  expect(of("ses_task_sub_sub")).toMatchObject({ bot: "bot_lead", root: "ses_task" })
  expect(of("ses_older")).toMatchObject({ bot: "bot_lead" })
  expect(of("ses_seat")).toMatchObject({ bot: "bot_lead", botOwn: true })
  expect(of("ses_alone").bot).toBeUndefined()

  const look = { color: "indigo", shape: "squircle", face: "calm" }
  const summary = await summaryOf(records, "", [{ id: "bot_lead", name: "Lead", look }])
  const bots = list(summary, "bots")
  expect(bots.name).toBe("Bots")
  const lead = bots.rows.find((row) => row.id === "bot_lead")!
  expect(lead).toMatchObject({ kind: "bot", name: "Lead", look, note: "2 sessions", tokens: 7 * processed() })
  expect(lead.sessions!.map(({ id, note, tokens }) => ({ id, note, tokens }))).toEqual([
    { id: "ses_task", note: "2 subagents", tokens: 3 * processed() },
    { id: "ses_lead", note: "Own chat", tokens: 2 * processed() },
    { id: "ses_older", note: undefined, tokens: processed() },
    { id: "ses_seat", note: "Own chat", tokens: processed() },
  ])
  expect(bots.rows.find((row) => row.id === "ses_alone")).toMatchObject({ kind: "session", name: "Title of ses_alone", note: "No bot" })
  expect(bots.rows.find((row) => row.id === "bot_other")).toMatchObject({ kind: "bot", name: "Other", tokens: processed() })
  expect(summary.totals.tokens).toBe(9 * processed())

  // Narrowed to the bot: only what it's over, its sessions listed.
  const narrowed = await summaryOf(records, "&bot=bot_lead")
  expect(narrowed.totals.tokens).toBe(7 * processed())
  expect(list(narrowed, "sessions")).toMatchObject({ name: "Sessions" })
  expect(list(narrowed, "sessions").rows.map((row) => row.id)).toEqual(["ses_task", "ses_lead", "ses_older", "ses_seat"])

  // Narrowed to one of its sessions: it and its subagents, each on its own.
  const task = await summaryOf(records, "&session=ses_task")
  expect(task.totals.tokens).toBe(3 * processed())
  expect(list(task, "sessions").rows.map((row) => row.id).sort()).toEqual(["ses_task", "ses_task_sub", "ses_task_sub_sub"])
})

test("each model has one name however its id was written, and narrows by it", async () => {
  for (const id of ["claude-opus-5-5", "claude-opus-5.5", "claude-opus-5-5-20261101", "anthropic/claude-opus-5.5", "Claude Opus 5 5", "Opus 5.5", "claude-opus-5-5[1m]"]) {
    expect(modelName(id), id).toEqual({ key: "opus-5.5", name: "Opus 5.5" })
  }
  expect(modelName("claude-3-5-sonnet-20241022").name).toBe("Sonnet 3.5")
  expect(modelName("gpt-5.5-codex").name).toBe("GPT-5.5 Codex")
  expect(modelName("gpt-4o-mini").name).toBe("GPT-4o Mini")
  expect(modelName("grok-4-4").name).toBe("Grok 4.4")
  expect(modelName(null).name).toBe("Unknown model")

  const records = [
    call("one", "a", { model: "claude-opus-5-5" }),
    call("two", "b", { model: "claude-opus-5.5-20261101" }),
    call("three", "c", { model: "anthropic/claude-opus-5.5", provider: "opencode" }),
    call("four", "d", { model: "gpt-5.5", provider: "codex" }),
  ]
  const summary = await summaryOf(records)
  expect(list(summary, "models").rows.map(({ id, name, provider, tokens }) => ({ id, name, provider, tokens }))).toEqual([
    { id: "opus-5.5", name: "Opus 5.5", provider: "claude", tokens: 3 * processed() },
    { id: "gpt-5.5", name: "GPT-5.5", provider: "codex", tokens: processed() },
  ])
  expect(list(summary, "providers").rows.map(({ name, tokens }) => [name, tokens])).toEqual([
    ["Claude Code", 2 * processed()], ["Codex", processed()], ["OpenCode", processed()],
  ])
  expect((await summaryOf(records, "&model=opus-5.5")).totals.tokens).toBe(3 * processed())
})

test("the cache rate is cached input over all input, at every level", async () => {
  const records = [
    call("warm", "a", {}, 900),
    call("cold", "b", { provider: "codex", model: "gpt-5.5", computer: "first" }, 0),
  ]
  const summary = await summaryOf(records)
  // 900 cached of 100 + 900 + 2, and none of 102.
  expect(summary.totals.cacheRate).toBeCloseTo(900 / (1002 + 102), 9)
  const providers = list(summary, "providers").rows
  expect(providers.find((row) => row.id === "claude")!.cacheRate).toBeCloseTo(900 / 1002, 9)
  expect(providers.find((row) => row.id === "codex")!.cacheRate).toBe(0)
  expect(list(summary, "bots").rows.find((row) => row.id === "cold")!.cacheRate).toBe(0)
  expect(list(summary, "computers").rows).toMatchObject([{ id: "first", cacheRate: summary.totals.cacheRate }])
  // No input counted: no rate, rather than 0%.
  const unknown = await summaryOf([call("partial", "a", { tokens: undefined, partial: true })])
  expect(unknown.totals.cacheRate).toBeNull()
})

test("the change is against the period of the same length just before", async () => {
  const before = (session: string, days: number, cached: number) => call(session, "old", { at: now - days * day }, cached)
  const records = [
    call("now-1", "a"), call("now-2", "b"),
    before("then", 40, 0),
    // (Older than both periods: not counted.)
    before("long-ago", 70, 0),
  ]
  const summary = await summaryOf(records)
  expect(summary.totals.tokens).toBe(2 * processed())
  expect(summary.change!.tokens).toBeCloseTo((2 * processed() - processed(0)) / processed(0), 9)
  expect(summary.change!.cost).toBeGreaterThan(0)
  expect(summary.change!.cacheRate).toBeCloseTo(500 / 602 - 0, 9)
  expect(summary.trend).toHaveLength(30)
  expect(summary.trend.at(-1)).toMatchObject({ day: "2026-11-18", tokens: 2 * processed() })
  // Nothing before: no change to state.
  expect((await summaryOf([call("now-1", "a")])).change).toBeNull()
})

test("at most three sessions stand out, each for one reason, once there are enough", async () => {
  const many = (session: string, calls: number, cached = 500, patch: Partial<InsightRecord> = {}) =>
    Array.from({ length: calls }, (_, i) => call(session, `${session}-${i}`, patch, cached))
  const subagents = ["s1", "s2", "s3"].map((sub) => call(sub, "sub", { root: "fanout" }))
  const records = [...many("costly", 6), ...many("cold", 3, 0), ...many("fanout", 1), ...subagents, ...many("quiet", 2), ...many("plain", 2)]
  const summary = await summaryOf(records)
  expect(summary.standouts.map(({ id, reason, kind }) => ({ id, reason, kind }))).toEqual([
    { id: "costly", reason: "Highest cost", kind: "cost" },
    { id: "cold", reason: "Low cache", kind: "cache" },
    { id: "fanout", reason: "3 subagents", kind: "subagents" },
  ])
  expect((await summaryOf([...many("costly", 6), ...many("cold", 3, 0)])).standouts).toEqual([])
})
