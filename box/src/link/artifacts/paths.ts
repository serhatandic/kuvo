// One file, one name in the record: a file reached through a link (a worktree opened at
// `/tmp/kuvo-ai`, `/tmp` itself on a Mac) is the same file as by its own path. The same on macOS and
// Linux: plain `realpath`.

import { promises as fs, realpathSync } from "node:fs"
import { basename, dirname, join, sep } from "node:path"

/**
 * Where `path` really is, through every link; for one that isn't there (not yet, or no more), its
 * nearest folder that is, with the rest as given. Undefined when nothing of it can be read.
 */
export function realOf(path: string): string | undefined {
  let rest = ""
  for (let at = path; ; ) {
    try {
      return realpathSync(at) + rest
    } catch {}
    const up = dirname(at)
    if (up === at) return undefined
    rest = sep + basename(at) + rest
    at = up
  }
}

/** Resolve a path from an older journal without holding up requests while the disk answers. */
export async function realOfLater(path: string): Promise<string> {
  let rest = ""
  for (let at = path; ; ) {
    try { return (await fs.realpath(at)) + rest } catch {}
    const up = dirname(at)
    if (up === at) return path
    rest = sep + basename(at) + rest
    at = up
  }
}

/**
 * `path` as the first of `folders` that holds it spells it (a conversation's folder, the bots'),
 * whichever way it was reached; one in none of them, as given.
 */
export function spelledIn(path: string, folders: (string | undefined)[]): string {
  const real = realOf(path)
  if (!real) return path
  for (const folder of folders) {
    if (!folder) continue
    const home = realOf(folder)
    if (!home) continue
    if (real === home) return folder
    if (real.startsWith(home + sep)) return join(folder, real.slice(home.length + 1))
  }
  return path
}
