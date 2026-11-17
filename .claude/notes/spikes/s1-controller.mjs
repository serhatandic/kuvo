// S1 controller: a fresh Agent SDK query() whose Claude Code process is the holder's.
// role A starts the holder (and the CLI under it) and sends the prompt; B and C only attach.
// usage: bun s1-controller.mjs <role> <dir> <sdk.mjs> <claude executable>
import { spawn } from "node:child_process"
import { EventEmitter } from "node:events"
import fs from "node:fs"
import net from "node:net"
import path from "node:path"

const [role, dir, sdkPath, claude] = process.argv.slice(2)
const { query } = await import(sdkPath)
const sock = path.join(dir, "hold.sock")
const say = (what) => fs.appendFileSync(path.join(dir, "events.log"), JSON.stringify({ role, at: Date.now(), ...what }) + "\n")

function attach(options) {
  if (role === "A") {
    const holder = spawn(process.execPath, [path.join(import.meta.dirname, "hold-spike.mjs"), sock, "claude", path.join(dir, "journal.log"), "--", options.command, ...options.args], {
      cwd: options.cwd, env: options.env, detached: true, stdio: "ignore",
    })
    holder.unref()
    say({ spawned: [options.command, ...options.args].join(" ").slice(0, 400) })
  }
  // (spawnClaudeCodeProcess is synchronous: wait for the holder's socket here.)
  const nap = new Int32Array(new SharedArrayBuffer(4))
  for (let i = 0; i < 200 && !fs.existsSync(sock); i++) Atomics.wait(nap, 0, 0, 50)
  const process_ = new EventEmitter()
  const socket = net.connect(sock)
  socket.on("error", (error) => say({ socketError: String(error) }))
  socket.on("close", () => { process_.exitCode = 0; process_.emit("exit", 0, null) })
  Object.assign(process_, { stdin: socket, stdout: socket, killed: false, exitCode: null, kill: () => (socket.destroy(), true) })
  return process_
}

async function* prompt() {
  if (role === "A") yield { type: "user", message: { role: "user", content: "Run echo one, then echo two, then say a long thing." }, parent_tool_use_id: null, session_id: "" }
  await new Promise(() => {})
}

const q = query({
  prompt: prompt(),
  options: {
    pathToClaudeCodeExecutable: claude,
    cwd: path.join(dir, "work"),
    includePartialMessages: true,
    permissionMode: "default",
    env: JSON.parse(fs.readFileSync(path.join(dir, "env.json"), "utf8")),
    canUseTool: async (name, input, options) => {
      say({ canUseTool: name, input, toolUseID: options?.toolUseID })
      if (role === "A") return new Promise(() => {}) // A is killed while this is open.
      return { behavior: "allow", updatedInput: input }
    },
    hooks: { PreToolUse: [{ hooks: [async (input) => {
      say({ hook: input.tool_name, command: input.tool_input?.command })
      if (role === "A" && process.env.SPIKE_HANG === "hook") return new Promise(() => {}) // A is killed while this is open.
      return { continue: true }
    }] }] },
    spawnClaudeCodeProcess: attach,
    stderr: (text) => say({ stderr: text.slice(0, 300) }),
  },
})

try {
  for await (const message of q) {
    if (message.type === "stream_event") {
      const delta = message.event?.delta
      if (delta?.type === "text_delta") say({ text: delta.text })
      continue
    }
    say({ message: message.type, subtype: message.subtype, result: message.result, is_error: message.is_error, num_turns: message.num_turns })
    if (message.type === "result") process.exit(0)
  }
  say({ ended: "iterator done" })
} catch (error) {
  say({ error: String(error?.message ?? error).slice(0, 500) })
  process.exit(1)
}
