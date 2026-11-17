// Which coding agents' processes run under holders (client.ts), and what a bridge starting finds of
// them: the turns an earlier bridge had under way there when it ended without stopping them (a
// crash, a SIGKILL), each taken up again in its session (sessions.ts `resumeHeld`), or the agent
// stopped as that bridge would have stopped it.
//
// A holder's record says what a bridge adopting it needs (`HeldMeta`, written by the bridge that
// started it as its turns start and end): the agent, and each turn under way in it with where that
// turn's lines begin in the holder's journal. The new bridge attaches from the oldest of them, so
// each turn's message is made again whole from what the agent said, and an ask still open is asked
// once (the holder never replays one that was answered, nor Claude Code's own control requests).

import type { AgentKind } from "../accounts"
import { machineHome } from "../machine"
import { attach, forgetOldPrograms, heldIn, holdFolder, isHolder, type Held, type HoldRecord } from "./client"

/**
 * The agents whose processes start under a holder, and whose turns a bridge started after one that
 * crashed takes up again (spikes S1 and S2, .claude/notes/update-handover.md). Antigravity's and
 * Cursor's run as they always have, unheld: no spike has shown that a new bridge can take theirs
 * over, so a bridge that crashes ends their turns (as before), and one that makes way for a newer
 * build waits for them to end (renewal.ts).
 */
export const HELD_KINDS: ReadonlySet<AgentKind> = new Set<AgentKind>(["codex", "claude", "grok"])

/** The first Claude Code that sends a client joining it the asks it has open (`pending_permission_requests`, spike S1). */
export const CLAUDE_REJOINS = "2.1.268"

/** Whether a held agent can be taken up: Claude Code only from the version that sends its open asks again (an older one's ask would wait forever). */
export function adoptable(meta: Partial<HeldMeta>): boolean {
  if (!meta.kind || !HELD_KINDS.has(meta.kind)) return false
  if (meta.kind !== "claude") return true
  return typeof meta.version === "string" && atLeast(meta.version, CLAUDE_REJOINS)
}

function atLeast(version: string, least: string): boolean {
  const parts = (text: string) => (text.match(/\d+(\.\d+)*/)?.[0] ?? "0").split(".").map(Number)
  const [have, want] = [parts(version), parts(least)]
  for (let at = 0; at < Math.max(have.length, want.length); at++) {
    if ((have[at] ?? 0) !== (want[at] ?? 0)) return (have[at] ?? 0) > (want[at] ?? 0)
  }
  return true
}

/** Whether agents start under holders at all: a test of how an agent's own process is started turns it off. */
export const holding = { on: true }

/** Where this box's agents of `kind` are held: none for an agent that isn't, or off a box. */
export function agentHoldFolder(kind: AgentKind): string | undefined {
  if (!holding.on || !HELD_KINDS.has(kind) || !machineHome) return undefined
  return holdFolder(machineHome)
}

/** A turn under way in a held agent, as its bridge noted it. */
export interface HeldTurnNote {
  session: string
  /** The agent's own name for the turn, once it's known (Codex's). */
  turn?: string
  /** The last line of the agent's its bridge had taken in as it sent the turn's message. */
  from: number
  /** The request that the turn's end answers (an ACP agent's `session/prompt`). */
  request?: number | string
}

/** What a held agent's record says of it, for a bridge adopting it. */
export interface HeldMeta {
  /** Its provider (an account's, `codex-cli@…`). */
  provider: string
  kind: AgentKind
  /** Its turns under way, by the agent's conversation (Codex's thread, Claude Code's session, an ACP session). */
  turns: Record<string, HeldTurnNote>
  [more: string]: unknown
}

/**
 * Lets a held agent's lines come once every turn taken up again in it is ready for them (Codex's
 * app-server runs several sessions' threads): `open` is given those that were let go instead,
 * whose turns the agent is to stop. With none taken up, the agent is stopped.
 */
export class HeldGate {
  private waiting: Set<string>
  private dropped: string[] = []
  private opened = false
  private opener?: (dropped: string[]) => void
  private taken = 0

  constructor(
    readonly held: Held,
    threads: string[],
  ) {
    this.waiting = new Set(threads)
  }

  /** What opens it (the agent's RPC starting): set once, by the first turn taken up. */
  onOpen(open: (dropped: string[]) => void) {
    this.opener ??= open
    this.check()
  }

  ready(thread: string) {
    if (this.waiting.delete(thread)) this.taken++
    this.check()
  }

  drop(thread: string) {
    if (this.waiting.delete(thread)) this.dropped.push(thread)
    this.check()
  }

  /**
   * A turn that couldn't be taken up after all: let go of (as `drop`), or, once it was ready, no
   * longer counted; the agent stops once no turn taken up in it is left (another session's turn on
   * the same Codex goes on).
   */
  release(thread: string) {
    if (this.waiting.has(thread)) return this.drop(thread)
    if (this.released.has(thread)) return
    this.released.add(thread)
    if (this.opened && this.released.size >= this.taken) this.held.stop()
  }
  private released = new Set<string>()

  private check() {
    if (this.opened || this.waiting.size) return
    if (!this.taken) {
      this.opened = true
      return this.held.stop()
    }
    if (!this.opener) return
    this.opened = true
    this.opener(this.dropped)
  }
}

/** A turn a bridge takes up again, in its session: the agent's process, attached. */
export interface HeldTurn {
  held: Held
  meta: HeldMeta
  /** The agent's own conversation the turn is in. */
  thread: string
  note: HeldTurnNote
  gate: HeldGate
  /** Set by an agent whose process runs several conversations (Codex's app-server): once this turn's own first line comes again. Otherwise the agent's first line is. */
  began?: Promise<void>
}

/** A holder's process stopped (its agent first, as a bridge stops one), by its own pid, once it's
 * that very holder (never a process that took its number since). */
function stopHolder(record: HoldRecord) {
  if (!isHolder(record.pid, record.id)) return
  try {
    process.kill(record.pid, "SIGTERM")
  } catch {}
}

/**
 * This box's held agents, as a bridge starts: those with turns under way attached (the next
 * generation, from where the oldest turn began), by session; every other one stopped, as the
 * bridge before would have stopped it (nothing of it is under way; its kind isn't one this bridge
 * takes up; or what it said since its turn began is no longer all there).
 */
export async function adoptHeld(folder = machineHome ? holdFolder(machineHome) : undefined): Promise<Map<string, HeldTurn>> {
  const found = new Map<string, HeldTurn>()
  if (!folder) return found
  forgetOldPrograms(folder)
  for (const record of heldIn(folder)) {
    const meta = record.meta as Partial<HeldMeta>
    const turns = Object.entries(meta.turns ?? {})
    if (!meta.provider || !adoptable(meta) || !turns.length || record.exited) {
      stopHolder(record)
      continue
    }
    let held: Held
    try {
      held = await attach(folder, record, Math.max(0, Math.min(...turns.map(([, note]) => note.from))))
    } catch (error) {
      // (Another bridge starting at the same moment took it, a newer generation: it's that one's.)
      if (!/generation/.test(String((error as Error)?.message))) stopHolder(record)
      continue
    }
    if (held.gap) {
      held.stop()
      continue
    }
    const gate = new HeldGate(held, turns.map(([thread]) => thread))
    for (const [thread, note] of turns) found.set(note.session, { held, meta: meta as HeldMeta, thread, note, gate })
  }
  return found
}

/** The turns noted in a held agent's record, with `thread`'s set (or taken out, with none). */
export function noteTurn(held: Held, thread: string, note: HeldTurnNote | undefined) {
  const turns = { ...((held.meta.turns as Record<string, HeldTurnNote> | undefined) ?? {}) }
  if (note) turns[thread] = note
  else delete turns[thread]
  held.note({ turns })
}
