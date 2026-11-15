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
    const ask = async (method: string, path: string, body?: unknown) => {
      let status = 0, text = ""
      for await (const piece of handler({ method, path, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), fromApp: true }, undefined)) {
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
    expect(asked).toEqual([{ methodID: "device" }])
    expect((await ask("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_1")).body.userCode).toBe("8F2A-91KQ")
    // Refused as it should be: no way in named, another method, an account the engine hasn't.
    expect((await ask("POST", "kuvo/sign-in/start?integration=github-copilot", {})).status).toBe(400)
    expect((await ask("GET", "kuvo/sign-in/start?integration=github-copilot")).status).toBe(405)
    expect((await ask("POST", "kuvo/sign-in/start?integration=nope", { methodID: "device" })).status).toBe(404)
  } finally {
    engine.stop(true)
  }
})
