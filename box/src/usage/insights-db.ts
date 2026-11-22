// SQLite and pricing run on one worker. Its message queue orders writes, price changes and reads.
import type { Pricing } from "./pricing"
import type { InsightRecord } from "./insight-records"
import type { InsightBot, InsightsScan, Lineage } from "./insights-scan"
import type { CallsPage, Filter, InsightsStore, OriginScope, Overview, Route, Scope, SessionsPage } from "./insights-store"
import type { SummaryFacts } from "./insights-summary"

export type { Filter, Scope } from "./insights-store"
export type PricingSnapshot = {
  supplement: Pricing["supplement"]
  primary: Pricing["primary"]["entries"]
  secondary: Pricing["secondary"]["entries"]
}

export interface InsightScanInput {
  home: string
  cache: string
  bots: InsightBot[]
  computer: string
  engine?: string
}

type Commands = {
  open: [file: string]
  close: []
  read: [key: string]
  write: [key: string, value: unknown]
  scan: [input: InsightScanInput, since: number, now: number, origins: OriginScope]
  cursor: [sender: string, scope?: string]
  batch: [after: number, historical?: string[]]
  choices: [computers: string[]]
  view: [now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number, eligible?: string[]]
  summary: [now: number, zone: string, period: number, scope: Scope, computers: string[], eligible?: string[]]
  price: [now: number]
  attribute: [lineage: [string, Lineage][]]
  take: [records: InsightRecord[], now: number, sender?: string, cursor?: number, epoch?: string, origins?: OriginScope]
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
  private closing?: Promise<void>
  private id = 0
  private pricing?: Pricing
  private pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>()

  constructor(private file: string, private bundle?: string) {
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
      this.fail(new Error(event.message || "The Insights worker stopped."))
      this.worker.terminate()
    }
    this.worker.addEventListener("close", (event) => {
      this.fail(new Error(`The Insights worker exited (code ${event.code}).`))
    })
    this.opened = this.send(["open", [file]])
    void this.opened.catch((error) => {
      this.fail(error)
      this.worker.terminate()
    })
  }

  private fail(error: Error) {
    this.failure ??= error
    for (const waiting of this.pending.values()) waiting.reject(this.failure)
    this.pending.clear()
  }

  reopen() {
    return this.failure && !this.closing ? new InsightsDB(this.file, this.bundle) : this
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
    if (this.closing) throw new Error("The Insights index is closed.")
    return this.send<T>(command, pricing)
  }

  close() {
    return this.closing ??= this.finish()
  }

  private async finish() {
    try {
      await this.opened
      await this.send(["close", []])
    } catch (error) {
      if (!this.failure) throw error
    } finally {
      this.worker.terminate()
    }
  }
  read<T>(key: string) { return this.call<T | undefined>(["read", [key]]) }
  write(key: string, value: unknown) { return this.call<void>(["write", [key, value]]) }
  cursor(sender: string, scope?: string) { return this.call<{ after: number; epoch?: string }>(["cursor", [sender, scope]]) }
  batch(after: number, historical?: string[]) { return this.call<ReturnType<InsightsStore["batch"]>>(["batch", [after, historical]]) }
  choices(computers: string[]) { return this.call<ReturnType<InsightsStore["choices"]>>(["choices", [computers]]) }
  scan(input: InsightScanInput, pricing: Pricing, since: number, now: number, origins: OriginScope) {
    return this.call<Awaited<ReturnType<InsightsScan["scan"]>>["issues"]>(["scan", [input, since, now, origins]], pricing)
  }

  price(pricing: Pricing, now: number) { return this.call<void>(["price", [now]], pricing) }
  attribute(lineage: [string, Lineage][]) { return this.call<void>(["attribute", [lineage]]) }
  take(records: InsightRecord[], pricing: Pricing, now: number, sender?: string, cursor?: number, epoch?: string, origins?: OriginScope) {
    return this.call<void>(["take", [records, now, sender, cursor, epoch, origins]], pricing)
  }

  summary(now: number, zone: string, period: number, scope: Scope, computers: string[], eligible?: string[]) {
    return this.call<SummaryFacts>(["summary", [now, zone, period, scope, computers, eligible]])
  }

  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "overview", offset: number, eligible?: string[]): Promise<Overview>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "sessions", offset: number, eligible?: string[]): Promise<SessionsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: "detail", offset: number, eligible?: string[]): Promise<CallsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number, eligible?: string[]): Promise<Overview | SessionsPage | CallsPage>
  view(now: number, zone: string, period: number, filter: Filter, computers: string[], route: Route, offset: number, eligible?: string[]) {
    return this.call<Overview | SessionsPage | CallsPage>(["view", [now, zone, period, filter, computers, route, offset, eligible]])
  }
}
