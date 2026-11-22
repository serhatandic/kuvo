import "./safety"
import { afterAll, expect, test } from "bun:test"
import { rmSync } from "node:fs"
import { ask } from "./route-answer"

// Group chats as last read (PROTOCOL.md, `kuvo/groups`): the box keeps the list the coding agents'
// bridge last answered, so a client draws your chats at once while the bridge starts again, and
// they're empty only the very first time. The engine and the bridge are stand-ins.

const { engineHandler } = await import("../src/link/server/router")
const { GROUPS_FILE } = await import("../src/link/areas/groups")

const engine = Bun.serve({ port: 0, fetch: () => Response.json({ data: [] }) })
const team = { id: "grp_team", name: "Team", members: [], discussing: false, paused: false, unread: false }
const bridge = Bun.serve({
  port: 0,
  fetch(request) {
    if (new URL(request.url).pathname !== "/api/kuvo/link/groups") return Response.json({}, { status: 404 })
    return Response.json({ groups: [team] })
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
  rmSync(GROUPS_FILE, { force: true })
})

test("the chats the bridge last listed are answered at once while it starts again", async () => {
  rmSync(GROUPS_FILE, { force: true })
  const read = await ask(handler, "GET", "kuvo/groups")
  expect(read.status).toBe(200)
  expect(read.json.groups.map((group: any) => group.name)).toEqual(["Team"])
  // The bridge restarting: the list as it was, not a wait or "aren't answering".
  bridgeRuns = false
  const kept = await ask(handler, "GET", "kuvo/groups")
  expect(kept.status).toBe(200)
  expect(kept.json.groups).toEqual([team])
  bridgeRuns = true
})
