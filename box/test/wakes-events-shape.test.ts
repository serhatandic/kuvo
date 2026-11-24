import "./safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"

// What `kuvo/wakes/events` tells a client fits the route's declared shape, as `kuvo/wakes` does:
// each conversation's soonest wake-up and whether it watches, beside its wake-ups whole.

const { allWakes } = await import("../src/link/areas/wakes")
const { SESSION_WAKES } = await import("../src/link/home")
const { WAKE_LIST } = await import("../src/shared/routes/wakes")
const { fitsShape } = await import("../src/shared/shape")

const folder = join(SESSION_WAKES, "ses_eventshape1")
afterAll(() => rmSync(folder, { recursive: true, force: true }))

test("every line of kuvo/wakes/events says whether each conversation watches, and its soonest wake-up", () => {
  mkdirSync(join(folder, "wakes"), { recursive: true })
  mkdirSync(join(folder, "watches", "Wt1"), { recursive: true })
  writeFileSync(join(folder, "session"), "ses_eventshape1")
  writeFileSync(join(folder, "wakes", "4102444800-Wk1"), "Check the build")
  writeFileSync(join(folder, "watches", "Wt1", "check"), "true")
  writeFileSync(join(folder, "watches", "Wt1", "every"), "30m")
  // (As a client reads it, off the wire.)
  const line = JSON.parse(JSON.stringify({ sessions: allWakes() }))
  const mine = line.sessions.find((each: { session: string }) => each.session === "ses_eventshape1")
  expect(mine).toMatchObject({ next: 4102444800, watching: true, paused: false })
  expect(fitsShape(WAKE_LIST, line)).toBe(true)
})
