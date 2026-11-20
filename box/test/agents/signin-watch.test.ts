import "../safety"
import { expect, test } from "bun:test"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { BoxServicesClient } from "../../src/agents/box-services"
import { watchSignIn } from "../../src/agents/signin-watch"

test("completion reads without a client status request; failed and expired attempts do not", async () => {
  let state: any = "pending", reads = 0
  const attempt = { url: "", instructions: "", mode: "auto" as const, status: () => ({ status: state }), cancel() {} }
  watchSignIn(attempt, Date.now() + 1000, () => { reads++ }, 1)
  state = "complete"
  await new Promise(resolve => setTimeout(resolve, 10))
  expect(reads).toBe(1)
  state = "failed"
  watchSignIn(attempt, Date.now() + 1000, () => { reads++ })
  state = "expired"
  watchSignIn(attempt, Date.now() + 1000, () => { reads++ })
  expect(reads).toBe(1)
})

test("the bridge follows OAuth completion without polling and key sign-in survives a rejected limits read", async () => {
  const { handler } = await import("../../src/agents/server")
  let state: any = "pending"
  let signIn = "0123456789abcdef"
  const attempt = { url: "https://example.test", instructions: "", mode: "auto" as const, status: () => ({ status: state }), cancel() { state = "failed" } }
  // (Signing in asks the agent how its account stands before reading its limits: sign-ins.ts.)
  const agent = {
    providerID: "codex-cli",
    account: async () => ({ installed: true, signedIn: true }),
    usageSignIn: () => signIn,
    models: async () => [],
    signIn: async () => attempt,
    signInWithKey: async () => { signIn = "fedcba9876543210" },
  }
  // (Each read is waited for as it happens: the agent is asked first, so it's a few turns on.)
  let readNow = () => {}
  const read = () => new Promise<void>((resolve) => (readNow = resolve))
  const run = mkdtempSync(join(tmpdir(), "kuvo-signin-watch-"))
  const observations: unknown[] = []
  const services = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    async fetch(request) {
      observations.push({ path: new URL(request.url).pathname, body: await request.json() })
      readNow()
      return Response.json({ message: "maker unavailable" }, { status: 503 })
    },
  })
  try {
    writeFileSync(join(run, "box-services.json"), JSON.stringify({ pid: process.pid, port: services.port, password: "a".repeat(48) }))
    const boxServices = new BoxServicesClient(run)
    const answer = handler({ password: "pw", roster: { find: () => agent }, boxServices } as any)
    const request = (path: string) => answer(new Request("http://agent/api/integration/codex-cli/connect/" + path, { method: "POST", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" }, body: "{}" }))
    expect((await request("oauth")).status).toBe(200)
    const oauthRead = read()
    state = "complete"
    await oauthRead
    expect(observations).toEqual([
      { path: "/api/kuvo/usage-observed", body: { provider: "codex-cli", signedIn: true, signIn: "0123456789abcdef" } },
    ])
    const keyRead = read()
    expect((await request("key")).status).toBe(200)
    await keyRead
    expect(observations).toHaveLength(2)
    expect(observations[1]).toEqual({ path: "/api/kuvo/usage-observed", body: { provider: "codex-cli", signedIn: true, signIn: "fedcba9876543210" } })
    expect(boxServices.generation("codex-cli")).toBe(2)
  } finally {
    services.stop(true)
    rmSync(run, { recursive: true, force: true })
  }
})

test("cancelling stops watching even when the agent leaves its status pending", async () => {
  let asks = 0, reads = 0
  const attempt = { url: "", instructions: "", mode: "auto" as const, status: () => { asks++; return { status: "pending" as const } }, cancel() {} }
  const stop = watchSignIn(attempt, Date.now() + 1000, () => { reads++ }, 1)
  stop()
  const stoppedAt = asks
  await new Promise(resolve => setTimeout(resolve, 10))
  expect(asks).toBe(stoppedAt)
  expect(reads).toBe(0)
})
