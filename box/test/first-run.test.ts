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
  expect(said(listing())).toEqual({ ready: true, runs: true, needsYou: [] })
  // An engine plan signed in does as well, with no coding agent signed in.
  expect(said(listing({ claude: false, copilot: true }))).toEqual({ ready: true, runs: true, needsYou: [] })
})

test("it has something to ask where only the free models run, a plan's sign-in ended, or a side can't say", () => {
  // Nothing signed in: OpenCode Zen's free model runs, which isn't an account's.
  expect(said(listing({ claude: false }))).toEqual({ ready: false, runs: false, needsYou: [] })
  // A coding agent's sign-in ended: it needs you, whatever else runs.
  expect(said(listing({ copilot: true, ended: true }))).toEqual({ ready: false, runs: true, needsYou: ["claude-code"] })
  // An engine plan's saved sign-in the box saw end.
  expect(said(listing({ copilot: true }), new Map([["github-copilot", "cred_1"]]))).toEqual({ ready: false, runs: true, needsYou: ["github-copilot"] })
  // The coding agents' bridge can't say now (starting): never ready, though the engine's run.
  const given = listing({ copilot: true })
  expect(firstRunReady({ ...given.lists, bridge: null }, given.engine, null, new Map(), "Studio")).toEqual({ ready: false, runs: true, needsYou: [] })
  expect(firstRunReady({ ...given.lists, engine: null }, null, given.agents, new Map(), "Studio").ready).toBe(false)
})

test("an account you added beside an agent's own, and one a key is entered for, never hold it back", () => {
  const given = listing()
  // (A first run lists each agent once, and keys apart: what it doesn't show can't be what it waits on.)
  given.agents.push({ id: "claude-code@1f2e3d4c", name: "Claude Code · Work", methods: oauth, connections: signedIn, metadata: { agent: "claude", installed: true, signInEnded: true, accountID: "1f2e3d4c" } as any })
  given.engine.push({ id: "openai", name: "OpenAI", methods: [{ type: "key" }], connections: [{ type: "credential", id: "cred_k", method: "key", label: "API key" }] } as any)
  expect(firstRunReady(given.lists, given.engine, given.agents, new Map([["openai", "cred_k"]]), "Studio")).toEqual({ ready: true, runs: true, needsYou: [] })
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
    expect(await ask("GET", "kuvo/setup/ready")).toEqual({ status: 200, body: { ready: false, runs: false, needsYou: [] } })
    // Signed in to Copilot (the box hears its engine say so, and reads its lists again): its
    // model runs. Still not ready here: no bridge to say how the coding agents stand.
    copilot = true
    forgetModelLists()
    expect((await ask("GET", "kuvo/setup/ready")).body).toEqual({ ready: false, runs: true, needsYou: [] })
    // Its saved sign-in ends: said at the next ask, the accounts being read anew each time.
    mkdirSync(RUN, { recursive: true })
    writeFileSync(join(RUN, "ended-sign-ins.json"), JSON.stringify({ "github-copilot": "cred_1" }))
    expect((await ask("GET", "kuvo/setup/ready")).body).toEqual({ ready: false, runs: true, needsYou: ["github-copilot"] })
    // Signed in again: nothing needs you.
    rmSync(join(RUN, "ended-sign-ins.json"), { force: true })
    expect((await ask("GET", "kuvo/setup/ready")).body.needsYou).toEqual([])
  } finally {
    forgetModelLists()
    rmSync(join(RUN, "ended-sign-ins.json"), { force: true })
    await engine.stop(true)
  }
})
