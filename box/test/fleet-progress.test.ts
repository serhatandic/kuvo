import "./safety"
import { expect, test } from "bun:test"
import crypto from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import { fleetWith, heldRelease, HELD, watchHeld } from "../src/link/updates/fleet"
import { keepList } from "../src/link/lists/sessions"

// How one of your computers' update reads while it switches to the setup's release (PROTOCOL.md,
// "Updates", Request API): a computer restarting into the release reads as switching, never off;
// a moment's gap in hearing from one never flips its row; its row only moves forward; and the
// setup's update says where it is, in the box's words, and that it's done for a while after.
// Real: the box's own update functions and its run folder; what peers told is given as they'd tell it.

const { FINISHED_FILE, fleetSummary, localFleetUpdate, peerFleetUpdate, QUIET_GRACE, SETTLED_SHOWN, SWITCH_GRACE } = await import("../src/link/updates/progress")

const target = { version: "0.8.0-beta.42", sha256: "a".repeat(64) }
const staged = { version: target.version, sha256: target.sha256 }
const ready = { kuvo: "0.8.0-beta.41", staged }
// What a box on trial of the release tells (its version reads as the one before until the switch
// is confirmed, and its ready marker is spent).
const onTrial = { kuvo: "0.8.0-beta.41", fleetUpdate: { current: "0.8.0-beta.41", target: target.version, canRetry: false, state: "updating", stage: "switching" } } as any
const switched = { kuvo: target.version }

const row = (facts: any, away = false, awayFor = Infinity) => {
  const update = peerFleetUpdate(target, facts, away, awayFor)
  return `${update.state}${"stage" in update ? `/${update.stage}` : ""}: ${update.line?.words}`
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
  expect(row({ kuvo: "0.8.0-beta.41" }, true, 5_000)).toBe("getting-ready: Getting ready…")
  expect(row({ kuvo: "0.8.0-beta.41" }, true, QUIET_GRACE)).toBe("off: beta.41 · Can’t be reached: updates when it’s back")
  // (One that never told anything has nothing to keep.)
  expect(row(undefined, true, 1_000)).toBe("off: Can’t be reached: updates when it’s back")
})

test("a box switching into the release says so of itself", () => {
  // Its version reads as the one before, its ready marker spent: before, this was "getting ready".
  expect(localFleetUpdate(target, { kuvo: "0.8.0-beta.41" }, true)).toMatchObject({ state: "updating", stage: "switching", line: { words: "Switching to beta.42…", tone: "working" } })
  expect(localFleetUpdate(target, { kuvo: target.version }, true)).toMatchObject({ state: "up-to-date" })
  expect(localFleetUpdate(target, { kuvo: "0.8.0-beta.41" }, false)).toMatchObject({ state: "getting-ready" })
})

test("the update notifier stays up, counting the switching computer, while it restarts", () => {
  fs.rmSync(FINISHED_FILE, { force: true })
  const rows = (lokum: any, away: boolean, awayFor: number) => [
    { name: "hetzner", fleetUpdate: peerFleetUpdate(target, switched) },
    { name: "mini-local", fleetUpdate: peerFleetUpdate(target, ready) },
    { name: "lokum", fleetUpdate: peerFleetUpdate(target, lokum, away, awayFor) },
  ]
  const before = fleetSummary(rows(ready, false, Infinity), target)!
  expect(before).toMatchObject({ shown: true, phase: "switching", title: "Switching to beta.42 · 1 of 3 done", detail: "2 still on beta.41." })
  const restarting = fleetSummary(rows(ready, true, 4_000), target)!
  expect(restarting).toMatchObject({ shown: true, stage: "switching", phase: "switching", title: "Switching to beta.42 · 1 of 3 done" })
  const trial = fleetSummary(rows(onTrial, false, Infinity), target)!
  expect(trial).toMatchObject({ shown: true, stage: "switching", phase: "switching", title: "Switching to beta.42 · 1 of 3 done" })
  const lokumDone = fleetSummary(rows(switched, false, Infinity), target)!
  expect(lokumDone).toMatchObject({ shown: true, phase: "switching", title: "Switching to beta.42 · 2 of 3 done", detail: "1 still on beta.41." })
  const done = fleetSummary(rows(switched, false, Infinity).map(row => ({ ...row, fleetUpdate: peerFleetUpdate(target, switched) })), target)!
  expect(done).toMatchObject({ shown: true, phase: "done", title: "All 3 on beta.42" })
  expect(done.detail).toBeUndefined()
  // Never "Everything works on …" once one has switched.
  for (const summary of [before, restarting, trial, lokumDone, done]) expect(`${summary.title} ${summary.detail}`).not.toContain("Everything")
})

test("a computer's row only goes forward, getting ready, ready, switching, up to date; ready never reads as done", () => {
  const phases = [
    peerFleetUpdate(target, { kuvo: "0.8.0-beta.41" }),
    peerFleetUpdate(target, ready),
    peerFleetUpdate(target, ready, true, 2_000),
    peerFleetUpdate(target, onTrial),
    peerFleetUpdate(target, switched),
  ].map(update => [update.line?.phase, update.line?.words, update.line?.tone])
  expect(phases).toEqual([
    ["getting-ready", "Getting ready…", "working"],
    // (Its tone stays "done" for older clients; its phase says it's only ready.)
    ["ready", "Ready for beta.42", "done"],
    ["switching", "Switching to beta.42…", "working"],
    ["switching", "Switching to beta.42…", "working"],
    ["done", "beta.42 · Up to date", "done"],
  ])
  // Can't update: its tone says, no phase.
  const cannot = peerFleetUpdate(target, { kuvo: "0.8.0-beta.41", cannot: { ...staged, reason: "There isn’t room on it.", fixable: true } })
  expect(cannot.line).toEqual({ words: "beta.41 · There isn’t room on it.", tone: "warning", short: "Can’t update" })
})

test("how far a computer is in getting the release ready shows on its row, here and as it told", () => {
  const telling = (fill: number, version = target.version) => ({ kuvo: "0.8.0-beta.41", fleetUpdate: { current: "0.8.0-beta.41", target: version, canRetry: false, state: "getting-ready", fill } }) as any
  expect(peerFleetUpdate(target, telling(0.724))).toMatchObject({ state: "getting-ready", fill: 0.724, line: { words: "Getting ready · 72%", phase: "getting-ready", fill: 0.724, tone: "working" } })
  // All of it come: preparing it, no number to show.
  expect(peerFleetUpdate(target, telling(1.4))).toMatchObject({ fill: 1, line: { words: "Getting ready…", fill: 1 } })
  // How far it got with another release isn't how far it is with this one.
  const other = peerFleetUpdate(target, telling(0.5, "0.8.0-beta.43"))
  expect([other.fill, other.line?.words]).toEqual([undefined, "Getting ready…"])
  // This box, as far as its pull got.
  expect(localFleetUpdate(target, { kuvo: "0.8.0-beta.41" }, false, 0.5)).toMatchObject({ state: "getting-ready", fill: 0.5, line: { words: "Getting ready · 50%" } })
  // Ready, a fill says nothing more.
  const done = localFleetUpdate(target, { kuvo: "0.8.0-beta.41", staged }, false, 1)!
  expect([done.state, done.fill, done.line?.words]).toEqual(["ready", undefined, "Ready for beta.42"])
})

test("a computer that's off says when it was last seen", () => {
  const NOW = 1_800_000_000_000
  const off = (ago?: number, facts: any = { kuvo: "0.8.0-beta.43" }) => peerFleetUpdate(target, facts, true, Infinity, ago === undefined ? undefined : NOW - ago, NOW)
  expect(off(2 * 3_600_000 + 5 * 60_000)).toMatchObject({ state: "off", seen: NOW - 2 * 3_600_000 - 5 * 60_000, line: { words: "beta.43 · seen 2h ago", tone: "off", phase: "off" } })
  expect([30_000, 5 * 60_000 + 10_000, 26 * 3_600_000, 3 * 86_400_000 + 1].map(ago => off(ago).line?.words)).toEqual([
    "beta.43 · seen just now", "beta.43 · seen 5 min ago", "beta.43 · seen yesterday", "beta.43 · seen 3 days ago",
  ])
  // Never heard what it runs: the words alone.
  expect(off(60 * 60_000, {}).line?.words).toBe("Seen 1h ago")
  // Never heard from at all: as before.
  expect(off(undefined).line?.words).toBe("beta.43 · Can’t be reached: updates when it’s back")
  // Not off: no "seen".
  expect(peerFleetUpdate(target, ready, false, Infinity, NOW - 60_000, NOW).seen).toBeUndefined()
})

const named = (name: string, fleetUpdate: any) => ({ name, fleetUpdate })
const at = (name: string, state: string, current: string | null, more: any = {}) => named(name, { state, current, target: target.version, canRetry: false, ...more })

test("the setup's update says where it is, in words that only go forward", () => {
  fs.rmSync(FINISHED_FILE, { force: true })
  const NOW = 1_800_000_000_000
  const say = (rows: any[]) => {
    const summary = fleetSummary(rows, target, NOW)!
    return [summary.phase, summary.title, summary.detail, summary.shown]
  }
  expect(say([at("main-linux", "getting-ready", "0.8.0-beta.41"), at("mac-mini", "getting-ready", "0.8.0-beta.41"), at("spare", "off", "0.8.0-beta.38")]))
    .toEqual(["getting-ready", "Getting beta.42 ready", "Your computers stay on beta.41 meanwhile.", true])
  expect(say([at("main-linux", "ready", "0.8.0-beta.41"), at("mac-mini", "getting-ready", "0.8.0-beta.40"), at("spare", "off", "0.8.0-beta.38")]))
    .toEqual(["getting-ready", "Getting beta.42 ready · 1 of 3", "Your computers keep what they run meanwhile.", true])
  expect(say([at("main-linux", "ready", "0.8.0-beta.41"), at("mac-mini", "ready", "0.8.0-beta.41"), at("spare", "off", "0.8.0-beta.38")]))
    .toEqual(["ready", "beta.42 is ready · 2 of 3", "They switch next.", true])
  expect(say([at("main-linux", "up-to-date", target.version), at("mac-mini", "updating", "0.8.0-beta.41", { stage: "switching" }), at("macbook", "ready", "0.8.0-beta.41")]))
    .toEqual(["switching", "Switching to beta.42 · 1 of 3 done", "2 still on beta.41.", true])
  expect(say([at("main-linux", "up-to-date", target.version), at("mac-mini", "ready", "0.8.0-beta.41"), at("spare", "off", "0.8.0-beta.38")]))
    .toEqual(["switching", "Switching to beta.42 · 1 of 3 done", "2 still to switch.", true])
  expect(say([at("main-linux", "up-to-date", target.version), at("mac-mini", "up-to-date", target.version), at("spare", "off", "0.8.0-beta.41")]))
    .toEqual(["waiting", "beta.42 · 2 of 3 up to date", "spare updates when it’s back.", true])
  expect(say([at("main-linux", "up-to-date", target.version), at("spare", "off", "0.8.0-beta.41"), at("old-mini", "off", null)]))
    .toEqual(["waiting", "beta.42 · 1 of 3 up to date", "2 computers update when they’re back.", true])
  expect(say([at("main-linux", "up-to-date", target.version), at("mac-mini", "up-to-date", target.version)])).toEqual(["done", "All 2 on beta.42", undefined, true])
  expect(say([at("main-linux", "up-to-date", target.version)])).toEqual(["done", "Up to date on beta.42", undefined, true])
  // A fixable one keeps its warning, with where the rest are.
  const room = "There isn’t room on it to keep its data while it updates."
  expect(fleetSummary([at("main-linux", "up-to-date", target.version), at("spare", "cannot", "0.8.0-beta.41", { reason: room, fixable: true })], target, NOW))
    .toMatchObject({ warning: true, shown: true, title: "spare can’t update to beta.42", detail: `spare: ${room}`, phase: "waiting" })
})

test("a finished update shows for ten minutes on a box that saw it under way, and never on one that didn't", () => {
  fs.rmSync(FINISHED_FILE, { force: true })
  const NOW = 1_800_000_000_000
  const settled = [at("main-linux", "up-to-date", target.version), at("spare", "off", "0.8.0-beta.41")]
  // Never seen in progress here: nothing to tell.
  expect(fleetSummary(settled, target, NOW)).toMatchObject({ phase: "waiting", shown: false })
  expect(fleetSummary(settled, target, NOW + 60_000)!.detail).toBeUndefined()
  // Seen under way, then finished: shown from then, for ten minutes.
  expect(fleetSummary([at("main-linux", "ready", "0.8.0-beta.41"), at("spare", "off", "0.8.0-beta.41")], target, NOW + 2 * 60_000)!.shown).toBe(true)
  const finished = NOW + 3 * 60_000
  expect(fleetSummary(settled, target, finished)).toMatchObject({ phase: "waiting", shown: true, title: "beta.42 · 1 of 2 up to date", detail: "spare updates when it’s back." })
  expect(fleetSummary(settled, target, finished + SETTLED_SHOWN - 1)!.shown).toBe(true)
  expect(fleetSummary(settled, target, finished + SETTLED_SHOWN)).toMatchObject({ phase: "waiting", shown: false })
  // Another release finished: this box never saw it under way.
  expect(fleetSummary(settled.map(row => ({ ...row, fleetUpdate: { ...row.fleetUpdate, target: "0.8.0-beta.43" } })), { version: "0.8.0-beta.43" }, finished + 60_000)!.shown).toBe(false)
})

test("the resident announces a kept release after its pack has been verified", async () => {
  const keys = crypto.generateKeyPairSync("ed25519")
  const key = keys.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64")
  const pack = Buffer.from("A signed release payload")
  const sha256 = crypto.createHash("sha256").update(pack).digest("hex")
  const version = "0.8.0-beta.43"
  const manifest = Buffer.from(JSON.stringify({
    kuvo: "box/1", version, engine: "1.0.0", spreads: true,
    pack: { url: "https://example.invalid/box.tar.gz", sha256 },
  }))
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), keys.privateKey).toString("base64")
  fs.mkdirSync(HELD, { recursive: true })
  fs.writeFileSync(path.join(HELD, "box.tar.gz"), pack)
  fs.writeFileSync(path.join(HELD, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), key, signature }))
  fleetWith({ keys: [key] })
  const stop = keepList()
  const heard = Promise.withResolvers<void>()
  const watcher = watchHeld(heard.resolve)
  try {
    expect(heldRelease()).toBeNull()
    await heard.promise
    expect(heldRelease()).toMatchObject({ version, sha256, size: pack.length })
  } finally {
    watcher.close()
    stop()
    fleetWith({})
    fs.rmSync(HELD, { recursive: true, force: true })
  }
})
