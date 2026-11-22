// Files a turn changed that no tool call named (a command's output), looked for as the turn ends:
// in a git repository, what git status lists (so what git ignores is left out); elsewhere, a walk
// of the folder for files changed since the turn began, which stops at a limit. The same on macOS
// and Linux: plain directory reads and git, no watch.

import { execFile } from "node:child_process"
import { promises as fs } from "node:fs"
import { dirname, join, sep } from "node:path"
import { ownArgsLater } from "../../shared/git"

/** Folders whose files are never what an agent made for you (tools' own, caches). */
export const skipped = new Set([".git", "node_modules", ".venv", "venv", "__pycache__", ".cache", ".pytest_cache", ".mypy_cache", ".next", ".turbo", ".gradle", "target", "DerivedData"])

/**
 * A project's scratch folders, whatever their case: what a command leaves there (a render to check
 * its work) isn't what it made for you. (A bot's own folder keeps what it puts in its tmp.)
 */
const scratch = new Set(["tmp", "temp", ".tmp"])

/** Whether `path` is in a scratch folder of `project`'s. */
export function inScratch(path: string, project: string): boolean {
  if (!path.startsWith(project + sep)) return false
  return path.slice(project.length + 1).split(sep).slice(0, -1).some((part) => scratch.has(part.toLowerCase()))
}

/**
 * How much earlier than the turn a file's time may say it changed and still count: Linux stamps
 * files from a coarse clock (a few milliseconds behind the time read just before the write), and
 * some file systems keep whole seconds.
 */
const clockSlack = 2000

/** Most entries a walk looks at, and most files it gives back. */
const walkLimit = 20_000
const fileLimit = 200

/**
 * Kuvo Code's own folders on this computer (the box's, with the files sent with messages, its run
 * state and records): never what an agent made, though a session working in the home folder walks
 * over them. On a machine they're `~/.kuvo-code`; on a Mac, the app's support folder, which holds
 * the bridge's and the box's, and what the app names (`KUVO_OWN_FOLDERS`, a JSON list: a Mac's
 * caches, logs and preferences, folders or files).
 */
function ownFolders(): string[] {
  const agents = process.env.KUVO_AGENTS_HOME
  let named: string[] = []
  try {
    const list = JSON.parse(process.env.KUVO_OWN_FOLDERS ?? "[]")
    if (Array.isArray(list)) named = list.filter((each): each is string => typeof each === "string")
  } catch {}
  return [process.env.KUVO_HOME, agents ? dirname(agents) : undefined, ...named].filter((folder): folder is string => !!folder)
}

/** The git repository `folder` is in, or none. */
async function repositoryOf(folder: string): Promise<string | undefined> {
  for (let at = folder; ; at = dirname(at)) {
    try { await fs.access(join(at, ".git")); return at } catch {}
    if (dirname(at) === at) return undefined
  }
}

/**
 * Files under `folder` changed since `since` (ms), at most a couple of hundred.
 * Directory reads, stats and git run asynchronously so serving the record can continue.
 */
export async function changedSince(
  folder: string, since: number, keep: (relative: string) => boolean = () => true,
  descend: (relative: string) => boolean = () => true,
): Promise<string[]> {
  const top = await repositoryOf(folder)
  const own = ownFolders().filter((inside) => inside.startsWith(folder + sep))
  const allowed = (path: string) => {
    const relative = path.slice(folder.length + 1)
    return !relative.split(sep).some((part) => skipped.has(part) || scratch.has(part.toLowerCase()))
      && !own.some((inside) => path === inside || path.startsWith(inside + sep))
  }
  const found: string[] = []
  const check = async (paths: string[]) => {
    const matching = await Promise.all(paths.map(async path => {
      try {
        const stat = await fs.stat(path)
        if (stat.isFile() && stat.mtimeMs >= since - clockSlack) return path
      } catch {}
      return undefined
    }))
    for (const path of matching) if (path && found.length < fileLimit) found.push(path)
  }
  const paths = top && await fromGit(top, folder)
  if (paths) {
    const candidates = paths.filter(path => allowed(path) && keep(path.slice(folder.length + 1)))
    for (let i = 0; i < candidates.length && found.length < fileLimit; i += 32) await check(candidates.slice(i, i + 32))
    return found
  }
  let looked = 0
  const visit = async (at: string): Promise<void> => {
    const entries = await fs.readdir(at, { withFileTypes: true }).catch(() => [])
    const pending: string[] = []
    const folders: string[] = []
    for (const entry of entries) {
      if (++looked > walkLimit || found.length >= fileLimit) break
      const path = join(at, entry.name)
      if (!allowed(path)) continue
      const relative = path.slice(folder.length + 1)
      if (entry.isDirectory()) {
        if (descend(relative)) folders.push(path)
      } else if ((entry.isFile() || entry.isSymbolicLink()) && keep(relative)) {
        pending.push(path)
        if (pending.length === 32) await check(pending.splice(0))
      }
    }
    if (pending.length && found.length < fileLimit) await check(pending)
    for (const child of folders) {
      if (looked >= walkLimit || found.length >= fileLimit) break
      await visit(child)
    }
  }
  await visit(folder)
  if (looked >= walkLimit) console.error(`artifacts: stopped looking through ${folder} after ${walkLimit} entries`)
  return found
}

/** What git status lists under `folder`: changed and new files, each by its whole path. */
async function fromGit(top: string, folder: string): Promise<string[] | undefined> {
  const args = await ownArgsLater(["-C", folder, "status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."])
  if (!args) return undefined
  const out = await new Promise<string | undefined>(done => execFile("git", args, {
    timeout: 15_000,
    maxBuffer: 16 * 1024 * 1024,
    encoding: "utf8",
    env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_TERMINAL_PROMPT: "0" },
  }, (error, stdout) => done(error ? undefined : stdout)))
  if (out === undefined) return undefined
  const paths: string[] = []
  const entries = out.split("\0")
  for (let i = 0; i < entries.length; i++) {
    const entry = entries[i]
    if (entry.length < 4) continue
    // A rename names where it went first, then where it came from (skipped).
    if (entry[0] === "R" || entry[0] === "C") i++
    if (entry[0] === "D" || entry[1] === "D") continue
    const path = join(top, entry.slice(3))
    if (path.startsWith(folder + sep)) paths.push(path)
  }
  return paths
}
