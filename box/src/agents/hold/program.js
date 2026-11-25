// kuvo-hold (hold/1): keeps one coding agent's process alive apart from the bridge that started
// it, so a bridge that crashes or makes way for a newer one doesn't take the agent's turn with it.
// It runs the agent as its own child on pipes (its parent never changes), numbers each line the
// agent writes, keeps the latest few MB of them, and lets one bridge at a time drive the agent over
// a unix socket: a bridge attaches with a generation, and a higher generation fences off the older
// one (only the newest may write). It ends once its agent has ended and a bridge has heard so; an
// agent no bridge attaches to for a while is stopped, as a bridge stops one.
//
// Protocol-agnostic, and changed rarely: a newer one is used only for agents started after it, and
// older ones live on until their agent ends. Plain JavaScript on Node's modules alone, so the
// bridge embeds the shared IPC module, writes the program out and runs it on its own Bun (client.ts).
//
// usage: <bun> kuvo-hold.js <folder> <id> <kind>, with one line on stdin: {key, spec}
//   kind: "jsonrpc" (Codex, ACP: requests the agent sent that no bridge answered are sent again to
//         the next one), "claude" (its control requests are never sent again, nor replayed: Claude
//         Code sends its open asks again itself when a new SDK client initializes, and cancels open
//         hook callbacks, so a replayed one would be asked twice), or "raw" (lines as written).
//   key:  what a bridge gives first (never in an environment or on a command line)
//   spec: {command: [program, ...args], cwd, meta, holder, bridge (its pid), bun?, journalBytes?, linger?, orphan?}
//   env:  the agent's
//
// Frames, one JSON object per line. A bridge sends first {hello: "hold/1", key, generation, from?};
// then {i: line} (to the agent's stdin), {ack: seq}, {meta: {...}}, {end: true} (close its stdin),
// {signal: name}, {bye: true}. The holder answers {hello, pid, child, generation, seq, first, gap,
// exited, stderr, meta} or {refused}, then sends {s: seq, o: line} (again: true for a request sent
// again), {e: text} (stderr), {x: {code, signal, heardAll?}} (the agent ended, after all it wrote;
// heardAll: its stderr ended too, rather than being held open past the wait), {fenced:
// true} (a newer bridge took over). Files: <folder>/<id>.sock, <id>.json (its record, with the
// key) and <id>.journal (what its agent said lately, 64 MB at most); the folder is 0700.

import { spawn } from "node:child_process"
import { randomBytes, timingSafeEqual } from "node:crypto"
import { appendFileSync, closeSync, openSync, readSync, renameSync, rmSync, writeFileSync, writeSync } from "node:fs"
import { join } from "node:path"

const PROTO = "hold/1"
const [folder, id, kind] = process.argv.slice(2)
const ipc = localIpc(join(folder ?? "", `${id}.sock`))
const socketPath = ipc.path
const recordPath = join(folder ?? "", `${id}.json`)
const journalPath = join(folder ?? "", `${id}.journal`)
/** Most asks it keeps open for a bridge to come (the oldest go first). */
const MOST_PENDING = 1_000
/** How long a bridge has to say hello, and how long its hello may be. */
const HELLO_WITHIN = 10_000
const HELLO_BYTES = 64_000
/** How long the agent's stdout may stay open after it ended (something it started holds it). */
const OUTPUT_AFTER_END = 2_000
/** How long a bridge may leave what's sent to it unread before it's let go (hung), to attach again. */
const HUNG = 60_000

const given = await new Promise((resolve) => {
  let text = ""
  process.stdin.setEncoding("utf8")
  process.stdin.on("data", (chunk) => (text += chunk))
  process.stdin.on("end", () => {
    try {
      resolve(JSON.parse(text) ?? {})
    } catch {
      resolve({})
    }
  })
})
const key = Buffer.from(String(given.key ?? ""))
const spec = given.spec ?? {}
if (key.length < 16 || !Array.isArray(spec.command) || !spec.command.length || !folder || !/^[0-9a-f]{12}$/.test(id ?? "")) {
  console.error("kuvo-hold: no key, command or id")
  process.exit(2)
}
// (Nor does the agent get the setting that runs the engine's binary as Bun, the holder's own,
// unless its bridge gave the agent one: `spec.bun`.)
const env = { ...process.env }
delete env.BUN_BE_BUN
if (typeof spec.bun === "string") env.BUN_BE_BUN = spec.bun
// Its journal, on disk beside its record: what a turn said since it began, for a bridge taking it up
// (a long turn of Claude Code's, its words streamed and its tools' output, runs to tens of MB).
const journalBytes = Number(spec.journalBytes) || 64_000_000
// How long it waits for a bridge to hear its agent ended, with none attached, before it goes.
const linger = Number(spec.linger) || 15 * 60_000
// How long its agent runs with no bridge attached before it's stopped (none came to adopt it).
const orphan = Number(spec.orphan) || 15 * 60_000

/**
 * The latest lines the agent wrote, oldest first: each line's number, where it is in the journal's
 * file, and whether it's never to be replayed (`ask`). The file holds at most `journalBytes`: past
 * that, its older half goes. Line `s` is `journal[s - journal[0].s]`.
 */
let journal = []
let journalFile = openSync(journalPath, "w+", 0o600)
let journaled = 0
let pinned
let pausedForJournal = false
let seq = 0
/** Requests the agent sent that no bridge has answered, by id: their line and its number. */
const pending = new Map()
/** What the bridges attached so far took in: where the next one starts, unless it says. */
let acked = 0
let generation = 0
let meta = spec.meta ?? {}
let controller
let exited = null
let stderrTail = ""
let leaving = false
let stopping = false
let orphaned
let lingering
let ending = false
const started = Date.now()

// In a process group of its own: stopping it reaches what it started too, as stopping the bridge's
// group did before holders.
const child = spawn(spec.command[0], spec.command.slice(1), { cwd: spec.cwd || undefined, env, stdio: ["pipe", "pipe", "pipe"], detached: true })
// A holder can outlive its bridge. Keep exit evidence in the private holder directory,
// without recording agent output, request bodies, or credentials.
function report(event, fields = {}) {
  try {
    appendFileSync(join(folder, "lifecycle.log"), JSON.stringify({ at: new Date().toISOString(), event, id, holder: process.pid, child: child.pid, bridge: spec.bridge, provider: meta.provider, generation, ...fields }) + "\n", { mode: 0o600 })
  } catch {}
}
/** A signal to the agent and all it started (its group), or to it alone if that's gone. */
function signal(name) {
  try {
    if (child.pid) process.kill(-child.pid, name)
  } catch {
    try {
      child.kill(name)
    } catch {}
  }
}
child.stdin.on("error", () => {})

function writeRecord() {
  const record = { proto: PROTO, holder: spec.holder ?? null, id, kind, pid: process.pid, child: child.pid ?? null, program: spec.command[0], cwd: spec.cwd ?? null, started, bridge: spec.bridge ?? null, generation, exited, meta, key: key.toString() }
  const temporary = `${recordPath}.${randomBytes(4).toString("hex")}`
  try {
    writeFileSync(temporary, JSON.stringify(record), { mode: 0o600 })
    renameSync(temporary, recordPath)
  } catch {
    rmSync(temporary, { force: true })
  }
}

const idKey = (value) => (value === undefined || value === null ? undefined : JSON.stringify(value))

/** What a line of the agent's says of the asks open: a new one, or one it settled itself. (Both
 * kinds keep `ask` on a journal line that's never to be replayed: Claude Code's control requests,
 * and a JSON-RPC ask once it's answered or settled.) */
function noteAsks(line) {
  let message
  try {
    message = JSON.parse(line)
  } catch {
    return
  }
  if (!message || typeof message !== "object" || typeof message.method !== "string") return
  const asked = idKey(message.id)
  if (asked !== undefined) {
    pending.set(asked, { s: seq, line, method: message.method })
    if (pending.size > MOST_PENDING) pending.delete(pending.keys().next().value)
    return
  }
  // Codex settled one itself (`serverRequest/resolved`), or an agent took one back (`$/cancelRequest`).
  if (message.method === "serverRequest/resolved" || message.method === "$/cancelRequest") {
    const settled = message.params?.requestId ?? message.params?.id
    if (settled === undefined || settled === null) return
    for (const each of [settled, String(settled), Number(settled)]) settle(idKey(each))
  }
}

function isControlRequest(line) {
  try {
    return JSON.parse(line)?.type === "control_request"
  } catch {
    return false
  }
}

/** A line the agent wrote: numbered, kept, and sent to the bridge attached. */
function fromAgent(line) {
  seq++
  if (kind === "jsonrpc") noteAsks(line)
  const text = Buffer.from(line + "\n")
  // (Claude Code's asks and hook callbacks: the live bridge gets them, a later one never.)
  const ask = kind === "claude" && line.includes('"control_request"') && isControlRequest(line)
  writeSync(journalFile, text, 0, text.length, journaled)
  journal.push({ s: seq, at: journaled, bytes: text.length, ask })
  journaled += text.length
  if (journaled > journalBytes) {
    if (pinned === undefined) prune()
    else {
      pausedForJournal = true
      child.stdout.pause()
    }
  }
  pump(controller)
}

/** The journal's older half gone: the newer copied to a file of its own, put in its place. */
function prune() {
  let from = journal.length - 1
  let kept = journal[from].bytes
  while (from > 0 && kept + journal[from - 1].bytes <= journalBytes / 2) kept += journal[--from].bytes
  const start = journal[from].at
  const temporary = `${journalPath}.${randomBytes(4).toString("hex")}`
  const fresh = openSync(temporary, "w+", 0o600)
  const chunk = Buffer.alloc(1_000_000)
  for (let at = start; at < journaled; ) {
    const read = readSync(journalFile, chunk, 0, Math.min(chunk.length, journaled - at), at)
    writeSync(fresh, chunk, 0, read, at - start)
    at += read
  }
  closeSync(journalFile)
  renameSync(temporary, journalPath)
  journalFile = fresh
  journal = journal.slice(from).map((entry) => ({ ...entry, at: entry.at - start }))
  journaled -= start
}

/** Line `s` as the journal has it, or none if it's gone from it. */
function journalLine(s) {
  const entry = journal[s - (journal[0]?.s ?? 0)]
  if (!entry || entry.s !== s) return undefined
  const text = Buffer.alloc(entry.bytes)
  readSync(journalFile, text, 0, entry.bytes, entry.at)
  return { entry, line: text.toString("utf8", 0, entry.bytes - 1) }
}

/** An ask no longer open: not sent again, nor replayed (a new bridge would ask it again). */
function settle(found) {
  const open = pending.get(found)
  if (!open) return
  pending.delete(found)
  const entry = journal[open.s - (journal[0]?.s ?? 0)]
  if (entry?.s === open.s) entry.ask = true
}

/** A line a bridge sent the agent: an answer to one of its requests is no longer open. */
function toAgent(line) {
  if (kind === "jsonrpc") {
    try {
      const message = JSON.parse(line)
      const answered = message && typeof message === "object" && message.method === undefined ? idKey(message.id) : undefined
      if (answered !== undefined) settle(answered)
    } catch {}
  }
  if (!exited) child.stdin.write(line + "\n")
}

/** A frame to a bridge, whatever's waiting for it (its hello, a refusal, its stderr). */
function send(socket, frame) {
  if (!socket || socket.destroyed) return true
  return socket.write(JSON.stringify(frame) + "\n")
}

/**
 * What the bridge attached hasn't had yet, in order: the lines from the journal (those after where
 * it started, as it attached, then each as it comes), and the agent's end once all are sent. While
 * the bridge doesn't keep up, its socket full, the agent's stdout isn't read (the agent waits, as
 * on any pipe); a bridge that reads nothing for a minute is let go (hung), to attach again from
 * its ack.
 */
function pump(socket) {
  if (!socket || socket !== controller || socket.destroyed || socket.waiting) return
  while (socket.sent < seq) {
    const next = socket.sent + 1
    const found = journalLine(next)
    socket.sent = next
    if (!found) {
      // (Gone from the journal before it was sent: this bridge can't be told all. It attaches again.)
      if (journal[0] && next < journal[0].s) {
        report("journal gap", { next, first: journal[0].s })
        return socket.destroy()
      }
      continue
    }
    // (An ask never replayed: one said before this bridge attached, which it's told otherwise.)
    if (found.entry.ask && next <= socket.liveFrom) continue
    if (!socket.write(JSON.stringify({ s: next, o: found.line }) + "\n")) return wait(socket)
  }
  if (socket.replayThrough !== undefined && socket.releaseReplay && socket.sent >= socket.replayThrough) {
    socket.replayThrough = undefined
    unpin()
  }
  if (exited && !socket.heardEnd) {
    socket.heardEnd = true
    send(socket, { x: exited })
  }
}

/** Its socket full: the agent's output waits until the bridge reads on (or is let go). */
function wait(socket) {
  socket.waiting = true
  child.stdout.pause()
  const hung = setTimeout(() => {
    report("controller backpressure timeout", { sent: socket.sent, seq, acked })
    socket.destroy()
  }, HUNG)
  /** Waits no more: the agent's output read on (and this bridge sent more, if it's still the one). */
  socket.unwait = () => {
    clearTimeout(hung)
    socket.off("drain", go)
    socket.off("close", go)
    socket.waiting = false
    if (!pausedForJournal) child.stdout.resume()
  }
  const go = () => {
    socket.unwait()
    pump(socket)
  }
  socket.on("drain", go)
  socket.on("close", go)
}

/** Lines from a stream; `exact`: as written (blank ones and carriage returns too). Returns what's left, to say at its end. */
function lines(stream, each, { exact = false, most } = {}) {
  let buffer = ""
  stream.setEncoding("utf8")
  stream.on("data", (chunk) => {
    buffer += chunk
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = exact ? buffer.slice(0, at) : buffer.slice(0, at).replace(/\r$/, "")
      buffer = buffer.slice(at + 1)
      if (exact || line.trim()) each(line)
    }
    if (most && buffer.length > most()) stream.destroy()
  })
  return () => {
    if (exact ? buffer.length : buffer.trim()) each(buffer)
    buffer = ""
  }
}

// (Its stderr too: what it says as it stops is why it did. Something it left running can hold
// either open, which the wait after its end bounds.)
let heardAll = false
const outputDone = Promise.all([
  new Promise((resolve) => child.stdout.once("close", resolve)),
  new Promise((resolve) => child.stderr.once("close", () => resolve(heardAll = true))),
])
const restOfStdout = lines(child.stdout, fromAgent, { exact: kind === "raw" })
child.stderr.setEncoding("utf8")
child.stderr.on("data", (text) => {
  stderrTail = (stderrTail + text).slice(-8_000)
  send(controller, { e: text })
})

/** A bridge's hello: the right key, and a generation no older than the newest's. */
function welcome(socket, hello) {
  const offered = Buffer.from(String(hello?.key ?? ""))
  if (hello?.hello !== PROTO || offered.length !== key.length || !timingSafeEqual(offered, key)) {
    send(socket, { refused: "Not this holder's key." })
    return socket.end()
  }
  const wanted = Number(hello.generation)
  if (!Number.isSafeInteger(wanted) || wanted < generation || (wanted === generation && controller && !controller.destroyed)) {
    send(socket, { refused: `A bridge of generation ${generation} has this agent.` })
    return socket.end()
  }
  if (hello.complete && hello.from + 1 < (journal[0]?.s ?? seq + 1)) {
    send(socket, { refused: "The turn is no longer in the journal." })
    return socket.end()
  }
  if (controller && controller !== socket) {
    // (One that stopped reading holds nothing back now: the newer bridge's lines go on at once.)
    if (controller.waiting) controller.unwait()
    send(controller, { fenced: true })
    controller.end()
  }
  controller = socket
  generation = wanted
  report("controller attached")
  clearTimeout(orphaned)
  const from = Math.min(Number.isSafeInteger(hello.from) ? hello.from : acked, seq)
  const first = journal[0]?.s ?? seq + 1
  send(socket, { hello: PROTO, pid: process.pid, child: child.pid ?? null, generation, seq, first, gap: from + 1 < first, exited, stderr: stderrTail, meta })
  // What it asked and no bridge answered, which the new one has already seen: asked again, first.
  for (const { s, line } of pending.values()) if (s <= from) send(socket, { s, o: line, again: true })
  // Then what came since where it starts (what's still in the journal), and on as it comes.
  socket.sent = Math.max(from, first - 1)
  socket.liveFrom = seq
  if (pinned !== undefined) {
    socket.replayThrough = seq
    socket.releaseReplay = !hello.complete
  }
  pump(socket)
  writeRecord()
}

function unpin() {
  pinned = undefined
  if (!pausedForJournal) return
  pausedForJournal = false
  if (journaled > journalBytes) prune()
  if (!controller?.waiting && !ending) child.stdout.resume()
}

function fromBridge(socket, frame) {
  if (socket !== controller || !frame || typeof frame !== "object") return
  if (typeof frame.i === "string") toAgent(frame.i)
  else if (Number.isSafeInteger(frame.ack)) acked = Math.max(acked, Math.min(frame.ack, seq))
  else if (frame.meta && typeof frame.meta === "object") {
    meta = { ...meta, ...frame.meta }
    writeRecord()
  } else if (frame.prepare !== undefined) {
    // The current controller reserves the replay range before agreeing to a transfer.
    const from = frame.prepare
    // ACP may be waiting for a file write or another side effect already begun by this
    // controller. Only permission decisions replay safely without that controller.
    const portable = meta.kind !== "antigravity" || [...pending.values()].every(ask => ask.method === "session/request_permission")
    const ready = portable && !exited && Number.isSafeInteger(from) && from <= seq && from + 1 >= (journal[0]?.s ?? seq + 1)
    if (ready) pinned = from
    send(socket, { prepared: ready })
  } else if (frame.unpin) {
    if (socket.replayThrough === undefined) unpin()
    else {
      socket.releaseReplay = true
      pump(socket)
    }
    if (exited && socket.heardEnd && pinned === undefined) leave()
  } else if (frame.end) {
    report("controller closed stdin")
    child.stdin.end()
  } else if (typeof frame.signal === "string" && !exited) {
    report("controller signalled child", { signal: frame.signal })
    signal(frame.signal)
  }
  else if (frame.bye && exited && pinned === undefined) leave()
}

function serverError(error) {
  report("holder server error", { code: error?.code })
  console.error("kuvo-hold:", error?.message ?? error)
  signal("SIGTERM")
  process.exit(1)
}

rmSync(socketPath, { force: true })
const server = ipc.listen((socket) => {
  let hello = false
  const late = setTimeout(() => !hello && socket.destroy(), HELLO_WITHIN)
  socket.on("error", () => {})
  socket.on("close", () => {
    clearTimeout(late)
    // (Only the bridge attached counts: one fenced off as it closes says nothing of this one.)
    if (socket !== controller) return
    controller = undefined
    if (!exited) report("controller disconnected", { seq, acked })
    // Heard of its agent's end, and gone (or asked to stop, its agent ended): nothing left to hold.
    if (exited && (stopping || (socket.heardEnd && pinned === undefined))) return leave()
    if (!ending) waitForBridge()
  })
  lines(
    socket,
    (line) => {
      let frame
      try {
        frame = JSON.parse(line)
      } catch {
        return socket.destroy()
      }
      if (!hello) {
        hello = true
        clearTimeout(late)
        return welcome(socket, frame)
      }
      fromBridge(socket, frame)
    },
    { most: () => (hello ? Infinity : HELLO_BYTES) },
  )
}, {
  onError: serverError,
  onListening: () => {
    report("holder listening")
    writeRecord()
    if (!controller && !ending) waitForBridge()
    // Ready: said once to the bridge starting it, and its stdout let go (that bridge may be gone soon).
    process.stdout.write(JSON.stringify({ ready: PROTO, pid: process.pid, child: child.pid ?? null }) + "\n", () => process.stdout.destroy())
  },
})
void server.ready.catch(serverError)

/** With no bridge attached, its agent is stopped after a while (none came to adopt it). */
function waitForBridge() {
  clearTimeout(orphaned)
  orphaned = setTimeout(() => stop("orphan timeout"), orphan)
}

/** Its agent ended: said once all it wrote is in (or a moment later, if something it started keeps its output open). */
function ended(code, signal) {
  if (ending) return
  ending = true
  report("child exited", { code, signal, stopping, stderrBytes: Buffer.byteLength(stderrTail) })
  clearTimeout(orphaned)
  void Promise.race([outputDone, new Promise((resolve) => setTimeout(resolve, OUTPUT_AFTER_END))]).then(() => {
    restOfStdout()
    child.stdout.destroy()
    exited = heardAll ? { code, signal, heardAll } : { code, signal }
    writeRecord()
    if (controller) pump(controller)
    // (Asked to stop, with no bridge there: none is waiting to hear it.)
    if (stopping && !controller) return leave()
    lingering = setTimeout(leave, linger)
  })
}
child.on("exit", (code, signal) => ended(code, signal))
child.on("error", (error) => {
  report("child spawn error", { code: error?.code })
  stderrTail = (stderrTail + String(error?.message ?? error)).slice(-8_000)
  child.stdout.destroy()
  ended(127, null)
})

/** Its agent ended and a bridge heard (or none came in time): its files go, and so does it. */
function leave() {
  if (leaving) return
  leaving = true
  clearTimeout(lingering)
  clearTimeout(orphaned)
  rmSync(socketPath, { force: true })
  rmSync(recordPath, { force: true })
  try {
    closeSync(journalFile)
  } catch {}
  rmSync(journalPath, { force: true })
  server.close()
  if (controller && !controller.destroyed) {
    // (What it was sent goes first.)
    setTimeout(() => process.exit(0), 1_000)
    controller.end(() => process.exit(0))
  } else process.exit(0)
}

/** Stops its agent as a bridge stops one: its stdin closed, then told, then made to. */
function stop(reason) {
  if (exited) return leave()
  if (stopping) return
  stopping = true
  report("holder stopping child", { reason })
  child.stdin.end()
  const told = setTimeout(() => signal("SIGTERM"), 5_000)
  const made = setTimeout(() => signal("SIGKILL"), 15_000)
  child.once("exit", () => {
    clearTimeout(told)
    clearTimeout(made)
    // (What it left running in its group goes with it, as the box stops.)
    signal("SIGTERM")
  })
}

// Asked to stop (the box stopping): its agent too, and it goes once that has.
process.on("SIGTERM", () => stop("SIGTERM"))
process.on("SIGHUP", () => {})
process.on("SIGINT", () => {})
