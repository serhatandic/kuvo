// A message's envelope and its parts (PROTOCOL.md, "A message's parts"): the box writes what the agent
// reads from a client's fields, in the words Kuvo Code wrote before, byte for byte, and reads the
// parts back, from the text of one from before too (protocol/vectors/message-parts.json,
// which KuvoClient's MessagePartsTests reads as a client does). Then `kuvo/sessions/<id>/send`,
// `queue/add` and `kuvo/messages/parts`, with the engine and the agents bridge as stand-ins (their
// HTTP APIs) and the box's code real.

import "./safety"
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test"
import { readFileSync, rmSync } from "node:fs"
import { composeMessage, composeWithParts, partsOf } from "../src/link/messages/parts"
import type { AnswerPiece, RouteContext } from "../src/link/server/context"

const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/message-parts.json", import.meta.url), "utf8"))

describe("the envelope, as Kuvo Code wrote it before", () => {
  test.each(vectors.compose.map((each: any) => [each.about, each]))("%s", (_, each: any) => {
    const { text, parts } = composeWithParts(each.fields, { excluding: each.excluding ?? null, here: each.here ?? null, agents: vectors.agents }, each.kept ?? {}, each.phone === true)
    expect(text).toBe(each.text)
    // The parts kept are the fields', and read the same from the text as a message from before.
    expect(JSON.parse(JSON.stringify(parts))).toEqual(each.parts)
    expect(JSON.parse(JSON.stringify(partsOf(text)))).toEqual(each.parts)
  })
})

describe("the parts of a message from before", () => {
  test.each(vectors.read.map((each: any) => [each.about, each]))("%s", (_, each: any) => {
    expect(JSON.parse(JSON.stringify(partsOf(each.text)))).toEqual(each.parts)
  })
})

// ——— The routes ———

type Asked = { method: string; path: string; body: any }

let engine: ReturnType<typeof Bun.serve>
let bridge: ReturnType<typeof Bun.serve>
let engineAsked: Asked[] = []
let bridgeAsked: Asked[] = []
let engineAnswers = 200
let sendRoute: typeof import("../src/link/areas/messages").sendRoute
let partsRoute: typeof import("../src/link/areas/messages").partsRoute
let SEND_ROUTE: RegExp
let queueRoute: typeof import("../src/link/areas/queue").queueRoute
let QUEUE_ROUTE: RegExp

async function asked(request: Request): Promise<Asked> {
  const text = await request.text()
  return { method: request.method, path: new URL(request.url).pathname, body: text ? JSON.parse(text) : null }
}

beforeAll(async () => {
  ;({ sendRoute, partsRoute, SEND_ROUTE } = await import("../src/link/areas/messages"))
  ;({ queueRoute, QUEUE_ROUTE } = await import("../src/link/areas/queue"))
  engine = Bun.serve({
    port: 0,
    async fetch(request) {
      const each = await asked(request)
      engineAsked.push(each)
      if (each.path === "/api/session/ses_main") return Response.json({ data: { id: "ses_main", location: { directory: "/srv/app" }, time: { created: 1, updated: 1 } } })
      // (On a model that reads only words.)
      if (each.path === "/api/session/ses_text") return Response.json({ data: { id: "ses_text", location: { directory: "/srv/app" }, time: { created: 1, updated: 1 }, model: { id: "words", providerID: "p", capabilities: { input: ["text"] } } } })
      if (/^\/api\/session\/ses_text\//.test(each.path)) return Response.json({ data: {} })
      if (each.path === "/api/session/ses_main/inbox") return Response.json({ data: [] })
      if (/^\/api\/session\/ses_main\//.test(each.path)) return Response.json({ data: {} }, { status: engineAnswers })
      if (/^\/api\/session\//.test(each.path)) return Response.json({ message: "Not here." }, { status: 404 })
      return Response.json({ data: null })
    },
  })
  bridge = Bun.serve({
    port: 0,
    async fetch(request) {
      const each = await asked(request)
      bridgeAsked.push(each)
      if (each.path === "/api/session/ses_claude") return Response.json({ data: { id: "ses_claude", location: { directory: "/srv/app" }, time: { created: 1, updated: 1 } } })
      if (/^\/api\/session\/ses_claude\//.test(each.path)) return Response.json({ data: {} })
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
})

afterAll(() => {
  engine.stop(true)
  bridge.stop(true)
})

beforeEach(() => {
  engineAsked = []
  bridgeAsked = []
  engineAnswers = 200
})

/** A route's answer to `method route` with `body`, asked by `from` (a phone, unless the app or a Mac). */
async function ask(answer: (context: RouteContext) => AsyncIterable<AnswerPiece>, method: string, route: string, body: unknown, from: "phone" | "app" | "mac" = "app"): Promise<{ status: number; json: any }> {
  const json = (status: number, value: unknown): AnswerPiece[] => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }]
  const record = (server: ReturnType<typeof Bun.serve>) => ({ pid: process.pid, port: server.port!, password: "pw", version: "test" })
  const context = {
    request: { method, path: route, headers: {}, body: Buffer.from(JSON.stringify(body)), fromApp: from === "app", fromMac: from === "mac" },
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

const send = (session: string, body: unknown, from: "phone" | "app" | "mac" = "app") => {
  const route = `kuvo/sessions/${session}/send`
  return ask(context => sendRoute(context, route.match(SEND_ROUTE)!), "POST", route, body, from)
}
const prompted = (asked: Asked[]) => asked.filter(each => each.method === "POST" && /\/prompt$/.test(each.path))

describe("kuvo/sessions/<id>/send", () => {
  test("the box writes what the agent reads and keeps its parts with it; a keyword an older client sends is left out", async () => {
    const comments = [{ file: "a.js", line: 4, side: "new", snippet: "x()", text: "Call it once" }]
    const { status } = await send("ses_main", { id: "msg_one", words: " Have a look ", keyword: "/plan", comments, notes: { paths: ["/x/y.txt"] }, delivery: "steer" })
    expect(status).toBe(200)
    const [sent] = prompted(engineAsked)
    expect(sent.path).toBe("/api/session/ses_main/prompt")
    expect(sent.body.id).toBe("msg_one")
    expect(sent.body.delivery).toBe("steer")
    expect(sent.body.text).toBe(composeMessage({ words: "Have a look", comments: comments as any, notes: { paths: ["/x/y.txt"] } }, { excluding: "ses_main", here: null, agents: [] }))
    expect(sent.body.text.startsWith("Have a look\n\n<review-comments>\n")).toBe(true)
    expect(sent.body.metadata.kuvo.parts).toEqual({ words: "Have a look", comments, paths: ["/x/y.txt"] })
  })

  test("a reply to two quotes keeps each answer under its quote, as written, for the agent and kept", async () => {
    // As every client writes it (KuvoClient's `Quoting.message`): each quote, its answer under it.
    const words = "> Keep what was typed\n\nYes, keep it\n\n> Show the error under the field\n\nok\n\n---\n\nThen run the tests"
    await send("ses_main", { id: "msg_quotes", words }, "mac")
    const [sent] = prompted(engineAsked)
    expect(sent.body.text).toBe(words)
    expect(partsOf(sent.body.text).words).toBe(words)
    expect(sent.body.metadata.kuvo.parts).toEqual({ words })
  })

  test("a phone's says it's from the phone; a Mac's doesn't", async () => {
    await send("ses_main", { id: "msg_phone", words: "What's in the report?" }, "phone")
    await send("ses_main", { id: "msg_mac", words: "What's in the report?" }, "mac")
    const [phone, mac] = prompted(engineAsked)
    expect(phone.body.text).toEndWith("</from-phone>")
    expect(phone.body.metadata.kuvo.parts).toEqual({ words: "What's in the report?", phone: true })
    expect(mac.body.text).toBe("What's in the report?")
  })

  test("a file it carries is kept here, and the note says where", async () => {
    const zip = `data:application/zip;base64,${Buffer.from("PK").toString("base64")}`
    await send("ses_main", { id: "msg_files", words: "Here", files: [{ uri: zip, name: "b.zip" }], notes: { unsent: ["/Users/me/huge.mov"] } }, "mac")
    const [sent] = prompted(engineAsked)
    expect(sent.body.files).toBeUndefined()
    const note = sent.body.text.split("\n")
    expect(note.slice(0, 5)).toEqual(["Here", "", "<attached-files>", "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):", "/Users/me/huge.mov"])
    expect(note[5]).toBe("Open these attached files with your tools if you need them:")
    expect(note[6]).toMatch(/\/attachments\/ses_main\/[0-9a-f]{8}\/b\.zip$/)
    expect(sent.body.metadata.kuvo.parts.paths).toEqual(["/Users/me/huge.mov", note[6]])
    rmSync(note[6], { force: true })
  })

  test("sent again under its id, it goes once", async () => {
    const body = { id: "msg_twice", words: "Only once" }
    expect((await send("ses_main", body)).status).toBe(200)
    expect((await send("ses_main", body)).status).toBe(200)
    expect(prompted(engineAsked).length).toBe(1)
  })

  test("a refusal goes back as the engine said it, and the same send then goes again", async () => {
    engineAnswers = 500
    expect((await send("ses_main", { id: "msg_again", words: "Try" })).status).toBe(500)
    engineAnswers = 200
    expect((await send("ses_main", { id: "msg_again", words: "Try" })).status).toBe(200)
    expect(prompted(engineAsked).length).toBe(2)
  })

  test("a coding agent's conversation gets it through their bridge", async () => {
    await send("ses_claude", { id: "msg_claude", words: "Hello" })
    expect(prompted(engineAsked)).toEqual([])
    expect(prompted(bridgeAsked)[0].body.metadata.kuvo.parts).toEqual({ words: "Hello" })
  })

  test("one sent again (Retry) goes as the agent read it, its parts kept", async () => {
    const text = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nAsk again"
    await send("ses_main", { id: "msg_retry", text, files: [{ uri: "file:///x/a.png", name: "a.png" }] })
    const [sent] = prompted(engineAsked)
    expect(sent.body.text).toBe(text)
    expect(sent.body.files).toEqual([{ uri: "file:///x/a.png", name: "a.png" }])
    expect(sent.body.metadata.kuvo.parts).toEqual({ words: "Ask again", mail: { name: "Ada", session: "ses_ada" } })
  })

  test("words starting with \"/\" go as a message: a command an older client names isn't run", async () => {
    expect((await send("ses_main", { id: "msg_slash", words: "/review-quick main", command: "review-quick" })).status).toBe(200)
    expect(engineAsked.some(each => each.path.endsWith("/command"))).toBe(false)
    const [sent] = prompted(engineAsked)
    expect(sent.body).toMatchObject({ id: "msg_slash", text: "/review-quick main", metadata: { kuvo: { parts: { words: "/review-quick main" } } } })
  })

  test("one sent again (Retry) after switching to a model that reads only words sends its picture as a path", async () => {
    const png = `data:image/png;base64,${Buffer.from("png").toString("base64")}`
    expect((await send("ses_text", { id: "msg_againpic", text: "What's this?", files: [{ uri: png, name: "a.png" }] })).status).toBe(200)
    const [sent] = prompted(engineAsked)
    expect(sent.body.files).toBeUndefined()
    const lines = sent.body.text.split("\n")
    expect(lines.slice(0, 4)).toEqual(["What's this?", "", "<attached-files>", "Open these attached files with your tools if you need them:"])
    expect(lines[4]).toMatch(/\/attachments\/ses_text\/[0-9a-f]{8}\/a\.png$/)
    expect(sent.body.metadata.kuvo.parts).toEqual({ words: "What's this?", paths: [lines[4]] })
    rmSync(lines[4], { force: true })
  })

  test("words that only start like another agent's message stay yours", async () => {
    const words = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nhi"
    await send("ses_main", { id: "msg_lookalike", words })
    expect(prompted(engineAsked)[0].body.metadata.kuvo.parts).toEqual({ words })
  })

  test("an agent picked from the \"&\" list that this computer doesn't list yet is said, not dropped", async () => {
    const { status, json } = await send("ses_main", { id: "msg_unlisted", words: "Ask &Nova", named: ["ses_nova"] })
    expect(status).toBe(409)
    expect(json.message).toContain("isn't known to this computer yet")
    expect(prompted(engineAsked)).toEqual([])
  })

  test("nothing written and nothing with it isn't sent; nor is it to a conversation that isn't here", async () => {
    expect((await send("ses_main", { words: "  " })).status).toBe(400)
    expect((await send("ses_main", { text: " " })).status).toBe(400)
    expect((await send("ses_gone", { words: "Hi" })).status).toBe(404)
    expect(prompted(engineAsked)).toEqual([])
  })
})

describe("queue/add", () => {
  test("a message as fields is written by the box, its parts kept, and goes on while nothing's paused", async () => {
    const route = "kuvo/sessions/ses_main/queue/add"
    const { status, json } = await ask(context => queueRoute(context, route.match(QUEUE_ROUTE)!), "POST", route, { id: "msg_queued", words: "After this", widget: "Plans", delivery: "queue" })
    expect(status).toBe(200)
    expect(json.waits).toBe(false)
    const [sent] = prompted(engineAsked)
    expect(sent.body.text).toBe("After this\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nPlans\n</from-widget>")
    expect(sent.body.metadata.kuvo.parts).toEqual({ words: "After this", widget: "Plans" })
  })


})

describe("queue/add's other forms", () => {
  const add = (body: unknown) => {
    const route = "kuvo/sessions/ses_main/queue/add"
    return ask(context => queueRoute(context, route.match(QUEUE_ROUTE)!), "POST", route, body)
  }

  test("one sent again as the agent read it goes as it was, its parts read from it", async () => {
    const letter = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nThe build is green."
    await add({ id: "msg_letter", text: letter, again: true, words: "The build is green.", delivery: "queue" })
    const [sent] = prompted(engineAsked)
    expect(sent.body.text).toBe(letter)
    expect(sent.body.metadata.kuvo.parts).toEqual({ words: "The build is green.", mail: { name: "Ada", session: "ses_ada" } })
  })

  test("one put back as it was kept keeps the parts it had", async () => {
    await add({ id: "msg_kept", text: "check CI", kept: true, words: "check CI", metadata: { kuvo: { parts: { words: "check CI", widget: "Build" } } } })
    expect(prompted(engineAsked)[0].body.metadata.kuvo.parts).toEqual({ words: "check CI", widget: "Build" })
  })
})

describe("kuvo/messages/parts", () => {
  test("reads each text from before, in order", async () => {
    const texts = vectors.read.map((each: any) => each.text)
    const { status, json } = await ask(context => partsRoute(context), "POST", "kuvo/messages/parts", { texts })
    expect(status).toBe(200)
    expect(json.parts).toEqual(vectors.read.map((each: any) => each.parts))
  })

  test("takes texts only, and not too many at once", async () => {
    expect((await ask(context => partsRoute(context), "POST", "kuvo/messages/parts", { texts: [1] })).status).toBe(400)
    expect((await ask(context => partsRoute(context), "POST", "kuvo/messages/parts", { texts: Array(501).fill("a") })).status).toBe(400)
  })
})
