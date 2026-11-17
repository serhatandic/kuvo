// An agent's process under a holder (src/agents/hold, step 8 of the update design): the bridge that
// started a turn is killed mid-turn, with an approval open; the agent lives on; a new bridge attaches
// with the next generation, answers the approval, and the turn ends normally, the command run once.
// A newer bridge fences off the older one, a stale or keyless one is refused, and once the agent
// ends the holder goes with its files. Real processes throughout: the holder, a stand-in agent that
// speaks JSON-RPC as Codex's app-server does, and the first bridge as a process of its own (RPC over
// the holder, as the bridge runs it), killed with SIGKILL. (The stand-ins are scripts on the run's
// own Bun, which on Linux is the engine's binary, Bun only with BUN_BE_BUN: given to them as an
// agent's own setting, which a holder keeps.)

import "../safety"
import { afterAll, describe, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, watch, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { attach, hold, heldIn, holdFolder, HOLDER, type HoldRecord } from "../../src/agents/hold/client"
import { RPC } from "../../src/agents/rpc"

// (In the run's own home: short enough for a socket's path, and never /tmp.)
const root = mkdtempSync(join(process.env.HOME!, "h"))
const folders: string[] = []
// (Holders run in sessions of their own: any a failed test left are stopped, by their own pid.)
afterAll(() => {
  for (const folder of folders) for (const record of heldIn(folder)) process.kill(record.pid, "SIGTERM")
  rmSync(root, { recursive: true, force: true })
})

/** A Codex-like agent: `turn/start` asks to run a command, runs it once approved, and ends the turn. */
const AGENT = String.raw`
const fs = require("node:fs");
const [log] = process.argv.slice(2);
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
let buffer = "";
process.stdin.on("end", () => process.exit(0));
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    fs.appendFileSync(log, JSON.stringify(message) + "\n");
    if (message.method === "turn/start") {
      send({ id: message.id, result: { turn: { id: "u1", status: "inProgress" } } });
      send({ method: "turn/started", params: { turn: { id: "u1" } } });
      send({ id: 0, method: "item/commandExecution/requestApproval", params: { turnId: "u1", command: "echo one" } });
    } else if (message.id === 0 && message.result) {
      fs.appendFileSync(log + ".ran", message.result.decision + "\n");
      send({ method: "item/completed", params: { turnId: "u1", item: { type: "agentMessage", text: "Done" } } });
      send({ method: "turn/completed", params: { turn: { id: "u1", status: "completed" } } });
    } else if (message.method === "ping") send({ id: message.id, result: "pong" });
  }
});
`

/** The first bridge, as a process of its own: starts the agent under a holder, starts a turn, and never answers the approval. */
const FIRST_BRIDGE = String.raw`
const [client, rpcModule, folder, agent, log] = process.argv.slice(2);
const { hold } = await import(client);
const { RPC } = await import(rpcModule);
const held = await hold([process.execPath, agent, log], { folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH, HOME: process.env.HOME }, meta: { provider: "codex", session: "ses_1" } });
const rpc = new RPC(held);
rpc.onRequest = (method) => {
  console.log(JSON.stringify({ asked: method, id: held.id, pid: held.pid }));
  return new Promise(() => {});
};
rpc.start();
await rpc.request("turn/start", { threadId: "t1", input: [] });
`

/** A test's own short folder for its holders: never the fallback in /tmp (`holdFolder`). */
function shortFolder() {
  const dir = mkdtempSync(join(root, "/"))
  const folder = join(dir, "h")
  if (Buffer.byteLength(folder) + 18 >= 104) throw new Error(`This run's home is too deep for a socket's path: ${folder}`)
  folders.push(folder)
  return { dir, folder }
}

function files() {
  const { dir, folder } = shortFolder()
  const agent = join(dir, "agent.js")
  const bridge = join(dir, "bridge.mjs")
  writeFileSync(agent, AGENT)
  writeFileSync(bridge, FIRST_BRIDGE)
  return { dir, agent, bridge, log: join(dir, "agent.log"), folder }
}

const alive = (pid: number) => {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/** Starts the first bridge, and waits for it to say the agent asked to run its command. */
async function firstBridge(paths: ReturnType<typeof files>) {
  const bridge = spawn(process.execPath, [paths.bridge, join(import.meta.dir, "../../src/agents/hold/client.ts"), join(import.meta.dir, "../../src/agents/rpc.ts"), paths.folder, paths.agent, paths.log], { stdio: ["ignore", "pipe", "inherit"] })
  const gone = new Promise<void>((resolve) => bridge.once("exit", () => resolve()))
  const asked = await new Promise<{ asked: string; id: string; pid: number }>((resolve, reject) => {
    let said = ""
    bridge.stdout!.setEncoding("utf8")
    bridge.stdout!.on("data", (chunk: string) => {
      said += chunk
      if (said.includes("\n")) resolve(JSON.parse(said.split("\n")[0]!))
    })
    bridge.once("exit", (code) => reject(new Error(`The first bridge ended (${code}) before the agent asked.`)))
  })
  return { bridge, gone, asked }
}

const recordOf = (folder: string, id: string) => heldIn(folder).find((record) => record.id === id)!

describe("an agent under a holder", () => {
  test("outlives its bridge's SIGKILL mid-turn, and a new bridge takes the turn to its end", async () => {
    const paths = files()
    const { bridge, gone, asked } = await firstBridge(paths)
    expect(asked.asked).toBe("item/commandExecution/requestApproval")

    bridge.kill("SIGKILL")
    await gone
    expect(alive(asked.pid)).toBe(true)
    const record = recordOf(paths.folder, asked.id)
    expect(record).toMatchObject({ proto: "hold/1", holder: HOLDER, kind: "jsonrpc", child: asked.pid, generation: 1, meta: { provider: "codex", session: "ses_1" } })
    // (Only this user reaches the holder: its folder and its record, which has the key.)
    expect(statSync(paths.folder).mode & 0o777).toBe(0o700)
    expect(statSync(join(paths.folder, `${asked.id}.json`)).mode & 0o777).toBe(0o600)

    // The new bridge: the open approval is asked again, answered, and the turn ends.
    const held = await attach(paths.folder, record)
    expect(held.generation).toBe(2)
    expect(held.pid).toBe(asked.pid)
    const rpc = new RPC(held, { firstID: 1_000_000 })
    const asks: string[] = []
    rpc.onRequest = async (method) => {
      asks.push(method)
      return { decision: "accept" }
    }
    const ended = new Promise<string>((resolve) => {
      rpc.onNotification = (method, params) => {
        if (method === "turn/completed") resolve(params.turn.status)
      }
    })
    rpc.start()
    expect(await ended).toBe("completed")
    expect(asks).toEqual(["item/commandExecution/requestApproval"])
    expect(readFileSync(paths.log + ".ran", "utf8")).toBe("accept\n")
    // (It still answers this bridge.)
    expect(await rpc.request<string>("ping")).toBe("pong")

    // Its stdin closed, as a bridge stops it: the agent ends, this bridge hears it, and the holder
    // goes with its files.
    held.end()
    expect(await rpc.exited).toBe(0)
    await held.closed
    expect(readdirSync(paths.folder).filter((name) => name.startsWith(asked.id))).toEqual([])
    expect(alive(asked.pid)).toBe(false)
  })

  test("a newer bridge fences off the older one; a stale generation or a wrong key is refused", async () => {
    const paths = files()
    const { bridge, gone, asked } = await firstBridge(paths)
    const record = recordOf(paths.folder, asked.id)

    // A bridge of the same generation as the one attached, or without the key, is turned away.
    await expect(attach(paths.folder, { ...record, generation: 0 })).rejects.toThrow(/generation 1/)
    await expect(attach(paths.folder, { ...record, key: "0".repeat(48) })).rejects.toThrow(/key/)

    const second = await attach(paths.folder, record)
    const secondRPC = new RPC(second, { firstID: 1_000_000 })
    secondRPC.onRequest = () => new Promise(() => {})
    secondRPC.start()
    // (The first bridge, fenced off as the second attached, still runs: it's ended here.)
    bridge.kill("SIGKILL")
    await gone

    const third = await attach(paths.folder, recordOf(paths.folder, asked.id))
    expect(third.generation).toBe(3)
    await second.closed
    expect(second.fenced).toBe(true)
    // The fenced bridge's requests fail at once, and none of it reaches the agent.
    await expect(secondRPC.request("ping")).rejects.toThrow()
    expect(readFileSync(paths.log, "utf8")).not.toContain('"ping"')
    // The one now attached is asked the open approval, and its answer is the one that counts.
    const thirdRPC = new RPC(third, { firstID: 2_000_000 })
    thirdRPC.onRequest = async () => ({ decision: "decline" })
    const ended = new Promise<void>((resolve) => (thirdRPC.onNotification = (method) => method === "turn/completed" && resolve()))
    thirdRPC.start()
    await ended
    expect(readFileSync(paths.log + ".ran", "utf8")).toBe("decline\n")
    // A stale generation still can't come back.
    await expect(attach(paths.folder, { ...record, generation: 1 })).rejects.toThrow(/generation 3/)
    thirdRPC.kill()
    await third.closed
  })

  test("a bridge that attaches after the agent ended hears its end, and the holder goes", async () => {
    const paths = files()
    const { bridge, gone, asked } = await firstBridge(paths)
    bridge.kill("SIGKILL")
    await gone
    const record: HoldRecord = recordOf(paths.folder, asked.id)
    // The agent ends with no bridge attached (its stdin closed through a bridge that attaches and lets go).
    const closing = await attach(paths.folder, record)
    closing.end()
    await closing.exited
    expect(alive(asked.pid)).toBe(false)
    await closing.closed
    expect(existsSync(join(paths.folder, `${asked.id}.sock`))).toBe(false)
  })
})

test("an agent no bridge attaches to is stopped, as a bridge stops one, and its holder goes", async () => {
  const paths = files()
  const held = await hold([process.execPath, paths.agent, paths.log], { folder: paths.folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH }, orphan: 100 })
  const record = join(paths.folder, `${held.id}.json`)
  const gone = untilGone(record)
  // (Let go of, as a bridge that crashed lets go: no newer one comes.)
  held.detach()
  await gone
  expect(alive(held.pid!)).toBe(false)
  expect(existsSync(join(paths.folder, `${held.id}.sock`))).toBe(false)
})

/** Once `path` is gone (its folder watched, not polled). */
function untilGone(path: string) {
  return new Promise<void>((resolve) => {
    const watcher = watch(dirname(path), () => {
      if (existsSync(path)) return
      watcher.close()
      resolve()
    })
    if (!existsSync(path)) {
      watcher.close()
      resolve()
    }
  })
}

describe("what a holder sends again to a new bridge", () => {
  /** An agent that says one line per line it reads, the first a request (with an id). */
  const ECHO = String.raw`
let n = 0;
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  for (const line of chunk.split("\n").filter(Boolean)) {
    n++;
    process.stdout.write(JSON.stringify(n === 1 ? { type: "control_request", request_id: "r1", id: 7, method: "ask" } : { said: line }) + "\n");
  }
});
process.stdin.on("end", () => process.exit(0));
`

  async function sendAgain(kind: "jsonrpc" | "claude") {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "echo.js")
    writeFileSync(agent, ECHO)
    const first = await hold([process.execPath, agent], { folder, kind, env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    const seen: string[] = []
    const asked = new Promise<void>((resolve) => first.onLine((line) => (seen.push(line), resolve())))
    first.write("go")
    await asked
    // (It took the ask in, and says so as it lets go: the next bridge starts after it.)
    first.detach()
    await first.closed
    const record = heldIn(folder).find((each) => each.id === first.id)!
    const second = await attach(folder, record)
    const again: { line: string; again: boolean }[] = []
    second.onLine((line, isAgain) => again.push({ line, again: isAgain }))
    // A line said after it attached comes as itself: whatever came again came before it.
    const later = new Promise<void>((resolve) => second.onLine((line) => line.includes("later") && resolve()))
    second.write("later")
    await later
    second.end()
    await second.closed
    return again
  }

  test("JSON-RPC: what it asked and no bridge answered", async () => {
    const again = await sendAgain("jsonrpc")
    expect(again[0]).toEqual({ line: JSON.stringify({ type: "control_request", request_id: "r1", id: 7, method: "ask" }), again: true })
    expect(again.slice(1).map((each) => each.again)).toEqual([false])
  })

  test("Claude Code: nothing (its CLI asks again itself as the new SDK client initializes)", async () => {
    const again = await sendAgain("claude")
    expect(again.map((each) => each.again)).toEqual([false])
    expect(again[0]!.line).toContain("later")
  })
})

test("a box's holders are in its run folder; one too deep for a socket's path on macOS goes in Kuvo Code's short folder in /tmp", () => {
  expect(holdFolder("/home/me/.kuvo-code")).toBe("/home/me/.kuvo-code/run/agents/hold")
  const deep = `/Users/me/${"deep/".repeat(20)}.kuvo-code`
  if (process.platform === "darwin") expect(holdFolder(deep)).toMatch(/^\/private\/tmp\/kuvo-ssh-\d+\/hold-[0-9a-f]{8}$/)
  else expect(holdFolder(deep)).toBeUndefined()
})
