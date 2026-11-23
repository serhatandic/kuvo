import { VoiceWords, voiceRoutes } from "./voice"
import { readBridgeFile, readEngineFile } from "./engine/client"
import { server as requests } from "../agents/groups/api"
import type { Target } from "../shared/records"
import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../shared/route-version"
// The resident box serves these routes on a private endpoint also used by the bridge.
import { existsSync, mkdirSync, rmSync } from "node:fs"
import { dirname, join } from "node:path"
import { randomBytes, timingSafeEqual } from "node:crypto"
import { leftSignIns } from "../agents/signin"
import { writeSecret } from "../shared/files"
import { githubPath, readServices, servicesAuthorization, servicesFile, servicesRecordFormat } from "../shared/box-services"
import { HangWatch, isEngine } from "../shared/hang"
import { usageReader, type UsageOptions } from "../usage/service"
import { artifactService } from "./artifacts/service"
import { ownComputerName } from "./computer-name"
import { APP_FOLDER, located } from "./home"
import { GitHubSignIn } from "./work-queue/github-sign-in"
import { DATA_ID, ownerOf } from "./ownership/resolve"
import { seatPeers } from "./peers/seats"
import { peersOf } from "./peers/known"
import { readLink } from "./state"

export function serveBoxServices(options: UsageOptions & { artifacts: string }) {
  const { handle, started, closeInsights } = usageReader({
    ...options,
    initialPeers: seatPeers(),
    conversationHome: (id, computer) => {
      if (!DATA_ID.test(id)) return
      const link = readLink()
      const owner = ownerOf({ kind: "conversations", id, computer }, { link })
      if (owner?.key && (owner.local || peersOf(link).some(peer => peer.key === owner.key))) return owner.key
      return undefined
    },
  })
  const artifacts = artifactService(options.artifacts, options.bots, join(options.home, "sessions"), { library: join(dirname(options.artifacts), "library"), computer: ownComputerName })
  const api = (read: () => Target) => ({
    request<T>(...args: Parameters<ReturnType<typeof requests>["request"]>): Promise<T> {
      const { port, password } = read()
      return requests(`http://127.0.0.1:${port}`, password).request<T>(...args)
    },
  })
  const voice = new VoiceWords({
    engine: { api: api(readEngineFile) },
    bridge: { api: api(readBridgeFile) },
    isBridge: id => existsSync(join(options.home, "sessions", id, "session.json")),
  })
  const temporary = join(options.run, "github-sign-in")
  mkdirSync(temporary, { recursive: true, mode: 0o700 })
  leftSignIns(temporary)
  // These replies used the bridge's working directory before GitHub sign-in moved here.
  const github = new GitHubSignIn({ home: options.box, temporary }, located(APP_FOLDER ? options.home : join(options.box, "workspace")))
  const password = randomBytes(24).toString("hex")
  const expected = Buffer.from(servicesAuthorization(password))
  const server = Bun.serve({
    hostname: "127.0.0.1", port: 0, idleTimeout: 0,
    async fetch(request) {
      const auth = Buffer.from(request.headers.get("authorization") ?? "")
      if (auth.length !== expected.length || !timingSafeEqual(auth, expected)) return new Response(null, { status: 401 })
      const refusal = routeVersionError(request.headers.get(ROUTE_VERSION_HEADER) ?? undefined)
      if (refusal) return Response.json(refusal, { status: 400, headers: { [ROUTE_VERSION_HEADER]: ROUTE_VERSION } })
      const { pathname } = new URL(request.url)
      const response = await (githubPath(pathname) ? github.handle(request)
        : /^\/api\/kuvo\/(link\/)?(artifacts|library)(?:\/|$)/.test(pathname) ? artifacts(request)
        : /^\/api\/kuvo\/voice(?:\/|$)/.test(pathname) ? voiceRoutes(voice, request.method, pathname.split("/").slice(4))
        : handle(request))
      const headers = new Headers(response.headers)
      headers.set(ROUTE_VERSION_HEADER, ROUTE_VERSION)
      return new Response(response.body, { status: response.status, headers })
    },
  })
  const file = servicesFile(options.run)
  const leave = () => {
    if (readServices(options.run)?.pid === process.pid) rmSync(file, { force: true })
    github.cancel()
    server.stop(true)
    closeInsights()
  }
  new HangWatch({ name: "engine", run: options.run, record: join(options.run, "engine.json"), isIt: isEngine }).start()
  started(artifacts.ready)
  writeSecret(file, JSON.stringify(servicesRecordFormat.write({ pid: process.pid, port: server.port, password })))
  return leave
}
