import "../safety"
import { expect, test } from "bun:test"
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Accounts, providerID } from "../../src/agents/accounts"
import { handler } from "../../src/agents/server"
import { Roster } from "../../src/agents/roster"
import { agentPrograms } from "../usage/support/agent-programs"

test("POST accounts with provider creates an isolated account and leaves the first folder untouched", async () => {
  const root = mkdtempSync(join(tmpdir(), "kuvo-add-account-"))
  try {
    const accounts = new Accounts(root)
    const first = accounts.add("codex", "First")
    const folder = accounts.home(first)!
    writeFileSync(join(folder, "auth.json"), "first-secret")
    const roster = { accounts, sync() {}, all: () => accounts.all().map(record => ({ id: record.kind, instance: record, providerID: providerID(record.kind, record.id) })) }
    let transferring = true
    const answer = handler({ roster, password: "pw" } as any, () => transferring)
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
    expect(refused.headers.get("x-kuvo-route-version")).toBe("1")
    expect((await refused.json() as { code: string }).code).toBe("unsupportedFormatVersion")
    const fenced = await add("1")
    expect(fenced.status).toBe(503)
    expect(fenced.headers.get("x-kuvo-route-version")).toBe("1")
    expect(fenced.headers.get("x-kuvo-bridge-handover")).toBe("1")
    expect(accounts.all().map(record => providerID(record.kind, record.id))).toEqual(before)
    transferring = false
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

test("the account index accepts legacy arrays and guards account changes against future formats", () => {
  const root = mkdtempSync(join(tmpdir(), "kuvo-add-account-"))
  try {
    const file = join(root, "accounts.json")
    const legacy = { kind: "codex", id: "saved", label: "Saved" } as const
    writeFileSync(file, JSON.stringify([legacy]))
    const accounts = new Accounts(root)
    expect(accounts.all()).toContainEqual(legacy)
    accounts.rename("codex-cli@saved", "Renamed")
    const current = JSON.parse(readFileSync(file, "utf8"))
    expect(current).toEqual([{ formatVersion: 1 }, { ...legacy, label: "Renamed" }])
    expect(current.filter((record: { kind?: string }) => record.kind === "codex")).toEqual([{ ...legacy, label: "Renamed" }])
    const future = JSON.stringify([{ formatVersion: 2 }, legacy])
    writeFileSync(file, future)
    expect(() => new Accounts(root)).toThrow("Managed accounts format version 2 is unsupported")
    expect(() => accounts.add("codex", "Another")).toThrow("Managed accounts format version 2 is unsupported")
    expect(() => accounts.remove("codex-cli@saved")).toThrow("Managed accounts format version 2 is unsupported")
    expect(readFileSync(file, "utf8")).toBe(future)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test.each(["codex", "grok"] as const)("removing an added %s account signs it out through the agent, in that account's own folder, and its folder goes", async kind => {
  const root = mkdtempSync(join(tmpdir(), "kuvo-remove-account-"))
  const programs = agentPrograms(join(root, "programs"))
  const path = process.env.PATH
  // The agent's own program, as the bridge finds it: first on the PATH.
  process.env.PATH = `${join(root, "programs")}:${path}`
  try {
    const accounts = new Accounts(join(root, "agents"))
    const record = accounts.add(kind, "Work")
    const folder = accounts.home(record)!
    writeFileSync(join(folder, "auth.json"), "{}")
    const roster = new Roster(accounts, { home: join(root, "agents"), temporary: join(root, "temporary") } as any)
    mkdirSync(join(root, "temporary"), { recursive: true })
    const answer = handler({ roster, password: "pw", sessions: { forgetProvider() {} } } as any)
    const response = await answer(new Request(`http://agent/api/kuvo/accounts/${providerID(kind, record.id)}`, { method: "DELETE", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
    expect(response.status).toBe(200)
    // Its sign-out, by the agent itself, with that account's folder (where a Keychain or keyring item is tied to it).
    expect(programs.asked()).toEqual([{ program: kind, home: folder, args: ["logout"] }])
    expect(existsSync(folder)).toBe(false)
    expect(accounts.all().some(each => each.id === record.id)).toBe(false)
    roster.all().forEach(agent => agent.shutdown?.())
  } finally {
    process.env.PATH = path
    rmSync(root, { recursive: true, force: true })
  }
})
