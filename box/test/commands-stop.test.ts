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
/** The commands of yours the engine's settings have (`api/config`), by name; none: it can't say. */
let commandsSet: Record<string, string> | null = null
/** The conversation looked up while a send keeps its files, held until this is let go. */
let lookHeld: { at: number; reached: () => void; go: Promise<void> } | null = null
let looks = 0
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
        const event = { type: "session.inbox.enqueued", created: Date.now(), data: { sessionID: "ses_cmd", inboxID: item.id, item: { type: "user", delivery: item.delivery, payload: item.payload } } }
        return () => mod.heardCommand(event)
      }
      if (each.path === "/api/session/ses_cmd") {
        looks++
        if (lookHeld && looks === lookHeld.at) {
          lookHeld.reached()
          await lookHeld.go
        }
        return session("ses_cmd")
      }
      if (each.path === "/api/config") return commandsSet ? Response.json([{ info: { commands: Object.fromEntries(Object.entries(commandsSet).map(([name, template]) => [name, { template }])) } }]) : Response.json({ message: "Not now." }, { status: 503 })
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
        // (As one of yours with "Review the code: $ARGUMENTS" says, or the engine's own.)
        const heard = enqueue({ id: `msg_made${++made}`, type: "user", delivery: each.body.delivery ?? "steer", payload: { text: `Review the code: ${each.body.text}`.trim() } })
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
  commandsSet = null
  lookHeld = null
  looks = 0
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
    const reading = readParts({ ids: [made!.id], texts: [made!.payload.text], session: "ses_cmd" })
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

describe("what the engine makes of one of your commands", () => {
  // The engine's own filling-in (2.2.5's), case by case.
  test.each([
    ["$ARGUMENTS takes it all, as written", "Review $ARGUMENTS now", " the branch ", "Review  the branch  now"],
    ["none named: on a line after it", "Tidy up the imports.", "src/app.ts", "Tidy up the imports.\nsrc/app.ts"],
    ["none named and nothing written: as it is", "Tidy up the imports.\n", "", "Tidy up the imports."],
    ["$1 $2: a word each, the highest the rest", "Fix $1 in $2", "login page header", "Fix login in page header"],
    ["quoted words keep together, without their quotes", "Fix $1 in $2", "\"login page\" 'the header'", "Fix login page in the header"],
    ["a word not given is left out", "Fix $1 in $2", "login", "Fix login in"],
    ["a picture counts as a word", "Look at $1 then $2", "[Image 1] closely", "Look at [Image 1] then closely"],
  ])("%s", async (_, template, text, made) => {
    const { filledIn } = await import("../src/link/messages/commands")
    expect(filledIn(template, text)).toBe(made)
  })
})

describe("a queued message taken back to edit keeps its keyword", () => {
  test("one the engine holds", async () => {
    await send("ses_cmd", { id: "msg_plan", words: "sketch it", keyword: "/plan", delivery: "queue" })
    const { status, json } = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plan", undefined)
    expect(status).toBe(200)
    // (With it in front: Edit puts back what you sent, keyword and all.)
    expect(json.removed).toMatchObject({ id: "msg_plan", words: "sketch it", keyword: "/plan", draft: "/plan sketch it" })
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
    // (A /loop's words alone: its wake-up was set as it was sent, and "/loop …" again would set another.)
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

  test("a wake-up landing while a command is on its way isn't taken for the command's message", async () => {
    hearing = "hold"
    const ran = whenAsked(each => each.path === "/api/session/ses_cmd/command")
    const sending = send("ses_cmd", { words: "main", command: "review" })
    await ran
    mod.heardCommand({ type: "session.inbox.enqueued", created: Date.now(), data: { sessionID: "ses_cmd", inboxID: "msg_wake", item: { type: "user", payload: { text: "[Wake-up] Check the deploy main" } } } })
    held.forEach(heard => heard())
    await sending
    const made = inbox.find(each => each.id.startsWith("msg_made"))!
    const { json } = await readParts({ ids: ["msg_wake", made.id], texts: ["[Wake-up] Check the deploy main", made.payload.text] })
    expect(json.parts).toEqual([{ words: "[Wake-up] Check the deploy main" }, { words: "main", keyword: "/review", command: true }])
  })

  test("a plain message landing during a command with nothing after it is never taken for it, and Edit keeps its words", async () => {
    hearing = "hold"
    const ran = whenAsked(each => each.path === "/api/session/ses_cmd/command")
    const sending = send("ses_cmd", { words: "", command: "review" })
    await ran
    // (Sent straight to the engine, with no parts of the box's: a terminal's, a client's from before.)
    inbox.unshift({ id: "msg_plain", type: "user", delivery: "queue", payload: { text: "Use the staging database" } })
    mod.heardCommand({ type: "session.inbox.enqueued", created: Date.now(), data: { sessionID: "ses_cmd", inboxID: "msg_plain", item: { type: "user", payload: { text: "Use the staging database" } } } })
    held.forEach(heard => heard())
    await sending
    const { json } = await readParts({ ids: ["msg_plain"], texts: ["Use the staging database"] })
    expect(json.parts).toEqual([{ words: "Use the staging database" }])
    const taken = await queueAsk("DELETE", "kuvo/sessions/ses_cmd/queue/msg_plain", undefined)
    expect(taken.json.removed).toMatchObject({ words: "Use the staging database", keyword: null, draft: "Use the staging database" })
  })

  test("one of yours the box can work out is matched by the very prompt it makes, with nothing after it too", async () => {
    commandsSet = { review: "Review the code: $ARGUMENTS" }
    hearing = "hold"
    const ran = whenAsked(each => each.path === "/api/session/ses_cmd/command")
    const sending = send("ses_cmd", { words: "", command: "review" })
    await ran
    // (A message with the same words, but not the prompt: not it.)
    mod.heardCommand({ type: "session.inbox.enqueued", created: Date.now(), data: { sessionID: "ses_cmd", inboxID: "msg_other", item: { type: "user", payload: { text: "Review the code: later" } } } })
    held.forEach(heard => heard())
    await sending
    const made = inbox.find(each => each.id.startsWith("msg_made"))!
    const { json } = await readParts({ ids: ["msg_other", made.id], texts: ["Review the code: later", made.payload.text], session: "ses_cmd" })
    expect(json.parts).toEqual([{ words: "Review the code: later" }, { words: "", keyword: "/review", command: true }])
  })

  test("one the engine made after it answered isn't the command's", async () => {
    const ran = whenAsked(each => each.path === "/api/session/ses_cmd/command")
    hearing = "hold"
    await send("ses_cmd", { words: "main", command: "review" })
    await ran
    mod.heardCommand({ type: "session.inbox.enqueued", created: Date.now() + 1000, data: { sessionID: "ses_cmd", inboxID: "msg_later", item: { type: "user", payload: { text: "Review main again" } } } })
    const { json } = await readParts({ ids: ["msg_later"], texts: ["Review main again"] })
    expect(json.parts).toEqual([{ words: "Review main again" }])
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
