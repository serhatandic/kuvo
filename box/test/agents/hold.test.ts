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
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, watch, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { attach, hold, heldIn, heldProcess, holdFolder, HOLDER, type HoldRecord } from "../../src/agents/hold/client"
import { forgetOldAntigravity } from "../../src/agents/agents/antigravity"
import { ANTIGRAVITY_ACP, adoptHeld } from "../../src/agents/hold/adopt"
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
    const lifecycle = readFileSync(join(paths.folder, "lifecycle.log"), "utf8").trim().split("\n").map(line => JSON.parse(line))
    expect(lifecycle.map(entry => entry.event)).toEqual([
      "holder listening", "controller attached", "controller disconnected",
      "controller attached", "controller closed stdin", "child exited",
    ])
    expect(lifecycle.at(-1)).toMatchObject({ id: asked.id, child: asked.pid, holder: record.pid, code: 0, signal: null, generation: 2 })
    expect(lifecycle.every(entry => Number.isFinite(Date.parse(entry.at)))).toBe(true)
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

  test("an agent that ends with no bridge attached: the next bridge hears its end, and the holder goes", async () => {
    const paths = files()
    const { bridge, gone, asked } = await firstBridge(paths)
    bridge.kill("SIGKILL")
    await gone
    const record: HoldRecord = recordOf(paths.folder, asked.id)
    // (Ended from outside, with no bridge there: the agent this test's holder started.)
    const ended = new Promise<void>((resolve) => {
      const watcher = watch(paths.folder, () => {
        if (recordOf(paths.folder, asked.id)?.exited) (watcher.close(), resolve())
      })
    })
    process.kill(asked.pid, "SIGTERM")
    await ended
    const later = await attach(paths.folder, record)
    expect(await later.exited).toEqual({ code: null, signal: "SIGTERM", heardAll: true })
    await later.closed
    expect(existsSync(join(paths.folder, `${asked.id}.sock`))).toBe(false)
  })

  test("a line far longer than the journal reaches the bridge whole, and the bridge stays attached", async () => {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "long.js")
    writeFileSync(agent, `process.stdin.once("data", () => process.stdout.write(JSON.stringify({ said: "x".repeat(5_000_000) }) + "\\n")); process.stdin.on("end", () => process.exit(0));`)
    const held = await hold([process.execPath, agent], { folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    const said = new Promise<string>((resolve) => held.onLine((line) => resolve(line)))
    held.write("go")
    expect(JSON.parse(await said).said.length).toBe(5_000_000)
    expect(held.isOpen).toBe(true)
    held.end()
    expect((await held.exited).code).toBe(0)
  })

  /** An agent that, told to go, writes what `script` makes (on its stdout), then "done". */
  async function writer(script: string) {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "writer.js")
    writeFileSync(agent, `process.stdin.once("data", () => { ${script}; process.stdout.write(JSON.stringify({ done: true }) + "\\n"); }); process.stdin.on("end", () => process.exit(0));`)
    const held = await hold([process.execPath, agent], { folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    return { held, folder }
  }

  /** All it said until "done", in order. */
  function untilDone(held: Awaited<ReturnType<typeof hold>>) {
    return new Promise<string[]>((resolve) => {
      const said: string[] = []
      held.onLine((line) => {
        said.push(line)
        if (line.includes('"done"')) resolve(said)
      })
    })
  }

  test("a line longer than the journal's half, and one after it: both reach the bridge, which stays attached", async () => {
    const { held } = await writer(`process.stdout.write(JSON.stringify({ said: "x".repeat(4_500_000) }) + "\\n"); process.stdout.write(JSON.stringify({ after: 1 }) + "\\n")`)
    const said = untilDone(held)
    held.write("go")
    const lines = await said
    expect(lines.map((line) => Object.keys(JSON.parse(line))[0])).toEqual(["said", "after", "done"])
    expect(JSON.parse(lines[0]!).said.length).toBe(4_500_000)
    expect(held.isOpen).toBe(true)
    held.end()
    expect((await held.exited).code).toBe(0)
  })

  test("a fast burst of 5 MB of lines: each reaches the bridge, in order, and it stays attached", async () => {
    const { held } = await writer(`for (let n = 0; n < 50_000; n++) process.stdout.write(JSON.stringify({ n, pad: "y".repeat(90) }) + "\\n")`)
    const said = untilDone(held)
    held.write("go")
    const lines = await said
    expect(lines.length).toBe(50_001)
    expect(lines.slice(0, -1).every((line, index) => JSON.parse(line).n === index)).toBe(true)
    expect(held.isOpen).toBe(true)
    held.end()
    expect((await held.exited).code).toBe(0)
  })

  test("a turn longer than 4 MB comes again whole to a bridge that attaches after (the journal is on disk)", async () => {
    const { held, folder } = await writer(`for (let n = 0; n < 100_000; n++) process.stdout.write(JSON.stringify({ n, pad: "z".repeat(90) }) + "\\n")`)
    const said = untilDone(held)
    held.write("go")
    await said
    held.detach()
    await held.closed
    const later = await attach(folder, heldIn(folder).find((each) => each.id === held.id)!, 0)
    expect(later.gap).toBe(false)
    const again = await untilDone(later)
    expect(again.length).toBe(100_001)
    expect(statSync(join(folder, `${held.id}.journal`)).mode & 0o777).toBe(0o600)
    later.end()
    await later.closed
    expect(existsSync(join(folder, `${held.id}.journal`))).toBe(false)
  })

  test("a managed Antigravity release stays installed through its last holder", async () => {
    const { dir, folder } = shortFolder()
    const old = join(dir, "antigravity", "1.2.0")
    const current = join(dir, "antigravity", ANTIGRAVITY_ACP)
    mkdirSync(old, { recursive: true })
    mkdirSync(current, { recursive: true })
    const agent = join(dir, "old-antigravity.js")
    writeFileSync(agent, 'process.stdin.on("end", () => process.exit(0)); process.stdin.resume();')
    const held = await hold([process.execPath, agent], {
      folder, kind: "raw", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH },
      meta: { kind: "antigravity", version: "1.2.0" },
    })
    const complete = join(current, ".complete")
    writeFileSync(complete, "")
    forgetOldAntigravity(dir, folder)
    expect(existsSync(old)).toBe(true)
    expect(existsSync(current)).toBe(true)
    rmSync(complete)
    held.end()
    await held.closed
    forgetOldAntigravity(dir, folder)
    expect(existsSync(old)).toBe(true)
    writeFileSync(join(current, ".complete"), "")
    forgetOldAntigravity(dir, folder)
    expect(existsSync(old)).toBe(false)
    expect(existsSync(current)).toBe(true)
  })

  test("a transfer pins replay until the successor releases it", async () => {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "pin.js")
    writeFileSync(agent, 'process.stdin.on("data", () => process.stdout.write(JSON.stringify({ value: "x".repeat(200) }) + "\\n")); process.stdin.on("end", () => process.exit(0));')
    const first = await hold([process.execPath, agent], { folder, kind: "raw", journalBytes: 64, env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    expect(await first.prepare(0)).toBe(true)
    const said = new Promise<string>(resolve => first.onLine(resolve))
    first.write("go")
    const original = await said
    const later = await attach(folder, recordOf(folder, first.id), 0, true)
    expect(await new Promise<string>(resolve => later.onLine(resolve))).toBe(original)
    later.unpin()
    later.end()
    expect((await later.exited).code).toBe(0)
  })

  test("a missing replay range is refused without fencing the controller", async () => {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "gap.js")
    writeFileSync(agent, 'process.stdin.on("data", () => process.stdout.write("x".repeat(200) + "\\n" + "y".repeat(200) + "\\n")); process.stdin.on("end", () => process.exit(0));')
    const held = await hold([process.execPath, agent], { folder, kind: "raw", journalBytes: 64, env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    const processed = new Promise<void>(resolve => held.onLine((_line, _again, seq) => { if (seq === 2) resolve() }))
    held.write("go")
    await processed
    expect(await held.prepare(0)).toBe(false)
    await expect(attach(folder, recordOf(folder, held.id), 0, true)).rejects.toThrow("no longer in the journal")
    expect(held.isOpen).toBe(true)
    held.end()
    expect((await held.exited).code).toBe(0)
  })

  test("stopping it stops what its agent started too", async () => {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "parent.js")
    const started = join(dir, "started")
    // (What it starts keeps its output open, so its end is said once that's gone too.)
    writeFileSync(agent, `const child = require("node:child_process").spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: ["ignore", "inherit", "inherit"] }); require("node:fs").writeFileSync(${JSON.stringify(started)}, String(child.pid)); process.stdout.write("{}\\n"); process.stdin.on("end", () => process.exit(0)); process.stdin.resume();`)
    const held = await hold([process.execPath, agent], { folder, kind: "raw", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH } })
    await new Promise<void>((resolve) => held.onLine(() => resolve()))
    const grandchild = Number(readFileSync(started, "utf8"))
    expect(alive(grandchild)).toBe(true)
    held.stop()
    await held.exited
    expect(alive(grandchild)).toBe(false)
  })

  test("a bridge that stopped reading, fenced off by a newer one, holds nothing back from it", async () => {
    // (More than a socket's buffers hold: the agent's output waits for the bridge that stopped.)
    const { held, folder } = await writer(`for (let n = 0; n < 200_000; n++) process.stdout.write(JSON.stringify({ n, pad: "y".repeat(90) }) + "\\n")`)
    held.detach()
    await held.closed
    const record = heldIn(folder).find((each) => each.id === held.id)!
    // The hung bridge, a process of its own: it attaches, asks for the burst, and is stopped (SIGSTOP).
    const hung = spawn(process.execPath, ["-e", `
      const socket = require("node:net").connect(${JSON.stringify(join(folder, `${held.id}.sock`))});
      socket.write(JSON.stringify({ hello: "hold/1", key: ${JSON.stringify(record.key)}, generation: ${record.generation + 1} }) + "\\n");
      socket.once("data", () => { socket.pause(); socket.write(JSON.stringify({ i: "go" }) + "\\n"); console.log("attached"); });
    `], { stdio: ["ignore", "pipe", "ignore"] })
    const gone = new Promise<void>((resolve) => hung.once("exit", () => resolve()))
    await new Promise<void>((resolve) => hung.stdout!.once("data", () => resolve()))
    process.kill(hung.pid!, "SIGSTOP")
    // A newer bridge takes over: all of it comes, without waiting the minute a hung bridge gets.
    const newer = await attach(folder, { ...record, generation: record.generation + 1 }, 0)
    const lines = await untilDone(newer)
    expect(lines.length).toBe(200_001)
    process.kill(hung.pid!, "SIGKILL")
    await gone
    newer.end()
    await newer.closed
  })

  test("two bridges taking up the same held agent at once: one has it, and the other leaves it be", async () => {
    const { dir, folder } = shortFolder()
    const agent = join(dir, "quiet.js")
    writeFileSync(agent, `process.stdin.on("data", () => process.stdout.write("{}\\n")); process.stdin.on("end", () => process.exit(0));`)
    const first = await hold([process.execPath, agent], { folder, kind: "jsonrpc", env: { BUN_BE_BUN: process.env.BUN_BE_BUN, PATH: process.env.PATH }, meta: { provider: "codex-cli", kind: "codex", turns: { t1: { session: "ses_x", from: 0 } } } })
    first.detach()
    await first.closed
    const [one, other] = await Promise.all([adoptHeld(folder), adoptHeld(folder)])
    const taken = [one, other].filter((found) => found.turns.size)
    expect(taken.length).toBe(1)
    const winner = taken[0]!.turns.get("ses_x")!.held
    // (Its agent still answers the one that has it.)
    const answered = new Promise<string>((resolve) => winner.onLine(() => resolve("answered")))
    winner.write("ping")
    expect(await Promise.race([answered, winner.exited.then(() => "stopped")])).toBe("answered")
    winner.end()
    await winner.closed
  })

  test("a record left by a holder that's gone, its number another process's now: removed, and that process left alone", async () => {
    const { folder } = shortFolder()
    // (A process of this test's own, standing for whatever took the number.)
    const other = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" })
    const gone = new Promise<void>((resolve) => other.once("exit", () => resolve()))
    const id = "0123456789ab"
    mkdirSync(folder, { recursive: true, mode: 0o700 })
    writeFileSync(join(folder, `${id}.json`), JSON.stringify({ proto: "hold/1", holder: HOLDER, id, kind: "jsonrpc", pid: other.pid, child: null, generation: 1, exited: null, meta: { provider: "codex-cli", kind: "codex", turns: { t1: { session: "ses_x", from: 0 } } }, key: "k".repeat(48) }))
    writeFileSync(join(folder, `${id}.sock`), "")
    expect(await adoptHeld(folder)).toEqual({ turns: new Map(), idle: [] })
    expect(existsSync(join(folder, `${id}.json`))).toBe(false)
    expect(existsSync(join(folder, `${id}.sock`))).toBe(false)
    expect(alive(other.pid!)).toBe(true)
    other.kill()
    await gone
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

test("a box's holders are in its run folder; one too deep for a socket's path holds none", () => {
  expect(holdFolder("/home/me/.kuvo-code")).toBe("/home/me/.kuvo-code/run/agents/hold")
  expect(holdFolder(`/Users/me/${"deep/".repeat(20)}.kuvo-code`)).toBeUndefined()
})

test("an agent stopped while its holder is starting, whose holder then can't start, isn't started as the bridge's own instead", async () => {
  // (A holder that can't start: its folder can't be made, under a file.)
  const blocked = join(root, "blocked")
  writeFileSync(blocked, "")
  const ran = join(root, "fell-back")
  const agent = heldProcess(hold([process.execPath, "-e", ""], { folder: join(blocked, "hold"), kind: "claude", env: {} }), () => spawn(process.execPath, ["-e", `require("node:fs").writeFileSync(${JSON.stringify(ran)}, "")`], { stdio: ["pipe", "pipe", "pipe"] }))
  const exited = new Promise<number | null>((resolve) => agent.once("exit", (code: number | null) => resolve(code)))
  agent.on("error", () => {})
  // Stopped (its session closed) before the holder has said anything.
  agent.kill()
  expect(await exited).toBe(1)
  expect(existsSync(ran)).toBe(false)
})
