// How fast each window of your plans is going (PROTOCOL.md, "Plan limits", `card`): the box keeps
// the recent readings of each account's windows, from whichever computer read them, and works out
// from their recent rate whether a window runs out before it starts over, and about when. Simple
// on purpose: the rate is the change between the first and last readings of a recent stretch, and
// where there aren't enough readings over long enough, nothing is said rather than a guess.

const MINUTE = 60_000
const HOUR = 60 * MINUTE

/** One reading of a window: when, and how much of it was used then (0 to 1). */
export interface PaceSample {
  at: number
  used: number
}

/** What a window's readings say of it now. */
export type Prediction =
  /** Used up: back when it starts over. */
  | { state: "used-up" }
  /** At the recent rate, used up about `at`, before it starts over. */
  | { state: "runs-out"; at: number }
  /** At the recent rate, some of it is left when it starts over. */
  | { state: "lasts" }

interface Trail {
  resetsAt?: number
  samples: PaceSample[]
}

/** What a window is, to its trail: when it was read, how much was used, when it starts over, and how long it is. */
export interface PacedWindow {
  read: number
  used: number
  resetsAt?: number
  minutes?: number
}

/** How far back the recent rate looks: a fifth of the window, from half an hour to a day. */
export function lookback(minutes: number | undefined): number {
  if (!minutes) return HOUR
  return Math.min(24 * HOUR, Math.max(30 * MINUTE, (minutes * MINUTE) / 5))
}

/** The fewest readings, and the shortest stretch they cover, before a rate is said. */
export const FEWEST_READINGS = 3
const shortestStretch = (minutes: number | undefined) => Math.max(15 * MINUTE, lookback(minutes) / 4)

/** Used up from here: a reading this close to the whole of it. */
const USED_UP = 0.995
/** Reset times can move slightly between readings, especially between computers. */
const RESET_JITTER = 5 * MINUTE
/** Keep the first and last readings in each time bucket, at most 98 over two lookbacks. */
const BUCKETS_PER_LOOKBACK = 24

function startedOver(previousReset: number | undefined, last: PaceSample, window: PacedWindow): boolean {
  const { resetsAt, read, minutes, used } = window
  if (resetsAt === undefined || previousReset === undefined || resetsAt <= previousReset + RESET_JITTER) return false
  if (read >= previousReset) return true
  // An early refill can also start a period: its new start and drop in usage must agree.
  return minutes !== undefined && resetsAt - minutes * MINUTE >= last.at - RESET_JITTER && used < last.used - 0.02
}

export class Pace {
  private trails = new Map<string, Trail>()

  /** Keeps a window's reading (once for each time it was read), starting its trail over when its period did. */
  note(key: string, window: PacedWindow, now: number) {
    if (!Number.isFinite(window.read) || !Number.isFinite(window.used)) return
    // (One whose reset has passed started over: not read since, nothing to add.)
    if (window.resetsAt !== undefined && window.resetsAt <= now) return
    let trail = this.trails.get(key)
    const last = trail?.samples.at(-1)
    // A peer can disappear and expose an older reading of the same window.
    if (last && window.read <= last.at) return
    if (!trail || (last && startedOver(trail.resetsAt, last, window))) {
      trail = { samples: [] }
      this.trails.set(key, trail)
    }
    if (window.resetsAt !== undefined) trail.resetsAt = window.resetsAt
    const samples = trail.samples
    const stretch = lookback(window.minutes)
    const bucket = (at: number) => Math.floor(at / (stretch / BUCKETS_PER_LOOKBACK))
    const beforePrevious = samples.at(-2)
    if (beforePrevious && bucket(beforePrevious.at) === bucket(window.read)) samples.pop()
    samples.push({ at: window.read, used: window.used })
    const since = window.read - 2 * stretch
    while (samples[0]!.at < since) samples.shift()
  }

  /** What a window's readings say of it now; nothing where they're too few or too close together. */
  predict(key: string, window: PacedWindow, now: number): Prediction | undefined {
    if (window.resetsAt !== undefined && window.resetsAt <= now) return undefined
    if (window.used >= USED_UP) return { state: "used-up" }
    if (window.resetsAt === undefined) return undefined
    const trail = this.trails.get(key)
    if (!trail) return undefined
    const latest = trail.samples.at(-1)
    if (!latest || latest.at !== window.read) return undefined
    const recent = trail.samples.filter((sample) => sample.at >= latest.at - lookback(window.minutes))
    const first = recent[0]!
    if (recent.length < FEWEST_READINGS || latest.at - first.at < shortestStretch(window.minutes)) return undefined
    const rate = (latest.used - first.used) / (latest.at - first.at)
    if (!(rate > 0)) return { state: "lasts" }
    const out = latest.at + (1 - latest.used) / rate
    if (out >= window.resetsAt - MINUTE) return { state: "lasts" }
    // (To the nearest five minutes: it's an estimate, and says so.)
    return { state: "runs-out", at: Math.max(now, Math.round(out / (5 * MINUTE)) * 5 * MINUTE) }
  }

  /** What's kept between runs: trails read within the last eight days (one longer gone is an account or limit that went). */
  save(now: number): Record<string, Trail> {
    return Object.fromEntries([...this.trails].filter(([, trail]) => (trail.samples.at(-1)?.at ?? 0) > now - 8 * 24 * HOUR))
  }

  load(saved: unknown) {
    if (!saved || typeof saved !== "object") return
    for (const [key, trail] of Object.entries(saved as Record<string, any>)) {
      if (!trail || !Array.isArray(trail.samples)) continue
      // Old saves capped by count at 120. Time buckets keep new trails below that bound.
      const samples = trail.samples.filter((sample: any) => Number.isFinite(sample?.at) && Number.isFinite(sample?.used)).slice(-120)
      this.trails.set(key, { ...(typeof trail.resetsAt === "number" ? { resetsAt: trail.resetsAt } : {}), samples })
    }
  }
}
