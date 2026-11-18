// The models offered (`kuvo/models/offered`, PROTOCOL.md "The models offered"): each with the side a
// conversation on it runs on and its efforts, so a client offers in place only its own side's and
// draws their efforts without reading the engine's or the bridge's lists itself.

import "./safety"
import { expect, test } from "bun:test"
import { offeredAnswer, modelsOfferedRoute } from "../src/link/areas/models-offered"
import { forgetModelLists, noteModelsEvent } from "../src/link/work/model-rule"
import type { RouteContext } from "../src/link/server/context"
import type { Listed, Lists } from "../src/link/work/model-rule"

const listed = (providerID: string, id: string, name: string, efforts: string[]): Listed => ({
  providerID, id, name, aliases: [], efforts, free: false, provisional: false, hidden: false, retiring: false, released: 1,
})

const lists: Lists = {
  engine: { ready: new Set(["anthropic"]), models: [listed("anthropic", "claude-opus-5-5", "Claude Opus 5.5", ["low", "high", "max"])], accounts: new Map(), fallback: null },
  bridge: {
    models: [{ ...listed("claude-code", "opus", "Opus", ["low", "medium", "high"]), aliases: ["default"] }],
    accounts: new Map([["claude-code", { id: "claude-code", name: "Claude Code", methods: [], signedIn: true, installed: true, complete: true }]]),
    auto: [{ id: "auto_1", name: "Everyday", entries: 2 }],
  },
}

test("each model says its side and its efforts: the engine's, the coding agents', and Custom Auto's lists on the agents' side", () => {
  const answer = offeredAnswer(lists, null)
  expect(answer.models.map(model => [model.providerID, model.id, model.side, model.efforts])).toEqual([
    ["anthropic", "claude-opus-5-5", "engine", ["low", "high", "max"]],
    ["claude-code", "opus", "agents", ["low", "medium", "high"]],
    ["auto", "auto_1", "agents", []],
  ])
  expect(answer.known).toEqual({ engine: true, agents: true })
  // Other ids a conversation may name one by: Claude Code's `default`, the model it stands for.
  expect(answer.models[1].aliases).toEqual(["default"])
})

test("a side that can't say has none of its models, and says so", () => {
  const answer = offeredAnswer({ ...lists, bridge: null }, null)
  expect(answer.models.map(model => model.side)).toEqual(["engine"])
  expect(answer.known).toEqual({ engine: true, agents: false })
})

// Which models need no account (#733), so a client says "free models" without knowing OpenCode's
// provider: OpenCode Zen's free ones (signed out); never one that costs nothing only with its
// account (Copilot's), one of your own listing no prices (perhaps a paid gateway), a paid one, or a
// coding agent's.
test("a model that needs no account says so: OpenCode Zen's free ones, not Copilot's or one of your own", () => {
  const free = (model: Listed): Listed => ({ ...model, free: true })
  const answer = offeredAnswer({
    engine: {
      ready: new Set(["anthropic", "github-copilot", "ollama"]),
      models: [
        listed("anthropic", "claude-opus-5-5", "Claude Opus 5.5", []),
        free(listed("github-copilot", "gpt-5-mini", "GPT-5 mini", [])),
        free(listed("opencode", "big-pickle", "Big Pickle", [])),
        free(listed("ollama", "qwen3", "Qwen3", [])),
      ],
      accounts: new Map([
        ["anthropic", { id: "anthropic", name: "Anthropic", methods: [["oauth", "claude"], ["key", ""]], signedIn: true, installed: true, complete: false }],
        ["github-copilot", { id: "github-copilot", name: "GitHub Copilot", methods: [["oauth", "device"]], signedIn: true, installed: true, complete: false }],
        ["opencode", { id: "opencode", name: "OpenCode Zen", methods: [["key", ""]], signedIn: false, installed: true, complete: false }],
      ]),
      fallback: null,
    },
    bridge: { ...lists.bridge!, models: [free(listed("claude-code", "opus", "Opus", []))] },
  }, null)
  expect(answer.models.map(model => [model.providerID, model.id, model.free ?? false])).toEqual([
    ["github-copilot", "gpt-5-mini", false],
    ["anthropic", "claude-opus-5-5", false],
    ["opencode", "big-pickle", true],
    ["ollama", "qwen3", false],
    ["claude-code", "opus", false],
    ["auto", "auto_1", false],
  ])
})

// Each provider named as its side names it, so every client's picker says where a model is from
// (`models.offered.providers`), in the order its models come.
test("each provider is named as its side lists it, a coding agent's second account with its label", () => {
  const answer = offeredAnswer({
    engine: { ...lists.engine!, accounts: new Map([["anthropic", { id: "anthropic", name: "Anthropic", methods: [], signedIn: true, installed: true, complete: false }]]) },
    bridge: { ...lists.bridge!, accounts: new Map([["claude-code", { id: "claude-code", name: "Claude Code", label: "Work", methods: [], signedIn: true, installed: true, complete: true }]]) },
  }, null)
  expect(answer.providers).toEqual([
    { id: "anthropic", name: "Anthropic" },
    { id: "claude-code", name: "Claude Code", label: "Work" },
    { id: "auto", name: "Custom Auto" },
  ])
  // None starred: an empty list, not left out.
  expect(answer.starred).toEqual([])
})

test("a provider its side doesn't name is named by its id", () => {
  const answer = offeredAnswer(lists, null)
  expect(answer.providers?.find(each => each.id === "anthropic")).toEqual({ id: "anthropic", name: "anthropic" })
})

// Exercise the actual box route and its cached engine reads: the engine provider is independent
// of the coding agents, including when a bot is currently using a coding agent's model.
test("the route offers an API-key provider and refreshes its models when credentials change", async () => {
  let connected = false
  const server = Bun.serve({
    port: 0,
    fetch(request) {
      const route = new URL(request.url).pathname
      if (route.startsWith("/api/session/")) return Response.json({ data: { id: route.split("/").at(-1), metadata: route.endsWith("bot-session") ? { kuvo: { bot: "test-bot" } } : {} } })
      const data = route === "/api/provider" ? [{ id: "anthropic" }, ...(connected ? [{ id: "opencode-go" }] : [])]
        : route === "/api/model" ? [listed("anthropic", "opus", "Opus", []), listed("opencode-go", "glm-5", "GLM 5", [])]
        : route === "/api/integration" ? [
            { id: "anthropic", name: "Anthropic", methods: [{ type: "key" }], connections: [{}] },
            { id: "opencode-go", name: "OpenCode Go", methods: [{ type: "key" }], connections: connected ? [{}] : [] },
          ]
        : null
      return Response.json({ data })
    },
  })
  const target = { port: server.port!, password: "test", pid: process.pid }
  const read = async (session?: string) => {
    let answer: any
    const context = {
      request: { method: "GET", path: `kuvo/models/offered${session ? `?session=${session}` : ""}`, headers: {} },
      readEngine: () => target,
      bridge: () => null,
      json: (status: number, value: unknown) => { expect(status).toBe(200); answer = value; return [] },
    } as unknown as RouteContext
    for await (const _ of modelsOfferedRoute(context)) {}
    return answer
  }
  forgetModelLists()
  try {
    expect((await read()).models.map((model: any) => model.providerID)).toEqual(["anthropic"])
    connected = true
    noteModelsEvent({ type: "credential.updated" })
    const answer = await read()
    expect(answer.models.map((model: any) => [model.providerID, model.side])).toEqual([["anthropic", "engine"], ["opencode-go", "engine"]])
    const scoped = await read("bot-session")
    expect(scoped.scoped).toBe(true)
    expect(scoped.models).toEqual(answer.models)
    expect(scoped.note).toBe("To use a coding agent in this bot, change its model in Edit Bot.")
    const coding = await read("coding-session")
    expect(coding.scoped).toBe(true)
    expect(coding.note).toContain("This conversation uses provider models")
    expect(answer.providers).toContainEqual({ id: "opencode-go", name: "OpenCode Go" })
    connected = false
    noteModelsEvent({ type: "credential.switched" })
    expect((await read()).models.map((model: any) => model.providerID)).toEqual(["anthropic"])
  } finally {
    server.stop(true)
    forgetModelLists()
  }
})

test("bridge bots offer provider models too; engine bots explain choosing a coding agent in Edit Bot", async () => {
  let sessionOnBridge = false
  let botSession = true
  const serve = (agents: boolean) => Bun.serve({ port: 0, fetch(request) {
    const route = new URL(request.url).pathname
    if (route.startsWith("/api/session/") && sessionOnBridge !== agents) return new Response("", { status: 404 })
    const provider = agents ? "claude-code" : "opencode-go"
    const data = route.startsWith("/api/session/")
      ? { id: "bot-session", metadata: botSession ? { kuvo: { bot: "bot-one" } } : {} }
      : route === "/api/provider" ? [{ id: provider }]
      : route === "/api/model" ? [listed(provider, "model", "Model", [])]
      : route === "/api/integration" ? [{ id: provider, name: provider, methods: [{ type: "key" }], connections: [{}], metadata: { modelsComplete: true } }]
      : route === "/api/kuvo/auto" ? { lists: [] } : null
    return Response.json({ data })
  } })
  const engine = serve(false), agents = serve(true)
  const target = (server: typeof engine) => ({ port: server.port!, password: "test", pid: process.pid })
  const read = async (onBridge: boolean) => {
    sessionOnBridge = onBridge
    let answer: any
    const context = {
      request: { method: "GET", path: "kuvo/models/offered?session=bot-session", headers: {} },
      readEngine: () => target(engine),
      bridge: () => target(agents),
      json: (status: number, value: unknown) => { expect(status).toBe(200); answer = value; return [] },
    } as unknown as RouteContext
    for await (const _ of modelsOfferedRoute(context)) {}
    return answer
  }
  forgetModelLists()
  try {
    const bridge = await read(true)
    expect(bridge.models.map((model: any) => model.providerID)).toEqual(["opencode-go", "claude-code"])
    expect(bridge.note).toBeUndefined()
    const answer = await read(false)
    expect(answer.models.map((model: any) => model.providerID)).toEqual(["opencode-go"])
    expect(answer.note).toBe("To use a coding agent in this bot, change its model in Edit Bot.")
    botSession = false
    const ordinary = await read(false)
    expect(ordinary.models.map((model: any) => model.providerID)).toEqual(["opencode-go"])
    expect(ordinary.note).toContain("This conversation uses provider models")
  } finally { engine.stop(true); agents.stop(true); forgetModelLists() }
})

test("opening the picker bypasses the list cache and asks the bridge to refresh upstream", async () => {
  let upstream = "old"
  let cached = upstream
  const bridge = Bun.serve({ port: 0, fetch(request) {
    const url = new URL(request.url)
    if (url.searchParams.get("refresh") === "1") cached = upstream
    const data = url.pathname === "/api/model" ? [listed("codex-cli", cached, cached, [])]
      : url.pathname === "/api/integration" ? [{ id: "codex-cli", name: "Codex", connections: [{}] }]
      : { lists: [] }
    return Response.json({ data })
  } })
  const ask = async (refresh = false) => {
    let answer: any
    const context = {
      request: { method: "GET", path: `kuvo/models/offered${refresh ? "?refresh=1" : ""}`, headers: {} },
      readEngine: () => { throw new Error("No engine") },
      bridge: () => ({ port: bridge.port!, password: "test", pid: process.pid }),
      json: (status: number, value: unknown) => { expect(status).toBe(200); answer = value; return [] },
    } as unknown as RouteContext
    for await (const _ of modelsOfferedRoute(context)) {}
    return answer.models.map((model: any) => model.id)
  }
  try {
    expect(await ask()).toEqual(["old"])
    upstream = "released"
    expect(await ask(true)).toEqual(["released"])
  } finally {
    bridge.stop(true)
    forgetModelLists()
  }
})
