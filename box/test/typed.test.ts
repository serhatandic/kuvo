// What a typed message means (PROTOCOL.md, "What a typed message means"): what's written goes to the
// agent as a message, as written ("/" and "!" too: Kuvo has no typed keywords or commands), unless
// the conversation's model can't run. `kuvo/sessions/<id>/draft` (the send path) and the legacy
// `kuvo/sessions/<id>/typed` (0.7.x clients), with the engine and the agents bridge as stand-ins
// (their HTTP APIs) and the box's code real.

import "./safety"
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test"
import { readFileSync, rmSync, writeFileSync } from "node:fs"
import type { AnswerPiece, RouteContext } from "../src/link/server/context"

type Asked = { method: string; path: string; folder: string | null; body: any }
type Conversation = { id: string; parentID?: string; agent?: string; model?: { providerID: string; id: string }; location: { directory: string }; time: { created: number; updated: number }; metadata?: any }

let engine: ReturnType<typeof Bun.serve>
let bridge: ReturnType<typeof Bun.serve>
let engineAsked: Asked[] = []
let bridgeAsked: Asked[] = []
let conversations: Record<string, Conversation> = {}
/** The coding agents' conversations, the bridge's. */
let bridgeConversations: Record<string, Conversation> = {}
let bridgeRuns = true
/** What the engine lists of its providers, models and accounts (by path), where a test says. */
let engineLists: Record<string, unknown> = {}
/** What the bridge answers what isn't a conversation: its status and body. */
let bridgeSays: (asked: Asked) => { status: number; body: unknown } = () => ({ status: 200, body: {} })
let draftRoute: typeof import("../src/link/areas/typed").draftRoute
let DRAFT_ROUTE: RegExp
let typedRoute: typeof import("../src/link/areas/typed").typedRoute
let forgetModelLists: () => void
let forgetModelNames: () => void
let TYPED_SENT: string
let TYPED_ROUTE: RegExp

const asked = async (request: Request): Promise<Asked> => {
  const url = new URL(request.url)
  const text = request.method === "GET" || request.method === "DELETE" ? "" : await request.text()
  const folder = request.headers.get("x-opencode-directory")
  return { method: request.method, path: url.pathname, folder: folder ? decodeURIComponent(folder) : null, body: text ? JSON.parse(text) : null }
}

function startBridge() {
  bridge = Bun.serve({
    port: 0,
    async fetch(request) {
      const each = await asked(request)
      bridgeAsked.push(each)
      // (It has none of the engine's conversations: its own only.)
      const own = /^\/api\/session\/([^/]+)$/.exec(each.path)?.[1]
      if (own && bridgeConversations[decodeURIComponent(own)]) return Response.json({ data: bridgeConversations[decodeURIComponent(own)] })
      if (/^\/api\/session\//.test(each.path)) return Response.json({ message: "Not here." }, { status: 404 })
      const said = bridgeSays(each)
      return Response.json(said.body, { status: said.status })
    },
  })
}

beforeAll(async () => {
  ;({ typedRoute, TYPED_ROUTE, TYPED_SENT, draftRoute, DRAFT_ROUTE } = await import("../src/link/areas/typed"))
  ;({ forgetModelLists, forgetModelNames } = await import("../src/link/work/model-rule"))
  engine = Bun.serve({
    port: 0,
    async fetch(request) {
      const each = await asked(request)
      engineAsked.push(each)
      const session = /^\/api\/session\/([^/]+)$/.exec(each.path)?.[1]
      if (session) {
        const conversation = conversations[decodeURIComponent(session)]
        if (!conversation) return Response.json({ message: "Not here." }, { status: 404 })
        if (each.method === "PATCH") Object.assign(conversation, each.body)
        return Response.json({ data: conversation })
      }
      const listed = engineLists[each.path]
      if (listed !== undefined) return Response.json({ data: listed })
      return Response.json({ data: {} })
    },
  })
  startBridge()
})

afterAll(() => {
  engine.stop(true)
  bridge.stop(true)
})

beforeEach(() => {
  engineAsked = []
  bridgeAsked = []
  bridgeRuns = true
  engineLists = {}
  // (What the engine listed in one test isn't another's, nor the names it kept.)
  forgetModelLists()
  forgetModelNames()
  rmSync(TYPED_SENT, { force: true })
  bridgeSays = () => ({ status: 200, body: {} })
  bridgeConversations = {}
  conversations = {
    ses_main: { id: "ses_main", location: { directory: "/srv/app" }, time: { created: 1, updated: 1 }, metadata: {} },
    ses_sub: { id: "ses_sub", parentID: "ses_main", location: { directory: "/srv/app" }, time: { created: 1, updated: 1 } },
  }
})

/** The route's answer to `method kuvo/sessions/<session>/typed` with `body`: its status and JSON. */
async function ask(method: string, session: string, body?: unknown, signal?: AbortSignal, draft = false): Promise<{ status: number; json: any }> {
  const route = `kuvo/sessions/${session}/${draft ? "draft" : "typed"}`
  const json = (status: number, value: unknown): AnswerPiece[] => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }]
  const record = (server: ReturnType<typeof Bun.serve>) => ({ pid: process.pid, port: server.port!, password: "pw", version: "test" })
  const context = {
    request: { method, path: route, headers: {}, fromApp: draft, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)) },
    phone: undefined, signal, route,
    readEngine: () => record(engine), readBridge: () => record(bridge), bridge: () => (bridgeRuns ? record(bridge) : null),
    info: { name: "Test" }, phonesChanged: () => {}, hold: null, json,
    text: (status: number, words: string): AnswerPiece[] => [{ head: { status, headers: {} } }, { body: Buffer.from(words) }],
  } as unknown as RouteContext
  let status = 0
  const parts: Buffer[] = []
  for await (const piece of draft ? draftRoute(context, route.match(DRAFT_ROUTE)!) : typedRoute(context, route.match(TYPED_ROUTE)!)) {
    if ("head" in piece) status = piece.head.status
    else parts.push(piece.body)
  }
  return { status, json: JSON.parse(Buffer.concat(parts).toString("utf8")) }
}

/** The engine's lists with DeepSeek V4 signed in and the default, so a conversation's model runs. */
const RUNS = () => ({
  "/api/provider": [{ id: "deepseek" }], "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4" }],
  "/api/integration": [{ id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential" }] }],
  "/api/model/default": { providerID: "deepseek", id: "v4" },
})

/** Whether anything but sending a message was asked of the engine or the bridge: a command, a shell
 * command, a mode switch, a compaction, a goal, a side question, a handoff, a review, a wake-up. */
const intercepted = () => [...engineAsked, ...bridgeAsked].filter(each => each.method !== "GET" && !each.path.endsWith("/prompt"))

describe("kuvo/sessions/<id>/typed, for 0.7.x clients", () => {
  test("\"/\" offers nothing: no keywords, no commands, and the engine isn't asked for its own", async () => {
    const { status, json } = await ask("GET", "ses_main")
    expect([status, json]).toEqual([200, { keywords: [], commands: [] }])
    expect(engineAsked.some(each => each.path === "/api/command")).toBe(false)
  })

  test("what's written is the client's to send as a message, \"/\" and \"!\" too: nothing is done for it", async () => {
    engineLists = RUNS()
    for (const text of ["fix the tests", "/goal ship it", "! ls", "/plan", "/compact", "/review-quick main", "/init now"])
      expect((await ask("POST", "ses_main", { text })).json).toEqual({ send: { text, quoted: true } })
    expect(intercepted()).toEqual([])
  })

  test("the same send again after the box started again: answered as it was", async () => {
    const { forgetEverythingSent } = await import("../src/link/server/sent-once")
    const first = await ask("POST", "ses_main", { text: "fix it", id: "snd_restart" })
    const { formatVersion, ...legacy } = JSON.parse(readFileSync(TYPED_SENT, "utf8"))
    expect(formatVersion).toBe(1)
    writeFileSync(TYPED_SENT, JSON.stringify(legacy))
    // The box stops and starts again: what it held in memory is gone.
    forgetEverythingSent()
    expect(await ask("POST", "ses_main", { text: "fix it", id: "snd_restart" })).toEqual(first)
    const future = JSON.stringify({ ...legacy, formatVersion: 2 })
    writeFileSync(TYPED_SENT, future)
    await expect(ask("POST", "ses_main", { text: "another", id: "snd_other" })).rejects.toThrow("Typed send receipts format version 2 is unsupported")
    expect(readFileSync(TYPED_SENT, "utf8")).toBe(future)
  })

  test("a model that can't run: said in the box's words with what to do, all of it kept, \"!\" too", async () => {
    engineLists = {
      "/api/provider": [{ id: "deepseek" }],
      "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4" }, { id: "claude-opus-5-5", providerID: "anthropic", name: "Claude Opus 5.5" }],
      "/api/integration": [
        { id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential" }] },
        { id: "anthropic", name: "Anthropic", methods: [{ type: "oauth", id: "browser" }, { type: "key" }], connections: [] },
      ],
      "/api/model/default": { providerID: "anthropic", id: "claude-opus-5-5" },
    }
    const refused = (text: string) => ({
      restore: { text }, error: "Claude Opus 5.5 needs a sign-in to Anthropic: sign in, or choose another model.",
      action: { do: "signIn", label: "Sign In", help: "Sign in to Anthropic" },
    })
    // (What a client sends as its own words is no longer what's said: the box decides.)
    expect((await ask("POST", "ses_main", { text: "/goal ship it", refusal: "Sign in to Anthropic again." })).json).toEqual(refused("/goal ship it"))
    expect((await ask("POST", "ses_main", { text: "!npm test" })).json).toEqual(refused("!npm test"))
    // On a model that runs, it goes, whatever a client says.
    conversations.ses_main.model = { providerID: "deepseek", id: "v4" }
    expect((await ask("POST", "ses_main", { text: "fix it", refusal: "Sign in to Anthropic again." })).json).toEqual({ send: { text: "fix it", quoted: true } })
    expect(intercepted()).toEqual([])
  })

  test("a coding agent's own subagent isn't refused for its model: its agent runs it on what it gives it", async () => {
    // Claude Code signed out on the bridge: its conversation is refused, its subagent isn't.
    bridgeSays = each => {
      if (each.path === "/api/integration") return { status: 200, body: { data: [{ id: "claude-code", name: "Claude Code", metadata: { source: "agent", installed: true, modelsComplete: true }, methods: [{ type: "oauth", id: "browser" }], connections: [] }] } }
      if (each.path === "/api/model") return { status: 200, body: { data: [] } }
      if (each.path === "/api/kuvo/auto") return { status: 200, body: { data: { lists: [] } } }
      return { status: 200, body: {} }
    }
    const on = { location: { directory: "/srv/app" }, time: { created: 1, updated: 1 }, model: { providerID: "claude-code", id: "opus" } }
    bridgeConversations = { ses_agent: { id: "ses_agent", ...on }, ses_agentSub: { id: "ses_agentSub", parentID: "ses_agent", ...on } }
    expect((await ask("POST", "ses_agent", { text: "fix it" })).json.error).toBe("opus needs a sign-in to Claude Code: sign in, or choose another model.")
    expect((await ask("POST", "ses_agentSub", { text: "fix it" })).json).toEqual({ send: { text: "fix it", quoted: true } })
  })

  test("none to run on: choose one; the engine not saying its lists lets it go, as a client does", async () => {
    engineLists = { "/api/provider": [{ id: "deepseek" }], "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4" }], "/api/integration": [], "/api/model/default": null }
    expect((await ask("POST", "ses_main", { text: "fix it" })).json).toEqual({
      restore: { text: "fix it" }, error: "Choose a model to send it.", action: { do: "choose", label: "Choose a Model", help: "Choose a model" },
    })
    forgetModelLists()
    engineLists = {}
    expect((await ask("POST", "ses_main", { text: "fix it" })).json).toEqual({ send: { text: "fix it", quoted: true } })
  })

  test("a conversation that isn't here, and a send without text", async () => {
    const none = await ask("POST", "ses_none", { text: "/goal ship it" })
    expect([none.status, none.json]).toEqual([404, { message: "There's no such conversation here.", restore: { text: "/goal ship it" } }])
    expect((await ask("POST", "ses_main", {})).status).toBe(400)
  })
})

describe("kuvo/sessions/<id>/draft", () => {
  let old: Buffer | undefined
  let marker = ""
  beforeAll(async () => {
    const { K } = await import("../src/link/home")
    marker = K + "/folder-version.json"
    try { old = readFileSync(marker) } catch {}
    writeFileSync(marker, '{"version":7}')
  })
  afterAll(() => {
    if (old) writeFileSync(marker, old)
    else rmSync(marker, { force: true })
  })
  const send = (id: string, text: string) => ask("POST", "ses_main", { id, text, message: {} }, undefined, true)
  const prompts = () => engineAsked.filter(call => call.method === "POST" && call.path === "/api/session/ses_main/prompt")

  test("a model that can't run: nothing is sent, and what was written goes back", async () => {
    engineLists = { "/api/provider": [{ id: "deepseek" }], "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4" }],
      "/api/integration": [], "/api/model/default": null }
    const refused = await send("msg_staleready", "Check it")
    expect(refused.json.answer).toMatchObject({ restore: { text: "Check it" }, error: "Choose a model to send it." })
    expect(prompts()).toHaveLength(0)
  })

  test("\"/goal ship it\" and \"! ls\" go to the agent as plain messages, once each by their id: nothing intercepted", async () => {
    engineLists = RUNS()
    for (const [id, text] of [["msg_slashgoal", "/goal ship it"], ["msg_bangls", "! ls"]] as const) {
      const sent = await send(id, text)
      expect(sent.status).toBe(200)
      expect(sent.json).toMatchObject({ answer: { send: { text, quoted: true } }, message: id })
      expect(prompts().at(-1)?.body).toMatchObject({ id, text })
      // The same draft again: answered as it was, sent once; another text under its id is refused.
      expect(await send(id, text)).toEqual(sent)
      expect((await send(id, `${text} again`)).status).toBe(409)
    }
    expect(prompts().map(call => call.body.text)).toEqual(["/goal ship it", "! ls"])
    expect(intercepted()).toEqual([])
  })
})
