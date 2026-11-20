// The link records both conversation sources and serves the artifact index and file copies.
import { readFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { citedIn } from "../../agents/agents/codex-citations"
import { parseProvider } from "../../agents/accounts"
import { readSavedSessions, readSession } from "../../agents/store"
import { followEvents } from "../../shared/event-stream"
import { UnsupportedFormatVersion } from "../../shared/format-version"
import type { Target } from "../../shared/records"
import { engineGet, readBridgeFile, readEngineFile } from "../engine/client"
import { record } from "./record"
import type { Event, Source } from "./recorder"
import { artifactRoutes, notedRoute } from "./routes"
import { ArtifactStore } from "./store"

const sessionRoute = (id: string) => `api/session/${encodeURIComponent(id)}`
const sessionFrom = async (target: Target, id: string) => {
  const found: any = await engineGet(target, sessionRoute(id))
  return found?.data ?? found
}

export function artifactService(root: string, bots: string, sessions: string) {
  let store: ArtifactStore
  try {
    const shown: { path: string; at: number }[] = []
    for (const { session, messages } of readSavedSessions(sessions)) {
      for (const message of messages) {
        // Old directives may already have been normalized by the bridge.
        citedIn(message)
        if (message.type !== "assistant") continue
        for (const part of message.content) {
          if (part.type !== "tool" || !part.id.startsWith(`cited-${message.id}-`)) continue
          const path = (part.state.input as { path?: unknown } | undefined)?.path
          if (typeof path === "string") shown.push({ path: resolve(session.location.directory, path), at: message.time.completed ?? message.time.created })
        }
      }
    }
    store = new ArtifactStore(root, { bots, shown })
  } catch (error) {
    console.error("artifacts:", error)
    return (request: Request) => error instanceof UnsupportedFormatVersion
      ? Response.json({ code: "unsupportedFormatVersion", message: error.message }, { status: 409 })
      : artifactRoutes(undefined, request.method, [], new URLSearchParams(), async () => ({}))
  }
  const { recorder } = record(store, { bots })
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
