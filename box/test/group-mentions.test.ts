import "./safety"
import { afterAll, expect, test } from "bun:test"
import { ask } from "./route-answer"

// What "&" offers in a group chat (PROTOCOL.md, "Mentions"): the box ranks the chat's own bots
// first, then the guests a message there can name, so every client shows the same list and only
// draws it. The engine (its sessions) and the coding agents' bridge (the chat's bots, as it lists
// them) are stand-ins.

const { engineHandler } = await import("../src/link/server/router")

// The engine's sessions, newest first: a chat bot's own conversation under another title, one by
// the name of the chat's bot on another computer (which `kuvo/named` lists by its own
// conversation there), one of a bot that left the chat, and others "&" can name.
const session = (id: string, title: string, updated: number) => ({ id, title, location: { directory: "/work/app" }, time: { created: updated, updated } })
const sessions = [
  session("ses_mira", "Mira’s desk", 100),
  session("ses_max_there", "Max", 95),
  session("ses_otto", "Otto", 90),
  session("ses_map", "Mapping work", 80),
  session("ses_notes", "Max’s notes", 70),
  session("ses_docs", "Docs", 60),
]
const engine = Bun.serve({
  port: 0,
  fetch(request) {
    const url = new URL(request.url)
    return url.pathname === "/api/session" ? Response.json({ data: sessions }) : Response.json({ message: "No such session." }, { status: 404 })
  },
})

// The chat's bots as the bridge lists them: one here, one on another computer (its conversation
// not said), and Otto, until he's taken out of the chat.
const member = (id: string, name: string, session: string, computer?: string) => ({ id, name, session, directory: computer ? "" : "/work/app", agents: false, ...(computer ? { computer } : {}) })
let members = [member("bot_mira", "Mira", "ses_mira"), member("bot_max", "Max", "", "Studio"), member("bot_otto", "Otto", "ses_otto")]
let answering = true
const bridge = Bun.serve({
  port: 0,
  fetch(request) {
    if (!answering) return Response.json({}, { status: 500 })
    if (new URL(request.url).pathname !== "/api/kuvo/link/groups") return Response.json({}, { status: 404 })
    return Response.json({ groups: [{ id: "grp_team", name: "Team", members, discussing: false, paused: false, unread: false }] })
  },
})
// (The box asks the bridge only while the process its record names is the bridge's.)
const running = Bun.spawn([process.execPath, "-e", "setInterval(() => {}, 1 << 30)", "kuvo-agents"], { stdout: "ignore", stderr: "ignore" })
let bridgeRuns = true
const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any, { name: "test" } as any, () => (bridgeRuns ? { port: bridge.port, password: "pw", pid: running.pid } : null) as any)
afterAll(() => {
  engine.stop(true)
  bridge.stop(true)
  running.kill()
})

const offered = async (query: string) => {
  const answer = await ask(handler, "GET", `kuvo/groups/grp_team/mentions?${query}`)
  expect(answer.status).toBe(200)
  return answer.json.mentions
}

test("\"&\" in a group chat offers its own bots first, then the guests a message can name, each once", async () => {
  // (Mira's conversation, and Max by his name, aren't offered again as guests.)
  const shown = await offered("q=")
  expect(shown.map((each: any) => each.title)).toEqual(["Mira", "Max", "Otto", "Mapping work", "Max’s notes", "Docs"])
  expect(shown[0]).toEqual({ text: "&Mira", title: "Mira", detail: "In this chat", kind: "bot", session: "ses_mira" })
  expect(shown[1]).toEqual({ text: "&Max", title: "Max", detail: "In this chat · on Studio", kind: "bot", session: null })
  expect(shown[3]).toEqual({ text: "&Mapping work", title: "Mapping work", detail: "app", kind: "session", session: "ses_map" })
})

test("what's typed ranks each part, the chat's bots still first", async () => {
  // Each part ranked on its own: the chat's bots, then the guests that match.
  expect((await offered("q=M")).map((each: any) => each.title)).toEqual(["Mira", "Max", "Mapping work", "Max’s notes"])
  expect((await offered("q=ma")).map((each: any) => each.title)).toEqual(["Max", "Mapping work", "Max’s notes"])
  expect((await offered("q=max%E2%80%99s")).map((each: any) => each.title)).toEqual(["Max’s notes"])
  expect(await offered("q=zed")).toEqual([])
  // A "?" typed in the name is part of it, as a client sends it unescaped.
  expect(await offered("q=?ma")).toEqual([])
  // At most what's asked, in all.
  expect((await offered("q=&limit=3")).map((each: any) => each.title)).toEqual(["Mira", "Max", "Otto"])
})

test("a bot taken out of the chat is offered as a guest, after the chat's own", async () => {
  const before = members
  members = members.filter(each => each.id !== "bot_otto")
  try {
    const shown = await offered("q=")
    expect(shown.map((each: any) => each.title)).toEqual(["Mira", "Max", "Otto", "Mapping work", "Max’s notes", "Docs"])
    expect(shown[2]).toEqual({ text: "&Otto", title: "Otto", detail: "app", kind: "session", session: "ses_otto" })
  } finally {
    members = before
  }
})

test("for no such chat, or while its bridge doesn't run or answer, it says so", async () => {
  expect((await ask(handler, "GET", "kuvo/groups/grp_none/mentions?q=")).status).toBe(404)
  try {
    answering = false
    expect((await ask(handler, "GET", "kuvo/groups/grp_team/mentions?q=")).status).toBe(503)
    answering = true
    bridgeRuns = false
    expect((await ask(handler, "GET", "kuvo/groups/grp_team/mentions?q=")).status).toBe(503)
  } finally {
    answering = true
    bridgeRuns = true
  }
})
