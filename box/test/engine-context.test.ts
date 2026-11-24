import "./safety"
import { expect, test } from "bun:test"
import { engineHandler } from "../src/link/server/router"

// The box's real session route, with a stand-in for the engine's HTTP API.
test("an engine session reports its latest measured step and drops it when a summary supersedes it", async () => {
  const ref = { id: "wide", providerID: "test" }
  const last = { type: "assistant", model: ref, tokens: { input: 10, output: 8, reasoning: 2, cache: { read: 400_000, write: 500 } } }
  let compacted = false
  let measured = false
  let catalog = true
  const engine = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/session/ses_context") return Response.json({ data: { id: "ses_context", model: ref, tokens: { input: 3_000_000 } } })
      if (url.pathname === "/api/session/ses_context/message" && url.searchParams.get("order") === "desc") {
        return Response.json({ data: measured ? [compacted ? { type: "compaction", status: "completed" } : { type: "idle" }, last] : [] })
      }
      if (url.pathname === "/api/model" && catalog) return Response.json({ data: [{ ...ref, limit: { context: 1_000_000 } }] })
      return new Response(null, { status: 404 })
    },
  })
  const route = engineHandler(() => {
    const port = engine.port
    if (port === undefined) throw new Error("The test engine did not bind a port.")
    return { pid: process.pid, port, password: "test" }
  })
  const read = async () => {
    let status = 0
    const body: Buffer[] = []
    for await (const piece of route({ method: "GET", path: "api/session/ses_context", headers: {}, fromApp: true }, undefined)) {
      if ("head" in piece) status = piece.head.status
      if ("body" in piece) body.push(piece.body)
    }
    return { status, json: JSON.parse(Buffer.concat(body).toString()) }
  }
  try {
    expect((await read()).json.data).toMatchObject({ contextWindow: 1_000_000 })
    catalog = false
    measured = true
    expect(await read()).toMatchObject({
      status: 200, json: { data: { context: { used: 400_520, window: 1_000_000 }, tokens: { input: 3_000_000 } } },
    })
    compacted = true
    const { context, contextWindow } = (await read()).json.data
    expect(context).toBeUndefined()
    expect(contextWindow).toBe(1_000_000)
  } finally {
    await engine.stop(true)
  }
})
