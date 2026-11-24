// A bridge's side of the holders (program.js, `kuvo-hold`): starting an agent's process under one,
// and attaching to one an earlier bridge started (a crash, or a newer bridge taking over), with a
// generation that fences off the older bridge. What it gives is the agent's pipes, line by line, as
// a process of the bridge's own would give them (`Held`), for RPC (Codex, ACP) and Claude's SDK.
//
// Built and tested, not yet what any agent is started
// with (adopting a live turn in a new bridge is still to come). A holder no bridge attaches to
// stops its agent after a while (program.js), so none is left running for good. A box's holders are in
// `run/agents/hold` (0700): `<id>.sock`, `<id>.json` (its record, with the key), and the program,
// `kuvo-hold-<HOLDER>.js`, which every holder started since it was written runs.

import { spawn, type ChildProcess } from "node:child_process"
import { createHash, randomBytes } from "node:crypto"
import { EventEmitter } from "node:events"
import { chmodSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import type net from "node:net"
import { join } from "node:path"
import { PassThrough, Writable } from "node:stream"
import { ipcSource } from "./ipc-source" with { type: "macro" }
import { localIpc } from "../../shared/local-ipc.js"
import holderSource from "./program.js" with { type: "text" }
import { runsAs } from "../../shared/platform"

const program = ipcSource() + "\n" + holderSource

export const HOLD_PROTO = "hold/1"
/** How long a holder has to start, or to answer a bridge attaching. */
const WITHIN = 10_000
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
  /** The bridge that started it. */
  bridge: number | null
  generation: number
  acked: number
  exited: { code: number | null; signal: string | null } | null
  meta: Record<string, unknown>
  key: string
}

/**
 * Where a box's holders go: `run/agents/hold` in its folder, which `kuvo-start stop` looks in to
 * stop them with the box. One too deep for a socket's path (104 bytes on macOS) is none: its agents
 * run as they always have, unheld.
 */
export function holdFolder(boxHome: string): string | undefined {
  if (process.platform === "win32") return undefined
  const own = join(boxHome, "run", "agents", "hold")
  // (A slash, a 12-letter id and ".sock": 18 bytes.)
  return Buffer.byteLength(own) + 18 < (process.platform === "darwin" ? 104 : 108) ? own : undefined
}

/** A folder that's this user's alone: a folder (not a link), theirs, open to no one else. */
function ownFolder(path: string): boolean {
  try {
    const seen = lstatSync(path)
    return seen.isDirectory() && seen.uid === process.getuid?.() && (seen.mode & 0o077) === 0
  } catch {
    return false
  }
}

/** The program in the folder, as this bridge has it (written whole, before it can be run, and
 * written again should what's there differ), in a folder that's this user's alone. */
function programIn(folder: string): string {
  mkdirSync(folder, { recursive: true, mode: 0o700 })
  chmodSync(folder, 0o700)
  if (!ownFolder(folder)) throw new Error(`${folder} isn't this user's alone: no agent is held there.`)
  const path = join(folder, `kuvo-hold-${HOLDER}.js`)
  let there: string | undefined
  try {
    there = readFileSync(path, "utf8")
  } catch {}
  if (there !== program) {
    const temporary = `${path}.${randomBytes(4).toString("hex")}`
    writeFileSync(temporary, program, { mode: 0o600 })
    renameSync(temporary, path)
  }
  return path
}

/**
 * The holders in a folder that still run, as they recorded themselves: each only when its process
 * is that very holder (its command line names its program and its id) and its socket is there. A
 * record that isn't (its holder crashed, or the computer restarted and another process has its
 * number since) is a leftover: removed, never signalled.
 */
export function heldIn(folder: string): HoldRecord[] {
  // Until the Windows process adapter is installed, an unverified record must stay on disk.
  if (process.platform === "win32") return []
  let names: string[]
  try {
    names = readdirSync(folder)
  } catch {
    return []
  }
  const found: HoldRecord[] = []
  for (const name of names) {
    const id = /^([0-9a-f]{12})\.json$/.exec(name)?.[1]
    if (!id) continue
    let record: HoldRecord | undefined
    try {
      record = JSON.parse(readFileSync(join(folder, name), "utf8")) as HoldRecord
    } catch {}
    if (record?.proto === HOLD_PROTO && record.id === id && isHolder(record.pid, id) && existsSync(join(folder, `${id}.sock`))) found.push(record)
    else if (!record || !isHolder(record.pid, id)) {
      for (const leftover of [`${id}.sock`, `${id}.journal`, name]) rmSync(join(folder, leftover), { force: true })
    }
  }
  return found
}

/** Whether `pid` is the holder of `id`. */
export function isHolder(pid: unknown, id: string): boolean {
  return /^[0-9a-f]{12}$/.test(id) && runsAs(pid, new RegExp(`kuvo-hold-[0-9a-f]+\\.js .* ${id} (jsonrpc|claude|raw)\\s*$`))
}

/** Holder programs no holder runs now (each loaded its own as it started): all but this bridge's. */
export function forgetOldPrograms(folder: string) {
  try {
    for (const name of readdirSync(folder)) if (/^kuvo-hold-[0-9a-f]+\.js(\.[0-9a-f]+)?$/.test(name) && name !== `kuvo-hold-${HOLDER}.js`) rmSync(join(folder, name), { force: true })
  } catch {}
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
  /** A test's: the journal's size, how long a holder whose agent ended waits for a bridge, and how long its agent runs with none. */
  journalBytes?: number
  linger?: number
  orphan?: number
}

/** Starts `command` under a holder of its own, and attaches to it as its first bridge. */
export async function hold(command: string[], options: HoldOptions): Promise<Held> {
  if (process.platform === "win32") throw new Error("Kuvo's secure Windows named pipes are unavailable.")
  const id = randomBytes(6).toString("hex")
  const key = randomBytes(24).toString("hex")
  const path = programIn(options.folder)
  const env: Record<string, string> = {}
  for (const [name, value] of Object.entries(options.env)) if (value !== undefined) env[name] = value
  // The engine's binary is Bun when told so (the holder takes it out of the agent's, but for the
  // agent's own, `bun`).
  env.BUN_BE_BUN = "1"
  const spec = { command, cwd: options.cwd, meta: options.meta ?? {}, holder: HOLDER, bridge: process.pid, bun: options.env.BUN_BE_BUN, journalBytes: options.journalBytes, linger: options.linger, orphan: options.orphan }
  // In a session of its own (no group signal reaches it), started in its own folder with no Bun
  // settings or .env of the project's read into it, and so into the agent's. Its key and what it
  // runs come on its stdin, never on a command line or in an environment.
  const starting = spawn(options.runtime ?? process.execPath, ["--config=/dev/null", "--no-env-file", path, options.folder, id, options.kind], {
    cwd: options.folder,
    env,
    detached: true,
    stdio: ["pipe", "pipe", "ignore"],
  })
  starting.stdin!.on("error", () => {})
  starting.stdin!.end(JSON.stringify({ key, spec }))
  const ready = new Promise<{ pid: number }>((resolve, reject) => {
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
    // (One that never says it's ready is given up on, and stopped below.)
    setTimeout(() => reject(new Error("The agent's holder didn't start in time.")), WITHIN).unref?.()
  })
  if (starting.pid) startingHolders.add(starting.pid)
  try {
    const { pid } = await ready
    starting.removeAllListeners("exit")
    starting.unref()
    return await connect(options.folder, id, key, 1, pid)
  } catch (error) {
    console.error("Holder startup failed", { id, holder: starting.pid, kind: options.kind, code: (error as NodeJS.ErrnoException | null)?.code, reason: error instanceof Error ? error.message : undefined })
    // (Its own holder, by the process it started: with the agent it may have started, it goes.)
    try {
      if (starting.pid) process.kill(starting.pid, "SIGTERM")
    } catch {}
    starting.unref()
    throw error
  } finally {
    if (starting.pid) startingHolders.delete(starting.pid)
  }
}

/** Holders this bridge started that aren't attached yet (by their own pid). */
const startingHolders = new Set<number>()

/** Attaches to a holder an earlier bridge started, one generation past the last that attached. */
export async function attach(folder: string, record: HoldRecord, from?: number, complete: boolean | "provisional" = false): Promise<Held> {
  return connect(folder, record.id, record.key, record.generation + 1, record.pid, from, complete)
}

async function connect(folder: string, id: string, key: string, generation: number, holder: number, from?: number, complete: boolean | "provisional" = false): Promise<Held> {
  const opening = localIpc(join(folder, `${id}.sock`)).connect()
  const socket = opening instanceof Promise ? await opening : opening
  const held = new Held(id, holder, socket, complete === "provisional")
  socket.write(JSON.stringify({ hello: HOLD_PROTO, key, generation, complete: Boolean(complete), ...(from !== undefined ? { from } : {}) }) + "\n")
  // (A holder that doesn't answer, wedged, is given up on.)
  const late = setTimeout(() => {
    console.error("Holder welcome timed out", { id, holder, generation })
    socket.destroy()
  }, WITHIN)
  try {
    await held.welcomed
  } finally {
    clearTimeout(late)
  }
  live.add(held)
  void held.closed.then(() => live.delete(held))
  return held
}

/** The holders this bridge drives now. */
const live = new Set<Held>()

/** The attached holders' current metadata is ahead of the copies on disk. */
export function controlledHolders(): readonly Held[] | undefined {
  return startingHolders.size ? undefined : [...live]
}

/**
 * A bridge making way for the next lets go of every agent it holds, idle or at work, and of those
 * still starting: none is stopped, written to or signalled from here on (whatever this bridge does
 * as it ends), and the next bridge takes each up (adopt.ts). Done once each has heard (2 s at most).
 */
export function releaseHolders(): Promise<void> {
  const letGo = [...live]
  for (const held of letGo) held.detach()
  startingHolders.clear()
  // Once each holder has all that was sent it (a turn's note, the last ack) and has let go too: a
  // bridge ending at once would lose what's still on its way.
  return Promise.race([Promise.all(letGo.map((held) => held.closed)).then(() => {}), new Promise<void>((resolve) => setTimeout(resolve, WITHIN / 5).unref?.())])
}

/** The sessions whose agents this bridge holds: theirs are the next bridge's to take up. */
export function heldSessions(): Set<string> {
  const found = new Set<string>()
  for (const held of live) {
    if (!held.isOpen) continue
    if (typeof held.meta.session === "string") found.add(held.meta.session)
    for (const note of Object.values((held.meta.turns ?? {}) as Record<string, { session?: unknown }>)) if (typeof note?.session === "string") found.add(note.session)
  }
  return found
}

/**
 * Every agent this bridge drives under a holder, stopped as it ends on purpose (Stop, making way
 * for a newer build): through each holder's own process, at once, whatever it does next.
 */
export function stopAllHeld() {
  for (const held of live) held.stop("bridge shutdown")
  // (And any still starting, which no bridge would come for.)
  for (const pid of startingHolders) {
    try {
      process.kill(pid, "SIGTERM")
    } catch {}
  }
}

/** How it ended; `heardAll`: all it said on stderr came (none held it open past the holder's wait). */
type Exit = { code: number | null; signal: string | null; heardAll?: boolean }

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
  /** The first of the agent's lines this bridge takes in. */
  readonly firstLine: Promise<void>
  private firstCame: () => void = () => {}
  readonly exited: Promise<Exit>
  /** Its socket closed: the holder ended (after its agent), or a newer bridge took over. */
  readonly closed: Promise<void>
  /** A newer bridge took this agent over: nothing this one writes reaches it any more. */
  fenced = false
  /** Its holder said the agent ended. */
  private heardEnd = false
  /** Let go of on purpose (`detach`): another bridge is to attach. */
  private letGo = false
  /** The last line of the agent's this bridge has taken in: where a turn it starts now begins (`from`). */
  seen = 0
  private lineListeners: { active: boolean; receive: (line: string, again: boolean, seq: number) => void }[] = []
  private errorListeners: ((text: string) => void)[] = []
  /** What came before anyone listened (the replay right after the hello), given to the first listener. */
  private early: { line: string; again: boolean; seq: number }[] = []
  private earlyErrors: string[] = []
  private ended = false
  private ackTimer?: ReturnType<typeof setTimeout>
  private acked = 0
  private preparing?: (ready: boolean) => void

  constructor(
    readonly id: string,
    readonly holder: number,
    private readonly socket: net.Socket,
    private provisional = false,
  ) {
    let welcome: () => void = () => {}
    let refuse: (error: Error) => void = () => {}
    this.welcomed = new Promise((resolve, reject) => ((welcome = resolve), (refuse = reject)))
    this.firstLine = new Promise((resolve) => (this.firstCame = resolve))
    let exit: (exit: Exit) => void = () => {}
    this.exited = new Promise((resolve) => (exit = resolve))
    let close: () => void = () => {}
    this.closed = new Promise((resolve) => (close = resolve))
    let hello = false
    /** Welcomed by its holder (not refused). */
    let attached = false
    let buffer = ""
    socket.setEncoding("utf8")
    let socketError: string | undefined
    socket.on("error", (error: NodeJS.ErrnoException) => { socketError = error.code })
    socket.on("close", () => {
      this.ended = true
      this.preparing?.(false)
      clearTimeout(this.ackTimer)
      if (!hello) refuse(new Error("The agent's holder isn't there."))
      // Gone with no word of its agent's end: a newer bridge took over (it's that one's now), or this
      // one lost its holder (let go as hung, say). Then its agent would run on with no one watching:
      // it's stopped, through its holder, once that's still the one.
      if (!this.heardEnd && !this.letGo) {
        console.error("Holder socket closed", { id, holder, child: this.pid, generation: this.generation, attached, fenced: this.fenced, socketError })
      }
      if (attached && !this.fenced && !this.heardEnd && !this.letGo) this.stopHolder("socket lost")
      exit({ code: null, signal: this.fenced ? "fenced" : this.letGo ? "detached" : "lost" })
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
          attached = true
          welcome()
          continue
        }
        if (typeof frame.o === "string" && typeof frame.s === "number") {
          // (Each line once: what came again is only what it asked and wasn't answered.)
          if (frame.s <= this.seen && !frame.again) continue
          this.seen = Math.max(this.seen, frame.s)
          this.deliver(frame.o, frame.again === true, frame.s)
          this.ackSoon()
        } else if (typeof frame.e === "string") {
          if (this.errorListeners.length) for (const each of this.errorListeners) each(frame.e)
          else this.earlyErrors.push(frame.e)
        } else if (frame.x) {
          this.heardEnd = true
          exit({ code: frame.x.code ?? null, signal: frame.x.signal ?? null, heardAll: frame.x.heardAll === true })
          this.send({ ack: this.seen })
          if (!this.provisional) this.send({ bye: true })
        } else if (typeof frame.prepared === "boolean") {
          this.preparing?.(frame.prepared)
        } else if (frame.fenced) {
          this.fenced = true
        }
      }
    })
  }

  private deliver(line: string, again: boolean, seq: number) {
    if (!this.lineListeners.some(reader => reader.active)) return void this.early.push({ line, again, seq })
    this.firstCame()
    for (const reader of this.lineListeners) if (reader.active) reader.receive(line, again, seq)
  }

  /** Install a reader without dispatching journal lines until the adapter enables it. */
  prepareReader(receive: (line: string, again: boolean, seq: number) => void): () => void {
    const reader = { active: false, receive }
    this.lineListeners.push(reader)
    return () => {
      reader.active = true
      const early = this.early
      this.early = []
      if (early.length) this.firstCame()
      for (const { line, again, seq } of early) receive(line, again, seq)
    }
  }

  /** Each line the agent wrote, from the first this bridge hasn't seen, with its number. */
  onLine(listener: (line: string, again: boolean, seq: number) => void) {
    this.prepareReader(listener)()
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
    if (this.ended || this.letGo || this.socket.destroyed) return false
    this.socket.write(JSON.stringify(frame) + "\n")
    return true
  }

  own() {
    this.provisional = false
    if (this.heardEnd) this.send({ bye: true })
  }

  get isOpen() {
    return !this.ended && !this.fenced
  }

  get relinquished() {
    return this.fenced || this.letGo
  }

  /** A line to the agent's stdin (a newline is added). */
  write(line: string) {
    if (this.fenced) return false
    return this.send({ i: line })
  }

  /** Its stdin closed, as a process the bridge started has it closed. */
  end() {
    if (!this.fenced && !this.provisional) this.send({ end: true })
  }

  kill(signal: NodeJS.Signals = "SIGTERM") {
    if (!this.fenced && !this.provisional) this.send({ signal })
  }

  /** Stops the agent as a bridge stops one, through its holder's own process (no frame that a bridge
   * exiting at once could leave unsent): its stdin closed, then told, then made to; the holder goes
   * once it has. Not after a newer bridge took it over. */
  stop(reason = "requested stop") {
    if (this.fenced || this.letGo || this.heardEnd || this.provisional) return
    // (Its socket closed: only once that's still the holder, never a process with its number since.)
    if (this.ended) return this.stopHolder(reason)
    console.error("Stopping holder", { id: this.id, holder: this.holder, child: this.pid, reason })
    try {
      process.kill(this.holder, "SIGTERM")
    } catch {}
  }

  private stopHolder(reason: string) {
    if (this.provisional || !isHolder(this.holder, this.id)) return
    console.error("Stopping disconnected holder", { id: this.id, holder: this.holder, child: this.pid, reason })
    try {
      process.kill(this.holder, "SIGTERM")
    } catch {}
  }

  /** More for its record: what a bridge adopting it reads (a thread's id once it's known, say). */
  note(meta: Record<string, unknown>) {
    this.meta = { ...this.meta, ...meta }
    if (!this.fenced && !this.provisional) this.send({ meta })
  }

  prepare(from: number): Promise<boolean> {
    if (this.ended || this.fenced || this.heardEnd || this.preparing) return Promise.resolve(false)
    return new Promise((done) => {
      const timer = setTimeout(() => finish(false), 2_000)
      const finish = (ready: boolean) => {
        clearTimeout(timer)
        this.preparing = undefined
        done(ready)
      }
      this.preparing = finish
      this.send({ prepare: from })
    })
  }

  unpin() {
    this.send({ unpin: true })
  }

  /** Let go of it without stopping it (another bridge will attach). */
  detach() {
    if (this.letGo) return
    clearTimeout(this.ackTimer)
    if (this.seen > this.acked) this.send({ ack: this.seen })
    // The Claude SDK can call its subprocess cleanup on process exit. This controller has
    // given up the agent, including its ability to stop it.
    this.letGo = true
    this.socket.end()
  }
}

/** A holder's files, once its process is gone (it removes them itself as it ends: these are a crash's). */
export function forgetHolder(folder: string, record: HoldRecord) {
  if (isHolder(record.pid, record.id)) return
  for (const leftover of [`${record.id}.sock`, `${record.id}.journal`, `${record.id}.json`]) rmSync(join(folder, leftover), { force: true })
}

/**
 * A held agent as the process Claude's SDK drives (its `spawnClaudeCodeProcess`, which must answer
 * at once: what the SDK writes before the holder answers waits for it). A new SDK `query()` over a
 * holder another bridge started is a client joining Claude Code's session: it initializes again,
 * and Claude Code sends the asks still open in its answer (spike S1).
 */
export function heldProcess(holding: Held | Promise<Held>, fallback?: () => ChildProcess) {
  const events = new EventEmitter()
  const stdout = new PassThrough()
  stdout.on("error", () => {})
  let held: Held | undefined
  let ownChild: ChildProcess | undefined
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
        if (ownChild) ownChild.stdin?.write(line + "\n")
        else if (held) held.write(line)
        else waiting.push(line)
      }
      done()
    },
    final(done) {
      if (ownChild) ownChild.stdin?.end()
      else if (held) held.end()
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
      // (Stopped through its holder's own process: it goes, whatever this bridge does next.)
      if (ownChild) ownChild.kill(signal)
      else if (held) held.stop()
      else void Promise.resolve(holding).then((each) => each.stop(), () => {})
      return true
    },
  })
  Promise.resolve(holding).then(
    (each) => {
      held = each
      for (const line of waiting.splice(0)) each.write(line)
      if (ending) each.end()
      each.onLine((line) => {
        if (!stdout.writableEnded) stdout.write(line + "\n")
      })
      void each.exited.then((exit) => {
        stdout.end()
        process_.exitCode = exit.code
        process_.signalCode = exit.signal && exit.signal !== "fenced" && exit.signal !== "lost" && exit.signal !== "detached" ? (exit.signal as NodeJS.Signals) : null
        events.emit("exit", exit.code, process_.signalCode)
        // (As a child process says once its output is in: all of it came before its end.)
        events.emit("close", exit.code, process_.signalCode)
      })
    },
    (error) => {
      // (No holder: the agent as a process of the bridge's own, given what was written meanwhile;
      // none once it was stopped meanwhile.)
      const own = process_.killed ? undefined : fallback?.()
      if (own?.stdin && own.stdout) {
        for (const line of waiting.splice(0)) own.stdin.write(line + "\n")
        if (ending) own.stdin.end()
        ownChild = own
        own.stdout.on("data", (chunk) => {
          if (!stdout.writableEnded) stdout.write(chunk)
        })
        own.stdout.on("end", () => stdout.end())
        own.once("exit", (code, signal) => {
          process_.exitCode = code
          process_.signalCode = signal
          events.emit("exit", code, signal)
        })
        own.once("close", (code, signal) => events.emit("close", code, signal))
        own.once("error", (failed) => events.emit("error", failed))
        return
      }
      stdout.end()
      events.emit("error", error instanceof Error ? error : new Error(String(error)))
      process_.exitCode = 1
      events.emit("exit", 1, null)
    },
  )
  return process_
}
