// Whether you've had Kuvo Code's first run with a box (#733, PROTOCOL.md "Your setup", `welcomed`):
// kept on the box, so every client of it agrees, and a client installed again doesn't welcome you
// again. Through the box's own routes, as a client asks them.

import "./safety"
import { expect, test } from "bun:test"
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { RUN } from "../src/link/home"

test("the first run is done once a client says so, for every client of the box, and can be had again", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const handler = engineHandler(() => { throw new Error("no engine") }, { name: "test" }, () => { throw new Error("none") })
  const ask = async (method: string, path: string, body?: unknown, from: { fromApp?: boolean } = {}) => {
    let status = 0, text = ""
    for await (const piece of handler({ method, path, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), ...from }, undefined)) {
      if ("head" in piece) status = piece.head.status
      if ("body" in piece) text += Buffer.from(piece.body).toString()
    }
    return { status, body: JSON.parse(text || "{}") }
  }
  const welcomed = async () => (await ask("GET", "kuvo/setup")).body.welcomed

  expect(await welcomed()).toBe(false)
  // The desktop app finishes its first run: the phone, asking the same box, isn't welcomed again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true }, { fromApp: true })).toEqual({ status: 200, body: { welcomed: true } })
  expect(await welcomed()).toBe(true)
  expect(existsSync(join(RUN, "welcomed.json"))).toBe(true)
  // Said again, it stays as it was.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true })).toEqual({ status: 200, body: { welcomed: true } })
  // Refused: nothing said, or not a yes or no; and only POST.
  expect((await ask("POST", "kuvo/setup/welcomed", {})).status).toBe(400)
  expect((await ask("POST", "kuvo/setup/welcomed", { welcomed: "yes" })).status).toBe(400)
  expect((await ask("GET", "kuvo/setup/welcomed")).status).toBe(405)
  expect(await welcomed()).toBe(true)
  // A client following the setup hears the change as it's said.
  const following = new AbortController()
  const lines: any[] = []
  const heard = (async () => {
    let text = ""
    for await (const piece of handler({ method: "GET", path: "kuvo/setup/events", headers: {}, body: undefined }, undefined, following.signal)) {
      if (!("body" in piece)) continue
      text += Buffer.from(piece.body).toString()
      const done = text.split("\n").slice(0, -1)
      text = text.slice(text.lastIndexOf("\n") + 1)
      lines.push(...done.filter(Boolean).map(line => JSON.parse(line)))
      if (lines.length === 1) await ask("POST", "kuvo/setup/welcomed", { welcomed: false })
      if (lines.length >= 2) break
    }
    following.abort()
  })()
  await heard
  expect(lines.map(line => line.welcomed)).toEqual([true, false])
  expect(await welcomed()).toBe(false)
  // Said again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true })).toEqual({ status: 200, body: { welcomed: true } })
  // To be had again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: false })).toEqual({ status: 200, body: { welcomed: false } })
  expect(await welcomed()).toBe(false)
  expect(existsSync(join(RUN, "welcomed.json"))).toBe(false)
})

// Whether a first run has anything to ask about your accounts (`kuvo/setup/ready`, `setup.ready`):
// decided by the box, so no client keeps the rule. From what it lists: the models that run, and
// how each plan stands.
import { firstRunReady } from "../src/link/setup/ready"
import { forgetModelLists, type Listed, type Lists } from "../src/link/work/model-rule"

const model = (providerID: string, id: string, free = false): Listed => ({ providerID, id, name: id, aliases: [], efforts: [], free, provisional: false, hidden: false, retiring: false, released: 1 })
const account = (id: string, signedIn: boolean, methods: string[][] = [["oauth", "browser"]]) => ({ id, name: id, methods, signedIn, installed: true, complete: true })
const oauth = [{ type: "oauth", id: "browser", label: "Sign in" }]
const signedIn = [{ type: "credential", id: "cred_1", method: "oauth", label: "you@example.com" }]

/** What a box lists: the engine with OpenCode Zen's free model and (signed in) Copilot's; the
 * bridge with Claude Code's, signed in or not. */
function listing({ claude = true, copilot = false, ended = false }: { claude?: boolean; copilot?: boolean; ended?: boolean } = {}) {
  const lists: Lists = {
    engine: {
      ready: new Set(copilot ? ["opencode", "github-copilot"] : ["opencode"]),
      models: [model("opencode", "big-pickle", true), model("github-copilot", "gpt-6")],
      accounts: new Map([["github-copilot", account("github-copilot", copilot, [["oauth", "device"]])]]),
      fallback: null,
    },
    bridge: { models: [model("claude-code", "opus")], accounts: new Map([["claude-code", account("claude-code", claude)]]), auto: [] },
  }
  const engine = [
    { id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth", id: "device", label: "Login" }], connections: copilot ? signedIn : [] },
    { id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [] },
  ]
  const agents = [{ id: "claude-code", name: "Claude Code", methods: oauth, connections: claude ? signedIn : [], metadata: { agent: "claude", installed: true, ...(ended ? { signInEnded: true } : {}) } }]
  return { lists, engine, agents }
}
const said = (given: ReturnType<typeof listing>, ended = new Map<string, string>()) => firstRunReady(given.lists, given.engine, given.agents, ended, "Studio")

test("a first run has nothing to ask where a model on one of your accounts runs and no plan needs you", () => {
  // Claude Code signed in: its models run.
  expect(said(listing())).toEqual({ ready: true, runs: true, needsYou: [], pending: [] })
  // An engine plan signed in does as well, with no coding agent signed in.
  expect(said(listing({ claude: false, copilot: true }))).toEqual({ ready: true, runs: true, needsYou: [], pending: [] })
})

test("it has something to ask where only the free models run, a plan's sign-in ended, or a side can't say", () => {
  // Nothing signed in: OpenCode Zen's free model runs, which isn't an account's.
  expect(said(listing({ claude: false }))).toEqual({ ready: false, runs: false, needsYou: [], pending: [] })
  // A coding agent's sign-in ended: it needs you, whatever else runs.
  expect(said(listing({ copilot: true, ended: true }))).toEqual({ ready: false, runs: true, needsYou: ["claude-code"], pending: [] })
  // An engine plan's saved sign-in the box saw end.
  expect(said(listing({ copilot: true }), new Map([["github-copilot", "cred_1"]]))).toEqual({ ready: false, runs: true, needsYou: ["github-copilot"], pending: [] })
  // The coding agents' bridge can't say now (starting): never ready, though the engine's run.
  const given = listing({ copilot: true })
  expect(firstRunReady({ ...given.lists, bridge: null }, given.engine, null, new Map(), "Studio")).toEqual({ ready: false, runs: true, needsYou: [], pending: ["agents"] })
  expect(firstRunReady({ ...given.lists, engine: null }, null, given.agents, new Map(), "Studio")).toMatchObject({ ready: false, pending: ["engine"] })
})

test("an account you added beside an agent's own, and one a key is entered for, never hold it back", () => {
  const given = listing()
  // (A first run lists each agent once, and keys apart: what it doesn't show can't be what it waits on.)
  given.agents.push({ id: "claude-code@1f2e3d4c", name: "Claude Code · Work", methods: oauth, connections: signedIn, metadata: { agent: "claude", installed: true, signInEnded: true, accountID: "1f2e3d4c" } as any })
  given.engine.push({ id: "openai", name: "OpenAI", methods: [{ type: "key" }], connections: [{ type: "credential", id: "cred_k", method: "key", label: "API key" }] } as any)
  expect(firstRunReady(given.lists, given.engine, given.agents, new Map([["openai", "cred_k"]]), "Studio")).toEqual({ ready: true, runs: true, needsYou: [], pending: [] })
})

test("only what a first run shows and a picker offers counts", () => {
  // An MCP server's sign-in and a web search service aren't accounts Accounts lists: one that
  // needs signing in again never holds a first run back.
  const mcp = listing()
  mcp.engine.push({ id: "linear", name: "Linear", metadata: { source: "mcp" }, methods: [{ type: "oauth", id: "mcp", label: "Sign in" }], connections: [{ type: "credential", id: "cred_m", method: "oauth", label: "you", status: { status: "needs_auth" } }] } as any)
  expect(said(mcp)).toMatchObject({ ready: true, needsYou: [] })
  // A plan whose sign-in in use says something's wrong needs you, as one that ended does.
  const failing = listing({ copilot: true })
  failing.engine[0].connections = [{ type: "credential", id: "cred_1", method: "oauth", label: "you@example.com", status: { status: "failed", message: "GitHub didn't answer." } }] as any
  expect(said(failing)).toMatchObject({ ready: false, needsYou: ["github-copilot"] })
  // What a picker leaves out isn't a model that runs for you: one being retired, one its agent
  // hides, or a Custom Auto list by itself.
  const nothing = listing({ claude: false })
  for (const change of [
    (lists: Lists) => { lists.bridge!.accounts.set("claude-code", account("claude-code", true)); lists.bridge!.models = [{ ...model("claude-code", "opus"), retiring: true }] },
    (lists: Lists) => { lists.bridge!.accounts.set("claude-code", account("claude-code", true)); lists.bridge!.models = [{ ...model("claude-code", "opus"), hidden: true }] },
    (lists: Lists) => { lists.bridge!.auto = [{ id: "auto_1", name: "Everyday", entries: 2 }] },
  ]) {
    const given = structuredClone({ engine: nothing.engine, agents: nothing.agents })
    const lists: Lists = { engine: { ...nothing.lists.engine!, accounts: new Map(nothing.lists.engine!.accounts) }, bridge: { models: [...nothing.lists.bridge!.models], accounts: new Map(nothing.lists.bridge!.accounts), auto: [] } }
    change(lists)
    expect(firstRunReady(lists, given.engine, given.agents, new Map(), "Studio").runs).toBe(false)
  }
  // A model of your own (a local provider, with no account to sign in to) is one you set up: it
  // runs, and isn't one of the free ones.
  const local = listing({ claude: false })
  local.lists.engine!.ready.add("ollama")
  local.lists.engine!.models.push(model("ollama", "qwen3", true))
  expect(said(local)).toMatchObject({ ready: true, runs: true })
})

test("a coding agent signed in whose models aren't listed yet is pending, not a no", () => {
  // Codex signed in, its bridge up, none of its models listed yet: it can't say whether one runs.
  const given = listing({ claude: false })
  given.lists.bridge = { models: [], accounts: new Map([["codex-cli", { ...account("codex-cli", true), complete: false }]]), auto: [] }
  given.agents = [{ id: "codex-cli", name: "Codex", methods: oauth, connections: signedIn, metadata: { agent: "codex", installed: true } }] as any
  expect(said(given)).toEqual({ ready: false, runs: false, needsYou: [], pending: ["agents"] })
})

test("kuvo/setup/ready follows the box as it changes: signed in, then a sign-in that ends", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  let copilot = false
  const engine = Bun.serve({
    port: 0,
    fetch(request) {
      const { pathname } = new URL(request.url)
      const data = (value: unknown) => Response.json({ data: value })
      if (pathname === "/api/provider") return data(copilot ? [{ id: "opencode" }, { id: "github-copilot" }] : [{ id: "opencode" }])
      if (pathname === "/api/model") return data([{ id: "big-pickle", providerID: "opencode", name: "Big Pickle", cost: { input: 0, output: 0 } }, { id: "gpt-6", providerID: "github-copilot", name: "GPT-6", cost: { input: 1, output: 2 } }])
      if (pathname === "/api/model/default") return data(null)
      if (pathname === "/api/integration") return data([{ id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth", id: "device", label: "Login" }], connections: copilot ? signedIn : [] }])
      return Response.json({ message: "Not here." }, { status: 404 })
    },
  })
  try {
    const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any, { name: "Studio" } as any, () => { throw new Error("none") })
    const ask = async (method: string, path: string) => {
      let status = 0, text = ""
      for await (const piece of handler({ method, path, headers: {}, body: undefined, fromApp: true }, undefined)) {
        if ("head" in piece) status = piece.head.status
        if ("body" in piece) text += Buffer.from(piece.body).toString()
      }
      return { status, body: JSON.parse(text || "{}") }
    }
    expect((await ask("POST", "kuvo/setup/ready")).status).toBe(405)
    // Only the free model runs; and the coding agents aren't running, so they can't say.
    forgetModelLists()
    expect(await ask("GET", "kuvo/setup/ready")).toEqual({ status: 200, body: { ready: false, runs: false, needsYou: [], pending: ["agents"] } })
    // Signed in to Copilot (the box hears its engine say so, and reads its lists again): its
    // model runs. Still not ready here: no bridge to say how the coding agents stand.
    copilot = true
    forgetModelLists()
    expect((await ask("GET", "kuvo/setup/ready")).body).toEqual({ ready: false, runs: true, needsYou: [], pending: ["agents"] })
    // Its saved sign-in ends: said at the next ask, the accounts being read anew each time.
    mkdirSync(RUN, { recursive: true })
    writeFileSync(join(RUN, "ended-sign-ins.json"), JSON.stringify({ "github-copilot": "cred_1" }))
    expect((await ask("GET", "kuvo/setup/ready")).body).toEqual({ ready: false, runs: true, needsYou: ["github-copilot"], pending: ["agents"] })
    // Signed in again: nothing needs you.
    rmSync(join(RUN, "ended-sign-ins.json"), { force: true })
    expect((await ask("GET", "kuvo/setup/ready")).body.needsYou).toEqual([])
  } finally {
    forgetModelLists()
    rmSync(join(RUN, "ended-sign-ins.json"), { force: true })
    await engine.stop(true)
  }
})
