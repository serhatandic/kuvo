// The link records both conversation sources and serves the artifact index and file copies.
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { parseProvider } from "../../agents/accounts"
import { readSession } from "../../agents/store"
import { followEvents } from "../../shared/event-stream"
import { UnsupportedFormatVersion } from "../../shared/format-version"
import type { Target } from "../../shared/records"
import { engineGet, readBridgeFile, readEngineFile } from "../engine/client"
import { record } from "./record"
import type { Event, Source } from "./recorder"
import { artifactRoutes, notedRoute } from "./routes"
import { ArtifactStore } from "./store"
import { libraryService } from "../library/service"
import { prepareOnWorker } from "./prepare-worker"
export { artifactCitations } from "./prepare"

const sessionRoute = (id: string) => `api/session/${encodeURIComponent(id)}`
const sessionFrom = async (target: Target, id: string) => {
  const found: any = await engineGet(target, sessionRoute(id))
  return found?.data ?? found
}

const libraryPath = /^\/api\/kuvo\/(link\/)?library(?:\/|$)/

function startupError(unsupported?: string) {
  return (request: Request) => libraryPath.test(new URL(request.url).pathname)
    ? Promise.resolve(Response.json({ message: "This computer's Library isn't running. Try again in a moment." }, { status: 503 }))
    : unsupported
    ? Promise.resolve(Response.json({ code: "unsupportedFormatVersion", message: unsupported }, { status: 409 }))
    : artifactRoutes(undefined, request.method, [], new URLSearchParams(), async () => ({}))
}

/** `library`: the Library's folder, told by the same recorder what was handed over (none: no Library). */
export function artifactService(root: string, bots: string, sessions: string, { library, computer }: { library?: string; computer?: () => string } = {}) {
  const ready = open()
  return Object.assign(async (request: Request) => (await ready)(request), { ready })

  async function open() {
    let store: ArtifactStore
    let shelf: ReturnType<typeof libraryService> | undefined
    try {
      const loaded = await prepareOnWorker(root, bots, sessions, library)
      if (!loaded.prepared) return startupError(loaded.unsupported)
      store = new ArtifactStore(root, { bots }, loaded.prepared.inventory)
      if (library && loaded.prepared.extra) {
        shelf = libraryService(library, { bots, sessions, artifacts: store, computer }, loaded.prepared.extra)
      }
    } catch (error) {
      console.error("artifacts:", error)
      return startupError(error instanceof UnsupportedFormatVersion ? error.message : undefined)
    }
    const { recorder } = record(store, { bots, handed: shelf?.handed })
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
      if (libraryPath.test(url.pathname)) {
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
