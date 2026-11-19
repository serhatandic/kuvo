import "./safety"
import { afterAll, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { judgeInstall } from "../src/link/install/verdict"

// What the box's installer says a computer holds (`kuvo-link.js install --status`, PROTOCOL.md
// "Installing this computer's box"): one verdict from the folders alone, each laid out here as the
// app, a move or a machine's box leaves it, in a home of the test's own.
const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-install-verdict-")))
afterAll(() => rmSync(root, { recursive: true, force: true }))
const mac = process.platform === "darwin"

let count = 0
/** A computer: its box's home (`K`) and the old app's folder (`S`), nothing in either yet. */
function computer() {
  const home = join(root, `c${++count}`)
  const K = join(home, ".kuvo-code"), S = join(home, "Library", "Application Support", "run.kuvo.code.trial713")
  const put = (file: string, text = "") => { mkdirSync(dirname(file), { recursive: true }); writeFileSync(file, text) }
  return { K, S, put, verdict: (helper?: boolean) => { const { verdict, verb } = judgeInstall(K, S, helper === undefined ? {} : { helperInPlace: () => helper }); return { verdict, verb } } }
}
/** A machine's box as the computer that set it up leaves it: its start script, its link and card. */
function machineBox({ K, put }: ReturnType<typeof computer>) {
  put(join(K, "bin", "kuvo-start"), "#!/bin/sh\n")
  put(join(K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }))
}
const journal = (step: string) => JSON.stringify({ v: 1, way: "move", began: 1, step, from: "/s", to: "/k", placements: [], clearing: [] })

test("fresh: nothing of Kuvo Code's here, or only what's nobody's data (the engine's download, caches, the desktop app's own folder)", () => {
  const empty = computer()
  expect(empty.verdict()).toEqual({ verdict: "fresh", verb: "new" })
  const opened = computer()
  opened.put(join(opened.S, "engine", "2.2.6", "opencode"))
  opened.put(join(opened.S, "desktop", "window.json"), "{}")
  opened.put(join(opened.S, "opencode", "cache", "models.json"), "{}")
  opened.put(join(opened.S, ".DS_Store"))
  // (And what the installer itself staged in the home.)
  opened.put(join(opened.K, "updates", "install", "release", "box.json"), "{}")
  expect(opened.verdict()).toEqual({ verdict: "fresh", verb: "new" })
})

test("ours: a Mac's own box in its home; `new` finishes it only while what starts it isn't in place", () => {
  const here = computer()
  here.put(join(here.K, "box", "kind"), "mac\n")
  expect(here.verdict(true)).toEqual({ verdict: "ours", verb: null })
  expect(here.verdict(false)).toEqual({ verdict: "ours", verb: "new" })
  // (Where that can't be told, a demo copy's home: nothing to run.)
  expect(here.verdict()).toEqual({ verdict: "ours", verb: null })
  // The old app's leftovers beside it change nothing: its box is here.
  here.put(join(here.S, "Chats", "c1", "notes.md"), "kept")
  expect(here.verdict(true).verdict).toBe("ours")
})

test("ours, moved by the box itself: a move that's done is never run again, whatever it still owes (opening or updating Kuvo never waits on the Keychain); its helper not in place, `new` starts it", () => {
  const moved = computer()
  moved.put(join(moved.K, "box", "kind"), "mac\n")
  moved.put(join(moved.S, "Chats", "c1", "notes.md"), "as it was: the move never changes the earlier app's folder")
  const state = (owed: string[]) => moved.put(join(moved.K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "move", owed, app: { bundle: "/Applications/Kuvo Code.app", id: "run.kuvo.code" }, from: moved.S, to: moved.K }))
  for (const owed of [["start", "keychain", "passwords"], ["keychain"], ["passwords"], ["agent"]]) {
    state(owed)
    expect({ owed, ...moved.verdict(true) }).toEqual({ owed, verdict: "ours", verb: null })
    expect(moved.verdict(false).verb).toBe("new")
  }
  state([])
  expect(moved.verdict(true)).toEqual({ verdict: "ours", verb: null })
  // (The keeper's move owed its app's executable, which no installer settles: never `move` for it.)
  moved.put(join(moved.K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "move", owed: ["keychain", "agent"], from: moved.S, to: moved.K }))
  expect(moved.verdict(true)).toEqual({ verdict: "ours", verb: null })
})

test("a new box's making cut short isn't a box yet: fresh, to be made again", () => {
  const cut = computer()
  cut.put(join(cut.K, "updates", "rehome", "new"), "1\n")
  cut.put(join(cut.K, "bin", "kuvo-link.js"), "// half of it")
  expect(cut.verdict()).toEqual({ verdict: "fresh", verb: "new" })
})

test("app-07: an earlier app's data in its folder, no box of its own there", () => {
  for (const file of ["opencode/data/opencode/opencode.db", "bots/b1/bot.json", "Chats/c1/notes.md", "groups.json", "personalities.json", "agents/claude/settings.json", "box/machines.json", "integrations/linear/token"]) {
    const earlier = computer()
    earlier.put(join(earlier.S, file), "x")
    expect({ file, ...earlier.verdict() }).toEqual({ file, verdict: "app-07", verb: "move" })
  }
})

test("app-box: a keeper build's box ran in the app's folder", () => {
  const keeper = computer()
  keeper.put(join(keeper.S, "box", "run", "keeper.json"), "{}")
  keeper.put(join(keeper.S, "opencode", "data", "opencode", "opencode.db"), "x")
  expect(keeper.verdict()).toEqual({ verdict: "app-box", verb: "move" })
})

test("moving and stuck: a move's journal, before and past the point it goes back from; one that can't be read is stuck", () => {
  const moving = computer()
  moving.put(join(moving.S, "box", "run", "keeper.json"), "{}")
  moving.put(join(moving.K, "updates", "rehome", "journal.json"), journal("clone"))
  expect(moving.verdict()).toEqual({ verdict: "moving", verb: "move" })
  expect(judgeInstall(moving.K, moving.S).status).toMatchObject({ state: "moving", step: "clone" })
  const stuck = computer()
  stuck.put(join(stuck.K, "box", "kind"), "mac\n")
  stuck.put(join(stuck.K, "updates", "rehome", "journal.json"), journal("clear"))
  expect(stuck.verdict()).toEqual({ verdict: "stuck", verb: "move" })
  const unreadable = computer()
  unreadable.put(join(unreadable.K, "updates", "rehome", "journal.json"), "{ not json")
  expect(unreadable.verdict()).toEqual({ verdict: "stuck", verb: "move" })
})

test("waits: another process holds the move's lock; one that's gone holds nothing", async () => {
  const held = computer()
  held.put(join(held.S, "box", "run", "keeper.json"), "{}")
  // A mover, by its command line (`… rehome`), alive until it's told to go.
  const mover = spawn(process.execPath, ["-e", "process.stdout.write('ready'); process.stdin.resume()", "rehome"], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await once(mover.stdout, "data")
    held.put(join(held.K, "updates", "rehome", "lock", "pid"), String(mover.pid))
    expect(held.verdict()).toEqual({ verdict: "waits", verb: null })
  } finally {
    const gone = new Promise(resolve => mover.once("exit", resolve))
    mover.kill()
    await gone
  }
  expect(held.verdict()).toEqual({ verdict: "app-box", verb: "move" })
})

test("what a move that's done left to finish holds the lock without a move: bringing saved sign-ins over, or settling what asks nothing, never keeps Kuvo from opening or updating", async () => {
  const moved = computer()
  moved.put(join(moved.K, "box", "kind"), "mac\n")
  moved.put(join(moved.K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "move", owed: ["passwords"], app: { bundle: "/Applications/Kuvo Code.app", id: "run.kuvo.code" }, from: moved.S, to: moved.K }))
  for (const verb of ["--sign-ins", "--settle"]) {
    // (As it runs: `… rehome .kuvo-code --sign-ins`, waiting on macOS's question.)
    const task = spawn(process.execPath, ["-e", "process.stdin.resume()", "rehome", ".kuvo-code", verb], { stdio: ["pipe", "ignore", "ignore"] })
    try {
      moved.put(join(moved.K, "updates", "rehome", "lock", "pid"), String(task.pid))
      expect({ task: verb, ...moved.verdict(true) }).toEqual({ task: verb, verdict: "ours", verb: null })
      expect(judgeInstall(moved.K, moved.S).status.signIns).toEqual(verb === "--sign-ins" ? { state: "asking" } : null)
    } finally {
      const gone = new Promise(resolve => task.once("exit", resolve))
      task.kill()
      await gone
    }
  }
})

test("back: moved back, it moves again only when asked", () => {
  const back = computer()
  back.put(join(back.S, "box", "run", "keeper.json"), "{}")
  back.put(join(back.K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "back", movedBack: true }))
  expect(back.verdict()).toEqual({ verdict: "back", verb: null })
})

test("machine and two: a machine's box in the home is never made over, with or without an earlier app's data", () => {
  const machine = computer()
  machineBox(machine)
  expect(machine.verdict()).toEqual({ verdict: "machine", verb: null })
  const two = computer()
  machineBox(two)
  two.put(join(two.S, "opencode", "data", "opencode", "opencode.db"), "x")
  expect(two.verdict()).toEqual({ verdict: "two", verb: null })
  // One the computer that set it up removed (turned off, its start script gone) is no box.
  const removed = computer()
  machineBox(removed)
  removed.put(join(removed.K, "run", "off"))
  expect(removed.verdict()).toEqual({ verdict: "fresh", verb: "new" })
})

test("on Linux a box's start script alone says a machine's box is there, but one the installer was cut short making", () => {
  const bare = computer()
  bare.put(join(bare.K, "bin", "kuvo-start"), "#!/bin/sh\n")
  expect(judgeInstall(bare.K, null).verdict).toBe(mac ? "fresh" : "machine")
  bare.put(join(bare.K, "updates", "install", "making"), "1\n")
  expect(judgeInstall(bare.K, null)).toMatchObject({ verdict: "fresh", verb: "new" })
  // (Whatever its start left before it failed, keys and a link too: not running, it isn't a box.)
  machineBox(bare)
  bare.put(join(bare.K, "box", "keys", "k"), "x")
  expect(judgeInstall(bare.K, null)).toMatchObject(mac ? { verdict: "machine" } : { verdict: "fresh", verb: "new" })
})
