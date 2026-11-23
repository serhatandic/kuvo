// Your accounts with each coding agent (docs/architecture/agent-accounts.md): an account is one
// agent pointed at one folder on one box. The first is the agent's own sign-in, where it always
// keeps it (`~/.claude`, `~/.codex`, `~/.grok`, on a Mac and on Linux alike, shared with Terminal;
// Cursor's and Antigravity's in the bridge's folder); each one you add gets a folder of its own in
// the bridge's (`<agent>/accounts/<id>`), which the agent is pointed at (CLAUDE_CONFIG_DIR,
// CODEX_HOME, GROK_HOME…), as T3 Code runs a second instance. An added account's models and
// sessions are its own: its provider is `<agent's provider>@<id>`.

import { randomBytes } from "node:crypto"
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, isAbsolute, join } from "node:path"
import { additiveListFormat, UnsupportedFormatVersion } from "../shared/format-version"

export type AgentKind = "claude" | "codex" | "cursor" | "grok" | "antigravity"

export const kinds: AgentKind[] = ["claude", "codex", "cursor", "grok", "antigravity"]

export const baseProvider: Record<AgentKind, string> = {
  claude: "claude-code",
  codex: "codex-cli",
  cursor: "cursor",
  grok: "grok-cli",
  antigravity: "antigravity",
}

export interface AccountRecord {
  kind: AgentKind
  /** `default` for the agent's own sign-in; otherwise a short random ID. */
  id: string
  /** What you called it ("Work"); an added account without one goes by its sign-in. */
  label?: string
  /** Its folder, within the bridge's, where an earlier version kept it
   * (claude-compat.ts); else `<agent>/accounts/<id>`. */
  folder?: string
}

/** One account as an agent runs it. */
export interface AgentAccount {
  id: string
  label?: string
  /** The folder the agent keeps this account in; none for the default (the agent's own). */
  home?: string
}

export function providerID(kind: AgentKind, id: string): string {
  return id === "default" ? baseProvider[kind] : `${baseProvider[kind]}@${id}`
}

/** The agent and account a provider ID names, if it's one of theirs. */
export function parseProvider(provider: string): { kind: AgentKind; id: string } | undefined {
  const [base, id] = provider.split("@", 2)
  const kind = kinds.find((kind) => baseProvider[kind] === base)
  if (!kind) return undefined
  return { kind, id: id ?? "default" }
}

const accountFormat = additiveListFormat("Managed accounts")

export class Accounts {
  private records: AccountRecord[]

  constructor(private readonly root: string) {
    this.records = this.read()
  }

  private read(): AccountRecord[] {
    try {
      const saved = accountFormat.read(JSON.parse(readFileSync(this.file, "utf8"))) as AccountRecord[]
      return saved.filter((record) => kinds.includes(record?.kind) && record?.id && record.id !== "default" && (record.folder === undefined || (typeof record.folder === "string" && !isAbsolute(record.folder) && !record.folder.split(/[\\/]/).includes(".."))))
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error
      return []
    }
  }

  private get file() {
    return join(this.root, "accounts.json")
  }

  /** Each agent's default account, then the ones you added, in the order you added them. */
  all(): AccountRecord[] {
    return [...kinds.map((kind) => ({ kind, id: "default" })), ...this.records]
  }

  /** The folder an added account is kept in; none for the first, which is the agent's own. */
  home(record: AccountRecord): string | undefined {
    if (record.id === "default") return undefined
    return record.folder ? join(this.root, record.folder) : join(this.root, record.kind, "accounts", record.id)
  }

  add(kind: AgentKind, label?: string, folder?: string): AccountRecord {
    const records = this.read()
    const record: AccountRecord = { kind, id: randomBytes(4).toString("hex"), label: label?.trim() || undefined, ...(folder ? { folder } : {}) }
    mkdirSync(this.home(record)!, { recursive: true, mode: 0o700 })
    this.save([...records, record])
    return record
  }

  rename(provider: string, label: string | undefined) {
    const parsed = parseProvider(provider)
    const records = this.read()
    const record = records.find((record) => record.kind === parsed?.kind && record.id === parsed?.id)
    if (!record) return
    record.label = label?.trim() || undefined
    this.save(records)
  }

  /** Forgets an added account, and everything kept in its folder. */
  remove(provider: string): AccountRecord | undefined {
    const parsed = parseProvider(provider)
    const records = this.read()
    const index = records.findIndex((record) => record.kind === parsed?.kind && record.id === parsed?.id)
    if (index < 0) return undefined
    const [record] = records.splice(index, 1)
    this.save(records)
    rmSync(this.home(record)!, { recursive: true, force: true })
    return record
  }

  private save(records: AccountRecord[]) {
    mkdirSync(this.root, { recursive: true })
    writeFileSync(this.file + ".new", JSON.stringify(accountFormat.write(records), null, 2))
    renameSync(this.file + ".new", this.file)
    this.records = records
  }
}

// Before 22 November 2026 a Linux box kept its agents' first sign-ins in
// the bridge's folder (`<agent>/home`). Goes 3 months after the release that carries this.
/** The agent's own folder, as the agent finds it (its variable, else its usual place). */
function ownFolder(kind: "claude" | "codex" | "grok"): string {
  const home = homedir()
  const set = { claude: process.env.CLAUDE_CONFIG_DIR, codex: process.env.CODEX_HOME, grok: process.env.GROK_HOME }[kind]?.trim()
  if (set) return set === "~" ? home : set.startsWith("~/") ? join(home, set.slice(2)) : set
  return join(home, `.${kind}`)
}

/** What the agent keeps there that's moved: its sign-in, and its conversations (each by its ID). */
const MOVED = {
  claude: { signIn: ".credentials.json", conversations: ["projects"] },
  codex: { signIn: "auth.json", conversations: ["sessions", "archived_sessions"] },
  grok: { signIn: "auth.json", conversations: ["sessions"] },
} as const

/** Each file of `from` moved into `to` where `to` has none by that name; `from` emptied of them. */
function moveInto(from: string, to: string) {
  let names: string[] = []
  try { names = readdirSync(from) } catch { return }
  mkdirSync(to, { recursive: true, mode: 0o700 })
  for (const name of names) {
    const source = join(from, name), target = join(to, name)
    if (statSync(source).isDirectory()) moveInto(source, target)
    else if (!existsSync(target)) renameSync(source, target)
  }
}

/**
 * A first account an earlier bridge kept in its folder (`<agent>/home`, on Linux), taken over by
 * the agent's own folder, once: its conversations moved there (so a session goes on with its
 * thread), and its sign-in too where the agent's own folder has none (moved, never copied: one
 * sign-in, which the agent renews). Where you're signed in there already, that one is the first
 * account, and the earlier one becomes an account you added ("Earlier sign-in"), its folder moved
 * whole. What's left of `<agent>/home` goes.
 */
export function adoptOwnFolders(root: string, accounts = new Accounts(root)) {
  for (const kind of ["claude", "codex", "grok"] as const) {
    const old = join(root, kind, "home")
    if (!existsSync(old)) continue
    try {
      adoptOne(kind, old, accounts)
    } catch (error) {
      // (Another disk, a file in the way: left as it is, and tried again at the next start.)
      console.error(`Couldn't move ${kind}'s earlier sign-in to its own folder:`, error instanceof Error ? error.message : error)
    }
  }
}

function adoptOne(kind: "claude" | "codex" | "grok", old: string, accounts: Accounts) {
  const own = ownFolder(kind)
  const { signIn, conversations } = MOVED[kind]
  for (const folder of conversations) moveInto(join(old, folder), join(own, folder))
  if (existsSync(join(old, signIn))) {
    if (!existsSync(join(own, signIn))) {
      mkdirSync(own, { recursive: true, mode: 0o700 })
      renameSync(join(old, signIn), join(own, signIn))
      // Claude Code's record of who's signed in, which it keeps in your home without CLAUDE_CONFIG_DIR.
      const record = process.env.CLAUDE_CONFIG_DIR?.trim() ? join(own, ".claude.json") : join(homedir(), ".claude.json")
      if (kind === "claude" && existsSync(join(old, ".claude.json")) && !existsSync(record)) renameSync(join(old, ".claude.json"), record)
    } else {
      const record = accounts.add(kind, "Earlier sign-in")
      const folder = accounts.home(record)!
      rmSync(folder, { recursive: true, force: true })
      mkdirSync(dirname(folder), { recursive: true })
      renameSync(old, folder)
      return
    }
  }
  rmSync(old, { recursive: true, force: true })
}
