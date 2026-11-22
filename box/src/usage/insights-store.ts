/// <reference types="bun" />
import { createHash } from "node:crypto"
import type { Database as SQLite } from "bun:sqlite"

// The link bundle is also imported by Node. Only the usage worker opens this database.
const Database = typeof Bun === "undefined" ? undefined : (await import("bun:sqlite")).Database
import { upgradeFormat } from "../shared/format-version"
import { dayIn, windowDays } from "./history"
import { AUTO_REVIEW, codexCost, pricingModelOf, resolveCodexRates } from "./codex-pricing"
import { insightKey, type InsightRecord } from "./insight-records"
import { type Pricing, type Tokens } from "./pricing"
import { modelName } from "./insights-names"
import type { Lineage } from "./insights-scan"
import { SUMS, summarize, type ComputerSums, type ModelSums, type SessionSums, type Sums, type SummaryFacts } from "./insights-summary"

const DAY = 86_400_000
const tokenFields = ["input", "cacheRead", "cacheWrite5m", "cacheWrite1h", "output"] as const
export type Filter = { computer?: string; bot?: string; account?: string; provider?: string; session?: string }
/** What a summary is narrowed to: a bot (and all it's over), provider, account, model (by its key), or a conversation with its subagents. */
export type Scope = { bot?: string; provider?: string; account?: string; model?: string; session?: string }
export type OriginScope = { eligible: string[]; historical: string[]; scope?: string }
type Bound = [name: string, start: number, end: number]
type Stored = { body: string; cost: number | null; seq: number; received: number; sender: string | null }

function oldObservationWins(before: InsightRecord, incoming: InsightRecord, oldAt: number, newAt: number) {
  const quality = (record: InsightRecord) => !record.tokens ? 0 :
    record.partial || tokenFields.some(field => record.tokens?.[field] == null) ? 1 : 2
  const difference = Number(!!before.attributed) - Number(!!incoming.attributed) || quality(before) - quality(incoming) || newAt - oldAt
  if (difference) return difference > 0
  const key = (record: InsightRecord) => JSON.stringify(Object.entries(record)
    .filter(([field, value]) => value !== undefined && field !== "account" && field !== "accountObserved").sort(([a], [b]) => a.localeCompare(b)))
  return key(before) <= key(incoming)
}

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

// The schema before the stamp is the same. Keep its tables and rows in place.
const insightsUpgrades = [(db: SQLite): SQLite => {
  db.exec(`
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
  return db
}]

export class InsightsStore {
  private db: SQLite
  private pricing?: Pricing
  private priceRevision?: string
  private priceEra?: string
  private overviews = new Map<string, { until: number; result: Overview }>()
  private summaries = new Map<string, { until: number; result: SummaryFacts }>()
  private dayCache = new Map<string, { start: number; end: number; result: Metrics }>()
  private boundsCache = new Map<string, Bound[]>()
  private dataVersion: number

  constructor(file: string) {
    this.db = new Database!(file)
    try {
      this.db.exec("PRAGMA busy_timeout = 3000")
      this.db.transaction(() => {
        const { user_version } = this.db.query<{ user_version: number }, []>("PRAGMA user_version").get()!
        upgradeFormat("Insights database", user_version, this.db, insightsUpgrades)
        if (user_version !== insightsUpgrades.length) this.db.exec(`PRAGMA user_version = ${insightsUpgrades.length}`)
      }).immediate()
      this.db.exec("PRAGMA journal_mode = WAL")
      this.dataVersion = this.db.query<{ data_version: number }, []>("PRAGMA data_version").get()!.data_version
    } catch (error) {
      this.db.close()
      throw error
    }
  }

  close() { this.db.close() }

  read<T>(key: string): T | undefined {
    const saved = this.db.query<{ body: string }, [string]>("SELECT body FROM state WHERE key = ?").get(key)
    return saved ? JSON.parse(saved.body) : undefined
  }

  write(key: string, value: unknown) {
    this.db.query("INSERT INTO state VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body").run(key, JSON.stringify(value))
  }

  checkpoint(entries: readonly (readonly [string, unknown])[]) {
    this.db.transaction(() => {
      for (const [key, value] of entries) this.write(key, value)
    }).immediate()
  }

  private invalidate(days?: Set<number>) {
    this.overviews.clear()
    this.summaries.clear()
    if (!days) return this.dayCache.clear()
    for (const [key, { start, end }] of this.dayCache) {
      const first = Math.floor(start / DAY), last = Math.floor((end - 1) / DAY)
      if (days.has(first) || days.has(last) || (last - first > 1 && days.has(first + 1))) this.dayCache.delete(key)
    }
  }

  price(pricing: Pricing, now: number) {
    const era = pricingModelOf(AUTO_REVIEW, now)
    const revision = this.pricing === pricing ? this.priceRevision! :
      createHash("sha256").update(JSON.stringify(
        [pricing.supplement, pricing.primary.entries, pricing.secondary.entries],
        (_key, value) => value instanceof Map ? [...value] : value,
      )).digest("hex")
    if (this.pricing === pricing && this.priceEra === era) return
    this.db.transaction(() => {
      const stamp = revision + "/" + era
      if (this.read<string>("prices-v1") === stamp) return
      const set = this.db.query("UPDATE event SET cost = ?1 WHERE key = ?2 AND cost IS NOT ?1")
      for (const { body, key } of this.db.query<{ key: string; body: string }, []>("SELECT key, body FROM event WHERE human = 0").iterate()) {
        set.run(estimate(JSON.parse(body), pricing, now), key)
      }
      this.write("prices-v1", stamp)
    }).immediate()
    this.pricing = pricing
    this.priceRevision = revision
    this.priceEra = era
    this.invalidate()
  }

  cursor(sender: string, scope?: string) {
    if ((this.read<string | null>(`cursor-scope/${sender}`) ?? undefined) !== scope) return { after: 0 }
    return { after: this.read<number>(`cursor/${sender}`) ?? 0, epoch: this.read<string>(`peer-epoch/${sender}`) }
  }

  /** Repair lineage independently of which computer supplied the retained usage observation. */
  attribute(lineage: [string, Lineage][]) {
    const changed = this.db.query<{ key: string; seq: number | null; body: string }, [string, string]>(`
      SELECT key, seq, json_patch(body, ?1) AS body FROM event
      WHERE session = ?2 AND event.body != json_patch(event.body, ?1)`)
    const update = this.db.query(`
      UPDATE event SET seq = ?1, body = ?2, bot = json_extract(?2, '$.bot'),
        bot_name = json_extract(?2, '$.botName') WHERE key = ?3`)
    const dirty = this.db.transaction(() => {
      const before = this.read<number>("sequence") ?? 0
      let seq = before
      let dirty = false
      for (const [session, line] of lineage) {
        const patch = JSON.stringify({
          ...(line.bot ? { bot: line.bot, botName: line.botName ?? null, botOwn: line.own ?? null } : {}),
          root: line.root === session ? null : line.root,
        })
        for (const row of changed.all(patch, session)) {
          update.run(row.seq === null ? null : ++seq, row.body, row.key)
          dirty = true
        }
      }
      if (seq !== before) this.write("sequence", seq)
      return dirty
    }).immediate()
    if (dirty) this.invalidate()
  }

  take(records: InsightRecord[], pricing: Pricing, now: number, sender?: string, cursor?: number, epoch?: string, origins?: OriginScope) {
    const eligible = origins && new Set(origins.eligible)
    const historical = new Set(origins?.historical)
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
          const beforeEligible = eligible?.has(before.computer), incomingEligible = eligible?.has(record.computer)
          if (beforeEligible && !incomingEligible) continue
          if (beforeEligible === incomingEligible && (before.attributed && !record.attributed ||
            !!before.attributed === !!record.attributed &&
              (old.received < firstSeen || (old.received === firstSeen && before.computer < record.computer)))) continue
        }
        const sameOrigin = before?.computer === record.computer
        const historicalCopy = old && before && sameOrigin && historical.has(record.computer)
        // Keep the existing counts when the incoming copy has none, or when this computer already
        // has counts and the peer has no better provenance. A peer can still resolve its account.
        const keepLocal = sender && old?.sender === null && sameOrigin && (!record.attributed || before?.attributed)
        const keepCounts = historicalCopy ? (
          // Keep the more complete observation. Conflicting copies of equal quality converge
          // even when survivors concurrently send them to one another.
          oldObservationWins(before, record, old.received, firstSeen)
        ) : before?.tokens && (!record.tokens || keepLocal)
        if (keepCounts) {
          if ((!sender && !historicalCopy) || !sameOrigin || !before || before.accountObserved || !record.accountObserved || !record.account) continue
          record = { ...before, account: record.account, accountObserved: true }
        }
        if (before?.accountObserved && sameOrigin) {
          record.account = before.account
          record.accountObserved = true
        }
        // An observation can come from a peer that has the call but only part of the session tree.
        if (before?.session === record.session) {
          record = { ...record, root: record.root ?? before.root, ...(!record.bot && before.bot
            ? { bot: before.bot, botName: before.botName, botOwn: before.botOwn } : {}) }
        }
        const body = JSON.stringify(record)
        const received = historicalCopy ? (keepCounts ? old.received : firstSeen)
          : old && sameOrigin ? Math.min(old.received, firstSeen) : firstSeen
        if (old?.body === body && (!historicalCopy || old.received === received) && (sender || old.seq !== null)) continue
        const t = record.tokens
        const sum = (...parts: (number | null | undefined)[]) => {
          const known = parts.filter((n): n is number => n != null)
          return known.length ? known.reduce((a, b) => a + b, 0) : null
        }
        put.run({
          $key: record.key, $seq: !sender || old?.seq != null ? ++seq : null, $sender: keepCounts && keepLocal ? null : sender ?? null,
          $received: received,
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
        this.write(`cursor-scope/${sender}`, origins?.scope ?? null)
      }
      this.write("sequence", seq)
      this.db.query("DELETE FROM event WHERE at < ?").run(now - 366 * DAY)
    }).immediate()
    if (dirty.size) this.invalidate(dirty)
  }

  batch(after: number, historical: string[] = []) {
    return this.db.transaction(() => {
      let sequence = this.read<number>("sequence") ?? 0
      // A survivor can relay a synced historical row once its origin can no longer serve it.
      if (historical.length) {
        const promoted = this.db.query(`
          WITH pending AS MATERIALIZED (
            SELECT key, ? + ROW_NUMBER() OVER (ORDER BY at, key) AS seq
            FROM event WHERE seq IS NULL AND computer IN (${historical.map(() => "?").join(",")})
          )
          UPDATE event SET seq = pending.seq FROM pending WHERE event.key = pending.key
        `).run(sequence, ...historical).changes
        if (promoted) this.write("sequence", sequence += promoted)
      }
      const reset = after > sequence
      const start = reset ? 0 : after
      const rows = this.db.query<Stored, [number]>("SELECT seq, body, received FROM event WHERE seq > ? ORDER BY seq LIMIT 100").all(start)
      return { records: rows.map(({ body, received }) => ({ ...JSON.parse(body) as InsightRecord, observedAt: received })), next: rows.at(-1)?.seq ?? start, more: rows.length === 100, reset }
    }).immediate()
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

  /** A restore can merge into a pre-existing SQLite index without replacing its inode. */
  private fresh(now: number) {
    const { data_version } = this.db.query<{ data_version: number }, []>("PRAGMA data_version").get()!
    if (this.dataVersion !== data_version) {
      const pricing = this.pricing
      this.dataVersion = data_version
      this.invalidate()
      if (pricing) {
        this.pricing = undefined
        this.price(pricing, now)
      }
    }
  }

  /** The period's summary (insights-summary.ts) within `scope`, with the period of the same length before it. */
  summary(now: number, zone: string, period: number, scope: Scope, computers: string[], eligible = computers): SummaryFacts {
    this.fresh(now)
    const days = this.range(now, zone, period * 2)
    const current = days.slice(period)
    const start = current[0]![1], priorStart = days[0]![1]
    const key = insightKey("summary", zone, start, period, computers, eligible, scope)
    const kept = this.summaries.get(key)
    if (kept && kept.until >= now) return kept.result
    const result = this.db.transaction(() => {
      const inList = (values: unknown[]) => values.map(() => "?").join(",")
      const clauses = ["event.at >= ?", "event.at < ?", `event.computer IN (${inList(computers)})`,
        `NOT EXISTS (SELECT 1 FROM event AS newer WHERE newer.supersedes = event.key AND newer.computer IN (${inList(eligible)}))`]
      const args: (number | string)[] = [start, now + 1, ...computers, ...eligible]
      for (const name of ["bot", "provider", "account"] as const) {
        if (!scope[name]) continue
        clauses.push(`event.${name} = ?`)
        args.push(scope[name])
      }
      if (scope.model !== undefined) {
        // (Each id the model was written as, over both periods.)
        const ids = this.db.query<{ model: string | null }, (number | string)[]>(`SELECT DISTINCT model FROM event
          WHERE at >= ? AND computer IN (${inList(computers)})`).all(priorStart, ...computers)
          .filter(({ model }) => modelName(model).key === scope.model).map(({ model }) => model)
        const named = ids.filter((id): id is string => id !== null)
        clauses.push(`(${[...(named.length ? [`event.model IN (${inList(named)})`] : []), ...(ids.includes(null) ? ["event.model IS NULL"] : []), "0"].join(" OR ")})`)
        args.push(...named)
      }
      if (scope.session) {
        clauses.push("(event.session = ? OR json_extract(event.body, '$.root') = ?)")
        args.push(scope.session, scope.session)
      }
      const where = clauses.join(" AND ")
      const within = (from: number, until: number) => [from, until, ...args.slice(2)]
      const rows = this.db.query<SessionSums, (number | string)[]>(`
        SELECT session, MAX(title) AS title, MAX(bot) AS bot, MAX(bot_name) AS botName,
          MAX(json_extract(body, '$.root')) AS root, MAX(json_extract(body, '$.botOwn')) AS own, ${SUMS}
        FROM event WHERE ${where} GROUP BY session`).all(...args)
      // (A top-level conversation with no calls in scope is named as it was last.)
      const present = new Set(rows.map((row) => row.session))
      const missing = [...new Set(rows.flatMap((row) => row.root && !present.has(row.root) ? [row.root] : []))]
      const titles = new Map<string, string>()
      for (let i = 0; i < missing.length; i += 500) {
        const part = missing.slice(i, i + 500)
        for (const { session, title } of this.db.query<{ session: string; title: string | null }, string[]>(
          `SELECT session, MAX(title) AS title FROM event WHERE session IN (${inList(part)}) GROUP BY session`).all(...part)) {
          if (title) titles.set(session, title)
        }
      }
      const models = this.db.query<ModelSums, (number | string)[]>(`SELECT provider, model, ${SUMS}
        FROM event WHERE ${where} AND human = 0 GROUP BY provider, model`).all(...args)
      const machines = this.db.query<ComputerSums, (number | string)[]>(`SELECT computer, ${SUMS}
        FROM event WHERE ${where} AND human = 0 GROUP BY computer`).all(...args)
      const prior = this.db.query<Sums & { calls: number }, (number | string)[]>(`SELECT ${SUMS}, COUNT(*) FILTER (WHERE human = 0) AS calls
        FROM event WHERE ${where}`).get(...within(priorStart, start))!
      const day = this.db.query<{ cost: number; tokens: number }, (number | string)[]>(`SELECT COALESCE(SUM(cost),0) AS cost,
        COALESCE(SUM(processed),0) AS tokens FROM event INDEXED BY event_time WHERE ${where}`)
      const trend = current.map(([name, from, until], i) => ({ day: name, ...day.get(...within(from, i === current.length - 1 ? now + 1 : until))! }))
      return summarize({ rows, titles, models, computers: machines, prior, trend, ...(scope.session ? { session: scope.session } : {}) })
    })()
    if (this.summaries.size >= 24) this.summaries.clear()
    this.summaries.set(key, { until: now + 10_000, result })
    return result
  }

  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "overview", offset: number, eligible?: string[]): Overview
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "sessions", offset: number, eligible?: string[]): SessionsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "detail", offset: number, eligible?: string[]): CallsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number, eligible?: string[]): Overview | SessionsPage | CallsPage
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number, eligible = computers): Overview | SessionsPage | CallsPage {
    this.fresh(now)
    const days = this.range(now, zone, period)
    const since = days[0]![1]
    const names: (keyof Filter)[] = ["computer", "bot", "account", "provider", "session"]
    const clauses = ["event.at >= ?", "event.at < ?", `event.computer IN (${computers.map(() => "?").join(",")})`,
      `NOT EXISTS (SELECT 1 FROM event AS newer WHERE newer.supersedes = event.key AND newer.computer IN (${eligible.map(() => "?").join(",")}))`]
    const args: (number | string)[] = [since, now + 1, ...computers, ...eligible]
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
          SELECT session, MAX(title) AS title, MAX(at) AS at, MAX(latest_computer) AS computer, ${metrics}
          FROM (
            SELECT event.*, FIRST_VALUE(computer) OVER (PARTITION BY session ORDER BY at DESC, key) AS latest_computer
            FROM event WHERE ${where}
          ) GROUP BY session ORDER BY at DESC, session LIMIT 51 OFFSET ?`).all(...args, offset)
        const sessions = rows.slice(0, 50).map((row) => ({
          ...row,
          costShare: total.cost && row.cost !== null ? row.cost / total.cost : null,
          tokenShare: total.tokens && row.tokens !== null ? row.tokens / total.tokens : null,
        }))
        return { sessions, next: rows.length > 50 ? offset + 50 : null }
      })()
    }
    const overviewKey = insightKey(zone, since, period, computers, eligible, filter)
    const kept = this.overviews.get(overviewKey)
    if (kept && kept.until >= now) return kept.result
    const totals = this.db.query<Metrics & { sessions: number }, (string | number)[]>(`
      SELECT ${metrics}, COUNT(DISTINCT session) AS sessions FROM event WHERE ${where}`).get(...args)!
    const dayQuery = this.db.query<Metrics, (number | string)[]>(`SELECT ${metrics} FROM event INDEXED BY event_time WHERE ${where}`)
    const daily = days.map(([name, start, end]) => {
      const key = insightKey(zone, start, end, computers, eligible, filter)
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
