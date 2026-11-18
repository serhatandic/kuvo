// Your plans' limits (a 5-hour window, a week, a model's own week, a month's requests) and credits,
// for each account this box runs that's signed in to Kuvo Code (each coding agent's accounts, and the
// engine's sign-ins), read from those sign-ins only, never a tool's own login elsewhere on the
// computer: what the clients' usage menus show, one row per account, and what an agent paces its
// work by (`plan_limits`). Each account reads its own (`LimitsSource`; an agent's is `Agent.limits`),
// and an agent tells what a turn heard on the way (`LimitsSource.heard`). This keeps the
// last reading of each, on disk too, reads again as often as each maker allows, holds off after a
// 429 for as long as it says (every account that shares the maker's limit on reads), and tells the
// clients when anything changes (`kuvo.limits.updated`). With your other computers' boxes it shares
// what it reads (`limits-peers.ts`): an account a peer read lately isn't read here too, and every
// box lists every account across them (`rows`). PROTOCOL.md, "Plan limits".

import { createHash } from "node:crypto"
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import { dirname } from "node:path"
import type { AgentKind } from "./accounts"
import { parseProvider } from "./accounts"
import type { Agent } from "./agent"
import { emit, subscribe } from "./events"
import type { LimitsPeers } from "./limits-peers"
import { resetRoutes, type LimitResets } from "./limits-resets"
import { limitRows, shownAccount, type LimitRow } from "./limits-rows"
import { initialState, meter, newNotice, currentNotice, visibleMetric, notificationAvailability, reconcileNotices, noticeAccountName, transitions, takeNotices, takeStates, type UsageNotice, type NoticeState } from "./usage/notices"
import type { UsageSettings } from "./usage/settings"

/** The services whose limits a box states, by the name clients know them by (their marks and colors). */
export type LimitsService = "claude" | "codex" | "grok" | "cursor" | "antigravity" | "copilot" | "openrouter" | "zai" | "ollama" | "opencode"

/**
 * Services only boxes read, never an app (Antigravity's, since #631 D2): a box covers them whether
 * or not one is signed in now, so a client's own card of one draws only what boxes state.
 */
export const BOX_ONLY_SERVICES: LimitsService[] = ["antigravity"]

/** One window of a plan's limit, as its maker says it. */
export interface LimitWindow {
  /** Its maker's own name for it: `five_hour`, `seven_day`, `seven_day_sonnet`, Codex's `primary`… */
  id: string
  /** How much of it is used, from 0 to 1. */
  used: number
  /** When it starts over, in ms since 1970, when its maker says. */
  resetsAt?: number
  /** How long it is, in minutes, when its maker says (300: five hours; 10080: a week). */
  minutes?: number
  /** The model it's for, as its maker names it ("Sonnet", "Fable"); none for the whole plan's. */
  model?: string
  /** Its maker's name for what it counts, where that isn't a model or the plan's time ("Premium requests", "Web searches"). */
  name?: string
  /** A count's or an amount's own numbers, where the limit is one (300 premium requests, $50): `used` is their share. */
  amount?: { used: number; limit: number; unit: "requests" | "usd" | "credits" }
  /** When this was read, in ms since 1970. */
  read: number
  /** Read from the maker's own account of the plan (`usage`), or heard in a turn (`turn`). */
  source: "usage" | "turn"
}

/**
 * A fact of a plan beyond its windows (PROTOCOL.md, "Plan limits", `extras`), by `kind`:
 * - `spent`: an amount used so far, with no limit to it (`amount`, `unit`): Cursor's on-demand
 *   spend, Claude's extra usage without a cap, an OpenRouter key's spend today, this week and this
 *   month, a Copilot organization's credits and spend this month;
 * - `capped`: an amount used of a limit (`amount` of `limit`, `unit`): Claude's extra usage with a
 *   monthly cap;
 * - `left`: an amount left to spend, with no cap to it (`amount`, `unit`): Codex's credits;
 * - `resets`: resets of the plan's limits the account holds (`count`; each one's expiry, soonest
 *   first, `expiries`; the windows one refills, where its maker says, `refills`), whether one can be
 *   used now (`usable`), and, in a row, the computer that would use it (`on`): Claude's reset
 *   grants, Codex's reset credits;
 * - `unreadable`: what this sign-in can't read (`message`): a Mac's Claude token, which can only
 *   run Claude, can't read its extra usage or resets.
 * A kind a newer box states is passed on as told, its other plain fields kept as they came.
 */
export interface LimitExtra {
  /** Its own name for it, the same from every box: `on_demand`, `extra_usage`, `resets`, `today`… */
  id: string
  /** What a client calls it, as its maker's own label ("On-demand", "Extra Usage", "Rate Limit Resets"). */
  name: string
  kind: "spent" | "capped" | "left" | "resets" | "unreadable" | (string & {})
  amount?: number
  /** For `capped`: the most it may come to. */
  limit?: number
  unit?: "usd" | "credits" | "requests"
  /** When it starts over, in ms since 1970, where its maker says. */
  resetsAt?: number
  /** For `resets`: how many there are now. */
  count?: number
  /** For `resets`: when each expires, soonest first (ms), where its maker says. */
  expiries?: number[]
  /** For `resets`: the windows one refills (`five_hour`, `seven_day`), where its maker says. */
  refills?: string[]
  /** For `resets`: whether one can be used now, from this sign-in. */
  usable?: boolean
  /** For `resets`, in a row: the computer of yours that would use one (this box's first). */
  on?: { key: string; name: string; here?: true }
  /** For `resets`: agents may use one without asking (Always Allow, kept with the account that spends them). */
  agents?: true
  /** For `unreadable`: why, in words. */
  message?: string
  [more: string]: unknown
}

/** How spending a reset went (`LimitsSource.reset`), as its maker answered. */
export type ResetOutcome =
  /** Done: the windows it refills started over (or this same ask was done before). */
  | "reset"
  /** Its maker took it but couldn't say it went through (Anthropic's `reset_unconfirmed`). */
  | "unconfirmed"
  /** Nothing to reset: the account isn't at a limit. Nothing was spent. */
  | "nothing-to-reset"
  /** No reset to spend (used, expired, or this account can't). Nothing was spent. */
  | "no-reset"
  /** Can't be used just now (a cooldown, its maker holding reads off): `until`, where it says. */
  | "not-now"
  /** Its maker turned it away (or nothing was sent): nothing was spent. */
  | "failed"
  /** No answer came (a timeout, the network, its maker failing): it may have been spent; asked again, it goes as the same request, so it can't spend twice. */
  | "unknown"

/**
 * One reset being spent: the request it goes as (its maker's idempotency key) and what it spends
 * (`target`: Anthropic's grant, Codex's credit), kept in the box's journal before any request goes,
 * so a retry (after a crash, a lost answer) is the same request and spends at most once.
 */
export interface ResetAttempt {
  request: string
  target?: string
  /** The sign-in its maker was asked about just before (`LimitsSource.spender`): spent with that one. */
  signIn?: unknown
  /** Keeps `target` in the journal: called before the request that spends it. */
  keep(target: string): void
}

/** What an agent read of its plan just now. */
export interface LimitReading {
  /** Incomplete response: missing windows keep their last successful readings. */
  partial?: boolean
  windows: Omit<LimitWindow, "read" | "source">[]
  /** The plan, in its maker's words ("Max 20x", "Pro"), when it says. */
  plan?: string
  /** Credits left to spend, where the account has them (OpenRouter's), and the most it can, when there's a cap. */
  balance?: { left: number; limit?: number; unit: "usd" | "credits" }
  /** What the plan has beyond its windows (`LimitExtra`). */
  extras?: LimitExtra[]
  /**
   * Only these extras were read now (Codex's reset credits, between reads of its windows): each
   * takes the place of the one of its `id`, and nothing else of the account changes, nor counts as
   * read.
   */
  extrasOnly?: true
  /** Who the account is, where only its maker's answer says (GitHub's login): as `LimitsSource.identity`. */
  identity?: string
}

/**
 * Why an account's limits couldn't be read just now:
 * - `rate-limited`: its maker turned the read away for now (`until`, when it says);
 * - `expired`: its sign-in lapsed or was turned away, and its agent renews it the next time it runs;
 * - `unreadable`: its sign-in can run the agent but not read the plan's limits;
 * - `failed`: anything else (the network, its maker answering something else);
 * - `ended`: its maker says the account has no plan now (a subscription that ended): listed, with
 *   nothing of its windows, so a client shows it as such and draws no ring for it;
 * - `none`: the account has no plan limits to read (signed out, or an API key): it isn't listed.
 */
export type LimitsProblemKind = "rate-limited" | "expired" | "unreadable" | "failed" | "ended" | "none"

export class LimitsProblem extends Error {
  constructor(
    readonly kind: LimitsProblemKind,
    message: string,
    /** When it may be read again, in ms since 1970, when its maker says. */
    readonly until?: number,
    /** Whether a request went to its maker (false: what's kept here said so first). */
    readonly sent = true,
  ) {
    super(message)
  }
}

/** How an account reads its plan's limits. */
export interface LimitsSource {
  /**
   * Reads them now (`now`, the box's clock); throws a `LimitsProblem` when it can't, and answers
   * null when there's nothing to read now (what it has stays). None for an account whose limits only
   * its turns tell (`heard`).
   */
  read?(now: number): Promise<LimitReading | null>
  /**
   * Its sign-in as it is now, as a fingerprint (never the secret): one that lapsed, or can't read its
   * limits, isn't asked again until this changes; for one only its turns tell of (no `read`), what
   * they told goes when it changes.
   */
  signIn?(): string | undefined
  /** The least time between two reads, in ms: as often as its maker's endpoint allows. */
  every: number
  /** Keep this account's last local reading visible after the usual freshness cutoff. */
  keepLastReading?: boolean
  /** Accounts whose reads share one limit at their maker (Anthropic's applies to the computer, not a sign-in): one turned away holds them all. */
  bucket?: string
  /** Set by the box: a window a turn heard of (a rate-limit event), told as it comes. */
  heard?: (window: Omit<LimitWindow, "read" | "source">, generation?: number) => void
  generation?: () => number
  /**
   * Who the account is at its maker, as its sign-in says, the same on every computer signed in to
   * it: its maker's own account ID (`anthropic:<account>|<organization>`, `chatgpt:<account>/<user>`,
   * `xai:<user>`), or `key:<the key>` for an API key. Never an email. Only ever hashed before it
   * leaves (`identityOf`); undefined where the sign-in doesn't say (a Mac's Claude token before
   * Claude Code has run with it).
   */
  identity?(): string | undefined
  /**
   * The lasting part of its sign-in, as a fingerprint (never the secret): the same through its
   * token's renewals, another once it's signed in again. For one whose sign-in doesn't say who it
   * is (no `identity`): who its maker last said it is holds while this stays as it was then
   * (`signIn` where there's none).
   */
  account?(): string | undefined
  /**
   * What's always so of it, read or not (a Mac's Claude token can't read its extra usage or
   * resets: `unreadable`): listed where nothing read says otherwise.
   */
  facts?(): LimitExtra[]
  /**
   * Spends one of the account's resets (its `resets` extra), with its own sign-in: its maker's
   * answer, as a `ResetOutcome` (`unknown` where the request may have gone and no answer said how
   * it went; `already`, its maker said this same request was done before). Never throws. `limits.ts`' journal (`limits-resets.ts`) keeps the
   * attempt before it's called, so a retry is the same request.
   */
  reset?(attempt: ResetAttempt, now: number): Promise<{ outcome: ResetOutcome; until?: number; already?: true }>
  /** A reset was spent (or may have been): what's kept of its resets is read again, now. */
  afterReset?(): void
  /**
   * Whether the sign-in a reset would be spent with is, as its maker says right now, the account
   * `identity` says (`same`), and that sign-in (`signIn`, given back to `reset` as `attempt.signIn`,
   * so it's the very one spent with): asked right before a reset goes, where the sign-in's local
   * files can lag (a new sign-in's record not written yet). Undefined where it can't be told now.
   */
  spender?(): Promise<{ same: boolean; signIn?: unknown } | undefined>
}

/** An account whose limits are read: a coding agent's, or a sign-in of the engine's. */
export interface LimitedAccount {
  /** Its provider: `claude-code`, `claude-code@<id>` for one you added, the engine's `github-copilot`… */
  provider: string
  service: LimitsService
  /** For a coding agent's: which agent, and `default` for its own sign-in or the ID of one you added. */
  agent?: { kind: AgentKind; account: string }
  /** What you called it ("Work"), for one you added. */
  label?: string
  limits: LimitsSource
}

/** One account's limits, as the box states them (`GET api/kuvo/limits`). */
export interface AccountLimits {
  provider: string
  service: LimitsService
  notices?: UsageNotice[]
  noticeState?: Record<string, NoticeState>
  /** For a coding agent's account (absent for the engine's sign-ins). */
  agent?: AgentKind
  account?: string
  label?: string
  /**
   * The account at its maker, the same from every computer signed in to it, so a client lists it
   * once: 16 hex characters of a hash, never the ID itself (`identityOf`). Absent where it isn't known.
   */
  identity?: string
  plan?: string
  windows: LimitWindow[]
  balance?: LimitReading["balance"]
  extras?: LimitExtra[]
  /** Agents may use its resets without asking (Always Allow, kept with it here), whether or not it has resets now. */
  agentsReset?: true
  /** When it was last read from its maker, whatever came of it, in ms. */
  checked?: number
  /** When it will be read again, in ms. */
  next?: number
  /** Why the last read didn't work (the windows are as last read, with when). */
  problem?: { kind: Exclude<LimitsProblemKind, "none">; message: string; until?: number }
}

interface Entry {
  notices?: UsageNotice[]
  noticeState?: Record<string, NoticeState>
  plan?: string
  windows: LimitWindow[]
  /** Missing windows without reset times survive just one partial reading. */
  partialMissing?: string[]
  balance?: LimitReading["balance"]
  extras?: LimitExtra[]
  checked?: number
  next: number
  problem?: { kind: LimitsProblemKind; message: string; until?: number }
  /** The sign-in a lapsed or unreadable one was, as `LimitsSource.signIn` gave it. */
  signIn?: string
  /** Reads turned away in a row (each holds off longer). */
  refusals?: number
  /** Reads that failed in a row. */
  failures?: number
  /** Who it is (`identityOf`), as last known. */
  identity?: string
  /** For an account only its turns tell of (no `read`): the sign-in (`LimitsSource.signIn`) what's kept was heard with. */
  heardAs?: string
  /** When another of your computers' reading of it last stood for a read here, in ms: listed, so your computers know it's signed in here too. */
  elsewhere?: number
  /**
   * For one whose sign-in doesn't say who it is (GitHub Copilot's: its maker's answer does), the
   * sign-in (`LimitsSource.account`) `identity` was last read with: signed in again since, who it
   * is isn't known until it's read again.
   */
  identityAs?: string
}

interface Saved {
  accounts: Record<string, Entry>
  /** Makers' holds on reads, by bucket: until when, in ms. */
  holds: Record<string, number>
  /** When each bucket was last asked, in ms. */
  asked?: Record<string, number>
  /** Reads each bucket turned away in a row. */
  refusals?: Record<string, number>
}

const MINUTE = 60_000
/** A 429 without a time: an hour, then longer each time, up to six hours. */
const FIRST_HOLD = 60 * MINUTE
const LONGEST_HOLD = 6 * 60 * MINUTE
/** A read that failed: five minutes, then longer, up to half an hour. */
const FIRST_RETRY = 5 * MINUTE
const LONGEST_RETRY = 30 * MINUTE
/** An account with nothing to read (signed out, an API key), looked at again. */
const NOTHING_AGAIN = 30 * MINUTE
/** A refresh asked for (a client, an agent's tool) reads an account read longer ago than this. */
const FRESH_ENOUGH = MINUTE
/** Past a peer's next read of an account, a little more before this box reads it, so the two don't both. */
const SHARED_SLACK = 2 * MINUTE

export class Limits {
  private entries = new Map<string, Entry>()
  private generations = new Map<string, number>()
  private holds = new Map<string, number>()
  /** When each maker's bucket was last asked, in ms. */
  private asked = new Map<string, number>()
  /** Reads each maker's bucket turned away in a row. */
  private refusals = new Map<string, number>()
  private reading?: Promise<void>
  private timer?: ReturnType<typeof setInterval>
  private said = ""

  constructor(
    private readonly accounts: () => LimitedAccount[],
    /** Where they're kept between runs (`limits.json` in the bridge's folder); none in a test that doesn't. */
    private readonly file?: string,
    private readonly now: () => number = Date.now,
    /** Before each round of reads: what lists the accounts asked again (the engine's sign-ins). */
    private readonly prepare?: () => Promise<void>,
    /** Whether what lists the accounts has said them all (the engine answered): until then, none is gone. */
    private readonly settled: () => boolean = () => true,
  ) {
    this.load()
    this.said = this.saying()
  }

  /** Your other computers' readings, once the bridge's link is there to carry them (`LimitsPeers`). */
  peers?: LimitsPeers
  settings?: UsageSettings
  ownKey: () => string = () => ""

  notices(): UsageNotice[] {
    const all = [...this.list(false).flatMap(a => a.notices ?? []), ...(this.peers?.notices() ?? [])]
    return reconcileNotices(all.filter(n => currentNotice(n, this.now()))).sort((a, b) => a.at - b.at)
  }

  /** Only a current direct account holder forwards a fact; peers never decide it again. */
  pushableNotices(): UsageNotice[] {
    return this.notices().filter(n => !!n.forwarder && (this.peers?.ownsPush(n.forwarder) ?? n.forwarder === this.ownKey()))
  }

  /** A decision made before the link named this box waits; pin it once, then never re-elect. */
  settleNoticeForwarders() {
    const own = this.peers?.ownKey() || this.ownKey()
    if (!own) return
    let changed = false
    for (const entry of this.entries.values()) for (const notice of entry.notices ?? []) {
      if (notice.forwarder || !currentNotice(notice, this.now())) continue
      const identity = notice.row.startsWith(`${notice.service}#`) ? notice.row.slice(notice.service.length + 1) : undefined
      const forwarder = identity && this.peers ? this.peers.forwarder(notice.service, identity) : own
      if (!forwarder) continue
      notice.forwarder = forwarder
      changed = true
    }
    if (changed) this.save()
  }

  /** Resolve conflicting pins for the same occurrence once; a later peer removal cannot undo it. */
  reconcileNoticeForwarders() {
    const pins = new Map(this.notices().filter(n => n.forwarder).map(n => [n.id, n.forwarder!]))
    let changed = false
    for (const entry of this.entries.values()) for (const notice of entry.notices ?? []) {
      const pin = pins.get(notice.id)
      if (pin && (!notice.forwarder || pin < notice.forwarder)) {
        notice.forwarder = pin
        changed = true
      }
    }
    this.peers?.reconcileForwarders(pins)
    if (changed) this.save()
  }

  deliverNotices(notices: UsageNotice[]) {
    const pushes = new Set(this.pushableNotices().map(n => n.id))
    for (const push of [false, true]) {
      const facts = notices.filter(n => currentNotice(n, this.now()) && pushes.has(n.id) === push)
      if (facts.length) emit("kuvo.usage.notices", { notices: facts, push })
    }
  }

  /** Spending resets, and whether agents may without asking (`limits-resets.ts`). */
  resets?: LimitResets

  /** This box's account of this provider, as it's read. */
  account(provider: string): LimitedAccount | undefined {
    return this.accounts().find((account) => account.provider === provider)
  }

  /** Something of this box's own accounts changed besides a reading (Always Allow): told and kept. */
  ownChanged() {
    this.changed()
  }

  /** Every account across your computers, one row each (`limits-rows.ts`): this box's first. */
  rows(presentUsage = false): LimitRow[] {
    const own = this.list(false)
    return this.peers ? this.peers.rows(own, presentUsage) : limitRows([{ key: "", name: "", here: true, away: false, accounts: own }], this.now(), presentUsage)
  }

  /** What a peer told changed what's listed: the clients are told. */
  peersChanged() {
    this.changed(true, false)
  }

  private saying() {
    return JSON.stringify([this.shown(), this.rows(), this.notices()])
  }

  /** Reads what's due every minute, and listens to what turns hear. */
  start() {
    this.timer ??= setInterval(() => void this.tick(), MINUTE)
    this.timer.unref?.()
    void this.tick()
    this.awaitTurn()
  }

  stop() {
    if (this.timer) clearInterval(this.timer)
    this.timer = undefined
    if (this.turnTimer) clearTimeout(this.turnTimer)
    this.turnTimer = undefined
    this.awaiting = undefined
  }

  /** This box's accounts as clients are shown them (`accounts`): each named, a window whose reset has passed started over. */
  shown(): AccountLimits[] {
    const now = this.now()
    return this.list().map((account) => shownAccount(account, now))
  }

  /**
   * When a window listed starts over next (after now, by the box's clock): what's listed changes
   * then with nothing read, so clients are told (`turn`). None while none says when it resets.
   */
  nextTurn(rows: LimitRow[] = this.rows()): number | undefined {
    const now = this.now()
    let soonest: number | undefined
    for (const row of rows) for (const window of row.windows) if (window.resetsAt && window.resetsAt > now && (soonest === undefined || window.resetsAt < soonest)) soonest = window.resetsAt
    return soonest
  }

  /** A window started over: clients are told what's listed now, and the next one is awaited. */
  turn() {
    this.changed(false, false)
  }

  private turnTimer?: ReturnType<typeof setTimeout>
  /** When the armed timer for the next window to start over fires (by the box's clock), if one is. */
  awaiting?: number

  /** Awaits the next window to start over, while this box reads (`start`). `rows`, as just listed. */
  private awaitTurn(rows?: LimitRow[]) {
    if (this.turnTimer) clearTimeout(this.turnTimer)
    this.turnTimer = undefined
    this.awaiting = undefined
    if (!this.timer) return
    const at = this.nextTurn(rows)
    if (at === undefined) return
    this.awaiting = at
    this.turnTimer = setTimeout(() => this.turn(), Math.min(Math.max(at - this.now(), 0) + 1, 2 ** 31 - 1))
    this.turnTimer.unref?.()
  }

  private recent(source: LimitsSource, window: LimitWindow): boolean {
    return this.now() - window.read <= Math.max(source.read ? 15 * MINUTE : 60 * MINUTE, 2 * source.every)
  }

  /**
   * The accounts with plan limits: each agent's own sign-in first, then the ones you added, then
   * the engine's. `borrowed`: one a peer's reading stood for a read of here is listed with that
   * reading's numbers where they're newer than its own (as clients from before `rows` read this
   * list alone); without, each as read here, as peers and `rows` take them.
   */
  list(borrowed = true): AccountLimits[] {
    const listed: AccountLimits[] = []
    for (const account of this.accounts()) {
      // Every account signed in here is listed (what lists them says which are: Accounts' word for
      // the coding agents'), read or not yet: nothing read is no windows, never another's.
      const entry = this.entries.get(account.provider) ?? { windows: [], next: 0 }
      if (entry.problem?.kind === "none") continue
      const identity = account.limits.identity?.()
      if (identity && entry.identity && identityOf(identity) !== entry.identity) continue
      // One only its turns tell of, with no plan sign-in now (a Mac's Claude with an Anthropic
      // Console account): no plan limits to tell.
      if (!account.limits.read && account.limits.signIn && account.limits.signIn() === undefined) continue
      const problem = entry.problem ? { kind: entry.problem.kind, message: entry.problem.message, ...(entry.problem.until ? { until: entry.problem.until } : {}) } : undefined
      // COMPAT(0.7 limits-own-numbers): a skipped account's own row carries the peer's newer numbers, for clients from before rows. Goes in #717's next part.
      const peer = borrowed && entry.elsewhere && entry.identity ? this.peers?.freshest(account.service, entry.identity) : undefined
      if (peer?.checked && peer.checked > (entry.checked ?? 0)) {
        listed.push({
          provider: account.provider,
          service: account.service,
          ...(account.agent ? { agent: account.agent.kind, account: account.agent.account } : {}),
          ...(account.label ? { label: account.label } : {}),
          identity: entry.identity!,
          ...((peer.plan ?? entry.plan) ? { plan: peer.plan ?? entry.plan } : {}),
          windows: peer.windows.filter(window => this.recent(account.limits, window)),
          ...((peer.balance ?? entry.balance) ? { balance: peer.balance ?? entry.balance } : {}),
          ...(peer.extras?.length ? { extras: peer.extras } : {}),
          ...(entry.checked ? { checked: entry.checked } : {}),
          ...(account.limits.read ? { next: entry.next } : {}),
        })
        continue
      }
      listed.push({
        provider: account.provider,
        service: account.service,
        ...(account.agent ? { agent: account.agent.kind, account: account.agent.account } : {}),
        ...(account.label ? { label: account.label } : {}),
        ...(entry.identity ? { identity: entry.identity } : {}),
        ...(entry.plan ? { plan: entry.plan } : {}),
        windows: account.limits.keepLastReading ? entry.windows : entry.windows.filter(window => this.recent(account.limits, window)),
        ...(entry.notices?.length ? { notices: entry.notices.filter(n => currentNotice(n, this.now())) } : {}),
        ...(entry.noticeState ? { noticeState: entry.noticeState } : {}),
        ...(entry.balance ? { balance: entry.balance } : {}),
        ...(extrasOf(entry, account.limits) ? { extras: extrasOf(entry, account.limits, this.resets?.agentsMay(account.provider)) } : {}),
        ...(this.resets?.agentsMay(account.provider) ? { agentsReset: true as const } : {}),
        ...(entry.checked ? { checked: entry.checked } : {}),
        // (One only its turns tell of is never read.)
        ...(account.limits.read ? { next: entry.next } : {}),
        ...(problem ? { problem } : {}),
      })
    }
    return listed
  }

  /** This box's account a row stands for (its id as `rows` name it), if it's one of this box's. */
  accountOf(rowID: string): LimitedAccount | undefined {
    const own = this.peers?.ownKey() ?? ""
    return this.accounts().find((account) => {
      const identity = this.entries.get(account.provider)?.identity
      return (identity ? `${account.service}#${identity}` : `${own}/${account.provider}`) === rowID
    })
  }

  /** The account signed in to this slot (by provider), by who it is (`<service>#<identity>`, as rows name it); none while that isn't known. */
  accountRow(provider: string): string | undefined {
    const account = this.account(provider)
    const identity = this.entries.get(provider)?.identity
    return account && identity ? `${account.service}#${identity}` : undefined
  }

  /**
   * The account signed in to this slot right now, by who its sign-in says it is (not as last
   * read): what a reset would be spent from. As last read for one whose sign-in doesn't say.
   */
  liveRow(provider: string): string | undefined {
    const account = this.account(provider)
    if (!account) return undefined
    if (!account.limits.identity) return this.accountRow(provider)
    const raw = account.limits.identity()
    return raw ? `${account.service}#${identityOf(raw)}` : undefined
  }

  /** Drop the previous reading before a sign-in or plan change is shown. An in-flight read
   * keeps its old entry, so its answer cannot repopulate this account. */
  invalidate(provider: string) {
    this.generations.set(provider, (this.generations.get(provider) ?? 0) + 1)
    const before = this.entries.get(provider)
    if (!before) return
    const held = before.problem?.kind === "rate-limited" && (before.problem.until ?? before.next) > this.now()
    this.entries.set(provider, { windows: [], next: held ? before.next : 0, ...(held ? { problem: before.problem, refusals: before.refusals } : {}) })
    this.changed()
  }

  /** Accounts to read here at their next round whoever else read them lately (a reset was spent here). */
  private forced = new Set<string>()

  /**
   * Reads this account again now, here, as its maker allows (a reset was spent: what it refilled
   * shows at once), and waits for that round.
   */
  async readAgain(provider: string): Promise<void> {
    // (Just signed in: listed once what lists the accounts says so, at the round's start.)
    if (!this.account(provider)) await this.tick()
    const account = this.account(provider)
    if (!account?.limits.read) return
    let entry = this.entries.get(provider)
    if (!entry) {
      entry = { windows: [], next: 0 }
      this.entries.set(provider, entry)
    }
    if (!this.heldBack(account.limits, entry, this.now())) entry.next = 0
    entry.elsewhere = undefined
    this.forced.add(provider)
    // (A round under way may have passed it already: one more after it.)
    return this.tick().then(() => (this.forced.has(provider) ? this.tick() : undefined))
  }

  /** Who an account here is (`identityOf`), as last known: what its usage rows go by too. */
  identity(provider: string): string | undefined {
    return this.entries.get(provider)?.identity
  }

  /**
   * Whether nothing more is to be asked of this account's maker now (Cursor's usage history waits
   * on it too): its sign-in was turned away and is as it was then, or its maker holds reads off.
   */
  held(provider: string): boolean {
    const entry = this.entries.get(provider)
    const source = this.accounts().find((each) => each.provider === provider)?.limits
    if (!entry || !source) return false
    if (entry.problem?.kind === "expired" && entry.signIn !== undefined && entry.signIn === source.signIn?.()) return true
    return this.heldBack(source, entry, this.now())
  }

  /** The accounts this box reads from their makers itself (not only from what turns hear), by provider. */
  reads(): Set<string> {
    return new Set(this.accounts().filter((account) => account.limits.read).map((account) => account.provider))
  }

  /** The services whose limits this box reads (`claude`, `codex`…), so a client knows what it covers. */
  services(): LimitsService[] {
    return [...new Set([...this.accounts().map((account) => account.service), ...this.covers(), ...this.alsoServices])]
  }

  /** Services read here though none may be signed in now (the engine's, and `BOX_ONLY_SERVICES`). */
  alsoServices: LimitsService[] = []

  /** Services read here whether or not one is signed in now (each coding agent's, signed in or out). */
  covers: () => LimitsService[] = () => []

  /**
   * Reads now each account read longer ago than a minute, unless its maker said to wait or it was
   * read as recently as its maker allows, here or on another of your computers: a client pulled to
   * refresh, or an agent asked. `only`: those a peer asked for.
   */
  /** Coalesce manual refreshes and announce completion even if the readings did not change. */
  refreshing?: Promise<void>
  refreshFailed = false
  refreshCompletedAt?: number

  refreshUsage(): Promise<void> {
    if (this.refreshing) return this.refreshing
    const before = new Map([...this.entries].map(([id, entry]) => [id, entry.checked]))
    this.refreshing = this.refresh().then(() => {
      const checked = [...this.entries].filter(([id, entry]) => entry.checked !== before.get(id))
      // Individual failures appear on their cards. A round with only failures has no new success.
      this.refreshFailed = checked.length > 0 && checked.every(([, entry]) => entry.problem && entry.problem.kind !== "ended")
      if (!this.refreshFailed) this.refreshCompletedAt = this.now()
      this.peers?.refreshElsewhere(this.rows(), this.reads())
    }, () => {
      this.refreshFailed = true
    }).finally(() => {
      this.refreshing = undefined
      emit("kuvo.limits.updated", { accounts: this.shown(), rows: this.rows(), notices: this.notices(), ownNotices: this.pushableNotices(), notificationAvailability: notificationAvailability(), refreshFinished: true })
    })
    return this.refreshing
  }

  async refresh(only?: (account: LimitedAccount, identity: string | undefined) => boolean) {
    const at = this.now()
    for (const account of this.accounts()) {
      const source = account.limits
      const entry = this.entries.get(account.provider)
      if (!source.read || !entry || this.heldBack(source, entry, at)) continue
      if (only && !only(account, entry.identity)) continue
      if (entry.checked && at - entry.checked < Math.max(FRESH_ENOUGH, source.every)) continue
      entry.next = Math.min(entry.next, at)
    }
    await this.tick()
  }

  /** Reads what's due, one account at a time (a busy computer isn't given more). */
  tick(): Promise<void> {
    this.reading ??= this.readDue().finally(() => (this.reading = undefined))
    return this.reading
  }

  private heldBack(source: LimitsSource, entry: Entry, at: number) {
    const hold = source.bucket ? (this.holds.get(source.bucket) ?? 0) : 0
    return at < hold || (entry.problem?.kind === "rate-limited" && at < (entry.problem.until ?? entry.next))
  }

  private async readDue() {
    await this.prepare?.()
    for (const account of this.accounts()) {
      const source = account.limits
      this.listen(account.provider, source)
      let entry = this.entries.get(account.provider)
      if (!entry) {
        entry = { windows: [], next: 0 }
        this.entries.set(account.provider, entry)
      }
      this.switched(entry, source)
      const says = source.identity?.()
      this.identify(entry, says)
      // Signed in again since its maker last said who it is (its sign-in doesn't say): who it is
      // isn't known now, so what was kept of it goes, no peer's reading stands for it, and it's
      // read soon.
      const as = source.account?.() ?? source.signIn?.()
      if (!says && entry.identityAs !== undefined && as !== undefined && as !== entry.identityAs) {
        this.forget(entry)
        entry.identity = undefined
        entry.identityAs = undefined
      }
      if (!source.read) continue
      const at = this.now()
      if (at < entry.next) continue
      // Another account of its maker was turned away meanwhile: this one waits as long.
      const hold = source.bucket ? (this.holds.get(source.bucket) ?? 0) : 0
      if (at < hold) {
        entry.next = hold
        continue
      }
      // Read on another of your computers lately: not asked again here, but a little after that
      // computer's next read, should it not have read it by then (PROTOCOL.md, "Plan limits").
      const known = says || (entry.identityAs !== undefined && entry.identityAs === as) ? entry.identity : undefined
      const elsewhere = known && !this.forced.has(account.provider) ? this.peers?.fresh(account.service, known) : undefined
      if (elsewhere !== undefined && at - elsewhere < source.every) {
        entry.elsewhere = elsewhere
        entry.next = elsewhere + source.every + SHARED_SLACK
        continue
      }
      // One read of a maker's bucket a minute, whatever asked (the timer, a refresh): a hold that
      // just ended isn't met with every account at once.
      const last = source.bucket ? (this.asked.get(source.bucket) ?? 0) : 0
      if (source.bucket && at - last < MINUTE) {
        entry.next = last + MINUTE
        continue
      }
      // A sign-in that lapsed or can't read its limits, as it was: nothing to ask its maker yet.
      if ((entry.problem?.kind === "expired" || entry.problem?.kind === "unreadable") && entry.signIn !== undefined && entry.signIn === source.signIn?.()) {
        entry.next = at + Math.max(source.every, FIRST_RETRY)
        continue
      }
      // Counted as asked before it's sent, and kept so (a restart meanwhile doesn't ask another
      // account of its maker at once); taken back when nothing went.
      const before = source.bucket ? this.asked.get(source.bucket) : undefined
      if (source.bucket) this.asked.set(source.bucket, at)
      this.forced.delete(account.provider)
      const sent = await this.readOne(source, entry, account)
      if (source.bucket && !sent) {
        if (before === undefined) this.asked.delete(source.bucket)
        else this.asked.set(source.bucket, before)
      }
      // Kept at once: a restart meanwhile never asks again what was just asked.
      this.changed()
    }
    this.forgetGone()
    this.changed()
  }

  /** Reads one account; whether a request went to its maker. */
  private async readOne(source: LimitsSource, entry: Entry, account: LimitedAccount): Promise<boolean> {
    const started = this.now()
    const identity = source.identity?.()
    // Not asked again before its time, even if the box stops while it waits for the answer.
    entry.next = Math.max(entry.next, started + source.every)
    this.save()
    try {
      const reading = await source.read!(started)
      if (this.entries.get(account.provider) !== entry || (identity !== undefined && source.identity?.() !== identity)) return true
      const at = this.now()
      // (Another account's since: what was kept goes, before this one's is kept.)
      if (reading) this.identify(entry, reading.identity)
      if (reading?.identity) entry.identityAs = source.account?.() ?? source.signIn?.()
      entry.next = at + source.every
      // Nothing to read now: as it was.
      if (!reading) return false
      // Only some of its extras, read between its windows: each in place of its own, nothing else.
      if (reading.extrasOnly) {
        const given = reading.extras ?? []
        const kept = (entry.extras ?? []).filter((extra) => !given.some((each) => each.id === extra.id))
        entry.extras = [...kept, ...given].length ? [...kept, ...given] : undefined
        return true
      }
      entry.checked = at
      entry.plan = reading.plan ?? entry.plan
      entry.balance = reading.balance
      entry.extras = reading.extras?.length ? reading.extras : undefined
      // What it says now, in place of all it said before; a window a turn told of since it was
      // asked, or that only turns tell, stays as heard.
      const missing = reading.partial ? entry.windows.filter(window => !reading.windows.some(each => each.id === window.id)) : []
      const heard = entry.windows.filter((window) => (reading.partial && !reading.windows.some((each) => each.id === window.id) && (window.resetsAt !== undefined ? at < window.resetsAt : !entry.partialMissing?.includes(window.id))) || (window.source === "turn" && (window.read >= started || !reading.windows.some((each) => each.id === window.id))))
      entry.partialMissing = reading.partial ? missing.map(window => window.id) : undefined
      entry.windows = [...reading.windows.filter((window) => !heard.some((each) => each.id === window.id)).map((window) => ({ ...window, used: clamp(window.used), read: at, source: "usage" as const })), ...heard]
      entry.problem = undefined
      entry.signIn = undefined
      entry.refusals = 0
      entry.failures = 0
      if (source.bucket) this.refusals.delete(source.bucket)
      this.decide(account, entry, reading.windows.filter(w => Number.isFinite(w.used)).map(w => w.id))
      return true
    } catch (caught) {
      if (this.entries.get(account.provider) !== entry || (identity !== undefined && source.identity?.() !== identity)) return true
      const at = this.now()
      // (Who it is may be known by now though its limits weren't: its sign-in says, as the read went.)
      if (!entry.identity) this.identify(entry, source.identity?.())
      entry.checked = at
      const problem = caught instanceof LimitsProblem ? caught : new LimitsProblem("failed", `Couldn't read its limits: ${caught instanceof Error ? caught.message : String(caught)}`)
      entry.problem = { kind: problem.kind, message: problem.message, ...(problem.until ? { until: problem.until } : {}) }
      switch (problem.kind) {
        case "rate-limited": {
          // Counted per maker's bucket where there is one (its hold is the computer's): turned away
          // on one account, then another, is twice.
          const refusals = (source.bucket ? (this.refusals.get(source.bucket) ?? 0) : (entry.refusals ?? 0)) + 1
          entry.refusals = refusals
          if (source.bucket) this.refusals.set(source.bucket, refusals)
          // As long as its maker says, never less than the account's own spacing; with no word, an
          // hour, doubling each time it's turned away again.
          const wait = problem.until ? Math.max(problem.until, at + source.every) : at + Math.min(FIRST_HOLD * 2 ** (refusals - 1), LONGEST_HOLD)
          entry.next = wait
          entry.problem.until = wait
          if (source.bucket) this.holds.set(source.bucket, Math.max(this.holds.get(source.bucket) ?? 0, wait))
          break
        }
        case "none":
          // Signed out, or no plan: nothing kept of it, and looked at again later.
          entry.windows = []
          entry.plan = undefined
          entry.balance = undefined
          entry.extras = undefined
          entry.next = at + NOTHING_AGAIN
          break
        case "ended":
          // No plan now: nothing of what was read before is current (its windows, balance, extras),
          // the plan's name kept to say which ended; asked again now and then, should it start again.
          entry.windows = []
          entry.partialMissing = undefined
          entry.balance = undefined
          entry.extras = undefined
          entry.failures = 0
          entry.next = at + Math.max(source.every, NOTHING_AGAIN)
          break
        case "expired":
        case "unreadable":
          // Its maker isn't asked again until the sign-in changes (looked at each round, here only).
          entry.signIn = source.signIn?.()
          entry.next = at + Math.max(source.every, FIRST_RETRY)
          break
        default:
          entry.failures = (entry.failures ?? 0) + 1
          entry.next = at + Math.max(source.every, Math.min(FIRST_RETRY * 2 ** (entry.failures - 1), LONGEST_RETRY))
      }
      return problem.sent
    }
  }

  /**
   * Who the account is now, as its sign-in says (none: as last known). Signed in to another account
   * since, what was kept of the one before isn't this one's: it goes, and this one is read soon.
   */
  private identify(entry: Entry, raw: string | undefined) {
    if (!raw) return
    const identity = identityOf(raw)
    if (entry.identity === identity) return
    if (entry.identity) this.forget(entry)
    entry.identity = identity
  }

  /** Signed in to another account: what was kept of the one before goes, and this one is read soon. */
  private forget(entry: Entry) {
    entry.windows = []
    entry.notices = undefined
    entry.noticeState = undefined
    entry.partialMissing = undefined
    entry.plan = undefined
    entry.balance = undefined
    entry.extras = undefined
    entry.checked = undefined
    // Listed meanwhile, saying why it has no numbers (a maker's hold can keep it unread for hours).
    entry.problem = { kind: "failed", message: "Signed in to another account here: its limits show once they're read." }
    entry.signIn = undefined
    entry.elsewhere = undefined
    entry.refusals = 0
    entry.failures = 0
    entry.next = 0
  }

  /**
   * An account only its turns tell of, signed in to another since what's kept was heard (a Mac's
   * Claude, given a new token): nothing kept is this one's, nor who it was, which its sign-in says
   * again in time (`identity`). Its windows start over with what its turns hear next.
   */
  private switched(entry: Entry, source: LimitsSource) {
    if (source.read || !source.signIn) return
    const now = source.signIn()
    if (now === undefined) return
    if (entry.heardAs !== undefined && entry.heardAs !== now) {
      entry.windows = []
      entry.notices = undefined
      entry.noticeState = undefined
      entry.plan = undefined
      entry.balance = undefined
      entry.extras = undefined
      entry.checked = undefined
      entry.problem = undefined
      entry.identity = undefined
      entry.elsewhere = undefined
    }
    entry.heardAs = now
  }

  /** What a turn of this account hears (its rate-limit events), kept as the window's latest. */
  private listen(provider: string, source: LimitsSource) {
    if (source.heard) return
    const generation = source.generation = () => this.generations.get(provider) ?? 0
    source.heard = (window, from) => {
      if (from !== undefined && from !== generation()) return
      if (!Number.isFinite(window.used)) return
      let entry = this.entries.get(provider)
      if (!entry) {
        entry = { windows: [], next: 0 }
        this.entries.set(provider, entry)
      }
      this.switched(entry, source)
      // (One that's never read has nothing a turn doesn't make current: whatever it said goes.
      // Nor one whose plan had ended: a turn ran on it, so it has one again.)
      if (entry.problem?.kind === "none" || entry.problem?.kind === "ended" || !source.read) entry.problem = undefined
      const before = entry.windows.find((each) => each.id === window.id)
      const heard: LimitWindow = {
        ...before,
        ...window,
        // What a turn doesn't say (how long it is, the model) is as last read.
        minutes: window.minutes ?? before?.minutes,
        model: window.model ?? before?.model,
        used: clamp(window.used),
        read: this.now(),
        source: "turn",
      }
      if (heard.minutes === undefined) delete heard.minutes
      if (heard.model === undefined) delete heard.model
      if (heard.name === undefined) delete heard.name
      if (heard.resetsAt === undefined) delete heard.resetsAt
      entry.windows = before ? entry.windows.map((each) => (each.id === window.id ? heard : each)) : [...entry.windows, heard]
      const account = this.accounts().find(a => a.provider === provider)
      if (account) this.decide(account, entry, [heard.id])
      this.changed(true)
    }
  }

  private decide(account: LimitedAccount, entry: Entry, ids: string[]) {
    const settings = this.settings?.values()
    const rawIdentity = account.limits.identity?.()
    const cardIdentity = rawIdentity && identityOf(rawIdentity) === entry.identity ? rawIdentity : undefined
    const visible = (window: LimitWindow) => visibleMetric(account.service, window, settings, cardIdentity)
    const toggles = settings?.notifications ?? { underTenPercent: false, healthyToClose: false, closeToRunningOut: false }
    const dormant = (state: NoticeState): NoticeState => ({ ...state, previous: "untracked", fired: [], under: false, primed: false })
    const peers = entry.identity ? this.peers?.noticeStates(account.service, entry.identity) ?? [] : []
    entry.noticeState ??= {}
    const siblings = entry.identity ? [...this.entries.values()].filter(e => e !== entry && e.identity === entry.identity) : []
    const candidates = [...peers, ...siblings.map(e => e.noticeState)]
    for (const candidate of candidates) for (const [id, state] of Object.entries(takeStates(candidate))) {
      if ((state.read ?? 0) > (entry.noticeState[id]?.read ?? -1)) entry.noticeState[id] = structuredClone(state)
    }
    const row = entry.identity ? `${account.service}#${entry.identity}` : `${this.ownKey()}/${account.provider}`
    const created: UsageNotice[] = []
    for (const window of entry.windows.filter(w => ids.includes(w.id))) {
      if (!visible(window)) {
        if (entry.noticeState[window.id]) entry.noticeState[window.id] = dormant(entry.noticeState[window.id]!)
        continue
      }
      const previous = entry.noticeState[window.id] ?? initialState()
      const result = transitions(meter(window, this.now()), (100 - window.used * 100) / 100, window.resetsAt, previous, toggles)
      result.state.fired.push(...result.fire)
      entry.noticeState[window.id] = { ...result.state, read: window.read }
      for (const milestone of result.fire) {
        const notice = newNotice(row, account.service, window, milestone, this.now(), entry.noticeState[window.id]!)
        notice.accountName = noticeAccountName(account.service, account.agent?.kind, account.label)
        const forwarder = entry.identity && this.peers ? this.peers.forwarder(account.service, entry.identity) : this.ownKey()
        if (forwarder) notice.forwarder = forwarder
        created.push(notice)
      }
    }
    const present = new Set(entry.windows.filter(visible).map(w => w.id))
    for (const id of Object.keys(entry.noticeState)) if (!present.has(id)) entry.noticeState[id] = dormant(entry.noticeState[id]!)
    if (created.length) {
      entry.notices = [...(entry.notices ?? []), ...created].filter(n => currentNotice(n, this.now())).slice(-100)
      // New facts must be durable before delivery. Ordinary turn samples retain changed()'s 30s save limit.
      this.save()
      this.deliverNotices(created)
    }
  }

  /** Accounts removed since: their records go. */
  private forgetGone() {
    if (!this.settled()) return
    const here = new Set(this.accounts().map((account) => account.provider))
    for (const provider of this.entries.keys()) if (!here.has(provider)) this.entries.delete(provider)
  }

  /** Tells the clients, and keeps it, when what's listed changed. */
  private lastHeardSave = 0

  /**
   * `heard`: from a turn's event, which comes with each response: kept on disk at most every half
   * minute, and told your other computers at most each minute. `own`: this box's own changed (not
   * only what a peer told).
   */
  private changed(heard = false, own = true) {
    const list = this.shown()
    const rows = this.rows()
    this.awaitTurn(rows)
    const notices = this.notices()
    const now = JSON.stringify([list, rows, notices])
    if (own && (!heard || this.now() - this.lastHeardSave >= 30_000)) {
      if (heard) this.lastHeardSave = this.now()
      this.save()
    }
    if (own) this.peers?.tell(!heard)
    if (now === this.said) return
    this.said = now
    emit("kuvo.limits.updated", { accounts: list, rows, notices, ownNotices: this.pushableNotices(), notificationAvailability: notificationAvailability() })
  }

  private load() {
    if (!this.file) return
    try {
      const saved = JSON.parse(readFileSync(this.file, "utf8")) as Saved
      for (const [provider, entry] of Object.entries(saved.accounts ?? {})) {
        if (!entry || !Array.isArray(entry.windows) || typeof entry.next !== "number") continue
        entry.notices = takeNotices(entry.notices)
        entry.noticeState = takeStates(entry.noticeState)
        this.entries.set(provider, entry)
      }
      for (const [bucket, until] of Object.entries(saved.holds ?? {})) if (typeof until === "number") this.holds.set(bucket, until)
      for (const [bucket, at] of Object.entries(saved.asked ?? {})) if (typeof at === "number") this.asked.set(bucket, at)
      for (const [bucket, count] of Object.entries(saved.refusals ?? {})) if (typeof count === "number") this.refusals.set(bucket, count)
    } catch {}
  }

  private kept = ""

  private save() {
    if (!this.file) return
    const saved = JSON.stringify({ accounts: Object.fromEntries(this.entries), holds: Object.fromEntries(this.holds), asked: Object.fromEntries(this.asked), refusals: Object.fromEntries(this.refusals) } satisfies Saved)
    if (saved === this.kept) return
    try {
      mkdirSync(dirname(this.file), { recursive: true })
      writeFileSync(this.file + ".new", saved)
      this.kept = saved
      renameSync(this.file + ".new", this.file)
    } catch {}
  }
}

/** What an account has beyond its windows: as read, else what's always so of it (`facts`). */
function extrasOf(entry: Entry, source: LimitsSource, agentsMay = false): LimitExtra[] | undefined {
  if (entry.extras?.length) return agentsMay ? entry.extras.map((extra) => (extra.kind === "resets" ? { ...extra, agents: true } : extra)) : entry.extras
  const facts = source.facts?.()
  return facts?.length ? facts : undefined
}

function clamp(used: number) {
  return Number.isFinite(used) ? Math.min(Math.max(used, 0), 1) : 0
}

/** The seconds or date a `Retry-After` header gives, as when it may be asked again (ms since 1970). */
export function retryAt(header: string | null | undefined, now = Date.now()): number | undefined {
  const raw = header?.trim()
  if (!raw) return undefined
  if (/^\d+$/.test(raw)) return now + Number(raw) * 1000
  const date = Date.parse(raw)
  return Number.isFinite(date) ? Math.max(date, now) : undefined
}

const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } })

/**
 * `GET api/kuvo/limits` → `{data: {accounts, services, rows}}`: this box's own accounts (as older
 * clients read them), and every account across your computers (`rows`); `POST
 * api/kuvo/limits/refresh` reads what may be read now, here and, for an account only on another of
 * your computers, there, and answers the same once it's read (PROTOCOL.md, "Plan limits").
 */
export async function limitsRoutes(limits: Limits | undefined, method: string, rest: string[], body: () => Promise<any> = async () => undefined): Promise<Response> {
  if (!limits) return json({ message: "Plan limits aren't read on this computer." }, 503)
  // Spending a reset (limits-resets.ts).
  if (rest[0] === "reset") return resetRoutes(limits.resets, method, rest.slice(1), body)
  const answer = () => {
    const rows = limits.rows(true)
    const usageUpdatedAt = Math.max(0, ...rows.map(row => row.usage?.updatedAt ?? 0)) || undefined
    return json({ data: {
      accounts: limits.shown(), services: limits.services(), rows, usageUpdatedAt,
      refreshPending: !!limits.refreshing, refreshFailed: limits.refreshFailed, refreshCompletedAt: limits.refreshCompletedAt,
      notificationAvailability: notificationAvailability(), notices: limits.notices(), ownNotices: limits.pushableNotices(),
    } })
  }
  if (!rest.length && method === "GET") return answer()
  if (rest.length === 1 && rest[0] === "events" && method === "GET") return limitsEvents()
  if (rest.length === 1 && rest[0] === "refresh" && method === "POST") {
    await Promise.race([limits.refreshUsage(), new Promise((resolve) => setTimeout(resolve, 2_000).unref?.())])
    return answer()
  }
  return json({ message: "Not available for this agent." }, 404)
}

function limitsEvents(): Response {
  const line = new TextEncoder().encode("{}\n")
  let stop = () => {}
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      const unsubscribe = subscribe(event => {
        if (event.type === "kuvo.limits.updated") controller.enqueue(line)
      })
      // Also retry the snapshot GET if an update signal arrived but the separate GET failed.
      const heartbeat = setInterval(() => controller.enqueue(line), 15_000).unref()
      stop = () => {
        unsubscribe()
        clearInterval(heartbeat)
      }
      controller.enqueue(line)
    },
    cancel() { stop() },
  })
  return new Response(stream, { headers: { "Content-Type": "application/x-ndjson", "Cache-Control": "no-cache" } })
}

const AGENT_SERVICES: Record<AgentKind, LimitsService> = { claude: "claude", codex: "codex", grok: "grok", cursor: "cursor", antigravity: "antigravity" }

/** The coding agents' accounts whose limits are read (an agent with `limits`). */
export function agentAccounts(agents: Agent[]): LimitedAccount[] {
  return agents.flatMap((agent) => {
    const parsed = parseProvider(agent.providerID)
    if (!agent.limits || !parsed) return []
    return [{ provider: agent.providerID, service: AGENT_SERVICES[parsed.kind], agent: { kind: parsed.kind, account: parsed.id }, ...(agent.instance.label ? { label: agent.instance.label } : {}), limits: agent.limits }]
  })
}

/** A sign-in's fingerprint, to tell when it changed: never the secret itself. */
export function fingerprint(secret: string | undefined): string | undefined {
  return secret ? createHash("sha256").update(secret).digest("hex").slice(0, 16) : undefined
}

/**
 * An account's identity as the clients get it (`AccountLimits.identity`): the same on every box, so
 * a client lists the account once, and nothing of what it's made of. Its makers' IDs are random
 * (UUIDs, keys) or public (a GitHub login); a hash of its own kind, not the bare SHA-256 of an ID,
 * so it matches nothing outside Kuvo Code.
 */
export function identityOf(raw: string): string {
  return createHash("sha256").update("kuvo-code plan limits account v1\n").update(raw).digest("hex").slice(0, 16)
}

/** What a JSON Web Token says of itself (its middle part), never checked: only to tell who it's for. */
export function tokenClaims(token: string | undefined): Record<string, any> | undefined {
  const part = token?.trim().split(".")[1]
  if (!part) return undefined
  try {
    const claims = JSON.parse(Buffer.from(part, "base64url").toString("utf8"))
    return claims && typeof claims === "object" ? claims : undefined
  } catch {
    return undefined
  }
}

/** A ChatGPT sign-in's account, as its token says (Codex's, and the engine's `openai`, alike). */
export function chatGPTIdentity(access: string | undefined, accountID?: unknown): string | undefined {
  const auth = tokenClaims(access)?.["https://api.openai.com/auth"]
  const account = text(auth?.chatgpt_account_id) ?? text(accountID)
  if (!account) return undefined
  // (A workspace's members share its account: each one's own limits.)
  const user = text(auth?.chatgpt_user_id) ?? text(auth?.user_id)
  return `chatgpt:${account}${user ? `/${user}` : ""}`
}

/** An xAI sign-in's user, as its token says (the Grok agent's, and the engine's `xai`, alike). */
export function xaiIdentity(...tokens: (string | undefined)[]): string | undefined {
  for (const token of tokens) {
    const user = text(tokenClaims(token)?.sub)
    if (user) return `xai:${user}`
  }
  return undefined
}

const text = (value: unknown) => (typeof value === "string" && value.trim() ? value.trim().toLowerCase() : undefined)
