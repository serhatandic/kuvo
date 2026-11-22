// SQLite and pricing run on one worker. Its message queue orders writes, price changes and reads.
import type { Pricing } from "./pricing"
import type { InsightRecord } from "./insight-records"
import type { InsightBot, Lineage } from "./insights-scan"
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
  scan: [input: InsightScanInput, since: number, now: number, origins: OriginScope, handled: string[]]
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

// Defined by build-box.sh for the shipped link. Source tests use the same worker's entrypoint.
declare const __KUVO_INSIGHTS_WORKER__: string | undefined
let embeddedWorker: string | undefined
function workerEntry() {
  if (typeof __KUVO_INSIGHTS_WORKER__ !== "string") return new URL("./insights-db-worker.ts", import.meta.url)
  return embeddedWorker ??= URL.createObjectURL(new Blob([__KUVO_INSIGHTS_WORKER__], { type: "application/javascript" }))
}

export class InsightsDB {
  private worker?: Worker
  private opened?: Promise<void>
  private parking?: ReturnType<typeof setTimeout>
  private retired?: Promise<void>
  private failure?: Error
  private closing?: Promise<void>
  private id = 0
  private pricing?: Pricing
  private pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>()

  constructor(private file: string, private readonly entry?: string) {}

  private start() {
    const worker = this.worker = new Worker(this.entry ?? workerEntry(), { smol: true, argv: this.entry ? [] : ["--insights-db-source-worker"] })
    worker.onmessage = ({ data }: MessageEvent<Reply>) => {
      const waiting = this.pending.get(data.id)
      if (!waiting) return
      this.pending.delete(data.id)
      if (data.error !== undefined) waiting.reject(new Error(data.error))
      else waiting.resolve(data.result)
      if (!this.pending.size && !this.closing) {
        this.parking = setTimeout(() => {
          if (this.worker !== worker || this.pending.size || this.closing) return
          this.park()
        }, 5_000)
        this.parking.unref?.()
      }
    }
    worker.onerror = (event) => {
      if (this.worker !== worker) return
      this.fail(new Error(event.message || "The Insights worker stopped."))
      this.park()
    }
    worker.addEventListener("close", (event) => {
      if (this.worker === worker) {
        this.worker = undefined
        this.fail(new Error(`The Insights worker exited (code ${event.code}).`))
      }
    })
    return this.opened = this.send<void>(["open", [this.file]]).catch((error) => {
      this.fail(error)
      this.park()
      throw error
    })
  }

  private park() {
    clearTimeout(this.parking)
    const worker = this.worker
    this.worker = this.opened = this.pricing = undefined
    if (worker) {
      this.retired = new Promise(resolve => {
        worker.addEventListener("close", () => resolve(), { once: true })
        worker.terminate()
      })
    }
  }

  private fail(error: Error) {
    this.failure ??= error
    for (const waiting of this.pending.values()) waiting.reject(this.failure)
    this.pending.clear()
  }

  reopen() {
    return this.failure && !this.closing ? new InsightsDB(this.file, this.entry) : this
  }

  private send<T>([method, args]: Command, pricing?: Pricing): Promise<T> {
    if (this.failure) return Promise.reject(this.failure)
    clearTimeout(this.parking)
    const catalog: PricingSnapshot | undefined = pricing && pricing !== this.pricing
      ? { supplement: pricing.supplement, primary: pricing.primary.entries, secondary: pricing.secondary.entries }
      : undefined
    return new Promise((resolve, reject) => {
      const id = ++this.id
      this.pending.set(id, { resolve: (value) => resolve(value as T), reject })
      try {
        this.worker!.postMessage({ id, method, args, catalog })
        if (pricing) this.pricing = pricing
      } catch (error) {
        this.pending.delete(id)
        reject(error)
      }
    })
  }

  private async call<T>(command: Command, pricing?: Pricing): Promise<T> {
    if (this.closing) throw new Error("The Insights index is closed.")
    if (this.failure) throw this.failure
    clearTimeout(this.parking)
    if (this.retired) await this.retired
    if (this.closing) throw new Error("The Insights index is closed.")
    await (this.opened ?? this.start())
    if (this.closing) throw new Error("The Insights index is closed.")
    return this.send<T>(command, pricing)
  }

  close() {
    return this.closing ??= this.finish()
  }

  private async finish() {
    try {
      if (this.opened) {
        await this.opened
        await this.send(["close", []])
      }
    } catch (error) {
      if (!this.failure) throw error
    } finally {
      this.park()
      await this.retired
    }
  }
  read<T>(key: string) { return this.call<T | undefined>(["read", [key]]) }
  write(key: string, value: unknown) { return this.call<void>(["write", [key, value]]) }
  cursor(sender: string, scope?: string) { return this.call<{ after: number; epoch?: string }>(["cursor", [sender, scope]]) }
  batch(after: number, historical?: string[]) { return this.call<ReturnType<InsightsStore["batch"]>>(["batch", [after, historical]]) }
  choices(computers: string[]) { return this.call<ReturnType<InsightsStore["choices"]>>(["choices", [computers]]) }
  async scan(input: InsightScanInput, pricing: Pricing, since: number, now: number, origins: OriginScope) {
    const issues = new Set<string>()
    const handled: string[] = []
    let transcripts = 0
    let observations = 0
    for (;;) {
      const part = await this.call<{ issues: string[]; transcripts: number; observations: number; more: boolean; selected?: string[] }>(
        ["scan", [input, since, now, origins, handled]], pricing)
      for (const issue of part.issues) issues.add(issue)
      transcripts += part.transcripts
      observations += part.observations
      if (!part.more) return { issues: [...issues], transcripts, observations }
      handled.push(...part.selected!)
      // The durable file checkpoints keep finished work. Release the decoder and SQLite pages
      // before seeking more history, unless a concurrent request is using this worker.
      if (!this.pending.size && !this.closing) this.park()
    }
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
