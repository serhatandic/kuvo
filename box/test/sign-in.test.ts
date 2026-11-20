// The ways to sign in to an account, in the order and words every client offers them, and what a
// client says of a sign-in that didn't work, as the box decides them (areas/sign-in.ts), held to the
// cases clients are held to too (protocol/vectors/sign-in.json).

import "./safety"
import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { accountKind, failureWords, isPlan, serviceOf, signInMethods, userCodeOf } from "../src/link/areas/sign-in"
import { accountState } from "../src/link/areas/sign-in-state"

const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/sign-in.json", import.meta.url), "utf8"))

test.each(vectors.methods.map((each: any, at: number) => [`${each.provider} #${at}`, each]))("%s: the ways offered, in order, with their words", (_, each: any) => {
  expect(signInMethods({ id: each.provider, name: each.name, methods: each.methods, connections: each.connections ?? [] })).toEqual(each.offered)
})

test.each(vectors.failures.map((each: any) => [`${each.status} ${JSON.stringify(each.message ?? null)}`, each]))("%s: what a client says", (_, each: any) => {
  expect(failureWords(each.status, each.message)).toBe(each.failure)
})

test.each(vectors.accounts.map((each: any) => [each.about, each]))("%s", (_, each: any) => {
  expect({ ...accountKind(each.account, each.agents === true), service: serviceOf(each.account, each.agents === true) }).toEqual({ kind: each.kind, listed: each.listed, service: each.service })
})

test.each(vectors.codes.map((each: any) => [each.about, each]))("a device sign-in's code: %s", (_, each: any) => {
  expect(userCodeOf(each.attempt)).toBe(each.userCode)
})

test.each(vectors.states.map((each: any) => [each.about, each]))("an account's state, line and actions: %s", (_, each: any) => {
  expect(signInMethods(each.account)).toEqual(each.offered)
  expect(isPlan(each.offered)).toBe(each.plan)
  expect(accountState(each.account, each.offered, { agents: each.agents === true, ended: each.ended, computer: each.computer })).toEqual({ state: each.state, line: each.line, actions: each.actions, connections: each.connections, ...(each.added ? { added: true } : {}) })
})

// Through the box's routes, the engine a stand-in: Accounts lists what the box says is a provider,
// with its mark, and a device sign-in started through the box shows its code apart, for the
// engine's own plans as for the coding agents'.
test("Accounts' list says which are providers and their marks; a sign-in started through the box has its page's code", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const asked: any[] = []
  const engine = Bun.serve({
    port: 0,
    async fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/integration") {
        return Response.json({ data: [
          { id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth", id: "device", label: "Login with GitHub Copilot" }], connections: [] },
          { id: "exa", name: "Exa", methods: [{ type: "key" }], connections: [] },
          { id: "linear", name: "Linear", metadata: { source: "mcp" }, methods: [{ type: "oauth", id: "mcp", label: "Sign in" }], connections: [] },
          { id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [] },
          // OpenAI as engine 2.2.5 lists it: its ChatGPT plan comes through Codex, so only its key.
          { id: "openai", name: "OpenAI", methods: [{ type: "oauth", id: "chatgpt-browser", label: "ChatGPT Pro/Plus (browser)" }, { type: "key", label: "Manually enter API Key" }], connections: [] },
        ] })
      }
      if (url.pathname === "/api/integration/github-copilot/connect/oauth" && request.method === "POST") {
        asked.push(await request.json())
        return Response.json({ data: { attemptID: "att_1", url: "https://github.com/login/device", instructions: "Enter code: 8F2A-91KQ", mode: "auto" } })
      }
      if (url.pathname === "/api/integration/github-copilot/connect/oauth/att_1") return Response.json({ data: { status: "pending", url: "https://github.com/login/device?user_code=8F2A-91KQ" } })
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
  try {
    const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any, { name: "test" }, () => { throw new Error("none") })
    const ask = async (method: string, path: string, body?: unknown, transport: Partial<import("../src/link/server/context").LinkRequest> = { fromApp: true, served: true }) => {
      let status = 0, text = ""
      for await (const piece of handler({ method, path, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), ...transport }, undefined)) {
        if ("head" in piece) status = piece.head.status
        if ("body" in piece) text += Buffer.from(piece.body).toString()
      }
      return { status, body: JSON.parse(text || "{}") }
    }
    const { body } = await ask("GET", "kuvo/sign-in/accounts")
    expect(body.accounts.map((each: any) => [each.id, each.kind, each.listed, each.service])).toEqual([
      ["github-copilot", "provider", true, "copilot"], ["exa", "search", false, undefined], ["linear", "mcp", false, undefined], ["deepseek", "provider", true, undefined], ["openai", "provider", true, "codex"],
    ])
    // A plan you sign in to, or an account you enter a key for (#733): a key is never "Sign In".
    expect(body.accounts.map((each: any) => [each.id, each.plan, each.actions.map((action: any) => action.label)])).toEqual([
      ["github-copilot", true, ["Sign In"]], ["exa", false, ["Add Key"]], ["linear", true, ["Sign In"]], ["deepseek", false, ["Add Key"]], ["openai", false, ["Add Key"]],
    ])
    expect(body.accounts[0].hint).toBe("Sign in")
    // How each stands and what can be done with it, in this box's words (its name among them).
    expect(body.accounts.map((each: any) => [each.id, each.state, each.line, each.actions.map((action: any) => action.id)])).toEqual([
      ["github-copilot", "signedOut", "Sign in", ["signIn"]], ["exa", "signedOut", "API key", ["signIn"]], ["linear", "signedOut", "Sign in", ["signIn"]], ["deepseek", "signedOut", "API key", ["signIn"]], ["openai", "signedOut", "API key", ["signIn"]],
    ])

    const started = await ask("POST", "kuvo/sign-in/start?integration=github-copilot", { methodID: "device" })
    expect(started).toEqual({ status: 200, body: { attemptID: "att_1", url: "https://github.com/login/device", instructions: "Enter code: 8F2A-91KQ", mode: "auto", userCode: "8F2A-91KQ" } })
    const path = "kuvo/sign-in/start?integration=github-copilot"
    for (const transport of [{ fromMac: true }, { desktop: true }, {}]) {
      expect((await ask("POST", path, { methodID: "device", remote: false }, transport)).body.userCode).toBe("8F2A-91KQ")
    }
    expect((await ask("POST", path, { methodID: "device", remote: true }, { fromMac: true, served: true })).status).toBe(200)
    const legacy = await ask("POST", "api/integration/github-copilot/connect/oauth", { methodID: "device", remote: false }, {})
    expect(legacy.body.data.attemptID).toBe("att_1")
    expect(asked).toEqual([
      { methodID: "device", remote: false },
      ...Array.from({ length: 3 }, () => ({ methodID: "device", remote: true })),
      { methodID: "device", remote: false },
      { methodID: "device", remote: true },
    ])
    expect((await ask("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_1")).body.userCode).toBe("8F2A-91KQ")
    // Refused as it should be: no way in named, another method, an account the engine hasn't.
    expect((await ask("POST", "kuvo/sign-in/start?integration=github-copilot", {})).status).toBe(400)
    expect((await ask("GET", "kuvo/sign-in/start?integration=github-copilot")).status).toBe(405)
    expect((await ask("POST", "kuvo/sign-in/start?integration=nope", { methodID: "device" })).status).toBe(404)
  } finally {
    engine.stop(true)
  }
})

test.each(["claude", "codex", "cursor", "grok", "antigravity"])("%s offers another account only on its own sign-in", agent => {
  const offered = { methods: [{ id: "oauth", type: "oauth" as const, label: "Sign In" }], environment: [], hint: "" }
  const account = { id: agent, name: agent, metadata: { addedAccounts: true } }
  expect(accountState(account, offered, { agents: true, computer: "Studio" }).actions.some(action => action.id === "addAnother")).toBe(true)
  expect(accountState({ ...account, id: agent + "@work" }, offered, { agents: true, computer: "Studio" }).actions.some(action => action.id === "addAnother")).toBe(false)
})

test("one Accounts catalog lists both sources in order and keeps the other when a source is unavailable", async () => {
  const { accountCatalogRoute } = await import("../src/link/areas/account-catalog")
  const { GITHUB_WORK_QUEUE } = await import("../src/shared/github")
  let engineAvailable = true
  let engineMalformed = false
  let bridgeMoved = false
  let bridgeUnavailable = false
  let engineReads = 0
  let agentReads = 0
  const engine = Bun.serve({ port: 0, fetch() {
    engineReads++
    if (engineMalformed) return Response.json({ data: {} })
    return Response.json({ data: [
      { id: "openai", name: "OpenAI", methods: [{ type: "key" }] },
      { id: "exa", name: "Exa", methods: [{ type: "key" }] },
      { id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential", id: "cred_a", method: "key" }] },
    ] })
  } })
  const oldBridge = Bun.serve({ port: 0, fetch(request) {
    if (new URL(request.url).pathname === "/api/integration") {
      bridgeMoved = true
      return Response.json({ message: "Moving" }, { status: 503, headers: { "x-kuvo-bridge-handover": "1" } })
    }
    return Response.json({ data: { id: GITHUB_WORK_QUEUE, name: "GitHub for Work Queue", methods: [{ type: "oauth" }] } })
  } })
  const bridge = Bun.serve({ port: 0, fetch(request) {
    if (new URL(request.url).pathname === `/api/integration/${GITHUB_WORK_QUEUE}`) {
      return Response.json({ data: { id: GITHUB_WORK_QUEUE, name: "GitHub for Work Queue", methods: [{ type: "oauth" }] } })
    }
    agentReads++
    if (bridgeUnavailable) return Response.json(null, { status: 503 })
    return Response.json({ data: [
      { id: "codex-cli", name: "Codex", metadata: { accountLabel: "Work", version: "2", programUpdate: { state: "updating", extra: "internal" }, extra: "internal" } },
      { id: "claude-code", name: "Claude Code" },
    ] })
  } })
  async function catalog(method = "GET") {
    let reply: { status: number; body: any } = { status: 0, body: null }
    const route = "kuvo/accounts/catalog"
    const readEngine = () => {
      if (!engineAvailable) throw new Error("Engine unavailable")
      return { port: engine.port, password: "pw" } as any
    }
    for await (const _ of accountCatalogRoute({
      route, readEngine, request: { method, path: route, headers: {}, fromApp: true, served: true },
      bridge: () => ({ port: (bridgeMoved ? bridge : oldBridge).port, password: "pw" }),
      json: function* (status: number, body: unknown) {
        reply = { status, body }
        yield { head: { status, headers: { "content-type": "application/json" } } }
        yield { body: Buffer.from(JSON.stringify(body)) }
      },
    } as any)) {}
    return reply
  }
  try {
    const { status, body } = await catalog()
    expect(status).toBe(200)
    expect(body.agents.map((row: any) => [row.id, row.section])).toEqual([["codex-cli", "codingAgents"], ["claude-code", "codingAgents"]])
    expect(body.agents[0].metadata).toEqual({ accountLabel: "Work", version: "2", programUpdate: { state: "updating" } })
    expect(body.engine.map((row: any) => [row.id, row.section])).toEqual([["openai", "addProvider"], ["deepseek", "providers"], [GITHUB_WORK_QUEUE, "addProvider"]])
    expect(body.engine[0].actions[0]).toMatchObject({ id: "signIn", sheetTitle: "Add Key · OpenAI" })
    expect(bridgeMoved).toBe(true)
    expect([engineReads, agentReads]).toEqual([1, 1])
    engineAvailable = false
    const partial = await catalog()
    expect(partial.status).toBe(200)
    expect(partial.body).toEqual({ agents: body.agents, engine: [], engineDown: { message: expect.any(String) } })
    expect([engineReads, agentReads]).toEqual([1, 2])
    engineAvailable = true
    bridgeUnavailable = true
    const withoutBridge = await catalog()
    expect(withoutBridge).toEqual({ status: 200, body: { agents: [], engine: body.engine, agentsDown: { message: "It said no (503)." } } })
    engineMalformed = true
    expect((await catalog()).status).toBe(502)
    expect((await catalog("POST")).status).toBe(405)
  } finally {
    engine.stop(true)
    oldBridge.stop(true)
    bridge.stop(true)
  }
})

test("an outside account request uses the display name, never the request info hostname", async () => {
  const { signInRoute } = await import("../src/link/areas/sign-in")
  const { ownComputerName } = await import("../src/link/computer-name")
  const engine = Bun.serve({ port: 0, fetch: () => Response.json({ data: [{ id: "codex-cli", name: "Codex", metadata: { installed: false, install: "install" }, methods: [] }] }) })
  let result: any
  try {
    for await (const _ of signInRoute({
      route: "kuvo/sign-in/accounts", request: { method: "GET", path: "kuvo/sign-in/accounts?agents=1", fromApp: false }, info: { name: "RAW-HOSTNAME.local" },
      bridge: () => ({ port: engine.port, password: "pw" }),
      json: function* (_: number, body: unknown) { result = body },
    } as any)) {}
    expect(result.accounts[0].line).toBe(`Not installed on ${ownComputerName()}`)
    expect(result.accounts[0].line).not.toContain("RAW-HOSTNAME")
  } finally { engine.stop(true) }
})

test("one sign-in action route forwards to the account's side and keeps the Work Queue attempt owner", async () => {
  const { signInActionRoute } = await import("../src/link/areas/sign-in-action")
  const seen: unknown[] = []
  const side = (name: string) => Bun.serve({ port: 0, async fetch(request) {
    seen.push({
      side: name, method: request.method, path: decodeURIComponent(new URL(request.url).pathname),
      body: request.method === "POST" ? await request.json() : null,
      owner: request.headers.get("x-kuvo-sign-in-owner"),
    })
    return Response.json({ data: {} })
  } })
  const engine = side("engine"), agents = side("agents")
  const target = (server: typeof engine) => ({ port: server.port, password: "pw" })
  const client = "a".repeat(32)
  const act = async (query: string, body: object) => {
    const route = "kuvo/sign-in/action"
    let status = 0
    for await (const piece of signInActionRoute({
      route, readEngine: () => target(engine), bridge: () => target(agents),
      request: { method: "POST", path: `${route}?${query}`, body: Buffer.from(JSON.stringify(body)), headers: { "content-type": "application/json", "x-kuvo-sign-in-client": client } },
      json: (status: number, value: unknown) => [{ head: { status, headers: {} } }, { body: Buffer.from(JSON.stringify(value)) }],
    } as any)) if ("head" in piece) status = piece.head.status
    return status
  }
  try {
    expect(await act("integration=bedrock?qa", { action: "external", methodID: "aws", answer: { region: "eu-west-1" } })).toBe(200)
    expect(await act("integration=claude%40work&agents=1", { action: "complete", attempt: "att_1", code: "test-code" })).toBe(200)
    expect(await act("integration=github-work-queue", { action: "cancel", attempt: "att_2" })).toBe(200)
    expect(await act("integration=bedrock", { action: "complete", attempt: "att_1" })).toBe(400)
    expect(seen).toEqual([
      { side: "engine", method: "POST", path: "/api/integration/bedrock?qa/connect/external", body: { methodID: "aws", answer: { region: "eu-west-1" } }, owner: null },
      { side: "agents", method: "POST", path: "/api/integration/claude@work/connect/oauth/att_1/complete", body: { code: "test-code" }, owner: null },
      { side: "agents", method: "DELETE", path: "/api/integration/github-work-queue/connect/oauth/att_2", body: null, owner: `client:${client}` },
    ])
  } finally {
    engine.stop(true)
    agents.stop(true)
  }
})

test("remote provider OAuth offers the callback address field and explains a local-only page", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const cancelled: string[] = []
  const engine = Bun.serve({ port: 0, fetch(request) {
    const route = new URL(request.url).pathname
    if (request.method === "DELETE") {
      cancelled.push(route)
      return Response.json({})
    }
    const url = route.includes("/local-only/") ? "http://localhost:1234/start" : "https://provider.example/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1234%2Fcallback&state=abc"
    return Response.json({ data: { attemptID: "att_1", url, mode: "auto" } })
  } })
  try {
    const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any)
    const ask = async (integration: string, transport: { fromApp?: boolean; fromMac?: boolean; served?: boolean }) => {
      let status = 0
      let body: any
      const request = { method: "POST", path: `kuvo/sign-in/start?integration=${integration}`, headers: { "x-kuvo-sign-in-client": "a".repeat(32) }, body: Buffer.from('{"methodID":"browser"}'), ...transport }
      for await (const part of handler(request, undefined)) {
        if ("head" in part) status = part.head.status
        if ("body" in part) body = JSON.parse(part.body.toString())
      }
      return { status, body }
    }
    expect((await ask("provider", { fromApp: true, served: true })).body.pasteAddress).toBeUndefined()
    expect((await ask("provider", { fromMac: true })).body.pasteAddress).toBe(true)
    expect((await ask("provider", {})).body.pasteAddress).toBe(true)
    const refused = await ask("local-only", { fromMac: true })
    expect(refused.status).toBe(409)
    expect(refused.body.message).toContain("Open Kuvo Code on")
    expect(cancelled).toEqual(["/api/integration/local-only/connect/oauth/att_1"])
  } finally {
    engine.stop(true)
  }
})

test("pasting a callback requires the starting client, account, attempt, callback and state", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const seen: string[] = []
  const listener = Bun.serve({ port: 0, fetch(request) {
    seen.push(request.url)
    return new Response("OK")
  } })
  const callback = `http://127.0.0.1:${listener.port}/callback`
  const engine = Bun.serve({ port: 0, fetch(request) {
    const url = new URL(request.url)
    const account = url.pathname.split("/")[3]!
    return Response.json({ data: request.method === "POST"
      ? { attemptID: `try-${account}`, url: `https://provider.example/auth?${new URLSearchParams({ redirect_uri: callback, state: account })}` }
      : url.pathname.includes("/safe-failure/")
        ? { status: "failed", message: `secret: ${callback}?code=private /home/user/.credential` }
        : { status: "pending" } })
  } })
  try {
    const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any)
    const ask = async (method: string, path: string, given?: unknown, client = "a") => {
      let status = 0, body: any
      for await (const part of handler({ method, path, headers: { "x-kuvo-sign-in-client": client.repeat(32) },
        body: given === undefined ? undefined : Buffer.from(JSON.stringify(given)) }, undefined)) {
        if ("head" in part) status = part.head.status
        if ("body" in part) body = JSON.parse(part.body.toString())
      }
      return { status, body }
    }
    for (const account of ["first", "second"]) {
      expect((await ask("POST", `kuvo/sign-in/start?integration=${account}`, { methodID: "browser" })).body.pasteAddress).toBe(true)
    }
    const post = (url: string, client = "a", attempt = "try-first") =>
      ask("POST", "kuvo/sign-in/return?check=1", { integration: "first", attempt, url }, client)
    expect((await post(callback + "?state=second&code=private")).status).toBe(400)
    expect((await post(callback + "/other?state=first&code=private")).status).toBe(400)
    expect((await post(callback + "?state=first&code=private", "b")).status).toBe(400)
    expect((await post(callback + "?state=first&code=private", "a", "try-second")).status).toBe(400)
    expect(seen).toEqual([])
    expect((await post(callback + "?state=first&code=private")).status).toBe(200)
    expect(seen).toHaveLength(1)
    expect((await ask("GET", "kuvo/sign-in/status?integration=safe-failure&attempt=ended")).body)
      .toEqual({ status: "failed", message: "Signing in didn’t work.", failure: "Signing in didn’t work." })
  } finally {
    listener.stop(true)
    engine.stop(true)
  }
})

test("an existing phone starts, polls and returns a browser sign-in with just its URL", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  let callbacks = 0
  let late = false
  const engine = Bun.serve({ port: 0, fetch(request) {
    const url = new URL(request.url)
    if (url.pathname === "/callback") {
      callbacks++
      return new Response("OK")
    }
    const state = url.pathname.includes("/later/") ? "later" : "browser"
    const page = `https://provider.example/auth?${new URLSearchParams({ redirect_uri: `${url.origin}/callback`, state })}`
    if (request.method === "POST") return Response.json({ data: { attemptID: "att_legacy", url: late ? "" : page } })
    return Response.json({ data: { status: "pending", url: page } })
  } })
  try {
    const handler = engineHandler(() => ({ port: engine.port!, password: "pw" }))
    const ask = async (method: string, path: string, given?: unknown, device = 1) => {
      let status = 0, body: any
      for await (const piece of handler({ method, path, headers: { "content-type": "application/json" }, body: given ? Buffer.from(JSON.stringify(given)) : undefined }, { key: Buffer.alloc(32, device) })) {
        if ("head" in piece) status = piece.head.status
        if ("body" in piece) body = JSON.parse(piece.body.toString())
      }
      return { status, body }
    }
    const endpoint = "kuvo/sign-in/return"
    for (const account of ["legacy", "later", "ambiguous"]) {
      late = account === "later"
      const address = `http://127.0.0.1:${engine.port}/callback?code=ok&state=${late ? "later" : "browser"}`
      const route = `api/integration/${account}/connect/oauth`
      expect((await ask("POST", route, { methodID: "browser" })).body.data.attemptID).toBe("att_legacy")
      expect((await ask("GET", `kuvo/sign-in/status?integration=${account}&attempt=att_legacy`)).body.status).toBe("pending")
      expect((await ask("POST", endpoint, { url: "" })).status).toBe(400)
      expect((await ask("POST", endpoint, { url: address }, 2)).status).toBe(400)
      expect((await ask("POST", endpoint, { url: address })).status).toBe(account === "ambiguous" ? 400 : 200)
    }
    expect(callbacks).toBe(2)
  } finally { engine.stop(true) }
})
