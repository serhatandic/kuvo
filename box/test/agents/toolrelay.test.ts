// Kuvo Code's tools for an agent run whole in the box's sandbox, through the bridge's relay: each
// call is its own session's, whatever it says, and reaches the tools where the sandbox can't.

import "../safety"
import { afterAll, afterEach, describe, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setToolProgram, setToolServer } from "../../src/agents/kuvotools"
import { agentToolsFor, dropRelayed, relayedTools, setToolRelayRun } from "../../src/agents/toolrelay"
import { toolForwarder } from "../../src/agents/tool-forwarder"
import { noteToolServer } from "../../src/agents/fence"

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-relay-test-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

/** The agent's thread holding a session's tools. */
const thread = {}

afterEach(() => {
  setToolRelayRun(undefined)
  setToolServer(undefined)
  setToolProgram(undefined)
  for (const session of ["ses_one", "ses_two"]) dropRelayed(session, thread)
})

/** Calls a tool through the relay, as the agent would, with `key`'s session and saying it's `claims`'s. */
async function call(tools: { url: string; headers: Record<string, string> }, id: number, claims?: string) {
  const response = await fetch(tools.url, {
    method: "POST",
    headers: { ...tools.headers, "Content-Type": "application/json", Accept: "application/json, text/event-stream" },
    body: JSON.stringify({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "show_in_panel", arguments: {}, ...(claims ? { _meta: { "ai.opencode/sessionID": claims } } : {}) } }),
  })
  return { status: response.status, body: (response.status === 200 ? await response.json() : undefined) as any }
}

test("the stdio forwarder waits for the relay and resends the same tool identity", async () => {
  const seen: string[] = []
  let listening = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(null, { status: 503 }) })
  const port = listening.port
  await listening.stop(true)
  const client = Bun.spawn([process.execPath, "--config=/dev/null", "--no-env-file", "-e", toolForwarder], {
    env: { ...process.env, KUVO_TOOL_RELAY: `http://127.0.0.1:${port}/mcp`, KUVO_TOOL_RELAY_KEY: "Bearer fixture", KUVO_TOOL_TIMEOUT: "5000" },
    stdin: "pipe", stdout: "pipe", stderr: "inherit",
  })
  const reader = client.stdout.getReader()
  try {
    client.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "send_message" } }) + "\n")
    client.stdin.flush()
    await Bun.sleep(350)
    listening = Bun.serve({ hostname: "127.0.0.1", port, async fetch(request) {
      const body: any = await request.json()
      expect(request.headers.get("authorization")).toBe("Bearer fixture")
      seen.push(body.params._meta["kuvo.operation"])
      if (seen.length === 1) return Response.json({ jsonrpc: "2.0", id: 1, error: { data: { kuvoRelayRestart: true } } })
      return Response.json({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "Sent." }] } })
    } })
    const answer = JSON.parse(new TextDecoder().decode((await reader.read()).value))
    expect(answer.result.content[0].text).toBe("Sent.")
    expect(seen).toHaveLength(2)
    expect(seen[0]).toMatch(/^[a-f0-9-]{36}$/)
    expect(seen[0]).toBe(seen[1])
  } finally {
    reader.releaseLock()
    client.stdin.end()
    await client.exited
    await listening.stop(true)
  }
})

describe("a new bridge", () => {
  /** A bridge's relay as a process of its own, on the box in `kuvo`: what it says first, and how to end it. */
  async function relayBridge(kuvo: string, args: string[]) {
    const child = Bun.spawn([process.execPath, join(import.meta.dir, "support", "relay-bridge.ts"), ...args], { env: { ...process.env, KUVO_HOME: kuvo }, stdin: "pipe", stdout: "pipe", stderr: "inherit" })
    const reader = child.stdout.getReader()
    let said = ""
    while (!said.includes("\n")) said += new TextDecoder().decode((await reader.read()).value)
    reader.releaseLock()
    return { said: JSON.parse(said.split("\n")[0]!), end: async () => (child.stdin.write("end\n"), child.stdin.flush(), await child.exited) }
  }

  test("an agent that outlived its bridge (held through an update) reaches its tools at the same address with the same key; a key the new bridge took up no agent for is turned down", async () => {
    const kuvo = mkdtempSync(join(base, "kuvo-"))
    const program = join(base, "said.js")
    writeFileSync(program, `let buffer = ""
process.stdin.on("data", (chunk) => {
  buffer += chunk
  let at
  while ((at = buffer.indexOf("\\n")) >= 0) {
    const message = JSON.parse(buffer.slice(0, at))
    buffer = buffer.slice(at + 1)
    if (message.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { content: [{ type: "text", text: process.env.KUVO_SESSION }] } }) + "\\n")
  }
})
`)
    const first = await relayBridge(kuvo, ["first", program, "ses_one", "ses_two"])
    const given = first.said as Record<string, { url: string; headers: Record<string, string> }>
    expect(await first.end()).toBe(0)
    // The next bridge took up ses_one's agent; ses_two's was gone by then.
    const next = await relayBridge(kuvo, ["next", program, "ses_one"])
    try {
      expect((await call(given.ses_one!, 1)).body.result.content[0].text).toBe("ses_one")
      expect((await call(given.ses_two!, 2)).status).toBe(401)
    } finally {
      await next.end()
    }
  })
})

describe("on a machine: the program, run outside the sandbox", () => {
  test("each call reaches the program as its own session's, even one that names another, and answers its own request", async () => {
    // The program, as the machine's engine has it: says which session each call was for.
    const program = join(base, "tools.js")
    writeFileSync(program, `let buffer = ""
process.stdin.on("data", (chunk) => {
  buffer += chunk
  let at
  while ((at = buffer.indexOf("\\n")) >= 0) {
    const message = JSON.parse(buffer.slice(0, at))
    buffer = buffer.slice(at + 1)
    if (message.id === undefined) continue
    const session = message.params?._meta?.["ai.opencode/sessionID"] ?? process.env.KUVO_SESSION
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { content: [{ type: "text", text: session }] } }) + "\\n")
  }
})
`)
    setToolProgram(JSON.stringify({ mcp: { servers: { "kuvo-browser": { command: [process.execPath, program], environment: {} } } } }))
    const one = relayedTools("ses_one", thread)!
    const two = relayedTools("ses_two", thread)!
    expect(one.url).toStartWith("http://127.0.0.1:")
    const [mine, claimed, theirs] = await Promise.all([call(one, 7), call(one, 7, "ses_two"), call(two, 7)])
    expect(mine.body).toEqual({ jsonrpc: "2.0", id: 7, result: { content: [{ type: "text", text: "ses_one" }] } })
    expect(claimed.body.result.content[0].text).toBe("ses_one")
    expect(theirs.body.result.content[0].text).toBe("ses_two")
  })

  test("two of its connections (Codex has one a thread) using the same request's number: a cancel stops only its own connection's call", async () => {
    // The program, holding each call until it's cancelled, then saying so (and noting each it holds).
    const program = join(base, "holds.js")
    const holding = join(base, "holding.txt")
    writeFileSync(program, `let buffer = ""
const held = new Set()
process.stdin.on("data", (chunk) => {
  buffer += chunk
  let at
  while ((at = buffer.indexOf("\\n")) >= 0) {
    const message = JSON.parse(buffer.slice(0, at))
    buffer = buffer.slice(at + 1)
    if (message.method === "initialize") process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: {} }) + "\\n")
    else if (message.method === "tools/call") held.add(message.id) && require("node:fs").appendFileSync(${JSON.stringify(holding)}, message.id + "\\n")
    else if (message.method === "notifications/cancelled" && held.delete(message.params.requestId)) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.params.requestId, result: { content: [{ type: "text", text: "cancelled" }] } }) + "\\n")
  }
})
`)
    setToolProgram(JSON.stringify({ mcp: { servers: { "kuvo-browser": { command: [process.execPath, program], environment: {} } } } }))
    const tools = relayedTools("ses_one", thread)!
    const post = (body: unknown, connection?: string) =>
      fetch(tools.url, { method: "POST", headers: { ...tools.headers, "Content-Type": "application/json", Accept: "application/json, text/event-stream", ...(connection ? { "mcp-session-id": connection } : {}) }, body: JSON.stringify(body) })
    const connect = async () => (await post({ jsonrpc: "2.0", id: 0, method: "initialize", params: {} })).headers.get("mcp-session-id")!
    const [first, second] = [await connect(), await connect()]
    expect(first).not.toBe(second)
    const call = (connection: string) => post({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "show_in_panel", arguments: {} } }, connection).then((response) => response.json() as Promise<any>)
    let secondDone = false
    const firstCall = call(first)
    const secondCall = call(second).then((answer) => ((secondDone = true), answer))
    while ((existsSync(holding) ? readFileSync(holding, "utf8").trim().split("\n").length : 0) < 2) await Bun.sleep(5)
    await post({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: 1 } }, first)
    expect((await firstCall).result.content[0].text).toBe("cancelled")
    expect(secondDone).toBe(false)
    await post({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: 1 } }, second)
    expect((await secondCall).result.content[0].text).toBe("cancelled")
  })

  test("a key it didn't give, or one whose session is done, reaches nothing", async () => {
    setToolProgram(JSON.stringify({ mcp: { servers: { "kuvo-browser": { command: [process.execPath, "-e", ""], environment: {} } } } }))
    const one = relayedTools("ses_one", thread)!
    expect((await call({ url: one.url, headers: { Authorization: "Bearer nope" } }, 1)).status).toBe(401)
    dropRelayed("ses_one", thread)
    expect((await call(one, 1)).status).toBe(401)
  })

  test("a session's other agent keeps reaching its tools when one is done with them (an account switched, say)", async () => {
    setToolProgram(JSON.stringify({ mcp: { servers: { "kuvo-browser": { command: [process.execPath, "-e", ""], environment: {} } } } }))
    const other = {}
    relayedTools("ses_one", thread)
    const one = relayedTools("ses_one", other)!
    dropRelayed("ses_one", thread)
    expect((await call({ url: one.url, headers: { Authorization: "Bearer nope" } }, 1)).status).toBe(401)
    expect((await call(one, 1)).status).not.toBe(401)
    dropRelayed("ses_one", other)
    expect((await call(one, 1)).status).toBe(401)
  })
})

describe("on this Mac: the app's server", () => {
  test("each call goes to the app's server with the app's password and its own session, whatever it says", async () => {
    const seen: { authorization: string | null; session: string | null; claims: unknown }[] = []
    const app = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      async fetch(request) {
        const body: any = await request.json()
        seen.push({ authorization: request.headers.get("authorization"), session: request.headers.get("x-kuvo-session"), claims: body.params?._meta?.["ai.opencode/sessionID"] })
        return Response.json({ jsonrpc: "2.0", id: body.id, result: { content: [] } })
      },
    })
    try {
      setToolServer({ url: `http://127.0.0.1:${app.port}/mcp`, token: "app-password", timeout: 10_000 })
      const one = relayedTools("ses_one", thread)!
      expect(one.headers.Authorization).not.toContain("app-password")
      const answer = await call({ ...one, headers: { ...one.headers, "X-Kuvo-Session": "ses_two" } }, 3, "ses_two")
      expect(answer.body).toEqual({ jsonrpc: "2.0", id: 3, result: { content: [] } })
      expect(seen).toEqual([{ authorization: "Bearer app-password", session: "ses_one", claims: "ses_one" }])
    } finally {
      app.stop(true)
    }
  })
})

test("desktop compatibility tools keep the same operation while their box HTTP endpoint is replaced", async () => {
  const run = mkdtempSync(join(base, "tool-http-"))
  const seen: any[] = []
  let started = false
  let box = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(null, { status: 503 }) })
  const record = () => writeFileSync(join(run, "tool-http.json"), JSON.stringify({ port: box.port, password: "fixture" }))
  record()
  await box.stop(true)
  setToolRelayRun(run)
  setToolServer({ url: "http://127.0.0.1:9/mcp", token: "app-password", timeout: 5000 })
  const tools = agentToolsFor("ses_one", thread, false)!
  if (tools.kind !== "stdio") throw new Error("Expected the desktop forwarder")
  const child = Bun.spawn([tools.command, ...tools.args], { env: { ...process.env, ...tools.env }, stdin: "pipe", stdout: "pipe" })
  const output = child.stdout.getReader()
  try {
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "send_message", arguments: { to: "ses_two", text: "test" } } }) + "\n")
    child.stdin.flush()
    await Bun.sleep(250)
    box = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
      seen.push(await request.json())
      if (!started) {
        started = true
        return Response.json({ retryToolCall: true }, { status: 503 })
      }
      return Response.json({ text: { unexpected: true }, message: "Sent.", isError: false })
    } })
    record()
    expect(JSON.parse(new TextDecoder().decode((await output.read()).value)).result.content[0].text).toBe("Sent.")
    expect(seen).toHaveLength(2)
    expect(seen[0].session).toBe("ses_one")
    expect(seen[0].operation).toMatch(/^[a-f0-9-]{36}$/)
    expect(seen[0]).toEqual(seen[1])
  } finally {
    output.releaseLock()
    child.stdin.end()
    await child.exited
    await box.stop(true)
  }
})

describe("this Mac's app's own server", () => {
  test("where the app says it is is noted for the box's sandbox to refuse, as Kuvo Code's other servers, and forgotten when it's gone", () => {
    const run = join(base, "run")
    const fence = join(run, "fence")
    const record = join(run, "tools.json")
    mkdirSync(fence, { recursive: true })
    process.env.KUVO_FENCE = fence
    try {
      noteToolServer("http://127.0.0.1:4567/mcp")
      // (As the box's link reads it: link/fence/state.ts.)
      expect(JSON.parse(readFileSync(record, "utf8"))).toEqual({ port: 4567 })
      noteToolServer(undefined)
      expect(existsSync(record)).toBe(false)
    } finally {
      delete process.env.KUVO_FENCE
    }
  })
})
