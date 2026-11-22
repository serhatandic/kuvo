// Walk the portable JSON array without decoding conversation bodies. Usage and artifacts consume
// only their fields; the bridge still uses the full reader for the conversation itself.
import { closeSync, openSync, readSync } from "node:fs"
import { StringDecoder } from "node:string_decoder"

type Pick = true | "file" | "citation" | "attachment" | { [key: string]: Pick } | readonly [Pick]
const model = { id: true, providerID: true } as const
const time = { created: true, completed: true, origin: true, usageAccount: true, usageSession: true } as const
const insight: Pick = {
  type: true, id: true, time, model, agent: true, metadata: { source: true },
  tokens: { input: true, output: true, reasoning: true, cache: { read: true, write: true } },
  content: [{ type: true, id: true, name: true, state: {
    status: true, input: { skill: true, name: true, file_path: true, filePath: true, path: true },
  } }],
}
const artifacts: Pick = {
  type: true, id: true, time, model, text: "attachment",
  files: [{ source: { type: true, uri: "file" }, uri: "file", url: "file" }],
  content: [{ type: true, id: true, name: true, text: "citation", state: {
    status: true, input: { path: true },
    content: [{ type: true, uri: "file" }],
  } }],
}

export function* projectedTranscript(file: string, view: "insight" | "artifacts"): Generator<unknown> {
  const descriptor = openSync(file, "r")
  try {
    const reader = new Reader(descriptor)
    reader.expect("[")
    yield* reader.elements("]", () => reader.value(view === "insight" ? insight : artifacts))
    if (reader.peek()) throw new SyntaxError("Unexpected text after the conversation.")
  } finally { closeSync(descriptor) }
}

class Reader {
  private readonly buffer = Buffer.allocUnsafe(64 * 1024)
  private readonly decoder = new StringDecoder("utf8")
  private readonly quotes = /["\\]/g
  private readonly delimiters = /["{}\[\]]/g
  private readonly whitespace = /\S/g
  private readonly primitive = /[\s,}\]]/g
  private chunk = ""
  private at = 0
  private ended = false

  constructor(private readonly fd: number) {}

  private more() {
    if (this.at < this.chunk.length) return true
    if (this.ended) return false
    const size = readSync(this.fd, this.buffer, 0, this.buffer.length, null)
    this.ended = !size
    this.chunk = size ? this.decoder.write(this.buffer.subarray(0, size)) : this.decoder.end()
    this.at = 0
    return this.chunk.length > 0
  }

  peek(): string | undefined {
    while (this.more()) {
      this.whitespace.lastIndex = this.at
      const token = this.whitespace.exec(this.chunk)
      if (token) {
        this.at = token.index
        return token[0]
      }
      this.at = this.chunk.length
    }
    return undefined
  }

  expect(char: string) {
    if (this.peek() !== char) throw new SyntaxError("Invalid saved conversation.")
    this.at++
  }

  *elements<Value>(end: string, read: () => Value): Generator<Value> {
    if (this.peek() !== end) for (;;) {
      yield read()
      if (this.peek() !== ",") break
      this.at++
    }
    this.expect(end)
  }

  /** Called just after an opening quote. Skip string escapes even when they cross the buffer. */
  private string(mode?: true | "file" | "citation" | "attachment") {
    if (mode === "file") {
      const first = this.chunk.slice(this.at, this.at + 5)
      if (first.length === 5 && first !== "file:" && !first.includes("\\")) mode = undefined
    }
    let parts: string[] | undefined
    if (mode) parts = []
    while (this.more()) {
      const start = this.at
      this.quotes.lastIndex = start
      let token: RegExpExecArray | null
      while ((token = this.quotes.exec(this.chunk))) {
        if (token[0] === '"') {
          if (parts) parts.push(this.chunk.slice(start, token.index))
          this.at = token.index + 1
          if (!parts) return undefined
          const text = JSON.parse('"' + parts.join("") + '"') as string
          return mode === "file" && !text.startsWith("file:") ||
            mode === "citation" && !text.includes(":codex-") ||
            mode === "attachment" && !text.includes("<attached-files>") ? undefined : text
        }
        if (this.quotes.lastIndex++ < this.chunk.length) continue
        if (parts) parts.push(this.chunk.slice(start))
        this.at = this.chunk.length
        if (this.more()) {
          if (parts) parts.push(this.chunk[0]!)
          this.at++
        }
        break
      }
      if (token) continue
      if (parts) parts.push(this.chunk.slice(start))
      this.at = this.chunk.length
    }
    throw new SyntaxError("Unfinished string in saved conversation.")
  }

  private skipCollection() {
    let depth = 1
    while (this.more()) {
      this.delimiters.lastIndex = this.at
      const token = this.delimiters.exec(this.chunk)
      if (!token) { this.at = this.chunk.length; continue }
      this.at = token.index + 1
      const char = token[0]
      if (char === '"') this.string()
      else if (char === "{" || char === "[") depth++
      else if (--depth === 0) return
    }
    throw new SyntaxError("Unfinished collection in saved conversation.")
  }

  value(pick?: Pick): unknown {
    const char = this.peek()
    if (!char) throw new SyntaxError("Unfinished saved conversation.")
    if (char === '"') {
      this.at++
      return this.string(typeof pick === "object" ? true : pick)
    }
    if (char === "[" || char === "{") {
      this.at++
      if (pick === undefined || typeof pick === "string") { this.skipCollection(); return undefined }
      if (char === "[") {
        const item = pick === true ? true : Array.isArray(pick) ? pick[0] : undefined
        if (!item) { this.skipCollection(); return undefined }
        return [...this.elements("]", () => this.value(item))]
      }
      const fields = pick === true ? undefined : pick as Record<string, Pick>
      const result = Object.create(null) as Record<string, unknown>
      for (const _ of this.elements("}", () => {
        this.expect('"')
        const key = this.string(true)!
        this.expect(":")
        const field = pick === true ? true : fields && Object.hasOwn(fields, key) ? fields[key] : undefined
        const value = this.value(field)
        if (value !== undefined) result[key] = value
      })) { /* Read the object's fields one at a time. */ }
      return result
    }
    let value = ""
    while (this.more()) {
      this.primitive.lastIndex = this.at
      const end = this.primitive.exec(this.chunk)?.index ?? this.chunk.length
      if (pick !== undefined) value += this.chunk.slice(this.at, end)
      this.at = end
      if (end < this.chunk.length) break
    }
    if (!value && pick !== undefined) throw new SyntaxError("Missing value in saved conversation.")
    return pick === undefined ? undefined : JSON.parse(value)
  }
}
