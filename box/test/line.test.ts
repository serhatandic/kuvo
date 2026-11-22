import "./safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { offShape } from "./route-answer"

// The one line (PROTOCOL.md, "The line"): one stream per client, carrying every topic it follows,
// numbered, resumed after a drop with what it missed, started over when the box can't. The box's
// real dispatcher; the engine and the coding agents' bridge are stand-ins that stream as theirs do
// (the engine's `api/event` and the bridge's, which carries Claude Code, Codex, Cursor, Grok and
// Antigravity alike; a bridge ndjson follow).

const { engineHandler } = await import("../src/link/server/router")
const { endLines, lineTiming } = await import("../src/link/server/line")

type Feed = { send: (text: string) => void; close: () => void }
const encoder = new TextEncoder()
/** Each open event stream of a stand-in, newest last. */
function feeds() {
  const open: Feed[] = []
  const waiting: (() => void)[] = []
  /** Once `count` have been opened. */
  const opened = (count: number) => new Promise<void>(done => {
    const check = () => (open.length >= count ? done() : waiting.push(check))
    check()
  })
  const answer = (type: string, first?: string) => new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      const feed = {
        send: (text: string) => { try { controller.enqueue(encoder.encode(text)) } catch {} },
        close: () => { try { controller.close() } catch {} },
      }
      open.push(feed)
      if (first) feed.send(first)
      for (const check of waiting.splice(0)) check()
    },
  }), { headers: { "content-type": type } })
  return { open, answer, opened }
}

const engineFeeds = feeds()
const engine = Bun.serve({ port: 0, hostname: "127.0.0.1", idleTimeout: 0, fetch: request => {
  const path = new URL(request.url).pathname
  if (path === "/api/event") return engineFeeds.answer("text/event-stream", ": connected\n\n")
  return Response.json({ message: "Not here." }, { status: 404 })
} })
const bridgeFeeds = feeds()
const sideFeeds = feeds()
let threads = { threads: [] as unknown[] }
const bridge = Bun.serve({ port: 0, hostname: "127.0.0.1", idleTimeout: 0, fetch: request => {
  const path = new URL(request.url).pathname
  if (path === "/api/event") return bridgeFeeds.answer("text/event-stream")
  if (path === "/api/kuvo/btw/ses_work/threads/events") return sideFeeds.answer("application/x-ndjson", JSON.stringify(threads) + "\n")
  return Response.json({ message: "Not available for this agent." }, { status: 404 })
} })
// (The bridge's record names a process that's it: one of this test's own, stopped at its end.)
const bridgeProcess = spawn(process.execPath, ["-e", "setTimeout(() => {}, 120_000)", "kuvo-agents"], { stdio: "ignore" })
afterAll(() => { engine.stop(true); bridge.stop(true); bridgeProcess.kill() })
afterEach(() => { endLines(); lineTiming.beat = 5_000 })

const handler = engineHandler(() => ({ port: engine.port, password: "test" }) as any, { name: "test" } as any, () => ({ port: bridge.port, password: "test", pid: bridgeProcess.pid }) as any)

const event = (type: string, sessionID: string, more: object = {}) => `data: ${JSON.stringify({ type, properties: { sessionID, ...more } })}\n\n`
const watchOf = (topics: Record<string, string>) => Buffer.from(JSON.stringify(topics)).toString("base64url")

/** A line opened as the desktop app opens it: each line it says, as it says it. */
function open(topics: Record<string, string>, resume?: { line: string; after: number }, once = false) {
  const stop = new AbortController()
  const query = new URLSearchParams({ watch: watchOf(topics), ...(resume ? { line: resume.line, after: String(resume.after) } : {}), ...(once ? { once: "1" } : {}) })
  const pieces = handler({ method: "GET", path: `kuvo/line?${query}`, headers: {}, fromApp: true, served: true }, undefined, stop.signal)
  let buffer = ""
  let status = 0
  const next = async (): Promise<any> => {
    for (;;) {
      const end = buffer.indexOf("\n")
      if (end >= 0) {
        const text = buffer.slice(0, end)
        buffer = buffer.slice(end + 1)
        const line = JSON.parse(text)
        const why = offShape("GET", "kuvo/line", status, line)
        if (why) throw new Error(why)
        return line
      }
      const { value, done } = await pieces.next()
      if (done) return null
      if ("head" in value) status = value.head.status
      if ("body" in value) buffer += Buffer.from(value.body).toString()
    }
  }
  return {
    next,
    /** The next line that isn't a heartbeat. */
    async said() {
      for (;;) {
        const line = await next()
        if (!line || !("beat" in line)) return line
      }
    },
    async drop() { stop.abort(); await pieces.return(undefined) },
  }
}

test("one line carries the engine's events, the coding agents' events and a list's states, numbered", async () => {
  threads = { threads: [] }
  const line = open({ engine: "api/event", agents: "kuvo/agents/api/event", side: "kuvo/agents/api/kuvo/btw/ses_work/threads/events" })
  const hello = await line.next()
  expect(hello.hello).toEqual({ line: expect.any(String), fresh: true, beat: 5_000 })
  const first = await line.said()
  expect(first).toEqual({ t: "side", n: 1, d: { threads: [] } })
  await engineFeeds.opened(1)
  await bridgeFeeds.opened(1)
  engineFeeds.open.at(-1)!.send(event("message.part.updated", "ses_engine"))
  expect(await line.said()).toEqual({ t: "engine", n: 2, d: { type: "message.part.updated", properties: { sessionID: "ses_engine" } } })
  // Claude Code's turn, through the bridge: the same line, the same way.
  const agents = bridgeFeeds.open.at(-1)!
  agents.send(event("session.status", "ses_claude", { status: { type: "busy" } }))
  agents.send(event("message.part.updated", "ses_claude", { part: { type: "tool", tool: "bash" } }))
  expect(await line.said()).toMatchObject({ t: "agents", n: 3, d: { type: "session.status" } })
  expect(await line.said()).toMatchObject({ t: "agents", n: 4, d: { properties: { part: { tool: "bash" } } } })
  await line.drop()
})

test("a dropped line, opened again from where it was, gets what it missed and then what comes", async () => {
  const line = open({ engine: "api/event" })
  const { hello } = await line.next()
  const before = engineFeeds.open.length
  await engineFeeds.opened(before + 1)
  const feed = engineFeeds.open.at(-1)!
  feed.send(event("message.part.updated", "ses_1", { step: 1 }))
  const one = await line.said()
  expect(one).toMatchObject({ n: 1, d: { properties: { step: 1 } } })
  await line.drop()
  // The turn goes on while the client is away (a restart of its connection, a slow loop).
  feed.send(event("message.part.updated", "ses_1", { step: 2 }))
  feed.send(event("message.updated", "ses_1", { step: 3 }))
  const again = open({ engine: "api/event" }, { line: hello.line, after: one.n })
  expect((await again.next()).hello).toEqual({ line: hello.line, fresh: false, beat: 5_000 })
  expect(await again.said()).toMatchObject({ t: "engine", n: 2, d: { properties: { step: 2 } } })
  expect(await again.said()).toMatchObject({ t: "engine", n: 3, d: { type: "message.updated" } })
  feed.send(event("session.idle", "ses_1"))
  expect(await again.said()).toMatchObject({ n: 4, d: { type: "session.idle" } })
  // (The same stream all along: nothing was opened again on the engine.)
  expect(engineFeeds.open.at(-1)).toBe(feed)
  await again.drop()
})

test("a line the box doesn't know (it started again) starts over, fresh, from each topic's state now", async () => {
  threads = { threads: [{ id: "btw_1" }] }
  const line = open({ side: "kuvo/agents/api/kuvo/btw/ses_work/threads/events" }, { line: "gone", after: 40 })
  const { hello } = await line.next()
  expect(hello.fresh).toBe(true)
  expect(hello.line).not.toBe("gone")
  expect(await line.said()).toEqual({ t: "side", n: 1, d: { threads: [{ id: "btw_1" }] } })
  await line.drop()
})

test("a topic the box turns down is said once and ends; the others go on", async () => {
  threads = { threads: [] }
  const line = open({ missing: "kuvo/agents/api/kuvo/nothing/events", side: "kuvo/agents/api/kuvo/btw/ses_work/threads/events" })
  await line.next()
  const seen = [await line.said(), await line.said()]
  expect(seen).toContainEqual({ t: "missing", n: expect.any(Number), refused: { status: 404, message: "Not available for this agent.", said: { message: "Not available for this agent." } } })
  expect(seen).toContainEqual({ t: "side", n: expect.any(Number), d: { threads: [] } })
  await line.drop()
})

test("an event stream that drops and comes back says there may be a gap, and goes on", async () => {
  const line = open({ engine: "api/event" })
  const before = engineFeeds.open.length
  await line.next()
  await engineFeeds.opened(before + 1)
  engineFeeds.open.at(-1)!.close()
  expect(await line.said()).toMatchObject({ t: "engine", gap: true })
  await engineFeeds.opened(before + 2)
  engineFeeds.open.at(-1)!.send(event("session.idle", "ses_1"))
  expect(await line.said()).toMatchObject({ t: "engine", d: { type: "session.idle" } })
  await line.drop()
})

test("a quiet line beats, so silence means the box is gone, not idle", async () => {
  lineTiming.beat = 30
  const line = open({})
  expect((await line.next()).hello.beat).toBe(30)
  expect(await line.next()).toEqual({ beat: 0 })
  expect(await line.next()).toEqual({ beat: 0 })
  await line.drop()
})

test("a topic added by request comes on the line already open", async () => {
  threads = { threads: [{ id: "btw_2" }] }
  const line = open({})
  const { hello } = await line.next()
  const watch = async (id: string) => {
    let status = 0
    let text = ""
    for await (const piece of handler({ method: "POST", path: "kuvo/line/watch", headers: {}, fromApp: true, served: true, body: Buffer.from(JSON.stringify({ line: id, watch: { side: "kuvo/agents/api/kuvo/btw/ses_work/threads/events" } })) }, undefined)) {
      if ("head" in piece) status = piece.head.status
      if ("body" in piece) text += Buffer.from(piece.body).toString()
    }
    return { status, json: JSON.parse(text) }
  }
  expect(await watch(hello.line)).toEqual({ status: 200, json: { line: hello.line } })
  expect(await line.said()).toEqual({ t: "side", n: 1, d: { threads: [{ id: "btw_2" }] } })
  // One it doesn't know: the client opens the line again.
  expect(await watch("gone")).toEqual({ status: 404, json: { fresh: true, message: "Open the line again." } })
  await line.drop()
})

test("once: each topic's state now, then it ends", async () => {
  threads = { threads: [] }
  const line = open({ side: "kuvo/agents/api/kuvo/btw/ses_work/threads/events", missing: "kuvo/agents/api/kuvo/nothing/events" }, undefined, true)
  expect((await line.next()).hello.fresh).toBe(true)
  const seen = [await line.said(), await line.said()]
  expect(seen.map(each => each.t).sort()).toEqual(["missing", "side"])
  expect(await line.said()).toBeNull()
})
