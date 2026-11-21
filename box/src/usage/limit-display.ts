// Display facts for the Usage page. The sidebar still reads the existing windows and rings.
import { hostname } from "node:os"
import type { AccountLimits, LimitsProblemKind, LimitWindow } from "./limits"
import type { LimitsBox, LimitLevel } from "./limits-rows"

// Readers run every 10–15 minutes in the ordinary case. Give them one missed round.
const FRESH_FOR = 30 * 60_000
const DAY = 24 * 60
const MINUTE_MS = 60_000
const HOUR_MS = 60 * MINUTE_MS
const DAY_MS = 24 * HOUR_MS

export interface UsageMeter {
  title: string
  share: number
  value: string
  level: LimitLevel
  readAt: number
  resetsAt?: number
  resetLabels: { at: number; words: string }[]
  freshForMs: number
}

export interface LimitDisplay {
  title: string
  updatedAt?: number
  computers: { name: string; here: boolean }[]
  status?: { words: string; warning: boolean }
  meters: UsageMeter[]
}

type Member = { box: LimitsBox; account: AccountLimits }

export function limitDisplay(account: AccountLimits, members: Member[], now: number, title: string): LimitDisplay {
  const problem = account.problem?.kind
  const computers = new Set<LimitsBox>()
  const meter = (source: Member, title: string, used: number, readAt: number, resetsAt?: number): UsageMeter => {
    computers.add(source.box)
    const share = Math.min(1, Math.max(0, used))
    const freshUntil = Math.min(readAt + FRESH_FOR, resetsAt ?? Infinity, source.box.awayAt ?? Infinity)
    const freshForMs = source.box.away || source.account.problem ? 0 : Math.max(0, Math.floor(freshUntil - now))
    return {
      title, share, value: `${Math.round(share * 100)}%`, level: share > 0.7 ? levelOf(share) : "calm",
      readAt, resetsAt, freshForMs, resetLabels: resetLabels(resetsAt, now, now + freshForMs),
    }
  }
  const meters = problem === "ended" ? [] : account.windows.map(window => {
    // Merging preserves the reading object, so its source is still identifiable.
    const source = members.find(member => member.account.windows.includes(window))!
    return meter(source, usageTitle(window), window.used, window.read, window.resetsAt)
  })
  if (problem !== "ended") {
    const balance = account.balance
    if (balance?.limit && balance.limit > 0) {
      const source = members.find(member => member.account.balance === balance)
      if (source) meters.push(meter(source, "Credits", 1 - balance.left / balance.limit, source.account.checked ?? 0))
    }
    for (const extra of account.extras ?? []) {
      if (extra.kind !== "capped" || extra.amount === undefined || !(extra.limit && extra.limit > 0)) continue
      const source = members.find(member => member.account.extras?.includes(extra))
      if (source) meters.push(meter(source, extra.name, extra.amount / extra.limit, source.account.checked ?? 0, extra.resetsAt))
    }
  }
  if (!meters.length) for (const member of members) computers.add(member.box)
  // (Contradicted: its maker's own numbers aren't known to be right, and its message says why.)
  const status = account.problem?.unknown ? { words: account.problem.message, warning: true } : statusOf(problem, meters.length > 0, account.checked !== undefined)
  return {
    title,
    updatedAt: Math.max(account.checked ?? 0, ...meters.map(meter => meter.readAt)) || undefined,
    computers: [...computers].map(box => ({ name: box.name || (box.here ? hostname() : "Another computer"), here: box.here })),
    ...(status ? { status } : {}),
    meters,
  }
}

/** Round up so a countdown never promises an earlier reset. Past a day use the viewer's local date. */
export function resetLabels(at: number | undefined, now: number, until: number): UsageMeter["resetLabels"] {
  if (at === undefined) return [{ at: now, words: "Reset time unavailable" }]
  if (at <= now) return [{ at: now, words: "Reset due" }]
  const labels: UsageMeter["resetLabels"] = []
  // A missing current label tells the client to show the reset's local weekday and time.
  let start = Math.max(now, at - DAY_MS + 1)
  while (start <= until && start < at) {
    const left = at - start
    const unit = Math.ceil(left / MINUTE_MS) >= 60 ? HOUR_MS : MINUTE_MS
    const count = Math.ceil(left / unit)
    labels.push({ at: start, words: `resets in ${count} ${unit === HOUR_MS ? "h" : "min"}` })
    start = at - (count - 1) * unit
    if (unit === HOUR_MS) start = Math.min(start, at - HOUR_MS + MINUTE_MS)
  }
  return labels
}

function usageTitle(window: LimitWindow): string {
  if (window.name) return window.name
  const span = window.minutes === 300 ? "5-hour" : windowTitle({ minutes: window.minutes })
  return window.model ? (span === "Limit" ? window.model : `${window.model} · ${span}`) : span
}

function statusOf(problem: LimitsProblemKind | undefined, hasMeters: boolean, checked: boolean): LimitDisplay["status"] {
  switch (problem) {
    case "ended": return { words: "Plan ended.", warning: false }
    case "expired": return { words: "Sign-in expired. Sign in again under Accounts.", warning: true }
    case "unreadable": return { words: "Can’t read limits. Check the sign-in under Accounts.", warning: true }
    case "rate-limited": return { words: "The provider paused readings. Kuvo will try again.", warning: true }
    case "failed": return { words: "Couldn’t read limits. Check the connection, then try again.", warning: true }
    default: return hasMeters ? undefined : { words: checked ? "No limits reported." : "Waiting for the first reading.", warning: false }
  }
}

/** A window's name: its maker's for what it counts, its model's, else how long it is ("Session", "Weekly", "Monthly", "3-day", "8-hour"); "Limit" where it says none. */
export function windowTitle(window: Pick<LimitWindow, "name" | "model" | "minutes">): string {
  if (window.name) return window.name
  if (window.model) return window.model
  const minutes = window.minutes
  if (minutes === 300) return "Session"
  if (minutes === 10_080) return "Weekly"
  if (minutes !== undefined && minutes >= 40_000 && minutes <= 45_000) return "Monthly"
  if (minutes !== undefined && minutes >= DAY) return `${Math.floor(minutes / DAY)}-day`
  if (minutes !== undefined && minutes >= 60) return `${Math.floor(minutes / 60)}-hour`
  return "Limit"
}

export const levelOf = (used: number): LimitLevel => (used >= 0.9 ? "over" : used >= 0.7 ? "near" : "calm")
