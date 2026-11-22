// A bridge update, as a box takes one (a newer build, or a release switch): the bridge before makes
// way and ends, and the next one starts. Every session's agent goes on in the same process, never
// stopped: one at work in a command finishes its turn, shown as still at work until then, and one
// between turns takes the next message as it is. For each coding agent the box holds: Codex's
// app-server and Claude Code (through its real SDK), stand-ins for their programs. The bridge before
// runs as a process of its own (support/bridge-before.ts), as the real one does, and ends.

import "../safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { adoptHeld } from "../../src/agents/hold/adopt"
import { heldIn, holdFolder } from "../../src/agents/hold/client"
import type { Sessions } from "../../src/agents/sessions"
import type { Message } from "../../src/agents/wire"
import { bridge, goOn, standInAcp, standInClaude, standInCodex, type Kind } from "./support/update-standins"

const root = mkdtempSync(join(process.env.HOME!, "u"))
afterAll(() => rmSync(root, { recursive: true, force: true }))
let after: Sessions | undefined
async function until(condition: () => boolean) {
  while (!condition()) await new Promise((resolve) => setTimeout(resolve, 10))
}

/** Each test's box: a Kuvo Code folder of its own (a short path: a holder's socket is in it), its holders its alone. */
let kuvo = ""
let holds: string | undefined

// Holders run in sessions of their own: those a test left are stopped, by their own pid.
afterEach(() => {
  after?.closeAll()
  after = undefined
  for (const record of holds ? heldIn(holds) : []) process.kill(record.pid, "SIGTERM")
  holds = undefined
  if (kuvo) rmSync(kuvo, { recursive: true, force: true })
})

/** The bridge before, left with its agent as `scenario` says, until it's told to make way: then it does, and ends. */
async function bridgeBefore(kind: Kind, folder: string, launcher: string, scenario: "working" | "idle"): Promise<string> {
  kuvo = mkdtempSync(join(process.env.HOME!, "k"))
  holds = holdFolder(kuvo)
  expect(holds).toBeDefined()
  const child = spawn(process.execPath, [join(import.meta.dir, "support", "bridge-before.ts"), kind, folder, launcher, scenario], { stdio: ["pipe", "pipe", "inherit"], env: { ...process.env, KUVO_HOME: kuvo } })
  const ended = new Promise<number | null>((resolve) => child.once("exit", resolve))
  const session = await new Promise<string>((resolve, reject) => {
    let said = ""
    child.stdout!.setEncoding("utf8")
    child.stdout!.on("data", (chunk: string) => {
      said += chunk
      if (said.includes("\n")) resolve(JSON.parse(said.split("\n")[0]!).session)
    })
    child.once("exit", (code) => reject(new Error(`The bridge before ended (${code}) before its agent was ${scenario}.`)))
  })
  child.stdin!.write("make way\n")
  expect(await ended).toBe(0)
  return session
}

/** The next bridge, as it starts: every agent held here taken up, turns under way and those between turns. */
async function nextBridge(kind: Kind, folder: string, launcher: string) {
  const adopted = await adoptHeld(holds)
  const next = bridge(kind, folder, launcher, adopted.turns)
  after = next.sessions
  next.sessions.resumeHeld()
  const idle = adopted.idle.map(({ held, meta }) => next.sessions.adoptIdle(held, meta))
  return { ...next, idle }
}

const pids = (folder: string) => readFileSync(join(folder, "pids"), "utf8").trim().split("\n").map(Number)
const idles = (sessions: Sessions, id: string) => sessions.messages(id).filter((message): message is Extract<Message, { type: "idle" }> => message.type === "idle")
const texts = (sessions: Sessions, id: string) => sessions.messages(id).flatMap((message) => (message.type === "assistant" ? message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])) : []))
const sent = (folder: string) => (existsSync(join(folder, "sent.jsonl")) ? readFileSync(join(folder, "sent.jsonl"), "utf8") : "")

/** What the agent was sent to start a turn, as its protocol says it. */
const turnStart = (kind: Kind) => (kind === "codex" ? '"turn/start"' : kind === "claude" ? '"type":"user"' : '"session/prompt"')

for (const kind of ["codex", "claude", "antigravity", "grok"] as const) {
  const launcherIn = (folder: string) => (kind === "codex" ? standInCodex(folder) : kind === "claude" ? standInClaude(folder) : standInAcp(folder, kind))

  test(`${kind}: a turn at work in a command goes on through a bridge update, in the same process, to its end`, async () => {
    const folder = mkdtempSync(join(root, "/"))
    const launcher = launcherIn(folder)
    const id = await bridgeBefore(kind, folder, launcher, "working")
    const [agent] = pids(folder)
    // Never stopped as the bridge before went.
    process.kill(agent!, 0)

    const { sessions } = await nextBridge(kind, folder, launcher)
    // Still at work, as it is.
    expect(idles(sessions, id)).toEqual([])
    await goOn(folder, 1)
    await until(() => idles(sessions, id).length === 1)
    expect(idles(sessions, id)[0]!.outcome).toBe("succeeded")
    expect(texts(sessions, id)).toContain("done 1")
    // And the next message goes to it as it is, never to one started again for it.
    sessions.prompt(id, { id: "msg_update_turn_00002", text: "and again", files: [], delivery: "steer" })
    await until(() => sent(folder).split(turnStart(kind)).length === 3)
    await goOn(folder, 2)
    await until(() => idles(sessions, id).length === 2)
    expect(idles(sessions, id)[1]!.outcome).toBe("succeeded")
    // The same process all along: none started again.
    expect(pids(folder)).toEqual([agent!])
    process.kill(agent!, 0)
  }, 60_000)

  test(`${kind}: an agent between turns goes on through a bridge update, and takes the next message as it is`, async () => {
    const folder = mkdtempSync(join(root, "/"))
    const launcher = launcherIn(folder)
    const id = await bridgeBefore(kind, folder, launcher, "idle")
    const [agent] = pids(folder)
    process.kill(agent!, 0)

    const { sessions, idle } = await nextBridge(kind, folder, launcher)
    expect(idle).toEqual([true])
    sessions.prompt(id, { id: "msg_update_turn_00002", text: "and again", files: [], delivery: "steer" })
    await until(() => sent(folder).split(turnStart(kind)).length === 3)
    await goOn(folder, 2)
    await until(() => idles(sessions, id).length === 2)
    expect(idles(sessions, id)[1]!.outcome).toBe("succeeded")
    expect(texts(sessions, id)).toContain("done 2")
    expect(pids(folder)).toEqual([agent!])
    process.kill(agent!, 0)
  }, 60_000)
}
