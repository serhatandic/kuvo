// Display words shared by answers; clients draw these titles as said.
const short: Record<string, string> = {
  "claude-code": "Claude", "codex-cli": "Codex", "grok-cli": "Grok", cursor: "Cursor", antigravity: "Antigravity",
  "github-copilot": "Copilot", openrouter: "OpenRouter", zai: "Z.ai", "zai-coding-plan": "Z.ai", "opencode-go": "OpenCode Go",
};
const cards: Record<string, string> = { claude: "Claude", codex: "Codex", cursor: "Cursor", antigravity: "Antigravity", copilot: "Copilot", grok: "Grok", opencode: "OpenCode", openrouter: "OpenRouter", zai: "Z.ai", ollama: "Ollama" };
/** Unknown accounts retain their provider's supplied name. */
export function workAccountTitle(provider: string, name: string, label?: string): string {
  const base = provider.split("@")[0]!;
  const title = Object.hasOwn(short, base) ? short[base]! : name;
  return label && label !== title ? `${title} · ${label}` : title;
}
/** Unknown services use a capitalized id, matching the former cards. */
export function spendTitle(service: string): string {
  return Object.hasOwn(cards, service) ? cards[service]! : service.replace(/\b\w/g, letter => letter.toUpperCase());
}

/** Plan names formerly drawn by clients; unknown services keep their id with an initial capital. */
export function serviceTitle(id: string): string {
  const names: Record<string, string> = { codex: "ChatGPT", grok: "SuperGrok", copilot: "Copilot", openrouter: "OpenRouter", zai: "Z.ai", ollama: "Ollama", opencode: "OpenCode Go" };
  return Object.hasOwn(names, id) ? names[id]! : (id.charAt(0).toUpperCase() + id.slice(1));
}
