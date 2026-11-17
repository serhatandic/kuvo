// Spike holder (S1/S2): runs an agent program on pipes, as its own child, and lets one controller
// at a time drive it over a unix socket. It survives its controller (spawned detached, its own
// session). On attach it replays what the controller must see again:
//   - claude: control requests the agent sent that no controller has answered yet;
//   - jsonrpc: requests the agent sent that no controller has answered yet.
// Every stdout line is journaled (seq) so the spike can show nothing was lost.
// usage: bun hold-spike.mjs <socket> <kind: claude|jsonrpc> <journal> -- <command> [args…]
import { spawn } from "node:child_process"
import fs from "node:fs"
import net from "node:net"

const [sock, kind, journal, , ...command] = process.argv.slice(2)
const child = spawn(command[0], command.slice(1), { stdio: ["pipe", "pipe", "pipe"] })
fs.writeFileSync(sock + ".pid", `${process.pid} ${child.pid}\n`)
const pending = new Map() // id -> line, the agent's requests still unanswered
let seq = 0
let controller
const log = (what) => fs.appendFileSync(journal, JSON.stringify({ seq, at: Date.now(), ...what }) + "\n")

const idOf = (message, from) => {
  if (kind === "claude") {
    if (from === "agent" && message.type === "control_request") return ["ask", message.request_id]
    if (from === "controller" && message.type === "control_response") return ["answer", message.response?.request_id]
    return []
  }
  if (from === "agent" && message.method !== undefined && message.id !== undefined) return ["ask", message.id]
  if (from === "controller" && message.method === undefined && message.id !== undefined) return ["answer", message.id]
  return []
}

const lines = (stream, each) => {
  let buffer = ""
  stream.setEncoding("utf8")
  stream.on("data", (chunk) => {
    buffer += chunk
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at)
      buffer = buffer.slice(at + 1)
      if (line.trim()) each(line)
    }
  })
}

lines(child.stdout, (line) => {
  seq++
  let message
  try { message = JSON.parse(line) } catch {}
  const [what, id] = message ? idOf(message, "agent") : []
  if (what === "ask") pending.set(id, line)
  log({ from: "agent", line: line.slice(0, 300) })
  controller?.write(line + "\n")
})
child.stderr.on("data", () => {})

net.createServer((socket) => {
  // The newest controller fences off the older one.
  controller?.destroy()
  controller = socket
  log({ attach: true, replay: [...pending.keys()] })
  lines(socket, (line) => {
    if (controller !== socket) return
    let message
    try { message = JSON.parse(line) } catch {}
    const [what, id] = message ? idOf(message, "controller") : []
    if (what === "answer") pending.delete(id)
    log({ from: "controller", line: line.slice(0, 300) })
    child.stdin.write(line + "\n")
  })
  socket.on("error", () => {})
  socket.on("close", () => { if (controller === socket) controller = undefined })
  // SPIKE_REPLAY=none: replay nothing; =hooks: only what isn't a permission ask (the CLI re-sends those).
  const replay = process.env.SPIKE_REPLAY ?? "all"
  for (const line of pending.values()) if (replay === "all" || (replay === "hooks" && !line.includes('"can_use_tool"'))) socket.write(line + "\n")
}).listen(sock)

child.on("exit", (code, signal) => {
  log({ exit: code, signal })
  controller?.end()
  try { fs.unlinkSync(sock) } catch {}
  process.exit(0)
})
process.on("SIGTERM", () => child.stdin.end())
