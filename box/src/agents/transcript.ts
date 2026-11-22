// Coding-agent messages are checkpointed by position. Portable moves and backups still use
// the legacy JSON array, read from one descriptor and decoded one message at a time.
import { closeSync, existsSync, fstatSync, openSync, readSync, renameSync, rmSync, statSync, utimesSync, writeSync } from "node:fs"
import { StringDecoder } from "node:string_decoder"
import { createHash } from "node:crypto"
import { dirname, join } from "node:path"
import { pathToFileURL } from "node:url"
import type { DatabaseSync } from "node:sqlite"
import { syncDurableFile, writeDurablePrivate } from "../shared/files"
import type { Attachment, Message, ToolOutput } from "./wire"
import { projectedTranscript } from "./transcript-projection"

/** Read a legacy array with its original whitespace, escapes and Unicode intact. */
function* legacyMessages(file: string): Generator<Message> {
  const fd = openSync(file, "r")
  const buffer = Buffer.allocUnsafe(128 * 1024)
  const decoder = new StringDecoder("utf8")
  const outside = /["{},\[\]]/g
  const inside = /["\\]/g
  const pieces: string[] = []
  let array = false, ended = false, comma = false, quoted = false, escaped = false, depth = 0
  try {
    for (;;) {
      const count = readSync(fd, buffer, 0, buffer.length, null)
      const chunk = count ? decoder.write(buffer.subarray(0, count)) : decoder.end()
      let from = 0
      if (!array) {
        const start = chunk.search(/\S/)
        if (start < 0 && count) continue
        if (chunk[start] !== "[") throw new SyntaxError("Expected a conversation array.")
        array = true
        from = start + 1
      }
      let cursor = from
      while (!ended && cursor < chunk.length) {
        const pattern = quoted ? inside : outside
        pattern.lastIndex = cursor + Number(escaped)
        escaped = false
        const next = pattern.exec(chunk)
        if (!next) break
        const char = next[0], at = next.index
        cursor = at + 1
        if (quoted) {
          if (char === '"') quoted = false
          else if (cursor < chunk.length) cursor++
          else escaped = true
        } else if (char === '"') quoted = true
        else if (char === "{" || char === "[") depth++
        else if (!depth && (char === "," || char === "]")) {
          pieces.push(chunk.slice(from, at))
          const text = pieces.join("")
          pieces.length = 0
          from = cursor
          if (text.trim() || comma || char === ",") yield JSON.parse(text) as Message
          comma = char === ","
          ended = char === "]"
        } else if (char !== ",") depth--
      }
      if (ended) {
        if (chunk.slice(from).trim()) throw new SyntaxError("Unexpected text after the conversation.")
      } else if (from < chunk.length) pieces.push(chunk.slice(from))
      if (!count) break
    }
    if (!ended) throw new SyntaxError("The saved conversation ended early.")
  } finally { closeSync(fd) }
}


export const transcriptDatabase = "messages-v1.sqlite"
export const transcriptPictures = "transcript-pictures"
const sessionID = /^ses_[A-Za-z0-9_-]{1,120}$/
const pictures: Record<string, string> = {
  "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp",
  "image/heic": "heic", "image/heif": "heif", "image/bmp": "bmp", "image/tiff": "tiff",
}

const types: Record<string, string> = {
  "application/pdf": "pdf", "text/plain": "txt", "text/markdown": "md", "application/json": "json",
}
function retainURI(root: string | undefined, id: string, uri: string, mime: string): string {
  if (!root || !sessionID.test(id) || typeof uri !== "string" || !uri.startsWith("data:")) return uri
  const comma = uri.indexOf(",")
  if (comma < 0 || uri.length - comma > 35 * 1024 ** 2) return uri
  let data: Buffer
  try {
    const payload = uri.slice(comma + 1)
    const base64 = uri.slice(5, comma).toLowerCase().endsWith(";base64")
    if (base64 && !/^[a-zA-Z0-9+/]*={0,2}$/.test(payload)) return uri
    data = base64 ? Buffer.from(payload, "base64") : Buffer.from(decodeURIComponent(payload))
  } catch { return uri }
  const file = saveTranscriptAttachment(root, id, mime, data)
  return file ? pathToFileURL(file).href : uri
}

export function saveTranscriptAttachment(root: string, id: string, mime: string, data: Buffer): string | undefined {
  if (!sessionID.test(id) || !data.length || data.length > 25 * 1024 ** 2) return undefined
  const type = mime?.toLowerCase() ?? ""
  const extension = pictures[type] ?? types[type] ?? "bin"
  const hash = createHash("sha256").update(data).digest("hex")
  const folder = join(root, id, transcriptPictures)
  const file = join(folder, hash + "." + extension)
  try {
    if (!existsSync(file)) writeDurablePrivate(file, data, dirname(root))
    return file
  } catch {
    // A full disk must leave the original picture readable.
    return undefined
  }
}

export function retainAttachment(root: string | undefined, id: string, file: Attachment): Attachment {
  const uri = file.data ? `data:${file.mime};base64,${file.data}` : file.source?.type === "uri" ? file.source.uri : undefined
  if (!uri) return file
  const kept = retainURI(root, id, uri, file.mime)
  if (kept === uri) return file
  const { data: _, ...metadata } = file
  return { ...metadata, source: { type: "uri", uri: kept } }
}

export function retainOutput(root: string | undefined, id: string, output: ToolOutput): ToolOutput {
  if (output?.type !== "file") return output
  const uri = retainURI(root, id, output.uri, output.mime)
  return uri === output.uri ? output : { ...output, uri }
}

function retainMessage(root: string | undefined, id: string, message: Message) {
  if (message.type === "user" && message.files) {
    const files = message.files.map(file => file && retainAttachment(root, id, file))
    if (files.some((file, index) => file !== message.files![index])) message.files = files
  }
  if (message.type === "assistant" && Array.isArray(message.content)) for (const part of message.content) {
    if (part.type !== "tool" || (part.state.status !== "completed" && part.state.status !== "error") || !part.state.content) continue
    const before = part.state.content
    const content = before.map(output => retainOutput(root, id, output))
    if (content.some((output, index) => output !== before[index])) part.state.content = content
  }
}

function database(file: string, read = false) {
  if (process.versions.bun) {
    const SQLite = require(["bun", "sqlite"].join(":")).Database as new (file: string, options?: { readonly: boolean }) => DatabaseSync
    return new SQLite(file, read ? { readonly: true } : undefined)
  }
  const SQLite = require("node:sqlite").DatabaseSync as typeof DatabaseSync
  return new SQLite(file, { readOnly: read })
}

export type MessageView = "body" | "insight" | "artifacts"

function projection(message: Message, view: MessageView): Message | undefined {
  if (view === "body") return message
  if (message?.type === "user") {
    if (view === "insight") return { type: "user", id: message.id, time: message.time, text: "", metadata: { source: message.metadata?.source } }
    const files = message.files?.filter(file => {
      const legacy = file as Attachment & { uri?: string; url?: string }
      const uri = legacy?.source?.type === "uri" ? legacy.source.uri : legacy?.uri ?? legacy?.url
      return typeof uri === "string" && uri.startsWith("file:")
    })
    const text = message.text?.includes("<attached-files>") ? message.text : ""
    if (files?.length || text) return { type: "user", id: message.id, time: message.time, files, text }
    return undefined
  }
  if (message?.type !== "assistant" || !Array.isArray(message.content)) return undefined
  const { id, time, model, agent, tokens } = message
  const content: typeof message.content = []
  for (const part of message.content) {
    if (view === "artifacts" && part.type === "text" && part.text?.includes(":codex-")) {
      content.push(part)
      continue
    }
    if (part.type !== "tool") continue
    const { state, name } = part
    const input = (state.status === "streaming" ? undefined : state.input) as Record<string, unknown> | undefined
    if (view === "insight" && state.status === "completed" && /^(skill|read|read_file)$/i.test(name)) {
      const { skill, name: named, file_path, filePath, path } = input ?? {}
      content.push({ type: "tool", id: part.id, name, state: { status: "completed", input: { skill, name: named, file_path, filePath, path }, content: [] } })
    } else if (view === "artifacts" && (/(^|_)(show_file|generate_image)$/.test(name) || part.id.startsWith(`cited-${id}-`))) {
      const files = (state.status === "completed" || state.status === "error")
        ? state.content?.filter(file => file?.type === "file" && file.uri?.startsWith("file:")) ?? [] : []
      content.push({ type: "tool", id: part.id, name, state: { status: "completed", input: { path: input?.path }, content: files } })
    }
  }
  if (view === "insight" || content.length) return { type: "assistant", id, time, model, agent, tokens, content }
  return undefined
}

export function* savedMessages(folder: string, view: MessageView = "body"): Generator<Message> {
  const file = join(folder, transcriptDatabase)
  if (!existsSync(file)) {
    const legacy = join(folder, "messages.json")
    if (!existsSync(legacy)) return
    if (view === "body") yield* legacyMessages(legacy)
    else for (const message of projectedTranscript(legacy, view)) {
      const projected = projection(message as Message, view)
      if (projected !== undefined) yield projected
    }
    return
  }
  const db = database(file, true)
  try {
    db.exec("PRAGMA busy_timeout=1000")
    for (const row of db.prepare(`SELECT ${view} AS body FROM message WHERE ${view} IS NOT NULL ORDER BY position`).iterate()) {
      yield JSON.parse(row.body as string) as Message
    }
  } finally { db.close() }
}

// File time is used only to decide whether a crashed turn's edits are recent enough to recover.
export function messageFileTime(folder: string) {
  for (const name of [transcriptDatabase + "-wal", transcriptDatabase, "messages.json"]) {
    try { return statSync(join(folder, name)).mtimeMs } catch {}
  }
  return undefined
}

/** Preferences have transcript markers of their own, but aren't conversation activity. */
export function messageActivity(message: Message): number | undefined {
  if (message.type === "model-switched" || message.type === "agent-switched") return
  return message.time?.completed ?? message.time?.streamed ?? message.time?.created
}

/** Walk the portable array from the end, so a list read needn't decode a conversation or migrate it. */
function* legacyTail(file: string): Generator<Message> {
  const fd = openSync(file, "r")
  const size = fstatSync(fd).size
  const block = Buffer.allocUnsafe(32 * 1024)
  let offset = -block.length
  const byte = (at: number) => {
    if (at < offset || at >= offset + block.length) {
      offset = Math.floor(at / block.length) * block.length
      readSync(fd, block, 0, Math.min(block.length, size - offset), offset)
    }
    return block[at - offset]
  }
  try {
    let end = 0, depth = 0, quoted = false
    for (let at = size - 1; at >= 0; at--) {
      const char = byte(at)
      if (char === 34) {
        let escape = at - 1
        while (escape >= 0 && byte(escape) === 92) escape--
        if ((at - escape) % 2 === 1) quoted = !quoted
      }
      if (quoted) continue
      if (char === 125) {
        if (depth === 0) end = at + 1
        depth++
      }
      if (char !== 123) continue
      depth--
      if (depth !== 0) continue
      const text = Buffer.allocUnsafe(end - at)
      for (let read = 0; read < text.length;) {
        const count = readSync(fd, text, read, text.length - read, at + read)
        if (!count) throw new SyntaxError("The saved conversation ended early.")
        read += count
      }
      yield JSON.parse(text.toString("utf8")) as Message
    }
  } finally { closeSync(fd) }
}

export function savedActivity(folder: string): number | undefined {
  const file = join(folder, transcriptDatabase)
  if (!existsSync(file)) {
    const legacy = join(folder, "messages.json")
    if (existsSync(legacy)) for (const message of legacyTail(legacy)) {
      const at = messageActivity(message)
      if (at !== undefined) return at
    }
    return
  }
  const db = database(file, true)
  try {
    db.exec("PRAGMA busy_timeout=1000")
    const row = db.prepare(`SELECT body FROM message WHERE json_extract(body, '$.type')
      NOT IN ('model-switched', 'agent-switched') ORDER BY position DESC LIMIT 1`).get()
    return row ? messageActivity(JSON.parse(row.body as string)) : undefined
  } finally { db.close() }
}

/** Publish all changed rows in one WAL commit. The legacy array stays readable until the
 * complete replacement is closed and renamed into place. */
export function writeTranscript(
  folder: string, messages: Iterable<[number, Message]>, length: number | undefined,
  attachments: string | undefined, session: string, durable: boolean,
) {
  const file = join(folder, transcriptDatabase)
  const creating = !existsSync(file)
  const target = creating ? file + ".new" : file
  if (creating) {
    rmSync(target, { force: true })
    closeSync(openSync(target, "wx", 0o600))
  }
  const db = database(target)
  try {
    db.exec(`PRAGMA journal_mode=${creating ? "DELETE" : "WAL"}; PRAGMA synchronous=${durable ? "FULL" : "NORMAL"};
      PRAGMA busy_timeout=1000;
      CREATE TABLE IF NOT EXISTS message(
        position INTEGER PRIMARY KEY, id TEXT, body TEXT NOT NULL, insight TEXT, artifacts TEXT
      );
      CREATE INDEX IF NOT EXISTS message_id ON message(id);
      BEGIN IMMEDIATE;`)
    const put = db.prepare(`INSERT INTO message(position, id, body, insight, artifacts) VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(position) DO UPDATE SET id=excluded.id, body=excluded.body, insight=excluded.insight, artifacts=excluded.artifacts
      WHERE body<>excluded.body`)
    for (const [index, message] of messages) {
      retainMessage(attachments, session, message)
      put.run(index, message.id, JSON.stringify(message),
        JSON.stringify(projection(message, "insight")) ?? null,
        JSON.stringify(projection(message, "artifacts")) ?? null)
    }
    if (length !== undefined) db.prepare("DELETE FROM message WHERE position>=?").run(length)
    db.exec("COMMIT")
  } catch (error) {
    db.close()
    if (creating) rmSync(target, { force: true })
    throw error
  }
  db.close()
  if (creating) {
    renameSync(target, file)
    if (durable) syncDurableFile(folder)
  }
}

/** The portable array for a move or backup, using one message's memory. */
export function* transcriptBytes(messages: Iterable<Message>) {
  let prefix = "["
  for (const message of messages) {
    yield prefix + JSON.stringify(message)
    prefix = ","
  }
  yield prefix === "[" ? "[]" : "]"
}

export function exportTranscript(file: string, messages: Iterable<Message>, observe?: (text: string) => void) {
  const fd = openSync(file, "w", 0o600)
  try {
    for (const text of transcriptBytes(messages)) {
      writeSync(fd, text)
      observe?.(text)
    }
  } finally { closeSync(fd) }
}

export function migrateTranscript(folder: string, attachments: string | undefined, id: string) {
  if (existsSync(join(folder, transcriptDatabase))) return
  function* entries(): Generator<[number, Message]> {
    let index = 0
    for (const message of savedMessages(folder)) yield [index++, message]
  }
  writeTranscript(folder, entries(), undefined, attachments, id, true)
  // One compact snapshot for older bridges that still open the legacy path.
  const legacy = join(folder, "messages.json")
  if (existsSync(legacy)) {
    const { atime, mtime } = statSync(legacy)
    utimesSync(join(folder, transcriptDatabase), atime, mtime)
    exportTranscript(legacy + ".new", savedMessages(folder))
    renameSync(legacy + ".new", legacy)
  }
}

/** Indexed reads for an idle conversation: the bridge need only decode the requested rows. */
export function savedPage(folder: string, after: string | undefined, limit: number, ascending: boolean) {
  const file = join(folder, transcriptDatabase)
  if (!existsSync(file)) return undefined
  const db = database(file, true)
  try {
    db.exec("PRAGMA busy_timeout=1000")
    const bound = after === undefined ? undefined
      : db.prepare("SELECT position FROM message WHERE id=? ORDER BY position " + (ascending ? "ASC" : "DESC") + " LIMIT 1").get(after)
    if (after !== undefined && !bound) return { data: [] as Message[], more: false }
    const direction = ascending ? "ASC" : "DESC"
    const where = bound ? (ascending ? "WHERE position > ?" : "WHERE position < ?") : ""
    const args = bound ? [bound.position as number, limit + 1] : [limit + 1]
    const rows = db.prepare(`SELECT body FROM message ${where} ORDER BY position ${direction} LIMIT ?`).all(...args)
    return { data: rows.slice(0, limit).map(row => JSON.parse(row.body as string) as Message), more: rows.length > limit }
  } finally { db.close() }
}
