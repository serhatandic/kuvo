// Kuvo's persisted session metadata and isolated usage readers feed one observation per call.
import { join } from "node:path"
import type { Store } from "../store"
import type { Agent } from "../agent"
import { parseProvider } from "../accounts"
import type { AssistantMessage } from "../wire"
import { databasesStamp, engineUsageAnswers } from "./engine-db"
import { AntigravityHistory } from "./antigravity-history"
import { antigravitySources, claudeSources, codexSources, grokSources, insideUsageHome, ownComputer } from "./usage"
import { insightKey, type InsightRecord, type NativeUsage, type TokenBuckets } from "./insight-records"
import type { Pricing } from "./pricing"
import { numberOf } from "./numbers"

const nativeKey = { claude: "claudeSession", codex: "codexThread", grok: "acpSession", antigravity: "acpSession" } as const
type Bot = { id: string; name: string; sessionID: string }

function skillsNamed(tools: readonly (readonly [unknown, unknown])[]): string[] {
  const names = new Set<string>()
  for (const [tool, value] of tools) {
    if (typeof tool !== "string" || !value || typeof value !== "object") continue
    const input = value as Record<string, unknown>
    if (/^skill$/i.test(tool)) {
      const name = input.skill ?? input.name
      if (typeof name === "string" && name.length <= 200) names.add(name)
    }
    const path = input.file_path ?? input.filePath ?? input.path
    if (/^(read|read_file)$/i.test(tool) && typeof path === "string") {
      const name = /(?:^|[/\\])skills[/\\]([^/\\]+)[/\\]SKILL\.md$/i.exec(path)?.[1]
      if (name && name.length <= 200) names.add(name)
    }
  }
  return [...names].sort().slice(0, 32)
}

export function observedSkills(step: AssistantMessage): string[] {
  return skillsNamed(step.content.flatMap((part) =>
    part.type === "tool" && part.state.status === "completed" ? [[part.name, part.state.input] as const] : []))
}

function seatBot(metadata: unknown): string | undefined {
  try {
    const value = typeof metadata === "string" ? JSON.parse(metadata) : metadata
    const bot = value?.kuvo?.seat?.bot
    return typeof bot === "string" ? bot : undefined
  } catch {
    return undefined
  }
}

const count = (value: unknown) => {
  const n = numberOf(value)
  return n !== undefined && Number.isSafeInteger(n) && n >= 0 ? n : null
}

/** Saved bridge and engine steps use disjoint cache and reasoning buckets. */
function buckets(given: unknown): TokenBuckets | undefined {
  if (!given || typeof given !== "object") return
  const raw = given as { input?: unknown; output?: unknown; reasoning?: unknown; cache?: { read?: unknown; write?: unknown } }
  const output = count(raw.output)
  return {
    input: count(raw.input), output: output === null ? null : output + (count(raw.reasoning) ?? 0),
    cacheRead: count(raw.cache?.read), cacheWrite5m: count(raw.cache?.write), cacheWrite1h: 0,
  }
}

function automated(metadata?: Record<string, unknown>) {
  return typeof metadata?.source === "string" && ["agent", "group", "queue", "wake", "system"].includes(metadata.source)
}

export class InsightsScan {
  private issues = new Set<string>()
  private native = new Map<string, NativeUsage[]>()
  private engine?: { stamp: string; since: number; records: InsightRecord[] }
  private sources: ReturnType<typeof claudeSources>[]

  constructor(private options: {
    root: string
    cache: string
    agents: () => Agent[]
    store: Store
    bots: () => Bot[]
    computer: () => string
    engine?: string
  }) {
    const reader = {
      owns: (path: string) => insideUsageHome(options.root, path),
      problem: (provider: string) => this.issues.add(provider),
      take: (provider: string, rows: NativeUsage[]) => { this.native.set(provider, rows) },
    }
    this.sources = [
      claudeSources(options.cache, ownComputer, reader),
      codexSources(options.cache, undefined, ownComputer, reader),
      grokSources(options.cache, ownComputer, reader),
      antigravitySources(options.cache, ownComputer, new AntigravityHistory(), reader),
    ]
  }

  async scan(pricing: Pricing, since: number) {
    const agents = this.options.agents().filter((agent) => agent.instance.home || parseProvider(agent.providerID)?.kind === "antigravity")
    const sources = this.sources.flatMap((list) => list(agents))
    this.issues.clear()
    const issues = this.issues
    this.native.clear()
    try {
      for (const source of sources) {
        try { await source.slots(pricing, since) }
        catch { issues.add(source.service) }
      }
    } finally {
      for (const files of new Set(sources.map((source) => source.files))) files?.endRound(issues.size === 0)
    }
    const botOf = new Map(this.options.bots().map((bot) => [bot.sessionID, bot]))
    const local = this.options.computer()
    const records: InsightRecord[] = []
    const taken = new Set<string>()
    const native = new Map<string, NativeUsage[]>()
    for (const [provider, rows] of this.native) {
      for (const row of rows) {
        if (row.at < since) continue
        const key = insightKey(provider, row.session)
        let list = native.get(key)
        if (!list) native.set(key, list = [])
        list.push(row)
      }
    }
    for (const list of native.values()) list.sort((a, b) => a.at - b.at)
    for (const session of this.options.store.all()) {
      const record = this.options.store.get(session.id)
      if (!record || session.time.updated < since) continue
      const seat = seatBot(session.metadata)
      const bot = botOf.get(session.id) ?? botOf.get(session.parentID ?? "")
      const base = { session: session.id, title: session.title, bot: bot?.id ?? seat, botName: bot?.name }
      for (const message of record.messages) {
        if (message.type !== "assistant" && message.type !== "user") continue
        const at = message.time.completed ?? message.time.created
        if (at < since) continue
        const model = message.type === "assistant" ? message.model : session.model
        const accountProvider = model?.providerID ?? "engine"
        const kind = parseProvider(accountProvider)?.kind
        const provider = kind ?? accountProvider
        const computer = message.time.origin ?? local
        const fields = { ...base, computer, provider, account: message.time.usageAccount ?? `${computer}/${accountProvider}`, at,
          ...(message.time.usageAccount ? { accountObserved: true as const } : {}),
          ...(message.time.origin ? { attributed: true as const } : {}) }
        if (message.time.usageSession && message.time.usageSession !== session.id) continue
        const key = insightKey("kuvo", session.id, message.id)
        if (message.type === "user") {
          if (!session.parentID && !seat && !automated(message.metadata)) records.push({ ...fields, key, call: message.id, human: true })
          continue
        }
        if (!message.time.completed) continue
        const skills = observedSkills(message)
        const step = { ...fields, call: message.id, key, model: model?.id, agent: message.agent, skills }
        // Cursor's SDK exposes the last context size for a run. It's not the run's API usage.
        const tokens = kind === "cursor" ? undefined : buckets(message.tokens)
        const field = kind && kind in nativeKey ? nativeKey[kind as keyof typeof nativeKey] : undefined
        const threads = record.agent.threads as Record<string, { state?: Record<string, unknown> }> | undefined
        const id = field && ((threads?.[accountProvider]?.state ?? record.agent)[field])
        const candidates = typeof id === "string" ? native.get(insightKey(accountProvider, id)) : undefined
        if (candidates?.length) {
          // Use the step's execution window to tie this observation to the computer that ran it.
          // Each native event is claimed once even when the same session has resumed many times.
          let pos = lowerBound(candidates, message.time.created - 1000)
          let matched = false
          while (pos < candidates.length && candidates[pos]!.at <= at + 1000) {
            const row = candidates[pos++]!
            const nativeID = insightKey(provider, row.session, row.call)
            if (taken.has(nativeID)) continue
            taken.add(nativeID)
            matched = true
            records.push({ ...step, ...row, supersedes: key, session: session.id, nativeSession: row.session, key: nativeID })
          }
          if (matched) continue
        }
        records.push({ ...step, tokens, ...(!tokens ? { partial: true as const } : {}) })
      }
    }
    if (this.options.engine) {
      try {
        const read = await this.engineRecords(this.options.engine, since, botOf)
        records.push(...read.records)
        if (read.partial) issues.add("opencode")
      } catch { issues.add("opencode") }
    } else { issues.add("opencode") }
    return { records, issues: [...issues] }
  }

  private async engineRecords(folder: string, since: number, bots: Map<string, Bot>) {
    const stamp = databasesStamp(folder)
    if (this.engine?.stamp === stamp && this.engine.since === since) return { records: this.engine.records }
    const records: InsightRecord[] = []
    const answers = await engineUsageAnswers(folder, since)
    const partial = answers?.some((answer) => answer.failed !== undefined)
    for (const answer of answers ?? []) {
      if (!answer.json) continue
      const rows: unknown[][] = JSON.parse(answer.json)
      for (const [session, call, role, created, completed, rawProvider, model, agent, usage, source, origin, observedAccount, title, metadata, parent, parts] of rows) {
        const at = completed ?? created
        if (typeof session !== "string" || !session || typeof call !== "string" || typeof at !== "number" || at < since) continue
        const seat = seatBot(metadata)
        if (role === "user" && (parent || seat || automated({ source }))) continue
        if (role === "assistant" && completed === null && !usage) continue
        const provider = typeof rawProvider === "string" ? rawProvider : "opencode"
        const computer = typeof origin === "string" ? origin : this.options.computer()
        const account = typeof observedAccount === "string" && observedAccount ? observedAccount : undefined
        const bot = bots.get(session)
        const tokens = role === "user" ? undefined : buckets(usage)
        records.push({
          key: insightKey("engine", session, call), session, call, at, computer, provider,
          account: account ?? `${computer}/${provider}`, ...(account ? { accountObserved: true as const } : {}), bot: bot?.id ?? seat, botName: bot?.name, title: typeof title === "string" ? title : undefined,
          ...(Array.isArray(parts) ? { skills: skillsNamed(parts.filter(Array.isArray) as [unknown, unknown][]) } : {}),
          model: typeof model === "string" ? model : undefined, agent: typeof agent === "string" ? agent : undefined,
          tokens, reportedTotal: usage && typeof usage === "object" ? count((usage as { total?: unknown }).total) ?? undefined : undefined,
          ...(role === "user" ? { human: true as const } : !tokens ? { partial: true as const } : {}),
        })
      }
    }
    if (!partial) this.engine = { stamp, since, records }
    return { records, partial }
  }
}

function lowerBound(rows: NativeUsage[], at: number) {
  let lo = 0, hi = rows.length
  while (lo < hi) {
    const middle = (lo + hi) >>> 1
    if (rows[middle]!.at < at) lo = middle + 1
    else hi = middle
  }
  return lo
}
