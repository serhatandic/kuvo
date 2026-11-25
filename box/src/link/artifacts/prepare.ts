import { closeSync, openSync, readFileSync, readSync } from "node:fs"
import { join, resolve } from "node:path"
import { citedIn } from "../../agents/agents/codex-citations"
import { readSavedSessionHeaders } from "../../agents/store"
import { savedMessages, transcriptDatabase } from "../../agents/transcript"
import { insightKey } from "../../usage/insight-records"
import { fileStamp, writeSecret } from "../../shared/files"
import { UnsupportedFormatVersion } from "../../shared/format-version"
import { ArtifactStore } from "./store"
import { prepareLibrary } from "../library/service"

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

/** One disk worker owns startup writes to both indexes until the resident adopts them. */
export async function prepareArtifactServices(root: string, bots: string, sessions: string, library?: string) {
  const checkpoint = join(root, "citations-v1.json")
  let previous: Checkpoints = {}
  try { previous = JSON.parse(readFileSync(checkpoint, "utf8")) ?? {} } catch {}
  const { shown, checkpoints } = citationUpdate(sessions, previous)
  try {
    const prepared = await ArtifactStore.prepare(root, { bots, shown }, artifacts => {
      if (!library) return
      try { return prepareLibrary(library, { bots, sessions, artifacts }) }
      catch (error) {
        console.error("library:", error)
        return undefined
      }
    })
    // An unchanged checkpoint stays as it is: a departed computer's released work
    // must not change by starting again.
    const same = (a: Checkpoints, b: Checkpoints) => Object.keys(a).length === Object.keys(b).length
      && Object.entries(a).every(([id, revision]) => b[id] === revision)
    try { if (!same(previous, checkpoints)) writeSecret(checkpoint, JSON.stringify(checkpoints)) }
    catch (error) { console.error("artifacts: could not checkpoint saved citations", error) }
    return { prepared }
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) return { unsupported: error.message }
    throw error
  }
}
