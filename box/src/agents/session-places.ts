import type { Requester } from "./groups/api"

/** Where a session runs: the engine's API, or this bridge's own, and its events. */
export interface SessionPlace {
  api: Requester
  /** Tells `told` each event of `session`'s; returns a stop. `reconnected` when the feed came back (events may have been missed). */
  listen(session: string, told: { event(type: string, data: any): void; reconnected(): void }): () => void
}

export interface SessionPlaces {
  engine: SessionPlace
  bridge: SessionPlace
  /** Whether a session is one of this bridge's (a coding agent's), not the engine's. */
  isBridge(session: string): boolean
}
