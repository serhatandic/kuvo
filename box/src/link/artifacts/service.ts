// The link records both conversation sources and serves the artifact index and file copies.
import { readdir } from "node:fs/promises"
import { closeSync, openSync, readFileSync, readSync } from "node:fs"
import { join, resolve } from "node:path"
import { citedIn } from "../../agents/agents/codex-citations"
import { parseProvider } from "../../agents/accounts"
import { readSavedSessionHeaders, readSession } from "../../agents/store"
import { savedMessages, transcriptDatabase } from "../../agents/transcript"
import { insightKey } from "../../usage/insight-records"
import { fileStamp, writeSecret } from "../../shared/files"
import { followEvents } from "../../shared/event-stream"
import { UnsupportedFormatVersion } from "../../shared/format-version"
import type { Target } from "../../shared/records"
import { engineGet, readBridgeFile, readEngineFile } from "../engine/client"
import { record } from "./record"
import type { Event, Source } from "./recorder"
import { artifactRoutes, notedRoute } from "./routes"
import { ArtifactStore } from "./store"
import { libraryService } from "../library/service"
import { artifactDisk } from "../server/disk-worker"

const sessionRoute = (id: string) => `api/session/${encodeURIComponent(id)}`
const sessionFrom = async (target: Target, id: string) => {
  const found: any = await engineGet(target, sessionRoute(id))
  return found?.data ?? found
}

type Citation = { path: string; at: number }
type Checkpoints = Record<string, string>
const sweep = typeof Bun !== "undefined" ? (await import("bun:jsc")).gcAndSweep : undefined

/** Avoid decoding legacy conversations with nothing that can mark an artifact as shown. */
function mayHaveCitations(file: string, buffer: Buffer) {
  const fd = openSync(file, "r")
  try {
    let overlap = 0
    for (;;) {
      const size = readSync(fd, buffer, overlap, buffer.length - overlap, null)
      if (!size) return false
      const bytes = buffer.subarray(0, overlap + size)
      // An escaped code point could spell part of a directive or citation id.
      if (bytes.includes(":codex-") || bytes.includes("cited-") || bytes.includes("\\u")) return true
      overlap = Math.min(bytes.length, ":codex-".length - 1)
      bytes.copy(buffer, 0, bytes.length - overlap)
    }
  } finally { closeSync(fd) }
}

export function artifactCitations(sessions: string) {
  return citationUpdate(sessions, {}).shown
}

export function citationUpdate(sessions: string, previous: Checkpoints) {
  const shown = new Map<string, number>()
  const checkpoints: Checkpoints = Object.create(null)
  let decoded = 0
  const buffer = Buffer.allocUnsafe(64 * 1024)
  for (const { folder, session } of readSavedSessionHeaders(sessions)) {
    try {
      const files = ["session.json", transcriptDatabase, transcriptDatabase + "-wal", "messages.json"]
        .map(name => fileStamp(join(folder, name)))
      const revision = insightKey(files)
      if (previous[session.id] !== revision && (files[1] || files[3] && mayHaveCitations(join(folder, "messages.json"), buffer))) {
        for (const message of savedMessages(folder, "artifacts")) {
          // Old directives may already have been normalized by the bridge.
          citedIn(message)
          if (message.type !== "assistant") continue
          for (const part of message.content) {
            if (part.type !== "tool" || !part.id.startsWith(`cited-${message.id}-`)) continue
            const path = (part.state.input as { path?: unknown } | undefined)?.path
            if (typeof path !== "string") continue
            const file = resolve(session.location.directory, path)
            const at = message.time.completed ?? message.time.created
            if (at > (shown.get(file) ?? -Infinity)) shown.set(file, at)
          }
        }
        decoded += files.reduce((sum, file) => sum + (file?.size ?? 0), 0)
        if (decoded >= 2 * 1024 * 1024) {
          decoded = 0
          sweep?.()
        }
      }
      checkpoints[session.id] = revision
    } catch (error) {
      console.error("Unable to read saved citations", session.id, error)
    }
  }
  return { shown: Array.from(shown, ([path, at]): Citation => ({ path, at })), checkpoints }
}

/** Read saved citations off the resident loop, starting at its last completed checkout of the transcripts. */
async function artifactHistory(sessions: string, file: string) {
  const entries = await readdir(sessions).catch(() => [])
  if (!entries.length) return { shown: [], checkpoints: {} }
  let checkpoints: Checkpoints = {}
  try { checkpoints = JSON.parse(readFileSync(file, "utf8")) ?? {} } catch {}
  return artifactDisk.run("citationUpdate", sessions, checkpoints)
}

/** `library`: the Library's folder, told by the same recorder what was handed over (none: no Library). */
export function artifactService(root: string, bots: string, sessions: string, { library, computer }: { library?: string; computer?: () => string } = {}) {
  const ready = open()
  return Object.assign(async (request: Request) => (await ready)(request), { ready })

  async function open() {
    let store: ArtifactStore
    try {
      const checkpoint = join(root, "citations-v1.json")
      const { shown, checkpoints } = await artifactHistory(sessions, checkpoint)
      store = await ArtifactStore.open(root, { bots, shown })
      try { writeSecret(checkpoint, JSON.stringify(checkpoints)) }
      catch (error) { console.error("artifacts: could not checkpoint saved citations", error) }
    } catch (error) {
      console.error("artifacts:", error)
      return (request: Request) => /^\/api\/kuvo\/(link\/)?library(?:\/|$)/.test(new URL(request.url).pathname)
        ? Promise.resolve(Response.json({ message: "This computer's Library isn't running. Try again in a moment." }, { status: 503 }))
        : error instanceof UnsupportedFormatVersion
        ? Response.json({ code: "unsupportedFormatVersion", message: error.message }, { status: 409 })
        : artifactRoutes(undefined, request.method, [], new URLSearchParams(), async () => ({}))
    }
    let shelf: ReturnType<typeof libraryService> | undefined
    if (library) {
      try {
        shelf = libraryService(library, { bots, sessions, artifacts: store, computer })
      } catch (error) {
        console.error("library:", error)
      }
    }
    const { recorder } = record(store, { bots, handed: shelf?.handed })
    // Filled once from what came before it, after the start (the record's own reading comes first).
    if (shelf) void shelf.backfillInBackground().catch(error => console.error("library:", error))
    const engine: Source = { name: "engine", session: id => sessionFrom(readEngineFile(), id), agent: () => "engine" }
    const bridge: Source = {
      name: "bridge",
      session: id => sessionFrom(readBridgeFile(), id),
      agent(id) {
        if (!/^ses_[A-Za-z0-9_-]+$/.test(id)) return undefined
        try {
          const session = readSession(JSON.parse(readFileSync(join(sessions, id, "session.json"), "utf8")))
          return parseProvider(session.model?.providerID ?? "")?.kind ?? "engine"
        } catch { return undefined }
      },
    }
    void follow(readEngineFile, engine)
    void follow(readBridgeFile, bridge)

    function follow(target: () => Target, source: Source) {
      return followEvents<Event>(async signal => {
        const { port, password } = target()
        return fetch(`http://127.0.0.1:${port}/api/event?passive=1`, {
          headers: { authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64") }, signal,
        })
      }, event => { void recorder.heard(source, event) })
    }

    return async (request: Request): Promise<Response> => {
      const url = new URL(request.url)
      if (/^\/api\/kuvo\/(link\/)?library(?:\/|$)/.test(url.pathname)) {
        return shelf ? shelf.handle(request) : Response.json({ message: "This computer's Library isn't running. Try again in a moment." }, { status: 503 })
      }
      const body = () => request.json().catch(() => ({}))
      if (url.pathname === "/api/kuvo/artifacts/noted") {
        return notedRoute((id, path, via) => recorder.noted([engine, bridge], id, path, via), request.method, body)
      }
      if (url.pathname === "/api/kuvo/artifacts/given" && request.method === "POST") {
        const { prompt, chat } = await body() as { prompt?: unknown; chat?: unknown }
        if (typeof prompt === "string" && typeof chat === "string") recorder.given(prompt, chat)
        return Response.json({})
      }
      return artifactRoutes(store, request.method, url.pathname.split("/").filter(Boolean).slice(4), url.searchParams, body, request.signal)
    }
  }
}
