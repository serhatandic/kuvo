import "./safety"
import { afterAll, expect, test } from "bun:test"

// A coding agent's subagent, opened as a conversation of its own (#950): its composer says the
// approval it really runs with, its conversation's (it asks there), and the model its agent ran it
// on, which no one picks; neither can be changed from it.

// (The router first: the link's modules alone load in an order that doesn't start.)
await import("../src/link/server/router")
const { approvalRoute } = await import("../src/link/areas/approval")
const { sessionModelsRoute } = await import("../src/link/areas/session-models")

const parent = { id: "ses_lead", directory: "/w", location: { directory: "/w" }, time: { created: 1, updated: 1 }, model: { providerID: "claude", id: "opus" } }
const explore = {
  id: "ses_sub1", parentID: "ses_lead", location: { directory: "/w" }, time: { created: Date.now(), updated: Date.now() }, model: { providerID: "claude", id: "opus" },
  metadata: { kuvoSubagent: true, kuvo: { messages: { via: "none", note: "Claude Code's Explore agents can't take messages." }, ranOn: "claude-haiku-4-5-20251001" } },
}
const fresh = { ...explore, id: "ses_sub2", metadata: { kuvoSubagent: true, kuvo: { messages: { via: "parent" } } } }
const sessions = new Map<string, any>([parent, explore, fresh].map(session => [session.id, session]))

const serve = (known: Map<string, any>) => Bun.serve({
  port: 0,
  fetch(request) {
    const url = new URL(request.url)
    const one = url.pathname.match(/^\/api\/session\/([^/]+)$/)
    if (one && known.has(one[1]!)) return Response.json({ data: structuredClone(known.get(one[1]!)) })
    if (url.pathname === "/api/session") return Response.json({ data: [...known.values()], cursor: { next: null } })
    return Response.json({ message: "Not here." }, { status: 404 })
  },
})
// The engine has none of them: they're the coding agents' bridge's.
const engine = serve(new Map())
const bridge = serve(sessions)
afterAll(() => { engine.stop(true); bridge.stop(true) })

// The routes as the box answers them, with the bridge running (its process checked by the dispatcher).
const target = (server: { port: number }) => ({ port: server.port, password: "pw" }) as any
async function ask(method: string, path: string, body?: unknown): Promise<{ status: number; body: any }> {
  let status = 0
  let text = ""
  const route = path.split("?")[0]!
  const json = async function* (code: number, value: unknown) { yield { head: { status: code, headers: {} } }; yield { body: Buffer.from(JSON.stringify(value)) } }
  const context = {
    request: { method, path, headers: {}, fromApp: true, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) },
    route, signal: new AbortController().signal, readEngine: () => target(engine), bridge: () => target(bridge), json,
  } as any
  const answer = route.endsWith("/approval") ? approvalRoute(context) : sessionModelsRoute(context)
  for await (const piece of answer as AsyncGenerator<any>) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
  }
  return { status, body: text ? JSON.parse(text) : null }
}

test("a subagent says its conversation's approval, and is changed there", async () => {
  const lead = await ask("GET", "kuvo/sessions/ses_lead/approval")
  expect(lead.status).toBe(200)
  expect(lead.body.approval).toBe("fullAccess")
  const sub = await ask("GET", "kuvo/sessions/ses_sub1/approval")
  expect(sub.status).toBe(200)
  // Not the Ask First it was guessed to be: its conversation's, which it asks in.
  expect(sub.body).toEqual({ ...lead.body, parent: "ses_lead" })
  const set = await ask("PUT", "kuvo/sessions/ses_sub1/approval", { approval: "askFirst" })
  expect(set.status).toBe(400)
  expect(set.body.message).toContain("conversation that started it")
})

test("a subagent names the model its agent ran it on, and offers no choice", async () => {
  const { status, body } = await ask("GET", "kuvo/sessions/models?sessions=ses_sub1,ses_sub2")
  expect(status).toBe(200)
  expect(body.sessions.ses_sub1).toEqual({ model: { providerID: "claude", id: "claude-haiku-4-5-20251001" }, name: "Haiku 4.5", runs: true, fixed: true })
  // Before its agent has said: the one it started on, its conversation's.
  expect(body.sessions.ses_sub2).toMatchObject({ model: { providerID: "claude", id: "opus" }, runs: true, fixed: true })
  expect(body.sessions.ses_sub2.name).toBeTruthy()
})
