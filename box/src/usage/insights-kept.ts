// The last Insights summary served for each period and narrowing (PROTOCOL.md, "Insights"): served
// at once when it's asked again, marked with when it was read (`asOf`), while the box reads it
// afresh, so Insights opens on what the box already has and never on a blank page. Kept on disk
// (`insights-kept.json` in the usage folder), so a box that just started has it too.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import { dirname } from "node:path"

/** Summaries kept: three periods, and what was narrowed to lately. */
const MOST = 24
/** One read this lately is the summary as it is: served as it is, not read again for it. */
export const FRESH = 30_000

type Summary = { state?: unknown } & Record<string, unknown>
type Kept = { at: number; data: Summary }

/** One summary's key: its query, in a fixed order. */
export const keptKey = (params: URLSearchParams) => [...params.entries()].sort(([a, x], [b, y]) => a < b ? -1 : a > b ? 1 : x < y ? -1 : x > y ? 1 : 0)
  .map(([name, value]) => `${encodeURIComponent(name)}=${encodeURIComponent(value)}`).join("&")

export class KeptSummaries {
  private kept = new Map<string, Kept>()
  private reading = new Map<string, Promise<Summary | undefined>>()

  constructor(private readonly file?: string, private readonly now: () => number = Date.now) {
    if (!file) return
    try {
      const saved = JSON.parse(readFileSync(file, "utf8")) as Record<string, Kept>
      for (const [key, each] of Object.entries(saved ?? {})) {
        if (typeof each?.at === "number" && each.data && typeof each.data === "object") this.kept.set(key, each)
      }
    } catch {}
  }

  /** The summary for `params`: one read in the last `FRESH` as it is; else the one last read, at
   * once (with `asOf`, when it was read), while `read` reads it afresh; with none kept, as `read`
   * gives it. Only a finished summary is kept (never one that says the box is still reading its
   * usage). */
  async answer(params: URLSearchParams, read: () => Promise<Summary | undefined>): Promise<Summary | undefined> {
    const key = keptKey(params)
    const kept = this.kept.get(key)
    if (kept && this.now() - kept.at < FRESH && kept.at <= this.now()) return kept.data
    let reading = this.reading.get(key)
    if (!reading) {
      reading = read().then(data => {
        if (data && data.state !== "loading") this.keep(key, data)
        return data
      }).finally(() => this.reading.delete(key))
      this.reading.set(key, reading)
    }
    if (!kept) return reading
    reading.catch(() => {})
    return { ...kept.data, asOf: kept.at }
  }

  private keep(key: string, data: Summary) {
    this.kept.delete(key)
    this.kept.set(key, { at: this.now(), data })
    while (this.kept.size > MOST) this.kept.delete(this.kept.keys().next().value!)
    if (!this.file) return
    try {
      mkdirSync(dirname(this.file), { recursive: true, mode: 0o700 })
      writeFileSync(`${this.file}.new`, JSON.stringify(Object.fromEntries(this.kept)), { mode: 0o600 })
      renameSync(`${this.file}.new`, this.file)
    } catch {}
  }
}
