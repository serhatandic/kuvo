// Spike S2: can a new controller take over a `codex app-server` turn that another (killed)
// controller started, mid-turn, with an approval pending? Real Codex, a stub Responses API (a
// temporary HOME and CODEX_HOME with a made-up key for a stub provider: your sign-in is never read).
//   MODE=holder  the app-server on stdio under hold-spike.mjs (Codex sees one connection throughout)
//   MODE=native  Codex's own `--listen unix://`: each controller is a new connection
// usage: MODE=holder|native bun s2-codex.mjs <codex executable>
import { spawn } from "node:child_process"
import fs from "node:fs"
import net from "node:net"
import os from "node:os"
import path from "node:path"

const [codex] = process.argv.slice(2)
const mode = process.env.MODE ?? "holder"
const dir = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), "s2-"))
const work = path.join(dir, "work")
fs.mkdirSync(work)
fs.mkdirSync(path.join(dir, "home/.codex"), { recursive: true })
const requests = []
const events = []
const say = (role, what) => events.push({ role, at: Date.now(), ...what })

const sse = (list, slow) =>
  new Response(
    new ReadableStream({
      async start(controller) {
        for (const event of list) {
          controller.enqueue(new TextEncoder().encode(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`))
          if (slow && event.type === "response.output_text.delta") await Bun.sleep(slow)
        }
        controller.close()
      },
    }),
    { headers: { "content-type": "text/event-stream" } },
  )
const done = (id) => ({ type: "response.completed", response: { id, usage: { input_tokens: 10, input_tokens_details: null, output_tokens: 5, output_tokens_details: null, total_tokens: 15 } } })
let tools = []
const call = (id, command) => {
  const shell = tools.includes("shell_command") ? ["shell_command", { command }] : tools.includes("exec_command") ? ["exec_command", { cmd: command }] : ["shell", { command: ["bash", "-lc", command] }]
  return [
    { type: "response.created", response: { id } },
    { type: "response.output_item.done", item: { type: "function_call", name: shell[0], arguments: JSON.stringify(shell[1]), call_id: "call_" + id } },
    done(id),
  ]
}
const text = (id, chunks) => [
  { type: "response.created", response: { id } },
  { type: "response.output_item.added", item: { type: "message", role: "assistant", id: "m" + id, content: [] } },
  ...chunks.map((delta) => ({ type: "response.output_text.delta", delta, item_id: "m" + id, output_index: 0, content_index: 0 })),
  { type: "response.output_item.done", item: { type: "message", role: "assistant", id: "m" + id, content: [{ type: "output_text", text: chunks.join("") }] } },
  done(id),
]
const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    if (!url.pathname.endsWith("/responses")) return Response.json({ data: [] })
    const body = await request.json()
    tools = (body.tools ?? []).map((tool) => tool.name ?? tool.type)
    const outputs = JSON.stringify(body.input).match(/"function_call_output"/g)?.length ?? 0
    requests.push({ outputs, tools: tools.slice(0, 12) })
    if (outputs === 0) return sse(call("r1", "echo one > one.txt"))
    return sse(text("r2", Array.from({ length: 10 }, (_, i) => `STREAM${i + 1} `)), 400)
  },
})

const home = path.join(dir, "home")
fs.writeFileSync(path.join(home, ".codex/config.toml"), `model = "gpt-5-codex"\nmodel_provider = "stub"\n[model_providers.stub]\nname = "stub"\nbase_url = "http://127.0.0.1:${server.port}/v1"\nwire_api = "responses"\nenv_key = "STUB_API_KEY"\n[projects."${work}"]\ntrust_level = "trusted"\n`)
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_") && !name.startsWith("CODEX") && !name.startsWith("OPENAI")))
Object.assign(env, { HOME: home, CODEX_HOME: path.join(home, ".codex"), STUB_API_KEY: "not-a-key", SPIKE_REPLAY: "all" })

const sock = path.join(dir, "hold.sock")
let pids = []
if (mode === "holder") {
  spawn(process.execPath, [path.join(import.meta.dirname, "hold-spike.mjs"), sock, "jsonrpc", path.join(dir, "journal.log"), "--", codex, "app-server"], { cwd: work, env, detached: true, stdio: "ignore" }).unref()
} else {
  const daemon = spawn(codex, ["app-server", "--listen", `unix://${sock}`], { cwd: work, env, detached: true, stdio: ["ignore", "ignore", "pipe"] })
  daemon.stderr.on("data", (chunk) => say("server", { stderr: String(chunk).slice(0, 300) }))
  pids = [daemon.pid]
}
process.on("exit", () => { for (const pid of pids) try { process.kill(pid, "SIGKILL") } catch {} })
setTimeout(() => { console.log("gave up"); process.exit(1) }, 90_000)
for (let i = 0; i < 200 && !fs.existsSync(sock); i++) await Bun.sleep(50)
if (mode === "holder") pids = fs.readFileSync(sock + ".pid", "utf8").trim().split(" ").map(Number)

/** A controller: a JSON-RPC client on the socket, ids from `first`. */
function controller(role, first, { answer }) {
  // (Codex's own socket is reached through its proxy, on the proxy's stdio.)
  const proxy = mode === "native" ? spawn(codex, ["app-server", "proxy", "--sock", sock], { env, stdio: ["pipe", "pipe", "ignore"] }) : undefined
  const socket = proxy
    ? Object.assign(proxy.stdout, { write: (data) => proxy.stdin.write(data), destroy: () => proxy.kill("SIGKILL") })
    : net.connect(sock)
  let next = first
  const waiting = new Map()
  const send = (message) => socket.write(JSON.stringify(message) + "\n")
  let buffer = ""
  socket.setEncoding("utf8")
  socket.on("error", (error) => say(role, { socketError: String(error) }))
  socket.on("data", (chunk) => {
    buffer += chunk
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at)
      buffer = buffer.slice(at + 1)
      let message
      try { message = JSON.parse(line) } catch { continue }
      if (message.method && message.id !== undefined) {
        say(role, { request: message.method, id: message.id, command: message.params?.command })
        if (answer) send({ id: message.id, result: { decision: "accept" } })
      } else if (message.method) {
        if (message.method === "item/agentMessage/delta") say(role, { delta: message.params?.delta })
        else if (/^(turn|item)\/(started|completed)$|^error$|^thread\/status/.test(message.method)) say(role, { note: message.method, item: message.params?.item?.type, status: message.params?.turn?.status ?? message.params?.status, text: message.params?.item?.text, error: message.params?.error?.message })
      } else if (message.id !== undefined) {
        const resolve = waiting.get(message.id)
        say(role, { response: message.id, mine: Boolean(resolve), error: message.error?.message, result: JSON.stringify(message.result ?? null).slice(0, 120) })
        resolve?.(message)
        waiting.delete(message.id)
      }
    }
  })
  const request = (method, params) => new Promise((resolve) => { const id = next++; waiting.set(id, resolve); send({ id, method, params }) })
  return { socket, request, notify: (method, params) => send({ method, params }) }
}
const until = async (what, seconds = 60) => {
  for (let i = 0; i < seconds * 20; i++) {
    if (events.some(what)) return true
    await Bun.sleep(50)
  }
  return false
}
const initialize = { clientInfo: { name: "spike", title: "Spike", version: "1" }, capabilities: { experimentalApi: true } }

const a = controller("A", 1, { answer: false })
console.log("A initialize:", JSON.stringify((await a.request("initialize", initialize)).error ?? "ok"))
a.notify("initialized")
const thread = (await a.request("thread/start", { cwd: work, approvalPolicy: "untrusted", sandbox: "workspace-write" })).result?.thread?.id
console.log("thread:", thread)
await a.request("turn/start", { threadId: thread, input: [{ type: "text", text: "write one, then talk", text_elements: [] }] })
console.log("A was asked to approve:", await until((e) => e.role === "A" && e.request))
a.socket.destroy() // A dies (its socket closes as a killed process's would).
await Bun.sleep(300)
const alive = (pid) => { try { process.kill(pid, 0); return true } catch { return false } }
console.log("after A died, server/holder processes alive:", pids.map(alive))

const b = controller("B", 1000, { answer: true })
const init = await b.request("initialize", initialize)
console.log("B initialize again:", JSON.stringify(init.error ?? "ok"))
b.notify("initialized")
if (mode === "native") console.log("B thread/resume:", JSON.stringify(((await b.request("thread/resume", { threadId: thread })).error ?? "ok")).slice(0, 200))
console.log("B was asked to approve:", await until((e) => e.role === "B" && e.request, 20))
console.log("B saw the stream:", await until((e) => e.role === "B" && /STREAM2/.test(e.delta ?? ""), 30))
b.socket.destroy()
await Bun.sleep(100)

const c = controller("C", 2000, { answer: true })
if (mode === "native") {
  await c.request("initialize", initialize)
  c.notify("initialized")
  await c.request("thread/resume", { threadId: thread })
}
console.log("C saw the turn end:", await until((e) => e.role === "C" && e.note === "turn/completed", 30))
c.socket.destroy()

for (const pid of pids) try { process.kill(pid, "SIGTERM") } catch {}
await Bun.sleep(1000)
console.log("after SIGTERM to the holder/server, alive:", pids.map(alive))
for (const pid of pids) try { process.kill(pid, "SIGKILL") } catch {}
console.log("\n== events ==")
for (const e of events) console.log(JSON.stringify(e).slice(0, 240))
if (mode === "holder") {
  console.log("\n== journal (attaches and requests) ==")
  for (const line of fs.readFileSync(path.join(dir, "journal.log"), "utf8").trim().split("\n")) if (/attach|requestApproval|"exit"/.test(line)) console.log(line.slice(0, 240))
}
console.log("\n== files ==", fs.readdirSync(work), "\n== model requests ==", JSON.stringify(requests.map((r) => r.outputs)), "tools:", JSON.stringify(requests[0]?.tools))
server.stop(true)
if (!process.env.KEEP) fs.rmSync(dir, { recursive: true, force: true })
else console.log("kept", dir)
process.exit(0)
