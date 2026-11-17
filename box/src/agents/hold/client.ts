// A bridge's side of the holders (program.js, `kuvo-hold`): starting an agent's process under one,
// and attaching to one an earlier bridge started (a crash, or a newer bridge taking over), with a
// generation that fences off the older bridge. What it gives is the agent's pipes, line by line, as
// a process of the bridge's own would give them (`Held`), for RPC (Codex, ACP) and Claude's SDK.
//
// Step 8 of .claude/notes/update-handover.md: built and tested, not yet what any agent is started
// with (adopting a live turn in a new bridge is still to come). A box's holders are in
// `run/agents/hold` (0700): `<id>.sock`, `<id>.json` (its record, with the key), and the program,
// `kuvo-hold-<HOLDER>.js`, which every holder started since it was written runs.

import { spawn } from "node:child_process"
import { createHash, randomBytes } from "node:crypto"
import { EventEmitter } from "node:events"
import { chmodSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import net from "node:net"
import { userInfo } from "node:os"
import { join } from "node:path"
import { PassThrough, Writable } from "node:stream"
import program from "./program.js" with { type: "text" }

export const HOLD_PROTO = "hold/1"
/** Which holder program this bridge starts (a hash of it): a newer one is a new file beside it. */
export const HOLDER = createHash("sha256").update(program).digest("hex").slice(0, 16)

/** How a holder treats what the agent asked as a new bridge attaches (program.js). */
export type HoldKind = "jsonrpc" | "claude" | "raw"

export interface HoldRecord {
  proto: string
  holder: string | null
  id: string
  kind: HoldKind
  pid: number
  child: number | null
  program: string
  cwd: string | null
  started: number
  generation: number
  acked: number
  exited: { code: number | null; signal: string | null } | null
  meta: Record<string, unknown>
  key: string
}

/**
 * Where a box's holders go: `run/agents/hold` in its folder. A socket's path is at most 104 bytes
 * on macOS, so one whose folder is too deep (a test's) goes in Kuvo Code's short folder in `/tmp`
 * for sockets, as the SSH agents' do (link/home.ts `SSH_AGENTS`). Elsewhere a folder too deep is
 * none at all: its agents run as they always have, unheld.
 */
export function holdFolder(boxHome: string): string | undefined {
  const own = join(boxHome, "run", "agents", "hold")
  // (A slash, a 12-letter id and ".sock": 18 bytes.)
  if (Buffer.byteLength(own) + 18 < (process.platform === "darwin" ? 104 : 108)) return own
  if (process.platform !== "darwin") return undefined
  return join("/private/tmp", `kuvo-ssh-${userInfo().uid}`, `hold-${createHash("sha256").update(boxHome).digest("hex").slice(0, 8)}`)
}

/** The program, written once into the folder (whole, before it can be run). */
function programIn(folder: string): string {
  mkdirSync(folder, { recursive: true, mode: 0o700 })
  chmodSync(folder, 0o700)
  const path = join(folder, `kuvo-hold-${HOLDER}.js`)
  if (!existsSync(path)) {
    const temporary = `${path}.${randomBytes(4).toString("hex")}`
    writeFileSync(temporary, program, { mode: 0o600 })
    renameSync(temporary, path)
  }
  return path
}

/** The holders in a folder whose process still runs, as they recorded themselves. */
export function heldIn(folder: string): HoldRecord[] {
  let names: string[]
  try {
    names = readdirSync(folder)
  } catch {
    return []
  }
  const found: HoldRecord[] = []
  for (const name of names) {
    if (!/^[0-9a-f]{12}\.json$/.test(name)) continue
    try {
      const record = JSON.parse(readFileSync(join(folder, name), "utf8")) as HoldRecord
      if (record.proto !== HOLD_PROTO || !alive(record.pid)) continue
      found.push(record)
    } catch {}
  }
  return found
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error: any) {
    return error?.code === "EPERM"
  }
}

export interface HoldOptions {
  /** Where its socket and record go (`holdFolder`). */
  folder: string
  kind: HoldKind
  cwd?: string
  /** The agent's whole environment (none of the bridge's is added). */
  env: Record<string, string | undefined>
  /** What a bridge adopting it needs to know of it (its provider, session, thread…), in its record. */
  meta?: Record<string, unknown>
  /** The Bun to run the holder on (the bridge's own, by default). */
  runtime?: string
  /** A test's: the journal's size, and how long a holder whose agent ended waits for a bridge. */
  journalBytes?: number
  linger?: number
}

/** Starts `command` under a holder of its own, and attaches to it as its first bridge. */
export async function hold(command: string[], options: HoldOptions): Promise<Held> {
  const id = randomBytes(6).toString("hex")
  const key = randomBytes(24).toString("hex")
  const path = programIn(options.folder)
  const env: Record<string, string> = {}
  for (const [name, value] of Object.entries(options.env)) if (value !== undefined) env[name] = value
  // The engine's binary is Bun when told so (the holder takes it out of the agent's).
  env.BUN_BE_BUN = "1"
  env.KUVO_HOLD_KEY = key
  // (The agent's own BUN_BE_BUN, if it has one, apart from the holder's.)
  const spec = { command, cwd: options.cwd, meta: options.meta ?? {}, holder: HOLDER, bun: options.env.BUN_BE_BUN, journalBytes: options.journalBytes, linger: options.linger }
  // In a session of its own (no group signal reaches it), started in its own folder with no Bun
  // settings or .env of the project's read into it, and so into the agent's.
  const starting = spawn(options.runtime ?? process.execPath, ["--config=/dev/null", "--no-env-file", path, options.folder, id, options.kind, JSON.stringify(spec)], {
    cwd: options.folder,
    env,
    detached: true,
    stdio: ["ignore", "pipe", "ignore"],
  })
  const ready = await new Promise<{ pid: number }>((resolve, reject) => {
    let said = ""
    starting.stdout!.setEncoding("utf8")
    starting.stdout!.on("data", (chunk: string) => {
      said += chunk
      const line = said.split("\n")[0]
      if (!said.includes("\n")) return
      try {
        const ready = JSON.parse(line!)
        if (ready.ready === HOLD_PROTO) return resolve({ pid: ready.pid })
      } catch {}
      reject(new Error("The agent's holder didn't start."))
    })
    starting.once("error", reject)
    starting.once("exit", (code) => reject(new Error(`The agent's holder stopped as it started (${code}).`)))
  })
  starting.removeAllListeners("exit")
  starting.unref()
  return connect(options.folder, id, key, 1, ready.pid)
}

/** Attaches to a holder an earlier bridge started, one generation past the last that attached. */
export async function attach(folder: string, record: HoldRecord, from?: number): Promise<Held> {
  return connect(folder, record.id, record.key, record.generation + 1, record.pid, from)
}

async function connect(folder: string, id: string, key: string, generation: number, holder: number, from?: number): Promise<Held> {
  const socket = net.connect(join(folder, `${id}.sock`))
  const held = new Held(id, holder, socket)
  socket.write(JSON.stringify({ hello: HOLD_PROTO, key, generation, ...(from !== undefined ? { from } : {}) }) + "\n")
  await held.welcomed
  return held
}

type Exit = { code: number | null; signal: string | null }

/**
 * An agent's process under a holder, as this bridge drives it: lines it wrote (each once, in order,
 * and what it asked that an earlier bridge never answered once more), what it says on stderr, and
 * its end. Writes reach it only while this bridge is the newest attached.
 */
export class Held {
  /** The agent's process. */
  pid: number | undefined
  generation = 0
  /** The holder's hello: whether lines were lost before the oldest it kept, and what it was told to keep. */
  gap = false
  meta: Record<string, unknown> = {}
  readonly welcomed: Promise<void>
  readonly exited: Promise<Exit>
  /** Its socket closed: the holder ended (after its agent), or a newer bridge took over. */
  readonly closed: Promise<void>
  /** A newer bridge took this agent over: nothing this one writes reaches it any more. */
  fenced = false
  private seen = 0
  private lineListeners: ((line: string, again: boolean) => void)[] = []
  private errorListeners: ((text: string) => void)[] = []
  /** What came before anyone listened (the replay right after the hello), given to the first listener. */
  private early: { line: string; again: boolean }[] = []
  private earlyErrors: string[] = []
  private ended = false
  private ackTimer?: ReturnType<typeof setTimeout>
  private acked = 0

  constructor(
    readonly id: string,
    readonly holder: number,
    private readonly socket: net.Socket,
  ) {
    let welcome: () => void = () => {}
    let refuse: (error: Error) => void = () => {}
    this.welcomed = new Promise((resolve, reject) => ((welcome = resolve), (refuse = reject)))
    let exit: (exit: Exit) => void = () => {}
    this.exited = new Promise((resolve) => (exit = resolve))
    let close: () => void = () => {}
    this.closed = new Promise((resolve) => (close = resolve))
    let hello = false
    let buffer = ""
    socket.setEncoding("utf8")
    socket.on("error", () => {})
    socket.on("close", () => {
      this.ended = true
      clearTimeout(this.ackTimer)
      if (!hello) refuse(new Error("The agent's holder isn't there."))
      // Gone with no word of its agent's end (a newer bridge took over, or the holder was killed):
      // this bridge has no agent any more.
      exit({ code: null, signal: this.fenced ? "fenced" : "lost" })
      close()
    })
    socket.on("data", (chunk: string) => {
      buffer += chunk
      for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
        const line = buffer.slice(0, at)
        buffer = buffer.slice(at + 1)
        let frame: any
        try {
          frame = JSON.parse(line)
        } catch {
          continue
        }
        if (!hello) {
          hello = true
          if (frame.refused || frame.hello !== HOLD_PROTO) {
            refuse(new Error(frame.refused ?? "The agent's holder speaks another protocol."))
            socket.destroy()
            return
          }
          this.pid = frame.child ?? undefined
          this.generation = frame.generation
          this.gap = Boolean(frame.gap)
          this.meta = frame.meta ?? {}
          if (frame.stderr) this.earlyErrors.push(frame.stderr)
          welcome()
          continue
        }
        if (typeof frame.o === "string" && typeof frame.s === "number") {
          // (Each line once: what came again is only what it asked and wasn't answered.)
          if (frame.s <= this.seen && !frame.again) continue
          this.seen = Math.max(this.seen, frame.s)
          this.deliver(frame.o, frame.again === true)
          this.ackSoon()
        } else if (typeof frame.e === "string") {
          if (this.errorListeners.length) for (const each of this.errorListeners) each(frame.e)
          else this.earlyErrors.push(frame.e)
        } else if (frame.x) {
          exit({ code: frame.x.code ?? null, signal: frame.x.signal ?? null })
          this.send({ ack: this.seen })
          this.send({ bye: true })
        } else if (frame.fenced) {
          this.fenced = true
        }
      }
    })
  }

  private deliver(line: string, again: boolean) {
    if (!this.lineListeners.length) return void this.early.push({ line, again })
    for (const each of this.lineListeners) each(line, again)
  }

  /** Each line the agent wrote, from the first this bridge hasn't seen. */
  onLine(listener: (line: string, again: boolean) => void) {
    this.lineListeners.push(listener)
    const early = this.early
    this.early = []
    for (const { line, again } of early) listener(line, again)
  }

  onStderr(listener: (text: string) => void) {
    this.errorListeners.push(listener)
    const early = this.earlyErrors
    this.earlyErrors = []
    for (const text of early) listener(text)
  }

  /** What this bridge has taken in, for the next one to start after: every 200 ms at most. */
  private ackSoon() {
    this.ackTimer ??= setTimeout(() => {
      this.ackTimer = undefined
      if (this.seen > this.acked) {
        this.acked = this.seen
        this.send({ ack: this.seen })
      }
    }, 200)
  }

  private send(frame: Record<string, unknown>) {
    if (this.ended || this.socket.destroyed) return false
    this.socket.write(JSON.stringify(frame) + "\n")
    return true
  }

  get isOpen() {
    return !this.ended && !this.fenced
  }

  /** A line to the agent's stdin (a newline is added). */
  write(line: string) {
    if (this.fenced) return false
    return this.send({ i: line })
  }

  /** Its stdin closed, as a process the bridge started has it closed. */
  end() {
    if (!this.fenced) this.send({ end: true })
  }

  kill(signal: NodeJS.Signals = "SIGTERM") {
    if (!this.fenced) this.send({ signal })
  }

  /** More for its record: what a bridge adopting it reads (a thread's id once it's known, say). */
  note(meta: Record<string, unknown>) {
    this.meta = { ...this.meta, ...meta }
    if (!this.fenced) this.send({ meta })
  }

  /** Let go of it without stopping it (another bridge will attach). */
  detach() {
    clearTimeout(this.ackTimer)
    if (this.seen > this.acked) this.send({ ack: this.seen })
    this.socket.end()
  }
}

/** A holder's files, once its process is gone (it removes them itself as it ends: these are a crash's). */
export function forgetHolder(folder: string, record: HoldRecord) {
  if (alive(record.pid)) return
  rmSync(join(folder, `${record.id}.sock`), { force: true })
  rmSync(join(folder, `${record.id}.json`), { force: true })
}

/**
 * A held agent as the process Claude's SDK drives (its `spawnClaudeCodeProcess`, which must answer
 * at once: what the SDK writes before the holder answers waits for it). A new SDK `query()` over a
 * holder another bridge started is a client joining Claude Code's session: it initializes again,
 * and Claude Code sends the asks still open in its answer (spike S1).
 */
export function heldProcess(holding: Held | Promise<Held>) {
  const events = new EventEmitter()
  const stdout = new PassThrough()
  let held: Held | undefined
  const waiting: string[] = []
  let ending = false
  let buffer = ""
  const stdin = new Writable({
    write(chunk, _encoding, done) {
      buffer += String(chunk)
      for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
        const line = buffer.slice(0, at)
        buffer = buffer.slice(at + 1)
        if (!line.trim()) continue
        if (held) held.write(line)
        else waiting.push(line)
      }
      done()
    },
    final(done) {
      if (held) held.end()
      else ending = true
      done()
    },
  })
  const process_ = Object.assign(events, {
    stdin,
    stdout,
    killed: false,
    exitCode: null as number | null,
    signalCode: null as NodeJS.Signals | null,
    kill(signal: NodeJS.Signals = "SIGTERM") {
      process_.killed = true
      if (held) held.kill(signal)
      else void Promise.resolve(holding).then((each) => each.kill(signal), () => {})
      return true
    },
  })
  Promise.resolve(holding).then(
    (each) => {
      held = each
      for (const line of waiting.splice(0)) each.write(line)
      if (ending) each.end()
      each.onLine((line) => stdout.write(line + "\n"))
      void each.exited.then((exit) => {
        stdout.end()
        process_.exitCode = exit.code
        process_.signalCode = exit.signal && exit.signal !== "fenced" && exit.signal !== "lost" ? (exit.signal as NodeJS.Signals) : null
        events.emit("exit", exit.code, process_.signalCode)
      })
    },
    (error) => {
      stdout.end()
      events.emit("error", error instanceof Error ? error : new Error(String(error)))
      process_.exitCode = 1
      events.emit("exit", 1, null)
    },
  )
  return process_
}
