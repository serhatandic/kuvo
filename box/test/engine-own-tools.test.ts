// A box's engine starts with Kuvo Code's own tools (start_session, make_project, send_message, a
// bot's set_job, the browser…): what its start scripts (`kuvo-launch`) get from `engine-config tools`.
// In 0.7 the Mac app added them to its engine and its machines'; the new desktop app gives a box's
// engine nothing, so a bot moved over (or made fresh) had none of them and said so. The real box
// script and the real built tools (dist/kuvo-browser.js); nothing stands in but the runtime, this
// test's Bun for the engine's.

import "./safety"
import { afterAll, expect, test } from "bun:test"
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { kuvoTools, setToolProgram } from "../src/agents/kuvotools"

const here = import.meta.dir
const main = join(here, "../src/link/main.ts")
const builtTools = join(here, "../dist/kuvo-browser.js")
const SPAWNS = 60_000

const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-own-tools-")))
afterAll(() => rmSync(root, { recursive: true, force: true }))

/** A Mac's own box in a home of its own, as the desktop app sets it up (`box/kind` says `mac`), with
 * its tools in `bin` as a release puts them there; and the environment its start scripts give it. */
function macBox() {
  const account = realpathSync(mkdtempSync(join(root, "mac-")))
  const home = join(account, ".kuvo-code")
  for (const folder of ["bin", "run", "box", "config"]) mkdirSync(join(home, folder), { recursive: true })
  writeFileSync(join(home, "box", "kind"), "mac\n")
  copyFileSync(builtTools, join(home, "bin", "kuvo-browser.js"))
  const env: Record<string, string> = {
    PATH: process.env.PATH!, HOME: account, KUVO_HOME: home, TMPDIR: tmpdir(), XDG_CONFIG_HOME: join(home, "config"),
    ...(process.env.BUN_BE_BUN ? { BUN_BE_BUN: "1" } : {}),
  }
  return { home, env }
}

async function engineConfig(env: Record<string, string>, ...args: string[]) {
  const run = Bun.spawn([process.execPath, main, "engine-config", ...args], { env, stdout: "pipe", stderr: "pipe" })
  const printed = await new Response(run.stdout).text()
  expect(await run.exited).toBe(0)
  return JSON.parse(printed)
}

/** The tools a local MCP server lists, started as the engine starts it: its command, with its
 * environment over the engine's. */
async function listed(server: { command: string[]; environment: Record<string, string> }, env: Record<string, string>): Promise<string[]> {
  const child = Bun.spawn(server.command, { env: { ...env, ...server.environment }, stdin: "pipe", stdout: "pipe", stderr: "ignore" })
  try {
    const send = (message: object) => child.stdin.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n")
    send({ id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "opencode", version: "test" } } })
    send({ method: "notifications/initialized" })
    send({ id: 2, method: "tools/list", params: {} })
    await child.stdin.flush()
    const reader = child.stdout.getReader()
    const decoder = new TextDecoder()
    let text = ""
    for (;;) {
      const { value, done } = await reader.read()
      if (done) throw new Error(`The tools ended without listing any: ${text}`)
      text += decoder.decode(value, { stream: true })
      const lines = text.split("\n")
      text = lines.pop()!
      for (const line of lines) {
        if (!line.trim()) continue
        const message = JSON.parse(line)
        if (message.id === 2) return message.result.tools.map((tool: { name: string }) => tool.name)
      }
    }
  } finally {
    // (Only the one this test started.)
    child.kill()
    await child.exited
  }
}

test("a Mac's own box starts its engine with Kuvo Code's tools, which a bot moved over or made fresh then has", async () => {
  const { home, env } = macBox()
  // As the rehearsal's engine started: nothing given (the desktop app gives none).
  const config = await engineConfig(env, "tools")
  const server = config.mcp.servers["kuvo-browser"]
  expect(server).toMatchObject({ type: "local", codemode: false, environment: { KUVO_HOME: home } })
  expect(server.command.slice(-2)).toEqual([join(home, "bin", "kuvo-browser.js"), "mcp"])
  // What a bot's session asks the engine for: there, by the names agents are told.
  const tools = await listed(server, env)
  for (const name of ["start_session", "make_project", "list_projects", "send_message", "list_agents", "set_job", "schedule_wake_up", "browser_navigate"]) {
    expect(tools).toContain(name)
  }
  // And the sandbox's note still among its instructions.
  expect(config.instructions.length).toBeGreaterThan(0)
  // The coding agents' bridge, started with the same settings, gives its agents the same tools.
  setToolProgram(JSON.stringify(config))
  try {
    expect(kuvoTools("ses_bot")).toMatchObject({ kind: "stdio", name: "kuvo-browser", command: process.execPath, env: { KUVO_SESSION: "ses_bot", KUVO_HOME: home } })
  } finally {
    setToolProgram(undefined)
  }
}, SPAWNS)

test("a machine's engine an older Mac still starts with its own tools keeps them as that Mac gave them", async () => {
  const { env } = macBox()
  const given = { mcp: { servers: { "kuvo-browser": { type: "local", command: ["sh", "-c", "the Mac's"] }, figma: { type: "remote", url: "https://example.com" } } } }
  const config = await engineConfig({ ...env, OPENCODE_CONFIG_CONTENT: JSON.stringify(given) }, "tools")
  expect(config.mcp.servers).toEqual(given.mcp.servers)
}, SPAWNS)

test("the retired Mac app's box, which gives its engine the app's own tools, gets none added", async () => {
  const { env } = macBox()
  const config = await engineConfig(env)
  expect(config.mcp).toBeUndefined()
}, SPAWNS)

test("a Mac's own box's start (kuvo-launch) gives its engine them", async () => {
  const { home, env } = macBox()
  const checkout = join(here, "../..")
  const keeper = join(checkout, "box/src/keeper")
  copyFileSync(join(checkout, "box/dist/kuvo-link.js"), join(home, "bin", "kuvo-link.js"))
  for (const file of ["kuvo-launch", "kuvo-setsid"]) {
    copyFileSync(join(keeper, file), join(home, "bin", file))
    chmodSync(join(home, "bin", file), 0o755)
  }
  // The engine: this test's Bun for the box's scripts, as the engine's own runtime runs them; started
  // to serve, it notes what it was started with, says it's listening, and is done.
  const startedWith = join(home, "engine-started-with")
  const engine = join(home, "engine", "test", "opencode")
  mkdirSync(join(home, "engine", "test"), { recursive: true })
  writeFileSync(engine, `#!/bin/sh
if [ "$1" = serve ]; then printf '%s' "$OPENCODE_CONFIG_CONTENT" >${JSON.stringify(startedWith)}; echo "server listening on http://127.0.0.1:$3"; exit 0; fi
exec ${JSON.stringify(process.execPath)} "$@"
`)
  chmodSync(engine, 0o755)
  // As the keeper starts it: what it's given on its first lines (launch.env), the desktop app giving
  // the engine no settings of its own.
  const given = ["KUVO_PORT=1", 'KUVO_RECORD="port":1,"password":"x"}', "KUVO_VERSION=test", "OPENCODE_CLIENT=desktop", ""].join("\n")
  const start = Bun.spawn(["sh", join(home, "bin", "kuvo-launch")], { env: { PATH: env.PATH, HOME: dirname(home), TMPDIR: env.TMPDIR }, stdin: new TextEncoder().encode(given), stdout: "pipe", stderr: "pipe" })
  const said = await new Response(start.stderr).text()
  expect({ exit: await start.exited, said }).toEqual({ exit: 0, said: "" })
  const server = JSON.parse(readFileSync(startedWith, "utf8")).mcp.servers["kuvo-browser"]
  expect(server.command.slice(-2)).toEqual([join(home, "bin", "kuvo-browser.js"), "mcp"])
  expect(await listed(server, env)).toContain("make_project")
}, SPAWNS)
