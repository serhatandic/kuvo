// A turn an earlier bridge left under way in a held agent (it crashed), taken up by the next: what
// was shown of it stays until the agent's lines come again, so a take-up that fails keeps it (its
// tools closed, as a crash leaves them) and stops the agent; one that goes through makes the turn
// again from those lines, once. Real sessions, store and holder; a scripted agent.

import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import type { Agent, RunInput, RunResult, Thread, TurnHooks } from "../../src/agents/agent"
import { Auto } from "../../src/agents/auto"
import { attach, heldIn, hold } from "../../src/agents/hold/client"
import { HeldGate, type HeldTurn } from "../../src/agents/hold/adopt"
import type { Roster } from "../../src/agents/roster"
import { Sessions } from "../../src/agents/sessions"
import { Store } from "../../src/agents/store"
import type { Turn } from "../../src/agents/turn"
import type { Message } from "../../src/agents/wire"

const root = mkdtempSync(join(process.env.HOME!, "r"))
const folders: string[] = []
afterAll(() => {
  for (const folder of folders) for (const record of heldIn(folder)) process.kill(record.pid, "SIGTERM")
  rmSync(root, { recursive: true, force: true })
})

/** An agent whose turn says what it's scripted to. */
function agent(script: (input: RunInput, turn: Turn, hooks: TurnHooks) => Promise<RunResult>): Agent {
  const thread: Thread = { run: script, interrupt: async () => {}, close: () => {} }
  return { id: "fake", providerID: "fake", name: "Fake", instance: { id: "default" }, methods: () => [], account: async () => ({ installed: true, signedIn: true }), models: async () => [], thread: () => thread } as unknown as Agent
}

/** A session whose turn the first bridge left under way (it said something, ran a tool, and was cut off), in a store of its own. */
async function leftUnderWay() {
  const dir = mkdtempSync(join(root, "/"))
  const store = new Store(join(dir, "sessions"))
  let cutOff!: () => void
  const said = new Promise<void>((resolve) => (cutOff = resolve))
  const first = agent(async (_input, turn) => {
    turn.startStep()
    turn.text("Before the crash.")
    turn.toolStart("call_1", "shell", { command: "make" })
    cutOff()
    return new Promise(() => {})
  })
  const sessions = new Sessions(store, { find: () => first } as unknown as Roster, new Auto(dir))
  const id = sessions.create({ model: { providerID: "fake", id: "default" }, location: { directory: dir } }).id
  sessions.prompt(id, { id: "msg_heldresume00000001", text: "build it", files: [], delivery: "steer" })
  await said
  store.flush()
  return { dir, id }
}

/** The agent's process under a holder, as the crashed bridge left it, attached by the next. */
async function heldAgent(dir: string, id: string): Promise<HeldTurn> {
  const folder = join(dir, "h")
  folders.push(folder)
  const script = join(dir, "agent.js")
  writeFileSync(script, `process.stdout.write(JSON.stringify({ said: "After the crash." }) + "\\n"); process.stdin.on("end", () => process.exit(0)); process.stdin.resume();`)
  const first = await hold([process.execPath, script], { folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
  first.detach()
  await first.closed
  const held = await attach(folder, heldIn(folder).find((each) => each.id === first.id)!, 0)
  const meta = { provider: "fake", kind: "codex" as const, turns: { t1: { session: id, from: 0 } } }
  return { held, meta, thread: "t1", note: meta.turns.t1, gate: new HeldGate(held, ["t1"]) }
}

/** The next bridge's sessions, taking the turn up with `next`, once it has ended there. */
async function takenUp(dir: string, id: string, taken: HeldTurn, next: Agent): Promise<Message[]> {
  const store = new Store(join(dir, "sessions"))
  const sessions = new Sessions(store, { find: () => next } as unknown as Roster, new Auto(dir), undefined, undefined, undefined, undefined, undefined, new Map([[id, taken]]))
  sessions.resumeHeld()
  for (;;) {
    const messages = store.get(id)!.messages
    if (messages.at(-1)?.type === "idle") return messages
    await new Promise<void>((resolve) => setImmediate(resolve))
  }
}

const textOf = (messages: Message[]) => messages.flatMap((message) => (message.type === "assistant" ? message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])) : []))

test("a take-up that fails keeps what was shown of the turn, its tool closed, and stops the agent", async () => {
  const { dir, id } = await leftUnderWay()
  const taken = await heldAgent(dir, id)
  const messages = await takenUp(dir, id, taken, agent(async () => {
    throw new Error("Claude Code isn't installed any more.")
  }))
  expect(textOf(messages)).toEqual(["Before the crash."])
  const tool = messages.flatMap((message) => (message.type === "assistant" ? message.content : [])).find((part) => part.type === "tool") as any
  expect(tool.state.status).toBe("error")
  expect(messages.at(-1)).toMatchObject({ type: "idle", outcome: "failed" })
  // (Saved so: what the next bridge reads is the same.)
  expect(textOf(new Store(join(dir, "sessions")).get(id)!.messages)).toEqual(["Before the crash."])
  expect((await taken.held.exited).code).not.toBeUndefined()
})

test("a take-up that goes through makes the turn again from the agent's lines, once", async () => {
  const { dir, id } = await leftUnderWay()
  const taken = await heldAgent(dir, id)
  const messages = await takenUp(dir, id, taken, agent(async (input, turn) => {
    turn.startStep()
    return new Promise<RunResult>((resolve) => {
      input.held!.held.onLine((line) => {
        turn.text(JSON.parse(line).said)
        turn.endStep()
        resolve({ outcome: "succeeded" })
      })
    })
  }))
  expect(textOf(messages)).toEqual(["After the crash."])
  expect(messages.filter((message) => message.type === "idle").map((message) => (message as any).outcome)).toEqual(["succeeded"])
  taken.held.stop()
  await taken.held.closed
})
