// Only the active bridge writes the store. A standby reserves its listener and checks the entire
// transfer before the active bridge checkpoints its state and exits.
import { spawn, type ChildProcess } from "node:child_process"
import { existsSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { writeDurablePrivate, writeSecret } from "../shared/files"
import { objectFormat } from "../shared/format-version"
import { runsAs } from "../shared/platform"
import { canTransfer, type TransferHolder } from "./hold/adopt"
import type { TransferSession } from "./sessions"

export const TRANSFER_PROTOCOL = "kuvo-live-handover/1"
const format = objectFormat("Bridge handover", [fields => fields])

export interface Transfer {
  formatVersion: 1
  from: string
  to: string
  old: number
  next: number
  holders: TransferHolder[]
  sessions: Record<string, TransferSession>
}

export const transferFile = (run: string) => join(run, "agents", "handover.json")
export const previousBridge = (run: string) => join(run, "agents", "previous", "kuvo-agents.js")

export function readTransfer(run: string): Transfer {
  const plan = JSON.parse(readFileSync(transferFile(run), "utf8"))
  if (plan.formatVersion !== format.version) throw new Error("Unsupported bridge handover format.")
  format.read(plan)
  return plan
}

export function finishTransfer(run: string) {
  rmSync(transferFile(run), { force: true })
  rmSync(previousBridge(run), { force: true })
}

/** Read compatibility and the socket before loading the store or attaching to an agent. */
export async function standBy(run: string, build: string | null) {
  if (!process.send) throw new Error("The standby bridge needs its controller.")
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, idleTimeout: 0, fetch: () => new Response(null, { status: 503 }) })
  const transfer = await new Promise<Transfer>((resolve, reject) => {
    let approved = false
    const timer = setTimeout(() => reject(new Error("The active bridge didn't complete the handover.")), 60_000)
    process.on("message", (plan: Transfer) => {
      approved = plan.formatVersion === 1 && plan.to === build && plan.next === process.pid && canTransfer(plan.holders)
      process.send?.(approved ? "compatible" : "incompatible", () => {})
    })
    // The sender exits without disconnecting first. EOF is the fence against two store writers.
    process.once("disconnect", () => {
      clearTimeout(timer)
      try {
        const plan = readTransfer(run)
        if (!approved || plan.to !== build || plan.next !== process.pid ||
            Number(readFileSync(join(run, "agents.pid"), "utf8")) !== process.pid ||
            ["stopped", "off", "paused"].some(mark => existsSync(join(run, mark)))) throw new Error("The bridge transfer was cancelled.")
        resolve(plan)
      } catch (error) {
        reject(error)
      }
    })
    process.send?.("standby", () => {})
  })
  return { server, transfer }
}

function expect(child: ChildProcess, expected: string): Promise<boolean> {
  return new Promise(resolve => {
    const settled = (message?: unknown) => {
      clearTimeout(timer)
      child.off("message", settled)
      child.off("exit", settled)
      child.off("error", settled)
      resolve(message === expected)
    }
    const timer = setTimeout(settled, 10_000)
    child.once("message", settled)
    child.once("exit", settled)
    child.once("error", settled)
  })
}

export async function prepareStandby(runtime: string, script: string, env: NodeJS.ProcessEnv, plan: Transfer): Promise<ChildProcess | undefined> {
  const child = spawn(runtime, ["--config=/dev/null", "--no-env-file", script, "handover-standby"], {
    env: { ...env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
    stdio: ["ignore", "inherit", "inherit", "ipc"],
    detached: true,
  })
  try {
    if (await expect(child, "standby")) {
      plan.next = child.pid!
      const check = expect(child, "compatible")
      child.send(plan, () => {})
      if (await check) return child
    }
  } catch {}
  child.kill()
}

/** Persist recovery before moving the keeper's pointer. No async work follows a successful write. */
export function commitTransfer(run: string, plan: Transfer, source: Uint8Array) {
  const pid = join(run, "agents.pid")
  if (Number(readFileSync(pid, "utf8")) !== process.pid ||
      ["stopped", "off", "paused"].some(mark => existsSync(join(run, mark)))) throw new Error("The keeper stopped this bridge.")
  if (existsSync(transferFile(run))) throw new Error("Another bridge handover is pending.")
  writeDurablePrivate(previousBridge(run), source, run)
  writeDurablePrivate(transferFile(run), JSON.stringify(format.write(plan)), run)
  try {
    writeSecret(join(run, "agents-renewing"), JSON.stringify({ at: Date.now(), pid: process.pid }))
    writeSecret(pid, String(plan.next))
  } catch (error) {
    finishTransfer(run)
    rmSync(join(run, "agents-renewing"), { force: true })
    throw error
  }
}

export function standbyFailed(plan: Transfer): boolean {
  // A recovery bridge runs only after the candidate is gone. The keeper serializes its launches.
  return !runsAs(plan.next, /kuvo-agents/)
}
