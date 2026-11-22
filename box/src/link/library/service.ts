// This computer's Library as the box's services keep it beside the artifact record: told by the
// same recorder what was handed over, filled once from what came before it, and read by the link
// (`kuvo/library`), which gathers it with your other computers' parts.

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs"
import { isAbsolute, join, relative, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"
import { readSavedSessions } from "../../agents/store"
import type { ArtifactStore } from "../artifacts/store"
import { copyLimit } from "../artifacts/store"
import { attachedPaths, toolName } from "../artifacts/recorder"
import { libraryRoutes, type LibraryHost } from "./routes"
import { LibraryStore, type Handover, type Item } from "./store"
import { Thumbnails } from "./thumbnails"

/** What the first start fills the Library from, once: bumped, it's filled again (what's there stays). */
const BACKFILL = 1

export interface LibraryOptions {
  /** The box's bots' folder. */
  bots: string
  /** The coding agents' saved conversations. */
  sessions: string
  /** The artifact record: its copies of files since gone, and what it saw before the Library. */
  artifacts?: ArtifactStore
  /** This computer's name, for a search. */
  computer?: () => string
  platform?: NodeJS.Platform
}

export function libraryService(root: string, options: LibraryOptions) {
  const store = new LibraryStore(root, { bots: options.bots })
  const thumbnails = new Thumbnails(join(root, "thumbnails"), options.platform)
  const described = new Map<string, { name: string | undefined; look: Record<string, unknown> | undefined; at: number }>()
  /** A bot's name and look as its folder describes it (read again after a while: it can be renamed). */
  const bot = (id: string) => {
    const known = described.get(id)
    if (known && Date.now() - known.at < 30_000) return known
    let name: string | undefined
    let look: Record<string, unknown> | undefined
    try {
      const said = JSON.parse(readFileSync(join(options.bots, id, "bot.json"), "utf8"))
      if (typeof said?.name === "string") name = said.name
      if (said?.look && typeof said.look === "object" && !Array.isArray(said.look)) {
        look = Object.fromEntries(["color", "shape", "face"].filter((key) => typeof said.look[key] === "string").map((key) => [key, said.look[key]]))
      }
    } catch {}
    const read = { name, look, at: Date.now() }
    described.set(id, read)
    return read
  }
  /** The artifact record's newest copy of its file, once the file itself is gone. */
  const copy = (item: Item): Buffer | null => {
    const artifact = options.artifacts?.at(item.path)
    const latest = artifact && [...artifact.versions].reverse().find((version) => version.kept)
    return (artifact && latest && options.artifacts!.bytes(artifact.id, latest.n)) || null
  }
  const host: LibraryHost = {
    bytes(item) {
      try {
        const stat = statSync(item.path)
        if (stat.isFile() && stat.size <= copyLimit) return readFileSync(item.path)
        if (stat.isFile()) return null
      } catch {}
      return copy(item)
    },
    there(item) {
      try {
        if (statSync(item.path).isFile()) return true
      } catch {}
      const artifact = options.artifacts?.at(item.path)
      return Boolean(artifact?.versions.some((version) => version.kept))
    },
    thumbnailable: (item) => thumbnails.can(item),
    thumbnail: (item) => thumbnails.of(item, () => copy(item)),
    botName: (id) => bot(id).name,
    botLook: (id) => bot(id).look,
    computer: () => options.computer?.() ?? "",
  }
  return {
    store,
    /** What the recorder tells it: a file handed over. */
    handed: (handover: Handover) => void store.hand(handover),
    /** Filled once from the coding agents' saved conversations and the artifact record. */
    backfill: () => backfill(store, root, options),
    handle(request: Request): Promise<Response> {
      const url = new URL(request.url)
      const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent)
      // `/api/kuvo/library…`, or `/api/kuvo/link/library…` as the link passes it on.
      const at = parts.indexOf("library")
      return libraryRoutes(store, host, request.method, parts.slice(at + 1), url.searchParams, () => request.json().catch(() => ({})), request.signal)
    },
  }
}

/**
 * What was handed over before the Library kept it, once: the coding agents' saved conversations
 * (a file `show_file` showed, a picture `generate_image` made, your messages' files, each at its
 * message) and, for the engine's, what the artifact record saw shown or generated (at the turn).
 * Nothing it already has newer.
 */
export function backfill(store: LibraryStore, root: string, options: LibraryOptions) {
  const marker = join(root, "backfill.json")
  try {
    if (JSON.parse(readFileSync(marker, "utf8"))?.done >= BACKFILL) return
  } catch {}
  const found: Handover[] = []
  const botOf = (directory: string | undefined, metadata: any): string | undefined => {
    if (typeof metadata?.kuvo?.seat?.bot === "string") return metadata.kuvo.seat.bot
    if (typeof metadata?.kuvo?.bot === "string") return metadata.kuvo.bot
    if (!directory || !(directory + sep).startsWith(options.bots + sep)) return undefined
    const first = relative(options.bots, directory).split(sep)[0]
    return first.startsWith("bot_") ? first : undefined
  }
  try {
    const saved = [...readSavedSessions(options.sessions)]
    const parents = new Map(saved.map(({ session }) => [session.id, (session as any).parentID as string | undefined]))
    const rootOf = (id: string) => {
      let at = id
      for (let depth = 0; depth < 8 && parents.get(at); depth++) at = parents.get(at)!
      return at
    }
    for (const { session, messages } of saved) {
      const directory = session.location?.directory
      const metadata = (session as any).metadata
      const seat = metadata?.kuvo?.seat
      const top = rootOf(session.id)
      const own = top === session.id
      const where = {
        ...(botOf(directory, metadata) ? { bot: botOf(directory, metadata) } : {}),
        ...(seat && typeof seat.group === "string" ? { group: seat.group } : { session: top }),
      }
      const absolute = (path: string) => (isAbsolute(path) ? path : resolve(directory ?? "/", path))
      for (const message of messages as any[]) {
        const at = new Date(message.time?.completed ?? message.time?.created ?? 0)
        if (message.type === "user" && own) {
          for (const path of attachedPaths({ files: message.files, text: message.text })) found.push({ path, how: "attached", ...where, message: message.id, at })
          continue
        }
        if (message.type !== "assistant") continue
        for (const part of message.content ?? []) {
          if (part?.type !== "tool") continue
          const name = toolName(String(part.name ?? ""))
          const input = part.state?.input
          const anchor = own ? { message: message.id, part: part.id } : {}
          if ((name === "show_file" || String(part.id).startsWith(`cited-${message.id}-`)) && typeof input?.path === "string") {
            found.push({ path: absolute(input.path), how: "shown", ...where, ...anchor, at })
          }
          if (name === "generate_image") {
            for (const output of Array.isArray(part.state?.content) ? part.state.content : []) {
              if (output?.type !== "file" || typeof output.uri !== "string" || !output.uri.startsWith("file:")) continue
              try {
                found.push({ path: fileURLToPath(output.uri), how: "generated", ...where, ...anchor, at })
              } catch {}
            }
          }
        }
      }
    }
  } catch (error: any) {
    console.error("library: the coding agents' conversations couldn't be read:", error?.message ?? error)
  }
  // The engine's: what the record saw `show_file` show or `generate_image` make, at its turn.
  for (const { artifact, version } of options.artifacts?.timeline({}, { limit: 100_000 }).entries ?? []) {
    const how = version.via.includes("show_file") ? "shown" : version.via.includes("generate_image") ? "generated" : undefined
    if (!how) continue
    const bot = version.by.bot ?? artifact.where.bots[0]
    const session = version.turn?.session ?? artifact.where.sessions[0]
    const group = version.group?.id
    found.push({
      path: artifact.path, how,
      ...(bot ? { bot } : {}),
      ...(group ? { group } : session ? { session } : {}),
      ...(version.turn?.message ? { message: version.turn.message } : {}),
      at: new Date(version.at),
    })
  }
  // Oldest first, so each file keeps its latest hand-over; one gone since isn't brought in.
  found.sort((a, b) => (a.at?.getTime() ?? 0) - (b.at?.getTime() ?? 0))
  for (const handover of found) if (existsSync(handover.path) || options.artifacts?.at(handover.path)?.versions.some((version) => version.kept)) store.hand(handover)
  try {
    writeFileSync(marker, JSON.stringify({ done: BACKFILL }))
  } catch {}
}
