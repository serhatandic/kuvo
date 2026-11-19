import { ClaudeAgent } from "../../src/agents/agents/claude"
import { ClaudePlan } from "../../src/usage/claude-limits"

export function claudeProbe(agent: ClaudeAgent, version: string) {
  return new ClaudePlan({
    byToken: ClaudeAgent.keychain,
    home: agent.instance.home,
    configHome: () => agent.configHome,
    token: () => agent.token(),
    signIn: () => agent.limitsSignIn(),
    version: async () => version,
  }).limits
}
