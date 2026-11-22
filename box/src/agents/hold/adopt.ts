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
import { attach, forgetOldPrograms, heldIn, holdFolder, isHolder, type Held, type HoldRecord, HOLD_PROTO } from "./client"

/** Agents whose live protocol still has a client when the bridge disconnects. */
export const HELD_KINDS: ReadonlySet<AgentKind> = new Set<AgentKind>(["codex", "claude", "grok", "cursor", "antigravity"])

/** The first Claude Code that sends a client joining it the asks it has open (`pending_permission_requests`, spike S1). */
export const ANTIGRAVITY_ACP = "1.3.0"
export const CURSOR_SDK = "1.0.35"
export const CLAUDE_SDK = "0.3.303"
export const CLAUDE_REJOINS = "2.1.268"

/** Whether a held agent can be taken up: Claude Code only from the version that sends its open asks again (an older one's ask would wait forever). */
export function adoptable(meta: Partial<HeldMeta>): boolean {
  if (!meta.kind || !HELD_KINDS.has(meta.kind)) return false
  // (Google's earlier agent pinned to its version, or its `agy` CLI, `version` "agy".)
  if (meta.kind === "antigravity") return meta.version === ANTIGRAVITY_ACP || meta.version === "agy"
  if (meta.kind === "cursor") return meta.sdk === CURSOR_SDK
  if (meta.kind !== "claude") return true
  return meta.sdk === CLAUDE_SDK && typeof meta.version === "string" && atLeast(meta.version, CLAUDE_REJOINS)
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
  /** When this ACP prompt was sent, to distinguish its transcript from earlier prompts. */
  sentAt?: number
  /** What a bridge taking the turn up needs that only the one before knew (Antigravity's place in its
   * transcript), written as that one made way (`Thread.handOver`). */
  state?: unknown
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
  failed = false
  private settle!: (ready: boolean) => void
  private activate!: () => void
  readonly readyToOwn = new Promise<boolean>(resolve => { this.settle = resolve })
  readonly activated = new Promise<void>(resolve => { this.activate = resolve })

  constructor(
    readonly held: Held,
    threads: string[],
    private provisional = false,
  ) {
    this.waiting = new Set(threads)
  }

  get preparing() { return this.provisional }

  /** Start replay only after every holder has an adapter ready to consume it. */
  commit() {
    this.provisional = false
    this.held.own()
    this.check()
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
    if (this.provisional) this.fail()
    if (this.waiting.delete(thread)) this.dropped.push(thread)
    this.check()
  }

  /**
   * A turn that couldn't be taken up after all: let go of (as `drop`), or, once it was ready, no
   * longer counted; the agent stops once no turn taken up in it is left (another session's turn on
   * the same Codex goes on).
   */
  release(thread: string) {
    if (this.provisional) return this.fail()
    if (this.waiting.has(thread)) return this.drop(thread)
    if (this.released.has(thread)) return
    this.released.add(thread)
    if (this.opened && this.released.size >= this.taken) this.held.stop()
  }
  private released = new Set<string>()

  private fail() {
    this.failed = true
    this.settle(false)
  }

  private check() {
    if (this.opened || this.waiting.size) return
    if (!this.taken) {
      this.opened = true
      return this.held.stop()
    }
    if (!this.opener) return
    this.settle(true)
    if (this.provisional) return
    this.opened = true
    this.activate()
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
  /** Extra provider state understood by a compatible live-transfer adopter. */
  state?: unknown
}

/** A holder's process stopped (its agent first, as a bridge stops one), by its own pid, once it's
 * that very holder (never a process that took its number since). */
function stopHolder(record: HoldRecord) {
  if (!isHolder(record.pid, record.id)) return
  try {
    process.kill(record.pid, "SIGTERM")
  } catch {}
}

/** What a bridge starting takes up of the agents held here: turns under way, by session, and agents between turns. */
export interface Adopted {
  turns: Map<string, HeldTurn>
  /** Agents with no turn under way, attached as they are (nothing said again), for their agent to take up (`Agent.adoptIdle`). */
  idle: { held: Held; meta: HeldMeta }[]
}

/**
 * This box's held agents, as a bridge starts (the one before made way, or crashed): each attached,
 * the next generation. One with turns under way from where the oldest of them began, each taken up
 * again in its session; one between turns as it is, for its next turn. Only one this bridge can't
 * take up is stopped (its kind or version isn't one it speaks to, or what it said
 * since its turn began is no longer all there). `skip`: those a bridge handover attached already.
 */
export async function adoptHeld(folder = machineHome ? holdFolder(machineHome) : undefined, skip: ReadonlySet<string> = new Set()): Promise<Adopted> {
  const found: Adopted = { turns: new Map(), idle: [] }
  if (!folder) return found
  forgetOldPrograms(folder)
  for (const record of heldIn(folder)) {
    if (skip.has(record.id)) continue
    const meta = record.meta as Partial<HeldMeta>
    const turns = Object.entries(meta.turns ?? {})
    // Antigravity's turn goes on only from its place in its transcript, which the bridge before
    // wrote as it made way: after a crash (none written), session repair ends that turn instead of
    // guessing what was shown.
    if (!meta.provider || !adoptable(meta) || (meta.kind === "antigravity" && turns.some(([, note]) => note.state === undefined)) || (record.exited && (!turns.length || meta.kind === "claude"))) {
      stopHolder(record)
      continue
    }
    let held: Held
    try {
      held = await attach(folder, record, turns.length ? Math.max(0, Math.min(...turns.map(([, note]) => note.from))) : Number.MAX_SAFE_INTEGER)
    } catch (error) {
      // (Another bridge starting at the same moment took it, a newer generation: it's that one's.)
      if (!/generation/.test(String((error as Error)?.message))) stopHolder(record)
      continue
    }
    if (!turns.length) {
      found.idle.push({ held, meta: meta as HeldMeta })
      continue
    }
    if (held.gap) {
      held.stop()
      continue
    }
    const gate = new HeldGate(held, turns.map(([thread]) => thread))
    for (const [thread, note] of turns) found.turns.set(note.session, { held, meta: meta as HeldMeta, thread, note, gate, state: note.state })
  }
  return found
}

/** The portable part of the current controller's plan. No credentials travel in it. */
export interface TransferHolder {
  id: string
  generation: number
  proto: string
  kind: AgentKind
  version?: string
  sdk?: string
  provider: string
  turns: Record<string, HeldTurnNote>
  acked: number
}

export function canTransfer(holders: TransferHolder[]): boolean {
  return holders.every(({ proto, kind, version, sdk }) => proto === HOLD_PROTO && adoptable({ kind, version, sdk }))
}

/** Validate the entire set before attaching. On failure leave the holders for the recovery bridge. */
export async function adoptTransfer(home: string, plan: TransferHolder[], recovering = false): Promise<Map<string, HeldTurn>> {
  const found = new Map<string, HeldTurn>()
  if (!plan.length) return found
  const folder = holdFolder(home)
  if (!folder || !canTransfer(plan)) throw new Error("The bridge cannot adopt these agents.")
  const records = new Map(heldIn(folder).map(record => [record.id, record]))
  const attached: Held[] = []
  try {
    const attempts = await Promise.allSettled(plan.map(async entry => {
      const record = records.get(entry.id)
      if (!record && recovering) return // A reboot or an agent exit can leave a checkpoint without its holder.
      if (!record || record.generation < entry.generation || record.meta.provider !== entry.provider) {
        throw new Error("A holder in the handover is unavailable.")
      }
      const turns = Object.entries(entry.turns)
      const held = await attach(folder, record, Math.min(...turns.map(([, note]) => note.from)), "provisional")
      attached.push(held)
      const gate = new HeldGate(held, turns.map(([thread]) => thread), true)
      for (const [thread, note] of turns) found.set(note.session, { held, gate, thread, note, meta: record.meta as HeldMeta })
    }))
    for (const attempt of attempts) if (attempt.status === "rejected") throw attempt.reason
    return found
  } catch (error) {
    for (const held of attached) held.detach()
    throw error
  }
}

/** The turns noted in a held agent's record, with `thread`'s set (or taken out, with none). */
export function noteTurn(held: Held, thread: string, note: HeldTurnNote | undefined) {
  const turns = { ...((held.meta.turns as Record<string, HeldTurnNote> | undefined) ?? {}) }
  if (note) turns[thread] = note
  else delete turns[thread]
  held.note({ turns })
}
