import { readSavedSessionHeaders } from "../agents/store"
import { savedMessages } from "../agents/transcript"
import { join } from "node:path"
import { UsageAccounts } from "./accounts"
import { InsightsScan } from "./insights-scan"
import { InsightsStore } from "./insights-store"
import { Catalog, Pricing } from "./pricing"
import type { DBRequest, InsightScanInput } from "./insights-db"

const sweep = typeof Bun === "undefined" ? undefined : (await import("bun:jsc")).gcAndSweep

export function serveInsightsDB() {
  let db: InsightsStore | undefined
  let pricing: Pricing | undefined
  let input: InsightScanInput
  let scanner: InsightsScan | undefined
  addEventListener("message", async ({ data }: MessageEvent<DBRequest>) => {
    const { id, method, args, catalog } = data
    try {
      if (catalog) {
        const { supplement, primary, secondary } = catalog
        pricing = new Pricing(supplement, new Catalog(primary), new Catalog(secondary))
      }
      let result: unknown
      switch (method) {
        case "open": db = new InsightsStore(args[0]); break
        case "close": db!.close(); break
        case "read": result = db!.read(args[0]); break
        case "write": db!.write(args[0], args[1]); break
        case "scan": {
          input = args[0]
          if (!scanner) {
            const accounts = new UsageAccounts(input.home)
            scanner = new InsightsScan({
              root: input.home, cache: input.cache, agents: () => accounts.all(),
              store: function* () {
                for (const row of readSavedSessionHeaders(join(input.home, "sessions"))) {
                  yield { ...row, messages: savedMessages(row.folder, "insight") }
                }
              },
              bots: () => input.bots, computer: () => input.computer, engine: input.engine,
            })
          }
          const [_, since, now, origins, handled] = args
          const index = db!
          index.price(pricing!, now)
          const { issues, transcripts, observations, more, selected } = await scanner.scan(pricing!, since, {
            read: key => index.read(key),
            write: entries => index.checkpoint(entries),
            reclaim: () => { sweep?.() },
            take: records => {
              index.price(pricing!, now)
              index.take(records, pricing!, now, undefined, undefined, undefined, origins)
            },
            attribute: lineage => index.attribute(lineage),
          }, new Set(handled))
          index.take([], pricing!, now)
          result = { issues, transcripts, observations, more, ...(more ? { selected } : {}) }
          break
        }
        case "cursor": result = db!.cursor(...args); break
        case "batch": result = db!.batch(...args); break
        case "choices": result = db!.choices(args[0]); break
        case "view": result = db!.view(...args); break
        case "summary": result = db!.summary(...args); break
        case "price": db!.price(pricing!, args[0]); break
        case "attribute": db!.attribute(...args); break
        case "take":
          db!.price(pricing!, args[1])
          db!.take(args[0], pricing!, args[1], args[2], args[3], args[4], args[5])
          break
      }
      postMessage({ id, result })
    } catch (error) {
      postMessage({ id, error: error instanceof Error ? error.message : String(error) })
    }
  })
}

if (process.argv.includes("--insights-db-source-worker")) serveInsightsDB()
