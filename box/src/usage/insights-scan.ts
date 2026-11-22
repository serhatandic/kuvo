// Kuvo's persisted session metadata and isolated usage readers feed one observation per call.
import { join } from "node:path"
import type { Record_, Store } from "../agents/store"
import type { UsageAgent } from "./accounts"
import { parseProvider } from "../agents/accounts"
import type { AssistantMessage } from "../agents/wire"
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

function kuvoOf(metadata: unknown): Record<string, any> | undefined {
  try {
    const value = typeof metadata === "string" ? JSON.parse(metadata) : metadata
    const kuvo = value?.kuvo
    return kuvo && typeof kuvo === "object" ? kuvo : undefined
  } catch {
    return undefined
  }
}

function seatBot(metadata: unknown): string | undefined {
  const bot = kuvoOf(metadata)?.seat?.bot
  return typeof bot === "string" ? bot : undefined
}

/** What a conversation says of where it sits: its parent (a subagent's), who started it, and whose it is. */
export type LineNode = { parent?: string; starters?: string[]; bot?: string; botName?: string; own?: true }
export type Lineage = { root: string; bot?: string; botName?: string; own?: true }

/** A conversation's place, from its session and its marks (`kuvo.seat`, `kuvo.by`, `kuvo.startedBy`). */
export function lineNode(session: { id: string; parentID?: unknown; metadata?: unknown }, bots: Map<string, Bot>, names: Map<string, string>): LineNode {
  const kuvo = kuvoOf(session.metadata)
  const parent = typeof session.parentID === "string" && session.parentID ? session.parentID : undefined
  const startedBy = typeof kuvo?.startedBy === "string" && kuvo.startedBy ? kuvo.startedBy : undefined
  const ancestors = Array.isArray(kuvo?.line) ? kuvo.line.slice(-20).filter((id): id is string => typeof id === "string" && !!id).reverse() : []
  const starters = startedBy ? [startedBy, ...ancestors.filter((id) => id !== startedBy)] : ancestors
  const line = { ...(parent ? { parent } : {}), ...(starters.length ? { starters } : {}) }
  const own = bots.get(session.id)
  if (own) return { ...line, bot: own.id, botName: own.name, own: true }
  const seat = seatBot(session.metadata)
  if (seat) return { ...line, bot: seat, botName: names.get(seat), own: true }
  const by = kuvo?.by
  if (by && typeof by.bot === "string" && by.bot) return { ...line, bot: by.bot, botName: typeof by.name === "string" && by.name || names.get(by.bot), own: undefined }
  return line
}

/**
 * Each conversation's top-level one (up its subagents' parents, as far as they're known) and the
 * bot over it: the top-level session's own bot or the bot over the conversation that started it.
 */
export function lineages(nodes: Map<string, LineNode>) {
  const roots = new Map<string, string>()
  const rootOf = (id: string) => {
    let root = id
    const visited = new Set<string>()
    while (true) {
      const kept = roots.get(root)
      if (kept) { root = kept; break }
      if (visited.has(root)) {
        const path = [...visited]
        root = path.slice(path.indexOf(root)).sort()[0]!
        break
      }
      const parent = nodes.get(root)?.parent
      if (!parent) break
      visited.add(root)
      root = parent
    }
    for (const session of visited) roots.set(session, root)
    return root
  }
  const owners = new Map<string, LineNode>()
  const ownerOf = (root: string, path: Set<string>): LineNode | undefined => {
    const top = owners.get(root) ?? nodes.get(root)
    if (top?.bot) return top
    if (!top?.starters || path.has(root)) return
    path.add(root)
    for (const starter of top.starters) {
      const owner = ownerOf(rootOf(starter), path)
      if (owner) {
        path.delete(root)
        owners.set(root, owner)
        return owner
      }
    }
    path.delete(root)
    return undefined
  }
  return (id: string): Lineage => {
    const root = rootOf(id)
    const owner = ownerOf(root, new Set())
    return { root, ...(owner?.bot ? { bot: owner.bot } : {}), ...(owner?.botName ? { botName: owner.botName } : {}),
      ...(nodes.get(root)?.own ? { own: true as const } : {}) }
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
  private engine?: { stamp: string; since: number; records: InsightRecord[]; sessions: Map<string, { parentID?: string; metadata?: unknown }> }
  private sources: ReturnType<typeof claudeSources>[]

  constructor(private options: {
    root: string
    cache: string
    agents: () => UsageAgent[]
    store: Pick<Store, "all" | "get"> | (() => Iterable<Record_>)
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
    const bots = this.options.bots()
    const botOf = new Map(bots.map((bot) => [bot.sessionID, bot]))
    const botNames = new Map(bots.map((bot) => [bot.id, bot.name]))
    const nodes = new Map(bots.map(({ sessionID, id, name }) => [sessionID, { bot: id, botName: name, own: true } as LineNode]))
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
    const store = this.options.store
    const recordsFromStore = typeof store === "function" ? store() : store.all().flatMap(session => store.get(session.id) ?? [])
    for (const record of recordsFromStore) {
      const session = record.session
      // (Kept for its place in a line even when its own calls are older.)
      nodes.set(session.id, lineNode(session, botOf, botNames))
      if (session.time.updated < since) continue
      const seat = seatBot(session.metadata)
      const base = { session: session.id, title: session.title }
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
        const read = await this.engineRecords(this.options.engine, since)
        records.push(...read.records)
        for (const [id, session] of read.sessions) {
          nodes.set(id, { ...lineNode({ id, ...session }, botOf, botNames), ...nodes.get(id) })
        }
        if (read.partial) issues.add("opencode")
      } catch { issues.add("opencode") }
    } else { issues.add("opencode") }
    const lineOf = lineages(nodes)
    const lined = records.map(({ bot: _bot, botName: _name, root: _root, botOwn: _own, ...record }): InsightRecord => {
      const line = lineOf(record.session)
      return {
        ...record, ...(line.bot ? { bot: line.bot } : {}), ...(line.botName ? { botName: line.botName } : {}),
        ...(line.root !== record.session ? { root: line.root } : {}), ...(line.own ? { botOwn: true as const } : {}),
      }
    })
    const lineage = [...nodes.keys()].flatMap((id): [string, Lineage][] => {
      const line = lineOf(id)
      return line.bot || line.root !== id ? [[id, line]] : []
    })
    return { records: lined, lineage, issues: [...issues] }
  }

  private async engineRecords(folder: string, since: number) {
    const stamp = databasesStamp(folder)
    if (this.engine?.stamp === stamp && this.engine.since === since) return { records: this.engine.records, sessions: this.engine.sessions }
    const records: InsightRecord[] = []
    const sessions = new Map<string, { parentID?: string; metadata?: unknown }>()
    const answers = await engineUsageAnswers(folder, since)
    const partial = answers?.some((answer) => answer.failed !== undefined)
    for (const answer of answers ?? []) {
      if (!answer.json) continue
      const channel = new Map<string, { parentID?: string; metadata?: unknown }>()
      for (const [id, parent, metadata] of answer.sessions ?? []) {
        channel.set(id, { ...channel.get(id), ...(parent ? { parentID: parent } : {}), ...(metadata ? { metadata } : {}) })
      }
      for (const [id, session] of channel) sessions.set(id, { ...sessions.get(id), ...session })
      const rows: unknown[][] = JSON.parse(answer.json)
      for (const [session, call, role, created, completed, rawProvider, model, agent, usage, source, origin, observedAccount, title, parts] of rows) {
        const at = completed ?? created
        if (typeof session !== "string" || !session || typeof call !== "string" || typeof at !== "number" || at < since) continue
        const saved = channel.get(session)
        if (role === "user" && (saved?.parentID || seatBot(saved?.metadata) || automated({ source }))) continue
        if (role === "assistant" && completed === null && !usage) continue
        const provider = typeof rawProvider === "string" ? rawProvider : "opencode"
        const computer = typeof origin === "string" ? origin : this.options.computer()
        const account = typeof observedAccount === "string" && observedAccount ? observedAccount : undefined
        const tokens = role === "user" ? undefined : buckets(usage)
        records.push({
          key: insightKey("engine", session, call), session, call, at, computer, provider,
          ...(typeof origin === "string" && origin ? { attributed: true as const } : {}),
          account: account ?? `${computer}/${provider}`, ...(account ? { accountObserved: true as const } : {}), title: typeof title === "string" ? title : undefined,
          ...(Array.isArray(parts) ? { skills: skillsNamed(parts.filter(Array.isArray) as [unknown, unknown][]) } : {}),
          model: typeof model === "string" ? model : undefined, agent: typeof agent === "string" ? agent : undefined,
          tokens, reportedTotal: usage && typeof usage === "object" ? count((usage as { total?: unknown }).total) ?? undefined : undefined,
          ...(role === "user" ? { human: true as const } : !tokens ? { partial: true as const } : {}),
        })
      }
    }
    if (!partial) this.engine = { stamp, since, records, sessions }
    return { records, sessions, partial }
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
