// A command's message shows as it was typed, a queued message taken back to edit keeps its keyword,
// and Stop takes the messages still on their way with it (PROTOCOL.md, "A message's parts" and "The
// queue"): `kuvo/sessions/<id>/send`, `kuvo/messages/parts`, the queue's items and DELETE, and
// `stop`, with the engine and the agents bridge as stand-ins (their HTTP APIs, and the engine's
// inbox events as the box's watcher hands them on) and the box's code real.

import "./safety"
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test"
import type { AnswerPiece, RouteContext } from "../src/link/server/context"

type Asked = { method: string; path: string; body: any }
type Item = { id: string; type: "user"; delivery: "steer" | "queue"; payload: { text: string; metadata?: unknown } }

let engine: ReturnType<typeof Bun.serve>
let bridge: ReturnType<typeof Bun.serve>
let engineAsked: Asked[] = []
let bridgeAsked: Asked[] = []
let inbox: Item[] = []
let made = 0
/** What the engine says of each command it's sent, as its event stream would (`hold` it to say it later). */
let hearing: "at once" | "hold" = "at once"
let held: (() => void)[] = []
/** A prompt the engine takes only once this is let go (a send still on its way). */
let slow: { arrived: () => void; go: Promise<void> } | null = null
/** Requests a test waits for, each told once one like it comes (after it's answered, for a command). */
let waits: { like: (each: Asked) => boolean; told: () => void }[] = []
const whenAsked = (like: (each: Asked) => boolean) => new Promise<void>(told => { waits.push({ like, told }) })
const tell = (each: Asked) => {
  for (const wait of waits.filter(wait => wait.like(each))) {
    waits.splice(waits.indexOf(wait), 1)
    wait.told()
  }
}

let mod: {
  sendRoute: typeof import("../src/link/areas/messages").sendRoute
  partsRoute: typeof import("../src/link/areas/messages").partsRoute
  SEND_ROUTE: RegExp
  queueRoute: typeof import("../src/link/areas/queue").queueRoute
  QUEUE_ROUTE: RegExp
  heardCommand: typeof import("../src/link/messages/commands").heardCommand
}

const session = (id: string) => Response.json({ data: { id, location: { directory: "/srv/app" }, time: { created: 1, updated: 1 } } })

beforeAll(async () => {
  const messages = await import("../src/link/areas/messages")
  const queue = await import("../src/link/areas/queue")
  const commands = await import("../src/link/messages/commands")
  mod = { sendRoute: messages.sendRoute, partsRoute: messages.partsRoute, SEND_ROUTE: messages.SEND_ROUTE, queueRoute: queue.queueRoute, QUEUE_ROUTE: queue.QUEUE_ROUTE, heardCommand: commands.heardCommand }
  engine = Bun.serve({
    port: 0,
    async fetch(request) {
      const url = new URL(request.url)
      const text = await request.text()
      const each = { method: request.method, path: url.pathname, body: text ? JSON.parse(text) : null }
      engineAsked.push(each)
      tell(each)
      const enqueue = (item: Item) => {
        inbox.push(item)
        const event = { type: "session.inbox.enqueued", data: { sessionID: "ses_cmd", inboxID: item.id, item: { type: "user", delivery: item.delivery, payload: item.payload } } }
        return () => mod.heardCommand(event)
      }
      if (each.path === "/api/session/ses_cmd") return session("ses_cmd")
      if (each.path === "/api/session/active") return Response.json({ data: [] })
      if (each.path === "/api/session/ses_cmd/inbox") return Response.json({ data: inbox })
      if (each.path === "/api/session/ses_cmd/message") return Response.json({ data: [] })
      const item = each.path.match(/^\/api\/session\/ses_cmd\/inbox\/(.+)$/)?.[1]
      if (item && each.method === "PATCH") {
        for (const entry of inbox) if (entry.id === item) entry.delivery = each.body.delivery
        return Response.json({})
      }
      if (item && each.method === "DELETE") {
        inbox = inbox.filter(entry => entry.id !== item)
        return Response.json({})
      }
      if (each.path === "/api/session/ses_cmd/command") {
        // (The engine makes the prompt of it itself, and says nothing of which message it made.)
        const heard = enqueue({ id: `msg_made${++made}`, type: "user", delivery: each.body.delivery ?? "steer", payload: { text: `Review the code: ${each.body.text}` } })
        if (hearing === "at once") heard()
        else held.push(heard)
        return Response.json({})
      }
      if (each.path === "/api/session/ses_cmd/prompt") {
        if (slow) {
          slow.arrived()
          await slow.go
        }
        enqueue({ id: each.body.id, type: "user", delivery: each.body.delivery, payload: { text: each.body.text, metadata: each.body.metadata } })()
        return Response.json({ data: {} })
      }
      if (/^\/api\/session\/ses_cmd\//.test(each.path)) return Response.json({})
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
  bridge = Bun.serve({
    port: 0,
    async fetch(request) {
      const text = await request.text()
      const each = { method: request.method, path: new URL(request.url).pathname, body: text ? JSON.parse(text) : null }
      bridgeAsked.push(each)
      if (each.path === "/api/session/ses_agent") return session("ses_agent")
      if (/^\/api\/session\/ses_agent\//.test(each.path)) return Response.json({ data: {} })
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
})

afterAll(() => {
  engine.stop(true)
  bridge.stop(true)
})

beforeEach(async () => {
  engineAsked = []
  bridgeAsked = []
  inbox = []
  hearing = "at once"
  held = []
  slow = null
  waits = []
  // (Each starts with nothing held, nothing paused.)
  await queueAsk("POST", "kuvo/sessions/ses_cmd/queue/resume", {})
})

async function ask(answer: (context: RouteContext) => AsyncIterable<AnswerPiece>, method: string, route: string, body: unknown): Promise<{ status: number; json: any }> {
  const json = (status: number, value: unknown): AnswerPiece[] => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }]
  const record = (server: ReturnType<typeof Bun.serve>) => ({ pid: process.pid, port: server.port!, password: "pw", version: "test" })
  const context = {
    request: { method, path: route, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), fromApp: true, fromMac: false },
    phone: undefined, signal: undefined, route,
    readEngine: () => record(engine), readBridge: () => record(bridge), bridge: () => record(bridge),
    info: { name: "Test" }, phonesChanged: () => {}, hold: null, json,
    text: (status: number, words: string): AnswerPiece[] => [{ head: { status, headers: {} } }, { body: Buffer.from(words) }],
  } as unknown as RouteContext
  let status = 0
  const parts: Buffer[] = []
  for await (const piece of answer(context)) {
    if ("head" in piece) status = piece.head.status
    else parts.push(piece.body)
  }
  const text = Buffer.concat(parts).toString("utf8")
  return { status, json: text ? JSON.parse(text) : null }
}

const send = (to: string, body: unknown) => {
  const route = `kuvo/sessions/${to}/send`
  return ask(context => mod.sendRoute(context, route.match(mod.SEND_ROUTE)!), "POST", route, body)
}
const queueAsk = (method: string, route: string, body: unknown) => ask(context => mod.queueRoute(context, route.split("?")[0]!.match(mod.QUEUE_ROUTE)!), method, route, body)
const readParts = (body: unknown) => ask(context => mod.partsRoute(context), "POST", "kuvo/messages/parts", body)

describe("a command's message shows as typed", () => {
  test("one of the engine's: the box keeps what was typed for the message it made, for every client to read by its id", async () => {
    expect((await send("ses_cmd", { words: " this branch ", command: "review" })).status).toBe(200)
    const [made] = inbox
    const { json } = await readParts({ ids: [made!.id, "msg_other"], texts: [made!.payload.text, "Hello"] })
    expect(json.parts).toEqual([{ words: "this branch", keyword: "/review", command: true }, { words: "Hello" }])
  })

  test("one a client sees before the engine has said which it was still reads as typed", async () => {
    hearing = "hold"
    const ran = whenAsked(each => each.path === "/api/session/ses_cmd/command")
    const sending = send("ses_cmd", { words: "main", command: "review" })
    // (The engine has it, and the client reads the message, before the engine's event comes.)
    await ran
    const [made] = inbox
    const reading = readParts({ ids: [made!.id], texts: [made!.payload.text] })
    held.forEach(heard => heard())
    expect((await reading).json.parts).toEqual([{ words: "main", keyword: "/review", command: true }])
    expect((await sending).status).toBe(200)
  })

  test("one waiting in the queue shows as typed too", async () => {
    await send("ses_cmd", { words: "the tests", command: "review", delivery: "queue" })
    const { json } = await queueAsk("GET", "kuvo/sessions/ses_cmd/queue?items=1", undefined)
    expect(json.items).toEqual([expect.objectContaining({ id: inbox[0]!.id, words: "the tests", keyword: "/review", from: null })])
  })

  test("a coding agent's: its parts go with it, as any message's", async () => {
    await send("ses_agent", { words: "this branch", command: "review" })
    const ran = bridgeAsked.find(each => each.path === "/api/session/ses_agent/command")!
    expect(ran.body.metadata.kuvo.parts).toEqual({ words: "this branch", keyword: "/review", command: true })
  })
})

describe("a queued message taken back to edit keeps its keyword", () => {
  test("one the engine holds", async () => {
    await send("ses_cmd", { id: "msg_plan", words: "sketch it", keyword: "/plan", delivery: "queue" })
    const { status, json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plan", undefined)
    expect(status).toBe(200)
    // (Its words alone go back: sent again, they shouldn't switch to Plan again.)
    expect(json.removed).toMatchObject({ id: "msg_plan", words: "sketch it", keyword: "/plan", draft: "sketch it" })
  })

  test("a command the engine holds: as it was typed", async () => {
    await send("ses_cmd", { words: "the tests", command: "review", delivery: "queue" })
    const { json } = await queueAsk("DELETE", `kuvo/sessions/ses_cmd/queue/${inbox[0]!.id}`, undefined)
    // (With it in front: sent again, it runs again.)
    expect(json.removed).toMatchObject({ words: "the tests", keyword: "/review", draft: "/review the tests" })
  })

  test("one waiting on the box", async () => {
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", {})
    await queueAsk("POST", "kuvo/sessions/ses_cmd/queue/add", { id: "msg_loop", words: "check CI", keyword: "/loop every hour", delivery: "queue" })
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_loop", undefined)
    expect(json.removed).toMatchObject({ id: "msg_loop", words: "check CI", keyword: "/loop every hour", draft: "check CI" })
  })

  test("a goal: with it in front", async () => {
    await send("ses_cmd", { id: "msg_goal", words: "all tests pass", keyword: "/goal", delivery: "queue" })
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_goal", undefined)
    expect(json.removed).toMatchObject({ keyword: "/goal", draft: "/goal all tests pass" })
  })

  test("a plain one says it has none", async () => {
    await send("ses_cmd", { id: "msg_plain", words: "then this", delivery: "queue" })
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plain", undefined)
    expect(json.removed).toMatchObject({ words: "then this", keyword: null, draft: "then this" })
  })
})

describe("Stop takes what's on its way with it", () => {
  test("a message the client named as on its way is held as it comes, not set going", async () => {
    expect((await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_late"] })).status).toBe(200)
    expect((await send("ses_cmd", { id: "msg_late", words: "and this too", delivery: "steer" })).status).toBe(200)
    const sent = engineAsked.find(each => each.path === "/api/session/ses_cmd/prompt")!
    expect(sent.body.delivery).toBe("queue")
    const { json } = await queueAsk("GET", "kuvo/sessions/ses_cmd/queue?items=1", undefined)
    expect(json).toMatchObject({ paused: true, held: ["msg_late"] })
    expect(json.items).toEqual([expect.objectContaining({ id: "msg_late", words: "and this too", held: true, delivery: "queue" })])
  })

  test("one the box is still sending as Stop comes reaches the engine first, and is held with the rest", async () => {
    let arrived = () => {}
    let go = () => {}
    slow = { arrived: () => arrived(), go: new Promise<void>(resolve => { go = resolve }) }
    const reached = new Promise<void>(resolve => { arrived = resolve })
    const sending = send("ses_cmd", { id: "msg_slow", words: "one more thing", delivery: "steer" })
    await reached
    // (The engine takes the send once Stop is at the box, looking for its conversation.)
    const looked = whenAsked(each => each.path === "/api/session/ses_cmd")
    const stopping = queueAsk("POST", "kuvo/sessions/ses_cmd/stop", {})
    await looked
    go()
    const [stopped] = await Promise.all([stopping, sending])
    expect(stopped.json).toMatchObject({ paused: true, held: ["msg_slow"] })
    const interrupted = engineAsked.findIndex(each => each.path === "/api/session/ses_cmd/interrupt")
    const queued = engineAsked.findIndex(each => each.method === "PATCH" && each.path === "/api/session/ses_cmd/inbox/msg_slow")
    expect(queued).toBeGreaterThan(-1)
    expect(queued).toBeLessThan(interrupted)
  })

  test("one Stop didn't name goes as you sent it", async () => {
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_other"] })
    await send("ses_cmd", { id: "msg_new", words: "carry on", delivery: "steer" })
    expect(engineAsked.find(each => each.path === "/api/session/ses_cmd/prompt")!.body.delivery).toBe("steer")
  })
})
