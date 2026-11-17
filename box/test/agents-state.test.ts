import "./safety"
// How a box says its coding agents stand while their bridge can't be asked (PROTOCOL.md, "The
// coding agents' state"): restarting while one makes way on purpose or starts, else down with one
// plain reason, and Try Again where it helps. Each case is a box's `run` folder of its own, as its
// keeper and bridge leave it, with processes told apart by their pids.
import { afterEach, beforeEach, expect, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { agentsAway, agentsState, agentsTiming, bridgeBack, forgetAgentsState, type AgentsPlace } from "../src/link/areas/agents-down"

let home: string
let run: string
let bin: string
// Which pids run which program, as a box's processes would.
let running: Map<number, string>
const NOW = 1_800_000_000_000

beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "agents-state-"))
  run = path.join(home, "run")
  bin = path.join(home, "bin")
  fs.mkdirSync(run)
  fs.mkdirSync(bin)
  fs.writeFileSync(path.join(bin, "kuvo-agents.js"), "")
  running = new Map([[100, "opencode serve"], [200, "sh kuvo-start keep"]])
  fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ pid: 100, port: 4096, password: "p" }))
  fs.writeFileSync(path.join(run, "box.pid"), "200")
  forgetAgentsState()
})
afterEach(() => fs.rmSync(home, { recursive: true, force: true }))

const place = (now = NOW): Partial<AgentsPlace> => ({ run, bin, now, runs: (pid, name) => running.get(Number(pid))?.includes(name) ?? false })
const ageFile = (file: string, ms: number) => fs.utimesSync(file, new Date(NOW - ms), new Date(NOW - ms))

test("a bridge that runs is running, and nothing is said", () => {
  running.set(300, "opencode kuvo-agents.js")
  fs.writeFileSync(path.join(run, "bridge.json"), JSON.stringify({ pid: 300, port: 5000, password: "q" }))
  expect(agentsState(place())).toEqual({ state: "running" })
  expect(agentsAway(place())).toBeNull()
})

test("a bridge making way for a newer one is restarting, never an error, until it's been gone too long", () => {
  fs.writeFileSync(path.join(run, "agents-renewing"), JSON.stringify({ at: NOW - 2_000, pid: 300 }))
  expect(agentsAway(place())).toEqual({ message: "The coding agents are restarting. This takes a few seconds.", reason: "agentsRestarting" })
  // Its keeper isn't there to start the next one either: still a hand-over, for a while.
  running.delete(200)
  expect(agentsState(place()).state).toBe("restarting")
  // Long after it went, with nothing come of it: down, and why.
  expect(agentsState(place(NOW + agentsTiming.renewing))).toMatchObject({ state: "down", why: "keeper" })
})

test("a bridge its keeper started that's still starting is restarting; one that never finished starting is stuck, with Try Again", () => {
  running.set(300, "opencode kuvo-agents.js")
  fs.writeFileSync(path.join(run, "agents.pid"), "300")
  ageFile(path.join(run, "agents.pid"), 5_000)
  expect(agentsState(place()).state).toBe("restarting")
  ageFile(path.join(run, "agents.pid"), agentsTiming.starting + 1_000)
  expect(agentsAway(place())).toEqual({ message: "The coding agents didn't finish starting.", reason: "agentsDown", action: { label: "Try Again", route: "kuvo/agents/start" } })
})

test("one that keeps stopping says when Kuvo tries again, with Try Again", () => {
  fs.writeFileSync(path.join(run, "agents.retry"), String(Math.round((NOW + 5 * 60_000) / 1000)))
  expect(agentsAway(place())).toEqual({ message: "The coding agents keep stopping. Kuvo tries again in about 5 minutes.", reason: "agentsDown", action: { label: "Try Again", route: "kuvo/agents/start" } })
  // Its next start a moment away: restarting.
  fs.writeFileSync(path.join(run, "agents.retry"), String(Math.round((NOW + 10_000) / 1000)))
  expect(agentsState(place()).state).toBe("restarting")
})

test("with no keeper to start them, they're down and Try Again starts them", () => {
  running.delete(200)
  expect(agentsAway(place())).toEqual({ message: "The coding agents stopped, and nothing is starting them again.", reason: "agentsDown", action: { label: "Try Again", route: "kuvo/agents/start" } })
})

test("while the engine isn't running, they wait for it, and the engine's own line says why", () => {
  running.delete(100)
  expect(agentsAway(place())).toEqual({ message: "The coding agents start once the engine is running.", reason: "agentsDown" })
  fs.rmSync(path.join(run, "engine.json"))
  expect(agentsState(place())).toMatchObject({ state: "down", why: "engine" })
})

test("a box without the bridge's program says it isn't installed", () => {
  fs.rmSync(path.join(bin, "kuvo-agents.js"))
  expect(agentsState(place())).toMatchObject({ state: "down", why: "notInstalled", tryAgain: false })
})

test("one that ended with nothing to explain it is restarting while its keeper would start it, then said to be down", () => {
  expect(agentsState(place()).state).toBe("restarting")
  expect(agentsState(place(NOW + agentsTiming.keeper - 1))).toMatchObject({ state: "restarting" })
  expect(agentsAway(place(NOW + agentsTiming.keeper))).toEqual({ message: "The coding agents haven't started.", reason: "agentsDown", action: { label: "Try Again", route: "kuvo/agents/start" } })
})

test("a request waits for a bridge that's restarting, and gets it once it's back", async () => {
  fs.writeFileSync(path.join(run, "agents-renewing"), JSON.stringify({ at: Date.now(), pid: 300 }))
  let looks = 0
  const back = { port: 5001, password: "r" }
  // The new bridge records itself a few looks on.
  const found = await bridgeBack(() => (++looks >= 5 ? back : null), undefined, 30_000, { ...place(), now: undefined })
  expect(found).toEqual(back)
  expect(looks).toBe(5)
})

test("a request doesn't wait for a bridge that's down", async () => {
  running.delete(200)
  ageFile(path.join(run, "box.pid"), 0)
  let looks = 0
  const from = Date.now()
  expect(await bridgeBack(() => (looks++, null), undefined, 30_000, place())).toBeNull()
  // (Looked at once, with no pause: the bridge is asked for as the state's worked out too.)
  expect(looks).toBeLessThanOrEqual(2)
  expect(Date.now() - from).toBeLessThan(200)
})

test("a bridge's program from an older Kuvo, which can't start under its keeper, is said to be: put back by itself, or by the next update where there's nothing sound to repair it from", () => {
  fs.writeFileSync(path.join(run, "agents.log"), "KUVO_AGENTS_HOME, KUVO_HOME, KUVO_AGENTS_PORT, KUVO_AGENTS_PASSWORD, KUVO_ENGINE_PORT and KUVO_ENGINE_PID are needed.\n")
  fs.writeFileSync(path.join(run, "agents.retry"), String(Math.round((NOW + 5 * 60_000) / 1000)))
  expect(agentsAway(place())).toEqual({ message: "The coding agents here are from an older Kuvo and can't start. Kuvo puts back the right ones by itself.", reason: "agentsDown", action: { label: "Try Again", route: "kuvo/agents/start" } })
  fs.writeFileSync(path.join(run, "repair.json"), JSON.stringify({ at: NOW, result: "damaged", version: "0.10.0" }))
  expect(agentsAway(place())).toEqual({ message: "The coding agents here are from an older Kuvo and can't start, and there's no sound copy here to repair them from. Kuvo's next update brings them.", reason: "agentsDown" })
})

test("a second outage long after the first is restarting again, not down: a request that reached a bridge counts as one seen", async () => {
  // The first: the keeper's starting it again.
  expect(agentsState(place()).state).toBe("restarting")
  // Back, found by a request.
  const back = { port: 5002, password: "s" }
  expect(await bridgeBack(() => back, undefined, 30_000, place())).toEqual(back)
  // Gone again days later: restarting, as the first time.
  expect(agentsState(place(NOW + 3 * 24 * 3600_000)).state).toBe("restarting")
})
