// The Cursor worker owns the SDK's live agent and run objects. On a box it can keep them across
// a bridge replacement through a holder. Where the sandbox is on, the worker runs through the
// box's launcher (`run/fence/shell/run`). The bridge handles account, sign-in and model requests.
// The worker exposes the SDK's Agent interface to the thread.

import { readFileSync, renameSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { RPCClosed, RPC, RPCError } from "../rpc"
import type { Held } from "../hold/client"
import { noteTurn, type HeldTurn } from "../hold/adopt"

/**
 * The worker, beside Cursor's SDK in its folder (where the SDK looks for its helpers, as beside the
 * bridge): each agent made or picked up again, each message sent, every update it sends back as it
 * comes (`delta`), and how each run ended (`ended`).
 */
const WORKER = `// Kuvo Code's agents bridge's Cursor, run in the box's sandbox (cursor-worker.ts).
console.log = console.error
// What ran it as Bun, never what Cursor's commands run with.
for (const name of ["BUN_BE_BUN", "BUN_RUNTIME_TRANSPILER_CACHE_PATH"]) delete process.env[name]
const sdk = await import(Bun.resolveSync("@cursor/sdk", import.meta.dir))
const agents = new Map()
const runs = new Map()
let warmed = false
const send = (message) => process.stdout.write(JSON.stringify(message) + "\\n")
const plain = (error) => ({ message: String(error?.message ?? (typeof error?.error === "string" ? error.error : error)), code: error?.code, name: error?.name, status: error?.status })
// Its conversations' store, by its folder (\`JsonlLocalAgentStore\`).
const made = (options) => (options?.local?.store ? { ...options, local: { ...options.local, store: new sdk.JsonlLocalAgentStore(options.local.store.root) } } : options)
async function answer(method, params) {
  if (method === "create") {
    const { workerPrewarm, ...options } = params.options
    if (workerPrewarm && !warmed) {
      warmed = true
      try {
        const cwd = options.local.cwd
        const platform = await sdk.createAgentPlatform({ workspaceRef: cwd })
        const release = await platform.prewarmLocalWorkspace({ apiKey: options.apiKey, local: { cwd, settingSources: [], sandboxOptions: { enabled: true } } })
        await release?.()
      } catch {}
    }
    const agent = params.resume ? await sdk.Agent.resume(params.resume, made(options)).catch(() => undefined) : await sdk.Agent.create(made(options))
    if (!agent) return { agentId: null }
    agents.set(agent.agentId, agent)
    return { agentId: agent.agentId }
  }
  if (method === "send") {
    const agent = agents.get(params.agent)
    if (!agent) throw new Error("Cursor's agent isn't here anymore.")
    const run = await agent.send(params.message, {
      ...params.options,
      onDelta: async ({ update }) => {
        try {
          send({ method: "delta", params: { run: params.run, update } })
        } catch (error) {
          console.error("Cursor sent an update Kuvo Code couldn't pass on: " + plain(error).message)
        }
      },
    })
    runs.set(params.run, run)
    run.wait().then(
      (result) => send({ method: "ended", params: { run: params.run, result: result?.error ? { ...result, error: plain(result.error) } : result } }),
      (error) => send({ method: "ended", params: { run: params.run, error: plain(error) } }),
    ).finally(() => runs.delete(params.run))
    return {}
  }
  if (method === "cancel") {
    await runs.get(params.run)?.cancel()
    return {}
  }
  if (method === "close") {
    const agent = agents.get(params.agent)
    agents.delete(params.agent)
    agent?.close?.()
    return {}
  }
  throw new Error("Method not found: " + method)
}
let buffer = ""
process.stdin.setEncoding("utf8")
process.stdin.on("data", (chunk) => {
  buffer += chunk
  let at
  while ((at = buffer.indexOf("\\n")) >= 0) {
    let message
    try {
      message = JSON.parse(buffer.slice(0, at))
    } catch {}
    buffer = buffer.slice(at + 1)
    if (message?.id === undefined) continue
    answer(message.method, message.params ?? {}).then(
      (result) => send({ id: message.id, result }),
      (error) => send({ id: message.id, error: { code: -32000, message: String(error?.message ?? error), data: plain(error) } }),
    )
  }
})
process.stdin.on("end", () => process.exit(0))
`

/** The worker's file, written beside Cursor's SDK in `folder` (where only the bridge writes), whole before another session's worker reads it. */
export function cursorWorker(folder: string): string {
  const file = join(folder, "kuvo-cursor-worker.mjs")
  try {
    if (readFileSync(file, "utf8") === WORKER) return file
  } catch {}
  const written = `${file}.${process.pid}.${Math.random().toString(36).slice(2)}`
  writeFileSync(written, WORKER, { mode: 0o600 })
  renameSync(written, file)
  return file
}

/** An error as the SDK threw it in the worker: its words, and what tells a limit or a sign-in apart. */
function revived(error: any): Error {
  const data = error instanceof RPCError ? (error.data as any) : error
  return Object.assign(new Error(data?.message ?? error?.message ?? "Cursor stopped with an error."), {
    ...(data?.code !== undefined ? { code: data.code } : {}),
    ...(data?.name ? { name: data.name } : {}),
    ...(data?.status !== undefined ? { status: data.status } : {}),
  })
}

type Delta = (event: { update: any }) => Promise<void>
export type CursorRun = { wait: () => Promise<any>; cancel: () => Promise<unknown> }

/** Cursor's SDK's `Agent`, as far as a thread uses it, in a worker run in the sandbox. */
export class CursorWorker {
  private readonly rpc: RPC
  private next: number
  private readonly runs = new Map<number, { delta: (update: any) => Promise<void>; ended: (params: any) => void }>()

  /** Its conversations' store, by its folder (made in the worker), as the SDK's is made (`agentStore`). */
  static readonly JsonlLocalAgentStore = class {
    constructor(readonly root: string) {}
  }

  constructor(command: string[] | Held, options: { cwd?: string; env?: Record<string, string | undefined>; ended?: () => void; session?: string; adopting?: boolean } = {}) {
    const generation = Array.isArray(command) ? 0 : command.generation
    this.next = generation * 1_000_000 + 1
    this.rpc = new RPC(command, { cwd: options.cwd, env: options.env, firstID: this.next })
    this.rpc.onNotification = (method, params) => {
      const run = this.runs.get(params?.run)
      if (!run) return
      if (method === "delta") void run.delta(params.update)
      else if (method === "ended") {
        this.runs.delete(params.run)
        run.ended(params)
      }
    }
    // What was running in it ends with it.
    this.rpc.onExit = () => {
      const error = this.rpc.stderr.trim().split("\n").slice(-3).join("\n") || "Cursor stopped."
      for (const [id, run] of this.runs) {
        this.runs.delete(id)
        run.ended({ error: this.rpc.agentExited ? new RPCClosed(error) : { message: error } })
      }
      options.ended?.()
    }
    this.session = options.session
    if (!options.adopting) this.rpc.start()
  }

  private readonly session?: string

  private watchRun(run: number, onDelta: Delta, agentId: string): CursorRun {
    let told = Promise.resolve()
    const ended = new Promise<any>((resolve, reject) =>
      this.runs.set(run, {
        delta: update => (told = told.then(() => onDelta({ update })).catch(() => {})),
        ended: params => {
          if (this.rpc.holder) noteTurn(this.rpc.holder, agentId, undefined)
          void told.then(() => (params.error ? reject(params.error instanceof RPCClosed ? params.error : revived(params.error)) : resolve(params.result)))
        },
      }),
    )
    ended.catch(() => {})
    return {
      wait: () => ended,
      cancel: () => this.rpc.request("cancel", { run }).catch(() => {}),
    }
  }

  /** The live SDK run stays in the worker. Subscribe to its old run ID before replay begins. */
  continue(taken: HeldTurn, onDelta: Delta): CursorRun {
    const run = Number(taken.note.turn)
    const active = this.watchRun(run, onDelta, taken.thread)
    this.rpc.onStray = (id, _result, error) => {
      if (id !== taken.note.request || !error) return
      const watching = this.runs.get(run)
      this.runs.delete(run)
      watching?.ended({ error })
    }
    taken.gate.onOpen(() => this.rpc.start())
    taken.gate.ready(taken.thread)
    return active
  }

  get isClosed() {
    return this.rpc.isClosed
  }

  readonly Agent = {
    create: (options: any) => this.agent(options),
    resume: (id: string, options: any) => this.agent(options, id),
  }

  private async agent(options: any, resume?: string) {
    const { agentId } = await this.rpc.request<{ agentId: string | null }>("create", { options, resume }).catch((error) => Promise.reject(error instanceof RPCClosed && this.rpc.agentExited ? error : revived(error)))
    if (!agentId) return undefined
    return this.attached(agentId)
  }

  /**
   * Its SDK agent `agentId`, live in it already (a worker the bridge before this one started, taken
   * up as it is): only what every worker answers (`send`, `close`), whichever bridge wrote it.
   */
  attached(agentId: string) {
    const worker = this
    const rpc = this.rpc
    return {
      agentId,
      close: () => void rpc.request("close", { agent: agentId }).catch(() => {}),
      async send(message: unknown, { onDelta, ...sendOptions }: { onDelta: Delta; [name: string]: unknown }) {
        const run = worker.next++
        const held = rpc.holder
        const active = worker.watchRun(run, onDelta, agentId)
        if (held && worker.session) noteTurn(held, agentId, { session: worker.session, turn: String(run), from: held.seen, request: rpc.nextID })
        try {
          await rpc.request("send", { agent: agentId, run, message, options: sendOptions })
        } catch (error) {
          worker.runs.delete(run)
          if (held) noteTurn(held, agentId, undefined)
          throw error instanceof RPCClosed && rpc.agentExited ? error : revived(error)
        }
        return active
      },
    }
  }

  /** More for its holder's record, for a bridge taking it up (none outside a holder). */
  note(meta: Record<string, unknown>) {
    this.rpc.holder?.note(meta)
  }

  stop() {
    this.rpc.kill()
  }
}
