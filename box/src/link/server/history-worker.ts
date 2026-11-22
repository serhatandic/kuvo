// These jobs write their own indexes while the resident continues serving requests.
if (process.argv.includes("--artifact-history-worker")) {
  const { prepareArtifactServices } = await import("../artifacts/prepare")
  addEventListener("message", async ({ data }: MessageEvent<Parameters<typeof prepareArtifactServices>>) => {
    try { postMessage({ result: await prepareArtifactServices(...data) }) }
    catch (error) { postMessage({ error: error instanceof Error ? error.message : String(error) }) }
  }, { once: true })
  postMessage("ready")
} else if (process.argv.includes("--insights-db-source-worker")) {
  const { serveInsightsDB } = await import("../../usage/insights-db-worker")
  serveInsightsDB()
  postMessage("ready")
}

export {}
