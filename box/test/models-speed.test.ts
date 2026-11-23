// The speed you pick for a provider (PROTOCOL.md, "The models offered", `models.speed`): kept by the
// box, so every client's picker shows the same, and Codex's turns run at it with no client there
// (test/agents/codex.test.ts has its turns). Real: the routes, the box's file and the model rule;
// stand-ins: the engine's and the coding agents' bridge's lists, over HTTP. Only Codex lists speeds
// (its service tiers); the other agents and the engine have none a box can set, so none is listed.

import "./safety"
import { expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { modelsSpeedRoute, setSpeed } from "../src/link/areas/models-speed"
import { modelsOfferedRoute } from "../src/link/areas/models-offered"
import { MODELS_FILE, setStarred } from "../src/link/areas/models-starred"
import { forgetModelLists } from "../src/link/work/model-rule"
import { pickedSpeed } from "../src/shared/model-speeds"
import type { RouteContext } from "../src/link/server/context"

const FAST = { id: "priority", name: "Fast", description: "1.5x speed" }

/** What the route answers `method` with `body` (sent as a client sends it). */
async function speedRoute(method: string, body?: unknown): Promise<{ status: number; value: any }> {
  let said = { status: 0, value: undefined as any }
  const context = {
    request: { method, path: "kuvo/models/speed", headers: {}, body: body === undefined ? null : Buffer.from(typeof body === "string" ? body : JSON.stringify(body)) },
    json: (status: number, value: unknown) => { said = { status, value }; return [] },
  } as unknown as RouteContext
  for await (const _ of modelsSpeedRoute(context)) {}
  return said
}

/** `kuvo/models/offered` as a picker reads it, from an engine with one model and a bridge with
 * Codex's own account and one you added ("Work"), whose models list their speeds as the bridge does. */
async function offered(): Promise<any> {
  const serve = (agents: boolean) => Bun.serve({ port: 0, fetch(request) {
    const route = new URL(request.url).pathname
    const account = (id: string, name: string, accountID: string, accountLabel?: string) => ({
      id, name, methods: [{ type: "oauth", id: "chatgpt" }], connections: [{}],
      metadata: { source: "agent", agent: "codex", accountID, ...(accountLabel ? { accountLabel } : {}), modelsComplete: true },
    })
    const data = !agents
      ? route === "/api/provider" ? [{ id: "anthropic" }]
        : route === "/api/model" ? [{ providerID: "anthropic", id: "claude-opus-5-5", name: "Claude Opus 5.5", variants: [] }]
        : route === "/api/integration" ? [{ id: "anthropic", name: "Anthropic", methods: [{ type: "key" }], connections: [{}] }]
        : null
      : route === "/api/provider" ? [{ id: "codex-cli" }, { id: "codex-cli@work" }]
        : route === "/api/model" ? [
            { providerID: "codex-cli", id: "gpt-6-sol", name: "GPT-6 Sol", variants: [{ id: "low" }], speeds: [FAST] },
            { providerID: "codex-cli", id: "gpt-mini", name: "GPT Mini", variants: [] },
            { providerID: "codex-cli@work", id: "gpt-6-sol", name: "GPT-6 Sol", variants: [{ id: "low" }], speeds: [FAST] },
          ]
        : route === "/api/integration" ? [account("codex-cli", "Codex", "default"), account("codex-cli@work", "Codex · Work", "work", "Work")]
        : route === "/api/kuvo/auto" ? { lists: [] }
        : null
    return Response.json({ data })
  } })
  const engine = serve(false), agents = serve(true)
  const target = (server: typeof engine) => ({ port: server.port!, password: "test", pid: process.pid })
  let answer: any
  const context = {
    request: { method: "GET", path: "kuvo/models/offered", headers: {} },
    readEngine: () => target(engine),
    bridge: () => target(agents),
    json: (status: number, value: unknown) => { expect(status).toBe(200); answer = value; return [] },
  } as unknown as RouteContext
  forgetModelLists()
  try {
    for await (const _ of modelsOfferedRoute(context)) {}
  } finally {
    engine.stop(true)
    agents.stop(true)
    forgetModelLists()
  }
  return answer
}

test("Fast picked for a Codex account is what every picker is then told, and what its turns read; Standard takes it back", async () => {
  rmSync(MODELS_FILE, { force: true })
  try {
    // Never picked: every model's speeds, and no provider says one.
    const before = await offered()
    expect(before.models.map((model: any) => [model.providerID, model.id, model.speeds ?? null])).toEqual([
      ["anthropic", "claude-opus-5-5", null],
      ["codex-cli", "gpt-6-sol", [FAST]],
      ["codex-cli", "gpt-mini", null],
      ["codex-cli@work", "gpt-6-sol", [FAST]],
    ])
    expect(before.providers.map((provider: any) => [provider.id, provider.speed ?? null])).toEqual([["anthropic", null], ["codex-cli", null], ["codex-cli@work", null]])
    expect(pickedSpeed("codex-cli@work")).toBeUndefined()

    expect(await speedRoute("POST", { providerID: "codex-cli@work", speed: "priority" })).toEqual({ status: 200, value: { providerID: "codex-cli@work", speed: "priority" } })
    // Read again, as the next client to ask would: that account's, not Codex's own.
    const fast = await offered()
    expect(fast.providers.map((provider: any) => [provider.id, provider.speed ?? null])).toEqual([["anthropic", null], ["codex-cli", null], ["codex-cli@work", "priority"]])
    // And as the bridge reads it as each turn starts, from the box's file.
    expect(pickedSpeed("codex-cli@work")).toBe("priority")
    expect(pickedSpeed("codex-cli")).toBeUndefined()

    expect(await speedRoute("POST", { providerID: "codex-cli@work", speed: null })).toEqual({ status: 200, value: { providerID: "codex-cli@work", speed: null } })
    const standard = await offered()
    expect(standard.providers.find((provider: any) => provider.id === "codex-cli@work")).not.toHaveProperty("speed")
    // Standard picked yourself, not none picked: its turns ask Codex for its standard speed.
    expect(pickedSpeed("codex-cli@work")).toBeNull()
  } finally {
    rmSync(MODELS_FILE, { force: true })
  }
})

test("a pick that doesn't say a provider and a speed is refused with words, and what was picked stays", async () => {
  rmSync(MODELS_FILE, { force: true })
  try {
    expect((await speedRoute("POST", { providerID: "codex-cli", speed: "priority" })).status).toBe(200)
    for (const body of [{}, { providerID: "codex-cli" }, { speed: "priority" }, { providerID: "", speed: "priority" }, { providerID: "codex-cli", speed: 5 }, { providerID: "codex-cli", speed: "very fast" }, "not json", undefined]) {
      const answer = await speedRoute("POST", body)
      expect(answer.status).toBe(400)
      expect(answer.value.message).toBe("Say which provider (providerID) and its speed (speed: a speed's id, or null for Standard).")
    }
    expect((await speedRoute("GET")).status).toBe(405)
    expect(pickedSpeed("codex-cli")).toBe("priority")
  } finally {
    rmSync(MODELS_FILE, { force: true })
  }
})

test("speeds and starred models are kept in the same file: picking one leaves the other", () => {
  const file = join(mkdtempSync(join(tmpdir(), "kuvo-speed-")), "models.json")
  setStarred({ providerID: "codex-cli", id: "gpt-6-sol" }, true, file)
  setSpeed("codex-cli", "priority", file)
  setStarred({ providerID: "anthropic", id: "claude-opus-5-5" }, true, file)
  setSpeed("codex-cli@work", null, file)
  const kept = JSON.parse(readFileSync(file, "utf8"))
  expect(kept.starred).toEqual([{ providerID: "codex-cli", id: "gpt-6-sol" }, { providerID: "anthropic", id: "claude-opus-5-5" }])
  expect(kept.speeds).toEqual({ "codex-cli": "priority", "codex-cli@work": null })
})
