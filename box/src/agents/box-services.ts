import { followEvents } from "../shared/event-stream"
// Compatibility for local clients that use the bridge's original usage URLs and event feed.
// Its only input to the reader is a sign-in change or a window an agent reported during a turn.
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { askServices, readServices } from "../shared/box-services"
import { emit } from "./events"
import type { LimitWindow } from "../usage/limits"

export class BoxServicesClient {
  private readonly generations = new Map<string, number>()
  private readonly observations = new Map<string, Promise<void>>()

  generation(provider: string): number {
    return this.generations.get(provider) ?? 0
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
    if (!target) return
    try {
      const response = await askServices(target, path, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
        signal: AbortSignal.timeout(2000),
      })
      await response.body?.cancel()
    } catch {}
  }

  observe(provider: string, update: { signedIn?: boolean; window?: Omit<LimitWindow, "read" | "source">; signIn: string | null }) {
    if (update.signedIn !== undefined) this.generations.set(provider, this.generation(provider) + 1)
    const generation = this.generation(provider)
    const waiting = this.observations.get(provider) ?? Promise.resolve()
    const sent = waiting.then(() => {
      if (this.generation(provider) === generation) return this.tell("api/kuvo/usage-observed", { provider, ...update })
    }).finally(() => {
      if (this.observations.get(provider) === sent) this.observations.delete(provider)
    })
    this.observations.set(provider, sent)
    return sent
  }

  attribution(): { computer?: string; accounts?: Record<string, string> } {
    try { if (this.run) return JSON.parse(readFileSync(join(this.run, "usage-attribution.json"), "utf8")) } catch {}
    return {}
  }

  async follow(signal: AbortSignal) {
    const run = this.run
    if (!run) return
    await followEvents<{ type: string; data?: Record<string, unknown> }>(async listening => {
      const target = readServices(run)
      if (!target) throw new Error("Box services are starting.")
      return askServices(target, "api/kuvo/usage-events", { signal: listening })
    }, ({ type, data }) => {
      if (type === "server.connected") emit("kuvo.limits.updated", {})
      if (type === "integration.updated" || (typeof type === "string" && /^kuvo\.(usage|limits)\./.test(type))) emit(type, data ?? {})
    }, signal)
  }
}
