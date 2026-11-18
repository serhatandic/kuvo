import "../safety"
import { expect, test } from "bun:test"
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
  let state: any = "pending", reads = 0
  const attempt = { url: "https://example.test", instructions: "", mode: "auto" as const, status: () => ({ status: state }), cancel() { state = "failed" } }
  // (Signing in asks the agent how its account stands before reading its limits: sign-ins.ts.)
  const agent = {
    providerID: "codex-cli",
    account: async () => ({ installed: true, signedIn: true }),
    models: async () => [],
    signIn: async () => attempt,
    signInWithKey: async () => {},
  }
  // (Each read is waited for as it happens: the agent is asked first, so it's a few turns on.)
  let readNow = () => {}
  const read = () => new Promise<void>((resolve) => (readNow = resolve))
  let invalidations = 0
  const limits = {
    invalidate: () => { invalidations++ },
    readAgain: async () => { reads++; readNow(); throw Error("maker unavailable") },
  }
  const answer = handler({ password: "pw", roster: { find: () => agent }, limits } as any)
  const request = (path: string) => answer(new Request("http://agent/api/integration/codex-cli/connect/" + path, { method: "POST", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" }, body: "{}" }))
  expect((await request("oauth")).status).toBe(200)
  const oauthRead = read()
  state = "complete"
  await oauthRead
  expect(reads).toBe(1)
  const keyRead = read()
  expect((await request("key")).status).toBe(200)
  await keyRead
  expect(reads).toBe(2)
  expect(invalidations).toBe(2)
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
