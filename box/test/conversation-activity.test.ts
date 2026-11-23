import "./safety"
import { expect, test } from "bun:test"
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ask } from "./route-answer"

const { BOTS } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { LIST_FILE, keepList, listNow, listPlaceHeard, noteListEvent, workOut } = await import("../src/link/lists/sessions")

// What a conversation's Outputs popover lists under Sessions and Subagents: what works for it
// besides its own turn, never the conversation itself, its subagents told apart from sessions.
test("a conversation's activity lists what works under it, not itself, and says which are subagents", async () => {
  const saved = existsSync(LIST_FILE) ? readFileSync(LIST_FILE) : undefined
  rmSync(LIST_FILE, { force: true })
  const at = (id: string, extra: Record<string, unknown> = {}) => ({
    id, title: `Title of ${id}`, location: { directory: "/work" }, time: { created: 1, updated: 1 }, ...extra,
  })
  const engine = Bun.serve({
    port: 0,
    fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/session/active") return Response.json({ data: { ses_root: true, ses_sub: true, ses_started: true } })
      if (url.pathname === "/api/session") {
        return Response.json({ data: [
          at("ses_root"),
          at("ses_sub", { parentID: "ses_root" }),
          at("ses_started", { metadata: { kuvo: { startedBy: "ses_root", line: ["ses_root"], depth: 1 } } }),
        ] })
      }
      return Response.json({ data: [] })
    },
  })
  try {
    const readEngine = () => ({ port: engine.port, password: "test" }) as never
    expect(await listNow({ readEngine, bridge: () => null })).not.toBeNull()
    const handler = engineHandler(readEngine, { name: "test" }, () => { throw new Error("no bridge") })
    const { status, json } = await ask(handler, "GET", "kuvo/work/background/ses_root")
    expect(status).toBe(200)
    const activity = json.activity as { working: number; sessions: { session: string; subagent?: boolean }[] }
    expect(activity.sessions.map(row => row.session).sort()).toEqual(["ses_started", "ses_sub"])
    expect(activity.working).toBe(2)
    expect(activity.sessions.find(row => row.session === "ses_sub")?.subagent).toBe(true)
    expect(activity.sessions.find(row => row.session === "ses_started")?.subagent).toBeUndefined()
  } finally {
    if (saved) writeFileSync(LIST_FILE, saved)
    else rmSync(LIST_FILE, { force: true })
    engine.stop(true)
  }
})

test("a bot-started session disappears from active work when its bridge becomes idle", async () => {
  const saved = existsSync(LIST_FILE) ? readFileSync(LIST_FILE) : undefined
  const bot = "bot_activity"
  const folder = join(BOTS, bot)
  const root = { id: "ses_bot", location: { directory: folder }, time: { created: 1, updated: 1 } }
  const child = { ...root, id: "ses_bot_work", location: { directory: "/work" }, metadata: { kuvo: { by: { bot, name: "Coordinator" }, startedBy: root.id, line: [root.id] } } }
  let ended = false
  const engine = Bun.serve({
    port: 0,
    fetch(request) {
      return Response.json({ data: new URL(request.url).pathname === "/api/session" ? [root] : [] })
    },
  })
  const bridge = Bun.serve({
    port: 0,
    fetch(request) {
      const route = new URL(request.url).pathname
      // A snapshot taken after execution ended still saw the bridge keeping the turn's files.
      if (route === "/api/session/active") return Response.json({ data: ended ? [] : [child.id] })
      if (route === "/api/session") return Response.json({ data: [child] })
      if (route === "/api/session/" + child.id) return Response.json({
        data: ended ? { ...child, outcome: "succeeded", time: { created: 1, updated: 2, idle: 2 } } : child,
      })
      return Response.json({ data: [] })
    },
  })
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Coordinator" }))
  writeFileSync(join(folder, "session"), root.id)
  const target = { port: bridge.port!, password: "test" }
  const readEngine = () => ({ port: engine.port, password: "test" }) as never
  const handler = engineHandler(readEngine)
  const read = () => ask(handler, "GET", "kuvo/work/background/" + root.id)
  const readBotWork = () => ask(handler, "GET", "kuvo/work?bot=" + bot)
  const stop = keepList()
  try {
    await listPlaceHeard("engine", readEngine())
    await listPlaceHeard("bridge", target)
    expect((await read()).json.activity.sessions.map((row: { session: string }) => row.session)).toEqual([child.id])
    ended = true
    await noteListEvent("bridge", target, { type: "session.idle", data: { sessionID: child.id } })
    workOut()
    expect((await read()).json.activity).toEqual({ working: 0, asking: 0, sessions: [] })
    expect((await readBotWork()).json.sessions).toMatchObject([{ session: child.id, state: "finished" }])
  } finally {
    stop()
    engine.stop(true)
    bridge.stop(true)
    rmSync(folder, { recursive: true, force: true })
    if (saved) writeFileSync(LIST_FILE, saved)
    else rmSync(LIST_FILE, { force: true })
  }
})
