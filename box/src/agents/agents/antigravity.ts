// Antigravity, as T3 Code runs it: Google's `agy` CLI (`agy acp`), installed with Google's own
// installer and updated with `agy update`, signed in with your Google account (`agy acp --login`,
// the code its page ends on pasted back), speaking ACP. Each account has a home of Kuvo Code's own
// (GEMINI_HOME, never your `~/.gemini`), the first one too: `agy` keeps one keyring sign-in for
// every home otherwise. An account signed in before `agy` keeps running on Google's earlier agent
// (1.3.0, downloaded then) until it's signed in again (docs/architecture/agent-accounts.md).
//
// While the box's sandbox is on, it runs whole in it (acp.ts `whole`), for its sessions and its
// sign-in alike, so nothing it does runs outside: its home (Kuvo Code's own, never one of yours)
// and its temporary folder written whole, its program and the bridge's browser catcher only read.
// It writes files itself then, with its own tools (the bridge's would write outside).

import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { pathToFileURL } from "node:url"
import { providerID, type AgentAccount } from "../accounts"
import type { Agent, AgentContext, Account, SignInAttempt, SubagentRun, Thread, SubagentTask, TurnHooks } from "../agent"
import { accountName, ContextWindows, failure, model } from "../agent"
import { agentHoldFolder, ANTIGRAVITY_ACP } from "../hold/adopt"
import { heldIn } from "../hold/client"
import { AcpThread } from "../acp"
import { fenceAgent, fenceRules } from "../fence"
import { emit } from "../events"
import { withOwnFolders } from "../machine"
import { newID } from "../ids"
import { antigravityCredential, antigravityGeneration } from "../../usage/antigravity-plan"
import { fingerprint } from "../../usage/limits"
import { placeWhole } from "../../shared/files"
import { RPC, RPCError } from "../rpc"
import { commandSignIn } from "../signin"
import { install, maintenance, update, type MaintenanceDefinition } from "../maintenance"
import { agyProgram } from "../../shared/agent-programs"
import { agentsUpdateThemselves } from "../../shared/agent-updates"
import { copySkillsInto, skillsFor, turnedOff } from "../skills"
import { antigravityCustomAgents, subagentsFor } from "../subagents"
import type { Turn } from "../turn"
import type { Model, OAuthStatus, Session } from "../wire"

const version = ANTIGRAVITY_ACP
function heldVersions(folder = agentHoldFolder("antigravity")) {
  return folder ? heldIn(folder).filter(holder => holder.meta.kind === "antigravity").map(holder => holder.meta.version) : []
}

/** Installed releases live side by side. A holder from an earlier bridge may still use one. */
export function forgetOldAntigravity(home: string, holders = agentHoldFolder("antigravity")) {
  const root = join(home, "antigravity")
  if (!existsSync(join(root, version, ".complete"))) return
  const held = heldVersions(holders)
  for (const name of readdirSync(root)) {
    if (name === version || !/^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(name)) continue
    if (!held.some(using => using === undefined || using === name)) rmSync(join(root, name), { recursive: true, force: true })
  }
}

/** Google's release feed for `agy`, which its installer and updater read (a test names its own). */
export const antigravityFeed = { base: "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests" }
let newestKept: { at: number; value?: string } | undefined

/** Its feed's name for this computer, as Google's installer works it out: `darwin_arm64`, `linux_amd64_musl`. */
function agyTarget(): string | undefined {
  const os = process.platform === "darwin" ? "darwin" : process.platform === "linux" ? "linux" : undefined
  const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "amd64" : undefined
  if (!os || !arch) return undefined
  const musl = os === "linux" && (existsSync("/lib/libc.musl-x86_64.so.1") || existsSync("/lib/libc.musl-aarch64.so.1"))
  return `${os}_${arch}${musl ? "_musl" : ""}`
}

/** The newest `agy` on Google's feed, asked at most every six hours unless `fresh`. */
async function agyNewest(fresh: boolean): Promise<string | undefined> {
  if (!fresh && newestKept && Date.now() - newestKept.at < 6 * 60 * 60_000) return newestKept.value
  const target = agyTarget()
  if (!target) return undefined
  try {
    const response = await fetch(`${antigravityFeed.base}/${target}.json`, { signal: AbortSignal.timeout(10_000) })
    const version = response.ok ? (((await response.json()) as { version?: unknown }).version) : undefined
    newestKept = { at: Date.now(), value: typeof version === "string" && /^\d+\.\d+\.\d+$/.test(version) ? version : undefined }
    return newestKept.value
  } catch {
    return newestKept?.value
  }
}

/** A model's name as the model picker shows it: Antigravity's Claude and GPT models named apart
 * from Claude Code's and Codex's own ("Claude Opus 5.5 (Antigravity)"); Gemini's as they are. */
export function antigravityModelName(name: string): string {
  return /gemini/i.test(name) || /\(Antigravity\)$/.test(name) ? name : `${name} (Antigravity)`
}

const executableName = "agy_acp_server.par"
const harnessName = "localharness_external"
/** What it says on stdout when it wants you to sign in (not ACP). */
const signInLine = "Open the following link to authenticate the ACP server:"

/**
 * `agy`'s own words asking for a sign-in: a line of its own, never a protocol message (one that
 * only carries the words, as a command's output or a file it shows does, isn't it).
 */
export function asksForSignIn(line: string): boolean {
  return !line.trimStart().startsWith("{") && line.includes(signInLine)
}

/** Thinking levels a model may have beyond those of the one a session is on. */
const levels = ["minimal", "low", "medium", "high", "xhigh", "max"]

/**
 * Each model's thinking levels, as a session lists them: a `<model>-<level>` entry in
 * `models.availableModels` for each level a model has (`gemini-4-neon-high`), in the order of its
 * `thought_level` option. Where it lists none, the model it's on has that option's.
 */
export function antigravityEfforts(setup: any, models: string[]): Record<string, string[]> {
  const option = (setup?.configOptions ?? []).find((option: any) => option?.id === "thought_level" || option?.category === "thought_level")
  const offered: string[] = (option?.options ?? []).map((entry: any) => entry?.value).filter((value: unknown): value is string => typeof value === "string" && value !== "")
  const known = [...offered, ...levels.filter((level) => !offered.includes(level))]
  const listed = new Set((setup?.models?.availableModels ?? []).map((info: any) => info?.modelId))
  const efforts: Record<string, string[]> = {}
  for (const model of models) {
    const own = known.filter((level) => listed.has(`${model}-${level}`))
    if (own.length) efforts[model] = own
  }
  const current = (setup?.configOptions ?? []).find((option: any) => option?.id === "model" || option?.category === "model")?.currentValue
  if (!Object.keys(efforts).length && offered.length && typeof current === "string") efforts[current] = offered
  return efforts
}

/** Google's agent stopped while signing in: why, in a line (what it said goes to the log). */
class AgentStopped extends Error {
  constructor(readonly said: string) {
    // (By its last line: a traceback's error, not a warning before it.)
    const last = said.trim().split("\n").at(-1) ?? ""
    super(
      /PermissionError|Operation not permitted|Permission denied|EACCES|EPERM/i.test(last)
        ? "Google's Antigravity agent stopped as it started: it wasn't allowed into a folder it needs."
        : "Google's Antigravity agent stopped before signing in finished.",
    )
  }
}

/** Your other Google keys and settings, kept from it: it signs in its own way. */
const scrubbed = [
  "GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_LOCATION",
  "GOOGLE_CLOUD_QUOTA_PROJECT", "GOOGLE_GENAI_USE_VERTEXAI", "GCLOUD_PROJECT", "CLOUDSDK_CORE_PROJECT", "AGY_ACP_CCPA_PROJECT",
  "AGY_ACP_ENABLE_OAUTH", "GEMINI_HOME", "AGY_ACP_FORCE_FILE_STORAGE", "ANTIGRAVITY_HARNESS_PATH", "BROWSER", "KUVO_URL_FILE", "PYTHONUNBUFFERED",
  "ELECTRON_RUN_AS_NODE", "AGY_CLI_DISABLE_AUTO_UPDATE", "SSH_CONNECTION", "KUVO_AGY_PROGRAM", "CHROME_CRASHPAD_PIPE_NAME",
]

/** The subagents an invoke_subagent call starts: {Subagents: [{Role, TypeName, Prompt, Model}]}, as an object or as JSON. */
/** The subagents an `invoke_subagent` call asks for (its `Subagents`: a list, or the list written as JSON). */
export function antigravitySubagents(args: any): SubagentTask[] {
  let list = args?.Subagents
  if (typeof list === "string") {
    try {
      list = JSON.parse(list)
    } catch {
      return []
    }
  }
  if (!Array.isArray(list)) return []
  return list.map((subagent: any) => ({
    ...(typeof subagent?.Model === "string" && subagent.Model.trim() ? { requestedModel: subagent.Model.trim() } : {}),
    title: String(subagent?.Role ?? subagent?.TypeName ?? "A subagent's task"),
    prompt: typeof subagent?.Prompt === "string" ? subagent.Prompt : undefined,
    agent: typeof subagent?.TypeName === "string" && subagent.TypeName !== "self" ? subagent.TypeName : undefined,
  }))
}

/**
 * The image a generate_image call saved in the conversation's folder (`<image_name>_<time>.jpg`),
 * the newest; one saved since the turn began (`since`, ms), when it isn't named (the folder holds
 * your attachments and earlier pictures too).
 */
export function generatedImages(folder: string, input: Record<string, unknown>, since?: number): { type: "file"; uri: string; mime: string; name: string }[] {
  const stem = String(decoded(input.image_name ?? input.ImageName ?? "") ?? "")
  if (!stem && since === undefined) return []
  let names: string[]
  try {
    names = readdirSync(folder)
  } catch {
    return []
  }
  const types: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp", gif: "image/gif" }
  const images = names
    .map((name) => ({ name, type: types[name.split(".").pop()?.toLowerCase() ?? ""] }))
    .filter((file) => file.type && (!stem || file.name.startsWith(`${stem}_`) || file.name.startsWith(`${stem}.`)))
    .map((file) => ({ ...file, at: (() => {
      try {
        return statSync(join(folder, file.name)).mtimeMs
      } catch {
        return 0
      }
    })() }))
    .filter((file) => stem || file.at >= since!)
    .sort((a, b) => b.at - a.at)
  const newest = images[0]
  return newest ? [{ type: "file", uri: pathToFileURL(join(folder, newest.name)).href, mime: newest.type!, name: newest.name }] : []
}

/** One step of Antigravity's record of a conversation (`brain/<id>/.system_generated/logs/transcript.jsonl`). */
interface Step {
  step_index: number
  type?: string
  status?: string
  created_at?: string
  truncated_fields?: string[]
  content?: string
  tool_calls?: { name?: string; args?: Record<string, unknown> }[]
}

interface Child {
  id: string
  row: string
  task: SubagentTask
  run: SubagentRun
  /** Each step shown so far, with its status then. */
  shown: Map<number, string>
  /** Its tool calls waiting for their results, oldest first, and the step that gave each a result. */
  open: string[]
  results: Map<number, string>
  toolsSinceText: boolean
  ended: boolean
}

/**
 * Antigravity tells the conversation's own words and tools over its protocol, but not its
 * subagents: it writes them down. This follows that record while a turn runs: a row and a session
 * for each subagent it starts (with its role and task), and each one's steps in its session.
 */
export class AntigravityFollower {
  private seen = new Set<number>()
  private pending: SubagentTask[] = []
  private children = new Map<string, Child>()
  private timer?: ReturnType<typeof setInterval>
  private turn?: Turn
  private hooks?: TurnHooks
  private usedSubagents = false
  private prompt?: {
    from: number
    sentAt?: number
    candidate?: number
    answer: () => { tail: string; length: number } | undefined
    finish: () => void
  }

  constructor(
    private readonly brain: string,
    private readonly sessionId: string,
    private readonly every = 1_000,
    restored?: readonly number[],
  ) {
    // What's there already (a conversation picked up again) was shown with its own turns.
    this.seen = new Set(restored ?? this.steps(sessionId).map(step => step.step_index))
  }

  /** Follows a turn of the conversation: each of its turns in turn, the one it began on its own too. */
  follow(turn: Turn, hooks: TurnHooks): this {
    clearInterval(this.timer)
    this.turn = turn
    this.hooks = hooks
    this.usedSubagents = false
    // One still at work as the last turn ended was ended with it: what it does next opens its session again.
    for (const child of this.children.values()) if (!child.ended && child.run.turn.isEnded) child.ended = true
    this.timer = setInterval(() => this.read(), this.every)
    return this
  }

  transferState(): number[] | false {
    // Already rendered subagent rows aren't in the ACP journal. Let this turn finish here.
    if (this.usedSubagents || this.pending.length || [...this.children.values()].some(child => !child.ended)) return false
    return [...this.seen]
  }

  watchPrompt(answer: () => { tail: string; length: number } | undefined, finish: () => void, sentAt?: number) {
    const from = sentAt === undefined ? this.steps(this.sessionId).reduce((last, step) => Math.max(last, step.step_index), -1) : -1
    this.prompt = { from, sentAt, answer, finish }
  }

  stop() {
    this.prompt = undefined
    clearInterval(this.timer)
    this.timer = undefined
    this.read()
    // One that finished its work is done; the rest end with the turn.
    for (const child of this.children.values()) if (!child.ended && this.isDone(child)) this.end(child)
  }

  private steps(conversation: string, full = false): Step[] {
    try {
      return readFileSync(join(this.brain, conversation, ".system_generated", "logs", full ? "transcript_full.jsonl" : "transcript.jsonl"), "utf8")
        .split("\n")
        .flatMap((line) => {
          try {
            const step = JSON.parse(line)
            return typeof step?.step_index === "number" ? [step as Step] : []
          } catch {
            return []
          }
        })
    } catch {
      return []
    }
  }

  read() {
    if (!this.turn || this.turn.isEnded) return
    const steps = this.steps(this.sessionId)
    for (const step of steps) {
      if (this.seen.has(step.step_index) || step.status === "RUNNING") continue
      this.seen.add(step.step_index)
      for (const call of step.tool_calls ?? []) if (call?.name === "invoke_subagent") {
        this.usedSubagents = true
        this.pending.push(...antigravitySubagents(call.args))
      }
      // "Created the following subagents": their conversations, in the order they were asked for.
      if (step.type === "INVOKE_SUBAGENT" && typeof step.content === "string") {
        for (const match of step.content.matchAll(/"conversationId":\s*"([^"]+)"/g)) this.start(match[1])
      }
    }
    for (const child of this.children.values()) this.readChild(child)
    this.readPromptEnd(steps)
  }

  private readPromptEnd(steps: Step[]) {
    const prompt = this.prompt
    const answer = prompt?.answer()
    if (!prompt || !answer) return
    const latest = steps.reduce<Step | undefined>((last, step) => !last || step.step_index > last.step_index ? step : last, undefined)
    const input = steps.findLast(step => step.type === "USER_INPUT" && step.step_index > prompt.from)
    const current = input && (prompt.sentAt === undefined || Date.parse(input.created_at ?? "") >= Math.floor(prompt.sentAt / 1000) * 1000)
    // The durable transcript can finish even when the ACP prompt response never arrives.
    // A tool-free, completed response to this prompt must also match the text ACP delivered.
    let terminal = current && latest && latest.step_index > input.step_index && latest.type === "PLANNER_RESPONSE" &&
      latest.status === "DONE" && !latest.tool_calls?.length ? latest : undefined
    if (terminal?.truncated_fields?.includes("content")) {
      const index = terminal.step_index
      terminal = this.steps(this.sessionId, true).find(step => step.step_index === index)
    }
    const candidate = terminal?.content?.length === answer.length && terminal.content.endsWith(answer.tail) ? terminal.step_index : undefined
    // Allow the normal ACP result (including usage and provider errors) to arrive first.
    if (candidate !== undefined && candidate === prompt.candidate) prompt.finish()
    else prompt.candidate = candidate
  }

  private start(id: string) {
    this.usedSubagents = true
    if (this.children.has(id)) return
    const task = this.pending.shift() ?? { title: "A subagent's task" }
    const row = `agy-subagent:${id}`
    this.turn!.toolStart(row, "subagent", { description: task.title, prompt: task.prompt, agent: task.agent })
    const run = this.hooks!.subagent(row, task)
    this.turn!.toolSucceeded(row, "Started.", { sessionID: run.sessionID })
    this.children.set(id, { id, row, task, run, shown: new Map(), open: [], results: new Map(), toolsSinceText: false, ended: false })
  }

  private readChild(child: Child) {
    const steps = this.steps(child.id)
    for (const step of steps) {
      const was = child.shown.get(step.step_index)
      if (was === step.status) continue
      // More to do after it was done (a message for it, a timer of its own): its session goes on.
      if (child.ended && was === undefined) {
        // (What it was asked first is in its session already; what it's told now shows as it comes.)
        child.run = this.hooks!.subagent(child.row, { ...child.task, prompt: undefined, sessionID: child.run.sessionID })
        child.ended = false
        child.toolsSinceText = false
      }
      child.shown.set(step.step_index, step.status ?? "")
      if (was === undefined) this.show(child, step)
      else this.settle(child, step)
    }
    if (!child.ended && this.isDone(child, steps)) this.end(child)
  }

  /** A step it took, in its session. */
  private show(child: Child, step: Step) {
    const turn = child.run.turn
    // The task it was given, and messages for it (a timer, another agent): in its session already, or not words of its own.
    if (step.type === "SYSTEM_MESSAGE" || step.type === "USER_INPUT") return
    if (step.type === "PLANNER_RESPONSE") {
      if (step.content?.trim()) {
        if (child.toolsSinceText) {
          turn.endStep()
          turn.startStep()
          child.toolsSinceText = false
        }
        turn.text(step.content)
        turn.endBlock()
      }
      ;(step.tool_calls ?? []).forEach((call, index) => {
        const id = `${child.id}:${step.step_index}:${index}`
        const tool = followedTool(call)
        turn.toolStart(id, tool.name, tool.input)
        child.open.push(id)
        child.toolsSinceText = true
      })
      return
    }
    // Anything else is what a tool call came back with, in order. One that goes on in the
    // background (a timer) came back too: its record just stays "running".
    const id = child.open[0]
    if (!id) return
    child.results.set(step.step_index, id)
    this.settle(child, step)
  }

  /** A tool's result, done (or failed). */
  private settle(child: Child, step: Step) {
    const id = child.results.get(step.step_index)
    if (!id || !child.open.includes(id)) return
    child.open = child.open.filter((open) => open !== id)
    const output = resultText(step.content)
    if (step.status === "ERROR" || step.status === "FAILED" || step.status === "CANCELED") child.run.turn.toolFailed(id, output || "The tool failed.")
    else child.run.turn.toolSucceeded(id, output)
  }

  /**
   * Done: its last step is its answer, and nothing it left running in the background (a timer)
   * is still to come back to it ("sender=<its task>" says one has).
   */
  private isDone(child: Child, steps = this.steps(child.id)): boolean {
    const last = steps.at(-1)
    if (!last || last.type !== "PLANNER_RESPONSE" || last.status !== "DONE" || last.tool_calls?.length) return false
    return steps.every((step) => {
      if (step.status !== "RUNNING") return true
      const task = /task id: (\S+)/.exec(step.content ?? "")?.[1]
      return Boolean(task && steps.some((later) => later.step_index > step.step_index && later.type === "SYSTEM_MESSAGE" && (later.content ?? "").includes(`sender=${task}`)))
    })
  }

  private end(child: Child) {
    child.ended = true
    child.run.end("succeeded")
  }
}

/** A tool call it wrote down, as the app shows the engine's. */
function followedTool(call: { name?: string; args?: Record<string, unknown> }): { name: string; input: Record<string, unknown> } {
  const args: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(call.args ?? {})) {
    if (key === "toolAction" || key === "toolSummary") continue
    args[key] = decoded(value)
  }
  const description = decoded(call.args?.toolSummary)
  const name = call.name ?? "tool"
  switch (name) {
    case "run_command":
      return { name: "shell", input: { command: args.CommandLine ?? args.Command, cwd: args.Cwd, description } }
    case "view_file":
      return { name: "read", input: { path: args.AbsolutePath ?? args.File } }
    case "write_to_file":
      return { name: "write", input: { path: args.TargetFile, content: args.CodeContent } }
    case "replace_file_content":
    case "multi_replace_file_content":
      return { name: "edit", input: { path: args.TargetFile, description } }
    case "grep_search":
      return { name: "grep", input: { pattern: args.Query, path: args.SearchPath } }
    case "list_dir":
      return { name: "list", input: { path: args.DirectoryPath } }
    case "find_by_name":
      return { name: "glob", input: { pattern: args.Pattern, path: args.SearchDirectory } }
    case "read_url_content":
      return { name: "webfetch", input: { url: args.Url } }
    case "search_web":
      return { name: "websearch", input: { query: args.query ?? args.Query } }
  }
  return { name, input: { ...args, ...(description ? { description } : {}) } }
}

/** Its arguments are written as JSON text ("\"hi\"", "180"): as values. */
function decoded(value: unknown): unknown {
  if (typeof value !== "string") return value
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}

/** A result without its "Created At: … Completed At: …" lines. */
function resultText(content: string | undefined): string {
  return (content ?? "").replace(/^(Created At|Completed At):[^\n]*\n?/gm, "").trim()
}

export class AntigravityAgent implements Agent {
  id = "antigravity"
  readonly providerID: string
  get name() {
    return accountName("Antigravity", this.instance)
  }
  /** Google's installer, as its docs give it (it puts `agy` in `~/.local/bin`). */
  install = "curl -fsSL https://antigravity.google/cli/install.sh | bash"

  constructor(
    readonly context: AgentContext,
    readonly instance: AgentAccount = { id: "default" },
  ) {
    this.providerID = providerID("antigravity", instance.id)
  }

  private get root() {
    return join(this.context.home, "antigravity")
  }

  /** Google's earlier agent (1.3.0), downloaded for an account signed in before `agy`. */
  private get folder() {
    return join(this.root, version)
  }

  /** Its home (GEMINI_HOME): Kuvo Code's own, never your `~/.gemini`, as T3 Code keeps one: its
   * sign-in, settings and conversations; an added account's in its own folder. */
  get profile() {
    return this.instance.home ? join(this.instance.home, "profile") : join(this.root, "profile")
  }

  /** The earlier agent's sign-in (Google's 1.3 kept it in a file). */
  private get tokenFile() {
    return join(this.profile, "antigravity-acp", "acp_token.json")
  }

  /** `agy`'s sign-in here, once `agy` itself said it's signed in (Google keeps the secret itself). */
  private get signInFile() {
    return join(this.profile, "antigravity-acp", "kuvo-sign-in.json")
  }

  private get modelsFile() {
    return join(this.profile, "kuvo-models.json")
  }

  executable(): string | undefined {
    return agyProgram()
  }

  /** `agy`'s sign-in here: an id of Kuvo Code's own for it (never a secret), when it has one. */
  private agySignIn(): { id: string } | undefined {
    try {
      const kept = JSON.parse(readFileSync(this.signInFile, "utf8"))
      return typeof kept?.id === "string" && kept.id ? { id: kept.id } : undefined
    } catch {
      return undefined
    }
  }

  /** Signed in before `agy`, on Google's earlier agent, which still runs it until you sign in again. */
  private earlier(): boolean {
    return !this.agySignIn() && existsSync(this.tokenFile) && existsSync(join(this.folder, ".complete"))
  }

  /** Which runs it: `agy` (signed in there, or not signed in at all), or Google's earlier agent. */
  private runtime(): "agy" | "earlier" {
    return this.earlier() ? "earlier" : "agy"
  }

  usageSignIn(): string | undefined {
    const agy = this.agySignIn()
    if (agy) return fingerprint(agy.id)
    try {
      const credential = antigravityCredential(JSON.parse(readFileSync(this.tokenFile, "utf8")))
      return credential ? antigravityGeneration(credential) : undefined
    } catch { return undefined }
  }

  /** The sign-in its saved model list belongs to: a new sign-in is someone else until it lists its own. */
  private modelCredential(): string | undefined {
    const agy = this.agySignIn()
    if (agy) return fingerprint(agy.id)
    try {
      const credential = antigravityCredential(JSON.parse(readFileSync(this.tokenFile, "utf8")))
      return fingerprint(credential?.refresh ?? credential?.access)
    } catch {
      return undefined
    }
  }

  methods() {
    return [{ type: "oauth" as const, id: "oauth-personal", label: "Sign in with Google" }]
  }

  /** Its sign-in's file as it was when Google ended the sign-in: until it's new. */
  private endedSignIn?: number

  private signInMark(): number | undefined {
    try {
      return statSync(this.runtime() === "earlier" ? this.tokenFile : this.signInFile).mtimeMs
    } catch {
      return undefined
    }
  }

  /** Google ended the sign-in: shown, and skipped by Custom Auto, until you sign in again. */
  signInEnded() {
    this.endedSignIn = this.signInMark()
    emit("integration.updated", { integrationID: this.providerID })
  }

  async account(): Promise<Account> {
    if (this.endedSignIn !== undefined) {
      if (this.signInMark() === this.endedSignIn) return { installed: Boolean(this.executable()) || this.earlier(), signedIn: false, ended: true, message: "Google ended this sign-in. Sign in again." }
      this.endedSignIn = undefined
    }
    // Signed in before `agy`: it keeps running on Google's earlier agent, until you sign in again.
    if (this.earlier()) return { installed: true, signedIn: true, label: "Google account", what: true, identity: this.modelCredential(), message: "Sign in again to update Antigravity." }
    if (!this.executable()) return { installed: false, signedIn: false, message: "Install Antigravity's CLI, then sign in here." }
    if (!this.agySignIn()) return { installed: true, signedIn: false, message: "Sign in with your Google account." }
    return { installed: true, signedIn: true, label: "Google account", what: true, identity: this.modelCredential() }
  }

  /** Google's own updater (`agy update`), and its release feed for the newest. */
  private get definition(): MaintenanceDefinition | undefined {
    const executable = this.executable()
    if (!executable) return undefined
    return { name: "Antigravity's CLI", executable, newest: agyNewest, nativeUpdate: { args: ["update"], isNative: () => true } }
  }

  async maintenance(fresh = false) {
    const definition = this.definition
    return definition ? maintenance(definition, fresh) : {}
  }

  async update(attempt?: string) {
    const definition = this.definition
    if (!definition) return { updated: false, message: "Antigravity's CLI isn't installed." }
    return update(definition, this.context.temporary, { home: this.context.home, program: this.id, attempt })
  }

  /** Its program, with Google's installer. */
  async installProgram() {
    return install("Antigravity's CLI", this.install, this.context.temporary, () => this.executable())
  }

  /** Counts updates of `agy`: a session's agent started before the last one starts again. */
  program = 0

  programUpdated() {
    this.program++
  }

  /** Where it reads skills itself: its home's (the CLI's) and a project's `.agents/skills` and `.gemini/skills`. */
  ownSkillFolders(directory: string): string[] {
    return [join(this.profile, "antigravity-cli", "skills"), join(directory, ".agents", "skills"), join(directory, ".gemini", "skills")]
  }

  /**
   * Its places, run whole in the box's sandbox (by the environment it starts with): its home
   * (GEMINI_HOME: your sign-in, its settings, conversations and skills, Kuvo Code's own) and its
   * temporary folder (it unpacks itself there) written whole.
   */
  places(env: Record<string, string | undefined>) {
    return { homes: [env.GEMINI_HOME, env.TMPDIR].filter((place): place is string => Boolean(place)), state: ["*"], kept: [] }
  }

  /** What it only reads there: its program. */
  reads(env: Record<string, string | undefined>): string[] {
    return [env.ANTIGRAVITY_HARNESS_PATH ?? env.KUVO_AGY_PROGRAM].filter((place): place is string => Boolean(place)).map((place) => dirname(place))
  }

  /**
   * How it's started: its own home and temporary folder (it unpacks itself there, about 1 GB), and
   * nothing of your other Google keys. `agy` keeps its sign-in in a file of its home rather than
   * the system keyring (which it shares between every home) when it thinks it runs over SSH, as
   * T3 Code starts it; and doesn't update itself unless you said agents may.
   */
  private launch(runtime = this.runtime()): { argv: string[]; env: Record<string, string | undefined>; cleanEnv: boolean; onExit: () => void; temporary: string } {
    mkdirSync(join(this.profile, "antigravity-acp", "tmp"), { recursive: true, mode: 0o700 })
    const settings = join(this.profile, "antigravity-acp", "settings.json")
    if (!existsSync(settings)) writeFileSync(settings, JSON.stringify({ auth: { type: "oauth-personal" } }))
    const temporary = join(this.context.temporary, "antigravity", newID("att"))
    mkdirSync(temporary, { recursive: true, mode: 0o700 })
    let env: Record<string, string | undefined> = {}
    for (const [key, value] of Object.entries(process.env)) if (!scrubbed.includes(key.toUpperCase())) env[key] = value
    // On a machine, its commands find the machine's own settings (`gh`, git's).
    env = withOwnFolders(env)
    // (No browser of Kuvo's: its commands open pages as they would anywhere, and only `agy`'s own
    // words, or its refusing its sign-in, say it wants a new one.)
    Object.assign(env, { GEMINI_HOME: this.profile, TMPDIR: temporary })
    const onExit = () => rmSync(temporary, { recursive: true, force: true })
    if (runtime === "earlier") {
      Object.assign(env, { AGY_ACP_FORCE_FILE_STORAGE: "1", PYTHONUNBUFFERED: "1", ANTIGRAVITY_HARNESS_PATH: join(this.folder, harnessName) })
      // (Not its start-up syslog, which leaves a marker folder in /tmp, whatever TMPDIR says.)
      const argv = [join(this.folder, executableName), "--nosyslog_on_start", ...(process.platform === "linux" ? ["--uid="] : [])]
      return { argv, env, cleanEnv: true, onExit, temporary }
    }
    const agy = this.executable()
    if (!agy) {
      onExit()
      throw new Error(`Install Antigravity's CLI first: ${this.install}`)
    }
    let real = agy
    try {
      real = realpathSync(agy)
    } catch {}
    Object.assign(env, { SSH_CONNECTION: "127.0.0.1 0 127.0.0.1 0", KUVO_AGY_PROGRAM: real, ...(agentsUpdateThemselves() ? {} : { AGY_CLI_DISABLE_AUTO_UPDATE: "1" }) })
    return { argv: [agy, "acp", "--gemini-dir", this.profile, "--app-data-dir", "antigravity-acp"], env, cleanEnv: true, onExit, temporary }
  }

  /** Its sign-in page's code is pasted back here, from any device. */
  readonly signsInRemotely = true

  /**
   * Signs in with `agy acp --login` in its home: it prints Google's page, and takes the code that
   * page ends on. Once it says it worked, `agy` itself is asked to use the sign-in (and lists the
   * models this account has): only then is it signed in here.
   */
  async signIn(): Promise<SignInAttempt> {
    const agy = this.executable()
    if (!agy) throw new Error(`Install Antigravity's CLI first: ${this.install}`)
    const launch = this.launch("agy")
    let exit: number | undefined
    let checked: Promise<boolean> | undefined
    let why: string | undefined
    const verify = () => (checked ??= this.verifySignIn().then(() => true, (error) => {
      // (Google's own refusals, by what it said, stopped or not; what it said goes to the log.)
      const said = String(error instanceof AgentStopped ? error.said : (error?.message ?? error))
      why = /SUBSCRIPTION_REQUIRED/i.test(said)
        ? "Google says this account needs an Antigravity subscription."
        : error instanceof AgentStopped
          ? error.message
          : `Antigravity didn't take the sign-in: ${failure(error).message}`
      return false
    }))
    // (Your other Google keys left out, as in its sessions; and no display, so it prints its page
    // rather than opening a browser here.)
    const unset = Object.fromEntries([...Object.keys(process.env).filter((key) => scrubbed.includes(key.toUpperCase())), "DISPLAY", "WAYLAND_DISPLAY"].map((key) => [key, undefined]))
    return commandSignIn({
      command: launch.argv[0],
      args: [...launch.argv.slice(1), "--login"],
      env: { ...unset, ...launch.env },
      home: this.context.home,
      temporary: this.context.temporary,
      instructions: "Sign in with Google on its page, then paste the code it gives you here.",
      // (Before it ends, a sign-in from before isn't this one.)
      signedIn: async () => (exit === undefined ? false : exit === 0 && (await verify())),
      codePrompt: /paste the authorization code[^\n]*press Enter:\s*/gi,
      printedPage: true,
      whyNot: () => why,
      onEnd: (code) => {
        exit = code
        launch.onExit()
      },
      onComplete: () => {
        this.endedSignIn = undefined
        this.programUpdated()
        emit("model.updated", {})
      },
    })
  }

  /** Asks `agy` to use its sign-in, and to list this account's models; noted as signed in only then. */
  private async verifySignIn() {
    const { rpc, cwd, initialize } = this.modelConnection(() => rpc.kill(), "agy")
    try {
      await initialize(120_000)
      await rpc.request("authenticate", { methodId: "oauth-personal" }, 120_000)
      const setup: any = await rpc.request("session/new", { cwd, mcpServers: [] }, 120_000)
      if (setup?.sessionId) this.removeSessionFiles(setup.sessionId)
      const id = newID("agy")
      placeWhole(this.signInFile, JSON.stringify({ id, at: Date.now() }))
      // (Google's earlier agent's sign-in is behind it now.)
      rmSync(this.tokenFile, { force: true })
      this.saveModels(setup, fingerprint(id))
    } catch (error) {
      // Stopped (not refused): what it said (a traceback, most often) is for the log, not the sheet.
      if (rpc.isClosed && !(error instanceof RPCError)) {
        await rpc.errorsRead
        console.error(`Google's Antigravity agent stopped while signing in:\n${rpc.stderr.trim()}`)
        throw new AgentStopped(rpc.stderr)
      }
      throw error
    } finally {
      rpc.kill()
    }
  }

  /** `agy` forgets its sign-in itself (its ACP `logout`), then what Kuvo Code kept of it goes. */
  private async forget() {
    if (this.agySignIn() && this.executable()) {
      const { rpc, initialize } = this.modelConnection(() => rpc.kill(), "agy")
      try {
        const init: any = await initialize(60_000)
        if (init?.agentCapabilities?.auth?.logout) await rpc.request("logout", {}, 90_000)
      } catch (error) {
        console.error(`Antigravity couldn't sign out: ${failure(error).message}`)
      } finally {
        rpc.kill()
      }
    }
    rmSync(this.signInFile, { force: true })
    rmSync(this.tokenFile, { force: true })
    rmSync(this.modelsFile, { force: true })
    this.listedHere = undefined
  }

  async signOut() {
    await this.forget()
  }

  /** An added account signs out as it goes: `agy` forgets the sign-in in its home, before the home goes. */
  async removeAccount() {
    await this.forget()
  }

  private saveModels(setup: any, credential: string | undefined) {
    if (!credential || credential !== this.modelCredential()) return false
    const option = (setup?.configOptions ?? []).find((option: any) => option.id === "model" || option.category === "model")
    const flat: { value: string; name: string }[] = []
    for (const entry of option?.options ?? []) {
      if (Array.isArray(entry.options)) for (const inner of entry.options) flat.push({ value: inner.value, name: inner.name ?? inner.value })
      else if (entry.value) flat.push({ value: entry.value, name: entry.name ?? entry.value })
    }
    if (!flat.length) {
      for (const info of setup?.models?.availableModels ?? []) flat.push({ value: info.modelId, name: info.name ?? info.modelId })
    }
    const listed = flat.length || (Array.isArray(setup?.models?.availableModels) && !setup.models.availableModels.length) ||
      (Array.isArray(option?.options) && !option.options.length)
    if (!listed || flat.some(info => typeof info.value !== "string" || !info.value || typeof info.name !== "string")) return false
    const efforts = antigravityEfforts(setup, flat.map((info) => info.value))
    const models = flat.map((info) => (efforts[info.value] ? { ...info, efforts: efforts[info.value] } : info))
    placeWhole(this.modelsFile, JSON.stringify({ credential, current: option?.currentValue, models }))
    this.listedHere = credential
    return true
  }

  private listedHere?: string

  /** Only once Antigravity said its models in this bridge's time (its file may be an older list). */
  listsEveryModel(): boolean {
    return !!this.listedHere && this.listedHere === this.modelCredential()
  }

  /** A session made only to check the sign-in leaves nothing behind. */
  private removeSessionFiles(sessionID: string) {
    if (typeof sessionID !== "string" || !/^[a-z0-9][a-z0-9_-]{0,127}$/i.test(sessionID) || !existsSync(this.profile)) return
    const folder = join(realpathSync(this.profile), "antigravity-acp")
    const conversations = join(folder, "conversations")
    const brain = join(folder, "brain")
    if (existsSync(conversations) && realpathSync(conversations) === conversations) {
      for (const suffix of [".db", ".db-wal", ".db-shm", ".db-journal", ".meta"]) rmSync(join(conversations, sessionID + suffix), { force: true })
    }
    if (existsSync(brain) && realpathSync(brain) === brain) rmSync(join(brain, sessionID), { recursive: true, force: true })
  }

  private modelConnection(found: (url: string) => void, runtime = this.runtime()) {
    const launch = this.launch(runtime)
    // It starts in an empty folder in its own temporary one (not where it unpacks itself), one of
    // its own places in the sandbox: it reads the folder it starts in, and the bridge's temporary
    // folder may be inside one the sandbox hides (on a Mac, the app's own temporary folder).
    const cwd = join(launch.temporary, "work")
    let whole: ReturnType<typeof fenceAgent>
    try {
      mkdirSync(cwd)
      // In the box's sandbox while it's on, as its sessions run: what it reads as it starts
      // (settings, skills, servers) runs there too, never outside.
      whole = fenceRules() ? fenceAgent(launch.argv[0], cwd, { ...this.places(launch.env), read: this.reads(launch.env) }) : undefined
      if (fenceRules() && !whole) throw new Error("Antigravity runs in the sandbox here, and it isn't ready: restart Kuvo Code.")
    } catch (error) {
      launch.onExit()
      throw error
    }
    const rpc = new RPC(whole ? [whole.command, ...launch.argv.slice(1)] : launch.argv, {
      cwd,
      env: { ...launch.env, ...whole?.env },
      cleanEnv: true,
      jsonrpc: true,
      firstID: 2 ** 30,
      filter: (line) => {
        const at = asksForSignIn(line) ? line.indexOf(signInLine) : -1
        if (at >= 0) {
          found(line.slice(at + signInLine.length).trim())
          return false
        }
        return true
      },
    })
    rpc.onExit = () => {
      whole?.ended()
      launch.onExit()
    }
    const initialize = (timeout: number) => rpc.request("initialize", {
      protocolVersion: 1, clientCapabilities: { fs: { readTextFile: !whole, writeTextFile: !whole }, terminal: false },
      clientInfo: { name: "kuvo-code", version: "1" },
    }, timeout)
    return { rpc, cwd, launch, initialize }
  }

  /** Whether it can run: `agy` signed in, or the earlier agent an account still runs on. */
  private canRun() {
    return this.earlier() || Boolean(this.executable() && this.agySignIn())
  }

  private async discoverModels() {
    const credential = this.modelCredential()
    // (It only uses the sign-in it has: asking for a new one ends it.)
    const { rpc, cwd, initialize } = this.modelConnection(() => rpc.kill())
    try {
      await initialize(20_000)
      await rpc.request("authenticate", { methodId: "oauth-personal" }, 20_000)
      const setup: any = await rpc.request("session/new", { cwd, mcpServers: [] }, 20_000)
      if (setup?.sessionId) this.removeSessionFiles(setup.sessionId)
      if (!this.saveModels(setup, credential)) throw new Error("Antigravity didn’t list its models.")
    } finally {
      rpc.kill()
    }
  }

  async models(options: { refresh?: boolean } = {}): Promise<Model[]> {
    if (options.refresh && this.canRun()) await this.discoverModels()
    const credential = this.modelCredential()
    const saved = JSON.parse(readFileSync(this.modelsFile, "utf8")) as { credential?: string; current?: string; models: { value: string; name: string; efforts?: string[] }[] }
    if (!credential || saved?.credential !== credential || !Array.isArray(saved.models) || saved.models.some(info => !info || typeof info.value !== "string" || !info.value || typeof info.name !== "string")) {
      throw new Error("Antigravity’s model list couldn’t be read.")
    }
    return saved.models.map((info, index) => model(this.providerID, info.value, antigravityModelName(info.name), { images: true, released: info.value === saved.current ? 2000 : 1000 - index, context: this.windows.get(info.value) ?? 1_000_000, variants: Array.isArray(info.efforts) ? info.efforts.filter((level) => typeof level === "string") : undefined }))
  }

  /** How much each model holds, as Antigravity says in a turn (its models aren't all Gemini's 1M). */
  private windows = new ContextWindows()
  thread(session: Session, state: Record<string, unknown>): Thread {
    const agent = this
    const credentials = new WeakMap<RPC, string>()
    const followers = new Map<string, AntigravityFollower>()
    return new AcpThread(
      {
        noteContext(model, size) {
          if (agent.windows.note(model, size)) emit("model.updated", {})
        },
        // (`agy`, or Google's earlier agent pinned to its version.)
        held: { kind: "antigravity", provider: agent.providerID, version: agent.runtime() === "agy" ? "agy" : version },
        fileSystem: true,
        readsUnsaid: true,
        whole: { homes: (env) => agent.places(env).homes, state: ["*"], kept: [], read: (env) => agent.reads(env) },
        authenticatesFirst: true,
        resumes: true,
        // Google's quotas are each model's own.
        limitScope: "model",
        // Its subagents: from its own record of the conversation (it doesn't tell them).
        // (One for the conversation, from turn to turn: what it has shown isn't shown again.)
        follow: (sessionId, turn, hooks, restored) => {
          let follower = followers.get(sessionId)
          if (!follower) followers.set(sessionId, (follower = new AntigravityFollower(join(agent.profile, "antigravity-acp", "brain"), sessionId, 1_000, Array.isArray(restored) ? restored.filter(Number.isSafeInteger) : undefined)))
          return follower.follow(turn, hooks)
        },
        hides: (call) => /\b(start_subagent|invoke_subagent)\b/.test(`${call?.title ?? ""} ${call?.rawInput?.name ?? ""}`),
        // An image it made is saved in its folder for the conversation, not handed back.
        madeFiles: (call, sessionId, since) => (/\bgenerate_image\b/.test(String(call?.title ?? "")) ? generatedImages(join(agent.profile, "antigravity-acp", "brain", sessionId), call?.rawInput ?? {}, since) : []),
        authMethod: () => "oauth-personal",
        async command() {
          if (!agent.canRun()) throw new Error("Sign in to Antigravity in Settings › Accounts first.")
          const credential = agent.modelCredential()
          return { ...agent.launch(), onOpen: (rpc: RPC) => { if (credential) credentials.set(rpc, credential) } }
        },
        // Settings › Skills' skills, in the skills folder of its home (Kuvo Code's): it reads
        // them from there as it starts. Its home is the account's, for all its sessions.
        async skills(directory, hooks) {
          return copySkillsInto(join(agent.profile, "config", "skills"), await skillsFor(directory, agent.ownSkillFolders(directory), turnedOff(hooks)))
        },
        // Your subagents (Settings › Agents), as its custom agents in its home's folder for them,
        // on Flash or Pro when their model is one (it takes no other, nor a model for its own).
        async subagents(directory) {
          return antigravityCustomAgents(agent.profile, await subagentsFor(directory))
        },
        // Its access lasts an hour, renewed from the sign-in it keeps: one that asks for a new
        // sign-in partway is started again, which renews it.
        asksSignIn: asksForSignIn,
        // `agy` asked to use a sign-in it no longer has: "Authentication required", or no browser
        // to sign in again with (it thinks it runs over SSH).
        signInRefused: (error) => error instanceof RPCError && (error.code === -32000 || /No browser is available here|Authentication required/i.test(error.message)),
        // "Usage Limit Reached. You have reached your current quota for this period. Your limit
        // will reset in 6 days, 19 hours.", as its whole answer.
        limitReply: (text) => {
          const plain = text.replace(/[#*_>`]/g, " ").replace(/\s+/g, " ").trim().toLowerCase()
          return /^usage limit reached\.? you have reached your current quota for this period\.?(?: your limit will reset in [^.]{1,80}\.?)?$/.test(plain)
        },
        signInEnded: () => agent.signInEnded(),
        async selectModel(rpc, sessionId, modelID, setup, variant) {
          if (setup?.kuvoModel !== modelID) {
            await rpc.request("session/set_config_option", { sessionId, configId: "model", value: modelID }, 30_000)
            setup.kuvoModel = modelID
            agent.saveModels(setup, credentials.get(rpc))
          }
          // Its thinking level: the one picked, set once as the conversation starts (set later,
          // it would drop what it has cached of the conversation).
          const fresh = setup?.kuvoFresh
          if (setup) delete setup.kuvoFresh
          const option = (setup?.configOptions ?? []).find((option: any) => option?.id === "thought_level" || option?.category === "thought_level")
          if (fresh && variant && option) await rpc.request("session/set_config_option", { sessionId, configId: option.id, value: variant }, 30_000)
        },
        async selectMode(rpc, sessionId, { approvesAll, plan }, setup) {
          const option = (setup?.configOptions ?? []).find((option: any) => option.id === "mode" || option.category === "mode")
          const values: string[] = (option?.options ?? []).map((entry: any) => entry.value)
          const wanted = plan && values.includes("plan") ? "plan" : approvesAll ? "yolo" : "default"
          const current = setup?.kuvoMode ?? option?.currentValue
          if (current === wanted) return
          // Running without asking where it should ask (an earlier Approve All session's mode),
          // and no way out of it: not this turn.
          const cantAsk = () => new Error("Antigravity couldn't be set to ask before it acts, so it didn't start. Try again, or pick Approve All for this session.")
          if (!option || (values.length && !values.includes(wanted))) {
            if (wanted !== "yolo" && current === "yolo") throw cantAsk()
            return
          }
          try {
            await rpc.request("session/set_config_option", { sessionId, configId: option.id, value: wanted }, 30_000)
          } catch {
            // Not running without asking when it was to: it asks, and that's answered for it.
            if (wanted !== "yolo") throw cantAsk()
            return
          }
          setup.kuvoMode = wanted
        },
      },
      session,
      state,
      this.name,
    )
  }
}


