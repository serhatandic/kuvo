// An engine's start stops the engines this box started before that still run though none is noted
// (seen on a Mac: ten engines of the box's own, each on a port of its own, after updates and
// restarts lost their notes). Only by the number each start noted, and only while that number is
// still this box's engine: never by name, never another program that took the number since.
import "./safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import { spawn, spawnSync, type ChildProcess } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

const launch = path.join(import.meta.dir, "../src/keeper/kuvo-launch")
const setsid = path.join(import.meta.dir, "../src/keeper/kuvo-setsid")
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-launch-strays-")))
const started: number[] = []
const children: ChildProcess[] = []
afterEach(() => {
  for (const pid of started.splice(0)) try { process.kill(pid, "SIGKILL") } catch {}
  for (const child of children.splice(0)) try { child.kill("SIGKILL") } catch {}
})
afterAll(() => fs.rmSync(root, { recursive: true, force: true }))

/** Whether it runs: one stopped but not yet reaped by this test (its parent) doesn't. */
const runs = (pid: number) => {
  const state = spawnSync("ps", ["-o", "stat=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim()
  return state !== "" && !state.startsWith("Z")
}

/** A box's home with a stand-in engine: it says it listens, and runs until stopped. */
function home(name: string) {
  const home = path.join(root, name), K = path.join(home, ".kuvo-code")
  const engine = path.join(K, "engine", "9.9.9", "opencode")
  fs.mkdirSync(path.dirname(engine), { recursive: true })
  fs.writeFileSync(engine, "#!/bin/sh\necho 'server listening'\nwhile :; do sleep 1; done\n", { mode: 0o755 })
  fs.mkdirSync(path.join(K, "bin"), { recursive: true })
  fs.copyFileSync(setsid, path.join(K, "bin", "kuvo-setsid"))
  fs.chmodSync(path.join(K, "bin", "kuvo-setsid"), 0o755)
  fs.mkdirSync(path.join(K, "run", "engines"), { recursive: true })
  return { home, K, engine }
}

/** One of the box's engines running with nothing noting it, as a lost note leaves one. */
function stray(engine: string) {
  const child = spawn(engine, ["serve", "--port", "4001"], { stdio: "ignore", detached: true })
  children.push(child)
  return child.pid!
}

function start(home: string) {
  const ran = spawnSync("sh", [launch], {
    input: "\n", encoding: "utf8", timeout: 60_000,
    env: { HOME: home, PATH: "/usr/bin:/bin:/usr/sbin:/sbin", KUVO_PORT: "4999", KUVO_RECORD: `"port":4999,"password":"x"}`, KUVO_VERSION: "9.9.9" },
  })
  expect(ran.status, ran.stderr).toBe(0)
  const pid = Number(ran.stdout.trim().split(/\s/)[0])
  started.push(pid)
  return pid
}

test("a start stops the engines this box started before that run unnoted, and notes its own", () => {
  const { home: at, K, engine } = home("strays")
  const lost = [stray(engine), stray(engine)]
  for (const pid of lost) fs.writeFileSync(path.join(K, "run", "engines", String(pid)), "")
  const pid = start(at)
  for (const pid of lost) expect(runs(pid)).toBe(false)
  expect(runs(pid)).toBe(true)
  expect(JSON.parse(fs.readFileSync(path.join(K, "run", "engine.json"), "utf8")).pid).toBe(pid)
  expect(fs.readdirSync(path.join(K, "run", "engines"))).toEqual([String(pid)])
})

test("a number noted once that's another program's now, and an engine this box never noted, are left alone", () => {
  const { home: at, K, engine } = home("others")
  const other = spawn("/bin/sh", ["-c", "while :; do sleep 1; done"], { stdio: "ignore" })
  children.push(other)
  fs.writeFileSync(path.join(K, "run", "engines", String(other.pid)), "")
  const unnoted = stray(engine)
  start(at)
  expect(runs(other.pid!)).toBe(true)
  expect(runs(unnoted)).toBe(true)
})

test("a number noted once that's the box's own server or bridge now (the same program, running the link) is left alone", () => {
  const { home: at, K, engine } = home("server")
  // As a Mac's helper or the keeper runs it: the engine's program, with the box's link.
  const server = spawn(engine, ["--config=/dev/null", "--no-env-file", path.join(K, "bin", "kuvo-link.js"), "serve"], { stdio: "ignore", detached: true })
  children.push(server)
  fs.writeFileSync(path.join(K, "run", "engines", String(server.pid)), "")
  start(at)
  expect(runs(server.pid!)).toBe(true)
})
