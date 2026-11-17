// Probe: what Grok's ACP mode (`grok agent stdio`) asks of its API, against a logging stub.
// A temporary HOME and GROK_HOME and a made-up XAI_API_KEY: your sign-in is never read.
// usage: bun s3-acp-probe.mjs <grok executable>
import { spawn } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

const [grok] = process.argv.slice(2)
const dir = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), "s3p-"))
fs.mkdirSync(path.join(dir, "work"))
fs.mkdirSync(path.join(dir, "home/.grok"), { recursive: true })
const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const body = await request.text()
    console.log("API", request.method, new URL(request.url).pathname, body.slice(0, 300))
    const url = new URL(request.url)
    if (url.pathname.endsWith("/models")) return Response.json({ object: "list", data: [{ id: "grok-4.6", object: "model", owned_by: "xai" }] })
    if (url.pathname.endsWith("/api-key")) return Response.json({ name: "stub", acls: ["api-key:model:*", "api-key:endpoint:*"], api_key_blocked: false, api_key_disabled: false, team_blocked: false })
    return Response.json({ error: "stub" }, { status: 500 })
  },
})
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_") && !name.startsWith("GROK") && !name.startsWith("XAI")))
Object.assign(env, { HOME: path.join(dir, "home"), GROK_HOME: path.join(dir, "home/.grok"), XAI_API_KEY: "not-a-key", GROK_XAI_API_BASE_URL: `http://127.0.0.1:${server.port}/v1`, GROK_MODELS_BASE_URL: `http://127.0.0.1:${server.port}/v1`, GROK_SANDBOX: "off", GROK_MEMORY: "0" })
const child = spawn(grok, ["--permission-mode", "default", "agent", "--no-leader", "stdio"], { cwd: path.join(dir, "work"), env, stdio: ["pipe", "pipe", "pipe"] })
child.stderr.on("data", (chunk) => console.log("ERR", String(chunk).slice(0, 300)))
let buffer = ""
child.stdout.on("data", (chunk) => {
  buffer += chunk
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at)
    try { const id = JSON.parse(line).result?.sessionId; if (id) globalThis.sessionId = id } catch {}
    if (!/servers_updated|session\/setup|available_commands/.test(line)) console.log("OUT", line.slice(0, 400))
    buffer = buffer.slice(at + 1)
  }
})
const send = (message) => child.stdin.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n")
send({ id: 1, method: "initialize", params: { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } }, _meta: { clientType: "extension" } } })
await Bun.sleep(1500)
send({ id: 2, method: "authenticate", params: { methodId: "xai.api_key" } })
await Bun.sleep(1000)
send({ id: 3, method: "session/new", params: { cwd: path.join(dir, "work"), mcpServers: [] } })
await Bun.sleep(3000)
send({ id: 4, method: "initialize", params: { protocolVersion: 1, clientCapabilities: {}, _meta: { clientType: "extension" } } })
await Bun.sleep(1500)
send({ id: 5, method: "session/prompt", params: { sessionId: globalThis.sessionId, prompt: [{ type: "text", text: "hi" }] } })
await Bun.sleep(5000)
child.kill("SIGKILL")
server.stop(true)
fs.rmSync(dir, { recursive: true, force: true })
process.exit(0)
