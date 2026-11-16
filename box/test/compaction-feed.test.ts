import "./safety"
import { afterAll, beforeAll, expect, test } from "bun:test"

// The engine's own live feed (`api/event`) through the link: a compaction event reaches a client
// with the box's words, as the coding agents' do, however the engine's bytes were cut on the way.

const started = { id: "evt_1", type: "session.compaction.started", created: 1, data: { sessionID: "ses_1", reason: "manual", inputID: "inp_1", note: "héllo ✓" } }
const ended = { id: "evt_2", type: "session.compaction.ended", created: 2, data: { sessionID: "ses_1", text: "The summary." } }
const failed = { id: "evt_3", type: "session.compaction.failed", created: 3, data: { sessionID: "ses_1" } }
const other = { id: "evt_4", type: "session.next.text.delta", created: 4, data: { sessionID: "ses_1", delta: "about session.compaction.started" } }
const sent = [": connected\n\n", ...[started, other, ended, failed].map(event => `data: ${JSON.stringify(event)}\n\n`), ": heartbeat\n\n"].join("")

let engine: ReturnType<typeof Bun.serve>
// Where the engine's bytes are cut, by the test.
let cuts: number[] = []

beforeAll(() => {
  engine = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    fetch(request) {
      if (new URL(request.url).pathname !== "/api/event") return new Response("no", { status: 404 })
      const bytes = Buffer.from(sent)
      const edges = [0, ...cuts, bytes.length]
      return new Response(new ReadableStream({
        start(controller) {
          for (let i = 0; i + 1 < edges.length; i++) controller.enqueue(bytes.subarray(edges[i]!, edges[i + 1]!))
          controller.close()
        },
      }), { headers: { "content-type": "text/event-stream" } })
    },
  })
})
afterAll(() => engine.stop(true))

async function feed(): Promise<{ status: number; events: any[]; text: string }> {
  const { forward } = await import("../src/link/server/forward")
  let status = 0
  const parts: Buffer[] = []
  for await (const piece of forward({ port: engine.port!, password: "p" }, "/api/event", { method: "GET", path: "/api/event", headers: {} }, undefined, () => true)) {
    if ("head" in piece && piece.head) status = piece.head.status
    if ("body" in piece && piece.body) parts.push(Buffer.from(piece.body))
  }
  const text = Buffer.concat(parts).toString("utf8")
  return { status, text, events: text.split("\n").filter(line => line.startsWith("data:")).map(line => JSON.parse(line.slice(5))) }
}

const expected = [
  { ...started, data: { ...started.data, title: "Summarizing the conversation…", workingTitle: "Summarizing the conversation…" } },
  other,
  { ...ended, data: { ...ended.data, title: "Conversation summarized" } },
  { ...failed, data: { ...failed.data, title: "Couldn’t summarize the conversation" } },
]

test("the engine's compaction events come through the link with their title", async () => {
  cuts = []
  const got = await feed()
  expect(got.status).toBe(200)
  expect(got.events).toEqual(expected)
  // What isn't a compaction event comes as the engine sent it.
  expect(got.text.startsWith(": connected\n\n")).toBe(true)
  expect(got.text.endsWith(": heartbeat\n\n")).toBe(true)
  expect(got.text).toContain(`data: ${JSON.stringify(other)}\n\n`)
})

test("an event the engine's bytes cut in two, inside a letter too, still comes whole with its title", async () => {
  const bytes = Buffer.from(sent)
  // Inside the "é" of the first event, and inside the second compaction event.
  cuts = [bytes.indexOf(Buffer.from("é")) + 1, bytes.indexOf("evt_2") + 2, bytes.indexOf("evt_3") - 20]
  const got = await feed()
  expect(got.events).toEqual(expected)
  expect(got.text.endsWith(": heartbeat\n\n")).toBe(true)
})
