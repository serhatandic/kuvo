import { readSavedSessions } from "../agents/store"
import { join } from "node:path"
import { UsageAccounts } from "./accounts"
import { InsightsScan } from "./insights-scan"
import { InsightsStore } from "./insights-store"
import { Catalog, Pricing } from "./pricing"
import type { DBRequest, InsightScanInput } from "./insights-db"

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
              store: () => readSavedSessions(join(input.home, "sessions")),
              bots: () => input.bots, computer: () => input.computer, engine: input.engine,
            })
          }
          result = await scanner.scan(pricing!, args[1])
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
