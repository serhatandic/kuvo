// Spike S1: can a fresh Agent SDK query() take over a Claude Code CLI that another (killed) query()
// started, mid-turn? Real Claude Code, a stub Messages API (no sign-in: a made-up API key, a
// temporary HOME and CLAUDE_CONFIG_DIR, so your own sign-in is never read).
// usage: bun s1-claude.mjs <sdk.mjs> <claude executable>
import { spawn } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

const [sdk, claude] = process.argv.slice(2)
const dir = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), "s1-"))
fs.mkdirSync(path.join(dir, "work"))
fs.mkdirSync(path.join(dir, "home/.claude"), { recursive: true })
const requests = []

const sse = (events, slow) =>
  new Response(
    new ReadableStream({
      async start(controller) {
        for (const event of events) {
          controller.enqueue(new TextEncoder().encode(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`))
          if (slow && event.type === "content_block_delta") await Bun.sleep(slow)
        }
        controller.close()
      },
    }),
    { headers: { "content-type": "text/event-stream" } },
  )
const start = { type: "message_start", message: { id: "msg_" + Math.random().toString(36).slice(2), type: "message", role: "assistant", model: "claude-sonnet-4-5", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 1 } } }
const toolUse = (id, command) => [
  start,
  { type: "content_block_start", index: 0, content_block: { type: "tool_use", id, name: "Bash", input: {} } },
  { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify({ command, description: command }) } },
  { type: "content_block_stop", index: 0 },
  { type: "message_delta", delta: { stop_reason: "tool_use", stop_sequence: null }, usage: { output_tokens: 10 } },
  { type: "message_stop" },
]
const text = (chunks) => [
  start,
  { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
  ...chunks.map((chunk) => ({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: chunk } })),
  { type: "content_block_stop", index: 0 },
  { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 10 } },
  { type: "message_stop" },
]

const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname.endsWith("/count_tokens")) return Response.json({ input_tokens: 10 })
    if (!url.pathname.endsWith("/v1/messages")) return Response.json({})
    const body = await request.json()
    const main = (body.tools ?? []).some((tool) => tool.name === "Bash")
    const results = JSON.stringify(body.messages).match(/"tool_result"/g)?.length ?? 0
    requests.push({ main, results })
    if (!main) return body.stream ? sse(text(["ok"])) : Response.json({ ...start.message, content: [{ type: "text", text: "ok" }], stop_reason: "end_turn" })
    if (results === 0) return sse(toolUse("toolu_one", "echo one > one.txt"))
    if (results === 1) return sse(toolUse("toolu_two", "echo two > two.txt"))
    return sse(text(Array.from({ length: 10 }, (_, i) => `STREAM${i + 1} `)), 400)
  },
})

const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_") && !name.startsWith("CLAUDE") && !name.startsWith("ANTHROPIC")))
Object.assign(env, {
  HOME: path.join(dir, "home"), CLAUDE_CONFIG_DIR: path.join(dir, "home/.claude"),
  ANTHROPIC_API_KEY: "sk-ant-spike-not-a-key", ANTHROPIC_BASE_URL: `http://127.0.0.1:${server.port}`,
  SPIKE_REPLAY: process.env.SPIKE_REPLAY ?? "all", DISABLE_AUTOUPDATER: "1", DISABLE_TELEMETRY: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
})
fs.writeFileSync(path.join(dir, "env.json"), JSON.stringify(env))

const events = () => (fs.existsSync(path.join(dir, "events.log")) ? fs.readFileSync(path.join(dir, "events.log"), "utf8").trim().split("\n").map((line) => JSON.parse(line)) : [])
const until = async (what, seconds = 60) => {
  for (let i = 0; i < seconds * 20; i++) {
    if (events().some(what)) return true
    await Bun.sleep(50)
  }
  return false
}
const controller = (role) => {
  const child = spawn(process.execPath, [path.join(import.meta.dirname, "s1-controller.mjs"), role, dir, sdk, claude], { stdio: "inherit" })
  child.gone = new Promise((resolve) => child.once("exit", resolve))
  return child
}

const a = controller("A")
const asked = await until((e) => e.role === "A" && (process.env.SPIKE_HANG === "hook" ? e.hook : e.canUseTool))
console.log("A asked to use a tool:", asked)
a.kill("SIGKILL")
await a.gone
console.log("A killed; holder and CLI alive:", fs.readFileSync(path.join(dir, "hold.sock.pid"), "utf8").trim().split(" ").map((pid) => { try { process.kill(Number(pid), 0); return true } catch { return false } }))

const b = controller("B")
const streaming = await until((e) => e.role === "B" && /STREAM2/.test(e.text ?? ""), 60)
console.log("B saw the stream:", streaming)
b.kill("SIGKILL")
await b.gone

const c = controller("C")
const ended = await until((e) => e.role === "C" && (e.message === "result" || e.error), 60)
console.log("C saw the turn end:", ended)
c.kill("SIGKILL")

const [holder, cli] = fs.readFileSync(path.join(dir, "hold.sock.pid"), "utf8").trim().split(" ").map(Number)
try { process.kill(holder, "SIGTERM") } catch {}
await Bun.sleep(1000)
const alive = (pid) => { try { process.kill(pid, 0); return true } catch { return false } }
console.log("after the holder closed its stdin, CLI alive:", alive(cli))
for (const pid of [holder, cli]) try { process.kill(pid, "SIGKILL") } catch {}
console.log("\n== events (controllers) ==")
for (const e of events()) console.log(JSON.stringify(e).slice(0, 260))
console.log("\n== journal (holder) ==")
for (const line of fs.readFileSync(path.join(dir, "journal.log"), "utf8").trim().split("\n")) console.log(line.slice(0, 260))
console.log("\n== files ==", fs.readdirSync(path.join(dir, "work")), "\n== model requests ==", JSON.stringify(requests))
server.stop(true)
if (!process.env.KEEP) fs.rmSync(dir, { recursive: true, force: true })
else console.log("kept", dir)
process.exit(0)
