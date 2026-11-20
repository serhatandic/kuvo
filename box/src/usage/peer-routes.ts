import type { LinkDirectory, PeerPost } from "../agents/groups/post"
import { isInsightsMail, type Insights } from "./insights"
import type { LimitsPeers } from "./limits-peers"
import { isResetMail, type LimitResets } from "./limits-resets"
import { isSettingsMail, type SettingsPeers } from "./settings-peers"
import { isUsageMail, type UsagePeers } from "./usage-peers"

export interface UsagePeerParts {
  post: PeerPost
  directory: LinkDirectory
  limits?: LimitsPeers
  resets?: LimitResets
  insights?: Insights
  settings?: SettingsPeers
  usage?: UsagePeers
}

export async function usagePeerRoutes(parts: UsagePeerParts, method: string, path: string[], body: () => Promise<any>, signal?: AbortSignal): Promise<Response> {
  const [route] = path
  if (!route && method === "PUT")
    return parts.directory.set(await body()) ? Response.json({}) : Response.json({ message: "That isn't who your computers are." }, { status: 400 })
  if (route === "mail" && method === "GET") return parts.post.follow(signal)
  if (route === "sent" && method === "POST") {
    const { ids } = await body()
    if (!Array.isArray(ids) || !ids.every(id => typeof id === "string")) return Response.json({ message: "Those aren't mail." }, { status: 400 })
    parts.post.sent(ids)
    return Response.json({})
  }
  if (route === "mail" && method === "POST") {
    const { from, mail } = await body()
    if (typeof from === "string" && from) {
      if (mail?.kind === "limits") parts.limits?.take(from, mail)
      else if (isResetMail(mail)) parts.resets?.take(from, mail)
      else if (isUsageMail(mail)) parts.usage?.take(from, mail)
      else if (isSettingsMail(mail)) parts.settings?.take(from, mail)
      else if (isInsightsMail(mail)) await parts.insights?.take(from, mail)
      else return Response.json({}, { status: 400 })
      return Response.json({})
    }
    return Response.json({}, { status: 400 })
  }
  return Response.json({ message: "Not found" }, { status: 404 })
}
