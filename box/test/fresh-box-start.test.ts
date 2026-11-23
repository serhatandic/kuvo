// A computer's first box, installed and started the real way, with the engine itself (#763): what
// its engine, its agents and its clients are then given, which an app gave them before. The retired
// Mac app handed its engine Kuvo Code's own tools in OPENCODE_CONFIG_CONTENT, wrote the engine's
// plugin into its settings, gave it your login shell's environment, and noted how it starts; once
// the box started itself (#713), nothing did, and the first real move found a Mac with no engine
// and then agents with no tools. This is what a person installing Kuvo Code gets:
//
// - the release packed by scripts/box-release.sh from this checkout (the keeper, the link, the
//   bridge and the tools as built), its engine the engine itself, "downloaded" from a file here;
// - `kuvo-install new`, as the desktop app runs it (on a Mac with `--app`: a Mac's own box, which
//   Kuvo Helper runs; on Linux, a machine's, started by the start script with its `@reboot` line);
// - on a Mac, then, what Kuvo Helper does once launchd starts it (mac/helper BoxHelper.swift):
//   `kuvo-start keep` in a session of its own and `kuvo-start serve` with its password and the
//   vault's secrets on stdin, both with a shell's few variables, and the Keychain behind its host
//   interface (a stand-in, `run/host.json`, as the helper writes it);
// - and then, as an agent finds them: the engine answers; Kuvo Code's tools are among its MCP
//   servers, connected, and a model is offered them, with the sandbox's note among its
//   instructions; the engine's plugin is loaded, so the commands agents run get your own folders
//   back and not the engine's settings, and (on a Mac) what your login shell exports; the agents'
//   bridge answers; and the vault answers a client.
//
// Stand-ins only for what's outside: the model (OpenAI's chat completions, here), launchd, cron and
// `open`, the keeper's `pkill` and `caffeinate`, and the Keychain. Skipped without the engine: name
// it in KUVO_TEST_ENGINE (the pinned build, never the real Kuvo Code's own copy in use); the Linux
// check (scripts/lokum-box-check.sh) runs the box's tests on the engine itself, which it takes.

import "./safety"
import { afterAll, expect, test } from "bun:test"
import { spawn, spawnSync, type ChildProcess } from "node:child_process"
import crypto from "node:crypto"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"
import "../bridge-tests/at-end.js"
// @ts-expect-error: a stand-in in plain JavaScript, shared with the bridge's tests
import { standInHost } from "../bridge-tests/box-stand-ins.js"

const mac = process.platform === "darwin"
const checkout = path.join(import.meta.dir, "../..")
const ENGINE = (() => {
  const named = process.env.KUVO_TEST_ENGINE
  if (named && fs.existsSync(named)) return named
  return path.basename(process.execPath) === "opencode" ? process.execPath : null
})()
const LONG = 600_000

const root = ENGINE ? fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-fresh-box-"))) : ""
const cleanups: (() => Promise<void> | void)[] = []
afterAll(async () => {
  // Cut off while it waited: what for, and what the box says of its engine and tools.
  if (waiting && boxHome) {
    const tail = (file: string) => { try { return fs.readFileSync(path.join(boxHome!, file), "utf8").split("\n").slice(-30).join("\n") } catch { return "(none)" } }
    if (waitingDetail) console.error(await waitingDetail().catch(error => String(error)))
    console.error(`Still waiting for ${waiting}.\nrun: ${fs.readdirSync(path.join(boxHome, "run")).join(" ")}\nrun/engine.log:\n${tail("run/engine.log")}\nrun/agents.log:\n${tail("run/agents.log")}`)
  }
  for (const cleanup of cleanups.reverse()) {
    try { await cleanup() } catch {}
  }
  if (root) fs.rmSync(root, { recursive: true, force: true })
})

function put(file: string, text: string | Buffer, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, text)
  fs.chmodSync(file, mode)
}
const read = (file: string) => fs.readFileSync(file, "utf8")
/** What `until` waits on, asked again and again until it's there (an outside process's state, which
 * no event says): the test's own limit stops it. */
async function until<T>(what: () => T | Promise<T>, said: string, detail?: () => Promise<string>): Promise<NonNullable<T>> {
  waiting = said
  waitingDetail = detail ?? null
  for (;;) {
    // (An outside process not answering yet is waited for; a turn that failed ends the wait.)
    const found = await Promise.resolve(what()).catch(error => { if (error instanceof Error && error.message.startsWith("The turn failed")) throw error; return undefined })
    if (found) {
      waiting = null
      return found as NonNullable<T>
    }
    await new Promise(resolve => setTimeout(resolve, 200))
  }
}
/** What the test waits for now, said should it be cut off (with what the box noted), and the box. */
let waiting: string | null = null
let waitingDetail: (() => Promise<string>) | null = null
let boxHome: string | null = null

/** The environment of the test's processes: none of the runner's own (Bun's), nor anything of a
 * Kuvo Code it runs in (test/safety). */
const clean = (): Record<string, string> => Object.fromEntries(Object.entries(process.env).filter(([name, value]) => value !== undefined && !/^(BUN_|KUVO_|OPENCODE_|XDG_)/.test(name))) as Record<string, string>

/** A model, as a stand-in: it answers each turn, noting the tools it was offered and what it was
 * told (its system message). */
async function modelStandIn() {
  const model = { tools: [] as string[][], told: [] as string[], url: "" }
  const server = http.createServer(async (request, response) => {
    const chunks: Buffer[] = []
    for await (const chunk of request) chunks.push(chunk as Buffer)
    const asked = JSON.parse(Buffer.concat(chunks).toString() || "{}")
    model.tools.push((asked.tools ?? []).map((each: { function?: { name?: string } }) => each.function?.name))
    model.told.push((asked.messages ?? []).filter((each: { role: string }) => each.role === "system").map((each: { content: unknown }) => typeof each.content === "string" ? each.content : JSON.stringify(each.content)).join("\n"))
    const chunk = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ id: "c", object: "chat.completion.chunk", created: 1, model: "m", choices: [{ index: 0, delta, finish_reason: finish }], ...(finish ? { usage: { prompt_tokens: 5, completion_tokens: 5, total_tokens: 10 } } : {}) })}\n\n`
    response.writeHead(200, { "content-type": "text/event-stream" })
    response.end(chunk({ role: "assistant", content: "Hello." }) + chunk({}, "stop") + "data: [DONE]\n\n")
  })
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve))
  cleanups.push(() => { server.closeAllConnections(); server.close() })
  model.url = `http://127.0.0.1:${(server.address() as { port: number }).port}/v1`
  return model
}

/** The release an app carries (`boxes.json`, the pack, `kuvo-install`), packed from this checkout,
 * with the engine at `engine` (its version `version`) as the one it pins. */
function release(version: string): string {
  const engineTar = path.join(root, `engine-${version}.tgz`)
  put(path.join(root, "engine-package", "package", "bin", "opencode"), fs.readFileSync(ENGINE!), 0o755)
  expect(spawnSync("tar", ["czf", engineTar, "-C", path.join(root, "engine-package"), "package"], { env: { ...clean(), COPYFILE_DISABLE: "1" } }).status).toBe(0)
  const sources = path.join(root, "sources")
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js", "box/dist/kuvo-link.js"]) {
    put(path.join(sources, file), fs.readFileSync(path.join(checkout, file)))
  }
  const build = { url: pathToFileURL(engineTar).href, integrity: "sha512-" + crypto.createHash("sha512").update(fs.readFileSync(engineTar)).digest("base64") }
  put(path.join(sources, "box/src/shared/engine-pin.json"), JSON.stringify({ version, linux: { x86_64: build, aarch64: build }, darwin: { x86_64: build, aarch64: build } }))
  // Kuvo Helper as its build leaves it, on a Mac: a program signed ad hoc (packing checks it). The
  // test does what it does (below), so it's never run.
  const helpers: string[] = []
  if (mac) {
    const helper = path.join(root, "helper", "Kuvo Helper.app")
    put(path.join(helper, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Kuvo Helper</string><key>CFBundleIdentifier</key><string>run.kuvo.code.tests763.box</string></dict></plist>\n`)
    put(path.join(helper, "Contents", "MacOS", "Kuvo Helper"), fs.readFileSync("/usr/bin/true"), 0o755)
    const signed = spawnSync("codesign", ["--force", "--sign", "-", helper], { encoding: "utf8" })
    expect(signed.status, signed.stderr).toBe(0)
    helpers.push(helper)
  }
  const out = path.join(root, "box-release")
  const { privateKey } = crypto.generateKeyPairSync("ed25519")
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", out, ...helpers], {
    cwd: checkout, encoding: "utf8",
    env: { ...clean(), BUN: process.env.KUVO_TEST_BUN || process.execPath, NODE: process.execPath, ...(process.env.BUN_BE_BUN ? { BUN_BE_BUN: "1" } : {}), BOX_SOURCES: sources, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) as string },
  })
  expect(packed.status, packed.stderr).toBe(0)
  return out
}

/** The engine noted where it answers (`run/engine.json`), as a client finds it. */
const engineOf = (K: string): { pid: number; port: number; password: string; agentsPort?: number; agentsPassword?: string } | null => {
  try { return JSON.parse(read(path.join(K, "run", "engine.json"))) } catch { return null }
}

test.if(!!ENGINE)("a computer's first box, installed and started the real way: its engine answers with Kuvo Code's tools, its plugin and the sandbox's note, its agents' bridge and its vault answer", async () => {
  const version = /(\d+\.\d+\.\d+[0-9A-Za-z.-]*)/.exec(spawnSync(ENGINE!, ["--version"], { env: clean(), encoding: "utf8" }).stdout)?.[1]
  expect(version).toBeTruthy()
  const from = release(version!)
  const home = path.join(root, "home"), K = path.join(home, ".kuvo-code"), tools = path.join(root, "tools")
  boxHome = K
  fs.mkdirSync(home, { recursive: true })
  // What starts a box from outside its folder, and what its keeper's loop reaches for: stand-ins,
  // never this computer's launchd, crontab, other processes or power settings.
  put(path.join(tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${root}/launchctl-asked"\ncase "$1" in print) [ -f "${root}/loaded" ] ;; bootstrap) touch "${root}/loaded" ;; bootout) rm -f "${root}/loaded" ;; esac\n`, 0o755)
  put(path.join(tools, "crontab"), `#!/bin/sh\nif [ "$1" = -l ]; then cat "${root}/crontab" 2>/dev/null || exit 1; else cat >"${root}/crontab"; fi\n`, 0o755)
  for (const tool of ["open", "pkill", "caffeinate"]) put(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>"${root}/reached"\n`, 0o755)
  // Your login shell, as it starts: what you export there (a tool's folder, and what a box's own
  // start never takes from it).
  const shell = mac ? "/bin/zsh" : fs.existsSync("/bin/bash") ? "/bin/bash" : "/bin/sh"
  // (And what a start's own shell must never take: a read-only variable of its own, and one named
  // as the start's own are.)
  const startup = "export KUVO763_TOOL_HOME=/opt/tool-763\nexport KUVO_NOT_YOURS=1\nexport BUN_BE_BUN=1\nexport SHELLOPTS=braceexpand 2>/dev/null\nexport port=1 K=/nowhere\n"
  for (const file of [".zprofile", ".bash_profile", ".profile"]) put(path.join(home, file), startup)

  // Installed, as the desktop app runs its installer.
  const bundle = path.join(home, "Applications", "Kuvo Code.app")
  if (mac) put(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.tests763</string></dict></plist>\n`)
  const shellEnv = { HOME: home, USER: os.userInfo().username, LOGNAME: os.userInfo().username, SHELL: shell, LANG: "en_US.UTF-8", TMPDIR: os.tmpdir(), PATH: `${tools}:/usr/bin:/bin:/usr/sbin:/sbin` }
  cleanups.push(() => {
    // Everything the box runs, ended as an uninstall ends it (`kuvo-start off`): its keeper's
    // session, the engine and its bridge; then whatever this test started itself.
    if (fs.existsSync(path.join(K, "bin", "kuvo-start"))) spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "off"], { env: shellEnv, timeout: 120_000 })
  })
  const installed = spawnSync("sh", [path.join(from, "kuvo-install"), "new", ...(mac ? ["--app", bundle] : [])], {
    env: { ...shellEnv, KUVO_TEST_UNINSTALL_TOOLS: tools, KUVO_TEST_INSTALL_FILE_ENGINE: "1" }, encoding: "utf8", timeout: 300_000,
  })
  const said = JSON.parse(installed.stdout.trim().split("\n").pop() ?? "null")
  expect({ status: installed.status, result: said?.result, started: said?.started }, installed.stderr + installed.stdout).toEqual({ status: 0, result: "done", started: true })
  // (A Mac's own box says so; a machine's says nothing.)
  expect(fs.existsSync(path.join(K, "box", "kind")) ? read(path.join(K, "box", "kind")).trim() : "machine").toBe(mac ? "mac" : "machine")

  // Your model, as you'd set one up in Settings (the engine's own settings, in the box's home).
  const model = await modelStandIn()
  // (A Mac's own box keeps them in `config`; a machine's, in `box`: kuvo-launch's OPENCODE_CONFIG.)
  const settings = mac ? path.join(K, "config", "opencode", "opencode.json") : path.join(K, "box", "opencode.json")
  const given = fs.existsSync(settings) ? JSON.parse(read(settings)) : {}
  put(settings, JSON.stringify({ ...given, provider: { lab: { name: "Lab", npm: "@ai-sdk/openai-compatible", options: { baseURL: model.url, apiKey: "none" }, models: { m: { name: "M", tool_call: true } } } }, model: "lab/m" }), 0o600)

  let serve: { url: string; password: string } | null = null
  if (mac) {
    // Kuvo Helper, as launchd starts it: its host interface (the Keychain), `serve` with its
    // password and the vault's secrets on stdin (kept open: it goes when that closes), and the
    // keeper in a session of its own.
    const hostPassword = crypto.randomBytes(24).toString("hex"), hostVaultSecret = crypto.randomBytes(32).toString("hex")
    const host = await standInHost(hostPassword)
    host.vaultSecret = hostVaultSecret
    cleanups.push(() => host.close?.())
    put(path.join(K, "run", "host.json"), JSON.stringify({ url: host.url, password: hostPassword }), 0o600)
    const password = crypto.randomBytes(24).toString("hex")
    const served: ChildProcess = spawn("/bin/sh", [path.join(K, "bin", "kuvo-start"), "serve"], { cwd: K, env: shellEnv, stdio: ["pipe", "ignore", "ignore"] })
    served.stdin!.write(JSON.stringify({ password, hostVaultSecret, vaultFileKey: crypto.randomBytes(32).toString("base64url") }) + "\n")
    cleanups.push(() => new Promise<void>(resolve => { if (served.exitCode !== null) return resolve(); served.once("exit", () => resolve()); served.stdin!.end() }))
    const keeper = spawn("/bin/sh", [path.join(K, "bin", "kuvo-start"), "keep"], { cwd: K, env: shellEnv, detached: true, stdio: "ignore" })
    keeper.unref()
    cleanups.push(() => { try { process.kill(-keeper.pid!, "SIGTERM") } catch {} })
    const record = await until(() => { try { return JSON.parse(read(path.join(K, "run", "serve.json"))) } catch { return null } }, "serve")
    serve = { url: `http://127.0.0.1:${record.port}`, password: record.password }
  } else {
    // A machine's: running already, started by its start script. With your model, started over.
    const restarted = spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "restart"], { env: shellEnv, encoding: "utf8", timeout: 300_000 })
    expect(restarted.status, restarted.stderr).toBe(0)
  }

  if (!mac) {
    const card = await until(() => JSON.parse(read(path.join(K, "run", "setup-card.json"))), "the standalone setup card")
    expect(card).toMatchObject({ role: "primary", epoch: 1, computers: 1, self: { kind: "machine" } })
    expect(card.primary.key).toBe(card.self.key)
    expect(fs.existsSync(path.join(K, "run", "link.needed"))).toBe(false)
  }

  // The engine answers where it's noted.
  const engine = await until(() => { const noted = engineOf(K); return noted?.port ? noted : null }, "the engine")
  const auth = { authorization: "Basic " + Buffer.from(`opencode:${engine.password}`).toString("base64"), "content-type": "application/json" }
  const api = async (method: string, route: string, body?: unknown) => {
    const response = await fetch(`http://127.0.0.1:${engine.port}/${route}`, { method, headers: auth, body: body === undefined ? undefined : JSON.stringify(body) })
    const text = await response.text()
    return { status: response.status, json: text ? JSON.parse(text) : null }
  }
  expect((await api("GET", "api/session")).status).toBe(200)

  // Kuvo Code's own tools: among its MCP servers, run from the box's own files, and connected; and
  // the sandbox's note among its instructions (what the box writes there once commands run in it).
  const documents: { info?: { mcp?: { servers?: Record<string, { command?: string[] }> }; instructions?: string[] } }[] = (await api("GET", "api/config")).json
  const servers = Object.assign({}, ...documents.map(each => each.info?.mcp?.servers ?? {})) as Record<string, { command?: string[] }>
  expect(servers["kuvo-browser"]?.command?.slice(-2)).toEqual([path.join(K, "bin", "kuvo-browser.js"), "mcp"])
  expect(documents.some(each => each.info?.instructions?.includes(path.join(K, "run", "fence", "agents.md")))).toBe(true)
  const tools763 = await until(async () => {
    const server = (await api("GET", "api/mcp")).json?.data?.find((each: { name: string }) => each.name === "kuvo-browser")
    return server && server.status?.status !== "pending" ? server : null
  }, "Kuvo Code's tools to connect")
  expect(tools763.status.status).toBe("connected")

  // An agent's turn: the model is offered Kuvo Code's tools by the names agents are told.
  const session = (await api("POST", "api/session", { location: { directory: path.join(K, "workspace") } })).json.data
  expect((await api("POST", `api/session/${session.id}/model`, { model: { providerID: "lab", id: "m" } })).status).toBeLessThan(300)
  expect((await api("POST", `api/session/${session.id}/prompt`, { text: "Hi", delivery: "steer" })).status).toBeLessThan(300)
  const offered = await until(async () => {
    const found = model.tools.find(each => each.some(name => name?.startsWith("kuvo-browser_")))
    if (found) return found
    // (A turn that ended without asking the model, said at once.)
    const last = (await api("GET", `api/session/${session.id}/message?limit=5`)).json?.data?.[0]
    if (last?.type === "idle" && last.outcome === "failed") throw new Error(`The turn failed: ${JSON.stringify(last.error)}`)
    return null
  }, "a turn offered Kuvo Code's tools",
    async () => `The model was asked ${model.tools.length} times, offered: ${JSON.stringify(model.tools.map(each => each.slice(0, 12)))}\nThe turn: ${JSON.stringify((await api("GET", `api/session/${session.id}/message?limit=20`)).json).slice(0, 3000)}\nMCP: ${JSON.stringify((await api("GET", "api/mcp")).json)}`)
  for (const name of ["start_session", "make_project", "list_projects", "send_message", "list_agents", "schedule_wake_up", "set_job", "queue_add"]) {
    expect(offered).toContain(`kuvo-browser_${name}`)
  }
  // The vault's and the sandbox's asks are the box's long-running process's, which a Mac's own box
  // always runs (Kuvo Helper's `serve`); a machine's runs once it has something to answer (a Mac that
  // set it up, a phone, a vault: PROTOCOL.md, "Paired phones"), and this one, installed with no Mac, has none.
  // (Whether a machine's should run with nothing to answer, so its sandbox and asks work with no
  // Mac, a desktop app on that computer say, is an open design question, not this test's.)
  for (const name of ["vault_ask", "sandbox_ask"]) {
    if (mac) expect(offered).toContain(`kuvo-browser_${name}`)
    else expect(offered).not.toContain(`kuvo-browser_${name}`)
  }

  // The engine's plugin, loaded: a command an agent runs gets your own folders back, never the
  // engine's settings (which hold keys), and Kuvo Code's commands (kuvo-wake) first on its PATH;
  // on a Mac, what your login shell exports too, but never what's a box's own.
  const plugins = (await api("GET", "api/plugin")).json.data
  expect(plugins.find((each: { id: string }) => each.id === "kuvo-code.commands")?.state?.status).toBe("active")
  const ran = (await api("POST", "api/shell", { command: "env" })).json.data
  const output = await until(async () => {
    const now = (await api("GET", `api/shell/${ran.id}`)).json?.data
    return now && now.status !== "running" ? fs.readFileSync(now.file, "utf8") : null
  }, "the command to finish")
  const env: Record<string, string> = Object.fromEntries(output.split("\n").filter(line => line.includes("=")).map(line => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]))
  expect(env.OPENCODE_CONFIG_CONTENT).toBeUndefined()
  expect(env.XDG_CONFIG_HOME).toBeUndefined()
  expect(env.PATH!.split(":")[0]).toBe(path.join(K, "bin"))
  expect(env.KUVO_WAKES).toBe(path.join(K, "wakes"))
  expect(env.KUVO_NOT_YOURS).toBeUndefined()
  expect(env.BUN_BE_BUN).toBeUndefined()
  expect(env.KUVO763_TOOL_HOME ?? "not given").toBe(mac ? "/opt/tool-763" : "not given")

  // The agents' bridge, kept by the keeper apart from the engine (which no longer names it), answers
  // where its own record says.
  expect(engine.agentsPort).toBeUndefined()
  const record = await until(() => { try { const said = JSON.parse(fs.readFileSync(path.join(K, "run", "bridge.json"), "utf8")); return said?.port ? said as { port: number; password: string } : null } catch { return null } }, "the agents' bridge")
  const bridge = await fetch(`http://127.0.0.1:${record.port}/api/info`, { headers: { authorization: "Basic " + Buffer.from(`opencode:${record.password}`).toString("base64") } })
  expect(bridge.status).toBe(200)

  // The vault answers a client: on a Mac, `serve`, as the desktop app asks it; on a machine, one
  // request through the box's own command, as a Mac reaches a machine's box over SSH (where, with
  // nothing to answer yet, it says its process isn't running: above).
  let vault: { status: number; body: { state?: unknown; message?: string } }
  if (serve) {
    const answered = await fetch(`${serve.url}/kuvo/vault`, { headers: { authorization: "Basic " + Buffer.from(`opencode:${serve.password}`).toString("base64"), "x-kuvo-from": "app" } })
    vault = { status: answered.status, body: (await answered.json()) as { state?: unknown; message?: string } }
  } else {
    const asked = spawnSync(path.join(K, "engine", version!, "opencode"), ["--config=/dev/null", "--no-env-file", path.join(K, "bin", "kuvo-link.js"), "box", "route"], {
      cwd: K, encoding: "utf8", timeout: 60_000, input: JSON.stringify({ method: "GET", path: "kuvo/vault", headers: { "x-kuvo-from": "app" } }),
      env: { ...shellEnv, KUVO_HOME: K, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
    })
    const answer = JSON.parse(asked.stdout.trim().split("\n").pop() ?? "null")
    vault = { status: answer?.status, body: typeof answer?.body === "string" ? JSON.parse(answer.body) : answer?.body }
  }
  if (mac) {
    expect(vault.status).toBe(200)
    expect(vault.body.state).toBeTruthy()
  } else {
    expect(vault).toMatchObject({ status: 503, body: { message: expect.stringContaining("link here isn't running") } })
  }
}, LONG)
