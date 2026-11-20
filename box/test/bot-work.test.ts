import "./safety"
import { expect, test } from "bun:test"
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ask } from "./route-answer"

const { BOTS } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { ASKS_FILE, noteAskEvent } = await import("../src/link/glance/asked")
const { LIST_FILE } = await import("../src/link/lists/sessions")

test("recent work caps finished sessions while older working or asking sessions stay visible", async () => {
  // These routes persist facts in the suite's shared scratch home.
  const saved = [ASKS_FILE, LIST_FILE].map(file => [file, existsSync(file) ? readFileSync(file) : undefined] as const)
  const bot = "bot_worklimit"
  const dir = join(BOTS, bot)
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Worker" }))
  writeFileSync(join(dir, "session"), "ses_bot_limit\n")
  const session = (id: string, created: number, outcome?: string) => ({
    id, title: id, outcome, location: { directory: "/work" },
    time: { created, updated: created },
    metadata: { kuvo: { startedBy: "ses_bot_limit", by: { bot, name: "Worker" }, depth: 1 } },
  })
  const finished = Array.from({ length: 61 }, (_, i) => session(`ses_finished${i}`, i + 10, "succeeded"))
  const permission = { id: "per_old_recent_work", sessionID: "ses_old_asking", action: "shell", resources: ["make"] }
  const engine = Bun.serve({
    port: 0,
    fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/session/active") return Response.json({ data: { ses_old_active: true } })
      if (url.pathname === "/api/session") {
        return Response.json({ data: [...finished, session("ses_old_active", 1), session("ses_old_asking", 2)] })
      }
      if (url.pathname === "/api/session/ses_old_asking/permission") {
        return Response.json({ data: [permission] })
      }
      return Response.json({ data: [] })
    },
  })
  try {
    noteAskEvent({ type: "permission.asked", data: permission }, "engine")
    const handler = engineHandler(() => ({ port: engine.port, password: "test" }) as never, { name: "test" }, () => { throw new Error("no bridge") })
    const { status, json } = await ask(handler, "GET", `kuvo/work?bot=${bot}`)
    expect(status).toBe(200)
    const rows = json.sessions as { session: string; state: string }[]
    expect(rows.filter(row => row.state === "finished")).toHaveLength(60)
    expect(rows.find(row => row.session === "ses_old_active")?.state).toBe("working")
    expect(rows.find(row => row.session === "ses_old_asking")?.state).toBe("asking")
  } finally {
    noteAskEvent({ type: "permission.replied", data: { requestID: permission.id } }, "engine")
    for (const [file, bytes] of saved) {
      if (bytes) writeFileSync(file, bytes)
      else rmSync(file, { force: true })
    }
    engine.stop(true)
    rmSync(dir, { recursive: true, force: true })
  }
})
