// The Library as a client reads this computer's part of it (`kuvo/library` in PROTOCOL.md, "Library"),
// which the box's link passes on to its services and gathers with its peers' parts: what was handed
// over here, filtered, followed as it changes; each item's file and thumbnail; removing one.

import type { Filter, Item, LibraryKind, LibraryStore } from "./store"

const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } })
const message = (status: number, text: string) => json({ message: text }, status)

const kinds = new Set<LibraryKind>(["image", "document", "page", "other"])
/** The most a list says at once (newest first): `more` says there were others. */
export const listLimit = 1000

/** What a route needs of this computer besides the store. */
export interface LibraryHost {
  /** Its file's bytes as they are now, or the artifact record's newest copy of them once it's gone. */
  bytes(item: Item): Buffer | null
  /** Whether it's there to open: on disk, or kept by the record. */
  there(item: Item): boolean
  /** Whether a thumbnail can be made of it here. */
  thumbnailable(item: Item): boolean
  /** Its thumbnail (a PNG, or the picture itself): null when none can be made. */
  thumbnail(item: Item): Promise<{ bytes: Buffer; type: string } | null>
  /** A bot's name, for a search, and its look, for a client to draw it with. */
  botName(bot: string): string | undefined
  botLook(bot: string): Record<string, unknown> | undefined
  /** This computer's name, for a search. */
  computer(): string
}

/** Times as a client takes them: seconds since 1970. */
const seconds = (text: string) => Date.parse(text) / 1000

export function clientItem(item: Item, host: LibraryHost) {
  const there = host.there(item)
  return {
    id: item.id, name: item.name, kind: item.kind, type: item.type, how: item.how, path: item.path,
    ...(item.bot ? { bot: item.bot } : {}),
    ...(item.session ? { session: item.session } : {}),
    ...(item.group ? { group: item.group } : {}),
    ...(item.message ? { message: item.message } : {}),
    ...(item.part ? { part: item.part } : {}),
    at: seconds(item.at),
    ...(item.hidden ? { hidden: seconds(item.hidden) } : {}),
    there,
    thumbnail: there && host.thumbnailable(item),
  }
}

/** The filter a list asks for (`source`, `kind`, `q`, `hidden=1`), or what's wrong with it. */
export function filterOf(query: URLSearchParams): Filter | string {
  const filter: Filter = {}
  const source = query.get("source")
  if (source) {
    if (source !== "all" && source !== "you" && !/^bot_[A-Za-z0-9]+$/.test(source)) return "Say whose: all, you or a bot's id (source)."
    filter.source = source
  }
  const kind = query.get("kind")
  if (kind && kind !== "all") {
    if (!kinds.has(kind as LibraryKind)) return `There's no kind called ${kind}.`
    filter.kind = kind as LibraryKind
  }
  const words = query.get("q")?.trim()
  if (words) filter.query = words.slice(0, 200)
  if (query.get("hidden") === "1") filter.hidden = true
  return filter
}

/** This computer's part of the Library for `filter`: the newest items, its bots and whether any is yours. */
export function listed(store: LibraryStore, host: LibraryHost, filter: Filter) {
  const all = store.list(filter, (bot) => host.botName(bot), host.computer())
  const items = all.slice(0, listLimit).map((item) => clientItem(item, host))
  return { items, more: all.length > items.length, sources: sources(store, host) }
}

function sources(store: LibraryStore, host: LibraryHost) {
  const { bots, you } = store.sources()
  return { bots: bots.map((id) => {
    const look = host.botLook(id)
    return { id, name: host.botName(id) ?? null, ...(look ? { look } : {}) }
  }), you }
}

/**
 * `/api/kuvo/library[/…]` on the box's services, as `parts` (what follows `library`). A box whose
 * Library didn't start answers 503.
 */
export async function libraryRoutes(store: LibraryStore | undefined, host: LibraryHost, method: string, parts: string[], query: URLSearchParams, body: () => Promise<any>, signal?: AbortSignal): Promise<Response> {
  if (!store) return message(503, "This computer's Library isn't running. Try again in a moment.")
  const [first, second] = parts
  if (!first) {
    if (method !== "GET") return message(405, "Read it with GET.")
    const filter = filterOf(query)
    if (typeof filter === "string") return message(400, filter)
    return json(listed(store, host, filter))
  }
  if (first === "peer-copy") {
    if (method !== "GET") return message(405, "Read it with GET.")
    const items = [...store.list(), ...store.list({ hidden: true })].map(item => clientItem(item, host))
    const botNames = Object.fromEntries([...new Set(items.flatMap(item => item.bot ? [item.bot] : []))]
      .map(id => [id, host.botName(id) ?? ""]))
    return json({ items, sources: sources(store, host), botNames })
  }
  if (first === "events") {
    if (method !== "GET") return message(405, "Follow it with GET.")
    const filter = filterOf(query)
    if (typeof filter === "string") return message(400, filter)
    return followed(store, () => listed(store, host, filter), signal)
  }
  const item = store.get(first)
  if (!item) return message(404, "That isn't in this computer's Library.")
  if (!second) return method === "GET" ? json(clientItem(item, host)) : message(405, "Read it with GET.")
  if (second === "file") {
    if (method !== "GET") return message(405, "Read it with GET.")
    const bytes = host.bytes(item)
    if (!bytes) return message(410, "It's no longer on this computer.")
    return new Response(bytes, { headers: { "Content-Type": item.type, "Content-Length": String(bytes.length) } })
  }
  if (second === "thumbnail") {
    if (method !== "GET") return message(405, "Read it with GET.")
    const made = host.thumbnailable(item) ? await host.thumbnail(item) : null
    if (!made) return message(404, "There's no thumbnail of it.")
    return new Response(made.bytes, { headers: { "Content-Type": made.type, "Content-Length": String(made.bytes.length), "Cache-Control": "no-cache" } })
  }
  if (second === "hide") {
    if (method !== "POST") return message(405, "Remove it with POST.")
    const { hidden = true } = (await body()) ?? {}
    if (typeof hidden !== "boolean") return message(400, "Say whether it's removed (hidden).")
    return json(clientItem(store.hide(item.id, hidden)!, host))
  }
  return message(404, "No such route.")
}

/** What `read` gives now, and again each time the Library changes, a line each. */
function followed(store: LibraryStore, read: () => unknown, signal?: AbortSignal): Response {
  const encoder = new TextEncoder()
  let stop = () => {}
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      if (signal?.aborted) return controller.close()
      let last = ""
      let pending: ReturnType<typeof setTimeout> | undefined
      const look = () => {
        pending = undefined
        const line = JSON.stringify(read())
        if (line === last) return
        last = line
        try {
          controller.enqueue(encoder.encode(line + "\n"))
        } catch {
          stop()
        }
      }
      // Several at once (a turn's pictures) go as one line.
      const unsubscribe = store.onChange(() => {
        pending ??= setTimeout(look, 100)
      })
      const onAbort = () => {
        stop()
        try {
          controller.close()
        } catch {}
      }
      stop = () => {
        unsubscribe()
        if (pending) clearTimeout(pending)
        signal?.removeEventListener("abort", onAbort)
      }
      signal?.addEventListener("abort", onAbort, { once: true })
      look()
    },
    cancel() {
      stop()
    },
  })
  return new Response(stream, { headers: { "Content-Type": "application/x-ndjson", "Cache-Control": "no-cache" } })
}
