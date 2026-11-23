// Every box test runs with a home of its own, never this computer's ~/.kuvo-code (test/safety.ts, set
// here before any test file loads, and taken away when the run ends). A test that sets KUVO_HOME
// itself and imports the link afterwards still gets this one: the link's modules were loaded already.

import "./safety"
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { afterAll } from "bun:test"
import { reachedRealAgents, stopTestProcesses } from "../bridge-tests/test-safety.js"

// Direct bun test runs need the same generated inputs as the test wrapper.
const build = fileURLToPath(new URL("../../scripts/build-built.sh", import.meta.url))
const pin = readFileSync(new URL("../.bun-version", import.meta.url), "utf8").trim()
const BUN = process.env.BUN ?? (Bun.version === pin ? process.execPath : undefined)
const built = spawnSync("sh", [build, "--if-needed"], { env: { ...process.env, BUN }, stdio: "inherit" })
if (built.status !== 0) throw new Error("Could not build test inputs. Run scripts/build-built.sh.", { cause: built.error })

// (Once, when the last test file is done: a preload's hook is the whole run's. Bun's runner ends
// without telling anything else.)
afterAll(async () => {
  await stopTestProcesses()
  process.emit("kuvo-test-folders-removed" as any)
})

// A run that reached your real coding agents (test-safety.js refused it) fails, however its tests
// took the refusal.
afterAll(() => {
  const reached = reachedRealAgents()
  if (reached.length) throw new Error(`This run reached your real coding agents ${reached.length} time(s). The first: ${reached[0]}`)
})
