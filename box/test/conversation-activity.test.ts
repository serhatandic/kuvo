import "./safety"
import { expect, test } from "bun:test"
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { ask } from "./route-answer"

const { engineHandler } = await import("../src/link/server/router")
const { LIST_FILE } = await import("../src/link/lists/sessions")

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
    const handler = engineHandler(() => ({ port: engine.port, password: "test" }) as never, { name: "test" }, () => { throw new Error("no bridge") })
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
