// The names Insights gives what it groups by: one name per provider and one per model, however a
// source wrote its id ("claude-opus-5-5", "claude-opus-5.5-20261101", "anthropic/claude-opus-5.5",
// "Claude Opus 5 5" and "Opus 5.5" are all "Opus 5.5"). Clients draw these as said.

const PROVIDERS: Record<string, string> = {
  claude: "Claude Code", codex: "Codex", cursor: "Cursor", grok: "Grok", antigravity: "Antigravity",
  opencode: "OpenCode", "opencode-go": "OpenCode Go", openai: "OpenAI", anthropic: "Anthropic", google: "Google",
  openrouter: "OpenRouter", "github-copilot": "GitHub Copilot", copilot: "GitHub Copilot", xai: "xAI", zai: "Z.ai",
  "zai-coding-plan": "Z.ai", ollama: "Ollama", mistral: "Mistral", deepseek: "DeepSeek", engine: "Kuvo",
}

const capital = (word: string) => word.charAt(0).toUpperCase() + word.slice(1)

/** A provider's name: "Claude Code" for `claude`; one this doesn't know, its id with a capital. */
export function providerName(id: string): string {
  const base = id.split("@")[0]!
  return Object.hasOwn(PROVIDERS, base) ? PROVIDERS[base]! : base.split(/[-_]/).filter(Boolean).map(capital).join(" ") || "Unknown"
}

const CLAUDE = new Set(["opus", "sonnet", "haiku", "fable"])
const UPPER = new Set(["gpt", "glm", "oss"])
const version = (part: string) => /^\d{1,2}$/.test(part)

/** Words with the version's numbers run together: ["grok", "4", "4"] → ["Grok", "4.4"]. */
function words(parts: string[]): string[] {
  const out: string[] = []
  for (let i = 0; i < parts.length; i++) {
    if (version(parts[i]!)) {
      let run = parts[i]!
      while (i + 1 < parts.length && version(parts[i + 1]!)) run += "." + parts[++i]!
      out.push(run)
    } else if (/^o\d/.test(parts[i]!)) out.push(parts[i]!)
    else out.push(UPPER.has(parts[i]!) ? parts[i]!.toUpperCase() : capital(parts[i]!))
  }
  return out
}

/** A model's one name and the key it's grouped and narrowed by. */
export function modelName(id: string | null | undefined): { key: string; name: string } {
  let text = (id ?? "").trim().toLowerCase()
  text = text.slice(text.lastIndexOf("/") + 1).replace(/\[[^\]]*\]$/, "").replace(/@.*$/, "")
  const parts = text.replace(/[\s_.:]+/g, "-").split("-").filter(Boolean)
  // A date or a "latest" after the version says when, not which.
  while (parts.length > 1 && (/^\d{6,8}$/.test(parts.at(-1)!) || parts.at(-1) === "latest")) parts.pop()
  if (parts.length > 3 && /^\d{4}$/.test(parts.at(-3)!) && /^\d{2}$/.test(parts.at(-2)!) && /^\d{2}$/.test(parts.at(-1)!)) parts.splice(-3)
  if (!parts.length) return { key: "unknown", name: "Unknown model" }
  let name: string
  const family = parts.find((part) => CLAUDE.has(part))
  if (family) {
    // Claude's: its family, then its version ("claude-3-5-sonnet" too), and anything else said after.
    const numbers = parts.filter(version)
    const rest = parts.filter((part) => part !== "claude" && part !== family && !version(part))
    name = [capital(family), numbers.join("."), ...words(rest)].filter(Boolean).join(" ")
  } else if (parts[0] === "gpt") {
    const [, ...after] = parts
    const numbers: string[] = []
    while (after.length && (version(after[0]!) || (!numbers.length && /^\d+[a-z]$/.test(after[0]!)))) numbers.push(after.shift()!)
    name = [numbers.length ? `GPT-${numbers.join(".")}` : "GPT", ...words(after)].join(" ")
  } else {
    name = words(parts).join(" ")
  }
  return { key: name.toLowerCase().replace(/[^a-z0-9.]+/g, "-"), name }
}
