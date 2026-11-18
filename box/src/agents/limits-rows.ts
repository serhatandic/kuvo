// Your plans' limits across your computers, one row per account (PROTOCOL.md, "Plan limits",
// `rows`): what this box read, and what each of your other computers' boxes last told it they read
// (`limits-peers.ts`), as one list, the same on every box. Worked out here, so no client merges.

import type { AgentKind } from "./accounts"
import type { AccountLimits, LimitWindow } from "./limits"
import { levelOf, limitDisplay, windowTitle, type LimitDisplay } from "./usage/limit-display"

export { levelOf, windowTitle } from "./usage/limit-display"

/** One computer's accounts, as this box has them: its own, or what a peer last told. */
export interface LimitsBox {
  /** Its key (this box's own, or "" where it isn't known yet). */
  key: string
  name: string
  /** This box. */
  here: boolean
  /** A peer not heard from for a while: what it said is as of then. */
  away: boolean
  /** When a peer would count as away without another update. */
  awayAt?: number
  accounts: AccountLimits[]
}

/** A computer an account is signed in on, as a row names it. */
export interface LimitRowBox {
  key: string
  name: string
  /** What it's called there (`claude-code`, `openai`…), the coding agent's, and the name you gave it there. */
  provider: string
  agent?: AgentKind
  account?: string
  label?: string
  /** When its maker last answered it there. */
  checked?: number
  here?: true
  away?: true
}

/** How close a share is to its limit, the same word in every client: `near` from 70% used, `over` from 90%. */
export type LimitLevel = "calm" | "near" | "over"
/** Which of an account's two rings a window is: its short limit (a 5-hour session) or its long one (the week, the month). */
export type LimitSpan = "short" | "long"

/**
 * A window as a row states it: its name (`title`), how close it is, and which ring it is, where it's
 * one. One whose `resetsAt` had passed when the box answered has started over (`expired`): not read
 * since, so `used` 0, `calm`, and neither its `resetsAt` nor its `amount`.
 */
export interface RowWindow extends LimitWindow {
  title: string
  level: LimitLevel
  span?: LimitSpan
  expired?: true
}

/** One of an account's rings: its window (by `id`), how much of it is gone, when it starts over, and how close that is. */
export interface LimitRing {
  window: string
  used: number
  resetsAt?: number
  level: LimitLevel
}

/** One account across your computers: its newest reading, and every computer it's signed in on. */
export interface LimitRow extends AccountLimits {
  /** Its name, the same in every client: its account's (`accountTitle`), told apart from another row of that name (`named`). */
  title: string
  windows: RowWindow[]
  /** Its two rings, the short limit and the long one; one alone where its plan has one window; none where it has none. */
  rings: { short?: LimitRing; long?: LimitRing }
  /** How close the account as a whole is to a limit, for one mark of it (a gauge): the higher of its
   * rings'; with no window, its balance's or a capped extra's (used of the most it may come to).
   * Left out where nothing says a limit (a balance with no cap, spend alone): not calm, unknown. */
  level?: LimitLevel
  /** The ring a client with room for one bar draws: the fuller of the two (the short one where they're as full), a ring that started over counting as empty. None with no rings. */
  bar?: LimitSpan
  /** The same for the account on every box: `<service>#<identity>`, or `<box key>/<provider>` for one that can't say who it is. */
  id: string
  boxes: LimitRowBox[]
  /** Display facts for the Usage page, added to HTTP answers. */
  usage?: LimitDisplay
  /** Every computer it's on is away: its numbers are as of their `read`, not live. */
  away?: true
  /**
   * Its plan isn't active now, as its maker says on every computer that read it (`problem.kind`
   * `ended`): no window, no ring. A client lists it where it lists accounts, saying so, and draws
   * no ring or bar for it.
   */
  ended?: true
}

/**
 * One row per account across the boxes, in their order (this box first): rows of one `service` with
 * one `identity` are one account, from any box; one without an identity is its box's own. `now`, the
 * box's clock: a window whose reset has passed by then has started over.
 */
export function limitRows(boxes: LimitsBox[], now: number, presentUsage = false): LimitRow[] {
  const groups: { id: string; members: { box: LimitsBox; account: AccountLimits }[] }[] = []
  const found = new Map<string, number>()
  for (const box of boxes) {
    for (const account of box.accounts) {
      const id = account.identity ? `${account.service}#${account.identity}` : `${box.key}/${account.provider}`
      const index = found.get(id)
      if (index !== undefined) groups[index]!.members.push({ box, account })
      else {
        found.set(id, groups.length)
        groups.push({ id, members: [{ box, account }] })
      }
    }
  }
  const rows = groups.map(({ id, members }) => {
    const account = merged(members)
    const row: LimitRow = {
      ...ringed(account, now),
      ...(presentUsage ? { usage: limitDisplay(account, members, now, accountTitle(account)) } : {}),
      title: "",
      id,
      boxes: members.map(({ box, account }) => ({
        key: box.key,
        name: box.name,
        provider: account.provider,
        ...(account.agent ? { agent: account.agent } : {}),
        ...(account.account ? { account: account.account } : {}),
        ...(account.label ? { label: account.label } : {}),
        ...(account.checked ? { checked: account.checked } : {}),
        ...(box.here ? { here: true as const } : {}),
        ...(box.away ? { away: true as const } : {}),
      })),
    }
    if (members.every((member) => member.box.away)) row.away = true
    // (Ended as the newest reading says: what another box kept from before is no plan's now.)
    if (row.problem?.kind === "ended") {
      Object.assign(row, { ended: true, windows: [], rings: {} })
      delete row.level
      delete row.bar
    }
    withResetsOn(row, members)
    return row
  })
  named(rows)
  return rows
}

/** The coding agents by their own names, and the engine's sign-ins by the plan they're for. */
const AGENT_NAMES: Record<string, string> = { claude: "Claude", codex: "Codex", grok: "Grok", cursor: "Cursor", antigravity: "Antigravity" }
const SERVICE_NAMES: Record<string, string> = { codex: "ChatGPT", grok: "SuperGrok", copilot: "Copilot", openrouter: "OpenRouter", zai: "Z.ai", ollama: "Ollama", opencode: "OpenCode Go" }
const capitalized = (word: string) => word.charAt(0).toUpperCase() + word.slice(1)

/** An account's name (PROTOCOL.md, "Plan limits", `title`): its coding agent's, else its plan's ("ChatGPT"), with what you called it ("Claude · Work"). */
export function accountTitle(account: { agent?: string; service: string; label?: string }): string {
  const base = account.agent ? (AGENT_NAMES[account.agent] ?? capitalized(account.agent)) : (SERVICE_NAMES[account.service] ?? capitalized(account.service))
  const label = account.label?.trim()
  return label ? `${base} · ${label}` : base
}

/**
 * Each row's name: its account's, unless two would have one name. Then a row whose sign-in on this
 * box is the engine's alone takes the service's name ("ChatGPT"); a row not on this box adds the
 * names of the computers it's on ("Claude · hetzner, lokum"); rows still alike, each on this box,
 * add their other computers' names.
 */
function named(rows: LimitRow[]) {
  const names = rows.map((row) => accountTitle(row))
  const here = (index: number) => rows[index]!.boxes.filter((box) => box.here)
  const elsewhere = (index: number) => [...new Set(rows[index]!.boxes.filter((box) => !box.here).map((box) => box.name))].join(", ")
  const alike = () => {
    const counts = new Map<string, number>()
    for (const name of names) counts.set(name, (counts.get(name) ?? 0) + 1)
    return new Set([...counts].filter(([, count]) => count > 1).map(([name]) => name))
  }
  let clash = alike()
  names.forEach((name, index) => {
    const mine = here(index)
    if (!clash.has(name) || !mine.length || mine.some((box) => box.agent)) return
    names[index] = accountTitle({ service: rows[index]!.service, label: mine[0]!.label })
  })
  for (const onHere of [false, true]) {
    clash = alike()
    names.forEach((name, index) => {
      if (!clash.has(name) || (here(index).length > 0) !== onHere) return
      const boxes = elsewhere(index)
      if (boxes) names[index] = `${name} · ${boxes}`
    })
  }
  rows.forEach((row, index) => (row.title = names[index]!))
}

const DAY = 24 * 60

/**
 * Which of an account's windows are its rings (PROTOCOL.md, "Plan limits", `rings`): among the
 * whole plan's windows (not one model's; its plainly timed ones before those named for what they
 * count, "Premium requests"), the shortest under a day is `short`, and the longest of a day or
 * more `long`, the first listed where two are as long. A plan that states no length has its first
 * window as `long`. Where every window is named for what it counts (Antigravity's pools, Copilot's
 * counts), the rings are of one pool, the first listed's (those whose `id` differs only in its
 * last part), so one pool's session is never paired with another's week. Where every window is a
 * model's, the first model's stand for the plan.
 */
export function spansOf(windows: LimitWindow[]): Map<LimitWindow, LimitSpan> {
  const spans = new Map<LimitWindow, LimitSpan>()
  if (!windows.length) return spans
  const plan = windows.filter((window) => !window.model)
  const first = windows[0]!.model?.toLowerCase()
  const whole = plan.length ? plan : windows.filter((window) => window.model?.toLowerCase() === first)
  const plain = whole.filter((window) => !window.name)
  // (Where each is named for what it counts, only those of one pool pair up: the first listed's,
  // by its id but its last part, `gemini-5h` with `gemini-weekly`, never with `3p-weekly`.)
  const stem = (window: LimitWindow) => window.id.replace(/[-_][^-_]*$/, "")
  const pool = plain.length ? plain : whole.filter((window) => stem(window) === stem(whole[0]!))
  let short: LimitWindow | undefined
  let long: LimitWindow | undefined
  for (const window of pool) {
    if (!window.minutes) continue
    if (window.minutes < DAY) {
      if (!short || window.minutes < short.minutes!) short = window
    } else if (!long || window.minutes > long.minutes!) long = window
  }
  if (!short && !long) long = pool[0]
  if (short) spans.set(short, "short")
  if (long) spans.set(long, "long")
  return spans
}

const LEVELS: LimitLevel[] = ["calm", "near", "over"]
const highest = (levels: LimitLevel[]): LimitLevel | undefined => levels.reduce<LimitLevel | undefined>((most, level) => (most === undefined || LEVELS.indexOf(level) > LEVELS.indexOf(most) ? level : most), undefined)

/**
 * How close an account as a whole is to a limit (PROTOCOL.md, "Plan limits", a row's `level`): the
 * higher of its rings' levels; with no ring, what it has with a most it may come to, by the same
 * shares (`levelOf`): its balance (what's gone of `limit`) and each `capped` extra (`amount` of
 * `limit`). None where nothing says a limit.
 */
export function accountLevel(account: Pick<AccountLimits, "balance" | "extras">, rings: LimitRow["rings"]): LimitLevel | undefined {
  const ringed = highest(Object.values(rings).map((ring) => ring!.level))
  if (ringed) return ringed
  const shares: number[] = []
  const balance = account.balance
  if (balance && typeof balance.limit === "number" && balance.limit > 0) shares.push(1 - balance.left / balance.limit)
  for (const extra of account.extras ?? []) if (extra.kind === "capped" && typeof extra.amount === "number" && typeof extra.limit === "number" && extra.limit > 0) shares.push(extra.amount / extra.limit)
  return highest(shares.map(levelOf))
}

/**
 * A window as a client is shown it at `now`: named, and one whose reset has passed started over
 * (`expired`: nothing used that's known, no reset or amount of the period that ended).
 */
export function shownWindow(window: LimitWindow, now: number): LimitWindow & { title: string; expired?: true } {
  const title = windowTitle(window)
  if (window.resetsAt === undefined || window.resetsAt > now) return { ...window, title }
  const { resetsAt: _, amount: __, ...rest } = window
  return { ...rest, used: 0, title, expired: true }
}

/** An account as a client is shown it at `now` (the `accounts` list): named, each window as `shownWindow`. */
export function shownAccount(account: AccountLimits, now: number): AccountLimits & { title: string } {
  return { ...account, title: accountTitle(account), windows: account.windows.map((window) => shownWindow(window, now)) }
}

/** An account as a row states it at `now`: each window named, with its level and which ring it is (one that started over empty, in its place), the rings themselves, the one a single bar shows, and its level as a whole. */
function ringed(account: AccountLimits, now: number): AccountLimits & Pick<LimitRow, "windows" | "rings" | "level" | "bar"> {
  const spans = spansOf(account.windows)
  const rings: LimitRow["rings"] = {}
  const windows = account.windows.map((read) => {
    const window = shownWindow(read, now)
    const level = levelOf(window.used)
    const span = spans.get(read)
    if (span) rings[span] = { window: window.id, used: window.used, ...(window.resetsAt ? { resetsAt: window.resetsAt } : {}), level }
    return { ...window, level, ...(span ? { span } : {}) }
  })
  const level = accountLevel(account, rings)
  const bar: LimitSpan | undefined = rings.short && (!rings.long || rings.short.used >= rings.long.used) ? "short" : rings.long ? "long" : undefined
  return { ...account, windows, rings, ...(level ? { level } : {}), ...(bar ? { bar } : {}) }
}

/**
 * The computer that would use one of a row's resets: the first of its boxes (this box's first)
 * whose own sign-in can (its `resets` extra says `usable`), one that's heard from before one that's
 * away. None where none
 * can: the row's resets are only counted.
 */
function withResetsOn(row: LimitRow, members: { box: LimitsBox; account: AccountLimits }[]) {
  const index = row.extras?.findIndex((extra) => extra.kind === "resets") ?? -1
  if (index < 0) return
  const can = (member: { box: LimitsBox; account: AccountLimits }) => member.account.extras?.some((extra) => extra.kind === "resets" && extra.usable === true)
  const holder = members.find((member) => can(member) && !member.box.away) ?? members.find(can)
  const extras = [...row.extras!]
  const { on: _, ...resets } = extras[index]!
  // (Whether agents may use them without asking is the holder's account's, kept on its box.)
  const agents = holder?.account.extras?.some((extra) => extra.kind === "resets" && extra.agents === true)
  delete resets.agents
  extras[index] = holder ? { ...resets, usable: true, ...(agents ? { agents: true } : {}), on: { key: holder.box.key, name: holder.box.name, ...(holder.box.here ? { here: true as const } : {}) } } : { ...resets, usable: false }
  row.extras = extras
}

/** When an account was last heard of: its maker's last answer, else its newest window. */
const newestOf = (account: AccountLimits) => account.checked ?? Math.max(0, ...account.windows.map((window) => window.read))

/**
 * One account as its boxes state it, together: named as a coding agent's where one is among them
 * (else the engine's sign-in's), with the first name you gave it; each window as read last; the
 * plan, credits, what it has beyond its windows (`extras`) and when it was checked as the newest says; a problem only when none of them could
 * read it.
 */
function merged(boxes: { box: LimitsBox; account: AccountLimits }[]): AccountLimits {
  const members = boxes.map((member) => member.account)
  const base = members.find((member) => member.agent) ?? members[0]!
  if (members.length === 1) return base
  const account: AccountLimits = { ...base }
  const label = members.map((member) => member.label?.trim()).find(Boolean)
  if (label) account.label = label
  else delete account.label
  const windows: LimitWindow[] = []
  for (const member of members) {
    for (const window of member.windows) {
      const index = windows.findIndex((each) => limitOf(each) === limitOf(window))
      if (index < 0) windows.push(window)
      else if (window.read > windows[index]!.read) windows[index] = window
    }
  }
  account.windows = windows
  const newest = [...members].sort((a, b) => newestOf(b) - newestOf(a))
  const plan = newest.find((member) => member.plan)?.plan
  const balance = newest.find((member) => member.balance)?.balance
  // (A reading without them says there are none now: the newest that read without a problem;
  // else, while none could read lately, what was last read, before what a sign-in can't read.)
  const read = newest.find((member) => member.checked && !member.problem)
  const extras = read ? read.extras : (newest.find((member) => member.extras?.some((extra) => extra.kind !== "unreadable")) ?? newest.find((member) => member.extras?.length))?.extras
  const checked = Math.max(0, ...members.map((member) => member.checked ?? 0))
  const next = Math.min(...members.map((member) => member.next ?? Infinity))
  for (const key of ["plan", "balance", "extras", "checked", "next", "problem"] as const) delete account[key]
  if (plan) account.plan = plan
  if (balance) account.balance = balance
  if (extras?.length) account.extras = extras
  if (checked) account.checked = checked
  if (Number.isFinite(next)) account.next = next
  if (members.every((member) => member.problem)) account.problem = newest[0]!.problem
  return account
}

/**
 * Which limit a window is, the same from any box and either sign-in to an account (a coding agent's,
 * the engine's), which may name it differently: what it counts, and how long it is.
 */
function limitOf(window: LimitWindow): string {
  if (window.model === undefined && window.name === undefined && window.minutes === undefined) return "id:" + window.id
  return `${window.model?.toLowerCase() ?? ""}|${window.name ?? ""}|${window.minutes ?? ""}`
}
