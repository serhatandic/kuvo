// Cursor through a bridge update, as a box takes one: the bridge before makes way and ends, and the
// next one starts. Cursor's worker (its SDK, with the agent live in it) goes on, never stopped: a
// run at work finishes, shown as still at work until then, and the next message goes to the same
// worker and agent; a worker between runs takes the next message as it is. Cursor's SDK is a
// stand-in on disk, loaded by the real worker; the bridge before runs as a process of its own
// (support/update-cursor.ts), as the real one does, and ends.

import "../safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { adoptHeld } from "../../src/agents/hold/adopt"
import { heldIn, holdFolder } from "../../src/agents/hold/client"
import type { Sessions } from "../../src/agents/sessions"
import type { Message } from "../../src/agents/wire"
import { cursorBridge, goOn, sentIn, standInCursor } from "./support/update-cursor"

const root = mkdtempSync(join(process.env.HOME!, "u"))
afterAll(() => rmSync(root, { recursive: true, force: true }))

async function until(condition: () => boolean) {
  while (!condition()) await new Promise((resolve) => setTimeout(resolve, 10))
}

/** Each test's box: a Kuvo Code folder of its own (a short path: a holder's socket is in it), its holders its alone. */
let kuvo = ""
let holds: string | undefined
let after: Sessions | undefined

// Holders run in sessions of their own: those a test left are stopped, by their own pid.
afterEach(() => {
  after?.closeAll()
  after = undefined
  delete process.env.CURSOR_API_KEY
  for (const record of holds ? heldIn(holds) : []) process.kill(record.pid, "SIGTERM")
  holds = undefined
  if (kuvo) rmSync(kuvo, { recursive: true, force: true })
})

/** A test's folder, with Cursor's SDK in its runtime. */
function folders() {
  const folder = mkdtempSync(join(root, "/"))
  const runtime = join(folder, "runtime")
  mkdirSync(runtime, { recursive: true })
  standInCursor(folder, runtime)
  return { folder, runtime }
}

/** The bridge before, a machine's, left with Cursor as `scenario` says, until it's told to make way: then it does, and ends. */
async function bridgeBefore(folder: string, runtime: string, scenario: "working" | "idle"): Promise<string> {
  kuvo = mkdtempSync(join(process.env.HOME!, "k"))
  holds = holdFolder(kuvo)
  expect(holds).toBeDefined()
  const child = spawn(process.execPath, [join(import.meta.dir, "support", "update-cursor.ts"), folder, runtime, scenario], {
    stdio: ["pipe", "pipe", "inherit"],
    env: { ...process.env, KUVO_HOME: kuvo, KUVO_AGENTS_REMOTE: "1", CURSOR_API_KEY: "fixture-only" },
  })
  const ended = new Promise<number | null>((resolve) => child.once("exit", resolve))
  const session = await new Promise<string>((resolve, reject) => {
    let said = ""
    child.stdout!.setEncoding("utf8")
    child.stdout!.on("data", (chunk: string) => {
      said += chunk
      if (said.includes("\n")) resolve(JSON.parse(said.split("\n")[0]!).session)
    })
    child.once("exit", (code) => reject(new Error(`The bridge before ended (${code}) before Cursor was ${scenario}.`)))
  })
  child.stdin!.write("make way\n")
  expect(await ended).toBe(0)
  return session
}

/** The next bridge, as it starts: every agent held here taken up, runs under way and those between them. */
async function nextBridge(folder: string, runtime: string) {
  process.env.CURSOR_API_KEY = "fixture-only"
  const adopted = await adoptHeld(holds)
  const next = cursorBridge(folder, runtime, adopted.turns)
  after = next.sessions
  next.sessions.resumeHeld()
  const idle = adopted.idle.map(({ held, meta }) => next.sessions.adoptIdle(held, meta))
  return { ...next, idle }
}

const pids = (folder: string) => readFileSync(join(folder, "pids"), "utf8").trim().split("\n").map(Number)
const idles = (sessions: Sessions, id: string) => sessions.messages(id).filter((message): message is Extract<Message, { type: "idle" }> => message.type === "idle")
const texts = (sessions: Sessions, id: string) => sessions.messages(id).flatMap((message) => (message.type === "assistant" ? message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])) : []))

test("Cursor: a run at work goes on through a bridge update, in the same worker, to its end; the next message goes to that worker and agent", async () => {
  const { folder, runtime } = folders()
  const id = await bridgeBefore(folder, runtime, "working")
  const [worker] = pids(folder)
  // Never stopped as the bridge before went.
  process.kill(worker!, 0)

  const { sessions } = await nextBridge(folder, runtime)
  // Still at work, as it is.
  expect(idles(sessions, id)).toEqual([])
  await goOn(folder, 1)
  await until(() => idles(sessions, id).length === 1)
  expect(idles(sessions, id)[0]!.outcome).toBe("succeeded")
  expect(texts(sessions, id)).toContain("done 1")
  // And the next message goes to it as it is, never to a worker started again for it.
  sessions.prompt(id, { id: "msg_update_cursor_00002", text: "and again", files: [], delivery: "steer" })
  await until(() => sentIn(folder).length === 2)
  await goOn(folder, 2)
  await until(() => idles(sessions, id).length === 2)
  expect(idles(sessions, id)[1]!.outcome).toBe("succeeded")
  expect(texts(sessions, id)).toContain("done 2")
  // The same worker, and the same agent in it, all along.
  expect(pids(folder)).toEqual([worker!])
  expect(new Set(sentIn(folder).map((sent) => sent.agentId)).size).toBe(1)
  process.kill(worker!, 0)
}, 60_000)

test("Cursor: a worker between runs goes on through a bridge update, and takes the next message as it is", async () => {
  const { folder, runtime } = folders()
  const id = await bridgeBefore(folder, runtime, "idle")
  const [worker] = pids(folder)
  process.kill(worker!, 0)

  const { sessions, idle } = await nextBridge(folder, runtime)
  expect(idle).toEqual([true])
  sessions.prompt(id, { id: "msg_update_cursor_00002", text: "and again", files: [], delivery: "steer" })
  await until(() => sentIn(folder).length === 2)
  await goOn(folder, 2)
  await until(() => idles(sessions, id).length === 2)
  expect(idles(sessions, id)[1]!.outcome).toBe("succeeded")
  expect(texts(sessions, id)).toContain("done 2")
  expect(pids(folder)).toEqual([worker!])
  expect(new Set(sentIn(folder).map((sent) => sent.agentId)).size).toBe(1)
  process.kill(worker!, 0)
}, 60_000)
