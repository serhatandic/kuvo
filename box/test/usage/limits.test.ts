import "../safety"
import { claudeProbe } from "./claude-probe"
// Your plans' limits as the box states them (`api/kuvo/limits`): read from Kuvo Code's own sign-in
// for each account, one row each, kept through a 429 for as long as Anthropic says, and through a
// restart; what a turn hears keeps them current. Anthropic and Codex are stand-ins here.

import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import { createHash } from "node:crypto"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { ClaudeAgent } from "../../src/agents/agents/claude"
import { claudeUsage } from "../../src/usage/claude-limits"
import { UsageAccounts } from "../../src/usage/accounts"
import { EngineAccounts, engineEndpoints } from "../../src/usage/engine-limits"
import { codexResetEndpoints } from "../../src/usage/codex-resets"
import { subscribe } from "../../src/agents/events"
import { agentAccounts, Limits, limitsRoutes, type AccountLimits } from "../../src/usage/limits"

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-limits-test-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

const keychain = ClaudeAgent.keychain
afterEach(() => {
  ClaudeAgent.keychain = keychain
})

/** Anthropic's usage endpoint: what it answers each sign-in (by its token), and every read it got. */
class Anthropic {
  answers = new Map<string, () => Response>()
  reads: { token: string; agent: string | null; beta: string | null }[] = []
  /** Each sign-in's reset grants, and every read of them. */
  grants = new Map<string, () => Response>()
  grantReads: { token: string; agent: string | null }[] = []
  private waiting: (() => void)[] = []
  /** Once the next read has come. */
  nextRead() {
    return new Promise<void>((resolve) => this.waiting.push(resolve))
  }
  private server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: (request) => {
      const token = (request.headers.get("authorization") ?? "").replace(/^Bearer /, "")
      // Its reset grants: Claude Code's own read of them, apart (at most hourly an account).
      if (new URL(request.url).searchParams.get("cedar_ember") === "1") {
        this.grantReads.push({ token, agent: request.headers.get("user-agent") })
        return this.grants.get(token)?.() ?? Response.json({ cedar_ember: null })
      }
      this.reads.push({ token, agent: request.headers.get("user-agent"), beta: request.headers.get("anthropic-beta") })
      for (const resolve of this.waiting.splice(0)) resolve()
      return this.answers.get(token)?.() ?? new Response("{}", { status: 401 })
    },
  })
  readonly url = `http://127.0.0.1:${this.server.port}/api/oauth/usage`
  stop() {
    this.server.stop(true)
  }
}

const usage = (fiveHour: number, week: number, extra: Record<string, unknown> = {}) => () =>
  Response.json({
    five_hour: { utilization: fiveHour, resets_at: "2026-11-09T02:00:00.000Z" },
    seven_day: { utilization: week, resets_at: "2026-11-12T09:00:00.000Z" },
    seven_day_sonnet: null,
    ...extra,
  })

class TestClaude extends ClaudeAgent {
  override readonly limits = claudeProbe(this, "2.1.300")
  constructor(home: string, instance?: { id: string; label?: string; home?: string }) {
    super({ home, temporary: home, deps: {} as any }, instance)
  }
}

/** Kuvo Code's own token for an account on a Mac, as signing in keeps it (`claude setup-token`). */
function signIn(home: string, account: string, token: string) {
  mkdirSync(join(home, "claude", "tokens"), { recursive: true })
  writeFileSync(join(home, "claude", "tokens", `${account}.json`), JSON.stringify({ token, made: 1 }))
}

/** A clock the test moves. */
class Clock {
  at = Date.parse("2026-11-08T20:00:00.000Z")
  now = () => this.at
  pass(ms: number) {
    this.at += ms
  }
}

const route = async (limits: Limits, method = "GET", rest: string[] = []) => ((await (await limitsRoutes(limits, method, rest)).json()) as any).data as { accounts: AccountLimits[]; services: string[] }

/**
 * Refreshed, as a client's pull does, once the reads are over: the route answers after two seconds
 * with what's read by then, and a Codex started to read takes longer than that on a busy Mac.
 */
const refreshed = async (limits: Limits) => {
  await limits.refresh()
  return route(limits)
}

let anthropic: Anthropic
let home: string
let clock: Clock

beforeEach(() => {
  anthropic = new Anthropic()
  claudeUsage.url = anthropic.url
  home = mkdtempSync(join(base, "case-"))
  clock = new Clock()
})

afterEach(() => anthropic.stop())

describe("Claude on a Mac", () => {
  beforeEach(() => {
    ClaudeAgent.keychain = true
  })

  /** Claude Code's rate-limit event, as 2.1.3xx sends it with every window from a response's headers. */
  const event = (fiveHour: number, week: number, fable?: number) => ({
    status: "allowed",
    rateLimitType: "five_hour",
    utilization: fiveHour,
    unifiedWindows: {
      five_hour: { utilization: fiveHour, resetsAt: 1_794_200_000, status: "allowed" },
      seven_day: { utilization: week, resetsAt: 1_794_600_000, status: "allowed" },
      ...(fable === undefined ? {} : { seven_day_overage_included: { utilization: fable, resetsAt: 1_794_600_000 } }),
    },
  })

  test("each Claude login is its own row, from what its turns hear, and Anthropic's usage endpoint is never asked", async () => {
    const { heardWindows } = await import("../../src/usage/claude-limits")
    const own = new TestClaude(home)
    const work = new TestClaude(home, { id: "1f2e3d4c", label: "Work", home: join(home, "claude", "accounts", "1f2e3d4c") })
    // Kuvo Code's own sign-ins: setup-tokens, which can only run Claude.
    signIn(home, "default", "token-own")
    signIn(home, "1f2e3d4c", "token-work")
    anthropic.answers.set("token-own", usage(1, 1))
    const limits = new Limits(() => agentAccounts([own, work]), undefined, clock.now)
    const told: any[] = []
    const stop = subscribe((each) => each.type === "kuvo.limits.updated" && told.push(each.data))
    await limits.refresh()
    // Signed in, each has its row before any turn told its limits: nothing read yet, no windows.
    expect((await route(limits)).accounts.map((each) => [each.provider, each.windows])).toEqual([["claude-code", []], ["claude-code@1f2e3d4c", []]])

    clock.pass(60_000)
    for (const window of heardWindows(event(0.42, 0.18, 0.07))) own.limits.heard!(window)
    for (const window of heardWindows(event(0.9, 0.61))) work.limits.heard!(window)
    stop()
    await limits.refresh()
    clock.pass(60 * 60_000)
    await limits.tick()

    const { accounts, services } = await route(limits)
    expect(services).toEqual(["claude"])
    expect(accounts.map((each) => [each.provider, each.label])).toEqual([["claude-code", undefined], ["claude-code@1f2e3d4c", "Work"]])
    expect(accounts[0]!.windows.map((window) => [window.id, window.used, window.minutes, window.model, window.resetsAt, window.source])).toEqual([
      ["five_hour", 0.42, 300, undefined, 1_794_200_000_000, "turn"],
      ["seven_day", 0.18, 10_080, undefined, 1_794_600_000_000, "turn"],
      ["seven_day_fable", 0.07, 10_080, "Fable", 1_794_600_000_000, "turn"],
    ])
    expect(accounts[1]!.windows.map((window) => window.used)).toEqual([0.9, 0.61])
    expect(told.at(-1).accounts).toHaveLength(2)
    expect(anthropic.reads).toHaveLength(0)
  })

  test("an older Claude Code's event, with only the window that limits it, keeps that one", async () => {
    const { heardWindows } = await import("../../src/usage/claude-limits")
    expect(heardWindows({ status: "allowed_warning", rateLimitType: "seven_day", utilization: 0.81, resetsAt: 1_794_600_000 })).toEqual([{ id: "seven_day", used: 0.81, minutes: 10_080, resetsAt: 1_794_600_000_000 }])
    // Turned away, with no amount: all of it.
    expect(heardWindows({ status: "rejected", rateLimitType: "five_hour" })[0]!.used).toBe(1)
    expect(heardWindows({ status: "allowed" })).toEqual([])
  })

  test("Claude Code's warning of a limit says how much of which one is used, and close only from 90% (#257)", async () => {
    const { limitWarning } = await import("../../src/usage/claude-limits")
    const said = (info: any) => limitWarning({ status: "allowed_warning", ...info }).text
    expect(said({ rateLimitType: "seven_day", utilization: 0.25, surpassedThreshold: 0.25 })).toBe("You've used 25% of your weekly Claude limit.")
    expect(limitWarning({ rateLimitType: "seven_day", utilization: 0.25 }).close).toBe(false)
    expect(said({ rateLimitType: "seven_day_opus", utilization: 0.756 })).toBe("You've used 76% of your weekly Opus limit.")
    expect(said({ rateLimitType: "five_hour", utilization: 0.9 })).toBe("You're close to your 5-hour Claude limit: 90% used.")
    expect(limitWarning({ rateLimitType: "five_hour", utilization: 0.9 }).close).toBe(true)
    expect(said({ rateLimitType: "seven_day_sonnet", utilization: 0.996 })).toBe("You're close to your weekly Sonnet limit: 99% used.")
    expect(said({ rateLimitType: "overage", utilization: 0.95 })).toBe("You're close to your Claude extra usage limit: 95% used.")
    // All of it, or past it in Claude Code's grace: not "close".
    expect(said({ rateLimitType: "seven_day", utilization: 1.05 })).toBe("You've used all of your weekly Claude limit.")
    // The figure from the window's own, where only that says it.
    expect(said({ rateLimitType: "seven_day", unifiedWindows: { seven_day: { utilization: 0.5 } } })).toBe("You've used 50% of your weekly Claude limit.")
    // No figure: the threshold it passed, else only that it warned.
    expect(said({ rateLimitType: "seven_day", surpassedThreshold: 0.5 })).toBe("You've used over 50% of your weekly Claude limit.")
    expect(said({ rateLimitType: "five_hour", surpassedThreshold: 0.9 })).toBe("You're close to your 5-hour Claude limit.")
    expect(said({ rateLimitType: "overage", utilization: null })).toBe("Claude Code warned about your Claude extra usage limit.")
    expect(said({})).toBe("Claude Code warned about your Claude usage limit.")
  })
})

describe("Claude on Linux", () => {
  beforeEach(() => {
    ClaudeAgent.keychain = false
  })

  /** Claude Code's own sign-in file, in Kuvo Code's folder for the account. */
  function credentials(folder: string, oauth: Record<string, unknown>) {
    mkdirSync(folder, { recursive: true })
    writeFileSync(join(folder, ".credentials.json"), JSON.stringify({ claudeAiOauth: oauth }))
  }
  const signedIn = (folder: string, token: string) => credentials(folder, { accessToken: token, expiresAt: clock.at + 30 * 86_400_000, scopes: ["user:inference", "user:profile"], subscriptionType: "max", rateLimitTier: "default_claude_max_20x" })

  test("its extra usage, and its reset grants read at most hourly: a failed read of them keeps what was read, never none", async () => {
    const folder = join(home, "claude", "home")
    signedIn(folder, "token-own")
    anthropic.answers.set("token-own", usage(30, 50, { extra_usage: { is_enabled: true, used_credits: 1240, monthly_limit: 5000 } }))
    const grants = (left: number) => () => Response.json({ cedar_ember: { eligible: true, next_grant_id: "promo_one", grants: [{ id: "promo_one", resets_left: left, ends_at: "2026-11-20T00:00:00.000Z", clears: ["five_hour", "seven_day"] }] } })
    anthropic.grants.set("token-own", grants(2))
    const claude = new TestClaude(home, { id: "default", home: folder })
    const limits = new Limits(() => agentAccounts([claude]), undefined, clock.now)
    await limits.tick()
    const extras = () => (limits.list()[0]?.extras ?? []).map((extra) => [extra.id, extra.kind, extra.amount ?? extra.count, extra.limit ?? null])
    expect(extras()).toEqual([["extra_usage", "capped", 12.4, 50], ["resets", "resets", 2, null]])
    // As Claude Code asks it, with Claude Code's own user agent (Anthropic offers grants to no other).
    expect(anthropic.grantReads).toEqual([{ token: "token-own", agent: "claude-cli/2.1.300 (external, cli)" }])

    // A read a quarter of an hour on: its windows again, its grants as they were.
    anthropic.grants.set("token-own", grants(1))
    clock.pass(15 * 60_000)
    await limits.tick()
    expect(anthropic.grantReads).toHaveLength(1)
    expect(extras()[1]).toEqual(["resets", "resets", 2, null])
    // An hour after: read again. Then Anthropic turns that read away: what was read stands.
    clock.pass(45 * 60_000)
    await limits.tick()
    expect(anthropic.grantReads).toHaveLength(2)
    expect(extras()[1]).toEqual(["resets", "resets", 1, null])
    anthropic.grants.set("token-own", () => new Response("{}", { status: 429 }))
    clock.pass(60 * 60_000)
    await limits.tick()
    expect(anthropic.grantReads).toHaveLength(3)
    expect(extras()[1]).toEqual(["resets", "resets", 1, null])
    // (Not asked again on the next read: a failed one counts as the hour's.)
    clock.pass(15 * 60_000)
    await limits.tick()
    expect(anthropic.grantReads).toHaveLength(3)
  })

  /** Two accounts, signed in in Kuvo Code's folders. */
  function two() {
    const ownHome = join(home, "claude", "home")
    const workHome = join(home, "claude", "accounts", "1f2e3d4c")
    signedIn(ownHome, "token-own")
    signedIn(workHome, "token-work")
    return [new TestClaude(home, { id: "default", home: ownHome }), new TestClaude(home, { id: "1f2e3d4c", label: "Work", home: workHome })]
  }

  test("each login is its own row, read with its own sign-in in Kuvo Code's folder, one a minute", async () => {
    const [own, work] = two()
    anthropic.answers.set("token-own", usage(42, 18, { limits: [{ kind: "weekly_scoped", percent: 7, resets_at: "2026-11-12T09:00:00.000Z", scope: { model: { display_name: "Fable" } } }] }))
    anthropic.answers.set("token-work", usage(90, 61))
    const limits = new Limits(() => agentAccounts([own!, work!]), undefined, clock.now)
    await limits.tick()
    // Anthropic's limit on reads is the computer's: never two at once.
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-own"])
    clock.pass(60_000)
    await limits.tick()
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-own", "token-work"])

    const { accounts } = await route(limits)
    expect(accounts.map((each) => [each.provider, each.label, each.plan])).toEqual([["claude-code", undefined, "Max 20x"], ["claude-code@1f2e3d4c", "Work", "Max 20x"]])
    expect(accounts[0]!.windows.map((window) => [window.id, window.used, window.minutes, window.model, window.source])).toEqual([
      ["five_hour", 0.42, 300, undefined, "usage"],
      ["seven_day", 0.18, 10_080, undefined, "usage"],
      ["seven_day_fable", 0.07, 10_080, "Fable", "usage"],
    ])
    expect(accounts[1]!.windows.map((window) => window.used)).toEqual([0.9, 0.61])
    // As Claude Code asks it.
    expect(anthropic.reads.every((read) => read.beta === "oauth-2025-04-20" && read.agent === "claude-cli/2.1.300 (external, cli)")).toBe(true)
  })

  test("a 429 holds every Claude login through a restart, and old readings stop showing", async () => {
    const [own, work] = two()
    anthropic.answers.set("token-own", usage(42, 18))
    anthropic.answers.set("token-work", usage(90, 61))
    const file = join(home, "limits.json")
    const limits = new Limits(() => agentAccounts([own!, work!]), file, clock.now)
    await limits.tick()
    clock.pass(60_000)
    await limits.tick()
    expect(anthropic.reads).toHaveLength(2)

    // Fifteen minutes on, Anthropic turns the first away, for 46 minutes.
    clock.pass(15 * 60_000)
    anthropic.answers.set("token-own", () => new Response(JSON.stringify({ error: { type: "rate_limit_error" } }), { status: 429, headers: { "retry-after": "2766" } }))
    await limits.tick()
    clock.pass(60_000)
    await limits.tick()
    // The other isn't asked meanwhile: the hold is the computer's.
    expect(anthropic.reads).toHaveLength(3)
    const held = await route(limits)
    expect(held.accounts[0]!.problem).toEqual({ kind: "rate-limited", message: expect.any(String), until: clock.at - 60_000 + 2_766_000 })
    expect(held.accounts.map((each) => each.windows.map((window) => window.used))).toEqual([[0.42, 0.18], [0.9, 0.61]])

    // Refreshing, and the minutes passing, ask nothing until then: not after a restart either.
    await limits.refresh()
    clock.pass(30 * 60_000)
    await limits.tick()
    const restarted = new Limits(() => agentAccounts([own!, work!]), file, clock.now)
    await restarted.refresh()
    await restarted.tick()
    expect(anthropic.reads).toHaveLength(3)
    expect((await route(restarted)).accounts.map((each) => each.windows)).toEqual([[], []])

    // Once it's time, they're read again, one a minute.
    anthropic.answers.set("token-own", usage(50, 20))
    clock.pass(16 * 60_000)
    await restarted.tick()
    clock.pass(60_000)
    await restarted.tick()
    expect(anthropic.reads).toHaveLength(5)
    const after = await route(restarted)
    expect(after.accounts[0]!.problem).toBeUndefined()
    expect(after.accounts[0]!.windows.map((window) => window.used)).toEqual([0.5, 0.2])
  })

  test("a read under way is kept before it's answered: a restart meanwhile doesn't ask again", async () => {
    const folder = join(home, "claude", "home")
    signedIn(folder, "token-own")
    let answer!: () => void
    anthropic.answers.set("token-own", () => new Promise<Response>((resolve) => (answer = () => resolve(usage(42, 18)()))) as any)
    const file = join(home, "limits.json")
    const own = new TestClaude(home, { id: "default", home: folder })
    const first = new Limits(() => agentAccounts([own]), file, clock.now)
    const asked = anthropic.nextRead()
    const reading = first.tick()
    await asked
    // The box starts again while Anthropic hasn't answered.
    const restarted = new Limits(() => agentAccounts([new TestClaude(home, { id: "default", home: folder })]), file, clock.now)
    await restarted.tick()
    expect(anthropic.reads).toHaveLength(1)
    answer()
    await reading
  })

  test("a refresh right after a read doesn't read another Claude account within the minute", async () => {
    const [own, work] = two()
    anthropic.answers.set("token-own", usage(42, 18))
    anthropic.answers.set("token-work", usage(90, 61))
    const limits = new Limits(() => agentAccounts([own!, work!]), undefined, clock.now)
    await limits.tick()
    clock.pass(5_000)
    await limits.refresh()
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-own"])
    clock.pass(55_000)
    await limits.refresh()
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-own", "token-work"])
  })

  test("a restart while one Claude account is being read doesn't read another in the same minute", async () => {
    const [own, work] = two()
    let answer!: () => void
    anthropic.answers.set("token-own", () => new Promise<Response>((resolve) => (answer = () => resolve(usage(42, 18)()))) as any)
    anthropic.answers.set("token-work", usage(90, 61))
    const file = join(home, "limits.json")
    const first = new Limits(() => agentAccounts([own!, work!]), file, clock.now)
    const asked = anthropic.nextRead()
    const reading = first.tick()
    await asked
    // The box starts again, seconds later.
    clock.pass(5_000)
    const restarted = new Limits(() => agentAccounts([own!, work!]), file, clock.now)
    await restarted.tick()
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-own"])
    answer()
    await reading
  })

  test("a 429 with no time waits an hour, then longer each time", async () => {
    const folder = join(home, "claude", "home")
    signedIn(folder, "token-own")
    anthropic.answers.set("token-own", () => new Response("{}", { status: 429 }))
    const limits = new Limits(() => agentAccounts([new TestClaude(home, { id: "default", home: folder })]), undefined, clock.now)
    await limits.tick()
    expect((await route(limits)).accounts[0]!.problem?.until).toBe(clock.at + 60 * 60_000)
    clock.pass(60 * 60_000)
    await limits.tick()
    expect(anthropic.reads).toHaveLength(2)
    expect((await route(limits)).accounts[0]!.problem?.until).toBe(clock.at + 2 * 60 * 60_000)
  })

  test("a sign-in Anthropic won't let read its limits isn't asked again until it's signed in again", async () => {
    const folder = join(home, "claude", "home")
    signedIn(folder, "token-old")
    anthropic.answers.set("token-old", () => Response.json({ error: { type: "permission_error", message: "OAuth token does not meet scope requirement user:profile" } }, { status: 403 }))
    anthropic.answers.set("token-new", usage(5, 5))
    const limits = new Limits(() => agentAccounts([new TestClaude(home, { id: "default", home: folder })]), undefined, clock.now)
    await limits.tick()
    expect((await route(limits)).accounts[0]!.problem?.kind).toBe("unreadable")
    clock.pass(3 * 60 * 60_000)
    await limits.tick()
    await limits.refresh()
    expect(anthropic.reads).toHaveLength(1)
    // Signed in again: read at its next turn.
    signedIn(folder, "token-new")
    clock.pass(15 * 60_000)
    await limits.tick()
    expect(anthropic.reads.map((read) => read.token)).toEqual(["token-old", "token-new"])
    expect((await route(limits)).accounts[0]!.problem).toBeUndefined()
  })

  test("what a turn hears keeps a window current between reads, even while a read is under way", async () => {
    const folder = join(home, "claude", "home")
    signedIn(folder, "token-own")
    const own = new TestClaude(home, { id: "default", home: folder })
    anthropic.answers.set("token-own", usage(42, 18))
    const limits = new Limits(() => agentAccounts([own]), undefined, clock.now)
    await limits.tick()
    clock.pass(60_000)
    own.limits.heard!({ id: "five_hour", used: 0.97, resetsAt: Date.parse("2026-11-09T02:00:00.000Z") })
    let window = (await route(limits)).accounts[0]!.windows.find((each) => each.id === "five_hour")!
    expect([window.used, window.minutes, window.source, window.read]).toEqual([0.97, 300, "turn", clock.at])
    // A read asked before a turn heard more doesn't take its place with older numbers.
    let answer!: () => void
    anthropic.answers.set("token-own", () => new Promise<Response>((resolve) => (answer = () => resolve(usage(50, 20)()))) as any)
    clock.pass(15 * 60_000)
    const asked = anthropic.nextRead()
    const reading = limits.tick()
    await asked
    own.limits.heard!({ id: "five_hour", used: 0.99 })
    answer()
    await reading
    const windows = (await route(limits)).accounts[0]!.windows
    expect(windows.map((each) => [each.id, each.used, each.source])).toEqual([["seven_day", 0.2, "usage"], ["five_hour", 0.99, "turn"]])
  })

  test("a lapsed or inference-only sign-in says why, and never goes to Anthropic", async () => {
    const lapsed = join(home, "claude", "home")
    const inference = join(home, "claude", "accounts", "a1")
    credentials(lapsed, { accessToken: "token-lapsed", expiresAt: clock.at - 1, scopes: ["user:profile"] })
    credentials(inference, { accessToken: "token-inference", expiresAt: clock.at + 3_600_000, scopes: ["user:inference"] })
    const limits = new Limits(() => agentAccounts([new TestClaude(home, { id: "default", home: lapsed }), new TestClaude(home, { id: "a1", home: inference })]), undefined, clock.now)
    const { accounts } = await route(limits, "POST", ["refresh"])
    expect(accounts.map((each) => each.problem?.kind)).toEqual(["expired", "unreadable"])
    expect(anthropic.reads).toHaveLength(0)
  })

  test("never reads a sign-in outside Kuvo Code's folders (yours in ~/.claude)", async () => {
    credentials(join(process.env.HOME!, ".claude"), { accessToken: "token-yours", scopes: ["user:profile"] })
    anthropic.answers.set("token-yours", usage(1, 1))
    const limits = new Limits(() => agentAccounts([new TestClaude(home)]), undefined, clock.now)
    const { accounts, services } = await refreshed(limits)
    expect(accounts).toEqual([])
    expect(services).toEqual(["claude"])
    expect(anthropic.reads).toHaveLength(0)
  })
})

const wham = (used: number, minutes = 300) => ({
  used_percent: used, limit_window_seconds: minutes * 60, reset_at: 1_794_200_000,
})
const codexFolder = () => join(home, "codex", "home")
function codexAuth(access = "fixture", account?: string) {
  mkdirSync(codexFolder(), { recursive: true })
  writeFileSync(join(codexFolder(), "auth.json"), JSON.stringify({ tokens: { access_token: access, account_id: account } }))
}
function codexLimits() {
  const agents = new UsageAccounts(home, true)
  return new Limits(() => agentAccounts(agents.all().filter(agent => agent.providerID === "codex-cli" && agent.limits?.signIn?.())), undefined, clock.now)
}

async function withChatGPT(run: (fixture: {
  usage: (body: unknown) => void
  resets: (body: unknown, status?: number) => void
  answers: { path: string; account: string | null }[]
}) => Promise<void>) {
  let usage: unknown = { plan_type: "plus", rate_limit: { primary_window: wham(12) } }
  let resets: unknown = {}
  let resetStatus = 200
  const answers: { path: string; account: string | null }[] = []
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
    const path = new URL(request.url).pathname
    answers.push({ path, account: request.headers.get("chatgpt-account-id") })
    return path === "/usage" ? Response.json(usage) : Response.json(resets, { status: resetStatus })
  } })
  const endpoints = { usage: engineEndpoints.chatgpt, resets: codexResetEndpoints.list }
  const environmentPath = process.env.PATH
  const starts = join(home, "starts.txt")
  mkdirSync(join(home, "bin"), { recursive: true })
  const launcher = join(home, "bin", "codex")
  writeFileSync(launcher, `#!/bin/sh\necho started >> "${starts}"\nexit 1\n`, { mode: 0o700 })
  engineEndpoints.chatgpt = `http://127.0.0.1:${server.port}/usage`
  codexResetEndpoints.list = `http://127.0.0.1:${server.port}/list`
  process.env.PATH = join(home, "bin") + ":" + environmentPath
  try {
    await run({ usage: body => { usage = body }, resets: (body, status = 200) => { resets = body; resetStatus = status }, answers })
    expect(existsSync(starts)).toBe(false)
  } finally {
    process.env.PATH = environmentPath
    engineEndpoints.chatgpt = endpoints.usage
    codexResetEndpoints.list = endpoints.resets
    server.stop(true)
  }
}

describe("Codex", () => {
  test("its plan's windows and a model's own are read with its ChatGPT sign-in", async () => withChatGPT(async ({ usage }) => {
    codexAuth()
    usage({ plan_type: "pro", rate_limit: { primary_window: wham(12), secondary_window: wham(40, 10080) },
      additional_rate_limits: [{ metered_feature: "codex_spark", limit_name: "GPT-5.3-Codex-Spark", rate_limit: { primary_window: wham(3) } }] })
    const { accounts } = await refreshed(codexLimits())
    expect(accounts).toHaveLength(1)
    expect(accounts[0]!.plan).toBe("Pro")
    expect(accounts[0]!.windows.map((each) => [each.id, each.used, each.minutes, each.model, each.resetsAt])).toEqual([
      ["primary", 0.12, 300, undefined, 1_794_200_000_000],
      ["secondary", 0.4, 10_080, undefined, 1_794_200_000_000],
      ["codex_spark.primary", 0.03, 300, "GPT-5.3-Codex-Spark", 1_794_200_000_000],
    ])
  }))

  test("its credits and resets are read without Codex and kept when a reset read fails", async () => withChatGPT(async ({ usage, resets, answers }) => {
    codexAuth(token({ "https://api.openai.com/auth": { chatgpt_account_id: "acct-9" } }), "acct-9")
    usage({ plan_type: "plus", rate_limit: { primary_window: wham(12) }, credits: { has_credits: true, unlimited: false, balance: "50.7" } })
    resets({ available_count: 1, credits: [{ id: "c-1", status: "available", expires_at: "2026-11-20T00:00:00Z" }, { id: "c-0", status: "redeemed", expires_at: "2026-11-19T00:00:00Z" }] })
    const limits = codexLimits()
    await limits.tick()
    const extras = () => (limits.list()[0]?.extras ?? []).map(extra => [extra.id, extra.kind, extra.amount ?? extra.count, extra.expiries ?? null])
    expect(extras()).toEqual([["credits", "left", 50, null], ["resets", "resets", 1, [Date.parse("2026-11-20T00:00:00Z")]]])
    expect(answers.filter(request => request.path === "/list").map(request => request.account)).toEqual(["acct-9"])
    resets({ available_count: 0, credits: [] })
    clock.pass(10 * 60_000)
    await limits.tick()
    expect(extras()).toEqual([["credits", "left", 50, null], ["resets", "resets", 0, null]])
    expect(limits.list()[0]!.windows.map(each => each.used)).toEqual([0.12])
    resets({}, 502)
    clock.pass(10 * 60_000)
    await limits.tick()
    expect(extras()).toEqual([["credits", "left", 50, null], ["resets", "resets", 0, null]])
  }))

  test("Codex limits refresh on schedule without starting an app-server", async () => withChatGPT(async ({ answers }) => {
    codexAuth()
    const limits = codexLimits()
    const reads = () => answers.filter(request => request.path === "/usage").length
    await limits.tick()
    expect(reads()).toBe(1)
    clock.pass(10 * 60_000)
    await limits.tick()
    expect(reads()).toBe(2)
    expect((await route(limits)).accounts[0]!.windows.map(each => each.used)).toEqual([0.12])
  }))

  test("a configured API key never starts Codex or reads ChatGPT plan limits", async () => withChatGPT(async ({ answers }) => {
    codexAuth()
    writeFileSync(join(codexFolder(), "auth.json"), JSON.stringify({ auth_mode: "apikey", OPENAI_API_KEY: "key", tokens: { access_token: "old-oauth" } }))
    const limits = codexLimits()
    await limits.tick()
    clock.pass(31 * 60_000)
    await limits.tick()
    expect(answers).toHaveLength(0)
    expect((await route(limits)).accounts).toEqual([])
  }))

  test("an API key has no plan limits: not listed", async () => withChatGPT(async ({ answers }) => {
    mkdirSync(codexFolder(), { recursive: true })
    writeFileSync(join(codexFolder(), "auth.json"), JSON.stringify({ OPENAI_API_KEY: "key" }))
    expect((await refreshed(codexLimits())).accounts).toEqual([])
    expect(answers).toHaveLength(0)
  }))
})

/** A sign-in's token as its maker signs it: only what it says of itself matters here. */
const token = (claims: Record<string, unknown>) => `eyJhbGciOiJSUzI1NiJ9.${Buffer.from(JSON.stringify(claims)).toString("base64url")}.signature`

describe("who each account is, the same from every computer", () => {
  let recordWrites = 0
  /** Claude Code's own record of who it's signed in as, beside its sign-in (Linux). */
  function signedInAs(folder: string, token: string, account: string, organization: string) {
    mkdirSync(folder, { recursive: true })
    writeFileSync(join(folder, ".credentials.json"), JSON.stringify({ claudeAiOauth: { accessToken: token, expiresAt: clock.at + 86_400_000, scopes: ["user:inference", "user:profile"] } }))
    const record = join(folder, ".claude.json")
    writeFileSync(record, JSON.stringify({ oauthAccount: { accountUuid: account, emailAddress: "ada@example.com", organizationUuid: organization } }))
    // Each record gets a later file time, as Claude Code's would: the box rereads it only when its
    // mtime or size changed, and Linux can give two same-size writes made close together one file
    // time, leaving the old account's identity in place.
    const at = new Date(clock.at + recordWrites++ * 60_000)
    utimesSync(record, at, at)
  }

  test("one Claude login on two computers is one identity, another login another, and neither says who", async () => {
    ClaudeAgent.keychain = false
    // Two computers' boxes, each with Kuvo Code's own sign-ins in its folders.
    const first = join(home, "first")
    const second = join(home, "second")
    signedInAs(join(first, "claude", "home"), "token-1", "6F1E0C4A-0000-4000-8000-000000000001", "org-1")
    signedInAs(join(second, "claude", "home"), "token-2", "6f1e0c4a-0000-4000-8000-000000000001", "ORG-1")
    signedInAs(join(second, "claude", "accounts", "a1"), "token-3", "6f1e0c4a-0000-4000-8000-000000000002", "org-2")
    for (const each of ["token-1", "token-2", "token-3"]) anthropic.answers.set(each, usage(10, 20))
    const one = new Limits(() => agentAccounts([new TestClaude(first, { id: "default", home: join(first, "claude", "home") })]), undefined, clock.now)
    const two = new Limits(() => agentAccounts([new TestClaude(second, { id: "default", home: join(second, "claude", "home") }), new TestClaude(second, { id: "a1", label: "Athas", home: join(second, "claude", "accounts", "a1") })]), undefined, clock.now)
    await one.tick()
    clock.pass(60_000)
    await two.tick()
    clock.pass(60_000)
    await two.tick()
    const [mine] = (await route(one)).accounts
    const [same, other] = (await route(two)).accounts
    expect(mine!.identity).toMatch(/^[0-9a-f]{16}$/)
    expect(same!.identity).toBe(mine!.identity!)
    expect(other!.identity).toMatch(/^[0-9a-f]{16}$/)
    expect(other!.identity).not.toBe(mine!.identity!)
    // Nothing of the account itself leaves the box.
    const said = JSON.stringify([await route(one), await route(two)])
    for (const secret of ["6f1e0c4a", "6F1E0C4A", "org-1", "org-2", "ada@example.com", "token-1"]) expect(said).not.toContain(secret)
  })

  /**
   * What Claude Code keeps in its settings folder as it runs with a token (`claude setup-token`'s, as
   * a Mac's Kuvo Code runs it): Anthropic's signed record of the policies of the account the token is
   * for (`aud` its organization, `sub` the account), and its note of which sign-in fetched it.
   */
  function policyRecord(folder: string, token: string, account: string, organization: string, options: { plan?: string; kind?: "token" | "org"; signed?: string } = {}) {
    mkdirSync(folder, { recursive: true })
    const sha256 = (text: string) => createHash("sha256").update(text).digest("hex")
    const body = `sha256:${sha256(`policies of ${account}`)}`
    // A sign-in Claude Code keeps itself (yours in Terminal) is stamped as its organization's.
    const principal = options.kind === "org" ? `org:${organization}` : `token:${sha256(token).slice(0, 32)}`
    writeFileSync(join(folder, "policy-limits.json"), "{}")
    writeFileSync(join(folder, "policy-limits.json.stamp.json"), JSON.stringify({ v: 1, identity: sha256(principal), kind: options.kind ?? "token", sha: body, confirmed_at: clock.at, hipaa_seen: [] }))
    const part = (value: unknown) => Buffer.from(JSON.stringify(value)).toString("base64url")
    const claims = { v: 1, typ: "policy-limits", aud: organization, sub: account, ws: "5e0c0000-0000-4000-8000-0000000000ff", plan: options.plan ?? "consumer", iat: clock.at / 1000, exp: clock.at / 1000 + 86_400, sha: options.signed ?? body }
    writeFileSync(join(folder, "policy-limits.json.signature.json"), JSON.stringify({ jws: `${part({ alg: "ES256", typ: "cc-signed-cache+jws", kid: "k1", x5c: ["MIIB"] })}.${part(claims)}.c2lnbmF0dXJl`, receivedAt: clock.at }))
  }

  test("one Claude account on three computers, a Mac's by Kuvo Code's token among them, is one identity; a second account there is another", async () => {
    // hetzner (Linux): Claude Code's own sign-in. mini-local (a Mac as a machine) and this Mac: Kuvo
    // Code's tokens, which say nothing of who they're for, and Claude Code's record as it ran with each.
    const account = "6F1E0C4A-0000-4000-8000-000000000001"
    const organization = "0A1B2C3D-0000-4000-8000-0000000000aa"
    const hetzner = join(home, "hetzner")
    signedInAs(join(hetzner, "claude", "home"), "token-linux", account, organization)
    anthropic.answers.set("token-linux", usage(29, 70))
    ClaudeAgent.keychain = false
    // (Made once, as the bridge keeps its agents: this test makes the Macs' after it.)
    const linuxClaude = new TestClaude(hetzner, { id: "default", home: join(hetzner, "claude", "home") })
    const linux = new Limits(() => agentAccounts([linuxClaude]), undefined, clock.now)
    await linux.tick()

    ClaudeAgent.keychain = true
    const mini = join(home, "mini-local")
    signIn(mini, "default", "token-mini")
    signIn(mini, "57eea190", "token-athas")
    policyRecord(join(mini, "claude", "home"), "token-mini", account.toLowerCase(), organization.toLowerCase())
    policyRecord(join(mini, "claude", "accounts", "57eea190"), "token-athas", "6f1e0c4a-0000-4000-8000-000000000002", "0a1b2c3d-0000-4000-8000-0000000000bb", { plan: "team" })
    const own = new TestClaude(mini, { id: "default", home: join(mini, "claude", "home") })
    const athas = new TestClaude(mini, { id: "57eea190", label: "Athas", home: join(mini, "claude", "accounts", "57eea190") })
    const machine = new Limits(() => agentAccounts([own, athas]), undefined, clock.now)
    await machine.tick()
    // (A Mac's Claude rows are what its turns hear.)
    own.limits.heard!({ id: "five_hour", used: 0.3, minutes: 300 })
    athas.limits.heard!({ id: "five_hour", used: 0.31, minutes: 300 })

    // This Mac's own box: its default account's settings are yours (`~/.claude`), Kuvo Code's token in its folder.
    const mac = join(home, "mac")
    const yours = join(mac, "your-claude")
    signIn(mac, "default", "token-mac")
    policyRecord(yours, "token-mac", account.toLowerCase(), organization.toLowerCase())
    const before = process.env.CLAUDE_CONFIG_DIR
    process.env.CLAUDE_CONFIG_DIR = yours
    try {
      const macClaude = new TestClaude(mac)
      const macLimits = new Limits(() => agentAccounts([macClaude]), undefined, clock.now)
      await macLimits.tick()
      macClaude.limits.heard!({ id: "five_hour", used: 0.3, minutes: 300 })

      const [linuxRow] = (await route(linux)).accounts
      const [miniRow, athasRow] = (await route(machine)).accounts
      const [macRow] = (await route(macLimits)).accounts
      expect(linuxRow!.identity).toMatch(/^[0-9a-f]{16}$/)
      expect(miniRow!.identity).toBe(linuxRow!.identity!)
      expect(macRow!.identity).toBe(linuxRow!.identity!)
      expect(athasRow!.label).toBe("Athas")
      expect(athasRow!.identity).toMatch(/^[0-9a-f]{16}$/)
      expect(athasRow!.identity).not.toBe(linuxRow!.identity!)
      // Nothing of the account, its organization or the token leaves the box.
      const said = JSON.stringify([await route(machine), await route(macLimits)])
      for (const secret of ["6f1e0c4a", "0a1b2c3d", "token-"]) expect(said).not.toContain(secret)
    } finally {
      if (before === undefined) delete process.env.CLAUDE_CONFIG_DIR
      else process.env.CLAUDE_CONFIG_DIR = before
    }
  })

  test("a Mac's record that isn't for Kuvo Code's token (yours in Terminal, another's signature) says nothing of who it is", async () => {
    ClaudeAgent.keychain = true
    const folder = join(home, "claude", "home")
    signIn(home, "default", "token-kuvo")
    // Your own Claude Code in Terminal ran last in the folder, signed in as someone else.
    policyRecord(folder, "token-yours", "00000000-0000-4000-8000-00000000000a", "00000000-0000-4000-8000-0000000000aa", { kind: "org" })
    const own = new TestClaude(home, { id: "default", home: folder })
    const limits = new Limits(() => agentAccounts([own]), undefined, clock.now)
    await limits.tick()
    own.limits.heard!({ id: "five_hour", used: 0.3, minutes: 300 })
    const [account] = (await route(limits)).accounts
    expect(account!.windows.map((each) => each.used)).toEqual([0.3])
    expect(account!.identity).toBeUndefined()
    // Stamped for Kuvo Code's token, but the signature beside it is of another record.
    policyRecord(folder, "token-kuvo", "00000000-0000-4000-8000-00000000000b", "00000000-0000-4000-8000-0000000000bb", { signed: "sha256:another" })
    clock.pass(60_000)
    await limits.tick()
    expect((await route(limits)).accounts[0]!.identity).toBeUndefined()
    // Claude Code runs with Kuvo Code's token: now it says.
    policyRecord(folder, "token-kuvo", "00000000-0000-4000-8000-00000000000b", "00000000-0000-4000-8000-0000000000bb")
    clock.pass(60_000)
    await limits.tick()
    const known = (await route(limits)).accounts[0]!
    expect(known.identity).toMatch(/^[0-9a-f]{16}$/)
    // Your Terminal's runs again, writing its own over it: who it is, and its numbers, stay.
    policyRecord(folder, "token-yours", "00000000-0000-4000-8000-00000000000a", "00000000-0000-4000-8000-0000000000aa", { kind: "org" })
    clock.pass(60_000)
    await limits.tick()
    const after = (await route(limits)).accounts[0]!
    expect([after.identity, after.windows.map((each) => each.used), after.problem]).toEqual([known.identity, [0.3], undefined])
  })

  test("a Mac's Claude signed in to another account: what's told after is that account's, never the one before's", async () => {
    ClaudeAgent.keychain = true
    const folder = join(home, "claude", "home")
    const file = join(home, "limits.json")
    signIn(home, "default", "token-ada")
    policyRecord(folder, "token-ada", "00000000-0000-4000-8000-00000000000a", "00000000-0000-4000-8000-0000000000aa")
    const own = new TestClaude(home, { id: "default", home: folder })
    const limits = new Limits(() => agentAccounts([own]), file, clock.now)
    await limits.tick()
    own.limits.heard!({ id: "five_hour", used: 0.3, minutes: 300 })
    const ada = (await route(limits)).accounts[0]!
    expect(ada.identity).toMatch(/^[0-9a-f]{16}$/)

    // Signed in as Bob: Claude Code hasn't run with his token yet, and his first turn is heard.
    signIn(home, "default", "token-bob")
    own.limits.heard!({ id: "five_hour", used: 0.9, minutes: 300 })
    const heard = (await route(limits)).accounts[0]!
    expect([heard.identity, heard.windows.map((each) => each.used), heard.problem]).toEqual([undefined, [0.9], undefined])
    // Claude Code's record for his token: his, with what his turn told, and nothing more to say.
    policyRecord(folder, "token-bob", "00000000-0000-4000-8000-00000000000b", "00000000-0000-4000-8000-0000000000bb")
    clock.pass(60_000)
    await limits.tick()
    own.limits.heard!({ id: "five_hour", used: 0.91, minutes: 300 })
    const bob = (await route(limits)).accounts[0]!
    expect(bob.identity).toMatch(/^[0-9a-f]{16}$/)
    expect(bob.identity).not.toBe(ada.identity!)
    expect([bob.windows.map((each) => each.used), bob.problem]).toEqual([[0.91], undefined])
    // Through a restart too.
    const restarted = new Limits(() => agentAccounts([new TestClaude(home, { id: "default", home: folder })]), file, clock.now)
    expect((await route(restarted)).accounts[0]!.identity).toBe(bob.identity!)
  })

  test("signed in to another account since, what was kept of the one before isn't shown as this one's", async () => {
    ClaudeAgent.keychain = false
    const folder = join(home, "claude", "home")
    signedInAs(folder, "token-old", "00000000-0000-4000-8000-00000000000a", "org-a")
    anthropic.answers.set("token-old", usage(80, 70))
    const file = join(home, "limits.json")
    const own = new TestClaude(home, { id: "default", home: folder })
    const limits = new Limits(() => agentAccounts([own]), file, clock.now)
    await limits.tick()
    const before = (await route(limits)).accounts[0]!
    expect(before.windows.map((each) => each.used)).toEqual([0.8, 0.7])

    // Signed in as someone else, and Anthropic holding reads off for now: nothing of the old one stays.
    signedInAs(folder, "token-new", "00000000-0000-4000-8000-00000000000b", "org-b")
    anthropic.answers.set("token-new", () => new Response("{}", { status: 429, headers: { "retry-after": "600" } }))
    clock.pass(60_000)
    await limits.tick()
    const after = (await route(limits)).accounts[0]!
    expect(after.identity).not.toBe(before.identity!)
    expect(after.windows).toEqual([])
    expect(after.problem?.kind).toBe("rate-limited")
    // Through a restart too.
    const restarted = new Limits(() => agentAccounts([own]), file, clock.now)
    expect((await route(restarted)).accounts[0]!.windows).toEqual([])
  })

  test("Codex's ChatGPT sign-in is its account and workspace member, the same as the engine's sign-in to it", async () => withChatGPT(async () => {
    const access = (user: string) => token({ "https://api.openai.com/auth": { chatgpt_account_id: "acct-77", chatgpt_user_id: user }, "https://api.openai.com/profile": { email: "ada@example.com" } })
    codexAuth(access("user-ada"), "acct-77")
    const agent = (await refreshed(codexLimits())).accounts[0]!
    const engine = (signedIn: string) => {
      const accounts = new EngineAccounts(async () => [{ integrationID: "openai", active: true, value: { type: "oauth", access: access(signedIn), expires: clock.at + 3_600_000, metadata: { accountID: "acct-77" } } }])
      return accounts.refresh().then(() => accounts.accounts()[0]!.limits.identity!())
    }
    const { identityOf } = await import("../../src/usage/limits")
    expect(agent.identity).toBe(identityOf((await engine("user-ada"))!))
    expect(identityOf((await engine("user-bob"))!)).not.toBe(agent.identity!)
    expect(JSON.stringify(agent)).not.toContain("acct-77")
  }))

  test("a switched Codex account is read with its own sign-in", async () => withChatGPT(async ({ usage, answers }) => {
    const access = (user: string) => token({ "https://api.openai.com/auth": { chatgpt_account_id: `acct-${user}`, chatgpt_user_id: user } })
    codexAuth(access("ada"), "acct-ada")
    usage({ plan_type: "plus", rate_limit: { primary_window: wham(70) } })
    const limits = codexLimits()
    const first = (await refreshed(limits)).accounts[0]!
    expect(first.windows.map(each => each.used)).toEqual([0.7])
    codexAuth(access("bob"), "acct-bob")
    clock.pass(60_000)
    await limits.tick()
    const switched = (await route(limits)).accounts[0]!
    expect(switched.identity).not.toBe(first.identity!)
    expect(switched.windows.map(each => each.used)).toEqual([0.7])
    expect(switched.problem).toBeUndefined()
    expect(answers.filter(request => request.path === "/usage").map(request => request.account)).toEqual(["acct-ada", "acct-bob"])
  }))

})

describe("plan_limits, an agent's read of them", () => {
  test("says each account's windows, how much is used, when each resets and as of when", async () => {
    ClaudeAgent.keychain = false
    const sign = (folder: string, token: string) => {
      mkdirSync(folder, { recursive: true })
      writeFileSync(join(folder, ".credentials.json"), JSON.stringify({ claudeAiOauth: { accessToken: token, scopes: ["user:profile"] } }))
    }
    sign(join(home, "claude", "home"), "token-own")
    sign(join(home, "claude", "accounts", "1f2e3d4c"), "token-work")
    const own = new TestClaude(home, { id: "default", home: join(home, "claude", "home") })
    const work = new TestClaude(home, { id: "1f2e3d4c", label: "Work", home: join(home, "claude", "accounts", "1f2e3d4c") })
    const real = Date.now()
    const at = (minutes: number) => new Date(real + minutes * 60_000).toISOString()
    anthropic.answers.set("token-own", () => Response.json({ five_hour: { utilization: 42, resets_at: at(130) }, seven_day: { utilization: 18, resets_at: at(4 * 1440) } }))
    anthropic.answers.set("token-work", () => Response.json({}, { status: 429, headers: { "retry-after": "1800" } }))
    let clockAt = real
    const limits = new Limits(() => agentAccounts([own, work]), undefined, () => clockAt)
    // (One Claude account a minute.)
    await limits.tick()
    clockAt += 60_000
    await limits.tick()
    // The bridge as the link reaches it.
    const bridge = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => limitsRoutes(limits, request.method, new URL(request.url).pathname.split("/").slice(4)) })
    try {
      const { planLimitsTool } = await import("../../src/link/work/plan-limits")
      const { text, isError } = await planLimitsTool({ port: bridge.port, password: "x" } as never)
      expect(isError).toBe(false)
      expect(text).toContain("- Claude Code, provider claude-code:")
      expect(text).toContain("5-hour window: 42% used, resets in 2h 10m")
      expect(text).toContain("weekly limit: 18% used, resets in 4 days")
      expect(text).toContain("as of just now")
      // One Anthropic turned away: said so, with when it's read again.
      expect(text).toContain("- Claude Code (Work), provider claude-code@1f2e3d4c:")
      expect(text).toContain("Not read just now: Anthropic is turning away reads of the plan's limits for now. Read again in 31m.")
    } finally {
      bridge.stop(true)
    }
  })

  test("with the coding agents not running, says so as an error", async () => {
    const { planLimitsTool } = await import("../../src/link/work/plan-limits")
    expect((await planLimitsTool(null)).isError).toBe(true)
  })
})

describe("an account's rings (#711)", () => {
  const { limitRows } = require("../../src/usage/limits-rows") as typeof import("../../src/usage/limits-rows")
  const rowOf = (service: string, windows: any[]) =>
    limitRows([{ key: "k", name: "mac", here: true, away: false, accounts: [{ provider: service, service, windows: windows.map((window) => ({ read: 1, source: "usage", ...window })) } as any] }], 0)[0]!

  test("Claude's rows say its two rings, the session and the week, each with how close it is, on the route and its event", async () => {
    ClaudeAgent.keychain = true
    const { heardWindows } = await import("../../src/usage/claude-limits")
    const own = new TestClaude(home)
    signIn(home, "default", "token-own")
    const limits = new Limits(() => agentAccounts([own]), undefined, clock.now)
    const told: any[] = []
    const stop = subscribe((each) => each.type === "kuvo.limits.updated" && told.push(each.data))
    await limits.refresh()
    clock.pass(60_000)
    for (const window of heardWindows({ status: "allowed", rateLimitType: "five_hour", utilization: 0.93, unifiedWindows: { five_hour: { utilization: 0.93, resetsAt: 1_794_200_000 }, seven_day: { utilization: 0.72, resetsAt: 1_794_600_000 }, seven_day_overage_included: { utilization: 0.99, resetsAt: 1_794_600_000 } } })) own.limits.heard!(window)
    await limits.refresh()
    clock.pass(60 * 60_000)
    await limits.tick()
    stop()

    const { rows } = (await route(limits)) as any
    expect(rows).toHaveLength(1)
    expect(rows[0].rings).toEqual({
      short: { window: "five_hour", used: 0.93, resetsAt: 1_794_200_000_000, level: "over" },
      long: { window: "seven_day", used: 0.72, resetsAt: 1_794_600_000_000, level: "near" },
    })
    // Every window says its level; only the two rings say a span (a model's own week is neither).
    expect(rows[0].windows.map((window: any) => [window.id, window.span, window.level])).toEqual([["five_hour", "short", "over"], ["seven_day", "long", "near"], ["seven_day_fable", undefined, "over"]])
    expect(told.at(-1).rows[0].rings).toEqual(rows[0].rings)
    // The box's own list (what peers are told) stays as read: the rings are the rows'.
    expect(told.at(-1).accounts[0].windows[0].level).toBeUndefined()
  })

  test("a plan with one window has one ring, and one with none has none", () => {
    expect(rowOf("grok", [{ id: "weekly", used: 0.2, minutes: 10_080, resetsAt: 5 }]).rings).toEqual({ long: { window: "weekly", used: 0.2, resetsAt: 5, level: "calm" } })
    expect(rowOf("claude", [{ id: "five_hour", used: 0.5, minutes: 300 }]).rings).toEqual({ short: { window: "five_hour", used: 0.5, level: "calm" } })
    // No length said (OpenRouter's key limit): its one limit is the long ring.
    expect(rowOf("openrouter", [{ id: "key", used: 0.7, name: "Key limit" }]).rings).toEqual({ long: { window: "key", used: 0.7, level: "near" } })
    expect(rowOf("ollama", []).rings).toEqual({})
  })

  // One level for the account as a whole (#718), for a client's one mark of it (a gauge): its rings'
  // higher one; with no window, what it has with a most it may come to; none where nothing says a
  // limit, which a client draws as unknown, never as calm.
  test("a row says the account's level: its rings' higher one, a balance's or a capped extra's, and none where nothing has a limit", () => {
    const row = (account: Record<string, unknown>) =>
      limitRows([{ key: "k", name: "mac", here: true, away: false, accounts: [{ provider: "x", service: "x", windows: [], ...account } as any] }], 0)[0]!
    const window = (id: string, used: number, minutes: number) => ({ id, used, minutes, read: 1, source: "usage" })
    expect(row({ windows: [window("five_hour", 0.2, 300), window("seven_day", 0.75, 10_080)] }).level).toBe("near")
    expect(row({ windows: [window("five_hour", 0.95, 300), window("seven_day", 0.1, 10_080)] }).level).toBe("over")
    expect(row({ windows: [window("weekly", 0.1, 10_080)] }).level).toBe("calm")
    // Credits alone, with the most they come to: by what's gone of it.
    expect(row({ balance: { left: 80, limit: 100, unit: "usd" } })).toMatchObject({ rings: {}, level: "calm" })
    expect(row({ balance: { left: 25, limit: 100, unit: "usd" } }).level).toBe("near")
    expect(row({ balance: { left: 4, limit: 100, unit: "credits" } }).level).toBe("over")
    // What it has beyond its windows, capped: what's spent of the cap.
    expect(row({ extras: [{ id: "on_demand", name: "On-demand", kind: "capped", amount: 46, limit: 50, unit: "usd" }] }).level).toBe("over")
    // Nothing says a limit: a balance with no cap, spend alone, nothing at all.
    expect("level" in row({ balance: { left: 12, unit: "usd" } })).toBe(false)
    expect("level" in row({ extras: [{ id: "today", name: "Today", kind: "spent", amount: 3, unit: "usd" }] })).toBe(false)
    expect("level" in row({})).toBe(false)
    // (Its rings say it where it has them, whatever it has beyond them.)
    expect(row({ windows: [window("weekly", 0.1, 10_080)], balance: { left: 1, limit: 100, unit: "usd" } }).level).toBe("calm")
  })

  test("the box picks which of a maker's windows is each ring", () => {
    const spans = (service: string, windows: any[]) => Object.fromEntries(Object.entries(rowOf(service, windows).rings).map(([span, ring]) => [span, ring!.window]))
    // Codex: its own names for them, and a model's windows beside the plan's.
    expect(spans("codex", [{ id: "spark_primary", used: 0.9, minutes: 300, model: "Spark" }, { id: "primary", used: 0.1, minutes: 300 }, { id: "secondary", used: 0.2, minutes: 10_080 }])).toEqual({ short: "primary", long: "secondary" })
    // Z.ai: what it counts apart (web searches) is no ring beside its session and week.
    expect(spans("zai", [{ id: "web_search", used: 0.9, name: "Web searches" }, { id: "session", used: 0.1, minutes: 300 }, { id: "weekly", used: 0.2, minutes: 10_080 }])).toEqual({ short: "session", long: "weekly" })
    // Copilot: a month's counts only; the first is its ring.
    expect(spans("copilot", [{ id: "premium", used: 0.4, minutes: 43_200, name: "Premium requests" }, { id: "chat", used: 0.1, minutes: 43_200, name: "Chat" }])).toEqual({ long: "premium" })
    // A plan with a session, a week and a month: the longest is the long ring.
    expect(spans("opencode", [{ id: "session", used: 0.1, minutes: 300 }, { id: "weekly", used: 0.2, minutes: 10_080 }, { id: "monthly", used: 0.3, minutes: 43_200 }])).toEqual({ short: "session", long: "monthly" })
    // A maker whose every window is a model's: the first model's stand for the plan.
    expect(spans("newer", [{ id: "gemini", used: 0.1, minutes: 300, model: "Gemini Pro" }, { id: "gemini_week", used: 0.2, minutes: 10_080, model: "Gemini Pro" }, { id: "claude", used: 0.9, minutes: 300, model: "Claude" }])).toEqual({ short: "gemini", long: "gemini_week" })
  })

  test("Antigravity's rings are one pool's, as its reader lists them: Gemini's session is never paired with Claude's week", async () => {
    const { antigravitySummary } = await import("../../src/usage/antigravity-plan")
    const bucket = (bucketId: string) => ({ bucketId, remainingFraction: 0.6, resetTime: "2026-11-20T00:00:00Z" })
    const read = (ids: string[]) => antigravitySummary([{ groups: [{ displayName: "Models", buckets: ids.map(bucket) }] }])!
    const rings = (ids: string[]) => Object.fromEntries(Object.entries(rowOf("antigravity", read(ids)).rings).map(([span, ring]) => [span, ring!.window]))
    expect(rings(["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"])).toEqual({ short: "gemini-5h", long: "gemini-weekly" })
    // One of Gemini's missing: its other alone, one ring.
    expect(rings(["gemini-5h", "3p-5h", "3p-weekly"])).toEqual({ short: "gemini-5h" })
    expect(rings(["gemini-weekly", "3p-5h", "3p-weekly"])).toEqual({ long: "gemini-weekly" })
    // Only Claude's pool read: its two.
    expect(rings(["3p-5h", "3p-weekly"])).toEqual({ short: "3p-5h", long: "3p-weekly" })
  })
})

// #668 5a-5b: the box names every row and window, and says which ring one bar shows and which
// window started over, so no client keeps a table of names or a clock of its own for them.
describe("names, the bar's ring and windows that started over (#668)", () => {
  const { limitRows } = require("../../src/usage/limits-rows") as typeof import("../../src/usage/limits-rows")
  const at = (window: any) => ({ read: 1, source: "usage", ...window })
  const box = (key: string, name: string, here: boolean, accounts: any[]) => ({ key, name, here, away: false, accounts: accounts.map((account) => ({ ...account, windows: (account.windows ?? []).map(at) })) })

  test("every provider's rows and windows have a name", () => {
    const accounts = [
      { provider: "claude-code", service: "claude", agent: "claude", account: "default", windows: [{ id: "five_hour", used: 0.1, minutes: 300 }, { id: "seven_day", used: 0.2, minutes: 10_080 }, { id: "seven_day_sonnet", used: 0.3, minutes: 10_080, model: "Sonnet" }] },
      { provider: "claude-code@1f2e3d4c", service: "claude", agent: "claude", account: "1f2e3d4c", label: " Work ", windows: [] },
      { provider: "codex-cli", service: "codex", agent: "codex", account: "default", windows: [{ id: "primary", used: 0.1, minutes: 300 }, { id: "secondary", used: 0.1, minutes: 10_080 }] },
      { provider: "grok-cli", service: "grok", agent: "grok", account: "default", windows: [{ id: "weekly", used: 0.1, minutes: 10_079 }] },
      { provider: "cursor", service: "cursor", agent: "cursor", account: "default", windows: [{ id: "total", used: 0.1, name: "Total usage" }, { id: "grok_bot", used: 0.1, minutes: 4_320, name: "Grok Bot usage" }] },
      { provider: "antigravity", service: "antigravity", agent: "antigravity", account: "default", windows: [{ id: "gemini-5h", used: 0.1, minutes: 300, name: "Gemini models" }] },
      { provider: "openai", service: "codex", windows: [{ id: "primary", used: 0.1, minutes: 480 }] },
      { provider: "xai", service: "grok", windows: [{ id: "weekly", used: 0.1, minutes: 10_080 }] },
      { provider: "github-copilot", service: "copilot", windows: [{ id: "premium", used: 0.1, minutes: 43_200, name: "Premium requests" }] },
      { provider: "openrouter", service: "openrouter", windows: [{ id: "key", used: 0.1, name: "Key limit" }] },
      { provider: "zai-coding-plan", service: "zai", windows: [{ id: "session", used: 0.1, minutes: 300 }, { id: "web_search", used: 0.1, name: "Web searches" }] },
      { provider: "opencode-go", service: "opencode", windows: [{ id: "monthly", used: 0.1, minutes: 44_640 }] },
      { provider: "ollama", service: "ollama", windows: [{ id: "odd", used: 0.1 }] },
      { provider: "newer", service: "newer", windows: [] },
    ]
    const rows = limitRows([box("mac", "Mac", true, accounts)], 0)
    expect(rows.map((row) => row.title)).toEqual(["Claude", "Claude · Work", "Codex", "Grok", "Cursor", "Antigravity", "ChatGPT", "SuperGrok", "Copilot", "OpenRouter", "Z.ai", "OpenCode Go", "Ollama", "Newer"])
    expect(rows.map((row) => row.windows.map((window) => window.title))).toEqual([
      ["Session", "Weekly", "Sonnet"], [], ["Session", "Weekly"], ["6-day"], ["Total usage", "Grok Bot usage"], ["Gemini models"],
      ["8-hour"], ["Weekly"], ["Premium requests"], ["Key limit"], ["Session", "Web searches"], ["Monthly"], ["Limit"], [],
    ])
  })

  test("two rows that would have one name are told apart, as seen from the box that answers", () => {
    const titles = (boxes: any[]) => limitRows(boxes, 0).map((row) => row.title)
    // Codex's own sign-in and the engine's ChatGPT, each a different account here: the engine's is its plan's.
    expect(titles([box("mac", "Mac", true, [{ provider: "codex-cli", service: "codex", agent: "codex" }, { provider: "openai", service: "codex", agent: undefined }])])).toEqual(["Codex", "ChatGPT"])
    // Two Claude accounts that can't say who they are, one here and one on two machines: the other gets its computers' names.
    expect(titles([
      box("mac", "Mac", true, [{ provider: "claude-code", service: "claude", agent: "claude" }]),
      box("h", "hetzner", false, [{ provider: "claude-code", service: "claude", agent: "claude", identity: "aaaaaaaaaaaaaaaa" }]),
      box("l", "lokum", false, [{ provider: "claude-code", service: "claude", agent: "claude", identity: "aaaaaaaaaaaaaaaa" }]),
    ])).toEqual(["Claude", "Claude · hetzner, lokum"])
    // Both here, one also on a machine: that one adds the machine's name.
    expect(titles([
      box("mac", "Mac", true, [{ provider: "claude-code", service: "claude", agent: "claude", identity: "bbbbbbbbbbbbbbbb" }, { provider: "claude-code@x", service: "claude", agent: "claude" }]),
      box("h", "hetzner", false, [{ provider: "claude-code", service: "claude", agent: "claude", identity: "bbbbbbbbbbbbbbbb" }]),
    ])).toEqual(["Claude · hetzner", "Claude"])
  })

  test("one bar shows the fuller ring, the session where they're as full, and a ring that started over counts as empty", () => {
    const bar = (windows: any[], now = 0) => limitRows([box("mac", "Mac", true, [{ provider: "x", service: "x", windows }])], now)[0]!.bar
    expect(bar([{ id: "five_hour", used: 0.3, minutes: 300 }, { id: "seven_day", used: 0.8, minutes: 10_080 }])).toBe("long")
    expect(bar([{ id: "five_hour", used: 0.9, minutes: 300 }, { id: "seven_day", used: 0.8, minutes: 10_080 }])).toBe("short")
    expect(bar([{ id: "five_hour", used: 0.5, minutes: 300 }, { id: "seven_day", used: 0.5, minutes: 10_080 }])).toBe("short")
    expect(bar([{ id: "five_hour", used: 0.9, minutes: 300, resetsAt: 100 }, { id: "seven_day", used: 0.4, minutes: 10_080, resetsAt: 900 }], 100)).toBe("long")
    expect(bar([{ id: "key", used: 0.2, name: "Key limit" }])).toBe("long")
    expect(bar([])).toBeUndefined()
  })

  test("a window whose reset has passed has started over: empty, calm, and without the old period's reset or amount", () => {
    const [row] = limitRows([box("mac", "Mac", true, [{ provider: "x", service: "x", windows: [
      { id: "five_hour", used: 0.95, minutes: 300, resetsAt: 100 },
      { id: "seven_day", used: 0.75, minutes: 10_080, resetsAt: 900 },
      { id: "searches", used: 0.97, name: "Web searches", resetsAt: 50, amount: { used: 97, limit: 100, unit: "requests" } },
    ] }])], 100)
    expect(row!.windows).toEqual([
      { id: "five_hour", used: 0, minutes: 300, read: 1, source: "usage", title: "Session", expired: true, level: "calm", span: "short" },
      { id: "seven_day", used: 0.75, minutes: 10_080, resetsAt: 900, read: 1, source: "usage", title: "Weekly", level: "near", span: "long" },
      { id: "searches", used: 0, name: "Web searches", read: 1, source: "usage", title: "Web searches", expired: true, level: "calm" },
    ])
    expect(row!.rings).toEqual({ short: { window: "five_hour", used: 0, level: "calm" }, long: { window: "seven_day", used: 0.75, resetsAt: 900, level: "near" } })
    expect(row!.level).toBe("near")
  })

  test("on the route and its event: named accounts and rows, and clients told when a window starts over", async () => {
    ClaudeAgent.keychain = true
    const { heardWindows } = await import("../../src/usage/claude-limits")
    const own = new TestClaude(home)
    signIn(home, "default", "token-own")
    const limits = new Limits(() => agentAccounts([own]), undefined, clock.now)
    await limits.refresh()
    const fiveHour = Math.floor(clock.at / 1000) + 3_600
    for (const window of heardWindows({ status: "allowed", rateLimitType: "five_hour", utilization: 0.93, unifiedWindows: { five_hour: { utilization: 0.93, resetsAt: fiveHour }, seven_day: { utilization: 0.72, resetsAt: fiveHour + 86_400 } } })) own.limits.heard!(window)
    await limits.refresh()
    const told: any[] = []
    const stop = subscribe((each) => each.type === "kuvo.limits.updated" && told.push(each.data))
    try {
      const before = (await route(limits)) as any
      expect(before.rows[0]).toMatchObject({ title: "Claude", bar: "short" })
      expect(before.rows[0].windows.map((window: any) => window.title)).toEqual(["Session", "Weekly"])
      expect(before.accounts[0]).toMatchObject({ title: "Claude" })
      expect(before.accounts[0].windows.map((window: any) => window.title)).toEqual(["Session", "Weekly"])
      // The box waits for the session's reset, and tells clients then, with nothing read.
      expect(limits.nextTurn()).toBe(fiveHour * 1000)
      clock.at = fiveHour * 1000
      limits.turn()
      const row = told.at(-1).rows[0]
      expect(row.windows[0]).toMatchObject({ id: "five_hour", used: 0, level: "calm", expired: true })
      expect(row.rings.short).toEqual({ window: "five_hour", used: 0, level: "calm" })
      expect(row.bar).toBe("long")
      expect(told.at(-1).accounts[0].windows[0]).toMatchObject({ id: "five_hour", used: 0, expired: true })
      expect(limits.nextTurn()).toBe((fiveHour + 86_400) * 1000)
    } finally {
      stop()
    }
  })

  test("a box that reads arms a timer for the next window to start over, and stopping clears it", async () => {
    ClaudeAgent.keychain = true
    const { heardWindows } = await import("../../src/usage/claude-limits")
    const own = new TestClaude(home)
    signIn(home, "default", "token-own")
    const limits = new Limits(() => agentAccounts([own]), undefined, clock.now)
    const fiveHour = Math.floor(clock.at / 1000) + 3_600
    limits.start()
    try {
      await limits.refresh()
      expect(limits.awaiting).toBeUndefined()
      for (const window of heardWindows({ status: "allowed", rateLimitType: "five_hour", utilization: 0.5, unifiedWindows: { five_hour: { utilization: 0.5, resetsAt: fiveHour }, seven_day: { utilization: 0.2, resetsAt: fiveHour + 86_400 } } })) own.limits.heard!(window)
      await limits.refresh()
      expect(limits.awaiting).toBe(fiveHour * 1000)
    } finally {
      limits.stop()
    }
    expect(limits.awaiting).toBeUndefined()
  })

  // An agent reading its limits is told a window that started over isn't known, never "0% used".
  test("the agents' plan_limits tool says a window that started over isn't read since", async () => {
    const { describePlanLimits } = await import("../../src/link/work/plan-limits")
    const rows = limitRows([box("mac", "Mac", true, [{ provider: "claude-code", service: "claude", agent: "claude", windows: [{ id: "five_hour", used: 0.95, minutes: 300, resetsAt: 100 }, { id: "seven_day", used: 0.4, minutes: 10_080, resetsAt: 900 }] }])], 100)
    const text = describePlanLimits({ accounts: [], rows }, 100)
    expect(text).toContain("5-hour window: started over, not read since")
    expect(text).not.toContain("5-hour window: 0% used")
    expect(text).toContain("weekly limit: 40% used")
  })

  // A plan that ended, as the newest reading says, while another computer still kept its old week
  // (turned away since): the row has no windows or rings, and an agent is told it ended.
  test("a plan that ended is one, whatever another computer kept of it", async () => {
    const { describePlanLimits } = await import("../../src/link/work/plan-limits")
    const ended = { provider: "cursor", service: "cursor", agent: "cursor", identity: "u1", windows: [], checked: 90, problem: { kind: "ended", message: "Its Cursor plan isn't active now." } } as any
    const kept = { provider: "cursor", service: "cursor", agent: "cursor", identity: "u1", windows: [{ id: "total", used: 0.3, minutes: 43_200, resetsAt: 900, read: 10 }], checked: 50, problem: { kind: "rate-limited", message: "Cursor is turning reads away for now." } } as any
    const [row] = limitRows([box("mac", "Mac", true, [ended]), box("dev", "devbox", false, [kept])], 100)
    expect([row!.ended, row!.windows, row!.rings, row!.level]).toEqual([true, [], {}, undefined])
    expect(describePlanLimits({ accounts: [], rows: [row] }, 100)).toContain("its plan has ended")
  })
})

test("a newly added provider is read immediately before its first scheduled tick", async () => {
  let reads = 0
  const limits = new Limits(() => [{ provider: "codex-cli@new", service: "codex", limits: { every: 60_000, read: async () => { reads++; return null } } }])
  await limits.readAgain("codex-cli@new")
  expect(reads).toBe(1)
})
