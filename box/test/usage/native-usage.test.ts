import "../safety"
// Plan usage asked of the coding agent itself, in each account's own folder (agent-accounts.md):
// Codex's app-server and Grok's ACP. Real: the usage reader's accounts, Limits, its routes and
// resets journal. Stand-ins: the `codex` and `grok` programs (support/agent-programs.ts), which
// answer from the folder they're given, as the real ones read their sign-in there. Claude, Cursor
// and Antigravity read their plans their own ways (usage-*.test.ts).
import { afterEach, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { AccountRefresh } from "../../src/agents/account-refresh"
import type { Agent } from "../../src/agents/agent"
import { subscribe } from "../../src/agents/events"
import { UsageAccounts } from "../../src/usage/accounts"
import { agentAccounts, Limits, limitsRoutes, type AccountLimits } from "../../src/usage/limits"
import { LimitResets } from "../../src/usage/limits-resets"
import { UsageSettings } from "../../src/usage/settings"
import { agentPrograms, keepJSON } from "./support/agent-programs"

const NOW = Date.parse("2026-11-22T09:00:00.000Z")
const DAY = 86_400_000
const seconds = (at: number) => Math.floor(at / 1000)
const folders: string[] = []
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }) })
const folder = () => {
  const made = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-native-usage-"))
  folders.push(made)
  return made
}

const codexSignIn = (home: string, account: string) => keepJSON(join(home, "auth.json"), { tokens: { access_token: `header.${Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: account, chatgpt_user_id: "user-1" } })).toString("base64url")}.signature`, account_id: account, refresh_token: "r-" + account } })
const codexUsage = (home: string, account: string, fiveHour: number, resets: { id: string; expiresIn: number }[] = []) => keepJSON(join(home, "usage.json"), { [account]: {
  ordinaryUsageAllowed: true,
  rateLimits: {},
  rateLimitsByLimitId: {
    codex: { limitId: "codex", planType: "pro", primary: { usedPercent: fiveHour, windowDurationMins: 300, resetsAt: seconds(NOW + 3_600_000) }, secondary: { usedPercent: 40, windowDurationMins: 10_080, resetsAt: seconds(NOW + 5 * DAY) }, credits: { hasCredits: true, unlimited: false, balance: "1240" } },
    codex_spark: { limitId: "codex_spark", limitName: "GPT-5.3-Codex-Spark", primary: { usedPercent: 5, windowDurationMins: 300, resetsAt: null }, secondary: null, credits: null },
  },
  rateLimitResetCredits: { availableCount: resets.length, credits: resets.map(each => ({ id: each.id, resetType: "codexRateLimits", status: "available", grantedAt: seconds(NOW - DAY), expiresAt: seconds(NOW + each.expiresIn), title: null, description: null })) },
} })

const refresh = async (limits: Limits) => ((await (await limitsRoutes(limits, "POST", ["refresh"])).json()) as any).data.accounts as AccountLimits[]

test("each Codex account's plan is what Codex itself says in that account's own folder: windows, plan, credits and resets", async () => {
  const root = folder()
  const programs = agentPrograms(join(root, "programs"))
  const own = join(root, "own"), added = join(root, "added")
  codexSignIn(own, "acct-own")
  codexSignIn(added, "acct-added")
  codexUsage(own, "acct-own", 12, [{ id: "credit_late", expiresIn: 9 * DAY }, { id: "credit_soon", expiresIn: 2 * DAY }])
  codexUsage(added, "acct-added", 67)
  const accounts = new UsageAccounts(root, { programs, codexKeychain: async () => undefined })
  const sources = [accounts.source("codex", "codex-cli", { id: "default", home: own }), accounts.source("codex", "codex-cli@b", { id: "b", label: "Work", home: added })]
  const limits = new Limits(() => agentAccounts(sources), undefined, () => NOW)

  const read = await refresh(limits)

  expect(read.map(each => [each.provider, each.label ?? null, each.plan, each.windows.map(window => [window.id, window.used, window.minutes ?? null, window.model ?? null])])).toEqual([
    ["codex-cli", null, "Pro", [["primary", 0.12, 300, null], ["secondary", 0.4, 10_080, null], ["codex_spark.primary", 0.05, 300, "GPT-5.3-Codex-Spark"]]],
    ["codex-cli@b", "Work", "Pro", [["primary", 0.67, 300, null], ["secondary", 0.4, 10_080, null], ["codex_spark.primary", 0.05, 300, "GPT-5.3-Codex-Spark"]]],
  ])
  expect(read[0]!.windows[0]!.resetsAt).toBe(NOW + 3_600_000)
  expect(read[0]!.extras).toEqual([
    { id: "credits", name: "Extra Usage", kind: "left", amount: 1240, unit: "credits" },
    { id: "resets", name: "Rate Limit Resets", kind: "resets", count: 2, expiries: [NOW + 2 * DAY, NOW + 9 * DAY], usable: true },
  ])
  // Each asked in its own folder, and nothing else.
  expect(programs.asked().filter(each => each.method === "account/rateLimits/read").map(each => each.home).sort()).toEqual([added, own].sort())
})

test("a Codex account signed out in its folder shows nothing to read, and Codex isn't started for it", async () => {
  const root = folder()
  const programs = agentPrograms(join(root, "programs"))
  const home = join(root, "home")
  const accounts = new UsageAccounts(root, { programs, codexKeychain: async () => undefined })
  const limits = new Limits(() => agentAccounts([accounts.source("codex", "codex-cli", { id: "default", home })]), undefined, () => NOW)
  expect((await refresh(limits)).map(each => each.windows)).toEqual([[]])
  expect(programs.asked()).toEqual([])
})

test("Codex publishes its limits immediately after sign-in and reports the first later threshold crossing", async () => {
  const root = folder()
  const programs = agentPrograms(join(root, "programs"))
  const home = join(root, "home")
  const accounts = new UsageAccounts(root, { programs, codexKeychain: async () => undefined })
  const source = accounts.source("codex", "codex-cli", { id: "default", home })
  let signedIn = false
  let now = NOW
  const limits = new Limits(() => signedIn ? agentAccounts([source]) : [], undefined, () => now)
  limits.settings = new UsageSettings()
  limits.settings.change({ notifications: { underTenPercent: true, healthyToClose: false, closeToRunningOut: false } })
  const refresh = new AccountRefresh(limits, () => now, (_provider, known) => { signedIn = known })
  const agent: Agent = {
    providerID: source.providerID, instance: source.instance, limits: source.limits,
    id: "codex", name: "Codex", methods: () => [],
    account: async () => ({ installed: true, signedIn: Boolean(source.limits?.signIn?.()) }),
    models: async () => [],
    thread: () => { throw new Error("The account should publish before starting any conversation.") },
  }
  const updates: unknown[] = []
  const stop = subscribe(event => {
    if (event.type === "kuvo.limits.updated") updates.push(event.data)
  })
  try {
    await limits.tick()
    expect(limits.rows()).toEqual([])
    codexSignIn(home, "acct-1")
    codexUsage(home, "acct-1", 50)
    await refresh.signedIn(agent)
    expect(updates.at(-1)).toMatchObject({
      accounts: [{ windows: expect.arrayContaining([expect.objectContaining({ id: "primary", used: 0.5, resetsAt: NOW + 3_600_000 })]) }],
      notices: [],
    })

    now += 60_000
    codexUsage(home, "acct-1", 99)
    await limits.readAgain(agent.providerID)
    expect(updates.at(-1)).toMatchObject({ notices: expect.arrayContaining([expect.objectContaining({
      service: "codex", milestone: "underTenPercent", resetsAt: NOW + 3_600_000,
    })]) })
  } finally {
    stop()
    limits.stop()
  }
})

test("a Codex reset credit is spent through Codex: the soonest to expire, once, as the journal's request, even confirmed twice", async () => {
  const root = folder()
  const programs = agentPrograms(join(root, "programs"))
  const home = join(root, "home")
  codexSignIn(home, "acct-1")
  codexUsage(home, "acct-1", 100, [{ id: "credit_late", expiresIn: 9 * DAY }, { id: "credit_soon", expiresIn: 2 * DAY }])
  const accounts = new UsageAccounts(root, { programs, codexKeychain: async () => undefined })
  const limits = new Limits(() => agentAccounts([accounts.source("codex", "codex-cli", { id: "default", home })]), join(root, "limits.json"), () => NOW)
  new LimitResets(limits, join(root, "limits-resets.json"), () => NOW)
  await limits.tick()
  const [row] = limits.rows()
  const post = async (body: unknown) => ((await (await limitsRoutes(limits, "POST", ["reset"], async () => body)).json()) as any).data

  const offered = await post({ row: row!.id })
  expect(offered.count).toBe(2)
  expect(programs.asked().filter(each => each.method === "account/rateLimitResetCredit/consume")).toEqual([])
  const done = await post({ row: row!.id, confirm: offered.confirm })
  const again = await post({ row: row!.id, confirm: offered.confirm })

  expect([done.outcome, again.outcome]).toEqual(["reset", "reset"])
  const consumes = programs.asked().filter(each => each.method === "account/rateLimitResetCredit/consume")
  expect(consumes.map(each => [each.home, each.params.creditId])).toEqual([[home, "credit_soon"]])
  expect(consumes[0]!.params.idempotencyKey).toMatch(/\S{8,}/)
})

test("Grok's plan is what Grok itself says in the account's folder, signing in with the sign-in it keeps there", async () => {
  const root = folder()
  const programs = agentPrograms(join(root, "programs"))
  const home = join(root, "grok")
  // A sign-in that has lapsed: Grok renews it itself as it answers; Kuvo asks it all the same.
  keepJSON(join(home, "auth.json"), { "https://auth.x.ai": { key: "grok-token", expires_at: new Date(NOW - DAY).toISOString() } })
  keepJSON(join(home, "billing.json"), { config: { currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start: new Date(NOW - 2 * DAY).toISOString(), end: new Date(NOW + 5 * DAY).toISOString() }, creditUsagePercent: 12.5 } })
  const accounts = new UsageAccounts(root, { programs })
  const limits = new Limits(() => agentAccounts([accounts.source("grok", "grok-cli@a1", { id: "a1", label: "Side", home })]), undefined, () => NOW)

  const read = await refresh(limits)

  expect(read.map(each => [each.provider, each.label, each.problem?.kind ?? null, each.windows.map(window => [window.id, window.used, window.minutes, window.resetsAt])])).toEqual([
    ["grok-cli@a1", "Side", null, [["weekly", 0.125, 7 * 24 * 60, NOW + 5 * DAY]]],
  ])
  expect(programs.asked().map(each => [each.home, each.method])).toEqual([[home, "initialize"], [home, "_x.ai/billing"], [home, "authenticate"], [home, "_x.ai/billing"]])
})
