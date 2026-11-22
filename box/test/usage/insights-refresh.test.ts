import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { LinkDirectory, PeerPost } from "../../src/agents/groups/post"
import type { Record_ } from "../../src/agents/store"
import type { Message } from "../../src/agents/wire"
import { Insights, insightsRoutes } from "../../src/usage/insights"
import { InsightsDB } from "../../src/usage/insights-db"
import { KeptSummaries } from "../../src/usage/insights-kept"
import { InsightsScan } from "../../src/usage/insights-scan"
import { PricingStore } from "../../src/usage/pricing"

// Insights' refresh (PROTOCOL.md, "Insights", `refresh=1` and `countedAt`): a summary asked for
// with `refresh=1` counts the box's usage again before it answers, so work done since the last
// count shows at once, and every summary says when the box last counted. Real: the scanner, the
// index, the kept summaries and the route. Stand-ins: the saved sessions it scans, and the clock.

const dir = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-insights-refresh-"))
afterAll(() => rmSync(dir, { recursive: true, force: true }))
const start = Date.parse("2026-11-18T09:00:00Z")
const processed = 100 + 50 + 500 + 2

const session = (id: string, at: number): Record_ => ({
  session: { id, projectID: "p", title: `Title of ${id}`, cost: 0, time: { created: at - 3000, updated: at },
    location: { directory: dir }, tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } } },
  messages: [{
    id: `call-${id}`, type: "assistant", model: { providerID: "claude-code", id: "claude-test" }, agent: "build", content: [],
    time: { created: at - 2000, completed: at - 1000, origin: "first" },
    tokens: { input: 100, output: 50, reasoning: 0, cache: { read: 500, write: 2 } },
  } as Message],
  agent: {},
} as Record_)

test("a year of local calls is saved by the Insights worker across write batches", async () => {
  const home = mkdtempSync(join(dir, "worker-"))
  const saved = session("ses_year", start)
  const folder = join(home, "sessions", saved.session.id)
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "session.json"), JSON.stringify(saved.session))
  writeFileSync(join(folder, "messages.json"), JSON.stringify(
    Array.from({ length: 801 }, (_, n) => ({ ...saved.messages[0], id: `call-${n}` })),
  ))
  const db = new InsightsDB(":memory:")
  try {
    await db.scan({ home, cache: join(home, "cache"), bots: [], computer: "first" },
      new PricingStore(undefined, null).current(), start - 365 * 86400_000, start, { eligible: ["first"], historical: [] })
    expect(await db.read("sequence")).toBe(801)
    expect(await db.batch(800)).toMatchObject({
      records: [{ call: "call-800", computer: "first", tokens: { input: 100, output: 50 } }],
      next: 801, more: false,
    })
  } finally { await db.close() }
})

test("a refresh counts again before it answers, and the summary says when the box last counted", async () => {
  let now = start
  const saved = [session("ses_one", start - 60_000)]
  const scanner = new InsightsScan({ root: dir, cache: join(dir, "cache"), agents: () => [], store: () => saved, bots: () => [], computer: () => "first" })
  const db = new InsightsDB(":memory:")
  const post = new PeerPost()
  const insights = new Insights(db, scanner, new PricingStore(undefined, null), new LinkDirectory(post), post, "first", () => now)
  insights.kept = new KeptSummaries(undefined, () => now)
  const summary = async (query = "") => {
    const answer = await insightsRoutes(insights, "GET", ["summary"], new URLSearchParams(`period=30&zone=UTC${query}`))
    expect(answer.status).toBe(200)
    return (await answer.json() as { data: { state: string; countedAt?: number; totals: { tokens: number } } }).data
  }
  try {
    await insights.refresh()
    expect(await summary()).toMatchObject({ state: "ready", countedAt: start, totals: { tokens: processed } })

    // Work done since: not counted yet, and asked again without a refresh, what was counted.
    now = start + 2 * 60_000
    saved.push(session("ses_two", now - 30_000))
    expect(await summary()).toMatchObject({ countedAt: start, totals: { tokens: processed } })

    // Refreshed: counted again now, the new work in it.
    const refreshed = await summary("&refresh=1")
    expect(refreshed).toMatchObject({ state: "ready", countedAt: start + 2 * 60_000, totals: { tokens: 2 * processed } })
    expect(refreshed).not.toHaveProperty("asOf")
    // (And the summary asked for next, without one, is the refreshed one.)
    expect(await summary()).toMatchObject({ countedAt: start + 2 * 60_000, totals: { tokens: 2 * processed } })
  } finally {
    await db.close()
  }
})
