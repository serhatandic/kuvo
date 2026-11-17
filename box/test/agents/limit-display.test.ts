import { expect, test } from "bun:test"
import { Limits, LimitsProblem, limitsRoutes, type AccountLimits, type LimitWindow } from "../../src/agents/limits"
import { resetLabels } from "../../src/agents/usage/limit-display"
import { limitRows, type LimitRow, type LimitsBox } from "../../src/agents/limits-rows"

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

test("Usage names windows, supplies their used percentages and lists a shared source once", () => {
  const boxes = [computer("Studio", [
    window("five", 300),
    window("weekly", 10_080, { used: 0.71 }),
    window("sonnet", 10_080, { used: 0.95, model: "Sonnet" }),
    window("monthly", 43_200, { used: 0.70, resetsAt: undefined }),
    window("premium", 43_200, { name: "Premium requests" }),
  ])]
  const usage = displayOn(boxes)
  expect(usage.computers).toEqual([{ name: "Studio", here: true }])
  expect(usage.meters.map(meter => meter.title)).toEqual(["5-hour", "Weekly", "Sonnet · Weekly", "Monthly", "Premium requests"])
  expect(usage.meters.map(({ value, level }) => [value, level])).toEqual([
    ["43%", "calm"], ["71%", "near"], ["95%", "over"], ["70%", "calm"], ["43%", "calm"],
  ])
  expect(usage.meters[0]).toMatchObject({ share: 0.425, freshForMs: 10 * minute })
  expect(usage.meters.every(meter => meter.readAt === now - minute)).toBe(true)
  expect(usage.meters[0]!.resetsAt).toBe(now + 10 * minute)
  expect(usage.meters[3]!.resetsAt).toBeUndefined()
})

test("Usage dates each merged meter by its own reading and names the computer that supplied it", () => {
  const usage = displayOn([
    computer("Studio", [window("five", 300, { read: now - 3 * minute }), window("weekly", 10_080)]),
    computer("Build", [window("primary", 300, { read: now, used: 0.83 }), window("secondary", 10_080, { read: now - minute * 2 })]),
  ])
  expect(usage.computers.map(computer => computer.name)).toEqual(["Build", "Studio"])
  expect(usage.meters).toHaveLength(2)
  expect(usage.meters[0]!.value).toBe("83%")
  expect(usage.meters[0]!.readAt).toBe(now)
  expect(usage.meters[1]!.readAt).toBe(now - minute)
})

test("Usage marks an overdue reset and leaves the historical value intact independently of the sidebar", () => {
  const boxes = [computer("Studio", [window("five", 300, { resetsAt: now })])]
  expect(displayOn(boxes, now - 1).meters[0]!.freshForMs).toBe(1)
  expect(displayOn(boxes).meters[0]).toMatchObject({ value: "43%", freshForMs: 0 })
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
    expect(usage.computers).toEqual([{ name: "Build", here: false }])
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

test("Usage draws capped credit balances and extras even without normal windows", () => {
  const account = {
    service: "openrouter", provider: "openrouter", checked: now - minute,
    balance: { left: 20, limit: 100, unit: "usd" },
  } satisfies Partial<AccountLimits>
  const credits = displayOn([computer("Studio", [], account)])
  expect(credits.status).toBeUndefined()
  expect(credits.meters).toMatchObject([{ title: "Credits", value: "80%" }])
  const usage = displayOn([computer("Studio", [], {
    ...account,
    extras: [{ id: "monthly", name: "Monthly spend", kind: "capped", amount: 45, limit: 50, unit: "usd", resetsAt: now + 60 * minute }],
  })])
  expect(usage.status).toBeUndefined()
  expect(usage.updatedAt).toBe(now - minute)
  expect(usage.meters).toMatchObject([
    { title: "Credits", value: "80%", level: "near", readAt: now - minute },
    { title: "Monthly spend", value: "90%", level: "over", resetsAt: now + 60 * minute },
  ])
})

test("Reset labels round up across hour and day boundaries, by elapsed time across DST", () => {
  const hour = 60 * minute
  const day = 24 * hour
  const label = (until: number) => resetLabels(now + until, now, now)[0]?.words
  expect(label(hour - minute - 1)).toBe("resets in 59 min")
  expect(label(hour - 1)).toBe("resets in 1 h")
  expect(label(hour)).toBe("resets in 1 h")
  expect(label(hour + 1)).toBe("resets in 2 h")
  expect(label(2 * hour - minute)).toBe("resets in 2 h")
  expect(label(day - 1)).toBe("resets in 24 h")
  expect(label(day)).toBeUndefined()
  expect(label(day + 1)).toBeUndefined()
  expect(resetLabels(now + day, now, now + 1)).toEqual([{ at: now + 1, words: "resets in 24 h" }])
  const beforeDST = Date.parse("2026-03-08T01:30:00-05:00")
  const afterDST = Date.parse("2026-03-08T04:29:00-04:00")
  expect(resetLabels(afterDST, beforeDST, beforeDST)).toEqual([{ at: beforeDST, words: "resets in 2 h" }])
})

async function snapshotOf(response: Response) {
  const { data } = await response.json() as { data: {
    rows: LimitRow[]
    usageUpdatedAt?: number
    refreshPending: boolean
    refreshFailed: boolean
    refreshCompletedAt?: number
  } }
  return data
}

test("Limits include the check time for an ended plan without meters", async () => {
  const limits = new Limits(() => [{
    provider: "claude-code", service: "claude", limits: {
      every: minute, read: async () => { throw new LimitsProblem("ended", "Plan ended") },
    },
  }], undefined, () => now)
  await limits.tick()
  const snapshot = await snapshotOf(await limitsRoutes(limits, "GET", []))
  expect(snapshot.usageUpdatedAt).toBe(now)
  expect(snapshot.rows[0]!.usage!).toMatchObject({ status: { words: "Plan ended." }, meters: [] })
})

test("A delayed provider leaves refresh pending and sends an update when the new snapshot is ready", async () => {
  let time = now
  let reads = 0
  const provider = Promise.withResolvers<void>()
  const limits = new Limits(() => [{
    provider: "claude-code", service: "claude", limits: {
      every: minute, read: async () => {
        if (++reads > 1) await provider.promise
        return { windows: [{ id: "five", used: reads / 4 }] }
      },
    },
  }], undefined, () => time)
  await limits.tick()
  time += 2 * minute
  const updates = (await limitsRoutes(limits, "GET", ["events"])).body!.getReader()
  try {
    await updates.read()
    const first = await snapshotOf(await limitsRoutes(limits, "POST", ["refresh"]))
    expect(first.refreshPending).toBe(true)
    expect(first.rows[0]!.usage!.meters[0]!.value).toBe("25%")
    const duplicate = limitsRoutes(limits, "POST", ["refresh"])
    provider.resolve()
    expect((await snapshotOf(await duplicate)).refreshPending).toBe(false)
    expect((await updates.read()).done).toBe(false)
    const after = await snapshotOf(await limitsRoutes(limits, "GET", []))
    expect(after.refreshPending).toBe(false)
    expect(after.usageUpdatedAt).toBe(time)
    expect(after.rows[0]!.usage!.meters[0]!.value).toBe("50%")
    expect(reads).toBe(2)
  } finally {
    provider.resolve()
    await updates.cancel()
  }
}, 8_000)

test("A failed provider refresh keeps the prior successful completion time", async () => {
  let time = now
  let failed = false
  const limits = new Limits(() => [{
    provider: "claude-code", service: "claude", limits: {
      every: minute, read: async () => {
        if (failed) throw new LimitsProblem("failed", "Could not read")
        return { windows: [{ id: "five", used: 0.5 }] }
      },
    },
  }], undefined, () => time)
  const first = await snapshotOf(await limitsRoutes(limits, "POST", ["refresh"]))
  expect(first.refreshCompletedAt).toBe(now)
  time += 2 * minute
  failed = true
  const second = await snapshotOf(await limitsRoutes(limits, "POST", ["refresh"]))
  expect(second.refreshFailed).toBe(true)
  expect(second.refreshCompletedAt).toBe(now)
  expect(second.rows[0]!.usage!.meters[0]!.value).toBe("50%")
})
