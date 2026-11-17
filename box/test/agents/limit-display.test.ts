import { expect, test } from "bun:test"
import type { AccountLimits, LimitWindow } from "../../src/agents/limits"
import { limitRows, type LimitsBox } from "../../src/agents/limits-rows"

const minute = 60_000
const now = Date.UTC(2026, 10, 17, 12)

function window(id: string, minutes: number, changes: Partial<LimitWindow> = {}): LimitWindow {
  return { id, minutes, used: 0.425, read: now - minute, resetsAt: now + 10 * minute, source: "usage", ...changes }
}

function computer(name: string, windows: LimitWindow[], changes: Partial<AccountLimits> = {}, away = false): LimitsBox {
  return {
    key: name, name, here: name === "Studio", away,
    accounts: [{ service: "claude", provider: "claude-code", identity: "0123456789abcdef", plan: "Max", windows, ...changes }],
  }
}

const rowOn = (boxes: LimitsBox[], at = now) => limitRows(boxes, at, true)[0]!
const displayOn = (boxes: LimitsBox[], at = now) => rowOn(boxes, at).usage!

test("Usage names windows, supplies their used percentages and shares the read line when possible", () => {
  const boxes = [computer("Studio", [
    window("five", 300),
    window("weekly", 10_080, { used: 0.71 }),
    window("sonnet", 10_080, { used: 0.95, model: "Sonnet" }),
    window("monthly", 43_200, { resetsAt: undefined }),
    window("premium", 43_200, { name: "Premium requests" }),
  ])]
  const usage = displayOn(boxes)
  expect(usage.detail).toStartWith("Max · Studio · Read ")
  expect(usage.meters.map(meter => meter.title)).toEqual(["5-hour", "Weekly", "Sonnet · Weekly", "Monthly", "Premium requests"])
  expect(usage.meters.map(({ value, level }) => [value, level])).toEqual([
    ["43% used", "calm"], ["71% used", "near"], ["95% used", "over"], ["43% used", "calm"], ["43% used", "calm"],
  ])
  expect(usage.meters[0]).toMatchObject({ share: 0.425, freshForMs: 10 * minute })
  expect(usage.meters.every(meter => meter.read === undefined)).toBe(true)
  expect(usage.meters[0]!.reset).toStartWith("Reset due ")
  expect(usage.meters[3]!.reset).toBe("Reset time unavailable")
})

test("Usage dates each merged meter by its own reading and names the computer that supplied it", () => {
  const usage = displayOn([
    computer("Studio", [window("five", 300, { read: now - 3 * minute }), window("weekly", 10_080)]),
    computer("Build", [window("primary", 300, { read: now, used: 0.83 }), window("secondary", 10_080, { read: now - minute * 2 })]),
  ])
  expect(usage.detail).toBe("Max")
  expect(usage.meters).toHaveLength(2)
  expect(usage.meters[0]!.value).toBe("83% used")
  expect(usage.meters[0]!.read).toStartWith("Build · Read ")
  expect(usage.meters[1]!.read).toStartWith("Studio · Read ")
})

test("Usage marks an overdue reset and leaves the historical value intact independently of the sidebar", () => {
  const boxes = [computer("Studio", [window("five", 300, { resetsAt: now })])]
  expect(displayOn(boxes, now - 1).meters[0]!.freshForMs).toBe(1)
  expect(displayOn(boxes).meters[0]).toMatchObject({ value: "43% used", old: "Old reading", freshForMs: 0 })
  const row = rowOn(boxes)
  expect(row.windows[0]).toMatchObject({ used: 0, expired: true, title: "Session" })
  expect(row.rings.short!.used).toBe(0)
})

test("Usage ages readings by their own time and marks away or failed sources old immediately", () => {
  for (const minutes of [29, 30, 31]) {
    const usage = displayOn([computer("Studio", [window("five", 300, { read: now - minutes * minute })], { checked: now })])
    expect(usage.meters[0]!.freshForMs).toBe(minutes === 29 ? minute : 0)
  }
  const failed = computer("Build", [window("five", 300)], { problem: { kind: "failed", message: "PRIVATE_ERROR" } })
  expect(displayOn([failed]).meters[0]!.freshForMs).toBe(0)
  expect(displayOn([computer("Build", [window("five", 300)], {}, true)]).meters[0]!.freshForMs).toBe(0)
  const peer = { ...computer("Build", [window("five", 300)]), awayAt: now + minute }
  expect(displayOn([peer]).meters[0]!.freshForMs).toBe(minute)
})

test("Usage gives plain next steps instead of exposing the provider’s error", () => {
  for (const kind of ["failed", "expired", "unreadable", "rate-limited"] as const) {
    const usage = displayOn([computer("Build", [], { problem: { kind, message: "PRIVATE_ERROR" } })])
    expect(usage.detail).toBe("Max · Build")
    expect(usage.status?.warning).toBe(true)
    expect(usage.status?.words).toMatch(/try again|Accounts/)
    expect(JSON.stringify(usage)).not.toContain("PRIVATE_ERROR")
    expect(usage.meters).toEqual([])
  }
  expect(displayOn([computer("Studio", [])]).status?.words).toBe("Waiting for the first reading.")
  expect(displayOn([computer("Studio", [], { checked: now })]).status?.words).toBe("No limits reported.")
  expect(displayOn([computer("Studio", [window("five", 300)], { problem: { kind: "ended", message: "PRIVATE_ERROR" } })]))
    .toMatchObject({ status: { words: "Plan ended.", warning: false }, meters: [] })
  expect(limitRows([], now, true)).toEqual([])
})
