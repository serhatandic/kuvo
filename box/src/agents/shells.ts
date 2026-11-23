// Commands an agent runs, listed as the engine lists its own (a conversation's Processes show
// them): those it left running in the background, and those its turn waits on in the foreground
// (`foreground`, which end with their tool call). Each with its session, what it runs, what it has
// printed so far, and a way to stop it. The agent that started one says when it ends.

import { emit } from "./events"
import { newID } from "./ids"
import { workOrder, type WorkOrder } from "../shared/work-order"

export interface ShellJob {
  id: string
  status: "running" | "exited"
  command: string
  cwd: string
  exit?: number
  metadata: { sessionID: string }
  time: { started: number; order: WorkOrder }
  /** Its turn waits on it: it ends with its tool call, never after the turn. */
  foreground?: true
}

interface Kept {
  job: ShellJob
  /** The end of what it printed, and how much it printed in all (bytes). */
  output: () => Promise<{ output: string; total: number }>
  stop: () => Promise<void>
}

const kept = new Map<string, Kept>()

/** How long an ended one's output can still be read. */
const keptAfter = 10 * 60_000

/** A command left running: its id here. */
export function addShell(sessionID: string, directory: string, command: string, output: Kept["output"], stop: Kept["stop"], started = Date.now(), order = workOrder(), foreground = false): string {
  const id = newID("shl")
  const job: ShellJob = { id, status: "running", command, cwd: directory, metadata: { sessionID }, time: { started, order }, ...(foreground ? { foreground: true as const } : {}) }
  kept.set(id, { job, output, stop })
  emit("shell.created", { info: job }, directory)
  return id
}

/** It ended (or its agent did): the bar lets it go. */
export function endShell(id: string, exit?: number) {
  const one = kept.get(id)
  if (!one || one.job.status === "exited") return
  one.job.status = "exited"
  if (exit !== undefined) one.job.exit = exit
  emit("shell.exited", { id }, one.job.cwd)
  const timer = setTimeout(() => kept.delete(id), keptAfter)
  ;(timer as any).unref?.()
}

export function shells(directory?: string): ShellJob[] {
  return [...kept.values()].map((one) => one.job).filter((job) => !directory || job.cwd === directory)
}

/** A session's commands running now that it left running (with `foreground`, its turn's too). */
export function runningShells(sessionID: string, foreground = false): ShellJob[] {
  return [...kept.values()].map((one) => one.job).filter((job) => job.status === "running" && job.metadata.sessionID === sessionID && (foreground || !job.foreground))
}

export function shell(id: string): ShellJob | undefined {
  return kept.get(id)?.job
}

/** What it printed from byte `cursor` on, `limit` bytes at most, as the engine reads a command's output. */
export async function shellOutput(id: string, cursor = 0, limit?: number): Promise<{ output: string; cursor: number; size: number } | undefined> {
  const one = kept.get(id)
  if (!one) return undefined
  const read = await one.output().catch(() => ({ output: "", total: 0 }))
  const bytes = Buffer.from(read.output)
  const total = Math.max(read.total, bytes.length)
  // Only its end is known: what's asked from before that comes from where it begins.
  const known = total - bytes.length
  const start = Math.max(cursor, known)
  const end = limit === undefined ? total : Math.min(total, cursor + limit)
  const output = start < end ? bytes.subarray(start - known, end - known).toString("utf8") : ""
  return { output, cursor: Math.max(start, end), size: total }
}

export async function stopShell(id: string): Promise<boolean> {
  const one = kept.get(id)
  if (!one) return false
  if (one.job.status === "running") await one.stop().catch(() => {})
  return true
}
