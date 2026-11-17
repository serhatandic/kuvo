// Spike S3 (ACP): Grok's `agent stdio` under hold-spike.mjs. A controller starts a turn and is
// killed with a permission ask open; B attaches (and sends `initialize` again), answers the replayed
// ask; B is killed mid-stream; C attaches and sees the prompt's answer (the turn's end).
// A stub chat/completions API, a temporary HOME and GROK_HOME and a made-up key: no sign-in is read.
// usage: bun s3-acp.mjs <grok executable>
import { spawn } from "node:child_process"
import fs from "node:fs"
import net from "node:net"
import os from "node:os"
import path from "node:path"

const [grok] = process.argv.slice(2)
const dir = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), "s3-"))
const work = path.join(dir, "work")
fs.mkdirSync(work)
fs.mkdirSync(path.join(dir, "home/.grok"), { recursive: true })
const events = []
const requests = []
const say = (role, what) => events.push({ role, at: Date.now(), ...what })
let tools = []
const chunk = (delta, finish = null) => `data: ${JSON.stringify({ id: "c1", object: "chat.completion.chunk", created: 1, model: "grok-4.6", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`
const stream = (parts, slow) =>
  new Response(
    new ReadableStream({
      async start(controller) {
        for (const part of parts) {
          controller.enqueue(new TextEncoder().encode(part))
          if (slow) await Bun.sleep(slow)
        }
        controller.enqueue(new TextEncoder().encode("data: [DONE]\n\n"))
        controller.close()
      },
    }),
    { headers: { "content-type": "text/event-stream" } },
  )
const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname.endsWith("/models")) return Response.json({ object: "list", data: [{ id: "grok-4.6", object: "model", owned_by: "xai" }] })
    if (url.pathname.endsWith("/api-key")) return Response.json({ name: "stub", acls: ["api-key:model:*", "api-key:endpoint:*"], api_key_blocked: false, api_key_disabled: false, team_blocked: false })
    if (!url.pathname.endsWith("/chat/completions")) return Response.json({})
    const body = await request.json()
    const main = (body.tools ?? []).length > 0
    tools = (body.tools ?? []).map((tool) => tool.function?.name)
    const results = body.messages.filter((message) => message.role === "tool").length
    requests.push({ main, results })
    if (!main) return stream([chunk({ role: "assistant", content: "ok" }), chunk({}, "stop")])
    if (results === 0) {
      const shell = tools.find((name) => /^(bash|shell|run_terminal_cmd|run_command|execute_command)$/i.test(name)) ?? tools.find((name) => /bash|shell|command|terminal/i.test(name))
      return stream([chunk({ role: "assistant", tool_calls: [{ index: 0, id: "call_one", type: "function", function: { name: shell, arguments: JSON.stringify({ command: "echo one > one.txt", description: "write one" }) } }] }), chunk({}, "tool_calls")])
    }
    return stream([chunk({ role: "assistant", content: "" }), ...Array.from({ length: 10 }, (_, i) => chunk({ content: `STREAM${i + 1} ` })), chunk({}, "stop")], 400)
  },
})
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_") && !name.startsWith("GROK") && !name.startsWith("XAI")))
Object.assign(env, { HOME: path.join(dir, "home"), GROK_HOME: path.join(dir, "home/.grok"), XAI_API_KEY: "not-a-key", GROK_XAI_API_BASE_URL: `http://127.0.0.1:${server.port}/v1`, GROK_MODELS_BASE_URL: `http://127.0.0.1:${server.port}/v1`, GROK_SANDBOX: "off", GROK_MEMORY: "0", SPIKE_REPLAY: "all" })

const sock = path.join(dir, "hold.sock")
spawn(process.execPath, [path.join(import.meta.dirname, "hold-spike.mjs"), sock, "jsonrpc", path.join(dir, "journal.log"), "--", grok, "--permission-mode", "default", "agent", "--no-leader", "stdio"], { cwd: work, env, detached: true, stdio: "ignore" }).unref()
for (let i = 0; i < 200 && !fs.existsSync(sock); i++) await Bun.sleep(50)
const pids = fs.readFileSync(sock + ".pid", "utf8").trim().split(" ").map(Number)
process.on("exit", () => { for (const pid of pids) try { process.kill(pid, "SIGKILL") } catch {} })
setTimeout(() => { console.log("gave up"); process.exit(1) }, 120_000)

function controller(role, first, { answer }) {
  const socket = net.connect(sock)
  let next = first
  const waiting = new Map()
  const send = (message) => socket.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n")
  let buffer = ""
  socket.setEncoding("utf8")
  socket.on("data", (data) => {
    buffer += data
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at)
      buffer = buffer.slice(at + 1)
      let message
      try { message = JSON.parse(line) } catch { continue }
      if (message.method && message.id !== undefined) {
        say(role, { request: message.method, id: message.id, options: message.params?.options?.map((option) => option.optionId) })
        if (!answer) continue
        if (message.method === "session/request_permission") {
          const allow = message.params.options.find((option) => option.kind === "allow_once") ?? message.params.options[0]
          send({ id: message.id, result: { outcome: { outcome: "selected", optionId: allow.optionId } } })
        } else send({ id: message.id, result: {} })
      } else if (message.method === "session/update") {
        const update = message.params?.update
        if (update?.sessionUpdate === "agent_message_chunk") say(role, { delta: update.content?.text })
        else if (/tool_call/.test(update?.sessionUpdate ?? "")) say(role, { tool: update.sessionUpdate, status: update.status, title: update.title })
      } else if (message.id !== undefined) {
        const resolve = waiting.get(message.id)
        say(role, { response: message.id, mine: Boolean(resolve), error: message.error?.message, result: JSON.stringify(message.result ?? null).slice(0, 100) })
        resolve?.(message)
        waiting.delete(message.id)
      }
    }
  })
  const request = (method, params) => new Promise((resolve) => { const id = next++; waiting.set(id, resolve); send({ id, method, params }) })
  return { socket, request }
}
const until = async (what, seconds = 60) => {
  for (let i = 0; i < seconds * 20; i++) {
    if (events.some(what)) return true
    await Bun.sleep(50)
  }
  return false
}
const initialize = { protocolVersion: 1, clientCapabilities: {}, _meta: { clientType: "extension" } }

const a = controller("A", 1, { answer: false })
await a.request("initialize", initialize)
await a.request("authenticate", { methodId: "xai.api_key" })
const session = (await a.request("session/new", { cwd: work, mcpServers: [] })).result?.sessionId
console.log("session:", session)
void a.request("session/prompt", { sessionId: session, prompt: [{ type: "text", text: "write one, then talk" }] })
console.log("A was asked:", await until((e) => e.role === "A" && e.request === "session/request_permission", 60))
a.socket.destroy()
await Bun.sleep(300)
const alive = (pid) => { try { process.kill(pid, 0); return true } catch { return false } }
console.log("after A died, holder/agent alive:", pids.map(alive))

const b = controller("B", 1000, { answer: true })
console.log("B initialize again:", JSON.stringify((await b.request("initialize", initialize)).error ?? "ok"))
console.log("B was asked:", await until((e) => e.role === "B" && e.request === "session/request_permission", 20))
console.log("B saw the stream:", await until((e) => e.role === "B" && /STREAM2/.test(e.delta ?? ""), 40))
b.socket.destroy()
await Bun.sleep(100)
const c = controller("C", 2000, { answer: true })
console.log("C saw the prompt's answer:", await until((e) => e.role === "C" && e.response === 4, 40))
c.socket.destroy()

console.log("\n== events ==")
for (const e of events) console.log(JSON.stringify(e).slice(0, 220))
console.log("\n== journal (attaches) ==")
for (const line of fs.readFileSync(path.join(dir, "journal.log"), "utf8").trim().split("\n")) if (/attach|request_permission|"exit"/.test(line)) console.log(line.slice(0, 220))
console.log("\n== files ==", fs.readdirSync(work), "\n== model requests ==", JSON.stringify(requests), "\ntools:", JSON.stringify(tools))
server.stop(true)
if (!process.env.KEEP) fs.rmSync(dir, { recursive: true, force: true })
process.exit(0)
