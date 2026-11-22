import { ClaudeAgent } from "../../src/agents/agents/claude"
import { ClaudePlan, credentialsIn } from "../../src/usage/claude-limits"

/** The usage reader's Claude limits for `agent`'s account: asked of `claude` (a stand-in, support/agent-programs.ts), its resets read from its sign-in file unless `keychain`. */
export function claudeProbe(agent: ClaudeAgent, version: string, options: { claude?: string; keychain?: boolean } = {}) {
  return new ClaudePlan({
    recordHome: agent.instance.home ?? process.env.CLAUDE_CONFIG_DIR ?? process.env.HOME!,
    keychain: options.keychain ?? false,
    program: () => (options.claude ? { executable: options.claude, home: agent.instance.home, cwd: agent.context.home } : undefined),
    signIn: () => credentialsIn(agent.configHome),
    version: async () => version,
  }).limits
}
