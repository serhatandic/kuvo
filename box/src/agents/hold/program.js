// kuvo-hold (hold/1): keeps one coding agent's process alive apart from the bridge that started
// it, so a bridge that crashes or makes way for a newer one doesn't take the agent's turn with it.
// It runs the agent as its own child on pipes (its parent never changes), numbers each line the
// agent writes, keeps the latest few MB of them, and lets one bridge at a time drive the agent over
// a unix socket: a bridge attaches with a generation, and a higher generation fences off the older
// one (only the newest may write). It ends once its agent has ended and a bridge has heard so.
//
// Protocol-agnostic, and changed rarely: a newer one is used only for agents started after it, and
// older ones live on until their agent ends. Plain JavaScript on Node's modules alone, so the
// bridge writes it out and runs it on its own Bun (client.ts), small, apart from the bridge's code.
//
// usage: <bun> kuvo-hold.js <folder> <id> <kind> <spec>
//   kind: "jsonrpc" (Codex, ACP: requests the agent sent that no bridge answered are sent again to
//         the next one), "claude" (its control requests are never sent again, nor replayed: Claude
//         Code sends its open asks again itself when a new SDK client initializes, and cancels open
//         hook callbacks, so a replayed one would be asked twice), or "raw" (nothing parsed).
//   spec: {command: [program, ...args], cwd, meta, holder, bun?, journalBytes?, linger?}
//   env:  the agent's, with KUVO_HOLD_KEY (the key a bridge gives first; the agent never sees it)
//
// Frames, one JSON object per line. A bridge sends first {hello: "hold/1", key, generation, from?};
// then {i: line} (to the agent's stdin), {ack: seq}, {meta: {...}}, {end: true} (close its stdin),
// {signal: name}, {bye: true}. The holder answers {hello, pid, child, generation, seq, first, gap,
// exited} or {refused}, then sends {s: seq, o: line} (again: true for a request sent again),
// {e: text} (stderr), {x: {code, signal}} (the agent ended), {fenced: true} (a newer bridge took
// over). Files: <folder>/<id>.sock and <id>.json (the record, with the key: the folder is 0700).

import { spawn } from "node:child_process"
import { randomBytes, timingSafeEqual } from "node:crypto"
import { renameSync, rmSync, writeFileSync } from "node:fs"
import net from "node:net"
import { join } from "node:path"

const PROTO = "hold/1"
const [folder, id, kind, specText] = process.argv.slice(2)
const spec = JSON.parse(specText ?? "{}")
const key = Buffer.from(process.env.KUVO_HOLD_KEY ?? "")
// (Nor does the agent get the key, nor the setting that runs the engine's binary as Bun, the
// holder's own, unless its bridge gave the agent one: `spec.bun`.)
const env = { ...process.env }
delete env.KUVO_HOLD_KEY
delete env.BUN_BE_BUN
if (typeof spec.bun === "string") env.BUN_BE_BUN = spec.bun
const socketPath = join(folder, `${id}.sock`)
const recordPath = join(folder, `${id}.json`)
const journalBytes = Number(spec.journalBytes) || 4_000_000
// How long it waits for a bridge to hear its agent ended, with none attached, before it goes.
const linger = Number(spec.linger) || 15 * 60_000

/** The latest lines the agent wrote, oldest first, at most `journalBytes` of them. */
const journal = []
let journaled = 0
let seq = 0
/** Requests the agent sent that no bridge has answered, by id, with their line's number. */
const pending = new Map()
let generation = 0
let acked = 0
let meta = spec.meta ?? {}
let controller
let exited = null
let stderrTail = ""
let leaving = false

if (key.length < 16 || !Array.isArray(spec.command) || !spec.command.length) {
  console.error("kuvo-hold: no key or command")
  process.exit(2)
}

const child = spawn(spec.command[0], spec.command.slice(1), { cwd: spec.cwd || undefined, env, stdio: ["pipe", "pipe", "pipe"] })
child.stdin.on("error", () => {})

function writeRecord() {
  const record = { proto: PROTO, holder: spec.holder ?? null, id, kind, pid: process.pid, child: child.pid ?? null, program: spec.command[0], cwd: spec.cwd ?? null, started, generation, acked, exited, meta, key: key.toString() }
  const temporary = `${recordPath}.${randomBytes(4).toString("hex")}`
  try {
    writeFileSync(temporary, JSON.stringify(record), { mode: 0o600 })
    renameSync(temporary, recordPath)
  } catch {
    rmSync(temporary, { force: true })
  }
}
let recordTimer
/** The record again soon (acks come often): once a second at most. */
function writeRecordSoon() {
  recordTimer ??= setTimeout(() => {
    recordTimer = undefined
    writeRecord()
  }, 1_000)
}
const started = Date.now()

const idOf = (message) => (message && typeof message === "object" && "id" in message && message.id !== undefined && message.id !== null ? JSON.stringify(message.id) : undefined)

/** A line the agent wrote: numbered, kept, and sent to the bridge attached. */
function fromAgent(line) {
  seq++
  if (kind === "jsonrpc") {
    try {
      const message = JSON.parse(line)
      const found = idOf(message)
      if (found !== undefined && typeof message.method === "string") pending.set(found, seq)
    } catch {}
  }
  const bytes = Buffer.byteLength(line)
  // (Claude Code's asks and hook callbacks: the live bridge gets them, a later one never.)
  const ask = kind === "claude" && line.includes('"control_request"') && isControlRequest(line)
  journal.push({ s: seq, line, bytes, ask })
  journaled += bytes
  while (journaled > journalBytes && journal.length > 1) journaled -= journal.shift().bytes
  send(controller, { s: seq, o: line })
}

function isControlRequest(line) {
  try {
    return JSON.parse(line)?.type === "control_request"
  } catch {
    return false
  }
}

/** A line a bridge sent the agent: an answer to one of its requests is no longer open. */
function toAgent(line) {
  if (kind === "jsonrpc") {
    try {
      const message = JSON.parse(line)
      const found = idOf(message)
      if (found !== undefined && message.method === undefined) pending.delete(found)
    } catch {}
  }
  if (!exited) child.stdin.write(line + "\n")
}

function send(socket, frame) {
  if (!socket || socket.destroyed) return
  socket.write(JSON.stringify(frame) + "\n")
}

function lines(stream, each) {
  let buffer = ""
  stream.setEncoding("utf8")
  stream.on("data", (chunk) => {
    buffer += chunk
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at).replace(/\r$/, "")
      buffer = buffer.slice(at + 1)
      if (line.trim()) each(line)
    }
  })
  return () => {
    if (buffer.trim()) each(buffer)
    buffer = ""
  }
}

const restOfStdout = lines(child.stdout, fromAgent)
child.stderr.setEncoding("utf8")
child.stderr.on("data", (text) => {
  stderrTail = (stderrTail + text).slice(-8_000)
  send(controller, { e: text })
})

/** A bridge's hello: the right key, and a generation no older than the newest's. */
function welcome(socket, hello) {
  const given = Buffer.from(String(hello?.key ?? ""))
  if (hello?.hello !== PROTO || given.length !== key.length || !timingSafeEqual(given, key)) {
    send(socket, { refused: "Not this holder's key." })
    return socket.end()
  }
  const wanted = Number(hello.generation)
  if (!Number.isSafeInteger(wanted) || wanted < generation || (wanted === generation && controller && !controller.destroyed)) {
    send(socket, { refused: `A bridge of generation ${generation} has this agent.` })
    return socket.end()
  }
  if (controller && controller !== socket) {
    send(controller, { fenced: true })
    controller.end()
  }
  controller = socket
  generation = wanted
  const from = Number.isSafeInteger(hello.from) ? hello.from : acked
  const first = journal[0]?.s ?? seq + 1
  send(socket, { hello: PROTO, pid: process.pid, child: child.pid ?? null, generation, seq, first, gap: from + 1 < first, exited, stderr: stderrTail, meta })
  for (const entry of journal) if (entry.s > from && !entry.ask) send(socket, { s: entry.s, o: entry.line })
  // What it asked and no bridge answered, which the new one has already seen: asked again.
  for (const at of pending.values()) {
    if (at > from) continue
    const entry = journal.find((each) => each.s === at)
    if (entry) send(socket, { s: entry.s, o: entry.line, again: true })
  }
  if (exited) send(socket, { x: exited })
  writeRecord()
}

function fromBridge(socket, frame) {
  if (socket !== controller) return
  if (typeof frame.i === "string") toAgent(frame.i)
  else if (Number.isSafeInteger(frame.ack) && frame.ack > acked) {
    acked = Math.min(frame.ack, seq)
    writeRecordSoon()
  } else if (frame.meta && typeof frame.meta === "object") {
    meta = { ...meta, ...frame.meta }
    writeRecord()
  } else if (frame.end) child.stdin.end()
  else if (typeof frame.signal === "string" && !exited) {
    try {
      child.kill(frame.signal)
    } catch {}
  } else if (frame.bye && exited) leave()
}

const server = net.createServer((socket) => {
  let hello = false
  socket.on("error", () => {})
  socket.on("close", () => {
    if (socket === controller) controller = undefined
    // (Heard of its agent's end, and gone: nothing left to hold.)
    if (exited && hello && socket.heardEnd) leave()
  })
  lines(socket, (line) => {
    let frame
    try {
      frame = JSON.parse(line)
    } catch {
      return socket.destroy()
    }
    if (!hello) {
      hello = true
      welcome(socket, frame)
      if (exited && socket === controller) socket.heardEnd = true
      return
    }
    fromBridge(socket, frame)
  })
})
server.on("error", (error) => {
  console.error("kuvo-hold:", error?.message ?? error)
  child.kill("SIGTERM")
  process.exit(1)
})
rmSync(socketPath, { force: true })
server.listen(socketPath, () => {
  writeRecord()
  // Ready: said once to the bridge starting it, and its stdout let go (that bridge may be gone soon).
  process.stdout.write(JSON.stringify({ ready: PROTO, pid: process.pid, child: child.pid ?? null }) + "\n", () => process.stdout.destroy())
})

let lingering
child.on("exit", (code, signal) => {
  restOfStdout()
  exited = { code, signal }
  writeRecord()
  if (controller) {
    send(controller, { x: exited })
    controller.heardEnd = true
  }
  lingering = setTimeout(leave, linger)
})
child.on("error", (error) => {
  stderrTail = (stderrTail + String(error?.message ?? error)).slice(-8_000)
  if (!exited) child.emit("exit", 127, null)
})

/** Its agent ended and a bridge heard (or none came in time): its files go, and so does it. */
function leave() {
  if (leaving) return
  leaving = true
  clearTimeout(lingering)
  clearTimeout(recordTimer)
  rmSync(socketPath, { force: true })
  rmSync(recordPath, { force: true })
  server.close()
  if (controller) controller.end()
  process.exit(0)
}

// Asked to stop (the box stopping): the agent is, as a bridge stops it (its stdin closed, then told).
process.on("SIGTERM", () => {
  if (exited) return leave()
  child.stdin.end()
  try {
    child.kill("SIGTERM")
  } catch {}
})
process.on("SIGHUP", () => {})
process.on("SIGINT", () => {})
