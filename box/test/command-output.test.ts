import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { ask, follow } from "./route-answer"

// What a command a session left running printed (`kuvo/work/background/<session>/commands/<id>/output`,
// PROTOCOL.md "Work › Background work"), as a client's Output shows it: the end of it, cut on the box,
// now and followed. The box's real code; the engine a stand-in that keeps its commands as the real one
// does (each known only in its own folder), and the pause between reads the test's own.

const { engineHandler } = await import("../src/link/server/router")
const { WORK_BACKGROUND } = await import("../src/link/work/background")
const { commandOf, followOutput, outputReader } = await import("../src/link/work/command-output")

// The engine's commands: where each runs, whether it still does, and what it printed.
let jobs: Map<string, { folder: string; status: string; printed: string }>
let failing = false
beforeEach(() => {
  jobs = new Map()
  failing = false
})
const engine = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: request => {
  const url = new URL(request.url)
  const asked = /^\/api\/shell\/([^/]+)(\/output)?$/.exec(url.pathname)
  if (!asked) return Response.json({ message: "Not found" }, { status: 404 })
  if (failing) return Response.json({ message: "Busy." }, { status: 500 })
  const job = jobs.get(asked[1]!)
  if (!job || job.folder !== decodeURIComponent(request.headers.get("x-opencode-directory") ?? "")) return Response.json({ message: `Shell command not found: ${asked[1]}` }, { status: 404 })
  if (!asked[2]) return Response.json({ data: { id: asked[1], status: job.status } })
  const bytes = Buffer.from(job.printed)
  const cursor = Number(url.searchParams.get("cursor") ?? 0)
  const limit = Number(url.searchParams.get("limit") ?? bytes.length)
  return Response.json({ data: { output: bytes.subarray(cursor, cursor + limit).toString("utf8"), cursor: Math.min(bytes.length, cursor + limit), size: bytes.length } })
} })
const target = { port: engine.port, password: "test" } as any
afterAll(() => engine.stop(true))

const folder = "/work/site"
const scratch = mkdtempSync(join(tmpdir(), "kuvo-command-output-"))
afterAll(() => rmSync(scratch, { recursive: true, force: true }))
/** What the box keeps of what sessions left running (`run/work/background.json`), as its watcher writes it. */
const keep = (file: string, place: "engine" | "bridge", session: string, commands: object[]) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, JSON.stringify({ [place]: { sessions: { [session]: { commands, subagents: [], tasks: [] } }, folders: [] } }))
}

/** The pause between reads, ended by the test: `tick()` once the follower is waiting; `waiting()`,
 * once it's waiting again after that. */
function pauses() {
  let resume = () => {}
  let reached = Promise.withResolvers<void>()
  return {
    wait: () => new Promise<void>(done => {
      resume = done
      reached.resolve()
    }),
    waiting: () => reached.promise,
    async tick() {
      await reached.promise
      reached = Promise.withResolvers<void>()
      resume()
    },
  }
}
/** The next line a follower sends, null once it's done. */
const lineOf = async (pending: Promise<IteratorResult<any>>) => {
  const { value, done } = await pending
  return done ? null : JSON.parse(Buffer.from(value.body).toString())
}

test("the end of what it printed, from its first whole line, read where it runs; one the box doesn't know, and an engine that's down, said so", async () => {
  // (The box's own file, as it was before, afterwards.)
  const before = existsSync(WORK_BACKGROUND) ? readFileSync(WORK_BACKGROUND) : null
  try {
    const printed = Array.from({ length: 3_000 }, (_, at) => `line ${at}`).join("\n") + "\n"
    jobs.set("sh_build", { folder, status: "running", printed })
    keep(WORK_BACKGROUND, "engine", "ses_site", [{ id: "sh_build", command: "npm run build", started: 5, folder, ports: [5173] }])
    const handler = engineHandler((() => target) as any, { name: "test" } as any, (() => { throw new Error("No bridge") }) as any)
    const said = await ask(handler, "GET", "kuvo/work/background/ses_site/commands/sh_build/output")
    expect(said.status).toBe(200)
    expect(said.json).toMatchObject({ command: { id: "sh_build", command: "npm run build", started: 5, folder }, cut: true, state: "running" })
    const output: string = said.json.output
    expect(Buffer.byteLength(output)).toBeLessThanOrEqual(24_000)
    expect(output.endsWith("line 2999")).toBe(true)
    // From a whole line: the one cut in two is left out.
    expect(printed[printed.lastIndexOf(output) - 1]).toBe("\n")
    expect(output.split("\n")[0]).toMatch(/^line \d+$/)

    expect((await ask(handler, "GET", "kuvo/work/background/ses_site/commands/sh_other/output")).json).toEqual({ command: null, output: "", cut: false, state: "ended" })
    const down = engineHandler((() => { throw new Error("No engine") }) as any, { name: "test" } as any, (() => { throw new Error("No bridge") }) as any)
    expect((await ask(down, "GET", "kuvo/work/background/ses_site/commands/sh_build/output")).json).toMatchObject({ output: "", cut: false, state: "unread" })
    // Followed, one it doesn't know is one line, and the end.
    const events = follow(handler, "kuvo/work/background/ses_site/commands/sh_other/output/events")
    expect(await events.next()).toEqual({ command: null, output: "", cut: false, state: "ended" })
    await expect(events.next()).rejects.toThrow("The stream ended.")
  } finally {
    if (before) writeFileSync(WORK_BACKGROUND, before)
    else rmSync(WORK_BACKGROUND, { force: true })
  }
})

test("followed: a line as it prints more, none while nothing changes, and once it ended its last output, and the end", async () => {
  const file = join(scratch, "growing.json")
  jobs.set("sh_test", { folder, status: "running", printed: "Compiling…\n" })
  keep(file, "engine", "ses_site", [{ id: "sh_test", command: "cargo test", started: 7, folder }])
  const command = commandOf("ses_site", "sh_test", file)
  const pause = pauses()
  const lines = followOutput(outputReader(command, () => ({ engine: target, bridge: null })), undefined, pause.wait)
  expect(await lineOf(lines.next())).toEqual({ command: { id: "sh_test", command: "cargo test", started: 7, folder }, output: "Compiling…", cut: false, state: "running" })
  jobs.get("sh_test")!.printed += "Running 12 tests\n"
  const more = lines.next()
  await pause.tick()
  expect(await lineOf(more)).toMatchObject({ output: "Compiling…\nRunning 12 tests", state: "running" })
  // A read that finds nothing new sends nothing; it ends, printing its last line: that's the last.
  const last = lines.next()
  await pause.tick()
  await pause.waiting()
  jobs.get("sh_test")!.printed += "test result: ok\n"
  jobs.get("sh_test")!.status = "exited"
  await pause.tick()
  expect(await lineOf(last)).toMatchObject({ output: "Compiling…\nRunning 12 tests\ntest result: ok", state: "ended" })
  expect(await lineOf(lines.next())).toBeNull()
})

test("followed: what it printed last stands while it can't be read, and once its engine no longer has it", async () => {
  const file = join(scratch, "gone.json")
  jobs.set("sh_dev", { folder, status: "running", printed: "Serving on :5173\n" })
  keep(file, "engine", "ses_site", [{ id: "sh_dev", command: "npm run dev", started: 9, folder }])
  const pause = pauses()
  const lines = followOutput(outputReader(commandOf("ses_site", "sh_dev", file), () => ({ engine: target, bridge: null })), undefined, pause.wait)
  expect(await lineOf(lines.next())).toMatchObject({ output: "Serving on :5173", state: "running" })
  failing = true
  let next = lines.next()
  await pause.tick()
  expect(await lineOf(next)).toMatchObject({ output: "Serving on :5173", state: "unread" })
  failing = false
  next = lines.next()
  await pause.tick()
  expect(await lineOf(next)).toMatchObject({ output: "Serving on :5173", state: "running" })
  // Its engine started again without it: ended, with what it printed last.
  jobs.delete("sh_dev")
  next = lines.next()
  await pause.tick()
  expect(await lineOf(next)).toEqual({ command: { id: "sh_dev", command: "npm run dev", started: 9, folder }, output: "Serving on :5173", cut: false, state: "ended" })
  expect(await lineOf(lines.next())).toBeNull()
})

test("a coding agent's command is read through their bridge, and still where it was once it's no longer listed", async () => {
  const file = join(scratch, "agents.json")
  // (Theirs are known by id alone: the bridge names no folder.)
  jobs.set("shl_agent", { folder: "", status: "running", printed: "watching for changes\n" })
  keep(file, "bridge", "ses_agent", [{ id: "shl_agent", command: "pnpm dev", started: 11 }])
  const command = commandOf("ses_agent", "shl_agent", file)
  expect(command).toEqual({ id: "shl_agent", command: "pnpm dev", started: 11, agents: true })
  const read = outputReader(command, () => ({ engine: null, bridge: target }))
  expect(await read()).toEqual({ command: command!, output: "watching for changes", cut: false, state: "running" })
  // It ended, and the box's list let it go: read where it was, its end said.
  jobs.get("shl_agent")!.status = "exited"
  keep(file, "bridge", "ses_agent", [])
  expect(await outputReader(commandOf("ses_agent", "shl_agent", file), () => ({ engine: null, bridge: target }))()).toMatchObject({ command: { id: "shl_agent", agents: true }, output: "watching for changes", state: "ended" })
})

test("a command opened from Activity after it finishes still shows its output", async () => {
  const before = existsSync(WORK_BACKGROUND) ? readFileSync(WORK_BACKGROUND) : null
  try {
    const command = { id: "sh_race", command: "cargo build", started: 15, folder }
    const route = "kuvo/work/background/ses_finish"
    const handler = engineHandler((() => target) as any, { name: "test" } as any, (() => { throw new Error("No bridge") }) as any)
    jobs.set(command.id, { folder, status: "running", printed: "Compiling\n" })
    keep(WORK_BACKGROUND, "engine", "ses_finish", [command])
    expect((await ask(handler, "GET", route)).json.commands).toEqual([command])
    jobs.set(command.id, { folder, status: "exited", printed: "Compiling\nFinished\n" })
    keep(WORK_BACKGROUND, "engine", "ses_finish", [])
    expect((await ask(handler, "GET", route)).json.commands).toEqual([])
    expect((await ask(handler, "GET", route + "/commands/sh_race/output")).json).toEqual({
      command, output: "Compiling\nFinished", cut: false, state: "ended",
    })
  } finally {
    if (before) writeFileSync(WORK_BACKGROUND, before)
    else rmSync(WORK_BACKGROUND, { force: true })
  }
})
