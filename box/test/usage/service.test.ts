import "../safety"
import { expect, test } from "bun:test"
import { createHash } from "node:crypto"
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { CursorStandIn } from "../agents/support/cursor-standin"
import { UsageAccounts } from "../../src/usage/accounts"
import { CodexCredentials } from "../../src/usage/codex-credentials"
import { claudeUsage } from "../../src/usage/claude-limits"
import { engineEndpoints } from "../../src/usage/engine-limits"
import { agentPrograms } from "./support/agent-programs"
import type { LimitRow } from "../../src/usage/limits-rows"
import { usageReader } from "../../src/usage/service"
import { handler, type Context } from "../../src/agents/server"
import { GrokAgent } from "../../src/agents/agents/grok"
import { BoxServicesClient } from "../../src/agents/box-services"
import { servicesFile, servicesRecordFormat } from "../../src/shared/box-services"
import { bridgeRecordFormat } from "../../src/shared/records"
import { serverEvents } from "../../src/shared/event-stream"
import type { ServerEvent } from "../../src/agents/wire"
import { Limits, type LimitedAccount } from "../../src/usage/limits"

const keep = (file: string, body: unknown) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, JSON.stringify(body))
}

const publishBridge = (run: string, pid = process.pid) => keep(join(run, "bridge.json"), bridgeRecordFormat.write({ pid, port: 1, password: "fixture" }))

const jwt = (id: string) => "header." + Buffer.from(JSON.stringify({
  "https://api.openai.com/auth": { chatgpt_account_id: id, chatgpt_user_id: id },
})).toString("base64url") + ".signature"

async function until<T>(read: () => Promise<T>, done: (value: T) => boolean): Promise<T> {
  const end = Date.now() + 8_000
  let value: T
  do {
    value = await read()
    if (done(value)) return value
    await Bun.sleep(20)
  } while (Date.now() < end)
  throw new Error(`Timed out waiting for the usage reader: ${JSON.stringify(value)}`)
}

test("the limits route reads and renders all six plans from their fixture credentials and APIs", async () => {
  const box = mkdtempSync(join(tmpdir(), "kuvo-usage-rings-"))
  const run = join(box, "run"), home = join(box, "agents")
  // The first accounts are each agent's own folder, in a home of this test's own.
  const yours = process.env.HOME, own = join(box, "you")
  process.env.HOME = own
  mkdirSync(run)
  publishBridge(run)
  const at = new Date(), soon = new Date(+at + 60 * 60_000)
  const week = new Date(+at + 7 * 86400_000), month = new Date(+at + 30 * 86400_000)
  const codexFile = join(own, ".codex", "auth.json")
  const codexAuth = (id: string) => keep(codexFile, { tokens: { access_token: jwt(id), account_id: id } })
  codexAuth("codex-first")
  // Codex and Grok answer for themselves, from their folders (support/agent-programs.ts).
  const codexWindows = (fiveHour: number) => ({ rateLimitsByLimitId: { codex: { limitId: "codex", planType: "pro", primary: { usedPercent: fiveHour, windowDurationMins: 300 }, secondary: { usedPercent: 40, windowDurationMins: 10080 } } } })
  keep(join(own, ".codex", "usage.json"), { "codex-first": codexWindows(12), "codex-second": codexWindows(67) })
  keep(join(own, ".grok", "billing.json"), { config: { currentPeriod: { start: at, end: week }, creditUsagePercent: 62 } })
  const programs = agentPrograms(join(box, "programs"))
  // Claude Code signed in in its own folder: who it is, and what it says of its plan (its `/usage`).
  keep(join(own, ".claude.json"), { oauthAccount: { accountUuid: "00000000-0000-4000-8000-0000000000c1", organizationUuid: "org-c" } })
  keep(join(own, ".claude", "usage.json"), { subscription_type: "max", rate_limits: { five_hour: { utilization: 32, resets_at: soon }, seven_day: { utilization: 45, resets_at: week } } })
  keep(join(own, ".grok", "auth.json"), { grok: { key: "fixture-grok", expires_at: week } })
  keep(join(home, "cursor", "auth.json"), { version: 1, backendUrl: "https://api2.cursor.sh", apiKey: "fixture-cursor" })
  keep(join(home, "antigravity", "profile", "antigravity-acp", "acp_token.json"), { access_token: "fixture-antigravity", expiry: week })
  const cursor = new CursorStandIn()
  cursor.keys.set("fixture-cursor", "auth0|fixture")
  cursor.usage = { enabled: true, billingCycleStart: +at, billingCycleEnd: +month, planUsage: { limit: 2000, totalSpend: 1300, totalPercentUsed: 65 } }
  cursor.grokBot = 404
  const called = new Set<string>()
  const fixture = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
    const route = new URL(request.url).pathname
    const using = request.headers.get("authorization")
    called.add(route)
    switch (route) {
      case "/go":
        expect(using).toBe("Bearer fixture-go")
        return Response.json({ usage: { rolling: { percent: 17 }, weekly: { percent: 33 }, monthly: { percent: 79 } } })
      case "/v1internal:retrieveUserQuotaSummary":
        expect(using).toBe("Bearer fixture-antigravity")
        return Response.json({ groups: [
          { buckets: [{ bucketId: "gemini-5h", remainingFraction: 0.75 }, { bucketId: "gemini-weekly", remainingFraction: 0.5 }] },
          { buckets: [{ bucketId: "3p-5h", remainingFraction: 0.8 }, { bucketId: "3p-weekly", remainingFraction: 0.6 }] },
        ] })
      case "/v1internal:loadCodeAssist": return Response.json({ paidTier: { id: "g1-pro-tier" } })
      default: return new Response(null, { status: 404 })
    }
  } })
  const base = `http://127.0.0.1:${fixture.port}`
  const endpoints = { ...engineEndpoints }, claude = claudeUsage.url
  Object.assign(engineEndpoints, { opencodeGo: base + "/go" })
  claudeUsage.url = base + "/claude"
  const accounts = new UsageAccounts(home, { claudeKeychain: false, antigravity: { bases: [base] }, programs, codexKeychain: async () => undefined })
  const reader = usageReader({ box, run, home, bots: join(box, "bots") }, {
    accounts: () => accounts.all(),
    engineCredentials: async () => [{ integrationID: "opencode-go", active: true, value: { type: "api", key: "fixture-go" } }],
  })
  const refresh = async () => {
    const response = await reader.handle(new Request("http://box/api/kuvo/limits/refresh", { method: "POST" }))
    expect(response.status).toBe(200)
    return ((await response.json()) as { data: { rows: LimitRow[] } }).data.rows
  }
  const rows = async () => ((await (await reader.handle(new Request("http://box/api/kuvo/limits"))).json()) as { data: { rows: LimitRow[] } }).data.rows
  const observe = (provider: string, signedIn: boolean, revision: string,
    signIn = accounts.all().find(account => account.providerID === provider)?.limits?.signIn?.(), bridge = process.pid) => {
    return reader.handle(new Request("http://box/api/kuvo/usage-observed", {
      method: "POST", body: JSON.stringify({ provider, signedIn, signIn: signIn ?? null, revision, bridge }),
    }))
  }
  try {
    for (const account of accounts.all()) await observe(account.providerID, true, account.providerID)
    await refresh()
    const before = await until(rows, result => result.length === 6 && result.every(row => row.rings.short || row.rings.long))
    expect(Object.fromEntries(before.map(row => [row.service, [row.rings.short?.used ?? null, row.rings.long?.used ?? null]]))).toEqual({
      claude: [0.32, 0.45], codex: [0.12, 0.4], antigravity: [0.25, 0.5], grok: [null, 0.62], cursor: [null, 0.65], opencode: [0.17, 0.79],
    })
    expect(called.has("/v1internal:retrieveUserQuotaSummary")).toBe(true)
    expect(programs.asked().filter(each => each.method === "get_usage").map(each => each.home)).toEqual([join(own, ".claude")])
    expect(cursor.asked.some(request => request.path.endsWith("GetCurrentPeriodUsage"))).toBe(true)
    const first = accounts.all().find(account => account.providerID === "codex-cli")!.limits!.signIn!()
    codexAuth("codex-second")
    // A prior sign-out can be delivered after a new credential arrived. It can't hide the new ring.
    await observe("codex-cli", true, "new-codex")
    expect((await observe("codex-cli", false, "old-codex", first)).status).toBe(409)
    expect((await observe("codex-cli", false, "retired-bridge", undefined, process.pid + 1)).status).toBe(409)
    const orphaned = await reader.handle(new Request("http://box/api/kuvo/usage-observed", {
      method: "POST", body: JSON.stringify({ bridge: process.pid, provider: "codex-cli", signIn: first, revision: "unknown", window: { id: "weekly", used: 0.99 } }),
    }))
    expect(orphaned.status).toBe(409)
    const after = (await until(rows, result => result.some(row => row.service === "codex" && row.rings.short?.used === 0.67))).find(row => row.service === "codex")!
    expect(after.rings.short?.used).toBe(0.67)
    expect(after.identity).not.toBe(before.find(row => row.service === "codex")!.identity)
  } finally {
    await reader.closeInsights()
    Object.assign(engineEndpoints, endpoints)
    claudeUsage.url = claude
    fixture.stop(true)
    cursor.stop()
    process.env.HOME = yours
    rmSync(box, { recursive: true, force: true })
  }
}, 20_000)

test("an open bridge feed learns of an external sign-in switch and a removed CLI with credentials left behind", async () => {
  const box = mkdtempSync(join(tmpdir(), "kuvo-usage-account-"))
  const run = join(box, "run"), home = join(box, "agents")
  const yours = process.env.HOME
  process.env.HOME = join(box, "you")
  const agentHome = join(box, "you", ".grok")
  const auth = join(agentHome, "auth.json"), executable = join(box, "grok")
  const remember = (name: string) => {
    const key = `header.${Buffer.from(JSON.stringify({ sub: name })).toString("base64url")}.signature`
    keep(auth, { login: { key } })
    // What Grok says of this sign-in's week (its ACP `_x.ai/billing`, support/agent-programs.ts).
    keep(join(agentHome, "billing.json"), { config: { currentPeriod: { start: new Date(), end: new Date(Date.now() + 604800_000) }, creditUsagePercent: name === "first" ? 20 : 70 } })
  }
  const programs = agentPrograms(join(box, "programs"))
  remember("first")
  writeFileSync(executable, '#!/bin/sh\nprintf "you are logged in\\n- grok-test (default)\\n"\n', { mode: 0o700 })
  class FixtureGrok extends GrokAgent {
    override executable() { return existsSync(executable) ? executable : undefined }
    override async maintenance() { return {} }
  }
  const agent = new FixtureGrok({ home, temporary: box } as ConstructorParameters<typeof GrokAgent>[0], { id: "default" })
  const accounts = new UsageAccounts(home, { programs })
  const reader = usageReader({ box, run, home, bots: join(box, "bots") }, {
    accounts: () => accounts.all().filter(source => source.providerID === agent.providerID),
    engineCredentials: async () => [],
  })
  const service = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: request => reader.handle(request) })
  keep(servicesFile(run), servicesRecordFormat.write({ pid: process.pid, port: service.port, password: "a".repeat(48) }))
  publishBridge(run)
  const client = new BoxServicesClient(run)
  const interval = globalThis.setInterval, clock = Date.now
  let now = Date.now(), recheck: (() => void) | undefined
  // Capture the real bridge schedule without making the test wait two minutes.
  globalThis.setInterval = ((callback: () => void, every: number) => every === 60_000
    ? (recheck = callback, { unref() { return this } }) : interval(callback, every)) as typeof setInterval
  Date.now = () => now
  let answer: ReturnType<typeof handler>
  try {
    answer = handler({ roster: { all: () => [agent], find: () => agent }, password: "pw", boxServices: client, refreshAccounts: true } as unknown as Context)
  } finally {
    globalThis.setInterval = interval
    Date.now = clock
  }
  const abort = new AbortController()
  const ask = (path: string, method = "GET") => answer(new Request(`http://bridge/${path}`, {
    method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") }, signal: abort.signal,
  }))
  const rows = async () => ((await (await ask("api/kuvo/limits")).json()) as { data: { rows: LimitRow[] } }).data.rows
  const eventAlreadyOpen = await ask("api/event")
  const feed = serverEvents<ServerEvent>(eventAlreadyOpen.body!)
  const changed = async () => {
    const event = await Promise.race([feed.next(), Bun.sleep(4_000).then(() => { throw new Error("No account update reached the feed") })])
    return event.value?.type === "integration.updated" && event.value.data?.integrationID === agent.providerID
  }
  try {
    expect((await feed.next()).value).toMatchObject({ type: "server.connected" })
    expect((await ask("api/kuvo/limits/refresh", "POST")).status).toBe(200)
    const [first] = await until(rows, result => result[0]?.rings.long?.used === 0.2)
    expect(recheck).toBeDefined()
    remember("second")
    recheck!()
    await until(changed, Boolean)
    const [second] = await until(rows, result => result[0]?.rings.long?.used === 0.7)
    expect(second!.identity).not.toBe(first!.identity)
    rmSync(executable)
    now += 60_000
    recheck!()
    await until(changed, Boolean)
    await until(rows, result => result.length === 0)
    expect(existsSync(auth)).toBe(true)
    expect((await rows()).some(row => row.service === "grok")).toBe(false)
    expect(programs.asked().filter(each => each.method === "_x.ai/billing").every(each => each.home === agentHome)).toBe(true)
  } finally {
    abort.abort()
    await feed.return(undefined)
    service.stop(true)
    await reader.closeInsights()
    process.env.HOME = yours
    rmSync(box, { recursive: true, force: true })
  }
}, 20_000)

test("the bridge retries its latest window after the services process restarts", async () => {
  const run = mkdtempSync(join(tmpdir(), "kuvo-usage-retry-"))
  const client = new BoxServicesClient(run)
  const abort = new AbortController()
  const received: { provider: string; signedIn?: boolean; signIn: string; revision: string; bridge: number; window?: { id: string; used: number } }[] = []
  let attempts = 0
  const service = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
    if (new URL(request.url).pathname === "/api/kuvo/usage-events") return new Response("")
    const update = await request.json() as (typeof received)[number]
    if (update.window && ++attempts === 1) return Response.json({}, { status: 503 })
    received.push(update)
    return Response.json({})
  } })
  const following = client.follow(abort.signal)
  try {
    void client.observe("codex-cli", { signedIn: true, signIn: "fixture" })
    void client.observe("codex-cli", { signIn: "fixture", window: { id: "weekly", used: 0.25 } })
    void client.observe("codex-cli", { signIn: "fixture", window: { id: "weekly", used: 0.75 } })
    void client.observe("codex-cli", { signedIn: true, signIn: "fixture" })
    keep(servicesFile(run), servicesRecordFormat.write({ pid: process.pid, port: service.port, password: "a".repeat(48) }))
    await until(async () => received, value => value.some(update => !!update.window))
    expect(attempts).toBe(2)
    expect(received[0]).toMatchObject({ provider: "codex-cli", signedIn: true, signIn: "fixture" })
    expect(received.filter(update => update.window)).toEqual([{ provider: "codex-cli", signIn: "fixture", revision: received[0]!.revision, bridge: process.pid, window: { id: "weekly", used: 0.75 } }])
    const beforeRestart = received.length
    keep(servicesFile(run), servicesRecordFormat.write({ pid: process.pid, port: service.port, password: "b".repeat(48) }))
    await until(async () => received, value => value.length > beforeRestart)
    expect(received[beforeRestart]).toEqual(received[0])
    expect(attempts).toBe(2)
  } finally {
    abort.abort()
    await following
    service.stop(true)
    rmSync(run, { recursive: true, force: true })
  }
}, 20_000)

test("a provider removed from the admitted accounts during a round is not read", async () => {
  const reading = Promise.withResolvers<void>(), finish = Promise.withResolvers<void>()
  let confirmed = true, probes = 0
  const first: LimitedAccount = { provider: "first", service: "codex", limits: { every: 1, async read() {
    reading.resolve()
    await finish.promise
    return null
  } } }
  const second: LimitedAccount = { provider: "second", service: "claude", limits: { every: 1, async read() { probes++; return null } } }
  const limits = new Limits(() => confirmed ? [first, second] : [first])
  const round = limits.tick()
  await reading.promise
  confirmed = false
  finish.resolve()
  await round
  expect(probes).toBe(0)
})

test("a Codex login stored only in Keychain is read with the exact account for its home", async () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-codex-keychain-"))
  try {
    writeFileSync(join(home, "config.toml"), 'cli_auth_credentials_store = "keyring"\n')
    const asked: string[] = []
    const credential = new CodexCredentials(home, async account => {
      asked.push(account)
      return JSON.stringify({ tokens: { access_token: "keychain-fixture", account_id: "fixture" } })
    })
    await credential.prepare()
    expect(asked).toEqual(["cli|" + createHash("sha256").update(realpathSync(home)).digest("hex").slice(0, 16)])
    expect(credential.current()).toEqual({ token: "keychain-fixture", account: "fixture", refresh: undefined })
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})

test("Codex auto storage joins pending keyring lookups and prefers the keyring credential", async () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-codex-auto-"))
  try {
    writeFileSync(join(home, "config.toml"), 'cli_auth_credentials_store = "auto"\n')
    keep(join(home, "auth.json"), { tokens: { access_token: "file-token", account_id: "file" } })
    const result = Promise.withResolvers<string | undefined>()
    let reads = 0
    const credential = new CodexCredentials(home, async () => ++reads === 1 ? result.promise : undefined)
    const first = credential.prepare()
    const second = credential.prepare().then(() => credential.current()?.account)
    result.resolve(JSON.stringify({ tokens: { access_token: "keyring-token", account_id: "keyring" } }))
    await first
    expect(await second).toBe("keyring")
    expect(reads).toBe(1)
    await credential.prepare(true)
    expect(credential.current()?.account).toBe("file")
    expect(await credential.forReset()).toBeUndefined()
    writeFileSync(join(home, "config.toml"), 'cli_auth_credentials_store = "file"\n')
    expect((await credential.forReset())?.account).toBe("file")
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})

test("a Codex reset goes only while Codex says it's signed in to the row's account, and through Codex itself", async () => {
  const box = mkdtempSync(join(tmpdir(), "kuvo-codex-reset-keyring-"))
  const home = join(box, "home")
  const programs = agentPrograms(join(box, "programs"))
  try {
    mkdirSync(home, { recursive: true })
    writeFileSync(join(home, "config.toml"), 'cli_auth_credentials_store = "keyring"\n')
    // The row is for the account the Keychain held when read; Codex, asked now, is signed in to another.
    const saved = (account: string) => JSON.stringify({ tokens: { access_token: "token-" + account, account_id: account } })
    const accounts = new UsageAccounts(box, { programs, codexKeychain: async () => saved("first") })
    const agent = accounts.source("codex", "codex-cli", { id: "default", home })
    await agent.prepare!()
    const source = agent.limits!
    keep(join(home, "auth.json"), JSON.parse(saved("second")))
    expect((await source.spender!())?.same).toBe(false)
    keep(join(home, "auth.json"), JSON.parse(saved("first")))
    expect((await source.spender!())?.same).toBe(true)
    expect(await source.reset!({ request: "fixture", target: "fixture-credit", keep: () => {} }, Date.now())).toEqual({ outcome: "reset" })
    expect(programs.asked().filter(each => each.method === "account/rateLimitResetCredit/consume").map(each => [each.home, each.params])).toEqual([[home, { idempotencyKey: "fixture", creditId: "fixture-credit" }]])
  } finally {
    rmSync(box, { recursive: true, force: true })
  }
})
