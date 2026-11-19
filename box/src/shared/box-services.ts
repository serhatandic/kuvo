// The link's local services have a private loopback endpoint. The bridge forwards its older
// local client routes here and continues to relay service events on its event feed.
import { GITHUB_WORK_QUEUE } from "./github"
import { join } from "node:path"
import { additiveObjectFormat } from "./format-version"
import { readRendezvous, type Target } from "./records"

export const servicesRecordFormat = additiveObjectFormat("Box services rendezvous")
export const BOX_SERVICES_ROLE = /kuvo-link\.js box-services(\s|$)/
export const servicesFile = (run: string) => join(run, "box-services.json")
export const usagePath = (path: string) => /^\/?api\/kuvo\/(limits|usage|insights)(?:[/?]|$)/.test(path)
export const githubPath = (path: string) => path === `/api/integration/${GITHUB_WORK_QUEUE}` || path.startsWith(`/api/integration/${GITHUB_WORK_QUEUE}/connect/oauth`)
export function servicePath(path: string): boolean {
  const pathname = "/" + path.split("?")[0]!.replace(/^\/+/, "")
  return usagePath(pathname) || /^\/api\/kuvo\/voice(?:\/|$)/.test(pathname) || githubPath(pathname) ||
    pathname === "/api/kuvo/artifacts/noted" || /^\/api\/kuvo\/link\/artifacts(?:\/|$)/.test(pathname)
}
export const usageMail = (kind: unknown) =>
  kind === "limits" || kind === "usage" || kind === "insights" || kind === "settings" || kind === "reset"

export function readServices(run: string): (Target & { pid: number }) | undefined {
  try {
    const record = readRendezvous<Target & { pid: number }>(servicesRecordFormat, servicesFile(run))
    if (Number.isSafeInteger(record.port) && record.port > 0 && record.port <= 65535 &&
        /^[a-f0-9]{48}$/.test(record.password) && Number.isSafeInteger(record.pid) && record.pid > 0) return record
  } catch {}
  return undefined
}

export const servicesAuthorization = (password: string) => "Basic " + Buffer.from(`opencode:${password}`).toString("base64")

export function askServices(target: Target, path: string, options: RequestInit = {}): Promise<Response> {
  const headers = new Headers(options.headers)
  headers.set("authorization", servicesAuthorization(target.password))
  return fetch(new URL("/" + path.replace(/^\/+/, ""), `http://127.0.0.1:${target.port}`), { ...options, headers, redirect: "error" })
}
