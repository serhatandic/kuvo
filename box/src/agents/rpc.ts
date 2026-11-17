// JSON-RPC over a child's stdin and stdout, one message per line: Codex's app-server speaks it
// without the `jsonrpc` field, ACP agents (Grok, Antigravity) with it. Both send requests of their
// own (an approval, a question), answered by `onRequest`.

import type { Subprocess } from "bun"
import { TookTooLong } from "./agent"
import type { Held } from "./hold/client"

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
  /** What the child wrote to stderr lately, for errors. */
  stderr = ""
  onNotification: (method: string, params: any) => void = () => {}
  onRequest: Handler = async (method) => {
    throw new RPCError(-32601, `Method not found: ${method}`)
  }
  onExit: (code: number) => void = () => {}
  readonly exited: Promise<number>

  /**
   * `command` started as the bridge's own child, or an agent's process under a holder (`Held`):
   * the same lines either way. A held one that a newer bridge took over is gone for this one
   * (its requests fail), though the agent goes on.
   */
  constructor(
    command: string[] | Held,
    options: { cwd?: string; env?: Record<string, string | undefined>; cleanEnv?: boolean; jsonrpc?: boolean; firstID?: number; filter?: (line: string) => boolean } = {},
  ) {
    this.jsonrpc = options.jsonrpc ?? false
    if (options.firstID) this.next = options.firstID
    this.filter = options.filter
    const ended = (code: number) => {
      this.closed = true
      for (const { reject } of this.pending.values()) reject(new Error(this.stderr.trim().split("\n").slice(-3).join("\n") || `The agent stopped (exit code ${code}).`))
      this.pending.clear()
      this.onExit(code)
      return code
    }
    if (!Array.isArray(command)) {
      const held = (this.held = command)
      held.onStderr((text) => (this.stderr = (this.stderr + text).slice(-8_000)))
      // (From `start()`: what the holder sends again as this bridge attaches waits for its
      // handlers, or an ask would be answered "not found".)
      this.begin = () =>
        held.onLine((line) => {
          const trimmed = line.replace(/\r$/, "")
          if (trimmed.trim() && (!this.filter || this.filter(trimmed))) this.receive(trimmed)
        })
      // (Taken over by a newer bridge: the agent didn't stop, but it's that bridge's now.)
      this.exited = held.exited.then((exit) => ended(exit.code ?? (exit.signal === "fenced" || exit.signal === "lost" ? -1 : 1)))
      this.errorsRead = this.exited.then(() => {})
      return
    }
    // `cleanEnv`: only what's given (an agent that mustn't see your other keys).
    const env = options.cleanEnv ? options.env : { ...process.env, ...options.env }
    const child = (this.child = Bun.spawn(command, { cwd: options.cwd, env: env as Record<string, string>, stdin: "pipe", stdout: "pipe", stderr: "pipe" }))
    this.exited = child.exited.then(ended)
    void this.read(child)
    this.errorsRead = this.readErrors(child).catch(() => {})
  }

  private begin?: () => void

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
      if (!pending) return
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
    if (this.closed) return Promise.reject(new Error(this.stderr.trim().split("\n").slice(-3).join("\n") || "The agent isn't running."))
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
    if (this.held) {
      this.held.end()
      this.held.kill()
      return
    }
    try {
      this.child!.stdin.end()
    } catch {}
    this.child!.kill()
  }
}
