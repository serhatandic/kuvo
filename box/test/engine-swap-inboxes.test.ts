import "./safety"
import { expect, test } from "bun:test"
import { keepInboxes } from "../src/link/updates/engine-swap"
import { moveInboxToBox, queueOf, releaseWaiting, sendInto } from "../src/link/engine/queue"

test("a restart keeps pending messages beyond the first page and defers on incomplete inventory", async () => {
  const first = Array.from({ length: 100 }, (_, n) => ({ id: `ses_scan_${n}`, time: { updated: 1 } }))
  const older = "ses_scan_101"
  let complete = false
  let pending = true
  let lostReply = true
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/session") {
        if (url.searchParams.has("archived")) return Response.json({ data: [] })
        if (!url.searchParams.has("cursor")) return Response.json({ data: first, cursor: { next: "page two" } })
        return Response.json(complete ? { data: [{ id: older, time: { updated: 1 } }] } : {})
      }
      if (url.pathname === `/api/session/${older}/inbox/msg_restart` && request.method === "DELETE") {
        pending = false
        if (lostReply) return new Response("Reply lost", { status: 502 })
        return Response.json({})
      }
      if (url.pathname.endsWith("/inbox")) {
        const data = pending && url.pathname === `/api/session/${older}/inbox`
          ? [{ id: "msg_restart", delivery: "queue", type: "user", payload: { text: "Keep this" } }]
          : []
        return Response.json({ data })
      }
      return new Response(null, { status: 404 })
    },
  })
  try {
    const target = { port: server.port!, password: "test" }
    expect(await keepInboxes(target)).toBe(false)
    expect(pending).toBe(true)
    complete = true
    expect(await keepInboxes(target)).toBe(false)
    expect(pending).toBe(false)
    expect(queueOf(older).waiting?.map(message => message.id)).toEqual(["msg_restart"])
    pending = true
    lostReply = false
    expect(await keepInboxes(target)).toBe(true)
    expect(pending).toBe(false)
    expect(queueOf(older).waiting?.map(message => message.id)).toEqual(["msg_restart"])
  } finally {
    server.stop(true)
  }
})

test("an interrupted inbox transfer delivers the messages in their original order", async () => {
  const session = "ses_retry_order"
  const inbox = ["A", "B", "C"].map(text => ({ id: `msg_${text}`, delivery: "queue", type: "user", payload: { text } }))
  const delivered: string[] = []
  const accepted = new Set<string>()
  let refused = 2
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const route = new URL(request.url).pathname
      if (route.endsWith("/inbox")) return Response.json({ data: inbox })
      if (request.method === "DELETE") {
        const id = route.split("/").at(-1)
        if (id === "msg_B" && refused > 0) {
          refused--
          return new Response(null, { status: 503 })
        }
        inbox.splice(inbox.findIndex(item => item.id === id), 1)
      }
      if (route === `/api/session/${session}/prompt`) {
        const { id, text } = await request.json() as { id: string; text: string }
        accepted.add(id)
        delivered.push(text)
      }
      const message = route.match(/\/message\/([^/]+)$/)
      if (message) {
        const id = message[1]
        return accepted.has(id) ? Response.json({ data: { info: { id } } }) : new Response(null, { status: 404 })
      }
      return Response.json({ data: {} })
    },
  })
  try {
    const target = { port: server.port!, password: "test" }
    await expect(moveInboxToBox(target, session)).rejects.toThrow("503")
    expect(inbox.map(item => item.id)).toEqual(["msg_B", "msg_C"])
    await releaseWaiting(target)
    expect(delivered).toEqual([])
    expect(await sendInto(target, session, { id: "msg_D", text: "D", delivery: "steer" })).toBe("waits")
    expect(await moveInboxToBox(target, session)).toBe(2)
    await releaseWaiting(target)
    expect(delivered).toEqual(["A", "B", "C", "D"])
  } finally {
    server.stop(true)
  }
})
