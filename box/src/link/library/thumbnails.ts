// A Library item's thumbnail, made on the computer it's on and kept in the Library's own folder
// (`library/thumbnails/<id>-<file's stamp>.png`), made again once its file changes.
//
// - macOS: a picture made small with `sips`; a PDF, a document or a page by Quick Look (`qlmanage`),
//   as Finder shows it.
// - Linux, which has neither: a picture as it is (the client makes it small to draw it); none for
//   the rest, which a client draws as its kind.

import { execFile } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, statSync } from "node:fs"
import { basename, join } from "node:path"
import type { Item } from "./store"

/** The longest side of a thumbnail made here, in pixels: two of the grid's tiles at 2x. */
export const thumbnailSide = 480
/** The biggest picture sent as it is, where it can't be made small. */
const asItIs = 25 * 1024 * 1024
/** How long one may take to make. */
const making = 20_000

export class Thumbnails {
  private running = 0
  private waiting: (() => void)[] = []
  private made = new Map<string, Promise<Buffer | null>>()

  constructor(
    private readonly folder: string,
    private readonly platform: NodeJS.Platform = process.platform,
  ) {}

  /** Whether one can be made of it here. */
  can(item: Item): boolean {
    if (item.kind === "image") return true
    return this.platform === "darwin" && (item.kind === "document" || item.kind === "page")
  }

  /** Its thumbnail: a PNG made here, or a picture as it is; null when none can be made. */
  async of(item: Item, copy: () => Buffer | null): Promise<{ bytes: Buffer; type: string } | null> {
    let stat
    try {
      stat = statSync(item.path)
    } catch {
      // Gone: a picture's kept copy, as it is.
      const bytes = item.kind === "image" ? copy() : null
      return bytes ? { bytes, type: item.type } : null
    }
    if (!stat.isFile()) return null
    if (this.platform !== "darwin") {
      if (item.kind !== "image" || stat.size > asItIs) return null
      try {
        return { bytes: readFileSync(item.path), type: item.type }
      } catch {
        return null
      }
    }
    const key = `${item.id}-${createHash("sha256").update(`${stat.size}:${stat.mtimeMs}:${stat.ino}`).digest("hex").slice(0, 16)}`
    const file = join(this.folder, key + ".png")
    try {
      return { bytes: readFileSync(file), type: "image/png" }
    } catch {}
    let making = this.made.get(key)
    if (!making) {
      making = this.turn(() => this.make(item, file)).finally(() => this.made.delete(key))
      this.made.set(key, making)
    }
    const bytes = await making
    return bytes ? { bytes, type: "image/png" } : null
  }

  /** One made at a time, beside one more: Quick Look takes a moment, and a grid asks for many. */
  private async turn<T>(work: () => Promise<T>): Promise<T> {
    if (this.running >= 2) await new Promise<void>((go) => this.waiting.push(go))
    this.running++
    try {
      return await work()
    } finally {
      this.running--
      this.waiting.shift()?.()
    }
  }

  private async make(item: Item, file: string): Promise<Buffer | null> {
    mkdirSync(this.folder, { recursive: true })
    const scratch = mkdtempSync(join(this.folder, ".making-"))
    try {
      let made: string
      if (item.kind === "image") {
        made = join(scratch, "thumbnail.png")
        await run("/usr/bin/sips", ["-Z", String(thumbnailSide), "-s", "format", "png", item.path, "--out", made])
      } else {
        await run("/usr/bin/qlmanage", ["-t", "-s", String(thumbnailSide), "-o", scratch, item.path])
        made = join(scratch, basename(item.path) + ".png")
      }
      const bytes = readFileSync(made)
      // The one before it (its file changed since) goes.
      for (const name of readdirSync(this.folder)) if (name.startsWith(item.id + "-")) rmSync(join(this.folder, name), { force: true })
      renameSync(made, file)
      return bytes
    } catch {
      return null
    } finally {
      rmSync(scratch, { recursive: true, force: true })
    }
  }
}

/** A program run to its end (stopped, by its own process, past `making`). */
function run(program: string, args: string[]): Promise<void> {
  return new Promise((done, failed) => {
    execFile(program, args, { timeout: making, killSignal: "SIGKILL" }, (error) => (error ? failed(error) : done()))
  })
}
