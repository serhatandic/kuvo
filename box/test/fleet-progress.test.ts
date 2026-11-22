import "./safety"
import { expect, test } from "bun:test"

// How one of your computers' update reads while it switches to the setup's release (PROTOCOL.md,
// "Updates", Request API): a computer restarting into the release reads as switching, never off;
// a moment's gap in hearing from one never flips its row; and its row only moves forward.

const { fleetSummary, localFleetUpdate, peerFleetUpdate, QUIET_GRACE, SWITCH_GRACE } = await import("../src/link/updates/progress")

const target = { version: "0.8.0-beta.42", sha256: "a".repeat(64) }
const staged = { version: target.version, sha256: target.sha256 }
const ready = { kuvo: "0.8.0-beta.41", staged }
// What a box on trial of the release tells (its version reads as the one before until the switch
// is confirmed, and its ready marker is spent).
const onTrial = { kuvo: "0.8.0-beta.41", fleetUpdate: { current: "0.8.0-beta.41", target: target.version, canRetry: false, state: "updating", stage: "switching" } } as any
const switched = { kuvo: target.version }

const row = (facts: any, away = false, awayFor = Infinity) => {
  const update = peerFleetUpdate(target, facts, away, awayFor)
  return `${update.state}${"stage" in update ? `/${update.stage}` : ""}: ${update.title}`
}

test("a computer that goes quiet right after getting ready reads as switching to the release, not off", () => {
  expect(row(ready)).toBe("ready: Ready for beta.42")
  // (The relay found it gone a moment ago: it's restarting.)
  expect(row(ready, true, 3_000)).toBe("updating/switching: Switching to beta.42…")
  // (Still quiet after a few minutes: a slow restart.)
  expect(row(ready, true, QUIET_GRACE + 60_000)).toBe("updating/switching: Switching to beta.42…")
  // Only once it's been quiet far longer than any switch takes is it out of reach.
  expect(row(ready, true, SWITCH_GRACE + 1)).toBe("off: beta.41 · Can’t be reached: updates when it’s back")
  expect(row(ready, true)).toBe("off: beta.41 · Can’t be reached: updates when it’s back")
})

test("a computer's row moves only forward while it switches: ready, switching, up to date", () => {
  const seen = [
    row(ready),
    row(ready, true, 2_000),
    // Back, on trial: before, this read as getting ready again.
    row(onTrial),
    row(onTrial, true, 1_000),
    row(switched),
  ]
  expect(seen).toEqual([
    "ready: Ready for beta.42",
    "updating/switching: Switching to beta.42…",
    "updating/switching: Switching to beta.42…",
    "updating/switching: Switching to beta.42…",
    "up-to-date: beta.42 · Up to date",
  ])
})

test("a moment's gap in hearing from a computer keeps what it last told", () => {
  expect(row(switched, true, 5_000)).toBe("up-to-date: beta.42 · Up to date")
  expect(row({ kuvo: "0.8.0-beta.41" }, true, 5_000)).toBe("getting-ready: Getting beta.42 ready…")
  expect(row({ kuvo: "0.8.0-beta.41" }, true, QUIET_GRACE)).toBe("off: beta.41 · Can’t be reached: updates when it’s back")
})

test("a box switching into the release says so of itself", () => {
  // Its version reads as the one before, its ready marker spent: before, this was "getting ready".
  expect(localFleetUpdate(target, { kuvo: "0.8.0-beta.41" }, true)).toMatchObject({ state: "updating", stage: "switching", title: "Switching to beta.42…" })
  expect(localFleetUpdate(target, { kuvo: target.version }, true)).toMatchObject({ state: "up-to-date" })
  expect(localFleetUpdate(target, { kuvo: "0.8.0-beta.41" }, false)).toMatchObject({ state: "getting-ready" })
})

test("the update notifier stays up, counting the switching computer, while it restarts", () => {
  const rows = (lokum: any, away: boolean, awayFor: number) => [
    { name: "hetzner", fleetUpdate: peerFleetUpdate(target, switched) },
    { name: "mini-local", fleetUpdate: peerFleetUpdate(target, ready) },
    { name: "lokum", fleetUpdate: peerFleetUpdate(target, lokum, away, awayFor) },
  ]
  const before = fleetSummary(rows(ready, false, Infinity), target)!
  expect(before).toMatchObject({ shown: true, title: "Updating to beta.42 · 3 of 3 ready" })
  const restarting = fleetSummary(rows(ready, true, 4_000), target)!
  expect(restarting).toMatchObject({ shown: true, stage: "switching", title: "Updating to beta.42 · 3 of 3 ready" })
  const trial = fleetSummary(rows(onTrial, false, Infinity), target)!
  expect(trial).toMatchObject({ shown: true, stage: "switching", title: "Updating to beta.42 · 3 of 3 ready" })
  const done = fleetSummary(rows(switched, false, Infinity), target)!
  expect(done).toMatchObject({ shown: true, title: "Updating to beta.42 · 3 of 3 ready" })
})
