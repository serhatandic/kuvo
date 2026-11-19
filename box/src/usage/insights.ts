// Insights serves a local SQLite index and exchanges bounded pages of usage metadata over peer mail.
import { hostname } from "node:os"
import type { LinkDirectory, PeerPost } from "../agents/groups/post"
import { dayIn, knownZone, ownZone } from "./history"
import { InsightsDB, type Filter } from "./insights-db"
import type { InsightRecord } from "./insight-records"
import type { InsightsScan } from "./insights-scan"
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
  const { key, at, session, nativeSession, call, provider, computer, account, bot, botName, model, pricingModels, agent, title, skills, supersedes, reportedTotal, observedAt } = value
  return {
    key, at, session, call, provider, computer, pricingModels, skills, supersedes, reportedTotal,
    account: optionalText(account), bot: optionalText(bot), botName: optionalText(botName), model: optionalText(model),
    agent: optionalText(agent), title: optionalText(title), nativeSession: optionalText(nativeSession),
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
  private scan?: Promise<void>
  private timer?: ReturnType<typeof setInterval>
  private refreshed?: number
  private requested = new Map<string, number>()
  private epoch?: Promise<string>
  private mail = Promise.resolve()

  constructor(
    private index: InsightsDB,
    private scanner: InsightsScan,
    private pricing: PricingStore,
    private directory: LinkDirectory,
    private post: PeerPost,
    private local: string,
    private now = Date.now,
  ) {
    this.directory.listen(() => this.sync())
    this.post.onFollowed(() => this.sync())
  }

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
      await this.db.take(records.slice(offset, offset + 800), this.pricing.current(), now)
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

  private sync(force = false) {
    const now = this.now()
    const fresh = force && (this.refreshed === undefined || now - this.refreshed > 30_000)
    if (fresh) this.refreshed = now
    for (const peer of this.directory.peerBoxes()) {
      if (!fresh && now - (this.requested.get(peer.key) ?? -Infinity) < 30_000) continue
      void this.pull(peer.key, fresh).catch((error) => console.error("Insights peer sync:", error))
    }
  }

  private async pull(to: string, refresh = false) {
    this.requested.set(to, this.now())
    this.post.send(to, { kind: "insights", action: "pull", ...await this.db.cursor(to), ...(refresh ? { refresh: true } : {}) }, false)
  }

  private async sendPage(to: string, after: number, epoch?: string) {
    const [ownEpoch, current, coverage] = await Promise.all([this.ownEpoch(), this.db.read<number>("sequence"), this.db.read<Coverage>("local")])
    // A paired computer may have rebuilt its index from scratch.
    const reset = (epoch !== undefined && epoch !== ownEpoch) || after > (current ?? 0)
    const batch = await this.db.batch(reset ? 0 : after)
    this.post.send(to, { kind: "insights", action: "page", after, ...batch, epoch: ownEpoch, ...(reset ? { reset: true } : {}), records: batch.records.map((record) => {
      const key = this.directory.own()?.key
      if (!key || record.computer !== this.local) return record
      return { ...record, computer: key, account: record.account?.replace(this.local + "/", key + "/") }
    }), ...coverage }, false)
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
    const { after: expected, epoch: priorEpoch } = await this.db.cursor(from)
    if (mail.after !== expected) return
    if (priorEpoch && priorEpoch !== mail.epoch && !mail.reset) return
    const own = this.directory.own()?.key ?? this.local
    const records = mail.records.map(recordFrom).filter((record): record is InsightRecord =>
      !!record && (record.computer === own || this.directory.isPeer(record.computer)))
    const pricing = this.pricing.current()
    await this.db.take(records, pricing, this.now(), from, mail.next, mail.epoch)
    if (!mail.more && validAt(mail.scanned) && mail.scanned <= this.now() + DAY) await this.db.write(`peer/${from}`, {
      scanned: mail.scanned,
      issues: Array.isArray(mail.issues) ? mail.issues.filter(small).slice(0, 12) : [],
    })
    if (mail.more && (mail.reset || mail.next > expected)) await this.pull(from)
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
    const own = this.directory.own() ?? { key: this.local, name: hostname() }
    const peers = this.directory.peerBoxes()
    const computers = [own, ...peers]
    const localKeys = own.key === this.local ? [own.key] : [own.key, this.local]
    const keys = [...localKeys, ...peers.map(({ key }) => key)]
    // A machine can have recorded work before the link learned its name.
    const selected = filter.computer === own.key ? localKeys : filter.computer ? keys.filter((key) => key === filter.computer) : keys
    delete filter.computer
    const data = await this.db.view(now, zone, period, filter, selected, route, offset)
    if ("sessions" in data) {
      const botHomes = new Map(this.directory.bots().map((bot) => [bot.session, bot.box]))
      const sessions = data.sessions.map((session) => ({ ...session, home: botHomes.get(session.session) ?? session.computer }))
      return { ...data, sessions, localComputers: localKeys }
    }
    if ("calls" in data) return { ...data, localComputers: localKeys }
    const local = await this.db.read<Coverage>("local")
    const coverage = await Promise.all(computers.filter(({ key }) => !params.has("computer") || params.get("computer") === key).map(async ({ key, name }) => {
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
  const route = path.length === 0 ? (params.has("session") ? "detail" : "overview") :
    path[0] === "sessions" && path.length === 1 ? "sessions" :
    path[0] === "sessions" && path.length === 2 ? "detail" : undefined
  if (!route) return json({ message: "Not found." }, 404)
  if (path.length === 2) params.set("session", path[1]!)
  const data = await insights.view(params, route)
  return data ? json({ data }) : json({ message: "Choose a 7, 30, 90 or 365 day period and a valid page." }, 400)
}
