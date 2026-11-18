// SQLite and pricing run on one worker. Its message queue orders writes, price changes and reads.
import type { Pricing } from "./pricing"
import type { InsightRecord } from "./insight-records"
import type { CallsPage, Filter, InsightsStore, Overview, Route, SessionsPage } from "./insights-store"

export type { Filter } from "./insights-store"
export type PricingSnapshot = {
  supplement: Pricing["supplement"]
  primary: Pricing["primary"]["entries"]
  secondary: Pricing["secondary"]["entries"]
}

type Commands = {
  open: [file: string]
  close: []
  read: [key: string]
  write: [key: string, value: unknown]
  cursor: [sender: string]
  batch: [after: number]
  choices: [computers: string[]]
  view: [now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number]
  price: [now: number]
  take: [records: InsightRecord[], now: number, sender?: string, cursor?: number, epoch?: string]
}
type Method = keyof Commands
export type DBRequest = { id: number; catalog?: PricingSnapshot } & {
  [Each in Method]: { method: Each; args: Commands[Each] }
}[Method]
type Command = { [Each in Method]: [method: Each, args: Commands[Each]] }[Method]
type Reply = { id: number; result?: unknown; error?: string }

export class InsightsDB {
  private worker: Worker
  private opened: Promise<void>
  private failure?: Error
  private id = 0
  private pricing?: Pricing
  private pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>()

  constructor(file: string, bundle?: string) {
    this.worker = bundle
      ? new Worker(bundle, { argv: ["--insights-db-worker"] })
      : new Worker(new URL("./insights-db-worker.ts", import.meta.url), { argv: ["--insights-db-source-worker"] })
    this.worker.onmessage = ({ data }: MessageEvent<Reply>) => {
      const waiting = this.pending.get(data.id)
      if (!waiting) return
      this.pending.delete(data.id)
      if (data.error !== undefined) waiting.reject(new Error(data.error))
      else waiting.resolve(data.result)
    }
    this.worker.onerror = (event) => {
      this.failure = new Error(event.message || "The Insights worker stopped.")
      for (const waiting of this.pending.values()) waiting.reject(this.failure)
      this.pending.clear()
    }
    this.opened = this.send(["open", [file]])
    void this.opened.catch(() => {})
  }

  private send<T>([method, args]: Command, pricing?: Pricing): Promise<T> {
    if (this.failure) return Promise.reject(this.failure)
    const catalog: PricingSnapshot | undefined = pricing && pricing !== this.pricing
      ? { supplement: pricing.supplement, primary: pricing.primary.entries, secondary: pricing.secondary.entries }
      : undefined
    return new Promise((resolve, reject) => {
      const id = ++this.id
      this.pending.set(id, { resolve: (value) => resolve(value as T), reject })
      try {
        this.worker.postMessage({ id, method, args, catalog })
        if (pricing) this.pricing = pricing
      } catch (error) {
        this.pending.delete(id)
        reject(error)
      }
    })
  }

  private async call<T>(command: Command, pricing?: Pricing): Promise<T> {
    await this.opened
    return this.send<T>(command, pricing)
  }

  async close() {
    try { await this.call(["close", []]) }
    finally { this.worker.terminate() }
  }
  read<T>(key: string) { return this.call<T | undefined>(["read", [key]]) }
  write(key: string, value: unknown) { return this.call<void>(["write", [key, value]]) }
  cursor(sender: string) { return this.call<{ after: number; epoch?: string }>(["cursor", [sender]]) }
  batch(after: number) { return this.call<ReturnType<InsightsStore["batch"]>>(["batch", [after]]) }
  choices(computers: string[]) { return this.call<ReturnType<InsightsStore["choices"]>>(["choices", [computers]]) }
  price(pricing: Pricing, now: number) { return this.call<void>(["price", [now]], pricing) }
  take(records: InsightRecord[], pricing: Pricing, now: number, sender?: string, cursor?: number, epoch?: string) {
    return this.call<void>(["take", [records, now, sender, cursor, epoch]], pricing)
  }

  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "overview", offset: number): Promise<Overview>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "sessions", offset: number): Promise<SessionsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "detail", offset: number): Promise<CallsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number): Promise<Overview | SessionsPage | CallsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number) {
    return this.call<Overview | SessionsPage | CallsPage>(["view", [now, zone, period, filter, computers, route, offset]])
  }
}
