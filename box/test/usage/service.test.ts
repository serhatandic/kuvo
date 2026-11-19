import "../safety"
import { expect, test } from "bun:test"
import { createHash } from "node:crypto"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { CursorStandIn } from "../agents/support/cursor-standin"
import { UsageAccounts } from "../../src/usage/accounts"
import { CodexCredentials } from "../../src/usage/codex-credentials"
import { claudeUsage } from "../../src/usage/claude-limits"
import { engineEndpoints } from "../../src/usage/engine-limits"
import { codexResetEndpoints } from "../../src/usage/codex-resets"
import type { LimitRow } from "../../src/usage/limits-rows"
import { usageReader } from "../../src/usage/service"

const keep = (file: string, body: unknown) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, JSON.stringify(body))
}

const jwt = (id: string) => "header." + Buffer.from(JSON.stringify({
  "https://api.openai.com/auth": { chatgpt_account_id: id, chatgpt_user_id: id },
})).toString("base64url") + ".signature"

test("the limits route reads and renders all six plans from their fixture credentials and APIs", async () => {
  const box = mkdtempSync(join(tmpdir(), "kuvo-usage-rings-"))
  const run = join(box, "run"), home = join(box, "agents")
  mkdirSync(run)
  const at = new Date(), soon = new Date(+at + 60 * 60_000)
  const week = new Date(+at + 7 * 86400_000), month = new Date(+at + 30 * 86400_000)
  const codexFile = join(home, "codex", "home", "auth.json")
  const codexAuth = (id: string) => keep(codexFile, { tokens: { access_token: jwt(id), account_id: id } })
  codexAuth("codex-first")
  keep(join(home, "claude", "home", ".credentials.json"), { claudeAiOauth: { accessToken: "fixture-claude", scopes: ["user:profile"] } })
  keep(join(home, "grok", "home", "auth.json"), { grok: { key: "fixture-grok", expires_at: week } })
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
      case "/claude":
        expect(using).toBe("Bearer fixture-claude")
        return Response.json({ five_hour: { utilization: 32, resets_at: soon }, seven_day: { utilization: 45, resets_at: week } })
      case "/chatgpt": {
        const account = request.headers.get("chatgpt-account-id")!
        expect(using).toBe("Bearer " + jwt(account))
        return Response.json({ plan_type: "pro", rate_limit: {
          primary_window: { used_percent: account === "codex-second" ? 67 : 12, limit_window_seconds: 18000 },
          secondary_window: { used_percent: 40, limit_window_seconds: 604800 },
        } })
      }
      case "/resets": return Response.json({ available_count: 0 })
      case "/grok":
        expect(using).toBe("Bearer fixture-grok")
        return Response.json({ config: { currentPeriod: { start: at, end: week }, creditUsagePercent: 62 } })
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
  const endpoints = { ...engineEndpoints }, claude = claudeUsage.url, resets = codexResetEndpoints.list
  Object.assign(engineEndpoints, { opencodeGo: base + "/go", chatgpt: base + "/chatgpt", grok: base + "/grok" })
  claudeUsage.url = base + "/claude"
  codexResetEndpoints.list = base + "/resets"
  const accounts = new UsageAccounts(home, true, { claudeByToken: false, antigravity: { bases: [base] } })
  const reader = usageReader({ box, run, home, bots: join(box, "bots"), managedDefaults: true }, {
    accounts: () => accounts.all(),
    engineCredentials: async () => [{ integrationID: "opencode-go", active: true, value: { type: "api", key: "fixture-go" } }],
  })
  const refresh = async () => {
    const response = await reader.handle(new Request("http://box/api/kuvo/limits/refresh", { method: "POST" }))
    expect(response.status).toBe(200)
    return ((await response.json()) as { data: { rows: LimitRow[] } }).data.rows
  }
  try {
    const before = await refresh()
    expect(Object.fromEntries(before.map(row => [row.service, [row.rings.short?.used ?? null, row.rings.long?.used ?? null]]))).toEqual({
      claude: [0.32, 0.45], codex: [0.12, 0.4], antigravity: [0.25, 0.5], grok: [null, 0.62], cursor: [null, 0.65], opencode: [0.17, 0.79],
    })
    expect(called.has("/v1internal:retrieveUserQuotaSummary")).toBe(true)
    expect(cursor.asked.some(request => request.path.endsWith("GetCurrentPeriodUsage"))).toBe(true)
    const first = accounts.all().find(account => account.providerID === "codex-cli")!.limits!.signIn!()
    codexAuth("codex-second")
    // A prior sign-out can be delivered after a new credential arrived. It can't hide the new ring.
    await reader.handle(new Request("http://box/api/kuvo/usage-observed", {
      method: "POST", body: JSON.stringify({ provider: "codex-cli", signedIn: false, signIn: first }),
    }))
    const after = (await refresh()).find(row => row.service === "codex")!
    expect(after.rings.short?.used).toBe(0.67)
    expect(after.identity).not.toBe(before.find(row => row.service === "codex")!.identity)
  } finally {
    await reader.closeInsights()
    Object.assign(engineEndpoints, endpoints)
    claudeUsage.url = claude
    codexResetEndpoints.list = resets
    fixture.stop(true)
    cursor.stop()
    rmSync(box, { recursive: true, force: true })
  }
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

test("a Codex reset checks the live keyring account and spends with the exact credential it checked", async () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-codex-reset-keyring-"))
  const calls: string[] = []
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
    calls.push(request.headers.get("authorization") ?? "")
    return Response.json({ code: "reset" })
  } })
  const consume = codexResetEndpoints.consume
  codexResetEndpoints.consume = `http://127.0.0.1:${server.port}/consume`
  try {
    writeFileSync(join(home, "config.toml"), 'cli_auth_credentials_store = "keyring"\n')
    const saved = (account: string, token: string) => JSON.stringify({ tokens: { access_token: token, account_id: account } })
    let value = saved("first", "first-token")
    const accounts = new UsageAccounts(home, true, { codexKeychain: async () => value })
    const agent = accounts.source("codex", "codex-cli", { id: "default", home })
    await agent.prepare!()
    value = saved("second", "second-token")
    const source = agent.limits!
    expect((await source.spender!())?.same).toBe(false)
    expect(await source.reset!({ request: "unchecked", target: "fixture-credit", keep: () => {} }, Date.now())).toEqual({ outcome: "no-reset" })
    expect(calls).toEqual([])
    value = saved("second", "renewed-token")
    const checked = await source.spender!()
    expect(checked?.same).toBe(true)
    value = saved("third", "third-token")
    expect((await source.spender!())?.same).toBe(false)
    expect(await source.reset!({ request: "fixture", target: "fixture-credit", keep: () => {}, signIn: checked?.signIn }, Date.now())).toEqual({ outcome: "reset" })
    expect(calls).toEqual(["Bearer renewed-token"])
  } finally {
    codexResetEndpoints.consume = consume
    server.stop(true)
    rmSync(home, { recursive: true, force: true })
  }
})
