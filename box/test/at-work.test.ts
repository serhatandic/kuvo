import "./safety"
import { afterAll, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ask, follow } from "./route-answer"

// What's at work in Kuvo on this computer, for Kuvo Helper's menu bar item (PROTOCOL.md, "In the
// menu bar", `kuvo/box/at-work`): the box's own facts, from its session list, the work its sessions
// left running and its plan limits' rings, which the helper draws as they are; and stopping it
// until it next starts (`kuvo/box/stop`), which only this computer's own clients may ask.

const { atWorkOf, ringsOf } = await import("../src/link/at-work")
const { BOTS } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")

const facts = (more: Record<string, unknown>) => ({ kind: "project", directory: "/work/site", created: 1, updated: 1, ...more })
const kept = (sessions: Record<string, Record<string, unknown>>, needsYou: unknown[] = []) => ({
  epoch: "e", seq: 1, floor: 0, gone: {}, projects: {}, needsYou,
  sessions: Object.fromEntries(Object.entries(sessions).map(([id, each]) => [id, { seq: 1, facts: facts(each) }])),
}) as any

// A bot that lives here, as its folder says it: what the item names a session working for it by.
mkdirSync(join(BOTS, "bot_mira"), { recursive: true })
writeFileSync(join(BOTS, "bot_mira", "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_mira", name: "Mira" }))
writeFileSync(join(BOTS, "bot_mira", "session"), "ses_mira\n")
afterAll(() => rmSync(join(BOTS, "bot_mira"), { recursive: true, force: true }))

test("what's at work: the sessions working here, the latest first, with the bot each works for; those waiting on you with what they ask", () => {
  const list = kept({
    ses_old: { title: "Nightly dependency bump", running: true, updated: 10 },
    ses_new: { title: "Fix sign-up\n form validation", running: true, updated: 20, bot: "bot_mira" },
    ses_ask: { title: "Deploy preview to staging", running: true, asked: 30 },
    // Not this box's to say: archived, another computer's copy, a subagent's, one kept out of sight.
    ses_archived: { title: "Old", running: true, archived: true },
    ses_away: { title: "Elsewhere", running: true, away: true },
    ses_sub: { kind: "subagent", title: "Sub", running: true },
    ses_hidden: { kind: "hidden", title: "Hidden", running: true },
    ses_done: { title: "Finished" },
  }, [{ session: "ses_ask", asks: { kind: "permission", summary: "asks to run a command" } }])
  const said = atWorkOf(list, [], {})
  expect(said.working).toBe(2)
  expect(said.sessions).toEqual([{ id: "ses_new", title: "Fix sign-up form validation", bot: "Mira" }, { id: "ses_old", title: "Nightly dependency bump" }])
  expect(said.asking).toEqual([{ id: "ses_ask", title: "Deploy preview to staging", asks: "asks to run a command" }])
  expect(said.waiting).toEqual({ sessions: 1, bots: 0, queue: 0 })
  expect(said.busy).toBe(true)
  // A Linux or test box has no menu bar of its own.
  expect(said.applies).toBe(false)
})

test("work a session left running counts as something that would stop; nothing at all is nothing", () => {
  const idle = kept({ ses_1: { title: "Finished" } })
  expect(atWorkOf(idle, [], {})).toMatchObject({ working: 0, sessions: [], asking: [], background: 0, busy: false })
  const left = atWorkOf(idle, [], { ses_1: { commands: 1, subagents: 0, tasks: 0, since: 1 } })
  expect(left).toMatchObject({ working: 0, background: 1, busy: true })
  // (One at work in its turn too is counted once, as at work.)
  const both = atWorkOf(kept({ ses_1: { running: true } }), [], { ses_1: { commands: 1, subagents: 0, tasks: 0, since: 1 } })
  expect(both).toMatchObject({ working: 1, background: 0 })
  // The engine not read yet: nothing is known to work.
  expect(atWorkOf(null, [], {})).toMatchObject({ working: 0, busy: false, waiting: { sessions: 0, bots: 0, queue: 0 } })
})

test("only the first twenty are named; every one is counted", () => {
  const many = Object.fromEntries(Array.from({ length: 25 }, (_, n) => [`ses_${n}`, { title: `Session ${n}`, running: true, updated: n }]))
  const said = atWorkOf(kept(many), [], {})
  expect(said.working).toBe(25)
  expect(said.sessions.length).toBe(20)
  expect(said.sessions[0]!.id).toBe("ses_24")
})

// The bridge, as it answers its limits: each account's row with its rings.
const rows = [
  { service: "claude", title: "Claude", rings: { short: { window: "five_hour", used: 0.62, level: "calm" }, long: { window: "week", used: 0.34, level: "calm" } } },
  { service: "codex", title: "Codex · Work", rings: { short: { window: "five_hour", used: 1.4, level: "over" } } },
  { service: "ollama", title: "Ollama", rings: {} },
  // One this can't read is left out, and a ring it can't read leaves its account the other.
  { service: "grok" },
  { service: "gemini", title: "Gemini", rings: { short: { used: "most" }, long: { window: "w", used: 0.1, level: "dire" } } },
]
const bridge = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: request => new URL(request.url).pathname === "/api/kuvo/limits" ? Response.json({ rows }) : new Response("no", { status: 404 }) })
const bridgeProcess = spawn(process.execPath, ["-e", "setInterval(() => {}, 1e9)", "kuvo-agents"], { stdio: "ignore" })
afterAll(() => { bridge.stop(true); bridgeProcess.kill() })

test("the rings are each account's, as the bridge says them, in its order", async () => {
  expect(await ringsOf({ port: bridge.port, password: "test" } as any)).toEqual([
    { service: "claude", title: "Claude", short: { used: 0.62, level: "calm" }, long: { used: 0.34, level: "calm" } },
    { service: "codex", title: "Codex · Work", short: { used: 1, level: "over" } },
    { service: "ollama", title: "Ollama" },
    { service: "gemini", title: "Gemini", long: { used: 0.1, level: "dire" } },
  ])
  // The bridge not running: none.
  expect(await ringsOf(null)).toEqual([])
})

const noEngine = () => { throw new Error("No engine") }
const handler = engineHandler(noEngine as any, { name: "test" }, () => ({ port: bridge.port, password: "test", pid: bridgeProcess.pid }) as any)

test("kuvo/box/at-work answers in its shape, and its events say it again as it changes", async () => {
  const said = await ask(handler, "GET", "kuvo/box/at-work")
  expect(said.status).toBe(200)
  expect(said.json.rings.map((each: any) => each.service)).toEqual(["claude", "codex", "ollama", "gemini"])
  const events = follow(handler, "kuvo/box/at-work/events")
  try {
    expect((await events.next()).busy).toBe(false)
  } finally {
    events.stop()
  }
})

test("kuvo/box/stop: only from this computer's own clients (over its server, with its password), and only where the helper starts it again", async () => {
  // A phone, another computer, `box route`: never.
  expect(await ask(handler, "POST", "kuvo/box/stop", {})).toEqual({ status: 403, json: { message: "Kuvo on this Mac is stopped only on this Mac." } })
  // Over its own server, but a box that isn't a Mac's own (nothing would start it again at login).
  let status = 0
  for await (const piece of handler({ method: "POST", path: "kuvo/box/stop", headers: {}, body: Buffer.from("{}"), served: true, fromApp: true } as any, undefined)) {
    if ("head" in piece) status = piece.head.status
  }
  expect(status).toBe(409)
})
