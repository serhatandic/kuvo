// An older message that ran a command still shows as it was typed, a queued message taken back to
// edit says its keyword and puts back its words alone, and Stop takes the messages still on their
// way with it (PROTOCOL.md, "A message's parts" and "The queue"): `kuvo/sessions/<id>/send`, `kuvo/messages/parts`, the queue's items and DELETE, and
// `stop`, with the engine and the agents bridge as stand-ins (their HTTP APIs, and the engine's
// inbox events as the box's watcher hands them on) and the box's code real.

import "./safety"
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import type { AnswerPiece, RouteContext } from "../src/link/server/context"

type Asked = { method: string; path: string; body: any }
type Item = { id: string; type: "user"; delivery: "steer" | "queue"; payload: { text: string; metadata?: unknown } }

let engine: ReturnType<typeof Bun.serve>
let bridge: ReturnType<typeof Bun.serve>
let engineAsked: Asked[] = []
let bridgeAsked: Asked[] = []
let inbox: Item[] = []
/** A prompt the engine takes only once this is let go (a send still on its way). */
let slow: { arrived: () => void; go: Promise<void> } | null = null
/** The conversation looked up while a send keeps its files, held until this is let go. */
let lookHeld: { at: number; reached: () => void; go: Promise<void> } | null = null
let looks = 0
/** Requests a test waits for, each told once one like it comes. */
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
}

const session = (id: string) => Response.json({ data: { id, location: { directory: "/srv/app" }, time: { created: 1, updated: 1 } } })

/** A message that ran the engine's /review before Kuvo stopped running commands, as its box kept it. */
const RAN = { id: "msg_ran", text: "Review the code: this branch", parts: { words: "this branch", keyword: "/review", command: true } }

beforeAll(async () => {
  const messages = await import("../src/link/areas/messages")
  const queue = await import("../src/link/areas/queue")
  // (Kept as a box that ran commands before left it, before anything reads it.)
  const { RUN } = await import("../src/link/home")
  mkdirSync(RUN, { recursive: true })
  writeFileSync(join(RUN, "commands.json"), JSON.stringify({ [RAN.id]: RAN.parts }))
  mod = { sendRoute: messages.sendRoute, partsRoute: messages.partsRoute, SEND_ROUTE: messages.SEND_ROUTE, queueRoute: queue.queueRoute, QUEUE_ROUTE: queue.QUEUE_ROUTE }
  engine = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const url = new URL(request.url)
      const text = await request.text()
      const each = { method: request.method, path: url.pathname, body: text ? JSON.parse(text) : null }
      engineAsked.push(each)
      tell(each)
      if (each.path === "/api/session/ses_cmd") {
        looks++
        if (lookHeld && looks === lookHeld.at) {
          lookHeld.reached()
          await lookHeld.go
        }
        return session("ses_cmd")
      }
      if (each.path === "/api/session/active") return Response.json({ data: [] })
      if (each.path === "/api/session/ses_cmd/inbox") return Response.json({ data: inbox })
      if (each.path === "/api/session/ses_cmd/message") return Response.json({ data: [] })
      // This stand-in leaves accepted prompts in its inbox; none is in the transcript yet.
      if (each.method === "GET" && each.path.startsWith("/api/session/ses_cmd/message/")) {
        return Response.json({ message: "Not here." }, { status: 404 })
      }
      const item = each.path.match(/^\/api\/session\/ses_cmd\/inbox\/(.+)$/)?.[1]
      if (item && each.method === "PATCH") {
        for (const entry of inbox) if (entry.id === item) entry.delivery = each.body.delivery
        return Response.json({})
      }
      if (item && each.method === "DELETE") {
        inbox = inbox.filter(entry => entry.id !== item)
        return Response.json({})
      }
      if (each.path === "/api/session/ses_cmd/prompt") {
        if (slow) {
          slow.arrived()
          await slow.go
        }
        inbox.push({ id: each.body.id, type: "user", delivery: each.body.delivery, payload: { text: each.body.text, metadata: each.body.metadata } })
        return Response.json({ data: {} })
      }
      if (/^\/api\/session\/ses_cmd\//.test(each.path)) return Response.json({})
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
  bridge = Bun.serve({
    hostname: "127.0.0.1",
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
  slow = null
  waits = []
  lookHeld = null
  looks = 0
  // (Each starts with nothing held, nothing paused, nothing another test left waiting.)
  const { forgetQueue } = await import("../src/link/engine/queue")
  forgetQueue("ses_cmd")
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

/** A message from before in the engine's inbox, waiting: `parts` as its box kept them. */
const older = (id: string, text: string, parts?: unknown) =>
  inbox.push({ id, type: "user", delivery: "queue", payload: { text, ...(parts ? { metadata: { kuvo: { parts } } } : {}) } })

describe("an older message that ran a command still shows as typed", () => {
  test("read by its id, for every client alike", async () => {
    const { json } = await readParts({ ids: [RAN.id, "msg_other"], texts: [RAN.text, "Hello"] })
    expect(json.parts).toEqual([RAN.parts, { words: "Hello" }])
  })

  test("waiting in the queue, and taken back to edit: its words alone go back", async () => {
    older(RAN.id, RAN.text)
    const { json } = await queueAsk("GET", "kuvo/sessions/ses_cmd/queue?items=1", undefined)
    expect(json.items).toEqual([expect.objectContaining({ id: RAN.id, words: "this branch", keyword: "/review", from: null })])
    const taken = await queueAsk("DELETE", `kuvo/sessions/ses_cmd/queue/${RAN.id}`, undefined)
    // (Typed again, "/review this branch" would only be words to the agent.)
    expect(taken.json.removed).toMatchObject({ words: "this branch", keyword: "/review", draft: "this branch" })
  })

  test("words starting with \"/\" sent now are a message like any other", async () => {
    expect((await send("ses_cmd", { id: "msg_slash", words: "/review this branch", delivery: "queue" })).status).toBe(200)
    expect(engineAsked.some(each => each.path.endsWith("/command"))).toBe(false)
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_slash", undefined)
    expect(json.removed).toMatchObject({ words: "/review this branch", keyword: null, draft: "/review this branch" })
  })
})

describe("a queued message taken back to edit says its keyword, and puts back its words alone", () => {
  test("one the engine holds, sent with a keyword (a bot's /goal, an older /plan)", async () => {
    older("msg_goal", "all tests pass", { words: "all tests pass", keyword: "/goal" })
    older("msg_plan", "sketch it", { words: "sketch it", keyword: "/plan" })
    expect((await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_goal", undefined)).json.removed).toMatchObject({ id: "msg_goal", keyword: "/goal", draft: "all tests pass" })
    expect((await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plan", undefined)).json.removed).toMatchObject({ id: "msg_plan", keyword: "/plan", draft: "sketch it" })
  })

  test("one waiting on the box", async () => {
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", {})
    await queueAsk("POST", "kuvo/sessions/ses_cmd/queue/add", { id: "msg_loop", text: "check CI\n\n<sent-with>…</sent-with>", kept: true, words: "check CI", metadata: { kuvo: { parts: { words: "check CI", keyword: "/loop every hour" } } }, delivery: "queue" })
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_loop", undefined)
    expect(json.removed).toMatchObject({ id: "msg_loop", words: "check CI", keyword: "/loop every hour", draft: "check CI" })
  })

  test("a plain one says it has none", async () => {
    await send("ses_cmd", { id: "msg_plain", words: "then this", delivery: "queue" })
    const { json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plain", undefined)
    expect(json.removed).toMatchObject({ words: "then this", keyword: null, draft: "then this" })
  })
})

describe("Stop takes what's on its way with it", () => {
  test("a message the client named as on its way is held as it comes, on the box: never set going", async () => {
    expect((await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_late", "msg_later"] })).status).toBe(200)
    expect((await send("ses_cmd", { id: "msg_late", words: "and this too", delivery: "steer" })).status).toBe(200)
    expect((await send("ses_cmd", { id: "msg_later", words: "and that", delivery: "steer" })).status).toBe(200)
    // (Nothing reaches the engine, which would run all it holds with the first thing to reach it.)
    expect(engineAsked.filter(each => each.path === "/api/session/ses_cmd/prompt")).toEqual([])
    const { json } = await queueAsk("GET", "kuvo/sessions/ses_cmd/queue?items=1", undefined)
    expect(json).toMatchObject({ paused: true, held: ["msg_late", "msg_later"] })
    expect(json.items).toEqual([
      expect.objectContaining({ id: "msg_late", words: "and this too", held: true, on: "box" }),
      expect.objectContaining({ id: "msg_later", words: "and that", held: true, on: "box" }),
    ])
    // Resume sends them, in that order.
    await queueAsk("POST", "kuvo/sessions/ses_cmd/queue/resume", {})
    expect(engineAsked.filter(each => each.path === "/api/session/ses_cmd/prompt").map(each => each.body.id)[0]).toBe("msg_late")
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

  test("a send slower than Stop's wait, named by it, still waits on the box as it comes", async () => {
    const arriving = await import("../src/link/messages/arriving")
    arriving.stopWaits.within = 0
    try {
      // (Its picture kept first: the conversation looked up again meanwhile, and held there.)
      let go = () => {}
      let reached = () => {}
      const looking = new Promise<void>(resolve => { reached = resolve })
      looks = 0
      // (The first look finds the conversation; the second, while its picture is kept, is held.)
      lookHeld = { at: 2, reached, go: new Promise<void>(resolve => { go = resolve }) }
      const picture = `data:image/png;base64,${Buffer.from("PNG").toString("base64")}`
      const sending = send("ses_cmd", { id: "msg_slow2", words: "like this", files: [{ uri: picture, name: "a.png" }], delivery: "steer" })
      await looking
      expect((await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_slow2"] })).json).toMatchObject({ paused: true })
      go()
      expect((await sending).json).toEqual({ waits: true })
      expect(engineAsked.filter(each => each.path === "/api/session/ses_cmd/prompt")).toEqual([])
      expect((await queueAsk("GET", "kuvo/sessions/ses_cmd/queue", undefined)).json.held).toEqual(["msg_slow2"])
    } finally {
      arriving.stopWaits.within = 10_000
    }
  })

  test("what Stop named is kept with the queue, for a box that starts again meanwhile", async () => {
    const { readQueues } = await import("../src/link/engine/queue")
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_kept_named"] })
    expect(Object.keys(readQueues().ses_cmd?.arriving ?? {})).toEqual(["msg_kept_named"])
    // (Never in what a client reads.)
    expect((await queueAsk("GET", "kuvo/sessions/ses_cmd/queue", undefined)).json.arriving).toBeUndefined()
  })

  test("one Stop named that comes after Resume goes as you sent it", async () => {
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_after"] })
    await queueAsk("POST", "kuvo/sessions/ses_cmd/queue/resume", {})
    await send("ses_cmd", { id: "msg_after", words: "go on", delivery: "steer" })
    expect(engineAsked.find(each => each.path === "/api/session/ses_cmd/prompt")!.body.delivery).toBe("steer")
    expect((await queueAsk("GET", "kuvo/sessions/ses_cmd/queue", undefined)).json).toMatchObject({ paused: false, held: [] })
  })

  test("one Stop didn't name goes as you sent it", async () => {
    await queueAsk("POST", "kuvo/sessions/ses_cmd/stop", { items: ["msg_other"] })
    await send("ses_cmd", { id: "msg_new", words: "carry on", delivery: "steer" })
    expect(engineAsked.find(each => each.path === "/api/session/ses_cmd/prompt")!.body.delivery).toBe("steer")
  })
})
