import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { FRESH, KeptSummaries } from "../../src/usage/insights-kept"

// Insights opens on what the box already has (PROTOCOL.md, "Insights", `asOf`): a period asked
// again is answered at once with the box's last summary for it while it's read afresh, a box that
// just started answers from the one it kept on disk, and a summary still being read is never kept.

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-insights-kept-test-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

/** A read the test finishes when it says. */
function deferred<T>() {
  let finish!: (value: T) => void
  const promise = new Promise<T>(resolve => { finish = resolve })
  return { promise, finish }
}

const month = new URLSearchParams({ period: "30", zone: "Europe/Istanbul" })
const week = new URLSearchParams({ period: "7", zone: "Europe/Istanbul" })
const summary = (cost: number, state = "ready") => ({ state, period: 30, totals: { cost } })

test("a period asked again is answered at once with the last summary, marked as of when, while it's read afresh", async () => {
  let now = 1_000_000
  const file = join(mkdtempSync(join(base, "box-")), "insights-kept.json")
  const kept = new KeptSummaries(file, () => now)
  let reads = 0
  const first = deferred<any>()
  const opening = kept.answer(month, () => { reads++; return first.promise })
  first.finish(summary(4.2))
  expect(await opening).toEqual(summary(4.2))

  // Asked again a moment later: as it is, nothing read again.
  now += FRESH - 1
  expect(await kept.answer(month, () => { reads++; return Promise.resolve(summary(9)) })).toEqual(summary(4.2))
  expect(reads).toBe(1)

  // Later: the last at once, as of when it was read, while it's read afresh (still reading).
  now += 60_000
  const again = deferred<any>()
  expect(await kept.answer(month, () => { reads++; return again.promise })).toEqual({ ...summary(4.2), asOf: 1_000_000 })
  // (Asked meanwhile: still the last, and not read twice.)
  expect(await kept.answer(month, () => { reads++; return Promise.resolve(summary(0)) })).toEqual({ ...summary(4.2), asOf: 1_000_000 })
  expect(reads).toBe(2)
  again.finish(summary(5.5))
  await again.promise
  expect(await kept.answer(month, () => { reads++; return Promise.resolve(summary(0)) })).toEqual(summary(5.5))

  // Another period is its own: never the month's figures.
  expect(await kept.answer(week, () => Promise.resolve({ ...summary(1.1), period: 7 }))).toEqual({ ...summary(1.1), period: 7 })

  // A box that just started has them too.
  now += 3_600_000
  const restarted = new KeptSummaries(file, () => now)
  const pending = deferred<any>()
  expect(await restarted.answer(month, () => pending.promise)).toEqual({ ...summary(5.5), asOf: now - 3_600_000 })
})

test("a summary that says the box is still reading its usage is never kept", async () => {
  let now = 5_000_000
  const kept = new KeptSummaries(join(mkdtempSync(join(base, "box-")), "insights-kept.json"), () => now)
  expect(await kept.answer(month, () => Promise.resolve(summary(0, "loading")))).toEqual(summary(0, "loading"))
  now += FRESH * 2
  expect(await kept.answer(month, () => Promise.resolve(summary(3)))).toEqual(summary(3))
})
