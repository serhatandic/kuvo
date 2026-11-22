import { fingerprint } from "./limits"
// Claude's plan limits for one account (limits.ts), asked of Claude Code itself in the account's
// folder (its own `/usage`, native-usage.ts): it reads and renews its own sign-in as it answers, on
// a Mac from the Keychain, and Kuvo never sends its token anywhere. Anthropic limits these reads
// hard, for the whole computer rather than a sign-in, so the box asks each account at most every
// 15 minutes, one at a time. What a turn hears (Claude Code's rate-limit events) keeps them current
// between reads. Its one-off resets have no ask of Claude Code's: they're read and spent as Claude
// Code does only where it keeps its sign-in in a file (Linux).

import { readFileSync, statSync } from "node:fs"
import { join } from "node:path"
import { askClaudeUsage, type ClaudeProgram } from "./native-usage"
import { LimitsProblem, type LimitExtra, type LimitsSource, type LimitReading, type LimitWindow, type ResetAttempt, type ResetOutcome } from "./limits"

/** Where the reads go: Anthropic's, or a test's stand-in. */
export const claudeUsage = { url: "https://api.anthropic.com/api/oauth/usage" }

/** Fifteen minutes between two reads of an account. */
export const CLAUDE_LIMITS_EVERY = 15 * 60_000

/** An hour between two reads of an account's reset grants. */
export const CLAUDE_GRANTS_EVERY = 60 * 60_000

/** Why a Mac's Claude account shows no resets. */
export const KEYCHAIN_RESETS = "Claude Code keeps this Mac's sign-in in the Keychain, so its resets show only from a Linux computer of yours signed in to Claude Code with the same account."

/** A Mac's Claude account's resets, as its row says them. */
const KEYCHAIN_RESETS_FACT: LimitExtra = { id: "resets", name: "Rate Limit Resets", kind: "unreadable", message: KEYCHAIN_RESETS }

/** A sign-in to read with: its token, and what Claude Code kept of it beside. */
export interface ClaudeSignIn {
  token: string
  /** "max", "pro"…, as Claude Code kept it as it signed in. */
  subscription?: string
  /** "default_claude_max_20x"…: its plan's size. */
  tier?: string
  /** When the token lapses, in ms. */
  expiresAt?: number
  /** What it was granted; reading the plan's limits takes `user:profile`. */
  scopes?: string[]
}

/** Claude Code's own sign-in file in a settings folder, for its resets (Linux; a Mac's is in the Keychain, never read here). */
export function credentialsIn(configHome: string): ClaudeSignIn | undefined {
  try {
    const oauth = JSON.parse(readFileSync(join(configHome, ".credentials.json"), "utf8"))?.claudeAiOauth
    if (typeof oauth?.accessToken !== "string" || !oauth.accessToken) return undefined
    return {
      token: oauth.accessToken,
      ...(typeof oauth.subscriptionType === "string" ? { subscription: oauth.subscriptionType } : {}),
      ...(typeof oauth.rateLimitTier === "string" ? { tier: oauth.rateLimitTier } : {}),
      ...(typeof oauth.expiresAt === "number" ? { expiresAt: oauth.expiresAt } : {}),
      ...(Array.isArray(oauth.scopes) ? { scopes: oauth.scopes.filter((scope: unknown) => typeof scope === "string") } : {}),
    }
  } catch {
    return undefined
  }
}

/**
 * Who a Claude Code sign-in is, as it keeps it beside the sign-in (`.claude.json`'s `oauthAccount`,
 * in `recordHome`: its folder when it's given one, else your home): the account and the organization
 * whose plan it uses (one account can have a personal plan and a team's, each with its own limits),
 * as `anthropic:<account>|<organization>`.
 */
export function claudeIdentity(recordHome: string): string | undefined {
  const file = join(recordHome, ".claude.json")
  try {
    // (It keeps each project's state too, and can be large: read again only when it changed.)
    const { mtimeMs, size } = statSync(file)
    const known = identities.get(file)
    if (known && known.mtimeMs === mtimeMs && known.size === size) return known.identity
    const identity = identityIn(readFileSync(file, "utf8"))
    identities.set(file, { mtimeMs, size, identity })
    return identity
  } catch {
    return undefined
  }
}

/** Its sign-in, as a fingerprint of who it is (the same through Claude Code's renewals, which Kuvo never sees). */
export function claudeSignInMark(recordHome: string): string | undefined {
  return fingerprint(claudeIdentity(recordHome))
}

const identities = new Map<string, { mtimeMs: number; size: number; identity: string | undefined }>()

function identityIn(text: string): string | undefined {
  try {
    const account = JSON.parse(text)?.oauthAccount
    const uuid = typeof account?.accountUuid === "string" ? account.accountUuid.trim().toLowerCase() : ""
    if (!uuid) return undefined
    const organization = typeof account.organizationUuid === "string" ? account.organizationUuid.trim().toLowerCase() : ""
    return `anthropic:${uuid}|${organization}`
  } catch {
    return undefined
  }
}

/** The windows Claude's answer has: the plan's five hours and week, and each model's own week. */
export function claudeWindows(body: any): Omit<LimitWindow, "read" | "source">[] {
  const windows: Omit<LimitWindow, "read" | "source">[] = []
  const add = (id: string, value: any, minutes: number, model?: string) => {
    const used = Number(value?.utilization)
    if (value == null || value.utilization == null || !Number.isFinite(used)) return
    const resets = typeof value.resets_at === "string" ? Date.parse(value.resets_at) : NaN
    windows.push({ id, used: used / 100, minutes, ...(Number.isFinite(resets) ? { resetsAt: resets } : {}), ...(model ? { model } : {}) })
  }
  add("five_hour", body?.five_hour, 300)
  add("seven_day", body?.seven_day, 10_080)
  add("seven_day_sonnet", body?.seven_day_sonnet, 10_080, "Sonnet")
  add("seven_day_opus", body?.seven_day_opus, 10_080, "Opus")
  // Each model's own week, as Claude Code lists them (`model_scoped`).
  for (const scoped of Array.isArray(body?.model_scoped) ? body.model_scoped : []) {
    const model = scoped?.display_name
    if (typeof model !== "string" || !model) continue
    const id = `seven_day_${model.toLowerCase().replace(/[^a-z0-9]+/g, "_")}`
    if (windows.some((window) => window.id === id)) continue
    add(id, scoped, 10_080, model)
  }
  return windows
}

/** "Max", "Pro": the plan, as Claude Code says it. */
const planOf = (subscription: unknown) =>
  typeof subscription === "string" && subscription.trim() ? subscription.trim().charAt(0).toUpperCase() + subscription.trim().slice(1).toLowerCase() : undefined

/** Claude Code's `/usage` answer (`get_usage`) as a reading: its plan's windows, each model's week, and its extra usage. */
export function claudeReading(answer: any): LimitReading {
  const plan = planOf(answer?.subscription_type)
  const extra = claudeExtraUsage(answer?.rate_limits?.extra_usage)
  return { windows: claudeWindows(answer?.rate_limits), ...(plan ? { plan } : {}), ...(extra ? { extras: [extra] } : {}) }
}

/**
 * Claude's extra usage, where it's on (`extra_usage`, in cents): what's used of its monthly cap, or,
 * without one, what's spent.
 */
export function claudeExtraUsage(value: any): LimitExtra | undefined {
  if (!value || typeof value !== "object" || value.is_enabled !== true) return undefined
  const used = Number(value.used_credits)
  if (!Number.isFinite(used)) return undefined
  const limit = Number(value.monthly_limit)
  const resets = typeof value.resets_at === "string" ? Date.parse(value.resets_at) : NaN
  const resetsAt = Number.isFinite(resets) ? { resetsAt: resets } : {}
  if (Number.isFinite(limit) && limit > 0) return { id: "extra_usage", name: "Extra Usage", kind: "capped", amount: Math.max(0, used) / 100, limit: limit / 100, unit: "usd", ...resetsAt }
  return { id: "extra_usage", name: "Extra Usage", kind: "spent", amount: Math.max(0, used) / 100, unit: "usd", ...resetsAt }
}

/** Anthropic's one-off resets of an account's limits (its `cedar_ember` program), as last read. */
export interface ClaudeGrants {
  eligible: boolean
  grants: { id: string; left: number; endsAt?: number; clears: string[] }[]
  /** The grant a reset spends now, as Anthropic names it. */
  next?: string
}

/** The grants Anthropic states (`cedar_ember`); none for a plan outside the program, or one not read whole (never half trusted). */
export function claudeGrants(value: any): ClaudeGrants | undefined {
  if (!value || typeof value !== "object") return undefined
  const grants: ClaudeGrants["grants"] = []
  for (const grant of Array.isArray(value.grants) ? value.grants : []) {
    const left = Number(grant?.resets_left)
    if (typeof grant?.id !== "string" || !grant.id || !Number.isFinite(left)) return undefined
    const ends = typeof grant.ends_at === "string" ? Date.parse(grant.ends_at) : typeof grant.ends_at === "number" ? grant.ends_at * (grant.ends_at < 1e12 ? 1000 : 1) : NaN
    grants.push({ id: grant.id, left: Math.max(0, Math.floor(left)), ...(Number.isFinite(ends) ? { endsAt: ends } : {}), clears: Array.isArray(grant.clears) ? grant.clears.filter((each: unknown) => typeof each === "string") : [] })
  }
  return { eligible: value.eligible === true, grants, ...(typeof value.next_grant_id === "string" && value.next_grant_id ? { next: value.next_grant_id } : {}) }
}

/** The grant a reset would spend now: the one Anthropic names next, with a reset left, for an eligible account. */
export function claimableGrant(grants: ClaudeGrants | undefined, now: number): ClaudeGrants["grants"][number] | undefined {
  if (!grants?.eligible || !grants.next) return undefined
  return grants.grants.find((grant) => grant.id === grants.next && grant.left > 0 && (grant.endsAt === undefined || grant.endsAt > now))
}

/** The grants as a `resets` extra: each reset left of an unexpired grant, for an eligible account. */
export function grantsExtra(grants: ClaudeGrants, now: number): LimitExtra {
  let count = 0
  const expiries: number[] = []
  if (grants.eligible) {
    for (const grant of grants.grants) {
      if (grant.left < 1 || (grant.endsAt !== undefined && grant.endsAt <= now)) continue
      count += grant.left
      if (grant.endsAt !== undefined) expiries.push(...Array(Math.min(grant.left, 50)).fill(grant.endsAt))
    }
  }
  const next = claimableGrant(grants, now)
  return { id: "resets", name: "Rate Limit Resets", kind: "resets", count, ...(expiries.length ? { expiries: expiries.sort((a, b) => a - b).slice(0, 50) } : {}), ...(next?.clears.length ? { refills: next.clears } : {}), usable: !!next }
}

/** Claude Code's own headers, as it reads its plan with its sign-in. */
const claudeHeaders = (token: string, version: string | undefined) => ({
  Authorization: `Bearer ${token.trim()}`,
  Accept: "application/json",
  "Content-Type": "application/json",
  "anthropic-beta": "oauth-2025-04-20",
  ...(version ? { "User-Agent": `claude-cli/${version} (external, cli)` } : {}),
})

/**
 * The account's grants, read now as Claude Code reads them when it checks for a reset to offer
 * (`cedar_ember=1&skip_spend=1`; Anthropic turns away a client it doesn't take for Claude Code's).
 * Anthropic limits this read hard: the box asks it at most hourly an account. Throws when it can't.
 */
export async function readClaudeGrants(signIn: ClaudeSignIn, version: string | undefined): Promise<ClaudeGrants | undefined> {
  const url = new URL(claudeUsage.url)
  url.searchParams.set("cedar_ember", "1")
  url.searchParams.set("skip_spend", "1")
  const response = await fetch(url, { headers: claudeHeaders(signIn.token, version), signal: AbortSignal.timeout(15_000) })
  if (!response.ok) throw new Error(`Anthropic answered ${response.status}.`)
  return claudeGrants(((await response.json()) as any)?.cedar_ember)
}

/**
 * Who a sign-in's token is for, as Anthropic says (its profile: the account and the organization
 * whose plan it uses), in `claudeIdentity`'s form: the token itself says nothing (it isn't one that
 * carries claims), and Claude Code's own record of it (`.claude.json`) can lag a sign-in to another
 * account. None where Anthropic doesn't answer now.
 */
export async function tokenAccount(signIn: ClaudeSignIn, version: string | undefined): Promise<string | undefined> {
  try {
    const response = await fetch(new URL("profile", claudeUsage.url), { headers: claudeHeaders(signIn.token, version), signal: AbortSignal.timeout(15_000) })
    if (!response.ok) return undefined
    const body = (await response.json()) as any
    const account = typeof body?.account?.uuid === "string" ? body.account.uuid.trim().toLowerCase() : ""
    const organization = typeof body?.organization?.uuid === "string" ? body.organization.uuid.trim().toLowerCase() : ""
    return account ? `anthropic:${account}|${organization}` : undefined
  } catch {
    return undefined
  }
}

/**
 * Whether two of `claudeIdentity`'s forms are one account: the same account, and the same
 * organization where both say one (Claude Code's record of a sign-in may leave it out).
 */
export function sameClaudeAccount(a: string, b: string): boolean {
  const [accountA, organizationA = ""] = a.replace(/^anthropic:/, "").split("|")
  const [accountB, organizationB = ""] = b.replace(/^anthropic:/, "").split("|")
  return !!accountA && accountA === accountB && (!organizationA || !organizationB || organizationA === organizationB)
}

/** The organization a sign-in claims against: as Claude Code keeps it beside the sign-in, else as Anthropic's profile says. */
export async function claudeOrganization(configHome: string | undefined, signIn: ClaudeSignIn, version: string | undefined): Promise<string | undefined> {
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
  const kept = configHome ? claudeIdentity(configHome)?.split("|")[1] : undefined
  if (kept && uuid.test(kept)) return kept
  try {
    const response = await fetch(new URL("profile", claudeUsage.url), { headers: claudeHeaders(signIn.token, version), signal: AbortSignal.timeout(15_000) })
    if (!response.ok) return undefined
    const organization = ((await response.json()) as any)?.organization?.uuid
    return typeof organization === "string" && uuid.test(organization) ? organization : undefined
  } catch {
    return undefined
  }
}

/**
 * Spends one reset grant, as Claude Code does (`POST /api/organizations/<org>/reset_rate_limits`,
 * with the request id the box's journal keeps: asked again, Anthropic answers `already_used`).
 * `grant`: the one to spend, as last read (`claimableGrant`).
 */
export async function claimClaudeGrant(signIn: ClaudeSignIn | undefined, version: string | undefined, organization: string | undefined, grant: string | undefined, attempt: ResetAttempt): Promise<{ outcome: ResetOutcome; until?: number; already?: true }> {
  const target = attempt.target ?? grant
  if (!signIn || !target) return { outcome: "no-reset" }
  if (!organization || !/^[a-z0-9_-]{1,40}$/.test(target) || !/^[A-Za-z0-9_-]{1,64}$/.test(attempt.request)) return { outcome: "failed" }
  if (!attempt.target) attempt.keep(target)
  try {
    const response = await fetch(new URL(`../organizations/${organization}/reset_rate_limits`, claudeUsage.url.replace(/\/oauth\/usage$/, "/oauth/")), {
      method: "POST",
      headers: claudeHeaders(signIn.token, version),
      body: JSON.stringify({ program: "cedar_ember", grant_id: target, request_id: attempt.request }),
      signal: AbortSignal.timeout(25_000),
    })
    // Turned away as asked: nothing spent. Turned away for its sign-in or for now, or failing:
    // whether an earlier try went through isn't said.
    if (!response.ok) return { outcome: response.status >= 500 || [401, 403, 429].includes(response.status) ? "unknown" : "failed" }
    const body: any = await response.json().catch(() => undefined)
    const cooldown = typeof body?.cooldown_until === "string" ? Date.parse(body.cooldown_until) : NaN
    const until = Number.isFinite(cooldown) ? { until: cooldown } : {}
    const code = body?.result === "unavailable" ? body?.reason : body?.result
    switch (code) {
      case "reset":
        return { outcome: "reset" }
      case "already_used":
        return { outcome: "reset", already: true }
      case "not_limited":
        return { outcome: "nothing-to-reset" }
      case "ineligible":
      case "no_grant":
      case "unknown_grant":
      case "not_next_grant":
        return { outcome: "no-reset" }
      case "reset_unconfirmed":
      case "stamp_indeterminate":
        return { outcome: "unconfirmed" }
      case "cooldown":
      case "paused":
      case "low_priority_active":
        return { outcome: "not-now", ...until }
      default:
        return { outcome: "unknown" }
    }
  } catch {
    return { outcome: "unknown" }
  }
}

/**
 * The windows a turn heard of (Claude Code's `rate_limit_event`): every window it tracks
 * (`unifiedWindows`, read from each response's headers, from Claude Code 2.1.3xx), else the one that
 * limits it now, when it says enough to keep.
 */
export function heardWindows(info: any): Omit<LimitWindow, "read" | "source">[] {
  const windows: Omit<LimitWindow, "read" | "source">[] = []
  const unified = info?.unifiedWindows
  if (unified && typeof unified === "object") {
    for (const [id, window] of Object.entries<any>(unified)) {
      const shape = HEARD[id]
      const used = Number(window?.utilization)
      if (!shape || !Number.isFinite(used)) continue
      const resetsAt = Number(window.resetsAt)
      windows.push({ id: shape.id, used, minutes: shape.minutes, ...(Number.isFinite(resetsAt) && resetsAt > 0 ? { resetsAt: resetsAt * 1000 } : {}), ...(shape.model ? { model: shape.model } : {}) })
    }
    if (windows.length) return windows
  }
  const shape = HEARD[info?.rateLimitType]
  if (!shape) return []
  const resetsAt = typeof info.resetsAt === "number" ? info.resetsAt * 1000 : undefined
  // How much is used, as it says; turned away with no word, all of it.
  const used = typeof info.utilization === "number" ? info.utilization : info.status === "rejected" ? 1 : undefined
  if (used === undefined) return []
  return [{ id: shape.id, used, minutes: shape.minutes, ...(resetsAt ? { resetsAt } : {}), ...(shape.model ? { model: shape.model } : {}) }]
}

/** How much of a limit is gone before a warning calls it close: 90%, where the Mac's usage panel turns a bar red. */
export const CLOSE_TO_LIMIT = 0.9

/**
 * What a turn says when Claude Code warns of a limit (`allowed_warning`): how much of which one is
 * used, and whether that's close. Claude Code warns early in a window too (a quarter of the week
 * gone in its first day), so it's "close" only once nearly all of it is.
 */
export function limitWarning(info: any): { text: string; close: boolean } {
  const limit = warnedLimit(info?.rateLimitType)
  const stated = [info?.utilization, info?.unifiedWindows?.[info?.rateLimitType]?.utilization].find((value) => typeof value === "number" && Number.isFinite(value))
  if (stated !== undefined) {
    // (Never all of it till it is: 99.6% is 99%.)
    const percent = stated < 1 ? Math.min(Math.round(Math.max(stated, 0) * 100), 99) : 100
    if (percent === 100) return { text: `You've used all of ${limit}.`, close: true }
    const close = percent >= CLOSE_TO_LIMIT * 100
    return { text: close ? `You're close to ${limit}: ${percent}% used.` : `You've used ${percent}% of ${limit}.`, close }
  }
  // No figure: the threshold it passed, at least.
  const passed = typeof info?.surpassedThreshold === "number" ? info.surpassedThreshold : NaN
  if (!(passed > 0)) return { text: `Claude Code warned about ${limit}.`, close: false }
  const close = passed >= CLOSE_TO_LIMIT
  return { text: close ? `You're close to ${limit}.` : `You've used over ${Math.round(passed * 100)}% of ${limit}.`, close }
}

/** The limit Claude Code warns of, by its name for it. */
function warnedLimit(type: unknown): string {
  if (type === "five_hour") return "your 5-hour Claude limit"
  if (type === "overage") return "your Claude extra usage limit"
  const shape = typeof type === "string" ? HEARD[type] : undefined
  if (shape?.minutes === 10_080) return shape.model ? `your weekly ${shape.model} limit` : "your weekly Claude limit"
  return "your Claude usage limit"
}

/** Claude Code's windows, as the usage endpoint names them (its overage-included week is Fable's, as Claude Code calls it). */
const HEARD: Record<string, { id: string; minutes: number; model?: string }> = {
  five_hour: { id: "five_hour", minutes: 300 },
  seven_day: { id: "seven_day", minutes: 10_080 },
  seven_day_sonnet: { id: "seven_day_sonnet", minutes: 10_080, model: "Sonnet" },
  seven_day_opus: { id: "seven_day_opus", minutes: 10_080, model: "Opus" },
  seven_day_overage_included: { id: "seven_day_fable", minutes: 10_080, model: "Fable" },
}

export class ClaudePlan {
  constructor(private readonly options: {
    /** Where Claude Code keeps who's signed in (`.claude.json`). */
    recordHome: string
    /** Claude Code, with the account's folder, to ask (none: not installed). */
    program(): ClaudeProgram | undefined
    /** Its sign-in file, where Claude Code keeps it in one (Linux), for its resets. */
    signIn(): ClaudeSignIn | undefined
    version(): Promise<string | undefined>
    /** Whether Claude Code keeps its sign-in in the Keychain here (a Mac): its resets can't be read. */
    keychain: boolean
  }) { this.limits = this.source() }

  readonly limits: LimitsSource

  private source(): LimitsSource {
    const { recordHome } = this.options
    const files = !this.options.keychain
    return {
      every: CLAUDE_LIMITS_EVERY,
      bucket: "anthropic",
      read: async (now) => {
        const program = this.options.program()
        if (!program) throw new LimitsProblem("failed", "Claude Code isn't installed here.")
        const reading = claudeReading(await askClaudeUsage(program))
        // Its resets, read after its windows, at most hourly (Anthropic limits this read hard).
        const signIn = files ? this.options.signIn() : undefined
        // (Kept in the Keychain: why they don't show, beside what was read, which a fact alone would give way to.)
        const resets = signIn ? await this.grantsExtra(signIn, await this.options.version(), now) : files ? undefined : KEYCHAIN_RESETS_FACT
        return resets ? { ...reading, extras: [...(reading.extras ?? []), resets] } : reading
      },
      signIn: () => claudeSignInMark(recordHome),
      identity: () => claudeIdentity(recordHome),
      ...(files ? {
        reset: async (attempt: ResetAttempt, now: number) => {
          // (The very sign-in Anthropic was asked about, where it was: one signed in since isn't spent with.)
          const signIn = (attempt.signIn as ClaudeSignIn | undefined) ?? this.options.signIn()
          const version = signIn ? await this.options.version() : undefined
          // None read since the box started (a restart since its row said it has them): read now,
          // as the hourly read would.
          if (signIn && !(this.grants && this.grants.as === fingerprint(signIn.token))) await this.grantsExtra(signIn, version, now)
          const grants = this.grants && this.grants.as === fingerprint(signIn?.token) ? this.grants.status : undefined
          const grant = claimableGrant(grants, now)?.id
          if (!signIn || (!grant && !attempt.target)) return { outcome: "no-reset" as const }
          return claimClaudeGrant(signIn, version, await claudeOrganization(recordHome, signIn, version), grant, attempt)
        },
        // Read again at the next read, what was read standing meanwhile (a read that fails then
        // keeps it, never none).
        afterReset: () => {
          if (this.grants) this.grants = { ...this.grants, at: -Infinity }
        },
        // Whether its sign-in is the account Claude Code's record says, as Anthropic says, right
        // before a reset is spent with it.
        spender: async () => {
          const signIn = this.options.signIn()
          const recorded = claudeIdentity(recordHome)
          const token = signIn ? await tokenAccount(signIn, await this.options.version()) : undefined
          return recorded && token ? { same: sameClaudeAccount(recorded, token), signIn } : undefined
        },
      } : {
        // Kept in the Keychain, which Kuvo never reads: its resets show from a Linux computer
        // signed in to the same account, where there's one (limits-rows.ts).
        facts: () => [KEYCHAIN_RESETS_FACT],
      }),
    }
  }

  /** Its reset grants as last read, with which sign-in (a fingerprint), and when. */
  private grants?: { as: string | undefined; at: number; status: ClaudeGrants | undefined }

  /**
   * Its reset grants as a `resets` extra: read again once an hour has gone by since the last try (a
   * failed one counts: an outage or a 429 isn't asked again every read), as last read meanwhile;
   * none where none was ever read.
   */
  private async grantsExtra(signIn: ClaudeSignIn, version: string | undefined, now: number) {
    const as = fingerprint(signIn.token)
    const kept = this.grants && this.grants.as === as ? this.grants : undefined
    if (!kept || now - kept.at >= CLAUDE_GRANTS_EVERY) {
      try {
        this.grants = { as, at: now, status: await readClaudeGrants(signIn, version) }
      } catch {
        this.grants = { as, at: now, status: kept?.status }
      }
    }
    const status = this.grants && this.grants.as === as ? this.grants.status : undefined
    return status ? grantsExtra(status, now) : undefined
  }

}
