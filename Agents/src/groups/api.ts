// What a group chat on a machine asks of where its bots' seats run: the machine's engine, or this
// bridge itself for a seat on a coding agent's model. Both answer the engine's API.

import type { ModelRef } from "../wire"
import type { Rule } from "./rules"

export interface Requester {
  request<T = any>(method: string, path: string, body?: unknown, timeout?: number): Promise<T>
}

/** A server that answers the engine's API at `url` behind `password` (this bridge, from inside it). */
export function server(url: string, password: string): Requester {
  const authorization = "Basic " + Buffer.from(`opencode:${password}`).toString("base64")
  return {
    async request<T>(method: string, path: string, body?: unknown, timeout = 30_000): Promise<T> {
      const response = await fetch(url.replace(/\/$/, "") + path, {
        method,
        headers: { Authorization: authorization, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeout),
      })
      const text = await response.text()
      let value: any
      try {
        value = text ? JSON.parse(text) : undefined
      } catch {
        value = text
      }
      if (!response.ok) {
        const error: any = new Error(value?.message ?? `It answered ${response.status}.`)
        error.status = response.status
        throw error
      }
      return value as T
    },
  }
}

const at = (id: string) => "/api/session/" + encodeURIComponent(id)

/** The calls a group chat makes, as the Mac's OpenCodeAPI makes them. */
export class SeatAPI {
  constructor(readonly requester: Requester) {}

  async active(): Promise<Set<string>> {
    const answer = await this.requester.request("GET", "/api/session/active")
    return new Set(Object.keys(answer?.data ?? {}))
  }

  async session(id: string): Promise<any> {
    return (await this.requester.request("GET", at(id)))?.data
  }

  /** The name of `model`'s provider when it has a way to sign in and isn't signed in here (a
   * model there would only fail: the engine drops a message to it without a word); else nothing. */
  /** The account `model` runs on: its provider's name, and whether it's one you sign in to that has no sign-in. */
  async account(model: ModelRef): Promise<{ name: string; signedOut: boolean } | undefined> {
    const listed: any[] = (await this.requester.request("GET", "/api/integration"))?.data
    const provider = Array.isArray(listed) ? listed.find((each) => each?.id === model.providerID) : undefined
    if (!provider) return undefined
    const needsOne = Array.isArray(provider.methods) && provider.methods.length > 0
    return { name: String(provider.name ?? provider.id), signedOut: needsOne && !(Array.isArray(provider.connections) && provider.connections.length > 0) }
  }

  async create(directory: string, title: string, agent: string | undefined, model: ModelRef | undefined, permissions: Rule[]): Promise<any> {
    const body: Record<string, unknown> = { title, location: { directory }, permissions }
    if (agent) body.agent = agent
    if (model) body.model = model
    return (await this.requester.request("POST", "/api/session", body))?.data
  }

  async setPermissions(id: string, rules: Rule[]) {
    await this.requester.request("PATCH", at(id), { permissions: rules })
  }

  async setMetadata(id: string, metadata: Record<string, unknown>) {
    await this.requester.request("PATCH", at(id), { metadata })
  }

  async setInstruction(id: string, key: string, text: string) {
    await this.requester.request("PUT", `/api/experimental/session/${encodeURIComponent(id)}/instructions/entries/${encodeURIComponent(key)}`, { value: text })
  }

  async removeInstruction(id: string, key: string) {
    await this.requester.request("DELETE", `/api/experimental/session/${encodeURIComponent(id)}/instructions/entries/${encodeURIComponent(key)}`)
  }

  async delete(id: string) {
    await this.requester.request("DELETE", at(id))
  }

  async prompt(id: string, message: string, text: string, delivery: "steer" | "queue") {
    await this.requester.request("POST", at(id) + "/prompt", { id: message, text, delivery })
  }

  async interrupt(id: string) {
    await this.requester.request("POST", at(id) + "/interrupt")
  }

  async selectModel(id: string, model: ModelRef) {
    await this.requester.request("POST", at(id) + "/model", { model })
  }

  async selectAgent(id: string, agent: string) {
    await this.requester.request("POST", at(id) + "/agent", { agent })
  }

  /** Waiting input to the next step (`steer`, which also wakes an idle session) or after the run. */
  async setDelivery(id: string, item: string, delivery: "steer" | "queue") {
    await this.requester.request("PATCH", `${at(id)}/inbox/${encodeURIComponent(item)}`, { delivery })
  }

  async cancelInbox(id: string, item: string) {
    await this.requester.request("DELETE", `${at(id)}/inbox/${encodeURIComponent(item)}`)
  }

  async inbox(id: string): Promise<{ id: string }[]> {
    return (await this.requester.request("GET", at(id) + "/inbox"))?.data ?? []
  }

  /** The newest messages, oldest first, and where the page before them starts. */
  async messages(id: string, limit: number, cursor?: string): Promise<{ data: any[]; cursor?: string }> {
    const query = cursor ? `limit=${limit}&cursor=${encodeURIComponent(cursor)}` : `limit=${limit}&order=desc`
    const answer = await this.requester.request("GET", `${at(id)}/message?${query}`)
    const data: any[] = Array.isArray(answer?.data) ? [...answer.data].reverse() : []
    return { data, cursor: answer?.cursor?.next ?? answer?.cursor?.previous }
  }

  async permissions(id: string): Promise<{ id: string }[]> {
    return (await this.requester.request("GET", at(id) + "/permission"))?.data ?? []
  }

  async reject(id: string, permission: string, message: string) {
    await this.requester.request("POST", `${at(id)}/permission/${encodeURIComponent(permission)}/reply`, { decision: "reject", message })
  }

  async forms(id: string): Promise<{ id: string }[]> {
    return (await this.requester.request("GET", at(id) + "/form"))?.data ?? []
  }

  async dismiss(id: string, form: string, message: string) {
    await this.requester.request("DELETE", `${at(id)}/form/${encodeURIComponent(form)}?message=${encodeURIComponent(message)}`)
  }
}
