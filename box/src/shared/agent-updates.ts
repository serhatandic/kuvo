// The box's "Update coding agents automatically" setting (docs/architecture/agent-accounts.md,
// "Install, updates and notices"), shared by the link (Settings › Computer) and the agents bridge,
// which starts each agent with its own updater turned off where it can while it's off. Off unless
// you turn it on.
import { readFileSync } from "node:fs"
import { join } from "node:path"

export const agentUpdatesFile = (home: string) => join(home, "box", "agents-update-themselves.json")

/** Whether the coding agents update themselves here (you turned it on). */
export function agentsUpdateThemselves(home = process.env.KUVO_HOME): boolean {
  try {
    return Boolean(home) && JSON.parse(readFileSync(agentUpdatesFile(home!), "utf8")) === true
  } catch {
    return false
  }
}
