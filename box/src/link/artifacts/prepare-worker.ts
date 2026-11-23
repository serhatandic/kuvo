import { historyWorkerEntry } from "../server/history-worker-entry"
import type { prepareArtifactServices } from "./prepare"

type Prepared = Awaited<ReturnType<typeof prepareArtifactServices>>
type Reply = { result: Prepared } | { error: string }

export async function prepareOnWorker(...args: Parameters<typeof prepareArtifactServices>): Promise<Prepared> {
  const worker = new Worker(historyWorkerEntry(), { smol: true, argv: ["--artifact-history-worker"] })
  const stopped = new Promise<void>(resolve => worker.addEventListener("close", () => resolve(), { once: true }))
  try {
    return await new Promise((resolve, reject) => {
      worker.onmessage = ({ data }: MessageEvent<Reply | "ready">) => {
        if (data === "ready") worker.postMessage(args)
        else if ("error" in data) reject(new Error(data.error))
        else resolve(data.result)
      }
      worker.onerror = event => reject(new Error(event.message || "The artifact history worker stopped."))
      worker.addEventListener("close", event => reject(new Error(`The artifact history worker exited (${event.code}).`)), { once: true })
    })
  } finally {
    worker.terminate()
    await stopped
  }
}
