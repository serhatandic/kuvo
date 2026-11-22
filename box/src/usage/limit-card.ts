// What a client shows of an account on its card, and how the rail lays the accounts out
// (PROTOCOL.md, "Plan limits", `card` and `rail`): worked out here, in words, so every client draws
// the same and none works out a pace, a state or a sentence of its own. A time in the words is a
// moment (`at`) the client writes in its own time zone (`as`: a time of day, or a date).

import type { LimitWindow } from "./limits"
import { statusOf, usageTitle } from "./limit-display"
import { baseTitle, limitOf, type LimitRow } from "./limits-rows"
import type { Pace, Prediction } from "./pace"

/** A piece of a line: its words, then the moment `at` written as a time of day or a date, where there's one. */
export interface CardWord {
  text: string
  at?: number
  as?: "time" | "date"
  tone?: "warning" | "danger" | "muted" | "faint"
  strong?: true
}

/** How a window, an account or a mark of one stands: fine, on pace to run out, used up, or not known. */
export type CardTone = "calm" | "warning" | "danger" | "unknown"

/** One window on a card: its name, what's left of it (0 to 1, for its bar), how it stands, and the rest of its line. */
export interface CardLine {
  window: string
  title: string
  left: number
  tone: CardTone
  words: CardWord[]
}

/** The account's resets, where one can be used now: how many and when the next expires, and the confirm before one's used. */
export interface CardResets {
  words: CardWord[]
  action: string
  confirm: { title: CardWord[]; message: string; button: string; cancel: string }
}

export interface LimitCard {
  /** Its name on the card: the agent's, with which account where you have more than one ("Claude · Max"). */
  title: string
  /** The account alone, among others of its agent ("Lab 12", "Max"). */
  name: string
  /** A dot on its ring: on pace to run out before a window starts over (`warning`), or used up (`danger`). */
  dot?: "warning" | "danger"
  /** How it stands as a whole, for a mark of it among others. */
  tone: CardTone
  /** What's left of it, at its tightest (0 to 1), for a mark of it among others; none where nothing's known. */
  left?: number
  /** Its windows: the shortest of the plan's first (5-hour, then Weekly), then what it counts, then each model's. */
  lines: CardLine[]
  /** Why there's nothing (or not everything) to show, in words. */
  note?: string
  /** Sessions ran on it lately, more lately than any other of its agent's accounts. */
  inUse?: true
  resets?: CardResets
}

/** The rail's rings, in order: one an account, or one for an agent's accounts where it has more than two. */
export interface RailItem {
  /** The agent (or service) its accounts are of. */
  key: string
  /** Its accounts (rows' `id`s): one, or, for an agent's many, each in the strip's order. */
  rows: string[]
  /** The row whose rings it draws: the account itself, or the agent's emptiest. */
  ring: string
  /** The account its card shows in full at first: the one in use, else the emptiest. */
  selected: string
  title: string
  dot?: "warning" | "danger"
  /** For an agent's many: how many there are, how many are fine ("17 of 20 fine"), and a mark of each, emptiest first. */
  count?: number
  summary?: string
  strip?: { row: string; name: string; left?: number; tone: CardTone; heading: CardWord[] }[]
}

const SEP: CardWord = { text: " · ", tone: "faint" }
const percent = (share: number) => `${Math.round(Math.max(0, Math.min(1, share)) * 100)}% left`
/** A window used lately in a turn is the account in use, for this long. */
const IN_USE_FOR = 30 * 60_000
/** An agent's accounts share one ring from this many. */
export const GROUP_FROM = 3

/** Where a window's readings are kept: the account and the limit, the same from any of your computers. */
export const paceKey = (row: { id: string }, window: Pick<LimitWindow, "id" | "model" | "name" | "minutes">) => `${row.id}|${limitOf(window)}`

/** The order windows are shown in: the plan's own by length (5-hour, then Weekly), then those named for what they count, then each model's. */
function rank(window: LimitWindow): [number, number] {
  const group = window.model ? 2 : window.name ? 1 : 0
  return [group, window.minutes ?? Number.MAX_SAFE_INTEGER]
}

function line(window: LimitRow["windows"][number], prediction: Prediction | undefined): CardLine {
  const title = usageTitle(window)
  if (window.expired) return { window: window.id, title, left: 1, tone: "calm", words: [{ text: "Started over, not read since", tone: "muted" }] }
  const left = Math.max(0, Math.min(1, 1 - window.used))
  if (prediction?.state === "used-up") {
    const words: CardWord[] = window.resetsAt ? [{ text: "Back at ", at: window.resetsAt, as: "time", tone: "danger", strong: true }] : [{ text: "Used up", tone: "danger", strong: true }]
    return { window: window.id, title, left: 0, tone: "danger", words }
  }
  if (prediction?.state === "runs-out") {
    return {
      window: window.id, title, left, tone: "warning",
      words: [{ text: percent(left) }, SEP, { text: "runs out ~", at: prediction.at, as: "time", tone: "warning", strong: true }, { text: ", resets ", at: window.resetsAt!, as: "time", tone: "muted" }],
    }
  }
  if (prediction?.state === "lasts") return { window: window.id, title, left, tone: "calm", words: [{ text: percent(left), tone: "muted" }, SEP, { text: "lasts until reset", tone: "muted" }] }
  // (Too few readings to say: what's left, and when it starts over.)
  return {
    window: window.id, title, left, tone: window.level === "over" ? "warning" : "calm",
    words: [{ text: percent(left), tone: "muted" }, ...(window.resetsAt ? [SEP, { text: "resets ", at: window.resetsAt, as: "time" as const, tone: "muted" as const }] : [])],
  }
}

function resetsOf(row: LimitRow): CardResets | undefined {
  const resets = row.extras?.find((extra) => extra.kind === "resets")
  const count = Number(resets?.count)
  if (!resets || resets.usable !== true || !resets.on || !(count >= 1)) return undefined
  const next = resets.expiries?.[0]
  const words: CardWord[] = [{ text: count === 1 ? "1 reset left" : `${count} resets left`, tone: "muted" }]
  if (next) words.push(SEP, { text: count === 1 ? "expires " : "next expires ", at: next, as: "date", tone: "muted" })
  return {
    words,
    action: "Use a reset",
    confirm: {
      title: next ? [{ text: "Use the reset that expires ", at: next, as: "date" }, { text: "?" }] : [{ text: "Use a reset?" }],
      message: `${baseTitle(row)}’s usage limits start over now.`,
      button: "Use Reset",
      cancel: "Cancel",
    },
  }
}

/** The account's own name among its agent's others: what you called it, else what tells it apart, else its plan. */
function nameOf(row: LimitRow): string {
  const label = row.label?.trim()
  if (label) return label
  const base = baseTitle(row)
  const rest = row.title.startsWith(`${base} · `) ? row.title.slice(base.length + 3) : ""
  return rest || row.plan || row.title
}

/**
 * An account's card at `now`: each of its windows' lines, with what the readings say of each
 * (`pace`), and nothing of any other account. `siblings`: how many accounts of its agent are on the
 * rail, so one of several is named in its title.
 */
export function cardOf(row: LimitRow, pace: Pace | undefined, now: number, siblings: number): LimitCard {
  const windows = [...row.windows].sort((a, b) => {
    const [ga, ma] = rank(a)
    const [gb, mb] = rank(b)
    return ga - gb || ma - mb
  })
  const lines = row.ended ? [] : windows.map((window) => line(window, window.expired ? undefined : pace?.predict(paceKey(row, window), window, now) ?? (window.used >= 0.995 ? { state: "used-up" } : undefined)))
  const name = nameOf(row)
  const base = baseTitle(row)
  const title = siblings > 1 ? (row.title.includes(" · ") ? row.title : `${base} · ${name}`) : row.title
  const problem = row.problem
  const note = problem?.unknown
    ? problem.message
    : lines.length
      ? undefined
      : problem
        ? statusOf(problem.kind, false, row.checked !== undefined)?.words
        : row.checked === undefined ? "Fills in once a session runs on this account" : "No limits reported."
  const danger = lines.some((each) => each.tone === "danger")
  const warning = lines.some((each) => each.tone === "warning" && each.words.some((word) => word.tone === "warning"))
  const ringed = lines.filter((each) => row.windows.find((window) => window.id === each.window)?.span)
  const counted = ringed.length ? ringed : lines
  const left = counted.length ? Math.min(...counted.map((each) => each.left)) : undefined
  const turnRead = Math.max(0, ...row.windows.filter((window) => window.source === "turn").map((window) => window.read))
  const resets = resetsOf(row)
  return {
    title,
    name,
    ...(danger ? { dot: "danger" as const } : warning ? { dot: "warning" as const } : {}),
    tone: danger ? "danger" : warning ? "warning" : left === undefined ? "unknown" : "calm",
    ...(left !== undefined ? { left } : {}),
    lines,
    ...(note ? { note } : {}),
    ...(turnRead && now - turnRead <= IN_USE_FOR ? { inUse: true as const } : {}),
    ...(resets ? { resets } : {}),
  }
}

/** Notes each window's reading on its trail, for what `cardOf` says of it later. */
export function notePace(row: LimitRow, pace: Pace, now: number) {
  for (const window of row.windows) {
    if (window.expired) continue
    pace.note(paceKey(row, window), { read: window.read, used: window.used, resetsAt: window.resetsAt, minutes: window.minutes }, now)
  }
}

/** The rail's accounts: those signed in on this computer, with a plan that hasn't ended. */
export const railed = (row: LimitRow) => row.boxes.some((box) => box.here) && !row.ended
/** What groups an account with others on the rail: its coding agent, else its service. */
export const railKey = (row: Pick<LimitRow, "agent" | "service">) => row.agent ?? row.service

const WORST: Record<CardTone, number> = { danger: 0, warning: 1, calm: 2, unknown: 3 }

/**
 * The rail's rings, in the rows' order (PROTOCOL.md, "Plan limits", `rail`): an account a ring, up to
 * two of an agent; from three, one ring for the agent, drawn as its emptiest account, and its card
 * a strip of every account, emptiest first, with the one in use shown in full.
 */
export function railOf(rows: LimitRow[]): RailItem[] {
  const shown = rows.filter((row) => railed(row) && row.card)
  const groups = new Map<string, LimitRow[]>()
  for (const row of shown) groups.set(railKey(row), [...(groups.get(railKey(row)) ?? []), row])
  const items: RailItem[] = []
  const placed = new Set<string>()
  for (const row of shown) {
    const key = railKey(row)
    const members = groups.get(key)!
    if (members.length < GROUP_FROM) {
      items.push({ key, rows: [row.id], ring: row.id, selected: row.id, title: row.card!.title, ...(row.card!.dot ? { dot: row.card!.dot } : {}) })
      continue
    }
    if (placed.has(key)) continue
    placed.add(key)
    const strip = [...members].sort((a, b) => {
      const la = a.card!.left
      const lb = b.card!.left
      if (la === undefined || lb === undefined) return (la === undefined ? 1 : 0) - (lb === undefined ? 1 : 0)
      return la - lb || WORST[a.card!.tone] - WORST[b.card!.tone]
    })
    const inUse = [...members].filter((member) => member.card!.inUse).sort((a, b) => newestTurn(b) - newestTurn(a))[0]
    const selected = inUse ?? strip[0]!
    const ring = strip.find((member) => member.rings.short || member.rings.long) ?? strip[0]!
    const fine = members.filter((member) => member.card!.tone === "calm").length
    const dot = members.some((member) => member.card!.dot === "danger") ? "danger" : members.some((member) => member.card!.dot === "warning") ? "warning" : undefined
    items.push({
      key,
      rows: strip.map((member) => member.id),
      ring: ring.id,
      selected: selected.id,
      title: baseTitle(row),
      ...(dot ? { dot } : {}),
      count: members.length,
      summary: `${fine} of ${members.length} fine`,
      strip: strip.map((member) => ({
        row: member.id,
        name: member.card!.name,
        ...(member.card!.left !== undefined ? { left: member.card!.left } : {}),
        tone: member.card!.tone,
        heading: [{ text: member.card!.name, strong: true as const }, ...(member === inUse ? [SEP, { text: "in use", tone: "muted" as const }] : [])],
      })),
    })
  }
  return items
}

const newestTurn = (row: LimitRow) => Math.max(0, ...row.windows.filter((window) => window.source === "turn").map((window) => window.read))

