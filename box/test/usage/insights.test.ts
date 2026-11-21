import "../safety"
import { afterAll, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import type { Agent } from "../../src/agents/agent"
import { LinkDirectory, PeerPost } from "../../src/agents/groups/post"
import { Store } from "../../src/agents/store"
import { Insights, insightsRoutes } from "../../src/usage/insights"
import { InsightsDB } from "../../src/usage/insights-db"
import { dayBounds, InsightsStore } from "../../src/usage/insights-store"
import { InsightsScan } from "../../src/usage/insights-scan"
import { insightKey, type InsightRecord } from "../../src/usage/insight-records"
import { generationEvent } from "../../src/usage/antigravity-history"
import { Catalog, Pricing, PricingStore, supplementFrom } from "../../src/usage/pricing"
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

// Bound the failure of these tests if a dead worker leaves an RPC pending.
async function workerReply<T>(pending: Promise<T>): Promise<T> {
  let deadline: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([pending, new Promise<never>((_, reject) => {
      deadline = setTimeout(() => reject(new Error("The Insights request never settled.")), 1000)
    })])
  } finally {
    clearTimeout(deadline)
  }
}

test("an Insights worker exit rejects requests and the bridge reopens the persisted index", async () => {
  const file = join(dir, "worker-exit.sqlite")
  const stored = new InsightsDB(file)
  await stored.take([event("persisted")], priced, now)
  await stored.close()

  const script = join(dir, "exiting-worker.ts")
  const marker = join(dir, "worker-started")
  const source = join(import.meta.dir, "../../src/usage/insights-db-worker.ts")
  writeFileSync(script, `
    import { existsSync, writeFileSync } from "node:fs"
    import { serveInsightsDB } from ${JSON.stringify(source)}
    const marker = ${JSON.stringify(marker)}
    if (!existsSync(marker)) {
      writeFileSync(marker, "")
      process.exit(7)
    }
    serveInsightsDB()
  `)
  const db = new InsightsDB(file, script)
  const post = new PeerPost()
  const insights = new Insights(db, {} as InsightsScan, new PricingStore(undefined, null), new LinkDirectory(post), post, "first", () => now)
  const params = new URLSearchParams("period=7&zone=UTC")
  const requests = [db.read("local"), insights.view(params, "overview")]
  const settled = await workerReply(Promise.allSettled(requests))
  const exited: PromiseRejectedResult = { status: "rejected", reason: new Error("The Insights worker exited (code 7).") }
  expect(settled).toEqual([exited, exited])
  await expect(workerReply(db.read("local"))).rejects.toThrow(/worker exited.*7/)
  try {
    const view = await workerReply(insights.view(params, "overview"))
    expect(view && "totals" in view && view.totals.calls).toBe(1)
  } finally {
    await insights["db"].close()
  }
})

test("closing the Insights index is idempotent and later requests reject", async () => {
  const db = new InsightsDB(":memory:")
  await db.write("ready", true)
  await Promise.all([workerReply(db.close()), workerReply(db.close())])
  await expect(workerReply(db.read("ready"))).rejects.toThrow(/index is closed/)
  await workerReply(db.close())
})

test("copies merge before filters, resumed sessions count once, and moved bot calls retain their origin", async () => {
  const db = new InsightsDB(":memory:")
  const one = event("previous", "first", { bot: "bot_demo", botName: "Demo", skills: ["research", "writing"] })
  const two = event("new", "second", { bot: "bot_demo", botName: "Demo" })
  const another = event("third", "second", { session: "another", model: "unpriced-model" })
  await db.take([one, two, another, event("previous", "second", { attributed: undefined }), event("unknown", "second", { tokens: undefined })], priced, now)
  await db.take([one, another], priced, now, "second", 100)
  expect(await db.read<number>("cursor/second")).toBe(100)
  expect((await show(db)).totals).toMatchObject({ tokens: 456, processed: 1956, sessions: 2, unpriced: 2, partial: 1 })
  expect((await show(db, { bot: "bot_demo" })).breakdowns.skill.map((row) => row.costShare)).toEqual([0.5, 0.5])
  expect((await show(db, { bot: "bot_demo" }, ["second"])).totals).toMatchObject({ tokens: 152, sessions: 1 })
  expect((await show(db, { account: "claude#same-account" })).totals?.tokens).toBe(456)
  const calls = (await db.view(now, "America/New_York", 30, { session: "session" }, ["first", "second"], "detail", 0)).calls
  expect(calls?.map((call) => call.computer).sort()).toEqual(["first", "second", "second"])
  const cost = (await show(db)).totals!.cost!
  await db.price(catalog(10), now)
  expect((await show(db)).totals?.cost).toBeGreaterThan(cost)
  expect((await show(db)).totals?.tokens).toBe(456)
  await db.close()
})

test("a peer account observation enriches the matching local copy and keeps its counts", async () => {
  const db = new InsightsDB(":memory:")
  try {
    const fallback = event("copied", "second", { account: "second/claude" })
    const observed = { ...fallback, account: "claude#observed", accountObserved: true as const }
    await db.take([fallback], priced, now)
    await db.take([{ ...observed, tokens: { ...tokens, input: 700 } }], priced, now, "second", 1)
    expect((await show(db, { account: "claude#observed" })).totals).toMatchObject({ calls: 1, tokens: 152 })
    expect((await show(db, { account: "second/claude" })).totals.calls).toBe(0)
    await db.take([{ ...observed, account: "claude#other" }], priced, now, "second", 2)
    await db.take([fallback], priced, now)
    expect((await show(db, { account: "claude#observed" })).totals).toMatchObject({ calls: 1, tokens: 152 })
    expect((await db.batch(1)).records).toMatchObject([observed])
  } finally {
    await db.close()
  }
})

test("a newly available provider call replaces its saved placeholder across filters", async () => {
  const db = new InsightsDB(":memory:")
  const saved = event("missing", "first", { tokens: undefined })
  await db.take([saved], priced, now)
  expect((await show(db)).totals?.partial).toBe(1)
  await db.take([event("native", "first", { supersedes: saved.key })], priced, now)
  expect((await show(db)).totals).toMatchObject({ sessions: 1, tokens: 152, partial: 0, unpriced: 0 })
  await db.close()
})

test("day bounds use the viewer's time zone, including DST transitions", async () => {
  const at = Date.parse("2026-03-09T12:00:00Z")
  const bounds = dayBounds(at, "America/New_York", 7)
  expect(bounds.at(-2)).toEqual(["2026-03-08", Date.parse("2026-03-08T05:00:00Z"), Date.parse("2026-03-09T04:00:00Z")])
  const midnight = Date.parse("2026-11-18T05:00:00Z")
  const db = new InsightsDB(":memory:")
  await db.take([event("yesterday", "first", { at: midnight - 1 }), event("today", "first", { at: midnight })], priced, now)
  expect((await show(db)).daily?.slice(-2).map(({ tokens }) => tokens)).toEqual([152, 152])
  expect((await db.view(now, "Asia/Kathmandu", 7, {}, ["first"], "overview", 0)).daily?.at(-1)?.tokens).toBe(304)
  await db.close()
})

test("a cold yearly query and repricing both leave the bridge event loop responsive", async () => {
  const db = new InsightsDB(":memory:")
  try {
    for (let start = 0; start < 40_000; start += 1_000) {
      await db.take(Array.from({ length: 1_000 }, (_, offset) => {
        const i = start + offset
        return event(String(i), "first", { session: "session-" + i % 4000, at: now - 2_000 - i % 365 * day })
      }), priced, now)
    }
    const turns: unknown[] = []
    async function whileServing<T>(start: () => T | Promise<T>) {
      const turn = new Promise((resolve) => setTimeout(() => resolve("bridge timer"), 0))
      const work = Promise.resolve(start())
      const first = await Promise.race([turn, work.then(() => "database finished")])
      const result = await work
      turns.push(first)
      return result
    }
    const year = await whileServing(() => show(db, {}, ["first"], 365))
    expect(year.daily).toHaveLength(365)
    expect(year.totals).toMatchObject({ sessions: 4000, tokens: 40_000 * 152 })
    expect((await show(db, {}, ["first"], 365)).daily).toEqual(year.daily)
    await whileServing(() => db.price(catalog(10), now))
    expect(turns).toEqual(["bridge timer", "bridge timer"])
    const repriced = await show(db, {}, ["first"], 365)
    expect(repriced.totals.cost).toBeGreaterThan(year.totals.cost!)
    expect(repriced.totals.tokens).toBe(year.totals.tokens)
  } finally {
    await db.close()
  }
}, 60_000)

test("a session page computes shares of the whole filtered snapshot after a scan", async () => {
  const db = new InsightsDB(":memory:")
  try {
    await db.take([event("early", "first", { bot: "bot_demo", skills: ["research", "writing"] })], priced, now)
    const filter = { bot: "bot_demo" }
    const old = await show(db, filter)
    expect(old.breakdowns.skill.map((row) => row.tokenShare)).toEqual([1, 1])
    expect(old.daily.reduce((sum, row) => sum + (row.tokens ?? 0), 0)).toBe(old.totals.tokens!)
    await db.take([
      event("late", "first", { bot: "bot_demo" }),
      event("other", "first", { bot: "bot_demo", session: "unpriced", model: "unpriced-model" }),
      event("excluded", "first", { session: "other-bot" }),
    ], priced, now)
    const page = await db.view(now, "America/New_York", 30, filter, ["first"], "sessions", 0)
    expect(page.sessions.find((row) => row.session === "session")).toMatchObject({ costShare: 1, tokenShare: 2 / 3 })
    expect(page.sessions.find((row) => row.session === "unpriced")).toMatchObject({ costShare: null, tokenShare: 1 / 3 })
  } finally {
    await db.close()
  }
})

test("resume peer pages, reject vouched computers, and price incoming records locally", async () => {
  const db = new InsightsDB(":memory:")
  const post = new PeerPost()
  const directory = new LinkDirectory(post)
  const mail = new Insights(db, {} as InsightsScan, new PricingStore(undefined, null), directory, post, "first", () => now)
  directory.set({ self: { key: "first", name: "First" }, boxes: [{ key: "second", name: "Second" }], also: [{ key: "shared", name: "Shared" }], bots: [] })
  const page = { kind: "insights" as const, action: "page" as const, after: 0, next: 100, more: true, epoch: "first-generation", scanned: now, records: [event("remote", "second"), event("outside", "shared")] }
  await mail.take("shared", page)
  expect(await db.read("cursor/shared")).toBeUndefined()
  await mail.take("second", page)
  await mail.take("second", page)
  await mail.take("second", { ...page, after: 100, next: 101, more: false, records: [event("next", "second")] })
  expect(await db.read<number>("cursor/second")).toBe(101)
  await mail.take("second", { ...page, after: 101, next: 2, more: false, epoch: "new-generation", reset: true, records: [event("next", "second")] })
  expect(await db.read<number>("cursor/second")).toBe(2)
  expect(await db.read<string>("peer-epoch/second")).toBe("new-generation")
  expect((await show(db, {}, ["second", "shared"])).totals).toMatchObject({ sessions: 1, calls: 2, tokens: 304 })
  const asked = new URLSearchParams("period=7&zone=UTC")
  expect((await insightsRoutes(mail, "GET", [], asked)).status).toBe(200)
  expect((await insightsRoutes(mail, "GET", ["sessions", "session"], asked)).status).toBe(200)
  expect((await insightsRoutes(mail, "GET", [], new URLSearchParams("period=10"))).status).toBe(400)
  await db.close()
})

test("rewind an in-range cursor, then reset again when the serving peer learns the retired origin", async () => {
  class CapturedPost extends PeerPost {
    mail: Parameters<PeerPost["send"]>[1][] = []
    override send(_to: string, mail: Parameters<PeerPost["send"]>[1]) { this.mail.push(mail) }
  }
  const peer = (key: string) => ({ key, name: key })
  const open = (own: string, other: string) => {
    const db = new InsightsDB(":memory:")
    const post = new CapturedPost()
    const directory = new LinkDirectory(post)
    const insights = new Insights(db, {} as InsightsScan, new PricingStore(undefined, null), directory, post, own, () => now)
    const set = (history = false) => directory.set({
      setup: "set_history", self: peer(own), boxes: [peer(other)], bots: [], insightOrigins: history ? [peer("pc5")] : [],
    })
    set()
    return { db, post, insights, set }
  }
  const sending = open("sender", "reader"), reading = open("reader", "sender")
  const original = event("skipped", "pc5")
  const pull = async (after: number, epoch?: string) => {
    await sending.insights.take("reader", { kind: "insights", action: "pull", after, epoch })
    const mail = sending.post.mail.at(-1)
    if (mail?.kind !== "insights" || mail.action !== "page") throw new Error("Expected a page")
    return mail
  }
  try {
    await sending.db.take([original], priced, now)
    const skipped = await pull(0)
    expect(skipped).toMatchObject({ after: 0, next: 1, records: [] })
    await reading.insights.take("sender", skipped)
    expect(await reading.db.read("cursor/sender")).toBe(1)
    expect((await pull(1, skipped.epoch)).reset).toBeUndefined()
    reading.set(true)
    await reading.db.read("sequence") // Flush the scope-triggered pull through the worker.
    expect(reading.post.mail.at(-1)).toMatchObject({ action: "pull", after: 0 })
    await reading.insights.take("sender", await pull(0))
    expect(await reading.db.read("cursor/sender")).toBe(1)
    sending.set(true)
    const recovered = await pull(1, skipped.epoch)
    expect(recovered).toMatchObject({ reset: true, records: [{ key: original.key, computer: "pc5" }] })
    await reading.insights.take("sender", recovered)
    const params = new URLSearchParams("period=7&zone=UTC&session=session")
    expect(await reading.insights.view(params, "detail")).toMatchObject({ calls: [{ key: original.key, computer: "pc5" }] })
  } finally {
    await sending.db.close()
    await reading.db.close()
  }
})

test("historical replay converges on complete counts and rejects a conflicting unrelated origin", async () => {
  const original = event("one historical call", "pc5", { observedAt: now - 200, attributed: undefined })
  const divergent = { ...original, tokens: { ...tokens, output: 77 }, observedAt: now - 100 }
  const unrelated = { ...original, computer: "clone", attributed: true as const, observedAt: now - 250 }
  const scope = { historical: ["pc5"], eligible: ["pc5", "pc4"] }
  for (const [first, second] of [[original, divergent], [divergent, original]] as const) {
    const db = new InsightsStore(":memory:")
    try {
      db.take([unrelated], priced, now)
      db.take([first, second], priced, now, "peer", 1, "stream", scope)
      const batch = db.batch(0, ["pc5"])
      db.take([divergent, unrelated], priced, now, "peer", 2, "stream", scope)
      expect(db.batch(batch.next, ["pc5"]).records).toHaveLength(0)
      expect(db.view(now, "UTC", 7, {}, ["pc5"], "detail", 0).calls).toMatchObject([
        { computer: "pc5", key: original.key, tokens },
      ])
    } finally {
      db.close()
    }
  }
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
  await db.close()
  const externalDb = join(dir, "outside", "external-engine.db")
  const outside = new Database(externalDb)
  outside.exec("CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)")
  outside.query("INSERT INTO message VALUES (?, ?, ?, ?)").run(
    "outside-call", "outside-session", at, JSON.stringify({ role: "assistant", modelID: "claude-test", tokens: { input: 700 } }),
  )
  outside.close()
  symlinkSync(externalDb, join(engine, "opencode-extra.db"))
  const options = { root: home, cache: join(home, "cache"), agents: () => agents, store, bots: () => [], computer: () => "first", account: () => "account", engine }
  const scanner = new InsightsScan(options)
  const { records, issues } = await scanner.scan(priced, now - 365 * day)
  expect(issues).toEqual([])
  expect(records.some((record) => record.session === "outside-session")).toBe(false)
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

test("legacy history stays on its provider slot through sign-in changes, while observed accounts stay pinned", async () => {
  const home = join(dir, "account-history")
  const store = new Store(join(home, "saved"))
  const model = { providerID: "claude-code", id: "claude-test" }
  const step = (id: string, usageAccount?: string): Message => ({
    id, type: "assistant", model, agent: "build", content: [],
    time: { created: now - 1000, completed: now - 500, origin: "first", usageAccount },
  })
  store.addWhole({
    session: { id: "historical", projectID: "p", cost: 0, time: { created: now - 2000, updated: now },
      location: { directory: home }, tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } } },
    messages: [step("legacy"), step("observed", "claude#account-a")], agent: {},
  })
  let signedIn = "claude#account-a"
  const options = { root: home, cache: join(home, "cache"), agents: () => [], store, bots: () => [],
    computer: () => "first", account: () => signedIn }
  const scanner = new InsightsScan(options)
  const scan = async () => Object.fromEntries((await scanner.scan(priced, now - day)).records.map((row) => [row.call, row.account]))
  expect(await scan()).toEqual({ legacy: "first/claude-code", observed: "claude#account-a" })
  signedIn = "claude#account-b"
  expect(await scan()).toEqual({ legacy: "first/claude-code", observed: "claude#account-a" })
  const db = new InsightsDB(":memory:")
  try {
    const { records } = await scanner.scan(priced, now - day)
    const known = records.find((row) => row.call === "observed")!
    await db.take([known], priced, now)
    await db.take([{ ...known, account: "first/claude-code", accountObserved: undefined }], priced, now)
    const saved = await db.view(now, "UTC", 7, {}, ["first"], "detail", 0)
    expect(saved.calls[0]?.account).toBe("claude#account-a")
  } finally {
    await db.close()
  }
})


test("opening an unversioned Insights database stamps it without losing records", () => {
  const file = join(dir, "database-version.sqlite");
  // Create exactly the deployed schema, then simulate a database from before the stamp.
  const previous = new InsightsStore(file);
  previous.take([event("previous")], priced, now);
  previous.write("cursor/peer", 17);
  previous.close();
  const raw = new Database(file);
  raw.exec("PRAGMA user_version = 0");
  raw.close();

  const updated = new InsightsStore(file);
  expect(updated.read<number>("cursor/peer")).toBe(17);
  expect(updated.read<number>("sequence")).toBe(1);
  updated.close();

  const check = new Database(file);
  expect(check.query("PRAGMA user_version").get()).toEqual({ user_version: 1 });
  expect(check.query("SELECT seq FROM event").get()).toEqual({ seq: 1 });
  check.exec("PRAGMA user_version = 2");
  check.close();
  expect(() => new InsightsStore(file)).toThrow("Insights database format version 2 is unsupported");
  const refused = new Database(file, { readonly: true });
  try {
    expect(refused.query("PRAGMA user_version").get()).toEqual({ user_version: 2 });
    expect(refused.query("SELECT seq FROM event").get()).toEqual({ seq: 1 });
  } finally {
    refused.close();
  }
});
