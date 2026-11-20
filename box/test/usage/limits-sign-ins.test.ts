import "../safety"
import { claudeProbe } from "./claude-probe"
import { UsageAccounts } from "../../src/usage/accounts"
// The rings follow Settings › Accounts (#778): each computer's are exactly the accounts signed in
// there, as its Accounts shows them, each with what's left; one signed out is gone at once, and
// what was read of it with it; a signed-in Claude Code has its ring before any turn told its
// limits; a ring never shows another provider's windows; and nothing kept from before is shown
// before the agents have said who's signed in. Cursor is a stand-in (support/cursor-standin.ts),
// and each agent's own word on its sign-in (Cursor's SDK, Claude Code's `auth status`) is the
// test's, as the agent would answer it.

import { afterAll, afterEach, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { Account, Agent } from "../../src/agents/agent"
import { ClaudeAgent } from "../../src/agents/agents/claude"
import { CursorAgent } from "../../src/agents/agents/cursor"
import { subscribe } from "../../src/agents/events"
import { agentAccounts, Limits, limitsRoutes, type LimitedAccount } from "../../src/usage/limits"
import { signedInAgents, SignIns } from "../../src/usage/sign-ins"
import { CursorStandIn } from "../agents/support/cursor-standin"

const { ringsOn } = await import("../../src/link/at-work")

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-limits-sign-ins-test-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

const keychain = ClaudeAgent.keychain
let cursor: CursorStandIn
beforeEach(() => {
  delete process.env.CURSOR_API_KEY
  cursor = new CursorStandIn()
})
afterEach(() => {
  cursor.stop()
  ClaudeAgent.keychain = keychain
})

/** Cursor as the agent has it, its sign-in in its folder; whether it's signed in, as Cursor's SDK
 * would answer the agent (`turnedDown`: Cursor said no, though the key is still in its folder). */
class TestCursor extends CursorAgent {
  override readonly limits = new UsageAccounts(this.context.home, true).source("cursor", this.providerID, this.instance).limits!
  turnedDown = false
  /** Its check fails (Cursor out of reach), or never answers. */
  failing: "throws" | "hangs" | undefined
  override async account(): Promise<Account> {
    if (this.failing === "throws") throw new Error("Couldn't reach Cursor.")
    if (this.failing === "hangs") return new Promise(() => {})
    if (!existsSync(join(this.folder, "auth.json"))) return { installed: true, signedIn: false, message: "Sign in with your Cursor account." }
    return this.turnedDown ? { installed: true, signedIn: false, message: "Cursor turned the sign-in down. Sign in again." } : { installed: true, signedIn: true, label: "you@example.com" }
  }
  override async signOut() {
    rmSync(join(this.folder, "auth.json"), { force: true })
  }
}

function cursorAgent(home: string, key?: string): TestCursor {
  const folder = join(home, "cursor")
  mkdirSync(folder, { recursive: true })
  if (key) writeFileSync(join(folder, "auth.json"), JSON.stringify({ version: 1, backendUrl: "https://api2.cursor.sh", apiKey: key, createdAtMs: 1 }))
  return new TestCursor({ home, temporary: home, deps: { ensure: async () => home, load: async () => ({}) } } as any)
}

/** A Mac's Claude Code, Kuvo Code's own token for it in its folder (`claude setup-token`). */
class TestClaude extends ClaudeAgent {
  override readonly limits = claudeProbe(this, "2.1.300")
  override executable() {
    return "/bin/true"
  }
  override async account(): Promise<Account> {
    return existsSync(this.tokenFile) ? { installed: true, signedIn: true, label: "Claude plan" } : { installed: true, signedIn: false, message: "Sign in to Claude Code." }
  }
}

function claudeAgent(home: string, token?: string): TestClaude {
  if (token) {
    mkdirSync(join(home, "claude", "tokens"), { recursive: true })
    writeFileSync(join(home, "claude", "tokens", "default.json"), JSON.stringify({ token, made: 1 }))
  }
  return new TestClaude({ home, temporary: home, deps: {} as any })
}

/** This box's limits as the bridge has them (main.ts): its agents' accounts that are signed in. */
function limitsOf(agents: Agent[], record: SignIns, file?: string, others: () => LimitedAccount[] = () => []): Limits {
  const limits = new Limits(() => [...agentAccounts(signedInAgents(agents, record)), ...others()], file, Date.now, () => record.round(agents), () => record.allSaid(agents))
  record.changed = () => {
    limits.ownChanged()
    void limits.tick()
  }
  return limits
}

const rows = async (limits: Limits) => ((await (await limitsRoutes(limits, "GET", [])).json()) as any).data.rows as any[]

/** The menu bar's rings for this computer (`kuvo/box/at-work`), as the link reads them from the bridge. */
async function menuBarRings(limits: Limits) {
  const bridge = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: async (request) => (new URL(request.url).pathname === "/api/kuvo/limits" ? Response.json(((await (await limitsRoutes(limits, "GET", [])).json()) as any).data) : new Response("no", { status: 404 })) })
  try {
    return (await ringsOn({ port: bridge.port, password: "test" } as any)).filter((each) => each.on.includes("here")).map((each) => each.rings.title)
  } finally {
    bridge.stop(true)
  }
}

test("signed out of Cursor in Accounts, its ring is gone at once, and what was read of it with it", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const agent = cursorAgent(mkdtempSync(join(base, "box-")), "key_free")
  const record = new SignIns()
  const limits = limitsOf([agent], record)
  await limits.tick()
  expect((await rows(limits)).map((row) => [row.title, row.windows.length > 0])).toEqual([["Cursor", true]])
  expect(await menuBarRings(limits)).toEqual(["Cursor"])

  const told: any[] = []
  const stop = subscribe((each) => each.type === "kuvo.limits.updated" && told.push(each.data))
  // As the bridge's sign-out does (server.ts, `disconnect`): signed out, then asked.
  await agent.signOut()
  await record.ask(agent)
  stop()
  expect(told.at(0)?.rows).toEqual([])
  expect(await rows(limits)).toEqual([])
  expect(await menuBarRings(limits)).toEqual([])

  // Signed in again, nothing of before comes back before it's read: Cursor turns this read away.
  await limits.tick()
  cursor.keys.set("key_again", "auth0|user_free")
  cursor.usageAnswers = [500, 500]
  writeFileSync(join(agent.folder, "auth.json"), JSON.stringify({ version: 1, backendUrl: "https://api2.cursor.sh", apiKey: "key_again", createdAtMs: 2 }))
  await record.ask(agent)
  await limits.readAgain(agent.providerID)
  const [row] = await rows(limits)
  expect(row).toMatchObject({ title: "Cursor", windows: [], rings: {}, problem: { kind: "failed" } })
})

test("Accounts' word on Cursor is the rings': its own key turned down there, no ring, though the key still reads; the Cursor app's sign-in is never read", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const home = mkdtempSync(join(base, "box-"))
  // The Cursor app's own login on this computer, where the app keeps it: never Kuvo Code's.
  const app = join(home, "Library", "Application Support", "Cursor", "User", "globalStorage")
  mkdirSync(app, { recursive: true })
  writeFileSync(join(app, "storage.json"), JSON.stringify({ "cursorAuth/accessToken": "app-token", "cursorAuth/cachedEmail": "app@example.com" }))
  const signedOut = cursorAgent(home)
  const record = new SignIns()
  let limits = limitsOf([signedOut], record)
  await limits.tick()
  expect(await rows(limits)).toEqual([])
  expect(cursor.asked).toEqual([])

  const agent = cursorAgent(mkdtempSync(join(base, "box-")), "key_free")
  agent.turnedDown = true
  limits = limitsOf([agent], new SignIns())
  await limits.tick()
  expect(await rows(limits)).toEqual([])
  expect(cursor.asked).toEqual([])
})

test("a Mac's Claude Code signed in has its ring before any turn told its limits; signed out, none", async () => {
  ClaudeAgent.keychain = true
  const home = mkdtempSync(join(base, "box-"))
  const agent = claudeAgent(home, "token-own")
  const record = new SignIns()
  const limits = limitsOf([agent], record)
  await limits.tick()
  const [row] = await rows(limits)
  expect(row).toMatchObject({ title: "Claude", service: "claude", agent: "claude", windows: [], rings: {} })
  expect(row.boxes.map((box: any) => box.here)).toEqual([true])
  expect(await menuBarRings(limits)).toEqual(["Claude"])

  // A turn tells its limits: the ring shows what's left.
  agent.limits.heard!({ id: "five_hour", used: 0.3, minutes: 300 })
  expect((await rows(limits))[0].rings.short).toMatchObject({ window: "five_hour", used: 0.3 })

  rmSync(agent.tokenFile)
  await record.ask(agent)
  expect(await rows(limits)).toEqual([])
})

test("what was kept from before is never shown before the agents have said who's signed in, and one signed out since goes from the file", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const home = mkdtempSync(join(base, "box-"))
  const file = join(home, "limits.json")
  const agent = cursorAgent(home, "key_free")
  let limits = limitsOf([agent], new SignIns(), file)
  await limits.tick()
  expect((await rows(limits))[0].windows.length).toBeGreaterThan(0)
  limits.stop()

  // Signed out while the bridge was off (in Cursor's folder): a fresh start lists none of it.
  await agent.signOut()
  const record = new SignIns()
  limits = limitsOf([agent], record, file)
  expect(await rows(limits)).toEqual([])
  await limits.tick()
  expect(await rows(limits)).toEqual([])
  expect(Object.keys(JSON.parse(readFileSync(file, "utf8")).accounts)).toEqual([])
})

test("each ring has its own provider's windows only: Cursor's own Grok Bot week stays Cursor's, and Grok's row has Grok's", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const agent = cursorAgent(mkdtempSync(join(base, "box-")), "key_free")
  const grok: LimitedAccount = {
    provider: "grok",
    service: "grok",
    agent: { kind: "grok", account: "default" },
    limits: { every: 600_000, read: async () => ({ windows: [{ id: "week", used: 0.4, minutes: 10_080 }] }), identity: () => "xai:someone" },
  }
  const limits = limitsOf([agent], new SignIns(), undefined, () => [grok])
  await limits.tick()
  const listed = Object.fromEntries((await rows(limits)).map((row) => [row.title, row.windows.map((window: any) => window.title)]))
  expect(listed).toEqual({ Cursor: ["Total usage", "Cursor Models", "Other Models", "On-demand", "Grok Bot usage"], Grok: ["Weekly"] })
})

test("a sign-in check that fails (offline) isn't a sign-out: the ring and its readings stay", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const agent = cursorAgent(mkdtempSync(join(base, "box-")), "key_free")
  const record = new SignIns()
  const limits = limitsOf([agent], record)
  await limits.tick()
  agent.failing = "throws"
  await record.ask(agent)
  await limits.tick()
  expect((await rows(limits)).map((row) => [row.title, row.windows.length > 0])).toEqual([["Cursor", true]])
})

test("an agent that never says how its sign-in stands holds up no reading: the others are read, and it's as it was", async () => {
  cursor.keys.set("key_free", "auth0|user_free")
  const hung = cursorAgent(mkdtempSync(join(base, "box-")), "key_free")
  const grok: LimitedAccount = { provider: "grok", service: "grok", agent: { kind: "grok", account: "default" }, limits: { every: 600_000, read: async () => ({ windows: [{ id: "week", used: 0.4, minutes: 10_080 }] }) } }
  hung.failing = "hangs"
  const limits = limitsOf([hung], new SignIns(1), undefined, () => [grok])
  await limits.tick()
  expect((await rows(limits)).map((row) => row.title)).toEqual(["Grok"])
})
