// Display facts for the Usage page. The sidebar still reads the existing windows and rings.
import { hostname } from "node:os"
import type { AccountLimits, LimitsProblemKind, LimitWindow } from "../limits"
import type { LimitsBox, LimitLevel } from "../limits-rows"

// Readers run every 10–15 minutes in the ordinary case. Give them one missed round.
const FRESH_FOR = 30 * 60_000
const DAY = 24 * 60

export interface UsageMeter {
  title: string
  share: number
  value: string
  level: LimitLevel
  readAt: number
  resetsAt?: number
  freshForMs: number
}

export interface LimitDisplay {
  title: string
  computers: { name: string; here: boolean }[]
  status?: { words: string; warning: boolean }
  meters: UsageMeter[]
}

type Member = { box: LimitsBox; account: AccountLimits }

export function limitDisplay(account: AccountLimits, members: Member[], now: number, title: string): LimitDisplay {
  const problem = account.problem?.kind
  const computers = new Set<LimitsBox>()
  const meters = problem === "ended" ? [] : account.windows.map(window => {
    // The merge keeps the winning window object, including when different windows came from
    // different computers. Use that reading's source, rather than the last account check.
    const source = members.find(member => member.account.windows.includes(window))!
    computers.add(source.box)
    const share = Math.min(1, Math.max(0, window.used))
    const freshUntil = Math.min(window.read + FRESH_FOR, window.resetsAt ?? Infinity, source.box.awayAt ?? Infinity)
    return {
      title: usageTitle(window),
      share,
      value: `${Math.round(share * 100)}%`,
      level: share > 0.7 ? levelOf(share) : "calm",
      readAt: window.read,
      resetsAt: window.resetsAt,
      freshForMs: source.box.away || source.account.problem ? 0 : Math.max(0, Math.floor(freshUntil - now)),
    }
  })
  if (!meters.length) for (const member of members) computers.add(member.box)
  const status = statusOf(problem, meters.length > 0, account.checked !== undefined)
  return {
    title,
    computers: [...computers].map(box => ({ name: box.name || (box.here ? hostname() : "Another computer"), here: box.here })),
    ...(status ? { status } : {}),
    meters,
  }
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
