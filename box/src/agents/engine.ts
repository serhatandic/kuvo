// The engine's own models (OpenCode Zen, your API keys, Copilot…) in a session here: a model of
// the engine you pick, or one in your Auto list. The message runs in a session of the engine's
// own, which the app keeps out of sight (its metadata says which session here it works for), and
// what happens there is told here as it happens: its steps, tool calls, and what it asks you.
//
// The app tells the bridge where the engine is (`PUT /api/kuvo/engine`) each time either starts.

import { resetsIn } from "../shared/limits"
import { stepContext } from "../shared/context"
import { readFileSync } from "node:fs"
import { join } from "node:path"
import type { Agent, PermissionAsk, RunInput, RunResult, Thread, TurnHooks } from "./agent"
import { failure } from "./agent"
import type { Limit } from "./auto"
import { newID } from "./ids"
import { blocksInFiles, blocksOf } from "./blocks"
import { matches, ruled } from "./rules"
import type { Turn } from "./turn"
import type { Decision, FormField, Model, ModelRef, Session, StructuredError, TokenUsage, ToolOutput, PermissionRule } from "./wire"

interface EngineEvent {
  id: string
  type: string
  data: Record<string, any>
}

type Listener = { event(event: EngineEvent): void; reconnected(): void; exited?(ready: Promise<void>): void }

/** The engine's address and its live feed, shared by every run here. */
export class EngineLink {
  private url?: string
  private authorization?: string
  private generation = 0
  private pid?: number
  private replacement?: PromiseWithResolvers<void>
  private stream?: AbortController

  private listeners = new Map<string, Set<Listener>>()
  /** Told of every event, whoever's session it's for (the record of what agents made). */
  private watchers = new Set<(event: EngineEvent) => void>()
  /** Sessions the engine started under one of ours (a subagent's): asks from them are ours too. */
  private children = new Map<string, string>()
  private names = new Map<string, string>()
  /** The engine's providers ready to use (connected, or needing no sign-in) and their models, a minute at a time. */
  private catalog?: { at: number; ready: Set<string>; models: Model[] }

  get ready() {
    return Boolean(this.url)
  }

  private waiting: (() => void)[] = []

  /** Once it's known where the engine is (at once on a machine, told by the app on the Mac). */
  whenReady(): Promise<void> {
    if (this.ready) return Promise.resolve()
    return new Promise((resolve) => this.waiting.push(resolve))
  }

  private processEnded() {
    this.replacement ??= Promise.withResolvers<void>()
    for (const set of this.listeners.values()) for (const listener of set) listener.exited?.(this.replacement.promise)
  }

  /** Where the engine is now; a new one (it restarted) starts a new feed. */
  configure(url: string | undefined, password: string | undefined, pid?: number) {
    const authorization = password ? "Basic " + Buffer.from(`opencode:${password}`).toString("base64") : undefined
    const address = url?.replace(/\/$/, "")
    const processID = pid !== undefined && Number.isSafeInteger(pid) && pid > 0 ? pid : undefined
    if (address === this.url && authorization === this.authorization && processID === this.pid) return
    const replaced = this.url !== undefined && (this.pid !== undefined && processID !== undefined ? this.pid !== processID : this.url !== address)
    this.pid = processID
    this.url = address
    this.authorization = authorization
    this.names.clear()
    this.catalog = undefined
    this.reads.clear()
    const generation = ++this.generation
    this.stream?.abort()
    this.stream = new AbortController()
    if (this.url) void this.feed(generation, this.stream.signal)
    if (replaced) this.processEnded()
    else for (const set of this.listeners.values()) for (const listener of set) listener.reconnected()
    if (this.url) for (const resolve of this.waiting.splice(0)) resolve()
  }

  /** Waits, before what would start a turn on the engine, while the engine is being changed
   * (PROTOCOL.md, "A new engine"): set by the bridge's start, where the box's run folder is known. */
  beforeTurn?: (alreadyAdmitted?: boolean) => Promise<void>

  async request<T = any>(method: string, path: string, body?: unknown, timeout = 30_000, directory?: string, alreadyAdmitted = false): Promise<T> {
    // (A conversation on the engine's models, a review, a seat in a group chat: each waits for the
    // new engine rather than starting on the one being stopped, bounded as that hold is.)
    if (this.beforeTurn && method === "POST" && /^\/?api\/session\/[^/]+\/(prompt|command|shell|compact|summarize)$/.test(path)) await this.beforeTurn(alreadyAdmitted)
    if (!this.url) throw new Error("The engine isn't running.")
    const response = await fetch(this.url + path, {
      method,
      headers: {
        Authorization: this.authorization ?? "",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        // What's tied to a project (a command's output) is asked for in its folder.
        ...(directory ? { "x-opencode-directory": encodeURIComponent(directory) } : {}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeout),
    })
    const text = await response.text()
    let value: any
    try {
      value = text ? JSON.parse(text) : undefined
    } catch {
      value = text
    }
    if (!response.ok) {
      const error = new EngineError(value?.message ?? `The engine answered ${response.status}.`)
      error.status = response.status
      throw error
    }
    return value as T
  }

  /** The engine's name for a provider ("OpenCode Zen"), once it has listed them. */
  providerName(providerID: string): string | undefined {
    if (!this.names.size && this.url) {
      this.names.set("", "")
      void this.request("GET", "/api/provider")
        .then((answer) => {
          for (const provider of answer?.data ?? []) if (provider?.id && provider?.name) this.names.set(provider.id, provider.name)
          // None yet (a folder it hasn't loaded): asked again next time.
          if (this.names.size === 1) this.names.clear()
        })
        .catch(() => this.names.clear())
    }
    return this.names.get(providerID)
  }

  private reads = new Map<string, { at: number; value?: any; failed?: boolean; pending?: Promise<any> }>()

  /**
   * One of the engine's lists for a folder, kept a while (one read at a time, a failure kept a
   * moment too, so a stalled engine doesn't hold up each message); as last known when it doesn't
   * answer, and undefined when it never has. `usable` says whether an answer is (a folder the
   * engine hasn't loaded yet answers with nothing at first). `settled`: asked for once the engine's
   * plugins are set up (`pluginsSetUp`), and taken as that says; one read without waiting for them
   * long enough isn't kept, so the next message reads it again.
   */
  private async read(path: string, directory: string, usable: (value: any) => boolean = () => true, fresh = false, settled?: (value: any) => any): Promise<any> {
    const key = `${path}\n${directory}`
    const kept = this.reads.get(key)
    if (!this.url) return kept?.value
    if (!fresh && kept && Date.now() - kept.at < (kept.failed ? 10_000 : 30_000)) return kept.value
    if (kept?.pending) return kept.pending
    // What a restarted engine answers counts, not an answer from the one before it.
    const generation = this.generation
    const keep = (entry: { at: number; value: any; failed?: boolean }) => {
      if (generation === this.generation) this.reads.set(key, entry)
    }
    let setUp = true
    const asked = settled
      ? this.pluginsSetUp(directory).then(async (done) => {
          setUp = done
          return settled(await this.request("GET", path, undefined, 4_000, directory))
        })
      : this.request("GET", path, undefined, 4_000, directory)
    const pending = asked.then(
      (value) => {
        if (usable(value)) keep({ at: setUp ? Date.now() : 0, value })
        else keep({ at: 0, value: kept?.value })
        return usable(value) ? value : kept?.value
      },
      () => {
        keep({ at: Date.now(), value: kept?.value, failed: true })
        return kept?.value
      },
    )
    this.reads.set(key, { at: 0, value: kept?.value, pending })
    return pending
  }

  /** The engine's settings files for a folder (yours, the project's), each as written; undefined when
   * never read, and with `current` when the engine didn't answer just now (not as last known). */
  private async settingsFiles(directory: string, fresh = false, current = false): Promise<any[] | undefined> {
    const generation = this.generation
    // Live permission replies cannot share a cached or already pending settings read.
    const answer = fresh ? await this.request("GET", "/api/config", undefined, 4_000, directory).catch(() => undefined)
      : await this.read("/api/config", directory)
    if (answer === undefined) return undefined
    if (current && !fresh && (!this.url || this.reads.get(`/api/config\n${directory}`)?.failed)) return undefined
    if (fresh && (generation !== this.generation || !(answer?.data ?? answer) || typeof (answer?.data ?? answer) !== "object")) return undefined
    return Array.isArray(answer) ? answer : Array.isArray(answer?.data) ? answer.data : [{ info: answer?.data ?? answer }]
  }

  /** The session's agent as the engine has it (yours, from Settings or a file): what it's told, and its permissions. */
  async agent(name: string, directory: string): Promise<{ system?: string; permissions?: PermissionRule[] } | undefined> {
    const listed = await this.agentList(directory, name)
    // Not known: what its settings say it's told, for one of yours.
    if (listed === undefined) return name === "build" || name === "plan" ? undefined : { system: await this.agentPrompt(name, directory) }
    const found = listed.find(named(name))
    if (!found) return undefined
    return { system: typeof found.system === "string" && found.system.trim() ? found.system.trim() : undefined, permissions: Array.isArray(found.permissions) ? found.permissions : undefined }
  }

  /** The engine's agents for a folder, as it lists them (yours and its own); undefined when it never answered. */
  async agents(directory: string): Promise<any[] | undefined> {
    return this.agentList(directory)
  }

  /**
   * The engine's agents for a folder, with Kuvo Code's rules (`withKuvoRules`); undefined when it
   * never answered. An engine that opened the folder a moment ago, or is reading it again, may
   * list them before its plugins are set up (2.2.7 does), without the rules the box's plugin gives
   * them: so they're asked for once its plugins are, and a list still without them has each agent
   * ask first outside the project.
   */
  private async agentList(directory: string, name?: string): Promise<any[] | undefined> {
    const usable = (value: any) => Array.isArray(value?.data) && value.data.length > 0
    let answer = await this.read("/api/agent", directory, usable, false, withKuvoRules)
    // One you've just added isn't in what was read a moment ago.
    if (answer !== undefined && name && !answer.data.some(named(name))) answer = await this.read("/api/agent", directory, usable, true, withKuvoRules)
    return answer?.data
  }

  /**
   * Once the engine has set up its plugins for a folder: its providers wait for them (its agents,
   * integrations and skills don't from 2.2.7 on, listing what it has so far, and then say
   * `agent.updated`). Not longer than a few seconds: a plugin that hangs doesn't hold up every
   * message (what's read then is taken as it is, with Kuvo Code's rules if it hasn't them).
   */
  private pluginsSetUp(directory: string): Promise<boolean> {
    return this.request("GET", "/api/provider", undefined, 5_000, directory).then(
      () => true,
      () => false,
    )
  }

  /**
   * The files the engine reads its settings from for a folder (`settingsLayers`), to read its Blocks
   * from when it can't be asked: it's parked while only coding agents work (its record gone, and this
   * still pointing where it was), or starting again. Set where the box's folders are known (main.ts).
   */
  settingsLayers?: (directory: string) => string[]

  /**
   * Settings › Permissions' Blocks for tools and commands (the engine's `policies`, "shell:git push
   * *"), as rules: what nothing allows. As the engine says now, or, when it can't, as its settings
   * files say now (`settingsLayers`), not as it said before it stopped. Undefined when they aren't
   * known (neither answers). `fresh`: read now, for a live answer, not kept from a moment ago.
   */
  async policies(directory: string, fresh = false): Promise<PermissionRule[] | undefined> {
    const files = await this.settingsFiles(directory, fresh, Boolean(this.settingsLayers))
    if (files) return blocksOf(files.flatMap((source) => (Array.isArray(source?.info?.policies) ? source.info.policies : [])))
    return this.settingsLayers ? blocksInFiles(this.settingsLayers(directory)) : undefined
  }

  /** The model for sessions' titles (Settings › Agents), when you picked one. */
  async titleModel(directory: string): Promise<ModelRef | undefined> {
    let picked: string | undefined
    for (const source of (await this.settingsFiles(directory)) ?? []) {
      const own = source?.info?.agents?.title?.model ?? source?.info?.agent?.title?.model
      if (typeof own === "string" && own.includes("/")) picked = own
    }
    if (!picked) return undefined
    const at = picked.indexOf("/")
    return { providerID: picked.slice(0, at), id: picked.slice(at + 1) }
  }

  /** What an agent of yours (Settings › Agents) is told to be: its prompt, from the last file that has it. */
  async agentPrompt(name: string, directory: string): Promise<string | undefined> {
    let prompt: string | undefined
    for (const source of (await this.settingsFiles(directory)) ?? []) {
      // `agents.<name>.system` (as Kuvo Code writes it); `agent.<name>.prompt` in older settings.
      const own = source?.info?.agents?.[name]?.system ?? source?.info?.agent?.[name]?.prompt
      if (typeof own === "string" && own.trim()) prompt = own.trim()
    }
    return prompt
  }

  /** Whether a provider is ready to use now, and its models; nothing known when the engine isn't. */
  async offered(providerID: string, fresh = false): Promise<{ ready: boolean; models: Model[]; listed?: boolean } | undefined> {
    if (!this.url) return undefined
    if (fresh || !this.catalog || Date.now() - this.catalog.at > 60_000) {
      try {
        const [providers, models] = await Promise.all([this.request("GET", "/api/provider"), this.request("GET", "/api/model")])
        const ready = new Set<string>((providers?.data ?? []).map((provider: { id: string }) => provider.id))
        // None (an engine that hasn't loaded its folder yet answers so at first): not kept.
        if (!ready.size) return { ready: false, models: [] }
        this.catalog = { at: Date.now(), ready, models: models?.data ?? [] }
      } catch {
        return undefined
      }
    }
    // `listed`: from the providers the engine listed, so one not among them isn't signed in.
    return { ready: this.catalog.ready.has(providerID), models: this.catalog.models.filter((model) => model.providerID === providerID), listed: true }
  }

  listen(sessionID: string, listener: Listener): () => void {
    let set = this.listeners.get(sessionID)
    if (!set) this.listeners.set(sessionID, (set = new Set()))
    set.add(listener)
    return () => {
      set!.delete(listener)
      if (set!.size) return
      // Its subagents' sessions too, once nothing follows it (another may still: a goal and a group chat both follow a bot's).
      this.listeners.delete(sessionID)
      for (const [child, parent] of this.children) if (parent === sessionID) this.children.delete(child)
    }
  }

  /** Every event of the engine's, as it comes. */
  watch(watcher: (event: EngineEvent) => void): () => void {
    this.watchers.add(watcher)
    return () => this.watchers.delete(watcher)
  }

  private dispatch(event: EngineEvent) {
    for (const watcher of this.watchers) watcher(event)
    const data = event.data ?? {}
    if (event.type === "session.created") {
      // The engine says it flat (`sessionID`, `parentID`); older ones, with the session.
      const session = data.session ?? data.info
      const child: string | undefined = data.sessionID ?? session?.id
      const parent: string | undefined = data.parentID ?? session?.parentID
      if (child && parent && (this.listeners.has(parent) || this.children.has(parent))) this.children.set(child, this.children.get(parent) ?? parent)
      return
    }
    let sessionID: string | undefined = data.sessionID ?? data.form?.sessionID
    if (!sessionID) return
    sessionID = this.children.get(sessionID) ?? sessionID
    for (const listener of this.listeners.get(sessionID) ?? []) listener.event(event)
  }

  /** The engine's events, read for as long as it's the engine here; read again after a drop. */
  private async feed(generation: number, signal: AbortSignal) {
    let first = true
    while (generation === this.generation && this.url) {
      try {
        const response = await fetch(this.url + "/api/event", { headers: { Authorization: this.authorization ?? "", Accept: "text/event-stream" }, signal })
        if (!response.ok || !response.body) throw new Error(`feed ${response.status}`)
        if (generation !== this.generation) {
          await response.body.cancel().catch(() => {})
          return
        }
        this.replacement?.resolve()
        this.replacement = undefined
        if (!first) for (const set of this.listeners.values()) for (const listener of set) listener.reconnected()
        first = false
        const reader = response.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ""
        while (generation === this.generation) {
          // Quiet for longer than its heartbeat: gone.
          const read = await Promise.race([reader.read(), new Promise<"quiet">((done) => setTimeout(() => done("quiet"), 45_000))])
          if (signal.aborted || read === "quiet" || read.done) break
          buffer += decoder.decode(read.value, { stream: true })
          let end: number
          while ((end = buffer.indexOf("\n\n")) >= 0) {
            const chunk = buffer.slice(0, end)
            buffer = buffer.slice(end + 2)
            const data = chunk
              .split("\n")
              .filter((line) => line.startsWith("data:"))
              .map((line) => line.slice(5).trimStart())
              .join("\n")
            if (!data) continue
            try {
              this.dispatch(JSON.parse(data))
            } catch {}
          }
        }
        await reader.cancel().catch(() => {})
      } catch (error) {
        // The Mac's engine address can come from the app without a PID.
        const failure = error as { code?: string; cause?: { code?: string } }
        const refused = (code?: string) => code === "ECONNREFUSED" || code === "ConnectionRefused"
        if (!signal.aborted && this.pid === undefined && (refused(failure?.code) || refused(failure?.cause?.code))) this.processEnded()
      }
      if (generation !== this.generation) return
      if (this.pid !== undefined) {
        try { process.kill(this.pid, 0) } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ESRCH") {
            this.processEnded()
          }
        }
      }
      first = false
      await Bun.sleep(1000)
    }
  }
}

/** An agent the engine lists by that name. */
const named = (name: string) => (agent: any) => agent?.id === name || agent?.name === name

/** A folder no rule of yours names: where the box's engine plugin (link/engine/defaults.ts) makes
 * an agent ask first, as it checks too. */
const OUTSIDE = "/kuvo-code/an/outside/folder/*"

/**
 * Whether the box's engine plugin (link/engine/defaults.ts) has given an agent its rules: its ask
 * outside the project, and then the engine's long outputs saved whole (`<data>/tool-output`)
 * open, which it gives every agent it gives the ask. (Your settings' rules come after them, and
 * agents of yours in Settings › Agents don't get them, as on the engine.)
 */
function fromPlugin(agent: any): boolean {
  const rules: any[] = Array.isArray(agent?.permissions) ? agent.permissions : []
  const at = rules.findIndex((rule) => rule?.action === "external_directory" && rule.resource === "*" && rule.effect === "ask")
  return at >= 0 && rules.slice(at + 1).some((rule) => rule?.action === "external_directory" && rule.effect === "allow" && typeof rule.resource === "string" && rule.resource.endsWith("/tool-output/*"))
}

/**
 * The engine's agents (its answer for `api/agent`) with Kuvo Code's rules: as listed once the
 * box's plugin has given them; before (an engine still setting up its plugins, or one without
 * it), the ask where the plugin puts it: after the rule that lets work outside the project through
 * (the engine's own `* allow`), before your rules about folders outside, which still win. An
 * agent that asks or refuses already, or whose own rule allows it outside, is left as it is.
 */
function withKuvoRules(answer: any): any {
  if (!Array.isArray(answer?.data) || answer.data.some(fromPlugin)) return answer
  const asking = (agent: any) => {
    const permissions: PermissionRule[] = Array.isArray(agent?.permissions) ? agent.permissions : []
    const effect = ruled(permissions, "external_directory", [OUTSIDE])
    if (effect === "ask" || effect === "deny") return agent
    // (The last rule that lets it out; none: the engine's own default does.)
    const at = permissions.findLastIndex((rule) => matches(rule.action, "external_directory") && matches(rule.resource, OUTSIDE))
    if (at >= 0 && permissions[at].action === "external_directory") return agent
    const ask: PermissionRule = { action: "external_directory", resource: "*", effect: "ask" }
    return { ...agent, permissions: [...permissions.slice(0, at + 1), ask, ...permissions.slice(at + 1)] }
  }
  return { ...answer, data: answer.data.map(asking) }
}

class EngineError extends Error {
  status?: number
}

/** A model of one of the engine's providers, run here. */
export class EngineAgent implements Agent {
  readonly id = "engine"
  readonly compacts = true
  readonly instance = { id: "default" }

  constructor(
    private readonly link: EngineLink,
    readonly providerID: string,
  ) {}

  get name() {
    return this.link.providerName(this.providerID) || this.providerID
  }

  methods() {
    return []
  }

  /** Signed in: the engine is there and has this provider ready (connected, or needing no sign-in). */
  async account() {
    const offered = await this.link.offered(this.providerID)
    return { installed: true, signedIn: this.link.ready && (offered?.ready ?? true) }
  }

  /** Not among the providers the engine listed as ready: a request on it, the engine drops. */
  async signedOut() {
    const cached = await this.link.offered(this.providerID).catch(() => undefined)
    if (!(cached?.listed === true && !cached.ready)) return false
    // Signed in since the engine was last asked (it's asked a minute at a time): asked again.
    const offered = await this.link.offered(this.providerID, true).catch(() => undefined)
    return offered?.listed === true && !offered.ready
  }

  /** Its models as the engine lists them (for Auto to pass by one it doesn't offer). */
  async models() {
    return (await this.link.offered(this.providerID))?.models ?? []
  }

  thread(session: Session, state: Record<string, unknown>): Thread {
    return new EngineThread(this.link, session, state)
  }

  async generate(prompt: string, model: ModelRef): Promise<string> {
    const answer = await this.link.request<{ data?: { text?: string } }>("POST", "/api/experimental/generate", { prompt, model: { providerID: model.providerID, id: model.id } }, 120_000)
    return answer?.data?.text ?? ""
  }

  /** The engine's session for a session here, gone with it. */
  async forgetConversation(state: Record<string, unknown>) {
    const id = state.engineSession as string | undefined
    if (id) await this.link.request("DELETE", `/api/session/${encodeURIComponent(id)}`).catch(() => {})
  }
}

/** Of a shell command's output, the end, as the agents' tool calls show it while it runs. */
const outputLimit = 64_000

class EngineThread implements Thread {
  private stop?: () => Promise<void>

  constructor(
    private readonly link: EngineLink,
    private readonly session: Session,
    private readonly state: Record<string, unknown>,
  ) {}

  /** The engine's session for this one, made the first time (or again, if it went). */
  private async engineSession(input: RunInput, hooks: TurnHooks): Promise<string> {
    const existing = this.state.engineSession as string | undefined
    if (existing) {
      try {
        await this.link.request("GET", `/api/session/${encodeURIComponent(existing)}`)
        return existing
      } catch (error) {
        if ((error as EngineError).status !== 404) throw error
      }
    }
    const created = await this.link.request("POST", "/api/session", {
      title: this.session.title ? `Kuvo Code: ${this.session.title}` : "Kuvo Code",
      location: { directory: this.session.location.directory },
      model: input.model,
      agent: input.plan ? "plan" : (this.session.agent ?? "build"),
      permissions: this.session.permissions ?? [],
      metadata: { kuvo: { engineFor: this.session.id } },
    })
    const id = created?.data?.id as string
    if (!id) throw new Error("The engine didn't start a session.")
    if (existing) this.startedOver = true
    this.state.engineSession = id
    delete this.state.engineInstructions
    hooks.saveState()
    return id
  }

  /** The session it had was gone: a new one took its place. */
  private startedOver = false

  async prepare(input: RunInput, hooks: TurnHooks) {
    if (!this.link.ready) return undefined
    await this.engineSession(input, hooks)
    const fresh = this.startedOver
    this.startedOver = false
    return { fresh }
  }

  async run(input: RunInput, turn: Turn, hooks: TurnHooks): Promise<RunResult> {
    if (!this.link.ready) return { outcome: "failed", error: { type: "engine", message: "The engine isn't running: wait for it to start, or restart Kuvo Code." } }
    const id = await this.engineSession(input, hooks)
    const path = `/api/session/${encodeURIComponent(id)}`

    // What it runs with: this session's rules, model, mode and the app's added instructions.
    await this.link.request("PATCH", path, { permissions: this.session.permissions ?? [] })
    await this.link.request("POST", `${path}/model`, { model: input.model })
    // As the session's agent (one of yours too), which the engine runs as it would its own.
    await this.link.request("POST", `${path}/agent`, { agent: input.plan ? "plan" : (this.session.agent ?? "build") })
    const instructions = hooks.instructions(false)
    if (instructions !== (this.state.engineInstructions ?? "")) {
      if (instructions) await this.link.request("PUT", `/api/experimental/session/${encodeURIComponent(id)}/instructions/entries/kuvo`, { value: instructions })
      else await this.link.request("DELETE", `/api/experimental/session/${encodeURIComponent(id)}/instructions/entries/kuvo`).catch(() => {})
      this.state.engineInstructions = instructions
      hooks.saveState()
    }
    // Still at work from before (a turn stopped here that the engine didn't): stopped.
    const active = await this.link.request("GET", "/api/session/active").catch(() => undefined)
    if (active?.data?.[id]) await this.link.request("POST", `${path}/interrupt`).catch(() => {})

    // The engine being changed (PROTOCOL.md, "A new engine"): the turn waits for the new one before
    // it begins (before it listens, or notes when it was sent), and Stop ends the wait.
    if (this.link.beforeTurn) {
      await Promise.race([this.link.beforeTurn(true), new Promise<void>(resolve => (hooks.signal.aborted ? resolve() : hooks.signal.addEventListener("abort", () => resolve(), { once: true })))])
      if (hooks.signal.aborted) return { outcome: "interrupted" }
    }
    const inputID = newID("msg")
    const sentAt = Date.now()
    let step: string | undefined
    let delivered = false
    let limit: Limit | undefined
    const shells = new Map<string, ReturnType<typeof setInterval>>()
    const stopShell = (tool: string) => {
      clearInterval(shells.get(tool))
      shells.delete(tool)
    }

    let settle!: (result: RunResult) => void
    const settled = new Promise<RunResult>((resolve) => (settle = resolve))
    let done = false
    const finish = (result: RunResult) => {
      if (done) return
      done = true
      settle(result)
    }

    const interrupt = async () => {
      await this.link.request("POST", `${path}/interrupt`).catch(() => {})
      // It's said to stop; if it doesn't say it has, it's taken as stopped.
      setTimeout(() => finish({ outcome: "interrupted" }), 10_000)
    }
    this.stop = interrupt

    const answerPermission = async (data: Record<string, any>) => {
      const ask: PermissionAsk = {
        toolCallID: data.source?.id,
        action: data.action,
        resources: data.resources ?? [],
        save: data.save,
        metadata: data.metadata,
        message: data.message,
        weighed: true,
      }
      let decision: Decision = "reject"
      try {
        decision = await hooks.permission(ask)
      } catch {}
      await this.link.request("POST", `/api/session/${encodeURIComponent(data.sessionID)}/permission/${encodeURIComponent(data.id)}/reply`, { decision }).catch(() => {})
    }

    const answerForm = async (form: { id: string; sessionID: string; title?: string; fields?: FormField[]; metadata?: Record<string, unknown> }) => {
      let answer: Record<string, unknown> | null = null
      let words: string | undefined
      try {
        // As the engine has it: its kind (a form with none isn't a question) and its tool call.
        answer = await hooks.question({ title: form.title ?? "A question", fields: form.fields ?? [], metadata: { kind: "form", ...form.metadata }, declined: (said) => { words = said } })
      } catch {}
      const formPath = `/api/session/${encodeURIComponent(form.sessionID)}/form/${encodeURIComponent(form.id)}`
      if (answer) await this.link.request("POST", `${formPath}/reply`, { answer }).catch(() => {})
      else await this.link.request("DELETE", words ? `${formPath}?message=${encodeURIComponent(words)}` : formPath).catch(() => {})
    }

    const followShell = (tool: string, shellID: string) => {
      if (shells.has(tool)) return
      // Read on from where the last read ended, keeping its end.
      let cursor = 0
      let output = ""
      let reading = false
      const read = async () => {
        if (reading) return
        reading = true
        try {
          const answer = await this.link.request("GET", `/api/shell/${encodeURIComponent(shellID)}/output?cursor=${cursor}`, undefined, 10_000, this.session.location.directory).catch(() => undefined)
          const more = String(answer?.data?.output ?? "")
          if (typeof answer?.data?.cursor === "number") cursor = answer.data.cursor
          if (!more) return
          output = (output + more).slice(-outputLimit)
          turn.toolProgress(tool, { output })
        } finally {
          reading = false
        }
      }
      shells.set(tool, setInterval(read, 1000))
    }

    const model = (await this.link.offered(input.model.providerID).catch(() => undefined))?.models.find(each => each.id === input.model.id)
    const onEvent = (event: EngineEvent) => {
      if (done) return
      const data = event.data
      const ownSession = data.sessionID === id
      switch (event.type) {
        case "permission.asked":
          void answerPermission(data)
          return
        case "form.created":
          if (data.form) void answerForm(data.form)
          return
      }
      // A subagent's own steps show in its own conversation, not in this one.
      if (!ownSession) return
      if (event.type === "session.compaction.ended") turn.clearContext()
      // A summary you asked for: how it ends, and none of its words.
      if (input.compact && !event.type.startsWith("session.execution.") && event.type !== "session.idle") return
      switch (event.type) {
        case "session.inbox.delivered":
          if (data.inboxID === inputID) delivered = true
          return
        case "session.step.started":
          // Started again after a retry: the same step.
          if (data.assistantMessageID === step) return
          step = data.assistantMessageID
          turn.startStep(data.model ?? input.model)
          return
        // Thinking and words, each block where the engine says it is (they can interleave).
        case "session.text.started":
        case "session.reasoning.started": {
          const kind = event.type === "session.text.started" ? "text" : "reasoning"
          turn.blockStart(`${kind}-${data.ordinal ?? 0}`, kind)
          return
        }
        case "session.text.delta":
        case "session.reasoning.delta": {
          const kind = event.type === "session.text.delta" ? "text" : "reasoning"
          turn.blockDelta(`${kind}-${data.ordinal ?? 0}`, kind, String(data.delta ?? ""))
          return
        }
        case "session.text.ended":
        case "session.reasoning.ended":
          turn.blockEnd(`${event.type === "session.text.ended" ? "text" : "reasoning"}-${data.ordinal ?? 0}`)
          return
        case "session.tool.input.started":
          turn.toolStart(data.id, data.name)
          return
        case "session.tool.input.delta":
          turn.toolInputDelta(data.id, String(data.delta ?? ""))
          return
        case "session.tool.called":
          turn.toolCalled(data.id, data.input)
          return
        case "session.tool.progress": {
          const { shellID, ...rest } = (data.metadata ?? {}) as Record<string, unknown>
          if (typeof shellID === "string") followShell(data.id, shellID)
          if (Object.keys(rest).length) turn.toolProgress(data.id, rest)
          return
        }
        case "session.tool.success": {
          stopShell(data.id)
          const { shellID: _, ...metadata } = (data.metadata ?? {}) as Record<string, unknown>
          turn.toolSucceeded(data.id, (data.content ?? []) as ToolOutput[], metadata)
          return
        }
        case "session.tool.failed": {
          stopShell(data.id)
          const { shellID: _, ...metadata } = (data.metadata ?? {}) as Record<string, unknown>
          const error = (data.error ?? {}) as StructuredError
          turn.toolFailed(data.id, error.message ?? "", metadata, error.type ?? "tool")
          return
        }
        case "session.step.ended":
        case "session.step.failed":
          if (data.assistantMessageID !== step) return
          step = undefined
          if (event.type === "session.step.ended") turn.context(stepContext(data.tokens), model?.limit.context)
          turn.endStep({ tokens: data.tokens as TokenUsage | undefined, cost: data.cost, finish: data.finish, error: data.error })
          return
        case "session.retry.scheduled": {
          const reached = limitOf(data.error)
          // On Auto, a limit is the next model's turn, not a wait.
          if (input.auto && reached) {
            limit = reached
            void interrupt()
          }
          return
        }
        case "session.idle":
          void onReconnected()
          return
        case "session.execution.succeeded":
          if (delivered) finish({ outcome: "succeeded" })
          else void onReconnected()
          return
        case "session.execution.interrupted":
          if (delivered || limit) finish(limit ? { outcome: "failed", error: { type: "limit", message: "A usage limit was reached." }, limit, taken: delivered } : { outcome: "interrupted" })
          return
        case "session.execution.failed": {
          const error = (data.error ?? { type: "engine", message: "The engine stopped with an error." }) as StructuredError
          const reported = limitOf(error)
          const reached = reported && limit?.scope === reported.scope
            ? { ...reported, resetsAt: reported.resetsAt ?? limit.resetsAt }
            : reported ?? limit
          finish({ outcome: "failed", error, limit: reached, taken: delivered })
          return
        }
      }
    }

    // Its feed dropped (or the engine restarted): what became of the run is asked.
    const onReconnected = async () => {
      if (done) return
      const active = await this.link.request("GET", "/api/session/active").catch(() => undefined)
      if (!active || (Array.isArray(active.data) ? active.data.includes(id) : active.data?.[id])) return
      const session = await this.link.request("GET", path).catch(() => undefined)
      if (!session?.data) return
      const outcome = session.data.outcome as RunResult["outcome"] | undefined
      // Its outcome is this message's only if it went idle after the message was sent.
      const idle = Number(session?.data?.time?.idle ?? 0)
      if (idle < sentAt) return
      if (outcome === "succeeded" || outcome === "interrupted") finish({ outcome })
      else if (outcome === "failed") finish({ outcome, error: { type: "engine", message: "The engine stopped with an error." } })
    }

    const unlisten = this.link.listen(id, {
      event: onEvent,
      reconnected: () => void onReconnected(),
      exited: (resumeQueue) => finish({ outcome: "interrupted", agentExited: true, taken: delivered, resumeQueue }),
    })
    const abort = () => void interrupt()
    hooks.signal.addEventListener("abort", abort)
    try {
      if (input.compact) delivered = true
      const submitted = input.compact
        ? this.link.request("POST", `${path}/compact`, { id: inputID, delivery: "steer" }, undefined, undefined, true)
        : this.link.request("POST", `${path}/prompt`, {
          id: inputID,
          text: input.text,
          files: input.files.length ? input.files.map((file) => ({ uri: file.uri, name: file.name })) : undefined,
          delivery: "steer",
        }, undefined, undefined, true)
      return await Promise.race([settled, submitted.then(() => {
        if (hooks.signal.aborted) void interrupt()
        else if (!delivered && !done) void onReconnected()
        return settled
      })])
    } catch (error) {
      return done ? await settled : { outcome: "failed", error: failure(error, "engine") }
    } finally {
      hooks.signal.removeEventListener("abort", abort)
      unlisten()
      for (const tool of [...shells.keys()]) stopShell(tool)
      this.stop = undefined
    }
  }

  async interrupt() {
    await this.stop?.()
  }

  close() {}
}

/** A usage limit in the engine's words: a quota is the account's; a rate limit, the model's. */
export function limitOf(error: StructuredError | undefined): Limit | undefined {
  if (!error) return undefined
  const scope = error.limit?.scope === "model" || error.limit?.scope === "provider" ? error.limit.scope
    : ["provider.quota", "provider.budget-limit", "provider.free-limit"].includes(error.type) ? "provider"
    : ["provider.rate-limit", "provider.model-budget-limit", "limit"].includes(error.type) ? "model" : undefined
  if (!scope) return undefined
  const resetsAt = Number.isFinite(error.limit?.resetsAt) ? error.limit?.resetsAt : resetsIn(error.message)
  return { scope, resetsAt, message: error.message }
}
