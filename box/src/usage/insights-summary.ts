// Insights' summary: what a period's usage comes to, the period before it, and where it went, by
// bot, provider, model and computer. Every grouping is made here; clients draw it as it comes.
import { modelName } from "./insights-names"

/** Cost, tokens (cache reads included) and input (cached, uncached and cache writes) summed. */
export type Sums = { cost: number; tokens: number; cached: number; input: number }
export type SessionSums = Sums & { session: string; title: string | null; bot: string | null; botName: string | null; root: string | null; own: number | null }
export type ModelSums = Sums & { provider: string; model: string | null }
export type ComputerSums = Sums & { computer: string }

/** A top-level conversation with its subagents' calls counted in. */
export type Conversation = Sums & { session: string; title: string | null; bot: string | null; botName: string | null; own: boolean; subagents: number }

export type SummaryFacts = {
  sessions: number
  totals: Sums
  /** The period of the same length before; null where nothing was recorded then. */
  prior: Sums | null
  trend: { day: string; cost: number; tokens: number }[]
  bots: { bot: string; name: string | null; sums: Sums; conversations: Conversation[] }[]
  /** Top-level conversations no bot is over. */
  loose: Conversation[]
  /** Narrowed to one conversation: it and its subagents, each on its own. */
  inside?: (Sums & { session: string; title: string | null })[]
  providers: (Sums & { provider: string })[]
  models: (Sums & { key: string; name: string; provider: string })[]
  computers: ComputerSums[]
  standouts: { session: string; title: string | null; reason: string; kind: "cost" | "cache" | "subagents"; sums: Sums }[]
}

export const SUMS = `COALESCE(SUM(cost),0) AS cost, COALESCE(SUM(processed),0) AS tokens, COALESCE(SUM(cache_read),0) AS cached,
  COALESCE(SUM(input),0) + COALESCE(SUM(cache_read),0) + COALESCE(SUM(cache_write),0) AS input`

const zero = (): Sums => ({ cost: 0, tokens: 0, cached: 0, input: 0 })
const add = (into: Sums, from: Sums) => {
  into.cost += from.cost
  into.tokens += from.tokens
  into.cached += from.cached
  into.input += from.input
  return into
}
const sums = ({ cost, tokens, cached, input }: Sums): Sums => ({ cost, tokens, cached, input })
const byCost = <T extends Sums>(a: T, b: T) => b.cost - a.cost || b.tokens - a.tokens

/** Cached input over all input; null where no input was counted. */
export const cacheRate = (of: Sums) => of.input > 0 ? Math.min(1, of.cached / of.input) : null

/** At most this many rows a list, and conversations a bot. */
export const MOST = 200

/** Each conversation's calls under its top-level one, which a bot's row and its sessions are made of. */
export function conversations(rows: SessionSums[], titles: Map<string, string>): Conversation[] {
  const tops = new Map<string, Conversation>()
  for (const row of rows) {
    const id = row.root ?? row.session
    let top = tops.get(id)
    if (!top) tops.set(id, top = { ...zero(), session: id, title: null, bot: null, botName: null, own: false, subagents: 0 })
    add(top, row)
    if (row.session === id) {
      top.title = row.title ?? top.title
      top.own = top.own || !!row.own
    } else top.subagents++
    top.bot ??= row.bot
    top.botName ??= row.botName
  }
  for (const top of tops.values()) top.title ??= titles.get(top.session) ?? null
  return [...tops.values()].sort(byCost)
}

export function summarize(input: {
  rows: SessionSums[]; titles: Map<string, string>; models: ModelSums[]; computers: ComputerSums[]
  prior: Sums & { calls: number }; trend: SummaryFacts["trend"]; session?: string
}): SummaryFacts {
  const totals = input.rows.reduce((all, row) => add(all, row), zero())
  const tops = conversations(input.rows, input.titles)
  const bots = new Map<string, SummaryFacts["bots"][number]>()
  const loose: Conversation[] = []
  for (const top of tops) {
    if (!top.bot) {
      loose.push(top)
      continue
    }
    let bot = bots.get(top.bot)
    if (!bot) bots.set(top.bot, bot = { bot: top.bot, name: top.botName, sums: zero(), conversations: [] })
    bot.name ??= top.botName
    add(bot.sums, top)
    bot.conversations.push(top)
  }
  const providers = new Map<string, Sums & { provider: string }>()
  const models = new Map<string, Sums & { key: string; name: string; provider: string; lead: number }>()
  const byProvider = new Map<string, number>()
  for (const row of input.models) {
    add(providers.get(row.provider) ?? providers.set(row.provider, { ...zero(), provider: row.provider }).get(row.provider)!, row)
    const { key, name } = modelName(row.model)
    const model = models.get(key) ?? models.set(key, { ...zero(), key, name, provider: row.provider, lead: -1 }).get(key)!
    add(model, row)
    // (Its mark: the provider most of its cost went through.)
    const through = (byProvider.get(key + "\n" + row.provider) ?? 0) + row.cost + row.tokens / 1e9
    byProvider.set(key + "\n" + row.provider, through)
    if (through > model.lead) {
      model.lead = through
      model.provider = row.provider
    }
  }
  return {
    sessions: input.rows.length,
    totals,
    prior: input.prior.calls ? sums(input.prior) : null,
    trend: input.trend,
    bots: [...bots.values()].sort((a, b) => byCost(a.sums, b.sums)),
    loose,
    ...(input.session ? { inside: input.rows.map((row) => ({ ...sums(row), session: row.session, title: row.title ?? input.titles.get(row.session) ?? null })).sort(byCost) } : {}),
    providers: [...providers.values()].sort(byCost),
    models: [...models.values()].map(({ lead: _, ...model }) => model).sort(byCost),
    computers: [...input.computers].sort(byCost),
    standouts: standouts(tops.filter((top) => !top.own), totals),
  }
}

/**
 * At most three conversations that stand out, each for one reason: the one that cost most, one
 * whose cache did much worse than the period's, and the one with the most subagents. Only once
 * there are enough to stand out among.
 */
export function standouts(tops: Conversation[], totals: Sums): SummaryFacts["standouts"] {
  if (tops.length < 4) return []
  const picked: SummaryFacts["standouts"] = []
  const taken = (top: Conversation) => picked.some((each) => each.session === top.session)
  const pick = (top: Conversation, reason: string, kind: "cost" | "cache" | "subagents") =>
    picked.push({ session: top.session, title: top.title, reason, kind, sums: sums(top) })
  const costly = tops.reduce((most, top) => top.cost > most.cost ? top : most)
  if (costly.cost > 0) pick(costly, "Highest cost", "cost")
  // (One that cost at least a middling session's: a cold one costs more per token, and has fewer of them.)
  const overall = cacheRate(totals)
  const middle = [...tops].sort((a, b) => a.cost - b.cost)[Math.floor(tops.length / 2)]!.cost
  const poor = tops.filter((top) => !taken(top) && top.cost >= middle && overall !== null && (cacheRate(top) ?? 1) <= overall - 0.15)
    .sort((a, b) => cacheRate(a)! - cacheRate(b)!)[0]
  if (poor) pick(poor, "Low cache", "cache")
  const busy = tops.filter((top) => !taken(top) && top.subagents >= 3).sort((a, b) => b.subagents - a.subagents || b.cost - a.cost)[0]
  if (busy) pick(busy, `${busy.subagents} subagents`, "subagents")
  return picked
}
