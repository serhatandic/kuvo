// The box's Library (PROTOCOL.md, "Library"): what was handed over to you on this computer, not
// everything its agents touched. A file an agent showed you as a result (`show_file`: a picture, a
// PDF, a page it built), a picture it generated, and a file you attached to a message. Never a
// project's code edits, a bot's notes or memory, logs, temporary files or the screenshots it took
// to check its own work: those stay in the artifact record (`kuvo/artifacts`) and the changes.
//
// Kept in one folder of the box's own (`library/` beside `artifacts/`):
//   log.jsonl   one line per item as it is now, read back at start (the last line of an id wins),
//               written again whole once it has grown well past what it holds

import { createHash } from "node:crypto"
import { appendFileSync, closeSync, mkdirSync, openSync, readFileSync, readSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { basename, extname, join, sep } from "node:path"
import { typeOf } from "../artifacts/kinds"
import { isSecret } from "../artifacts/policy"
import { libraryIndexFormat } from "../../shared/library-format"
import { isObject } from "../../shared/json"

/** The Library's tabs: what a thing is, by its name. */
export type LibraryKind = "image" | "document" | "page" | "other"
/** How it came: an agent showed it, generated it, or you attached it. */
export type How = "shown" | "generated" | "attached"

export interface Item {
  id: string
  path: string
  /** Its file's name; a page's own title when it has one. */
  name: string
  kind: LibraryKind
  type: string
  how: How
  /** The bot whose conversation it was handed over in; none for one of your own sessions. */
  bot?: string
  /** The conversation it was handed over in (a subagent's: its parent's). */
  session?: string
  /** The group chat, for one handed over in a chat. */
  group?: string
  /** The message it was handed over in, and the tool call's part, for "Show in conversation". */
  message?: string
  part?: string
  /** When it was handed over (the latest time, when it was again). */
  at: string
  /** When you removed it from the Library: it stays on disk, and in the log, hidden. */
  hidden?: string
}

/** A hand-over, as what saw it tells the Library. */
export interface Handover {
  path: string
  how: How
  bot?: string
  session?: string
  group?: string
  message?: string
  part?: string
  at?: Date
}

export interface Filter {
  /** `all`, `you` (your own sessions) or a bot's id. */
  source?: string
  kind?: LibraryKind
  /** Words in its name, its bot's name or this computer's. */
  query?: string
  /** Hidden ones only (to put one back). */
  hidden?: boolean
}

const images = new Set(["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "svg", "bmp", "tiff", "tif", "avif"])
const documents = new Set(["pdf", "doc", "docx", "rtf", "pages", "odt", "md", "markdown", "txt"])
const pages = new Set(["html", "htm"])
/** What's never handed over, whatever showed it: logs and an editor's or a download's file on its way. */
const never = new Set(["log", "tmp", "swp", "crdownload", "part"])

/** The tab a file goes in, by its name. */
export function libraryKind(path: string): LibraryKind {
  const ext = extname(path).slice(1).toLowerCase()
  if (images.has(ext)) return "image"
  if (pages.has(ext)) return "page"
  if (documents.has(ext)) return "document"
  return "other"
}

/**
 * Whether a file shown or made counts as handed over: not a likely secret, a log or a file on its
 * way, and not a bot's own notes (its memory, its job, a chat's recap) or a skill. `bots`: the box's
 * bots' folder.
 */
export function handedOver(path: string, bots?: string): boolean {
  const name = basename(path)
  if (!name || isSecret(path) || never.has(extname(name).slice(1).toLowerCase()) || name.endsWith("~")) return false
  if (bots && path.startsWith(bots + sep)) {
    const inside = path.slice(bots.length + 1).split(sep)
    if (inside.length === 2 && (inside[1] === "MEMORY.md" || inside[1] === "job.json")) return false
    if (inside[1] === "groups") return false
  }
  const parts = path.split(sep)
  const skills = parts.lastIndexOf("skills")
  if (skills > 0 && [".opencode", ".claude", "opencode"].includes(parts[skills - 1])) return false
  return true
}

/** A page's own title, from the start of its file, when it has one. */
export function titleOf(path: string): string | undefined {
  let text = ""
  try {
    const file = openSync(path, "r")
    try {
      const bytes = Buffer.alloc(64 * 1024)
      text = bytes.subarray(0, readSync(file, bytes, 0, bytes.length, 0)).toString("utf8")
    } finally {
      closeSync(file)
    }
  } catch {
    return undefined
  }
  const found = /<title[^>]*>([\s\S]*?)<\/title>/i.exec(text)?.[1]
  const title = found
    ?.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'")
    .replace(/\s+/g, " ").trim()
  return title ? title.slice(0, 120) : undefined
}

const stamp = (date: Date) => date.toISOString()

export class LibraryStore {
  private items = new Map<string, Item>()
  private listeners = new Set<(item: Item) => void>()
  private lines = 0

  constructor(
    private readonly root: string,
    private readonly options: { bots?: string; now?: () => Date } = {},
  ) {
    mkdirSync(root, { recursive: true })
    this.load()
  }

  static idOf(path: string) {
    return "lib_" + createHash("sha256").update(path).digest("hex").slice(0, 24)
  }

  private now() {
    return this.options.now?.() ?? new Date()
  }

  /** Told of each item as it changes. */
  onChange(listener: (item: Item) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  get(id: string): Item | undefined {
    return this.items.get(id)
  }

  /**
   * A file handed over: one item a file, the latest hand-over its time, where and how (a page's
   * title read again, a picture made again). Handed over again after you removed it, it's back.
   * Nothing for what doesn't count (`handedOver`).
   */
  hand(handover: Handover): Item | undefined {
    if (!handover.path.startsWith("/") || !handedOver(handover.path, this.options.bots)) return undefined
    const id = LibraryStore.idOf(handover.path)
    const kind = libraryKind(handover.path)
    const at = stamp(handover.at ?? this.now())
    const known = this.items.get(id)
    // An older hand-over (a backfill after a newer one): only what the newer one didn't say.
    if (known && known.at > at) return undefined
    const name = (kind === "page" ? titleOf(handover.path) : undefined) ?? basename(handover.path)
    const item: Item = {
      id, path: handover.path, name, kind, type: typeOf(handover.path), how: handover.how,
      ...(handover.bot ? { bot: handover.bot } : {}),
      ...(handover.session ? { session: handover.session } : {}),
      ...(handover.group ? { group: handover.group } : {}),
      ...(handover.message ? { message: handover.message } : {}),
      ...(handover.part ? { part: handover.part } : {}),
      at,
    }
    this.items.set(id, item)
    this.write(item)
    return this.told(item)
  }

  /** Removed from the Library (`hidden`), or put back: its file stays where it is either way. */
  hide(id: string, hidden = true): Item | undefined {
    const item = this.items.get(id)
    if (!item || Boolean(item.hidden) === hidden) return item
    if (hidden) item.hidden = stamp(this.now())
    else delete item.hidden
    this.write(item)
    return this.told(item)
  }

  /** What's in the Library for `filter`, newest first; `names` tells a bot's name for a search. */
  list(filter: Filter = {}, names: (bot: string) => string | undefined = () => undefined, computer = ""): Item[] {
    const words = (filter.query ?? "").toLowerCase().split(/\s+/).filter(Boolean)
    const source = filter.source && filter.source !== "all" ? filter.source : undefined
    return [...this.items.values()]
      .filter((item) => Boolean(item.hidden) === Boolean(filter.hidden))
      .filter((item) => !filter.kind || item.kind === filter.kind)
      .filter((item) => !source || (source === "you" ? !item.bot : item.bot === source))
      .filter((item) => {
        if (!words.length) return true
        const said = [item.name, item.bot ? names(item.bot) ?? "" : "you", computer].join("\n").toLowerCase()
        return words.every((word) => said.includes(word))
      })
      .sort((a, b) => (a.at === b.at ? (a.id < b.id ? 1 : -1) : a.at < b.at ? 1 : -1))
  }

  /** The bots with something in the Library, and whether any of your own sessions has. */
  sources(): { bots: string[]; you: boolean } {
    const bots = new Set<string>()
    let you = false
    for (const item of this.items.values()) {
      if (item.hidden) continue
      if (item.bot) bots.add(item.bot)
      else you = true
    }
    return { bots: [...bots].sort(), you }
  }

  private told(item: Item): Item {
    for (const listener of this.listeners) listener(item)
    return item
  }

  private write(item: Item) {
    appendFileSync(join(this.root, "log.jsonl"), JSON.stringify(libraryIndexFormat.write(item)) + "\n")
    this.lines++
    if (this.lines > 1000 && this.lines > this.items.size * 2) this.compact()
  }

  /** Written again, a line an item: aside, then moved in. */
  private compact() {
    const file = join(this.root, "log.jsonl")
    try {
      writeFileSync(file + ".new", [...this.items.values()].map((item) => JSON.stringify(libraryIndexFormat.write(item)) + "\n").join(""))
      renameSync(file + ".new", file)
      this.lines = this.items.size
    } catch (error: any) {
      rmSync(file + ".new", { force: true })
      console.error("library: the log couldn't be compacted:", error?.message ?? error)
    }
  }

  private load() {
    let text = ""
    try {
      text = readFileSync(join(this.root, "log.jsonl"), "utf8")
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error
    }
    for (const line of text.split("\n")) {
      if (!line) continue
      this.lines++
      let saved: unknown
      try {
        saved = JSON.parse(line)
      } catch {
        // A line cut short as the box stopped: the rest still counts.
        continue
      }
      if (!isObject(saved)) continue
      const item = libraryIndexFormat.read(saved) as unknown as Item
      if (typeof item.id !== "string" || typeof item.path !== "string" || typeof item.at !== "string") continue
      this.items.set(item.id, item)
    }
  }
}
