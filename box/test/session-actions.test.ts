import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ask } from "./route-answer"

// A session's menu done by its box (PROTOCOL.md, "A session's actions"): Pin, Rename, Archive and
// Delete, for the engine's sessions and every coding agent's through their bridge alike, with what
// goes with each, what Delete says it takes, and one moving to another computer, or living on
// another now, refused in words. The engine and the bridge are stand-ins that keep what a PATCH
// sends whole, as the real ones do.

const { engineHandler } = await import("../src/link/server/router")

const kuvo = process.env.KUVO_HOME!
const root = mkdtempSync(join(kuvo, "session-actions-"))
const wakes = join(kuvo, "wakes")

type Session = { id: string; title?: string; location: { directory: string }; time: { created: number; updated: number }; metadata?: Record<string, any>; parentID?: string }

/** A place keeping sessions as the engine does: what it deleted, and what it was asked to interrupt. */
function place(name: string) {
  const sessions = new Map<string, Session>()
  const deleted: string[] = []
  const interrupted: string[] = []
  const asking = new Set<string>()
  const server = Bun.serve({
    port: 0,
    async fetch(request) {
      const url = new URL(request.url)
      if (url.pathname === "/api/session") return Response.json({ data: [...sessions.values()] })
      if (url.pathname === "/api/session/active") return Response.json({ data: Object.fromEntries([...asking].map(id => [id, { type: "busy" }])) })
      const [, id, more] = /^\/api\/session\/([^/]+)(?:\/(.+))?$/.exec(url.pathname) ?? []
      const session = id ? sessions.get(decodeURIComponent(id)) : undefined
      if (more === "permission") return Response.json({ data: session && asking.has(session.id) ? [{ id: "per_1", sessionID: session.id, action: "shell", resources: ["git push"] }] : [] })
      if (more === "form" || more === "inbox") return Response.json({ data: [] })
      if (more === "interrupt") { if (session) interrupted.push(session.id); return Response.json({ data: true }) }
      if (more) return Response.json({ data: [] })
      if (!session) return Response.json({ message: `No such session on the ${name}.` }, { status: 404 })
      if (request.method === "DELETE") { sessions.delete(session.id); deleted.push(session.id); return Response.json({ data: true }) }
      if (request.method === "PATCH") {
        // (Whole, as the engine keeps what's sent.)
        const body = await request.json() as Partial<Session>
        if (body.title !== undefined) session.title = body.title
        if (body.metadata !== undefined) session.metadata = body.metadata
        session.time.updated += 1
      }
      return Response.json({ data: session })
    },
  })
  return { server, sessions, deleted, interrupted, asking }
}

const engine = place("engine")
const bridge = place("bridge")
// The bridge as the box knows it runs: a process of that name.
const bridgeProcess = spawn(process.execPath, ["-e", "setInterval(() => {}, 1e9)", "kuvo-agents"], { stdio: "ignore" })
const handler = engineHandler(() => ({ port: engine.server.port, password: "pw" }) as any, { name: "test" }, () => ({ port: bridge.server.port, password: "pw", pid: bridgeProcess.pid }) as any)
afterAll(() => {
  engine.server.stop(true)
  bridge.server.stop(true)
  bridgeProcess.kill()
  rmSync(root, { recursive: true, force: true })
  rmSync(wakes, { recursive: true, force: true })
  rmSync(join(kuvo, "run", "moves"), { recursive: true, force: true })
  rmSync(join(kuvo, "box", "worktrees.json"), { force: true })
})

const AGENTS = ["claude-code", "codex-cli", "cursor", "grok-cli", "antigravity"]
let clock = 1000
const session = (id: string, title: string, directory = root, metadata?: Record<string, any>): Session => ({ id, title, location: { directory }, time: { created: ++clock, updated: clock }, ...(metadata ? { metadata } : {}) })

beforeEach(() => {
  for (const each of [engine, bridge]) { each.sessions.clear(); each.deleted.length = 0; each.interrupted.length = 0; each.asking.clear() }
  rmSync(wakes, { recursive: true, force: true })
  rmSync(join(kuvo, "run", "moves"), { recursive: true, force: true })
  rmSync(join(kuvo, "box", "worktrees.json"), { force: true })
  engine.sessions.set("ses_plan", session("ses_plan", "Plan the trip", root, { kuvo: { filed: { folder: root, project: root } }, other: "kept" }))
  for (const agent of AGENTS) bridge.sessions.set(`ses_${agent.replace(/-/g, "")}`, session(`ses_${agent.replace(/-/g, "")}`, `With ${agent}`, root, { kuvoAgent: agent }))
})

/** A wake-up folder for `id`, as kuvo-wake makes one. */
function wakeFolder(id: string): string {
  const folder = join(wakes, id)
  mkdirSync(join(folder, "wakes"), { recursive: true })
  writeFileSync(join(folder, "session"), id)
  writeFileSync(join(folder, "wakes", "4000000000-abcd"), "Check the build")
  return folder
}

test("Pin and Unpin write the box's note and keep every other note", async () => {
  const pinned = await ask(handler, "POST", "kuvo/sessions/ses_plan/pin", { pinned: true })
  expect(pinned.status).toBe(200)
  expect(pinned.json.info.metadata).toEqual({ "kuvo.pinned": true, kuvo: { filed: { folder: root, project: root } }, other: "kept" })
  expect(engine.sessions.get("ses_plan")!.metadata!["kuvo.pinned"]).toBe(true)
  expect((await ask(handler, "POST", "kuvo/sessions/ses_plan/pin", { pinned: false })).json.info.metadata["kuvo.pinned"]).toBe(false)
  expect(engine.sessions.get("ses_plan")!.metadata!.other).toBe("kept")
  expect((await ask(handler, "POST", "kuvo/sessions/ses_plan/pin", {})).status).toBe(400)
})

test("every coding agent's session is archived and brought back through their bridge, by the same note", async () => {
  for (const agent of AGENTS) {
    const id = `ses_${agent.replace(/-/g, "")}`
    const folder = wakeFolder(id)
    expect((await ask(handler, "POST", `kuvo/sessions/${id}/archive`, { archived: true })).status).toBe(200)
    expect(bridge.sessions.get(id)!.metadata).toEqual({ kuvoAgent: agent, "kuvo.archived": true })
    // Its wake-ups wait while it's archived.
    expect(readFileSync(join(folder, "paused"), "utf8")).toBe("archived")
    const back = await ask(handler, "POST", `kuvo/sessions/${id}/archive`, { archived: false })
    expect(back.json.info.metadata).toEqual({ kuvoAgent: agent, "kuvo.archived": false })
    expect(existsSync(join(folder, "paused"))).toBe(false)
  }
  expect(engine.sessions.get("ses_plan")!.metadata!["kuvo.archived"]).toBeUndefined()
})

test("archived while it waits on you, its run stops; brought back, an older client's mark goes too", async () => {
  engine.asking.add("ses_plan")
  await ask(handler, "POST", "kuvo/sessions/ses_plan/archive", { archived: true })
  expect(engine.interrupted).toContain("ses_plan")
  // One an older client archived inside its `kuvo` notes.
  engine.sessions.set("ses_old", session("ses_old", "Old", root, { kuvo: { archived: true, filed: { folder: root, project: root } } }))
  const back = await ask(handler, "POST", "kuvo/sessions/ses_old/archive", { archived: false })
  expect(back.json.info.metadata).toEqual({ "kuvo.archived": false, kuvo: { filed: { folder: root, project: root } } })
})

test("Rename takes the name as written, trimmed; no name is refused", async () => {
  const renamed = await ask(handler, "POST", "kuvo/sessions/ses_codexcli/rename", { title: "  Ship it  " })
  expect(renamed.json.info.title).toBe("Ship it")
  expect(bridge.sessions.get("ses_codexcli")!.title).toBe("Ship it")
  expect((await ask(handler, "POST", "kuvo/sessions/ses_codexcli/rename", { title: "   " })).json).toEqual({ message: "Give it a name." })
})

test("Delete deletes it where it runs, its wake-ups with it; one gone already is done", async () => {
  const folder = wakeFolder("ses_plan")
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_plan")).json).toEqual({ session: "ses_plan" })
  expect(engine.deleted).toEqual(["ses_plan"])
  expect(existsSync(folder)).toBe(false)
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_cursor")).status).toBe(200)
  expect(bridge.deleted).toEqual(["ses_cursor"])
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_plan")).json).toEqual({ session: "ses_plan" })
  expect((await ask(handler, "POST", "kuvo/sessions/ses_plan/pin", { pinned: true })).status).toBe(404)
})

test("the menu says what can be done, and what Delete takes with it", async () => {
  const plan = await ask(handler, "GET", "kuvo/sessions/ses_plan/actions")
  expect(plan.json).toEqual({
    session: "ses_plan",
    actions: [
      { id: "pin", label: "Pin" },
      { id: "rename", label: "Rename" },
      { id: "archive", label: "Archive" },
      { id: "delete", label: "Delete", destructive: true, confirm: { title: "Delete “Plan the trip”?", message: "This can’t be undone.", label: "Delete" } },
    ],
  })
  await ask(handler, "POST", "kuvo/sessions/ses_plan/archive", { archived: true })
  expect((await ask(handler, "GET", "kuvo/sessions/ses_plan/actions")).json.actions.map((each: any) => each.id)).toEqual(["unarchive", "delete"])

  // In a worktree of its own: it goes too, with what isn't committed there.
  const tree = join(root, "tree")
  mkdirSync(tree)
  spawnSync("git", ["init", "-q", tree])
  mkdirSync(join(kuvo, "box"), { recursive: true })
  writeFileSync(join(kuvo, "box", "worktrees.json"), JSON.stringify({ worktrees: [{ project: root, root: tree, folder: tree, branch: "trip" }] }))
  engine.sessions.set("ses_tree", session("ses_tree", "In a tree", tree))
  const confirm = async () => (await ask(handler, "GET", "kuvo/sessions/ses_tree/actions")).json.actions.find((each: any) => each.id === "delete").confirm.message
  expect(await confirm()).toBe("Its worktree is deleted too.")
  // Clean now, but at work there: it may still write some.
  engine.asking.add("ses_tree")
  expect(await confirm()).toBe("Its worktree and changes that weren’t committed are deleted too.")
  engine.asking.clear()
  writeFileSync(join(tree, "notes.md"), "draft")
  expect(await confirm()).toBe("Its worktree and changes that weren’t committed are deleted too.")
  // An archived one left in it keeps what isn't committed; an open one keeps the worktree.
  engine.sessions.set("ses_kept", session("ses_kept", "Kept", tree, { "kuvo.archived": true }))
  expect(await confirm()).toBe("Its worktree is deleted too.")
  engine.sessions.set("ses_open", session("ses_open", "Open", tree))
  expect(await confirm()).toBe("This can’t be undone.")
})

test("what Delete says counts only the conversations a worktree's tidying counts: a coding agent's engine run there isn't one", async () => {
  const tree = join(root, "agent-tree")
  mkdirSync(tree)
  spawnSync("git", ["init", "-q", tree])
  writeFileSync(join(tree, "draft.md"), "not committed")
  mkdirSync(join(kuvo, "box"), { recursive: true })
  writeFileSync(join(kuvo, "box", "worktrees.json"), JSON.stringify({ worktrees: [{ project: root, root: tree, folder: tree, branch: "agent" }] }))
  bridge.sessions.set("ses_agent", session("ses_agent", "With Codex", tree, { kuvoAgent: "codex-cli" }))
  // Auto's engine run for it, in the same folder, and a group chat's seat: not conversations.
  engine.sessions.set("ses_run", session("ses_run", "Engine run", tree, { kuvo: { engineFor: "ses_agent" } }))
  engine.sessions.set("ses_seat", session("ses_seat", "Seat", tree, { kuvo: { seat: "grp_1" } }))
  const actions = (await ask(handler, "GET", "kuvo/sessions/ses_agent/actions")).json.actions
  expect(actions.find((each: any) => each.id === "delete").confirm.message).toBe("Its worktree and changes that weren’t committed are deleted too.")
})

test("a removed worktree's kept changes are said to go with the last conversation that could bring them back", async () => {
  const gone = join(root, "removed-tree")
  mkdirSync(join(kuvo, "box"), { recursive: true })
  writeFileSync(join(kuvo, "box", "worktrees.json"), JSON.stringify({ worktrees: [], restorable: [{ project: root, root: gone, folder: gone, branch: "old", head: "abc", snapshot: "def" }] }))
  engine.sessions.set("ses_last", session("ses_last", "Last", gone, { "kuvo.archived": true }))
  const confirm = async () => (await ask(handler, "GET", "kuvo/sessions/ses_last/actions")).json.actions.find((each: any) => each.id === "delete").confirm.message
  expect(await confirm()).toBe("The changes kept to restore its worktree are deleted too.")
  engine.sessions.set("ses_also", session("ses_also", "Also", gone, { "kuvo.archived": true }))
  expect(await confirm()).toBe("This can’t be undone.")
})

test("one the engine archived itself isn't offered Unarchive, and is refused it in words", async () => {
  engine.sessions.set("ses_old", { ...session("ses_old", "Old", root), time: { created: 1, updated: 2, archived: 3 } as any })
  expect((await ask(handler, "GET", "kuvo/sessions/ses_old/actions")).json.actions.map((each: any) => each.id)).toEqual(["delete"])
  const refused = await ask(handler, "POST", "kuvo/sessions/ses_old/archive", { archived: false })
  expect(refused.status).toBe(409)
  expect(engine.sessions.get("ses_old")!.metadata).toBeUndefined()
})

test("while the coding agents' bridge starts again, one of theirs isn't said gone, and a malformed id is refused", async () => {
  const restarting = engineHandler(() => ({ port: engine.server.port, password: "pw" }) as any, { name: "test" }, () => ({ port: bridge.server.port, password: "pw", pid: 999999999 }) as any)
  const said = { status: 503, json: { message: "The coding agents on this computer are starting. Try again in a moment." } }
  expect(await ask(restarting, "DELETE", "kuvo/sessions/ses_codexcli")).toEqual(said)
  expect(await ask(restarting, "POST", "kuvo/sessions/ses_codexcli/pin", { pinned: true })).toEqual(said)
  expect(bridge.deleted).toEqual([])
  expect((await ask(handler, "GET", "kuvo/sessions/ses%E0/actions")).status).toBe(400)
})

test("one moving to another computer is refused in words, and its menu says why it's held", async () => {
  mkdirSync(join(kuvo, "run", "moves"), { recursive: true })
  writeFileSync(join(kuvo, "run", "moves", "mov_trip.json"), JSON.stringify({ id: "mov_trip", kind: "chat", thing: "ses_plan", name: "Plan the trip", from: "a", to: "b", role: "from", stage: "copying", sessions: ["ses_plan"], peer: "peerkey", tries: 0, started: Date.now() }))
  const moving = "It’s moving to another computer. Try again once it’s there."
  for (const [method, route, body] of [["POST", "pin", { pinned: true }], ["POST", "archive", { archived: true }], ["POST", "rename", { title: "Elsewhere" }], ["DELETE", "", undefined]] as const) {
    const refused = await ask(handler, method, `kuvo/sessions/ses_plan${route ? `/${route}` : ""}`, body)
    expect(refused).toEqual({ status: 409, json: { message: moving } })
  }
  expect(engine.sessions.get("ses_plan")!.metadata!["kuvo.pinned"]).toBeUndefined()
  expect(engine.deleted).toEqual([])
  expect((await ask(handler, "GET", "kuvo/sessions/ses_plan/actions")).json.held).toBe(moving)
})

test("one that lives on another computer now is refused with where, for its box to be asked", async () => {
  mkdirSync(join(kuvo, "run", "moves"), { recursive: true })
  writeFileSync(join(kuvo, "run", "moves", "moved.json"), JSON.stringify([{ session: "ses_gone", key: "otherbox", at: Date.now() }]))
  const refused = await ask(handler, "POST", "kuvo/sessions/ses_gone/archive", { archived: true })
  expect(refused).toEqual({ status: 409, json: { message: "It’s on another of your computers now. Change it there.", computer: "otherbox" } })
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_gone")).status).toBe(409)
})

test("a subagent's session, a group chat's seat and a hidden run are refused in words: they go with what they're part of", async () => {
  engine.sessions.set("ses_sub", { ...session("ses_sub", "Sub", root), parentID: "ses_plan" })
  engine.sessions.set("ses_seat", session("ses_seat", "Seat", root, { kuvo: { seat: "grp_1" } }))
  engine.sessions.set("ses_run", session("ses_run", "Run", root, { kuvo: { engineFor: "ses_codexcli" } }))
  engine.sessions.set("ses_part", session("ses_part", "Part", root, { kuvo: { reviewPart: "rev_1" } }))
  for (const id of ["ses_sub", "ses_seat", "ses_run", "ses_part"]) {
    for (const [method, route, body] of [["DELETE", "", undefined], ["POST", "/archive", { archived: true }], ["GET", "/actions", undefined]] as const) {
      const refused = await ask(handler, method, `kuvo/sessions/${id}${route}`, body)
      expect(refused.status).toBe(409)
      expect(refused.json.message).toMatch(/goes with that one|change the group chat/)
    }
  }
  expect(engine.deleted).toEqual([])
})

test("Delete All asks once for the whole set, saying what goes with their worktrees", async () => {
  engine.sessions.set("ses_a1", session("ses_a1", "One", root, { "kuvo.archived": true }))
  engine.sessions.set("ses_a2", session("ses_a2", "Two", root, { "kuvo.archived": true }))
  const plain = await ask(handler, "POST", "kuvo/sessions/deletes", { sessions: ["ses_a1", "ses_a2", "ses_unknown"] })
  expect(plain.json).toEqual({ title: "Delete 2 sessions?", message: "Their messages and undo snapshots go too. This can’t be undone.", label: "Delete" })
  // Both of a removed worktree's last conversations: its kept changes go with them, though either
  // alone leaves the other to bring them back.
  const gone = join(root, "removed-all")
  mkdirSync(join(kuvo, "box"), { recursive: true })
  writeFileSync(join(kuvo, "box", "worktrees.json"), JSON.stringify({ worktrees: [], restorable: [{ project: root, root: gone, folder: gone, branch: "old", head: "abc", snapshot: "def" }] }))
  engine.sessions.set("ses_g1", session("ses_g1", "G1", gone, { "kuvo.archived": true }))
  engine.sessions.set("ses_g2", session("ses_g2", "G2", gone, { "kuvo.archived": true }))
  expect((await ask(handler, "GET", "kuvo/sessions/ses_g1/actions")).json.actions.find((each: any) => each.id === "delete").confirm.message).toBe("This can’t be undone.")
  const both = await ask(handler, "POST", "kuvo/sessions/deletes", { sessions: ["ses_g1", "ses_g2"] })
  expect(both.json.message).toBe("Their messages and undo snapshots go too. So are the changes kept to restore their worktrees. This can’t be undone.")
  expect((await ask(handler, "POST", "kuvo/sessions/deletes", { sessions: [] })).status).toBe(400)
})
