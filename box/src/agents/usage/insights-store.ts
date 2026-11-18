import { Database } from "bun:sqlite"
import { dayIn, windowDays } from "./history"
import { codexCost, resolveCodexRates } from "./codex-pricing"
import { insightKey, type InsightRecord } from "./insight-records"
import { type Pricing, type Tokens } from "./pricing"

const DAY = 86_400_000
const tokenFields = ["input", "cacheRead", "cacheWrite5m", "cacheWrite1h", "output"] as const
export type Filter = { computer?: string; bot?: string; account?: string; provider?: string; session?: string }
type Bound = [name: string, start: number, end: number]
type Stored = { body: string; cost: number | null; seq: number; received: number; sender: string | null }

export function dayBounds(now: number, zone: string, period: number): Bound[] {
  const days = windowDays(now, zone, period - 1).reverse()
  // Find each local midnight from both sides in UTC. This also handles skipped civil days.
  const startOf = (day: string) => {
    const utc = Date.parse(day)
    let lo = utc - DAY, hi = utc + DAY
    while (lo < hi) {
      const mid = Math.floor((lo + hi) / 2)
      if (dayIn(mid, zone) < day) lo = mid + 1
      else hi = mid
    }
    return lo
  }
  const starts = days.map(startOf)
  return days.map((day, i) => [day, starts[i]!, starts[i + 1] ?? now + 1])
}

export function estimate(record: InsightRecord, pricing: Pricing, now: number): number | null {
  if (!record.model || !record.tokens || record.partial || tokenFields.some((field) => record.tokens?.[field] == null)) return null
  const tokens = record.tokens as Tokens
  for (const model of record.pricingModels ?? [record.model]) {
    if (record.provider === "codex") {
      const rates = resolveCodexRates(pricing, model, now)
      if (rates.rates) return codexCost(rates.rates, tokens, rates.rateModel, rates.fastAlias ? rates.baseRates : !!tokens.fast, !!record.tokens.ultrafast)
    } else {
      const cost = pricing.cost(model, tokens, record.provider !== "cursor")
      if (cost !== undefined && Number.isFinite(cost)) return cost
    }
  }
  return null
}

const metrics = `
  SUM(cost) AS cost, SUM(tokens) AS tokens, SUM(cache_read) AS cached,
  SUM(input) AS input, SUM(cache_write) AS creation, SUM(output) AS output,
  COALESCE(SUM(human),0) AS messages,
  COUNT(*) FILTER (WHERE human = 0) AS calls,
  SUM(processed) AS processed,
  COUNT(*) FILTER (WHERE human = 0 AND cost IS NULL) AS unpriced,
  COUNT(*) FILTER (WHERE human = 0 AND partial = 1) AS partial`

type Metrics = {
  cost: number | null; tokens: number | null; cached: number | null; input: number | null; processed: number | null
  creation: number | null; output: number | null; messages: number; calls: number; unpriced: number; partial: number
}
type Row = { name: string; provider: string | null } & Metrics

type BreakdownRow = Row & { costShare: number | null; tokenShare: number | null }
export interface Overview {
  totals: Metrics & { sessions: number }
  daily: (Metrics & { name: string })[]
  providers: BreakdownRow[]
  breakdowns: { model: BreakdownRow[]; agent: BreakdownRow[]; day: (Metrics & { name: string })[]; skill: BreakdownRow[] }
}

export type SessionsPage = {
  sessions: (Metrics & { session: string; title: string | null; at: number; computer: string; costShare: number | null; tokenShare: number | null })[]
  next: number | null
}

export type CallsPage = {
  calls: (InsightRecord & { cost: number | null; tokenCount: number | null })[]
  next: number | null
}

export type Route = "overview" | "sessions" | "detail"

export class InsightsStore {
  private db: Database
  private pricing?: Pricing
  private overviews = new Map<string, { until: number; result: Overview }>()
  private dayCache = new Map<string, { start: number; end: number; result: Metrics }>()
  private boundsCache = new Map<string, Bound[]>()

  constructor(file: string) {
    this.db = new Database(file)
    this.db.exec(`
      PRAGMA journal_mode = WAL;
      CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS event (
        key TEXT PRIMARY KEY, seq INTEGER, sender TEXT, received INTEGER NOT NULL,
        at INTEGER NOT NULL, session TEXT NOT NULL, computer TEXT NOT NULL, bot TEXT, bot_name TEXT, title TEXT,
        provider TEXT NOT NULL, account TEXT, model TEXT, agent TEXT, human INTEGER NOT NULL, partial INTEGER NOT NULL,
        cost REAL, tokens INTEGER, input INTEGER, cache_read INTEGER, cache_write INTEGER, output INTEGER,
        skills TEXT NOT NULL, body TEXT NOT NULL, supersedes TEXT, processed INTEGER
      );
      CREATE INDEX IF NOT EXISTS event_time ON event(at, session);
      CREATE INDEX IF NOT EXISTS event_session ON event(session, at);
      CREATE INDEX IF NOT EXISTS event_computer ON event(computer, at, session);
      CREATE INDEX IF NOT EXISTS event_bot ON event(bot, at, session);
      CREATE INDEX IF NOT EXISTS event_provider ON event(provider, at, session);
      CREATE INDEX IF NOT EXISTS event_account ON event(account, at, session);
      CREATE INDEX IF NOT EXISTS event_seq ON event(seq);
      CREATE INDEX IF NOT EXISTS event_supersedes ON event(supersedes) WHERE supersedes IS NOT NULL;
    `)
  }

  close() { this.db.close() }

  read<T>(key: string): T | undefined {
    const saved = this.db.query<{ body: string }, [string]>("SELECT body FROM state WHERE key = ?").get(key)
    return saved ? JSON.parse(saved.body) : undefined
  }

  write(key: string, value: unknown) {
    this.db.query("INSERT INTO state VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body").run(key, JSON.stringify(value))
  }

  private invalidate(days?: Set<number>) {
    this.overviews.clear()
    if (!days) return this.dayCache.clear()
    for (const [key, { start, end }] of this.dayCache) {
      const first = Math.floor(start / DAY), last = Math.floor((end - 1) / DAY)
      if (days.has(first) || days.has(last) || (last - first > 1 && days.has(first + 1))) this.dayCache.delete(key)
    }
  }

  price(pricing: Pricing, now: number) {
    if (this.pricing === pricing) return
    this.db.transaction(() => {
      const set = this.db.query("UPDATE event SET cost = ? WHERE key = ?")
      for (const { body, key } of this.db.query<{ key: string; body: string }, []>("SELECT key, body FROM event WHERE human = 0").iterate()) {
        set.run(estimate(JSON.parse(body), pricing, now), key)
      }
    })()
    this.pricing = pricing
    this.invalidate()
  }

  take(records: InsightRecord[], pricing: Pricing, now: number, sender?: string, cursor?: number, epoch?: string) {
    const previous = this.db.query<Stored & { at: number }, [string]>("SELECT body, cost, seq, received, sender, at FROM event WHERE key = ?")
    const put = this.db.query(`
      INSERT OR REPLACE INTO event VALUES (
        $key,$seq,$sender,$received,$at,$session,$computer,$bot,$botName,$title,$provider,$account,$model,$agent,$human,$partial,
        $cost,$tokens,$input,$cacheRead,$cacheWrite,$output,$skills,$body,$supersedes,$processed
      )`)
    const dirty = new Set<number>()
    this.db.transaction(() => {
      let seq = this.read<number>("sequence") ?? 0
      for (const { observedAt, ...incoming } of records) {
        let record = incoming
        if (record.at < now - 366 * DAY || record.at > now + DAY) continue
        const old = previous.get(record.key)
        const before: InsightRecord | undefined = old && JSON.parse(old.body)
        // A stamped origin wins over a legacy observation. Older copies without a
        // stamp keep their first observation, even when peers receive them in a different order.
        const firstSeen = observedAt !== undefined ? Math.min(now, Math.max(record.at, observedAt)) : now
        if (old && before && before.computer !== record.computer) {
          if (before.attributed && !record.attributed) continue
          if (!!before.attributed === !!record.attributed &&
              (old.received < firstSeen || (old.received === firstSeen && before.computer < record.computer))) continue
        }
        const sameOrigin = before?.computer === record.computer
        // Keep the existing counts when the incoming copy has none, or when this computer already
        // has counts and the peer has no better provenance. A peer can still resolve its account.
        const keepLocal = sender && old?.seq != null && sameOrigin && (!record.attributed || before?.attributed)
        if (before?.tokens && (!record.tokens || keepLocal)) {
          if (!sender || !sameOrigin || before.accountObserved || !record.accountObserved || !record.account) continue
          record = { ...before, account: record.account, accountObserved: true }
        }
        if (before?.accountObserved && sameOrigin) {
          record.account = before.account
          record.accountObserved = true
        }
        const body = JSON.stringify(record)
        if (old?.body === body && (sender || old.seq !== null)) continue
        const t = record.tokens
        const sum = (...parts: (number | null | undefined)[]) => {
          const known = parts.filter((n): n is number => n != null)
          return known.length ? known.reduce((a, b) => a + b, 0) : null
        }
        put.run({
          $key: record.key, $seq: !sender || old?.seq != null ? ++seq : null, $sender: sender ?? null, $received: old && before?.computer === record.computer ? Math.min(old.received, firstSeen) : firstSeen,
          $at: record.at, $session: record.session, $computer: record.computer, $bot: record.bot ?? null,
          $botName: record.botName ?? null, $title: record.title ?? null, $provider: record.provider,
          $account: record.account ?? null, $model: record.model ?? null, $agent: record.agent ?? null,
          $human: record.human ? 1 : 0, $partial: !record.human && (record.partial || !t || tokenFields.some((field) => t[field] == null)) ? 1 : 0,
          $cost: estimate(record, pricing, now), $tokens: sum(t?.input, t?.output, t?.cacheWrite5m, t?.cacheWrite1h),
          $input: t?.input ?? null, $cacheRead: t?.cacheRead ?? null, $cacheWrite: sum(t?.cacheWrite5m, t?.cacheWrite1h), $output: t?.output ?? null,
          $skills: JSON.stringify(record.skills ?? []), $body: body, $supersedes: record.supersedes ?? null,
          $processed: record.reportedTotal ?? sum(t?.input, t?.output, t?.cacheRead, t?.cacheWrite5m, t?.cacheWrite1h),
        })
        dirty.add(Math.floor(record.at / DAY))
        if (old) dirty.add(Math.floor(old.at / DAY))
        const replaced = record.supersedes ? previous.get(record.supersedes) : undefined
        if (replaced) dirty.add(Math.floor(replaced.at / DAY))
      }
      if (sender && cursor !== undefined) {
        this.write(`cursor/${sender}`, cursor)
        if (epoch) this.write(`peer-epoch/${sender}`, epoch)
      }
      this.write("sequence", seq)
      this.db.query("DELETE FROM event WHERE at < ?").run(now - 366 * DAY)
    })()
    if (dirty.size) this.invalidate(dirty)
  }

  batch(after: number) {
    const rows = this.db.query<Stored, [number]>("SELECT seq, body, received FROM event WHERE seq > ? ORDER BY seq LIMIT 100").all(after)
    return { records: rows.map(({ body, received }) => ({ ...JSON.parse(body) as InsightRecord, observedAt: received })), next: rows.at(-1)?.seq ?? after, more: rows.length === 100 }
  }

  choices(computers: string[]) {
    const names = this.db.query<{ key: string; name: string }, string[]>(`
      SELECT bot AS key, COALESCE(MAX(bot_name), bot) AS name
      FROM event WHERE bot IS NOT NULL AND computer IN (${computers.map(() => "?").join(",")})
      GROUP BY bot ORDER BY name COLLATE NOCASE, key`).all(...computers)
    return names
  }

  private range(now: number, zone: string, period: number) {
    const day = dayIn(now, zone)
    const key = `${zone}/${period}/${day}`
    let bounds = this.boundsCache.get(key)
    if (!bounds) {
      if (this.boundsCache.size >= 32) this.boundsCache.clear()
      bounds = dayBounds(now, zone, period)
      this.boundsCache.set(key, bounds)
    }
    return bounds.map(([date, start, end], i): Bound => [date, start, i === bounds.length - 1 ? now + 1 : end])
  }

  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "overview", offset: number): Overview
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "sessions", offset: number): SessionsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "detail", offset: number): CallsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number): Overview | SessionsPage | CallsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number): Overview | SessionsPage | CallsPage {
    const days = this.range(now, zone, period)
    const since = days[0]![1]
    const names: (keyof Filter)[] = ["computer", "bot", "account", "provider", "session"]
    const clauses = ["event.at >= ?", "event.at < ?", `event.computer IN (${computers.map(() => "?").join(",")})`,
      "NOT EXISTS (SELECT 1 FROM event AS newer WHERE newer.supersedes = event.key)"]
    const args: (number | string)[] = [since, now + 1, ...computers]
    for (const name of names) {
      const value = filter[name]
      if (!value) continue
      clauses.push(`event.${name} = ?`)
      args.push(value)
    }
    const where = clauses.join(" AND ")
    if (route === "detail") {
      const rows = this.db.query<Stored & { tokenCount: number | null }, (number | string)[]>(`SELECT body, cost, tokens AS tokenCount FROM event WHERE ${where}
        AND human = 0 ORDER BY at, key LIMIT 101 OFFSET ?`).all(...args, offset)
      return { calls: rows.slice(0, 100).map(({ body, cost, tokenCount }) => ({ ...JSON.parse(body) as InsightRecord, cost, tokenCount })), next: rows.length > 100 ? offset + 100 : null }
    }
    if (route === "sessions") {
      return this.db.transaction(() => {
        const total = this.db.query<{ cost: number | null; tokens: number | null }, (number | string)[]>(
          `SELECT SUM(cost) AS cost, SUM(tokens) AS tokens FROM event WHERE ${where}`).get(...args)!
        const rows = this.db.query<Metrics & { session: string; title: string | null; at: number; computer: string }, (number | string)[]>(`
          SELECT session, MAX(title) AS title, MAX(at) AS at, (SELECT last.computer FROM event AS last WHERE last.session = event.session ORDER BY last.at DESC, last.key LIMIT 1) AS computer, ${metrics}
          FROM event WHERE ${where} GROUP BY session ORDER BY at DESC, session LIMIT 51 OFFSET ?`).all(...args, offset)
        const sessions = rows.slice(0, 50).map((row) => ({
          ...row,
          costShare: total.cost && row.cost !== null ? row.cost / total.cost : null,
          tokenShare: total.tokens && row.tokens !== null ? row.tokens / total.tokens : null,
        }))
        return { sessions, next: rows.length > 50 ? offset + 50 : null }
      })()
    }
    const overviewKey = insightKey(zone, since, period, computers, filter)
    const kept = this.overviews.get(overviewKey)
    if (kept && kept.until >= now) return kept.result
    const totals = this.db.query<Metrics & { sessions: number }, (string | number)[]>(`
      SELECT ${metrics}, COUNT(DISTINCT session) AS sessions FROM event WHERE ${where}`).get(...args)!
    const dayQuery = this.db.query<Metrics, (number | string)[]>(`SELECT ${metrics} FROM event INDEXED BY event_time WHERE ${where}`)
    const daily = days.map(([name, start, end]) => {
      const key = insightKey(zone, start, end, computers, filter)
      let cached = this.dayCache.get(key)
      if (!cached) {
        if (this.dayCache.size >= 2000) this.dayCache.clear()
        cached = { start, end, result: dayQuery.get(start, end, ...args.slice(2))! }
        if (name !== days.at(-1)?.[0]) this.dayCache.set(key, cached)
      }
      return { name, ...cached.result }
    })
    const grouped = (name: string, extra = "", group = "1") =>
      this.db.query<Row, (string | number)[]>(`SELECT ${name} AS name, MIN(provider) AS provider, ${metrics}
        FROM event ${extra} WHERE ${where} AND human = 0 GROUP BY ${group}
        ORDER BY COALESCE(cost,0) DESC, COALESCE(tokens,0) DESC, name LIMIT 2000`).all(...args)
    const shares = <T extends Metrics & { name: string }>(rows: T[]) => rows.map((row) => ({
      ...row, costShare: totals.cost && row.cost !== null ? row.cost / totals.cost : null,
      tokenShare: totals.tokens && row.tokens !== null ? row.tokens / totals.tokens : null,
    }))
    const result: Overview = {
      totals, daily, providers: shares(grouped("provider")),
      breakdowns: {
        model: shares(grouped("COALESCE(model,'Unknown model')", "", "1, provider")),
        agent: shares(grouped("COALESCE(MAX(bot_name),agent,provider)", "", "COALESCE(bot, agent, provider), CASE WHEN bot IS NULL THEN provider END")),
        day: shares(daily),
        skill: shares(grouped("skill.value", ", json_each(event.skills) AS skill")),
      },
    }
    if (this.overviews.size >= 24) this.overviews.clear()
    this.overviews.set(overviewKey, { until: now + 10_000, result })
    return result
  }
}
