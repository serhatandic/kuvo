// Each coding agent's program's update, as the box keeps it for every client (PROTOCOL.md,
// "Installing and updating an agent's program"): under way, updated to a version, already the
// newest, or why it couldn't be, by program (one program serves every account with it). Kept here,
// not by the client that asked, so a second one (the phone while the Mac updates) sees it under
// way, and so does the one that asked once it's opened again. Update All is the box's to pick too:
// each program a newer version is out for that its installer can update, once.

import { randomUUID } from "node:crypto"
import { readdirSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { HOLD_MOST } from "../shared/engine-swap"
import { writeDurablePrivate } from "../shared/files"
import type { Agent } from "./agent"
import { emit } from "./events"
import { compareVersions, finishProgramInstallation, isOutdated, programInstallerStarted, type Maintenance, type UpdateResult } from "./maintenance"

/** How a program's update stands, by when it last changed (`at`, its id for a client's "seen"). */
export type ProgramUpdate =
  | { state: "updating"; at: number }
  /** Updated just now (`updated`), or asked to and already on the newest (`current`): to this version. */
  | { state: "updated" | "current"; version: string; at: number }
  /** Why it couldn't be updated, in the installer's words, and the version it was left on. */
  | { state: "failed"; message: string; version?: string; at: number }

/** How long what an update brought is news. Why one failed stays until you've seen it (a client
 * says so: `forgetUpdate`), the next update, or the version moving past it. */
const NEWS_KEPT = 60 * 60_000

const kept = new Map<string, ProgramUpdate>()
const underWay = new Map<string, Promise<UpdateResult>>()

interface Drain {
  attempt: string
  provider: string
  deadline: number
}

interface ProgramUpdates {
  home?: string
  hold?: (program: string, deadline: number) => Promise<() => void>
}

const drainsIn = (home: string) => join(home, "program-draining")
const drainFile = (home: string, program: string) => join(drainsIn(home), program + ".json")

/** Whether its program is updating now (`agent.id`: one program serves every account with it). */
export function isUpdating(program: string): boolean {
  return underWay.has(program)
}

/** Whether any program is updating: a newer bridge waits for it (renewal.ts). */
export function updatesUnderWay(): boolean {
  return underWay.size > 0
}

/**
 * How its update stands, on `installed`, the version it has now: an outcome about an older version
 * (updated since in a terminal) is no news, and nor is one an hour old, unless it failed. (Only a
 * newer version: a read that began before the update ended still says the one before.)
 */
export function programUpdate(program: string, installed?: string, now = Date.now()): ProgramUpdate | undefined {
  const update = kept.get(program)
  if (!update || update.state === "updating") return update
  const movedOn = installed !== undefined && update.version !== undefined && compareVersions(installed, update.version) > 0
  const old = update.state !== "failed" && now - update.at > NEWS_KEPT
  if (!movedOn && !old) return update
  kept.delete(program)
  return undefined
}

/**
 * Its outcome seen (`at`: that one, not one after it), or no longer true (it was installed anew):
 * forgotten for every client. Not one under way. Whether there was one.
 */
export function forgetUpdate(program: string, at?: number): boolean {
  const update = kept.get(program)
  if (!update || update.state === "updating" || (at !== undefined && update.at !== at)) return false
  kept.delete(program)
  return true
}

/** Whether Update All updates it: a newer version is out, its installer can update it, and it isn't already. */
export function inUpdateAll(agent: Agent, maintained: Maintenance | undefined): boolean {
  return Boolean(agent.update && maintained?.command && isOutdated(maintained) && !isUpdating(agent.id))
}

/** Every account with `agent`'s program told it changed: each account's row says how it stands. */
function told(agent: Agent, accounts: Agent[]) {
  for (const each of accounts) if (each.id === agent.id) emit("integration.updated", { integrationID: each.providerID })
}

/** What an update's answer says of it. */
function outcome(result: UpdateResult): ProgramUpdate {
  const at = Date.now()
  if (result.message && !result.updated) return { state: "failed", message: result.message, version: result.version, at }
  if (result.version) return { state: result.updated ? "updated" : "current", version: result.version, at }
  return { state: "failed", message: "The update finished without saying which version it left.", at }
}

/** Keeps what an agent's program is now, as just read (its version, the newest): whether that's news. */
export type Noted = (agent: Agent, maintained: Maintenance) => boolean

/** Restore admission holds before waking inboxes or accepting requests. */
export function resumeProgramUpdates(accounts: () => Agent[], noted: Noted, control: ProgramUpdates) {
  const home = control.home
  if (!home) return
  let names: string[]
  try { names = readdirSync(drainsIn(home)) } catch { return }
  for (const name of names) {
    const program = /^([a-z][a-z0-9-]*)\.json$/.exec(name)?.[1]
    if (!program) continue
    const file = drainFile(home, program)
    try {
      const drain: Drain = JSON.parse(readFileSync(file, "utf8"))
      if (typeof drain.provider !== "string" || !Number.isFinite(drain.deadline) ||
          typeof drain.attempt !== "string" || !/^[a-f0-9-]{36}$/.test(drain.attempt)) continue
      // The installer already took over. Its process-group record holds turns until it exits.
      if (programInstallerStarted(home, program, drain.attempt)) {
        rmSync(file, { force: true })
        continue
      }
      const candidates = accounts().filter(agent => agent.id === program && agent.update)
      const agent = candidates.find(agent => agent.providerID === drain.provider) ?? candidates[0]
      if (agent) void updateProgram(agent, accounts, noted, control, drain)
      else {
        kept.set(program, outcome({ updated: false, message: "This program can no longer be updated here." }))
        rmSync(file, { force: true })
      }
    } catch (error) {
      console.error("Couldn't restore a program update:", error)
    }
  }
}

/**
 * Updates `agent`'s program through the installer that owns it, or the update of it under way:
 * every account with it says it's updating until it's done, then how it went. Done, what the
 * program is now (its version) is `noted` before anyone is told; each account with it runs the
 * new one (`programUpdated`). Never fails: what went wrong is its outcome.
 */
export function updateProgram(agent: Agent, accounts: () => Agent[], noted: Noted, control: ProgramUpdates = {}, saved?: Drain): Promise<UpdateResult> {
  const program = agent.id
  const running = underWay.get(program)
  if (running) return running
  const drain = saved ?? { attempt: randomUUID(), provider: agent.providerID, deadline: Date.now() + HOLD_MOST }
  const home = control.home
  const file = home && drainFile(home, program)
  if (file && !saved) writeDurablePrivate(file, JSON.stringify(drain), home)
  kept.set(program, { state: "updating", at: drain.deadline - HOLD_MOST })
  const done = (async (): Promise<UpdateResult> => {
    let result: UpdateResult = { updated: false, message: "The update stopped without saying how it went." }
    let release: (() => void) | undefined
    try {
      // Install the fence synchronously. Even if the callback throws, settle the attempt
      // after it has been registered in underWay.
      release = await (async () => control.hold?.(program, drain.deadline))()
      result = agent.update ? await agent.update(home ? drain.attempt : undefined) : { updated: false, message: `Update ${agent.name} the way you installed it.` }
      // Its version as it is now (the update read it afresh): what its accounts list from here.
      const maintained = await agent.maintenance?.().catch(() => undefined)
      if (maintained) noted(agent, maintained)
      // Every account with it runs that program.
      if (result.updated) for (const each of accounts()) if (each.id === program) each.programUpdated?.()
    } catch (error: any) {
      if (!result.updated) result = { updated: false, message: String(error?.message ?? error) }
    } finally {
      kept.set(program, outcome(result))
      underWay.delete(program)
      told(agent, accounts())
      if (result.updated) emit("model.updated", { program })
      try {
        if (home && file) {
          rmSync(file, { force: true })
          finishProgramInstallation(home, program, drain.attempt)
        }
      } catch (error) {
        console.error("Couldn't retire a program update:", error)
      } finally {
        release?.()
      }
    }
    return result
  })()
  underWay.set(program, done)
  told(agent, accounts())
  return done
}

/**
 * Update All: each program a newer version is out for that its installer can update, once (an
 * account you added runs its agent's program), not one updating already; asked of its own
 * sign-in first, then of the accounts added to it, by what's `known` of it (what Accounts showed),
 * read only where nothing is. Started, not waited for: the programs it started, by `agent.id`.
 */
export async function updateAll(agents: Agent[], accounts: () => Agent[], noted: Noted, known: (agent: Agent) => Maintenance | undefined, control?: ProgramUpdates): Promise<string[]> {
  const byProgram = new Map<string, Agent[]>()
  // An agent's own sign-in ahead of the accounts added to it.
  const ordered = [...agents.filter((agent) => agent.instance.id === "default"), ...agents.filter((agent) => agent.instance.id !== "default")]
  for (const agent of ordered) {
    if (!agent.update || !agent.maintenance) continue
    byProgram.set(agent.id, [...(byProgram.get(agent.id) ?? []), agent])
  }
  const picked = await Promise.all(
    [...byProgram.values()].map(async (candidates) => {
      if (isUpdating(candidates[0].id)) return undefined
      for (const agent of candidates) {
        let maintained = known(agent)
        if (!maintained) maintained = await agent.maintenance!().catch(() => undefined)
        // Read anew (ten minutes at most, as Accounts reads it): its accounts say so.
        if (maintained && noted(agent, maintained)) emit("integration.updated", { integrationID: agent.providerID })
        if (inUpdateAll(agent, maintained)) return agent
      }
      return undefined
    }),
  )
  const started: string[] = []
  for (const agent of picked) {
    if (!agent || isUpdating(agent.id)) continue
    void updateProgram(agent, accounts, noted, control)
    started.push(agent.id)
  }
  return started
}
