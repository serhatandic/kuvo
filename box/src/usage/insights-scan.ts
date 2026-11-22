// Kuvo's persisted session metadata and isolated usage readers feed one observation per call.
import { join } from "node:path"
import { statSync } from "node:fs"
import { setImmediate as nextTurn } from "node:timers/promises"
import { transcriptDatabase } from "../agents/transcript"
import type { Record_, Store } from "../agents/store"
import type { UsageAgent } from "./accounts"
import { parseProvider } from "../agents/accounts"
import type { AssistantMessage } from "../agents/wire"
import { engineDatabases, engineUsageAnswers, type EngineWatermarks } from "./engine-db"
import { AntigravityHistory, type CachedDatabase } from "./antigravity-history"
import { antigravitySources, claudeSources, codexSources, grokSources, insideUsageHome, ownComputer } from "./usage"
import { insightKey, type InsightRecord, type NativeUsage, type TokenBuckets } from "./insight-records"
import type { Pricing } from "./pricing"
import { numberOf } from "./numbers"

const DECODE_BUDGET = 8 * 1024 * 1024
const nativeKey = { claude: "claudeSession", codex: "codexThread", grok: "acpSession", antigravity: "acpSession" } as const
export type InsightBot = { id: string; name: string; sessionID: string }

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
export function lineNode(session: { id: string; parentID?: unknown; metadata?: unknown }, bots: Map<string, InsightBot>, names: Map<string, string>): LineNode {
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

type EngineSessions = [string, { parentID?: string; metadata?: unknown }][]
type EngineCheckpoint = { since: number; identity: string; revision: string; sessions: EngineSessions; watermarks?: EngineWatermarks }

type Conversation = Omit<Record_, "messages"> & { folder?: string; messages: Iterable<Record_["messages"][number]> }

export interface ScanIndex {
  read<T>(key: string): T | undefined
  write(entries: readonly (readonly [string, unknown])[]): void
  reclaim(): void
  take(records: InsightRecord[]): void
  attribute(lineage: [string, Lineage][]): void
}

export function fileStamp(file: string) {
  try {
    const stat = statSync(file, { bigint: true })
    return { identity: `${stat.dev}/${stat.ino}/${stat.birthtimeNs}`, revision: `${stat.size}/${stat.mtimeNs}/${stat.ctimeNs}`, size: Number(stat.size) }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error
    return undefined
  }
}

function conversationStamp(folder: string) {
  const sqlite = fileStamp(join(folder, transcriptDatabase))
  return sqlite ? [sqlite, fileStamp(join(folder, transcriptDatabase + "-wal"))]
    : [fileStamp(join(folder, "messages.json"))]
}

function nativeThreads(record: Conversation, providers: Iterable<string>) {
  const found = new Set<string>()
  const state = (provider: string, saved: Record<string, unknown>) => {
    const kind = parseProvider(provider)?.kind
    const key = kind && kind in nativeKey ? nativeKey[kind as keyof typeof nativeKey] : undefined
    if (key && typeof saved[key] === "string") found.add(insightKey(provider, saved[key]))
  }
  const threads = record.agent.threads as Record<string, { state?: Record<string, unknown> }> | undefined
  for (const [provider, thread] of Object.entries(threads ?? {})) state(provider, thread?.state ?? record.agent)
  for (const provider of providers) state(provider, threads?.[provider]?.state ?? record.agent)
  return found
}

export class InsightsScan {
  private issues = new Set<string>()
  private native = new Map<string, NativeUsage[]>()
  private antigravity = new AntigravityHistory()
  private sources: ReturnType<typeof claudeSources>[]

  constructor(private options: {
    root: string
    cache: string
    agents: () => UsageAgent[]
    store: Pick<Store, "all" | "get"> | (() => Iterable<Conversation>)
    bots: () => InsightBot[]
    computer: () => string
    engine?: string
  }) {
    const reader = {
      owns: (path: string) => insideUsageHome(options.root, path),
      has: (provider: string) => this.native.has(provider),
      problem: (provider: string) => this.issues.add(provider),
      take: (provider: string, rows: NativeUsage[]) => { this.native.set(provider, rows) },
    }
    this.sources = [
      claudeSources(options.cache, ownComputer, reader),
      codexSources(options.cache, undefined, ownComputer, reader),
      grokSources(options.cache, ownComputer, reader),
      antigravitySources(options.cache, ownComputer, this.antigravity, reader),
    ]
  }

  async scan(pricing: Pricing, since: number, index?: ScanIndex) {
    const agents = this.options.agents().filter((agent) => agent.instance.home || parseProvider(agent.providerID)?.kind === "antigravity")
    const sources = this.sources.flatMap((list) => list(agents))
    this.antigravity.checkpoints = index && {
      read: path => index.read<CachedDatabase>("scan/antigravity-v1/" + insightKey(path)),
      write: (path, value) => index.write([["scan/antigravity-v1/" + insightKey(path), value]]),
    }
    this.issues.clear()
    const issues = this.issues
    const providers = new Set(sources.map(source => source.provider))
    for (const provider of this.native.keys()) if (!providers.has(provider)) this.native.delete(provider)
    try {
      for (const source of sources) {
        try { await source.slots(pricing, since) }
        catch { issues.add(source.service) }
      }
    } finally {
      this.antigravity.checkpoints = undefined
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
    const conversations = typeof store === "function" ? [...store()] : store.all().flatMap(session => store.get(session.id) ?? [])
    for (const { session } of conversations) nodes.set(session.id, lineNode(session, botOf, botNames))
    const engineCheckpoints: [string, EngineCheckpoint][] = []
    let engineRecords: InsightRecord[] = []
    if (this.options.engine) {
      try {
        const folder = this.options.engine
        const selected = new Map<string, { key: string; identity: string; revision: string }>()
        const after: Record<string, EngineWatermarks> = {}
        const add = (sessions: EngineSessions) => {
          for (const [id, session] of sessions) {
            nodes.set(id, { ...lineNode({ id, ...session }, botOf, botNames), ...nodes.get(id) })
          }
        }
        if (index) for (const path of engineDatabases(folder)) {
          const file = fileStamp(path)
          if (!file) continue
          const key = "scan/engine-v2/" + insightKey(path)
          const identity = insightKey(file.identity, local)
          const revision = insightKey(file.revision, fileStamp(path + "-wal"))
          const saved = index.read<EngineCheckpoint>(key)
          if (saved?.identity === identity && saved.since <= since && saved.revision === revision) add(saved.sessions)
          else {
            if (saved?.identity === identity && saved.since <= since && saved.watermarks) after[path] = saved.watermarks
            selected.set(path, { key, identity, revision })
          }
        }
        if (!index || selected.size) {
          const read = await this.engineRecords(folder, since, index ? after : undefined, index ? new Set(selected.keys()) : undefined)
          engineRecords = read.records
          if (read.partial) issues.add("opencode")
          for (const item of read.channels) {
            add(item.sessions)
            const saved = selected.get(item.path)
            if (saved) engineCheckpoints.push([saved.key, { since, identity: saved.identity, revision: saved.revision, sessions: item.sessions, watermarks: item.watermarks }])
          }
        }
      } catch { issues.add("opencode") }
    } else { issues.add("opencode") }

    const lineOf = lineages(nodes)
    const withLine = ({ bot: _bot, botName: _name, root: _root, botOwn: _own, ...record }: InsightRecord): InsightRecord => {
      const line = lineOf(record.session)
      return {
        ...record, ...(line.bot ? { bot: line.bot } : {}), ...(line.botName ? { botName: line.botName } : {}),
        ...(line.root !== record.session ? { root: line.root } : {}), ...(line.own ? { botOwn: true as const } : {}),
      }
    }
    const lineage = [...nodes.keys()].flatMap((id): [string, Lineage][] => {
      const line = lineOf(id)
      return line.bot || line.root !== id ? [[id, line]] : []
    })
    const waiting: [string, { stamp: string; since: number }][] = []
    let observations = 0
    let transcripts = 0
    const flush = async () => {
      if (!index) return
      observations += records.length
      if (records.length) index.take(records.splice(0).map(withLine))
      if (waiting.length) index.write(waiting.splice(0))
      await nextTurn()
    }

    // Release engine data before the legacy transcript reader starts decoding large files.
    if (index) {
      const readEngine = engineRecords.length > 0
      for (const record of engineRecords) {
        records.push(record)
        if (records.length >= 400) await flush()
      }
      engineRecords = []
      await flush()
      if (engineCheckpoints.length) index.write(engineCheckpoints)
      if (readEngine) index.reclaim()
    }

    const stamps = new Map<string, { stamp: string; size: number }>()
    const nativeStamps = new Map<string, string>()
    const threads = new Map<string, string[]>()
    const byConversation = new Map<string, Set<string>>()
    const dirty = new Set<string>()
    if (index) for (const record of conversations as Conversation[]) {
      const { session, folder } = record
      if (!folder || session.time.updated < since) continue
      const keys = nativeThreads(record, this.native.keys())
      byConversation.set(session.id, keys)
      for (const key of keys) {
        let owners = threads.get(key)
        if (!owners) threads.set(key, owners = [])
        owners.push(session.id)
      }
      const source = [...keys].sort().map(key => {
        let stamp = nativeStamps.get(key)
        if (!stamp) nativeStamps.set(key, stamp = insightKey(key, native.get(key) ?? []))
        return stamp
      })
      const files = conversationStamp(folder)
      const stamp = insightKey(files, session.title, session.model, session.parentID,
        seatBot(session.metadata), local, source)
      stamps.set(session.id, { stamp, size: files.reduce((sum, file) => sum + (file?.size ?? 0), 0) })
      const saved = index.read<{ stamp: string; since: number }>("scan/transcript-v1/" + session.id)
      if (stamp !== saved?.stamp || saved.since > since) dirty.add(session.id)
    }
    // A native call can match only one saved conversation. Read all conversations that share a
    // changed thread so whichever one previously claimed the call keeps that precedence.
    for (const id of dirty) for (const key of byConversation.get(id) ?? []) {
      for (const owner of threads.get(key) ?? []) dirty.add(owner)
    }

    let decoded = 0
    for (const record of conversations as Conversation[]) {
      const session = record.session
      if (session.time.updated < since || (index && record.folder && !dirty.has(session.id))) continue
      transcripts++
      const saved = stamps.get(session.id)
      if (index && decoded && decoded + (saved?.size ?? 0) >= DECODE_BUDGET) {
        decoded = 0
        await flush()
        index.reclaim()
      }
      let complete = true
      const messages = function* () {
        try { yield* record.messages } catch (error) {
          if (!index) throw error
          complete = false
          issues.add("coding-agent history")
          console.error("Unable to index saved conversation", session.id, error)
        }
      }
      const seat = seatBot(session.metadata)
      const base = { session: session.id, title: session.title }
      for (const message of messages()) {
        if (index && records.length >= 400) await flush()
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
            if (index && records.length >= 400) await flush()
          }
          if (matched) continue
        }
        records.push({ ...step, tokens, ...(!tokens ? { partial: true as const } : {}) })
      }
      if (saved) {
        if (complete) waiting.push(["scan/transcript-v1/" + session.id, { stamp: saved.stamp, since }])
        decoded += saved.size
      }
      // Legacy JSON may contain inline attachments. Drop decoder and SQLite allocations after
      // each tranche of large files instead of waiting for pressure from the resident process.
      if (index && decoded >= DECODE_BUDGET) {
        decoded = 0
        await flush()
        index.reclaim()
      } else if (index && waiting.length >= 32) await flush()
    }
    if (index) {
      await flush()
      const revision = insightKey(lineage)
      if (index.read("scan/lineage-v1") !== revision) {
        index.attribute(lineage)
        index.write([["scan/lineage-v1", revision]])
      }
    } else for (const record of engineRecords) records.push(record)
    return { records: records.map(withLine), lineage, issues: [...issues], transcripts, observations }
  }

  private async engineRecords(folder: string, since: number, after?: Record<string, EngineWatermarks>, selected?: ReadonlySet<string>) {
    const records: InsightRecord[] = []
    const channels: { path: string; sessions: EngineSessions; watermarks?: EngineWatermarks }[] = []
    const sessions = new Map<string, Map<string, { parentID?: string; metadata?: unknown }>>()
    const answers = await engineUsageAnswers(folder, since, after, selected, answer => {
      const channel = sessions.get(answer.path) ?? new Map<string, { parentID?: string; metadata?: unknown }>()
      if (answer.sessions) {
        for (const [id, parent, metadata] of answer.sessions) {
          channel.set(id, { ...channel.get(id), ...(parent ? { parentID: parent } : {}), ...(metadata ? { metadata } : {}) })
        }
        sessions.set(answer.path, channel)
        channels.push({ path: answer.path, sessions: [...channel], watermarks: answer.watermarks })
      }
      if (!answer.json) return
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
    })
    const failures = new Set(answers?.filter(answer => answer.failed !== undefined).map(answer => answer.path))
    return { records, channels: channels.filter(channel => !failures.has(channel.path)), partial: failures.size > 0 }
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
