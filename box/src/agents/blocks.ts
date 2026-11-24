// Settings › Permissions' Blocks as the engine's settings files have them, read here when the engine
// can't be asked: it's parked while only coding agents work, or starting again. The same files the
// engine reads (its start names them, kuvo-launch): its settings folder (XDG_CONFIG_HOME), this
// box's own settings on a machine (OPENCODE_CONFIG), and the project's.

import { existsSync, readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { holdsBack } from "./rules"
import type { PermissionRule } from "./wire"

/** Blocks as rules: what nothing allows ("shell:git push *"). Other settings are left out. */
export function blocksOf(policies: unknown): PermissionRule[] {
  const rules: PermissionRule[] = []
  for (const policy of (Array.isArray(policies) ? policies : []) as { action?: string; resource?: string; effect?: string }[]) {
    if (policy?.action !== "tool.use" || typeof policy.resource !== "string") continue
    if (typeof policy.effect !== "string") continue
    const at = policy.resource.indexOf(":")
    // (One of a kind a newer engine has that this bridge doesn't know: nothing allows it either.)
    rules.push({ action: at < 0 ? policy.resource : policy.resource.slice(0, at), resource: at < 0 ? "*" : policy.resource.slice(at + 1) || "*", effect: holdsBack(policy.effect) ? "deny" : policy.effect })
  }
  return rules
}

/**
 * The files the engine reads its settings from for a folder, in its order: its settings folder's
 * (`engineConfig`, its XDG_CONFIG_HOME), the box's own settings on a machine (`box`, not on a Mac's
 * box, whose settings are in its settings folder), then the project's, from its repository's top
 * down to the folder (only the folder's own outside a repository).
 */
export function settingsLayers(directory: string, engineConfig: string | undefined, box?: string): string[] {
  const files: string[] = []
  if (engineConfig) for (const name of ["config.json", "opencode.json", "opencode.jsonc"]) files.push(join(engineConfig, "opencode", name))
  if (box) {
    const own = join(box, "box", "opencode.json")
    files.push(existsSync(own) ? own : join(box, "keys", "settings.json"))
  }
  const folders: string[] = []
  for (let folder = directory; ; folder = dirname(folder)) {
    folders.push(folder)
    if (existsSync(join(folder, ".git")) || dirname(folder) === folder) break
  }
  const top = folders[folders.length - 1]
  for (const folder of (existsSync(join(top, ".git")) ? folders : folders.slice(0, 1)).reverse()) {
    for (const name of ["opencode.json", "opencode.jsonc", ".opencode/opencode.json", ".opencode/opencode.jsonc"]) files.push(join(folder, name))
  }
  return files
}

/** The Blocks in these files (those there): undefined when one can't be read, so nothing is taken as allowed. */
export function blocksInFiles(files: string[]): PermissionRule[] | undefined {
  const rules: PermissionRule[] = []
  for (const file of files) {
    let text: string
    try {
      text = readFileSync(file, "utf8")
    } catch (error) {
      if ((error as NodeJS.ErrnoException)?.code === "ENOENT" || (error as NodeJS.ErrnoException)?.code === "ENOTDIR") continue
      return undefined
    }
    let settings: unknown
    try {
      settings = JSON.parse(withoutComments(text.replace(/^﻿/, "")))
    } catch {
      return undefined
    }
    if (!settings || typeof settings !== "object" || Array.isArray(settings)) return undefined
    rules.push(...blocksOf((settings as { policies?: unknown }).policies))
  }
  return rules
}

/** JSON with comments and trailing commas (as the engine reads `.jsonc`, and `.json` too) as plain JSON. */
function withoutComments(text: string): string {
  const out: string[] = []
  let quoted = false
  // Where the last character outside strings and comments went, to drop a comma before a close.
  let last = -1
  for (let at = 0; at < text.length; at++) {
    const char = text[at]
    if (quoted) {
      out.push(char)
      if (char === "\\") out.push(text[++at] ?? "")
      else if (char === '"') quoted = false
    } else if (char === "/" && text[at + 1] === "/") {
      while (at + 1 < text.length && text[at + 1] !== "\n") at++
    } else if (char === "/" && text[at + 1] === "*") {
      const end = text.indexOf("*/", at + 2)
      if (end < 0) throw new SyntaxError("A comment has no end.")
      at = end + 1
      out.push(" ")
    } else {
      if ((char === "]" || char === "}") && last >= 0 && out[last] === ",") out[last] = " "
      if (char === '"') quoted = true
      if (!/\s/.test(char)) last = out.length
      out.push(char)
    }
  }
  return out.join("")
}
