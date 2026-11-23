// Claude Code under a holder, driven through the Agent SDK as the bridge drives it: the bridge's SDK
// client is killed (SIGKILL) while Claude Code waits on a permission ask; Claude Code lives on; a new
// SDK `query()` attaches to the same process with the next generation, is asked again, answers, and
// the turn ends with its result, the command run once. The stand-in CLI does on a second
// `initialize` what Claude Code 2.1.325 did in spike S1: it
// accepts it and names the open ask in `pending_permission_requests`. The ask is asked once: the
// holder never replays Claude Code's control requests, which it asks again itself.

import "../safety"
import { afterAll, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { attach, heldIn, heldProcess } from "../../src/agents/hold/client"

const sdk = join(import.meta.dir, "../../node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs")
const root = mkdtempSync(join(process.env.HOME!, "c"))
const folders: string[] = []
// (Holders run in sessions of their own: any a failed test left are stopped, by their own pid.)
afterAll(() => {
  for (const folder of folders) for (const record of heldIn(folder)) process.kill(record.pid, "SIGTERM")
  rmSync(root, { recursive: true, force: true })
})

const CLI = String.raw`#!/usr/bin/env node
const fs = require("node:fs");
const ran = process.env.STAND_IN_RAN;
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
const ask = { type: "control_request", request_id: "p1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "echo one" }, tool_use_id: "toolu_1" } };
let open = false;
let buffer = "";
process.stdin.on("end", () => process.exit(0));
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.type === "control_request" && message.request.subtype === "initialize") {
      send({ type: "control_response", response: { subtype: "success", request_id: message.request_id, response: { commands: [], models: [], agents: [], account: {} }, pending_permission_requests: open ? [ask] : [] } });
    } else if (message.type === "user") {
      open = true;
      send(ask);
    } else if (message.type === "control_response" && message.response.request_id === "p1" && open) {
      open = false;
      fs.appendFileSync(ran, message.response.response.behavior + "\n");
      send({ type: "assistant", message: { id: "m1", role: "assistant", type: "message", model: "stand-in", content: [{ type: "text", text: "Done" }], stop_reason: "end_turn", usage: { input_tokens: 1, output_tokens: 1 } }, parent_tool_use_id: null, session_id: "s1", uuid: "00000000-0000-4000-8000-000000000001" });
      send({ type: "result", subtype: "success", is_error: false, result: "Done", num_turns: 1, duration_ms: 1, duration_api_ms: 1, total_cost_usd: 0, usage: { input_tokens: 1, output_tokens: 1 }, session_id: "s1", uuid: "00000000-0000-4000-8000-000000000002" });
    }
  }
});
`

/** The first bridge's SDK client, as a process of its own: Claude Code under a holder, a message sent, its ask never answered. */
const FIRST = String.raw`
const [sdk, client, folder, cli, ran] = process.argv.slice(2);
const { query } = await import(sdk);
const { hold, heldProcess } = await import(client);
async function* prompt() {
  yield { type: "user", message: { role: "user", content: "Run echo one" }, parent_tool_use_id: null, session_id: "" };
  await new Promise(() => {});
}
let held;
const q = query({
  prompt: prompt(),
  options: {
    pathToClaudeCodeExecutable: cli,
    canUseTool: async () => {
      console.log(JSON.stringify({ asked: true, id: (await held).id, pid: (await held).pid }));
      return new Promise(() => {});
    },
    spawnClaudeCodeProcess: (options) => heldProcess((held = hold([process.execPath, cli, ...options.args], { folder, kind: "claude", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH, HOME: process.env.HOME, STAND_IN_RAN: ran } }))),
    stderr: () => {},
  },
});
for await (const message of q) {}
`

test("Claude Code outlives its bridge's SIGKILL with an ask open, and a new SDK client takes the turn to its result", async () => {
  const dir = mkdtempSync(join(root, "/"))
  const folder = join(dir, "h")
  folders.push(folder)
  const cli = join(dir, "claude.js")
  const first = join(dir, "first.mjs")
  const ran = join(dir, "ran")
  writeFileSync(cli, CLI, { mode: 0o755 })
  writeFileSync(first, FIRST)

  const bridge = spawn(process.execPath, [first, sdk, join(import.meta.dir, "../../src/agents/hold/client.ts"), folder, cli, ran], { stdio: ["ignore", "pipe", "inherit"] })
  const gone = new Promise<void>((resolve) => bridge.once("exit", () => resolve()))
  const asked = await new Promise<{ id: string; pid: number }>((resolve, reject) => {
    let said = ""
    bridge.stdout!.setEncoding("utf8")
    bridge.stdout!.on("data", (chunk: string) => {
      said += chunk
      if (said.includes("\n")) resolve(JSON.parse(said.split("\n")[0]!))
    })
    bridge.once("exit", (code) => reject(new Error(`The first bridge ended (${code}) before Claude Code asked.`)))
  })
  bridge.kill("SIGKILL")
  await gone
  process.kill(asked.pid, 0)

  // The new bridge's SDK client, on the same Claude Code.
  const { query } = await import(sdk)
  const record = heldIn(folder).find((each) => each.id === asked.id)!
  const attaching = attach(folder, record)
  const asks: string[] = []
  async function* nothing() {
    await new Promise(() => {})
  }
  const q = query({
    prompt: nothing(),
    options: {
      pathToClaudeCodeExecutable: cli,
      canUseTool: async (name: string, input: Record<string, unknown>) => {
        asks.push(name)
        return { behavior: "allow", updatedInput: input }
      },
      spawnClaudeCodeProcess: () => heldProcess(attaching),
      stderr: () => {},
    },
  })
  let result: any
  for await (const message of q) {
    if (message.type === "result") {
      result = message
      break
    }
  }
  expect(result).toMatchObject({ subtype: "success", result: "Done" })
  expect(asks).toEqual(["Bash"])
  expect(readFileSync(ran, "utf8")).toBe("allow\n")
  const held = await attaching
  expect(held.generation).toBe(2)
  // Its stdin closed (or the SDK, done, stopping it): Claude Code ends, and the holder with it.
  held.end()
  await held.exited
  await held.closed
  expect(heldIn(folder)).toEqual([])
})
