// JSON-RPC over a child's stdin and stdout, one message per line: Codex's app-server speaks it
// without the `jsonrpc` field, ACP agents (Grok, Antigravity) with it. Both send requests of their
// own (an approval, a question), answered by `onRequest`.

import type { Subprocess } from "bun"
import { TookTooLong } from "./agent"
import type { Held } from "./hold/client"

export class RPCClosed extends Error {}

export class RPCError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message)
  }
}

/** Answers a request of the child's; `id` names it, should the child settle it itself later. */
type Handler = (method: string, params: any, id: number | string) => Promise<unknown>

export class RPC {
  private next = 1
  private pending = new Map<number | string, { resolve: (value: any) => void; reject: (error: Error) => void }>()
  /** The agent's process: one of the bridge's own, or one under a holder (hold/client.ts). */
  private child?: Subprocess<"pipe", "pipe", "pipe">
  private held?: Held
  private closed = false
  agentExited = false
  /** What the child wrote to stderr lately, for errors. */
  stderr = ""
  onNotification: (method: string, params: any) => void = () => {}
  onRequest: Handler = async (method) => {
    throw new RPCError(-32601, `Method not found: ${method}`)
  }
  onExit: (code: number, signal: string | null) => void = () => {}
  /** An answer to a request this RPC didn't send: one an earlier bridge sent the agent under a holder. */
  onStray: (id: number | string, result: unknown, error?: { code?: number; message?: string }) => void = () => {}
  readonly exited: Promise<number>

  /**
   * `command` started as the bridge's own child, or an agent's process under a holder (`Held`):
   * the same lines either way. Once the bridge gives up a holder, the new controller owns its
   * results, including the outcome of the turn for any requests still pending here.
   */
  constructor(
    command: string[] | Held,
    options: { cwd?: string; env?: Record<string, string | undefined>; cleanEnv?: boolean; jsonrpc?: boolean; firstID?: number; filter?: (line: string) => boolean } = {},
  ) {
    this.jsonrpc = options.jsonrpc ?? false
    if (options.firstID) this.next = options.firstID
    this.filter = options.filter
    const ended = (code: number, signal: string | null) => {
      this.closed = true
      // A handover can wait for other holders after this socket closes. Failing the old
      // requests or calling onExit during that wait would write a false end to the saved turn.
      if (this.held?.relinquished) {
        this.pending.clear()
        return code
      }
      this.agentExited = true
      for (const { reject } of this.pending.values()) reject(new RPCClosed(this.stderr.trim().split("\n").slice(-3).join("\n") || `The agent stopped (exit code ${code}).`))
      this.pending.clear()
      this.onExit(code, signal)
      return code
    }
    if (!Array.isArray(command)) {
      const held = (this.held = command)
      held.onStderr((text) => (this.stderr = (this.stderr + text).slice(-8_000)))
      // Install the reader before announcing readiness. Replay waits for the adapter's handlers
      // and, during live transfer, for the other adapters.
      const read = held.prepareReader((line, _again, seq) => {
        this.lineSeq = seq
        const trimmed = line.replace(/\r$/, "")
        if (trimmed.trim() && (!this.filter || this.filter(trimmed))) this.receive(trimmed)
      })
      const started = Promise.withResolvers<void>()
      this.begin = () => { read(); started.resolve() }
      // A stopped holder can have a turn result waiting in the replay buffer.
      const exit = started.promise.then(() => held.exited)
      // (Taken over by a newer bridge: the agent didn't stop, but it's that bridge's now.)
      this.exited = exit.then((exit) => ended(exit.code ?? (exit.signal === "fenced" || exit.signal === "lost" || exit.signal === "detached" ? -1 : 1), exit.signal))
      this.errorsRead = this.exited.then(() => {})
      return
    }
    // `cleanEnv`: only what's given (an agent that mustn't see your other keys).
    const env = options.cleanEnv ? options.env : { ...process.env, ...options.env }
    const child = (this.child = Bun.spawn(command, { cwd: options.cwd, env: env as Record<string, string>, stdin: "pipe", stdout: "pipe", stderr: "pipe" }))
    const output = this.read(child).catch(() => {})
    this.errorsRead = this.readErrors(child).catch(() => {})
    this.exited = child.exited.then(async code => {
      // Give the last protocol lines a chance to end the turn. A descendant can keep the
      // pipes open after the agent exits, so waiting for EOF has a bound.
      let timer: ReturnType<typeof setTimeout> | undefined
      await Promise.race([Promise.all([output, this.errorsRead]), new Promise<void>(resolve => { timer = setTimeout(resolve, 500) })])
      clearTimeout(timer)
      return ended(code, child.signalCode)
    })
  }

  private begin?: () => void
  /** Over a holder: the number of the agent's line being handled now (for what began before a turn's own lines, to set aside). */
  lineSeq = 0

  /** Over a holder: its lines start coming, to the handlers set by now (a process of its own starts at once). */
  start() {
    const begin = this.begin
    this.begin = undefined
    begin?.()
  }

  /** Once all it wrote to stderr is in `stderr` (after it's gone). */
  readonly errorsRead: Promise<void>

  private readonly jsonrpc: boolean
  private readonly filter?: (line: string) => boolean

  get pid() {
    return this.child ? this.child.pid : this.held!.pid
  }

  /** The holder it runs under, if it does. */
  get holder(): Held | undefined {
    return this.held
  }

  /** The id the next request will have. */
  get nextID() {
    return this.next
  }

  get isClosed() {
    return this.closed
  }

  private async read(child: Subprocess<"pipe", "pipe", "pipe">) {
    const decoder = new TextDecoder()
    let buffer = ""
    for await (const chunk of child.stdout) {
      buffer += decoder.decode(chunk, { stream: true })
      let newline: number
      while ((newline = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, newline).replace(/\r$/, "")
        buffer = buffer.slice(newline + 1)
        if (line.trim() && (!this.filter || this.filter(line))) this.receive(line)
      }
    }
  }

  private async readErrors(child: Subprocess<"pipe", "pipe", "pipe">) {
    const decoder = new TextDecoder()
    for await (const chunk of child.stderr) {
      this.stderr = (this.stderr + decoder.decode(chunk, { stream: true })).slice(-8_000)
    }
  }

  private receive(line: string) {
    if (this.closed || this.held?.relinquished) return
    let message: any
    try {
      message = JSON.parse(line)
    } catch {
      return
    }
    if (message.method !== undefined && message.id !== undefined) {
      void this.answer(message.id, message.method, message.params)
    } else if (message.method !== undefined) {
      this.onNotification(message.method, message.params)
    } else if (message.id !== undefined) {
      const pending = this.pending.get(message.id)
      if (!pending) return this.onStray(message.id, message.result, message.error)
      this.pending.delete(message.id)
      if (message.error) pending.reject(new RPCError(message.error.code ?? -32000, message.error.message ?? "The agent refused.", message.error.data))
      else pending.resolve(message.result)
    }
  }

  private async answer(id: number | string, method: string, params: unknown) {
    try {
      const result = await this.onRequest(method, params, id)
      this.write({ id, result: result ?? null })
    } catch (error) {
      const code = error instanceof RPCError ? error.code : -32603
      this.write({ id, error: { code, message: error instanceof Error ? error.message : String(error) } })
    }
  }

  private write(message: Record<string, unknown>) {
    if (this.closed) return
    const body = this.jsonrpc ? { jsonrpc: "2.0", ...message } : message
    if (this.held) return void this.held.write(JSON.stringify(body))
    try {
      this.child!.stdin.write(JSON.stringify(body) + "\n")
      this.child!.stdin.flush()
    } catch {}
  }

  request<T = any>(method: string, params?: unknown, timeout?: number): Promise<T> {
    if (this.closed) return Promise.reject(new RPCClosed(this.stderr.trim().split("\n").slice(-3).join("\n") || "The agent isn't running."))
    const id = this.next++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.write(params === undefined ? { id, method } : { id, method, params })
      if (timeout) {
        setTimeout(() => {
          if (!this.pending.has(id)) return
          this.pending.delete(id)
          reject(new TookTooLong(`${method} took too long.`))
        }, timeout)
      }
    })
  }

  notify(method: string, params?: unknown) {
    this.write(params === undefined ? { method } : { method, params })
  }

  kill() {
    if (this.closed) return
    if (this.held) return this.held.stop()
    try {
      this.child!.stdin.end()
    } catch {}
    this.child!.kill()
  }
}
