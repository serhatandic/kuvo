// The reader uses the same Codex home as the bridge. macOS's Codex keychain item is scoped to
// that home, including for an added account. Never search for other accounts or renew tokens here.
import { createHash } from "node:crypto"
import { readFileSync, realpathSync } from "node:fs"
import { join } from "node:path"
import { codexAuthFile, codexOAuth } from "../shared/agent-credentials"
import { run, which } from "../agents/proc"

export type KeychainRead = (account: string) => Promise<string | undefined>
const readMacKeychain: KeychainRead = async account => {
  const answer = await run("/usr/bin/security", ["find-generic-password", "-s", "Codex Auth", "-a", account, "-w"], { timeout: 3_000 })
  return answer.code === 0 ? answer.stdout : undefined
}

// Codex's Linux store caches its persistent Secret Service credential in the session keyring.
const readLinuxKeyring: KeychainRead = async account => {
  const keyctl = which("keyctl")
  if (keyctl) {
    const found = await run(keyctl, ["search", "@s", "user", `keyring-rs:${account}@Codex Auth`], { timeout: 1500 })
    const id = found.stdout.trim()
    if (found.code === 0 && /^\d+$/.test(id)) {
      const answer = await run(keyctl, ["pipe", id], { timeout: 1500 })
      if (answer.code === 0) return answer.stdout
    }
  }
  const lookup = which("secret-tool")
  if (!lookup) return undefined
  const answer = await run(lookup, ["lookup", "application", "rust-keyring", "service", "Codex Auth", "username", account, "target", "default"], { timeout: 3000 })
  return answer.code === 0 ? answer.stdout : undefined
}

export function codexKeychainAccount(home: string): string {
  let canonical = home
  try { canonical = realpathSync(home) } catch {}
  return "cli|" + createHash("sha256").update(canonical).digest("hex").slice(0, 16)
}

export class CodexCredentials {
  private keychain?: ReturnType<typeof codexAuthFile>
  private pending?: Promise<void>
  private next = 0

  constructor(private readonly home: string, private readonly keychainRead = process.platform === "darwin" ? readMacKeychain : process.platform === "linux" ? readLinuxKeyring : undefined) {}

  private mode(): "file" | "keyring" | "auto" {
    try {
      const mode = /^\s*cli_auth_credentials_store\s*=\s*["'](file|keyring|auto)["']/m.exec(readFileSync(join(this.home, "config.toml"), "utf8"))?.[1]
      return mode === "keyring" || mode === "auto" ? mode : "file"
    } catch { return "file" }
  }

  current() {
    const mode = this.mode()
    if (mode !== "file" && this.keychain) return codexOAuth(this.keychain)
    return mode === "keyring" ? undefined : codexOAuth(codexAuthFile(this.home))
  }

  async forReset() {
    await this.prepare(true)
    // The platform commands can return without a credential when a store is locked or
    // inaccessible. A read may use the fallback file then, but spending needs a confirmed login.
    return this.mode() === "file" ? codexOAuth(codexAuthFile(this.home)) : codexOAuth(this.keychain)
  }

  async prepare(fresh = false) {
    while (this.pending) {
      await this.pending
      if (!fresh) return
    }
    if (!this.keychainRead || this.mode() === "file" || (!fresh && Date.now() < this.next)) return
    const reading = this.readKeychain(this.keychainRead)
    this.pending = reading
    try { await reading } finally { if (this.pending === reading) this.pending = undefined }
  }

  private async readKeychain(read: KeychainRead) {
    try {
      const value = await read(codexKeychainAccount(this.home))
      const saved = value ? JSON.parse(value) : undefined
      this.keychain = saved && typeof saved === "object" && !Array.isArray(saved) ? saved : undefined
    } catch {
      this.keychain = undefined
    }
    this.next = Date.now() + (this.keychain ? 60_000 : 10 * 60_000)
  }
}
