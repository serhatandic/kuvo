// A bridge's tools relay, as a process of its own (toolrelay.test.ts), on the box in KUVO_HOME, with
// Kuvo Code's tools the program given: `first` gives sessions' agents their tools (says how each
// reaches them) and, told to, ends, as a bridge making way does; `next` starts as the next bridge
// does, taking up `sessions` (its agents still run), and says when it listens.
//
// usage: bun relay-bridge.ts <first|next> <program> <session...>

import { setToolProgram } from "../../../src/agents/kuvotools"
import { keepRelayKeys, relayedTools, relayPort } from "../../../src/agents/toolrelay"

const [mode, program, ...sessions] = process.argv.slice(2)
setToolProgram(JSON.stringify({ mcp: { servers: { "kuvo-browser": { command: [process.execPath, program], environment: {} } } } }))
if (mode === "first") {
  console.log(JSON.stringify(Object.fromEntries(sessions.map((session) => [session, relayedTools(session, {})]))))
} else {
  relayPort()
  keepRelayKeys(new Set(sessions))
  console.log(JSON.stringify({ listening: relayPort() }))
}
process.stdin.on("data", () => process.exit(0))
