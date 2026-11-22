// Insights serves a local SQLite index and exchanges bounded pages of usage metadata over peer mail.
import { hostname } from "node:os"
import type { LinkDirectory, PeerPost } from "../agents/groups/post"
import { dayIn, knownZone, ownZone } from "./history"
import { InsightsDB, type Filter, type Scope } from "./insights-db"
import { insightKey, type InsightRecord } from "./insight-records"
import { providerName } from "./insights-names"
import type { KeptSummaries } from "./insights-kept"
import type { InsightsScan } from "./insights-scan"
import { cacheRate, MOST, type Conversation, type Sums } from "./insights-summary"
import type { PricingStore } from "./pricing"

const DAY = 86_400_000
const SCAN_EVERY = 5 * 60_000
const MAX_PAGE = 100
type Request = { kind: "insights"; action: "pull"; after: number; epoch?: string; refresh?: true }
type Page = { kind: "insights"; action: "page"; after: number; next: number; more: boolean; epoch: string; reset?: true; scanned?: number; issues?: string[]; records: InsightRecord[] }
export type InsightsMail = Request | Page
export const isInsightsMail = (value: any): value is InsightsMail => value?.kind === "insights"

const optionalText = (value: unknown) => typeof value === "string" && value.length > 0 ? value.slice(0, 512) : undefined
const small = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 512
const validAt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0
function recordFrom(value: InsightRecord): InsightRecord | undefined {
  if (!value || typeof value !== "object" || !/^[a-f0-9]{64}$/.test(value.key) || !validAt(value.at)) return
  if (![value.computer, value.provider, value.session, value.call].every(small)) return
  if (value.supersedes !== undefined && !/^[a-f0-9]{64}$/.test(value.supersedes)) return
  for (const list of [value.skills, value.pricingModels]) {
    if (list !== undefined && (!Array.isArray(list) || list.length > 32 || !list.every(small))) return
  }
  if (value.reportedTotal !== undefined && !validAt(value.reportedTotal)) return
  const t = value.tokens
  if (t !== undefined && (!t || typeof t !== "object" || ![t.input, t.cacheRead, t.cacheWrite5m, t.cacheWrite1h, t.output].every((n) => n === null || validAt(n)))) return
  const { key, at, session, nativeSession, call, provider, computer, account, bot, botName, root, model, pricingModels, agent, title, skills, supersedes, reportedTotal, observedAt } = value
  return {
    key, at, session, call, provider, computer, pricingModels, skills, supersedes, reportedTotal,
    account: optionalText(account), bot: optionalText(bot), botName: optionalText(botName), model: optionalText(model),
    agent: optionalText(agent), title: optionalText(title), nativeSession: optionalText(nativeSession),
    ...(optionalText(root) && root !== session ? { root: optionalText(root) } : {}),
    ...(value.botOwn === true ? { botOwn: true } : {}),
    ...(validAt(observedAt) ? { observedAt } : {}),
    ...(value.accountObserved === true && optionalText(account) ? { accountObserved: true } : {}),
    ...(value.attributed === true ? { attributed: true } : {}),
    ...(value.human === true ? { human: true } : {}),
    ...(value.partial === true ? { partial: true } : {}),
    ...(t ? { tokens: { input: t.input, cacheRead: t.cacheRead, cacheWrite5m: t.cacheWrite5m, cacheWrite1h: t.cacheWrite1h,
      ...(t.fast === true ? { fast: true } : {}), ...(t.ultrafast === true ? { ultrafast: true } : {}), output: t.output } } : {}),
  }
}

export class Insights {
  /** The summaries last served, served at once while they're read afresh (insights-kept.ts). */
  kept?: KeptSummaries
  private scan?: Promise<void>
  private timer?: ReturnType<typeof setInterval>
  private refreshed?: number
  private requested = new Map<string, number>()
  private requestedScope?: string
  private epoch?: Promise<string>
  private mail = Promise.resolve()

  constructor(
    private index: InsightsDB,
    private scanner: InsightsScan,
    private pricing: PricingStore,
    private directory: LinkDirectory,
    private post: PeerPost,
    private localKey: string | (() => string),
    private now = Date.now,
    private conversationHome?: (session: string, origin?: string) => string | undefined,
    /** This computer's bots, for how each looks. */
    private localBots: () => { id: string; name: string; look?: unknown }[] = () => [],
  ) {
    this.directory.listen(() => this.sync())
    this.post.onFollowed(() => {
      this.requested.clear()
      this.sync()
    })
  }

  private get local() { return typeof this.localKey === "string" ? this.localKey : this.localKey() }

  private get db() {
    const reopened = this.index.reopen()
    if (reopened !== this.index) {
      this.index = reopened
      this.epoch = undefined
    }
    return this.index
  }

  start() {
    this.timer ??= setInterval(() => this.refresh(), SCAN_EVERY)
    this.timer.unref?.()
    this.refresh()
  }

  refresh(force = false) {
    this.sync(force)
    if (this.scan) return this.scan
    this.scan = this.round().catch((error) => {
      console.error("Insights scan:", error instanceof Error ? error.message : error)
      return this.db.write("failed", true).catch(() => {})
    }).finally(() => { this.scan = undefined })
    return this.scan
  }

  private async round() {
    const now = this.now()
    const pricing = this.pricing.current()
    const since = Math.floor(now / DAY) * DAY - 365 * DAY
    const { records, issues } = await this.scanner.scan(pricing, since)
    // Bound each worker message so the bridge can handle other requests during a backfill.
    for (let offset = 0; offset < records.length; offset += 800) {
      await this.db.take(records.slice(offset, offset + 800), this.pricing.current(), now, undefined, undefined, undefined, this.origins())
    }
    await this.db.take([], this.pricing.current(), now)
    await this.db.write("failed", false)
    await this.db.write("local", { scanned: this.now(), issues })
    this.sync()
    for (const peer of this.directory.peerBoxes()) {
      const served = await this.db.read<{ after: number; epoch?: string }>(`served/${peer.key}`)
      if (served) await this.sendPage(peer.key, served.after, served.epoch)
    }
  }

  private ownEpoch() {
    const db = this.db
    return this.epoch ??= (async () => {
      const kept = await db.read<string>("epoch")
      if (kept) return kept
      const epoch = crypto.randomUUID()
      await db.write("epoch", epoch)
      return epoch
    })()
  }

  private historyScope() {
    const origins = this.directory.insightOrigins()
    return origins.length ? insightKey("historical Insights v1", this.directory.setup, origins.map(({ key }) => key).sort()) : undefined
  }

  private origins() {
    const local = this.local
    return {
      eligible: [this.directory.own()?.key ?? local, local, ...this.directory.peerBoxes().map(({ key }) => key),
        ...this.directory.insightOrigins().map(({ key }) => key)],
      historical: this.directory.insightOrigins().map(({ key }) => key),
    }
  }

  private exportScope() {
    return insightKey(this.directory.setup, this.origins().eligible.sort())
  }

  private sync(force = false) {
    if (!this.post.followed) return
    const scope = this.historyScope()
    if (scope !== this.requestedScope) {
      this.requestedScope = scope
      this.requested.clear()
    }
    const now = this.now()
    const fresh = force && (this.refreshed === undefined || now - this.refreshed > 30_000)
    if (fresh) this.refreshed = now
    for (const peer of this.directory.peerBoxes()) {
      if (!fresh && now - (this.requested.get(peer.key) ?? -Infinity) < 30_000) continue
      void this.pull(peer.key, fresh).catch((error) => console.error("Insights peer sync:", error))
    }
  }

  private async pull(to: string, refresh = false) {
    const scope = this.historyScope()
    this.requested.set(to, this.now())
    const cursor = await this.db.cursor(to, scope)
    if (!this.directory.isPeer(to) || scope !== this.historyScope()) return
    this.post.send(to, { kind: "insights", action: "pull", ...cursor, ...(refresh ? { refresh: true } : {}) }, false)
  }

  private async sendPage(to: string, after: number, epoch?: string) {
    const scope = this.exportScope()
    const historical = this.directory.insightOrigins().map(({ key }) => key)
    const [indexEpoch, coverage] = await Promise.all([this.ownEpoch(), this.db.read<Coverage>("local")])
    // Export eligibility can change on this computer after a peer has already pulled a page.
    const ownEpoch = insightKey(indexEpoch, scope)
    const changed = epoch !== undefined && epoch !== ownEpoch
    const batch = await this.db.batch(changed ? 0 : after, historical)
    if (!this.directory.isPeer(to) || scope !== this.exportScope()) return
    const reset = changed || batch.reset
    const own = this.directory.own()?.key
    const local = this.local
    const records = batch.records.map(record => own && record.computer === local
      ? { ...record, computer: own, account: record.account?.replace(local + "/", own + "/") }
      : record).filter(record => this.directory.isInsightOrigin(record.computer))
    this.post.send(to, { kind: "insights", action: "page", after, next: batch.next, more: batch.more, epoch: ownEpoch, ...(reset ? { reset: true } : {}), records, ...coverage }, false)
  }

  take(from: string, mail: InsightsMail) {
    const handled = this.mail.then(() => this.receive(from, mail))
    // A failed delivery must not stop the next one from being processed.
    this.mail = handled.catch(() => {})
    return handled
  }

  private async receive(from: string, mail: InsightsMail) {
    if (!this.directory.isPeer(from) || !validAt(mail.after)) return
    if (mail.action === "pull") {
      const epoch = small(mail.epoch) ? mail.epoch : undefined
      await this.db.write(`served/${from}`, { after: mail.after, epoch })
      await this.sendPage(from, mail.after, epoch)
      if (mail.refresh === true) void this.refresh()
      return
    }
    if (mail.action !== "page" || !validAt(mail.next) || (!mail.reset && mail.next < mail.after) || !small(mail.epoch) || !Array.isArray(mail.records) || mail.records.length > MAX_PAGE) return
    const scope = this.historyScope()
    const { after: expected, epoch: priorEpoch } = await this.db.cursor(from, scope)
    if (!this.directory.isPeer(from) || scope !== this.historyScope()) return
    if (mail.after !== expected) return
    if (priorEpoch && priorEpoch !== mail.epoch && !mail.reset) return
    const records = mail.records.map(recordFrom).filter((record): record is InsightRecord =>
      !!record && this.directory.isInsightOrigin(record.computer))
    const pricing = this.pricing.current()
    await this.db.take(records, pricing, this.now(), from, mail.next, mail.epoch, { ...this.origins(), scope })
    if (!mail.more && validAt(mail.scanned) && mail.scanned <= this.now() + DAY) await this.db.write(`peer/${from}`, {
      scanned: mail.scanned,
      issues: Array.isArray(mail.issues) ? mail.issues.filter(small).slice(0, 12) : [],
    })
    if (mail.more && (mail.reset || mail.next > expected)) await this.pull(from)
  }

  /** Your computers as Insights counts them: this one (under each key it has recorded with) and the rest. */
  private computers() {
    const localKey = this.local
    const own = this.directory.own() ?? { key: localKey, name: hostname() }
    const peers = this.directory.peerBoxes()
    const computers = [own, ...peers, ...this.directory.insightOrigins()]
    const localKeys = own.key === localKey ? [own.key] : [own.key, localKey]
    const keys = [...new Set([...localKeys, ...computers.map(({ key }) => key)])]
    return { own, peers, computers, localKeys, keys }
  }

  /**
   * The period's summary (PROTOCOL.md, "Insights"): its totals and their change from the period
   * before, a day-by-day trend, ranked lists (bots with the sessions each is over, providers,
   * models and computers) and up to three standout sessions, each with cost, tokens and cache rate.
   */
  async summary(params: URLSearchParams) {
    const now = this.now()
    const period = Number(params.get("period") ?? 30)
    if (![7, 30, 90].includes(period)) return
    const zone = knownZone(params.get("zone")) ?? ownZone()
    const scope: Scope = {}
    for (const name of ["bot", "provider", "account", "model", "session"] as const) {
      const value = params.get(name)
      if (value) scope[name] = value.slice(0, 512)
    }
    const { own, computers, localKeys, keys } = this.computers()
    const computer = params.get("computer")
    // A machine can have recorded work before the link learned its name.
    const selected = computer === own.key ? localKeys : computer ? keys.filter((key) => key === computer) : keys
    const [facts, local] = await Promise.all([this.db.summary(now, zone, period, scope, selected, keys), this.db.read<Coverage>("local")])
    const named = new Map(computers.map(({ key, name }) => [key, name]))
    for (const key of localKeys) named.set(key, own.name)
    const bots = new Map<string, { name: string; look?: unknown }>()
    for (const bot of [...this.directory.bots(), ...this.localBots()]) bots.set(bot.id, { name: bot.name, ...(bot.look ? { look: bot.look } : {}) })
    // (A cache rate well under the period's is said to be low, for a client to mark.)
    const overall = cacheRate(facts.totals)
    const measures = (of: Sums) => {
      const rate = cacheRate(of)
      return { cost: Math.round(of.cost * 1e6) / 1e6, tokens: of.tokens, cacheRate: rate, ...(rate !== null && overall !== null && rate <= overall - 0.15 ? { lowCache: true } : {}) }
    }
    const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`
    const title = (words: string | null) => words || "Untitled session"
    const conversation = (top: Conversation, alone: boolean) => ({
      kind: "session", id: top.session, name: title(top.title),
      ...(top.own ? { note: "Own chat" } : top.subagents ? { note: plural(top.subagents, "subagent") } : alone ? { note: "No bot" } : {}),
      ...measures(top),
    })
    const lookOf = (id: string) => {
      const look = bots.get(id)?.look as Record<string, unknown> | undefined
      return look && ["color", "shape", "face"].every((field) => typeof look[field] === "string")
        ? { look: { color: look.color, shape: look.shape, face: look.face } } : {}
    }
    const first = scope.session
      ? { key: "sessions", name: "Sessions", rows: (facts.inside ?? []).map((row) => ({ kind: "session", id: row.session, name: title(row.title), ...measures(row) })) }
      : scope.bot
        ? { key: "sessions", name: "Sessions", rows: facts.bots.flatMap((bot) => bot.conversations).map((top) => conversation(top, false)) }
        : { key: "bots", name: "Bots", rows: [
            ...facts.bots.map((bot) => {
              const overseen = bot.conversations.filter((top) => !top.own).length
              return {
                kind: "bot", id: bot.bot, name: bot.name ?? bots.get(bot.bot)?.name ?? "A bot", ...lookOf(bot.bot),
                ...(overseen ? { note: plural(overseen, "session") } : {}), ...measures(bot.sums),
                sessions: bot.conversations.slice(0, MOST).map((top) => conversation(top, false)),
              }
            }),
            ...facts.loose.map((top) => conversation(top, true)),
          ].sort((a, b) => b.cost - a.cost || b.tokens - a.tokens) }
    const lists = [
      first,
      { key: "providers", name: "Providers", rows: facts.providers.map((row) => ({ kind: "provider", id: row.provider, name: providerName(row.provider), provider: row.provider, ...measures(row) })) },
      { key: "models", name: "Models", rows: facts.models.map((row) => ({ kind: "model", id: row.key, name: row.name, provider: row.provider, ...measures(row) })) },
      { key: "computers", name: "Computers", rows: facts.computers.map((row) => ({ kind: "computer", id: row.computer, name: named.get(row.computer) ?? "Another computer", ...measures(row) })) },
    ].map((list) => ({ ...list, rows: list.rows.slice(0, MOST) }))
    const { totals, prior } = facts
    const change = prior && {
      cost: prior.cost > 0 ? (totals.cost - prior.cost) / prior.cost : null,
      tokens: prior.tokens > 0 ? (totals.tokens - prior.tokens) / prior.tokens : null,
      cacheRate: cacheRate(totals) !== null && cacheRate(prior) !== null ? cacheRate(totals)! - cacheRate(prior)! : null,
    }
    return {
      state: !local && this.scan ? "loading" : facts.sessions ? "ready" : "empty", period, zone,
      totals: measures(totals), change, trend: facts.trend, lists,
      standouts: facts.standouts.map((each) => ({ id: each.session, name: title(each.title), reason: each.reason, kind: each.kind, ...measures(each.sums) })),
    }
  }

  async view(params: URLSearchParams, route: "overview" | "sessions" | "detail") {
    const now = this.now()
    const period = Number(params.get("period") ?? 30)
    if (![7, 30, 90, 365].includes(period)) return
    const zone = knownZone(params.get("zone")) ?? ownZone()
    const filter: Filter = {}
    for (const name of ["computer", "bot", "account", "provider", "session"] as const) {
      const value = params.get(name)
      if (value) filter[name] = value
    }
    const offset = Number(params.get("offset") ?? 0)
    if (!validAt(offset) || offset > 1_000_000 || (route === "detail" && !filter.session)) return
    const localKey = this.local
    const own = this.directory.own() ?? { key: localKey, name: hostname() }
    const peers = this.directory.peerBoxes()
    const currentComputers = [own, ...peers]
    const computers = [...currentComputers, ...this.directory.insightOrigins()]
    const localKeys = own.key === localKey ? [own.key] : [own.key, localKey]
    const keys = [...new Set([...localKeys, ...computers.map(({ key }) => key)])]
    // A machine can have recorded work before the link learned its name.
    const selected = filter.computer === own.key ? localKeys : filter.computer ? keys.filter((key) => key === filter.computer) : keys
    delete filter.computer
    const data = await this.db.view(now, zone, period, filter, selected, route, offset, keys)
    if ("sessions" in data) {
      const botHomes = new Map(this.directory.bots().map((bot) => [bot.session, bot.box]))
      const sessions = data.sessions.map((session) => ({ ...session, home: botHomes.get(session.session) ?? session.computer }))
      return { ...data, sessions, localComputers: localKeys }
    }
    if ("calls" in data) {
      const origin = data.calls.findLast(call => localKeys.includes(call.computer) || this.directory.isPeer(call.computer))?.computer
      const home = this.conversationHome?.(filter.session!, origin && (localKeys.includes(origin) ? own.key : origin))
      const destination = home && (localKeys.includes(home) || this.directory.isPeer(home))
        ? { status: "available", computer: home }
        : { status: "unavailable" }
      return { ...data, destination, localComputers: localKeys }
    }
    const local = await this.db.read<Coverage>("local")
    const coverage = await Promise.all(currentComputers.filter(({ key }) => !params.has("computer") || params.get("computer") === key).map(async ({ key, name }) => {
      const read = key === own.key ? local : await this.db.read<Coverage>(`peer/${key}`)
      const issues = read?.issues.filter((issue) => !filter.provider || filter.provider === issue) ?? []
      return { key, name, scanned: read?.scanned, stale: read ? read.scanned < now - 2 * SCAN_EVERY : false, issues }
    }))
    const partial = data.totals.partial > 0 || coverage.some((entry) => !entry.scanned || entry.issues.length > 0) || await this.db.read("failed")
    const state = !local && this.scan ? "loading"
      : coverage.some((entry) => entry.stale) ? "stale"
      : partial ? "partial"
      : data.totals.sessions ? "ready" : "empty"
    return {
      ...data, computers, bots: await this.db.choices(keys), coverage, state, zone, period,
      updatedDay: local ? dayIn(local.scanned, zone) : null,
      scanning: !!this.scan, localComputers: localKeys,
      unavailable: data.providers.some((row) => row.name === "cursor" && row.partial > 0)
        ? ["Cursor API call counts where the run only reports context size"] : [],
    }
  }
}

type Coverage = { scanned: number; issues: string[] }
const json = (body: unknown, status = 200) => Response.json(body, { status })

export async function insightsRoutes(insights: Insights | undefined, method: string, path: string[], params: URLSearchParams) {
  if (!insights) return json({ message: "Insights is unavailable on this computer." }, 503)
  if (method === "POST" && path.length === 1 && path[0] === "refresh") {
    void insights.refresh(true)
    return json({ data: { scanning: true } }, 202)
  }
  if (method !== "GET") return json({ message: "Not found." }, 404)
  if (path.length === 1 && path[0] === "summary") {
    // (What the box last read for it at once, while it's read afresh: insights-kept.ts.)
    const data = insights.kept ? await insights.kept.answer(params, () => insights.summary(params)) : await insights.summary(params)
    return data ? json({ data }) : json({ message: "Choose a 7, 30 or 90 day period." }, 400)
  }
  const route = path.length === 0 ? (params.has("session") ? "detail" : "overview") :
    path[0] === "sessions" && path.length === 1 ? "sessions" :
    path[0] === "sessions" && path.length === 2 ? "detail" : undefined
  if (!route) return json({ message: "Not found." }, 404)
  if (path.length === 2) params.set("session", path[1]!)
  const data = await insights.view(params, route)
  return data ? json({ data }) : json({ message: "Choose a 7, 30, 90 or 365 day period and a valid page." }, 400)
}
