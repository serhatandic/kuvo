import { followEvents } from "../shared/event-stream"
// Compatibility for local clients that use the bridge's original usage URLs and event feed.
// Its only input to the reader is a sign-in change or a window an agent reported during a turn.
import { readFileSync } from "node:fs"
import { randomUUID } from "node:crypto"
import { join } from "node:path"
import { askServices, readServices } from "../shared/box-services"
import { emit } from "./events"
import type { LimitsSource, LimitWindow } from "../usage/limits"

type WindowObservation = Omit<LimitWindow, "read" | "source">
type AccountObservation = { signedIn: boolean; signIn: string | null; revision: string }
type Observation = { account?: AccountObservation; sentTo?: string; windows: Map<string, { window: WindowObservation; signIn: string }> }

export class BoxServicesClient {
  private readonly generations = new Map<string, number>()
  private readonly observations = new Map<string, Observation>()
  private sending?: Promise<void>

  generation(provider: string): number {
    return this.generations.get(provider) ?? 0
  }

  observeTurns(provider: string, source: LimitsSource) {
    const generation = source.generation = () => this.generation(provider)
    source.heard = (window, from) => {
      if (from !== undefined && from !== generation()) return
      const signIn = source.signIn?.()
      if (signIn) void this.observe(provider, { window, signIn })
    }
  }

  constructor(private readonly run?: string) {}

  async forward(request: Request): Promise<Response> {
    const target = this.run && readServices(this.run)
    if (target) {
      const url = new URL(request.url)
      const method = request.method
      try {
        return await askServices(target, url.pathname + url.search, {
          method, headers: {
            "content-type": request.headers.get("content-type") ?? "application/json",
            "x-kuvo-sign-in-owner": request.headers.get("x-kuvo-sign-in-owner") ?? "",
          },
          body: method === "GET" || method === "HEAD" ? undefined : await request.arrayBuffer(),
          signal: request.signal,
        })
      } catch {}
    }
    return Response.json({ message: "The box's services are starting. Try again in a moment." }, { status: 503 })
  }

  async tell(path: string, body: unknown) {
    const target = this.run && readServices(this.run)
    if (target) await this.send(target, path, body)
  }

  private async send(target: NonNullable<ReturnType<typeof readServices>>, path: string, body: unknown) {
    try {
      const response = await askServices(target, path, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
        signal: AbortSignal.timeout(2000),
      })
      await response.body?.cancel()
      return response.ok
    } catch { return false }
  }

  observe(provider: string, update: { signedIn?: boolean; window?: WindowObservation; signIn: string | null }) {
    if (!this.run) return
    let pending = this.observations.get(provider)
    if (!pending) this.observations.set(provider, pending = { windows: new Map() })
    if (update.signedIn !== undefined) {
      // Finishing a sign-in check or learning a new account label must not reset readings
      // already accepted for these credentials. An unsent observation still needs flushing.
      if (pending.account?.signedIn === update.signedIn && pending.account.signIn === update.signIn) return this.flush()
      if (!update.signedIn || (pending.account && (!pending.account.signedIn || pending.account.signIn !== update.signIn))) {
        this.generations.set(provider, this.generation(provider) + 1)
        pending.windows.clear()
      }
      pending.account = { signedIn: update.signedIn, signIn: update.signIn, revision: randomUUID() }
      pending.sentTo = undefined
    } else if (update.window && update.signIn) {
      pending.windows.set(update.window.id, { window: update.window, signIn: update.signIn })
    }
    return this.flush()
  }

  private flush(): Promise<void> | undefined {
    if (this.sending) return this.sending
    const target = this.run && readServices(this.run)
    if (!target) return
    const path = "api/kuvo/usage-observed"
    this.sending = Promise.all([...this.observations].map(async ([provider, pending]) => {
      const account = pending.account
      if (!account) return
      if (pending.sentTo !== target.password) {
        if (!await this.send(target, path, { provider, ...account, bridge: process.pid }) || pending.account !== account) return
        pending.sentTo = target.password
      }
      for (const [id, observation] of pending.windows) {
        if (pending.account !== account) break
        const relevant = account.signedIn && observation.signIn === account.signIn
        if (relevant && !await this.send(target, path, { provider, ...observation, revision: account.revision, bridge: process.pid })) {
          pending.sentTo = undefined
          break
        }
        if (pending.windows.get(id) === observation) pending.windows.delete(id)
      }
    })).then(() => {}).finally(() => { this.sending = undefined })
    return this.sending
  }

  attribution(): { computer?: string; accounts?: Record<string, string> } {
    try { if (this.run) return JSON.parse(readFileSync(join(this.run, "usage-attribution.json"), "utf8")) } catch {}
    return {}
  }

  async follow(signal: AbortSignal) {
    const run = this.run
    if (!run) return
    const retry = setInterval(() => void this.flush(), 2_000).unref()
    try { await followEvents<{ type: string; data?: Record<string, unknown> }>(async listening => {
      const target = readServices(run)
      if (!target) throw new Error("Box services are starting.")
      return askServices(target, "api/kuvo/usage-events", { signal: listening })
    }, ({ type, data }) => {
      if (type === "server.connected") emit("kuvo.limits.updated", {})
      if (type === "integration.updated" || (typeof type === "string" && /^kuvo\.(usage|limits)\./.test(type))) emit(type, data ?? {})
    }, signal, () => void this.flush()) } finally { clearInterval(retry) }
  }
}
