// One shipped bundle serves the bridge and its SQLite worker.
import { isMainThread } from "node:worker_threads"

if (!isMainThread && process.argv.includes("--insights-db-worker")) {
  const { serveInsightsDB } = await import("./usage/insights-db-worker")
  serveInsightsDB()
} else {
  await import("./main")
}
