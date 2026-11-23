// Before 22 November 2026, Claude Code on a Mac ran on Kuvo's own
// sign-in rather than yours: a `claude setup-token` token in `claude/tokens/<account>.json`, or an
// Anthropic Console sign-in in Kuvo's folder (`claude/console`). Taken over once, on each Mac box,
// as docs/architecture/agent-accounts.md says ("Existing sign-ins on update"):
// - a token keeps working until Claude Code's own sign-in is there (`~/.claude`, or the added
//   account's folder), then it's deleted;
// - the Console folder becomes an account you added ("Anthropic Console"), left where it is, since
//   Claude Code names its Keychain item after the folder's path.
// Goes 3 months after the release that carries this.

import { existsSync, readdirSync, rmSync } from "node:fs"
import { join } from "node:path"
import { claudeToken } from "../shared/agent-credentials"
import type { Accounts } from "./accounts"

/** Where an earlier version kept an account's `setup-token` token. */
export function legacyTokenFile(root: string, id: string): string {
  return join(root, "claude", "tokens", `${id}.json`)
}

/** That token, while it's there. */
export function legacyToken(root: string, id: string): string | undefined {
  return claudeToken(legacyTokenFile(root, id))
}

/** Claude Code's own sign-in is there now: the token goes (and its folder, once it's empty). */
export function forgetLegacyToken(root: string, id: string) {
  rmSync(legacyTokenFile(root, id), { force: true })
  const folder = join(root, "claude", "tokens")
  try {
    if (!readdirSync(folder).length) rmSync(folder, { recursive: true, force: true })
  } catch {}
}

/** The folder an earlier version kept an Anthropic Console sign-in in, relative to the bridge's. */
export const CONSOLE_FOLDER = join("claude", "console")

/**
 * An Anthropic Console sign-in an earlier version kept in its own folder: an account you added, in
 * that same folder (its Keychain item is named after it). One Claude Code never ran in (no
 * `.claude.json`, no conversations) goes.
 */
export function adoptConsoleFolder(root: string, accounts: Accounts) {
  const folder = join(root, CONSOLE_FOLDER)
  if (!existsSync(folder)) return
  if (accounts.all().some((record) => record.kind === "claude" && record.folder === CONSOLE_FOLDER)) return
  if (existsSync(join(folder, ".claude.json")) || existsSync(join(folder, "projects"))) accounts.add("claude", "Anthropic Console", CONSOLE_FOLDER)
  else rmSync(folder, { recursive: true, force: true })
}
