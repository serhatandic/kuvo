import "../safety"
// What the rail's cards say of your plans (`GET api/kuvo/limits`, each row's `card`, and `rail`):
// how fast each window is going, from the box's own readings over time, worded by the box; each card
// only its own account's; a reset offered only where one can be used; an agent with many accounts
// as one ring. The box's limits are real (`Limits`, its route, its file); each maker is a stand-in
// that answers a scripted reading, and the clock is the test's.

import { afterAll, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Limits, limitsRoutes, type LimitedAccount, type LimitExtra, type LimitReading } from "../../src/usage/limits"
import type { CardWord, LimitCard, RailItem } from "../../src/usage/limit-card"
import type { AgentKind } from "../../src/agents/accounts"

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const START = Date.UTC(2026, 10, 22, 12)

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-limit-card-test-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

/** A coding agent's account whose maker answers `reading()` each time it's asked. */
function account(kind: AgentKind, id: string, reading: () => LimitReading, label?: string): LimitedAccount {
  const provider = id === "default" ? `${kind}-test` : `${kind}-test@${id}`
  const service = kind === "claude" ? "claude" : kind === "codex" ? "codex" : "antigravity"
  return {
    provider,
    service,
    agent: { kind, account: id },
    ...(label ? { label } : {}),
    limits: { every: 5 * MINUTE, read: async () => reading(), identity: () => `test:${provider}` },
  }
}

/** The box's limits, its clock the test's. */
function box(accounts: LimitedAccount[], file?: string) {
  const clock = { now: START }
  const limits = new Limits(() => accounts, file, () => clock.now)
  return { limits, clock }
}

async function answer(limits: Limits): Promise<{ rows: { title: string; card: LimitCard }[]; rail: RailItem[] }> {
  return ((await (await limitsRoutes(limits, "GET", [])).json()) as any).data
}

/** A line's words as they read, each moment as minutes from the start ("{+70m}"). */
const read = (words: CardWord[]) => words.map((word) => word.text + (word.at !== undefined ? `{+${Math.round((word.at - START) / MINUTE)}m}` : "")).join("")

/** Reads every 10 minutes, the 5-hour window at each of `used` in turn (resetting 3 hours from the start), the week steady at 20%. */
async function readings(used: number[], file?: string) {
  let at = 0
  const reading = (): LimitReading => ({
    windows: [
      { id: "seven_day", used: 0.2, minutes: 10_080, resetsAt: START + 4 * 24 * HOUR },
      { id: "five_hour", used: used[Math.min(at, used.length - 1)]!, minutes: 300, resetsAt: START + 3 * HOUR },
    ],
  })
  const { limits, clock } = box([account("claude", "default", reading)], file)
  for (at = 0; at < used.length; at++) {
    clock.now = START + at * 10 * MINUTE
    await limits.tick()
  }
  return { limits, clock }
}

test("a 5-hour window used at a steady pace says when it runs out, ahead of its reset, and the ring gets a warning dot", async () => {
  // 10% every 10 minutes: at 60% after half an hour, gone 40 minutes later (+70m), before its reset (+180m).
  const { limits } = await readings([0.3, 0.4, 0.5, 0.6])
  const { rows, rail } = await answer(limits)
  const card = rows[0]!.card
  expect(card.lines.map((line) => line.title)).toEqual(["5-hour", "Weekly"])
  expect(read(card.lines[0]!.words)).toBe("40% left · runs out ~{+70m}, resets {+180m}")
  expect(card.lines[0]!.tone).toBe("warning")
  expect(card.dot).toBe("warning")
  expect(rail).toEqual([expect.objectContaining({ rows: [expect.any(String)], dot: "warning" })])
})

test("a window used slowly lasts until its reset, and says so with no dot", async () => {
  const { limits } = await readings([0.3, 0.31, 0.32, 0.33])
  const card = (await answer(limits)).rows[0]!.card
  expect(read(card.lines[0]!.words)).toBe("67% left · lasts until reset")
  expect(card.lines[0]!.tone).toBe("calm")
  expect(card.dot).toBeUndefined()
})

test("with too few readings nothing is predicted: what's left and when it resets, no more", async () => {
  // Two readings ten minutes apart, however fast, aren't enough to say.
  const { limits } = await readings([0.3, 0.6])
  const card = (await answer(limits)).rows[0]!.card
  expect(read(card.lines[0]!.words)).toBe("40% left · resets {+180m}")
  expect(card.dot).toBeUndefined()
})

test("what the readings said is kept through a restart of the box", async () => {
  const file = join(mkdtempSync(join(base, "kept-")), "limits.json")
  const { clock } = await readings([0.3, 0.4, 0.5, 0.6], file)
  const again = new Limits(() => [account("claude", "default", () => ({ windows: [] }))], file, () => clock.now)
  const card = (await answer(again)).rows[0]!.card
  expect(read(card.lines.find((line) => line.title === "5-hour")!.words)).toBe("40% left · runs out ~{+70m}, resets {+180m}")
})

test("each card shows only its own account, named among its agent's others", async () => {
  const max = account("claude", "default", () => ({ plan: "Max", windows: [{ id: "five_hour", used: 0.62, minutes: 300, resetsAt: START + 2 * HOUR }] }), "Max")
  const twenty = account("claude", "a1", () => ({ plan: "Pro", windows: [{ id: "five_hour", used: 1, minutes: 300, resetsAt: START + 8 * HOUR }] }), "$20")
  const { limits } = box([max, twenty])
  await limits.tick()
  const { rows, rail } = await answer(limits)
  const cards = rows.map((row) => row.card)
  expect(cards.map((card) => card.title)).toEqual(["Claude · Max", "Claude · $20"])
  expect(cards[0]!.lines.map((line) => read(line.words))).toEqual(["38% left · resets {+120m}"])
  expect(cards[1]!.lines.map((line) => read(line.words))).toEqual(["Back at {+480m}"])
  expect(cards.map((card) => card.dot)).toEqual([undefined, "danger"])
  // Two accounts of an agent: a ring each.
  expect(rail.map((item) => item.title)).toEqual(["Claude · Max", "Claude · $20"])
})

/** Codex, holding `count` reset credits expiring after `days`. */
const codex = (count: number, days: number[]) => account("codex", "default", () => ({
  windows: [{ id: "primary", used: 0.22, minutes: 300, resetsAt: START + 3 * HOUR }, { id: "secondary", used: 0.69, minutes: 10_080, resetsAt: START + 2 * 24 * HOUR }],
  extras: [{ id: "resets", name: "Rate Limit Resets", kind: "resets", count, expiries: days.map((day) => START + day * 24 * HOUR), usable: count > 0 } satisfies LimitExtra],
}))

test("a reset is offered only where one can be used, naming the soonest to expire", async () => {
  const { limits } = box([codex(2, [8, 15])])
  await limits.tick()
  const resets = (await answer(limits)).rows[0]!.card.resets!
  expect(read(resets.words)).toBe(`2 resets left · next expires {+${8 * 24 * 60}m}`)
  expect(resets.words.find((word) => word.at)!.as).toBe("date")
  expect(resets.action).toBe("Use a reset")
  expect(read(resets.confirm.title)).toBe(`Use the reset that expires {+${8 * 24 * 60}m}?`)
  expect(resets.confirm).toMatchObject({ message: "Codex’s usage limits start over now.", button: "Use Reset", cancel: "Cancel" })

  const none = box([codex(0, [])])
  await none.limits.tick()
  expect((await answer(none.limits)).rows[0]!.card.resets).toBeUndefined()
})

test("an agent's many accounts share one ring, its card a strip of them emptiest first, fine ones counted", async () => {
  const lefts = Array.from({ length: 20 }, (_, ix) => (ix * 5) / 100)
  const accounts = lefts.map((left, ix) => account("antigravity", ix === 0 ? "default" : `a${ix}`, () => ({
    windows: [{ id: "gemini-5h", name: "Gemini · 5-hour", used: 1 - left, minutes: 300, resetsAt: START + 3 * HOUR }],
  }), `Lab ${String(ix + 1).padStart(2, "0")}`))
  // (Read in reverse, so the strip's order is the box's, not the reading's.)
  const { limits } = box([...accounts].reverse())
  await limits.tick()
  const { rail, rows } = await answer(limits)
  expect(rail).toHaveLength(1)
  const item = rail[0]!
  expect(item).toMatchObject({ title: "Antigravity", count: 20, summary: "19 of 20 fine", dot: "danger" })
  expect(item.strip!.map((each) => each.name).slice(0, 3)).toEqual(["Lab 01", "Lab 02", "Lab 03"])
  expect(item.strip!.map((each) => each.tone).slice(0, 2)).toEqual(["danger", "calm"])
  // Nothing in use: the emptiest is shown in full first.
  expect(item.selected).toBe(item.rows[0])
  expect(rows.find((row) => row.card.name === "Lab 01")!.card.title).toBe("Antigravity · Lab 01")
})
