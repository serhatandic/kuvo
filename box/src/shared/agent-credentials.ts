// Read the credential files left by agents. These helpers never refresh or mutate a sign-in.
import { readFileSync } from "node:fs"
import { join } from "node:path"
import type { AgentAccount } from "../agents/accounts"
import { homeFolder } from "./account"

function read<T>(file: string): T | undefined {
  try { return JSON.parse(readFileSync(file, "utf8")) as T } catch { return undefined }
}

type CodexAuth = {
  auth_mode?: unknown
  OPENAI_API_KEY?: unknown
  tokens?: { access_token?: unknown; account_id?: unknown; refresh_token?: unknown }
}

export function codexAuthFile(home: string): CodexAuth | undefined {
  return read<CodexAuth>(join(home, "auth.json"))
}

export function codexOAuth(saved?: CodexAuth) {
  if (saved?.auth_mode === "apikey" || (!saved?.auth_mode && typeof saved?.OPENAI_API_KEY === "string" && saved.OPENAI_API_KEY)) return undefined
  const tokens = saved?.tokens
  if (typeof tokens?.access_token !== "string" || !tokens.access_token) return undefined
  return {
    token: tokens.access_token,
    account: typeof tokens.account_id === "string" ? tokens.account_id : undefined,
    refresh: typeof tokens.refresh_token === "string" ? tokens.refresh_token : undefined,
  }
}

type GrokCredential = { key: string; id_token?: string; expires_at?: string; expiresAt?: string; expires?: string }
export function grokCredential(home: string): GrokCredential | undefined {
  const entries = read<Record<string, GrokCredential>>(join(home, "auth.json"))
  return entries && Object.values(entries).find(each => typeof each?.key === "string" && each.key.trim())
}

export function grokHome(instance: AgentAccount): string {
  if (instance.home) return instance.home
  const home = homeFolder()
  const set = process.env.GROK_HOME?.trim()
  return set === "~" ? home : set?.startsWith("~/") ? join(home, set.slice(2)) : set || join(home, ".grok")
}

export function claudeToken(file: string): string | undefined {
  const token = read<{ token?: unknown }>(file)?.token
  return typeof token === "string" && token ? token : undefined
}
