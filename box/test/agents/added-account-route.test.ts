import "../safety"
import { expect, test } from "bun:test"
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Accounts, providerID } from "../../src/agents/accounts"
import { handler } from "../../src/agents/server"

test("POST accounts with provider creates an isolated account and leaves the first folder untouched", async () => {
  const root = mkdtempSync(join(tmpdir(), "kuvo-add-account-"))
  try {
    const accounts = new Accounts(root)
    const first = accounts.add("codex", "First")
    const folder = accounts.home(first)!
    writeFileSync(join(folder, "auth.json"), "first-secret")
    const roster = { accounts, sync() {}, all: () => accounts.all().map(record => ({ id: record.kind, instance: record, providerID: providerID(record.kind, record.id) })) }
    const answer = handler({ roster, password: "pw" } as any)
    const add = (version?: string) => answer(new Request("http://agent/api/kuvo/accounts", {
      method: "POST",
      headers: {
        authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"),
        "content-type": "application/json",
        ...(version === undefined ? {} : { "x-kuvo-route-version": version }),
      },
      body: JSON.stringify({ provider: providerID("codex", first.id) }),
    }))
    const before = accounts.all().map(record => providerID(record.kind, record.id))
    const refused = await add("2")
    expect(refused.status).toBe(400)
    expect((await refused.json() as { code: string }).code).toBe("unsupportedFormatVersion")
    expect(accounts.all().map(record => providerID(record.kind, record.id))).toEqual(before)
    const response = await add()
    expect(response.status).toBe(200)
    expect(response.headers.get("x-kuvo-route-version")).toBe("1")
    const added = (await response.json() as any).data.providerID
    expect(added).toMatch(/^codex-cli@[a-f0-9]{8}$/)
    expect(added).not.toBe(providerID("codex", first.id))
    const record = accounts.all().find(record => providerID(record.kind, record.id) === added)!
    expect(existsSync(accounts.home(record)!)).toBe(true)
    expect(accounts.home(record)).not.toBe(folder)
    expect(readFileSync(join(folder, "auth.json"), "utf8")).toBe("first-secret")
    expect(existsSync(join(accounts.home(record)!, "auth.json"))).toBe(false)
  } finally { rmSync(root, { recursive: true, force: true }) }
})

test.each([true, false])("conditional cancellation preserves signed-in=%s and removes only unsigned accounts", async signedIn => {
  let removed = false
  const agent = { providerID: "codex-cli@new", instance: { id: "new" }, account: async (fresh: boolean) => { expect(fresh).toBe(true); return { signedIn } } }
  const answer = handler({ password: "pw", sessions: { forgetProvider() {} }, roster: { find: () => agent, sync() {}, accounts: { remove: () => { removed = true } } } } as any)
  const response = await answer(new Request("http://agent/api/kuvo/accounts/codex-cli@new?onlyUnsigned=1", { method: "DELETE", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
  expect(response.status).toBe(200)
  expect(removed).toBe(!signedIn)
})
