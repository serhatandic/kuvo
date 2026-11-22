// Sessions and their conversations, kept in the bridge's own folder: one folder per session with
// `session.json` (the session as the app reads it), the transcript, and `agent.json` (what the
// agent needs to go on, including its own conversation's ID).

import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { writeDurablePrivate } from "../shared/files"
import { additiveObjectFormat, UnsupportedFormatVersion } from "../shared/format-version"
import type { Attachment, Message, Session, ToolOutput } from "./wire"
import { exportTranscript, messageFileTime, migrateTranscript, retainAttachment, retainOutput, savedMessages, savedPage, transcriptDatabase, writeTranscript, type MessageView } from "./transcript"

const sessionFormat = additiveObjectFormat("Coding-agent session")
export const readSession = (value: unknown) => sessionFormat.read(value) as unknown as Session

export interface Record_ {
  session: Session
  messages: Message[]
  /** The agent's own state for this session (its conversation's ID, to resume it). */
  agent: Record<string, unknown>
}

export class Store {
  /** Set by the bridge for messages created on this computer. */
  origin?: () => string
  prepare?: (id: string) => void
  private prepared = new Set<string>()
  usageAccount?: (provider: string, origin: string) => string | undefined
  private records = new Map<string, Record_>()
  private loaded = new Map<string, { messages: Message[]; order: Message[]; dirty: Set<Message> }>()
  private pending = new Map<string, ReturnType<typeof setTimeout>>()

  private deferred = new Set<string>()

  constructor(private readonly root: string, private provisional = false, private readonly attachments?: string) {
    mkdirSync(root, { recursive: true })
    for (const { session, agent, folder } of readSavedSessionHeaders(root)) {
      this.keep(session, agent, folder)
    }
  }

  private keep(session: Session, agent: Record<string, unknown>, folder: string, initial?: Message[]) {
    const id = session.id
    const load = () => {
      const known = this.loaded.get(id)
      if (known) {
        return known
      }
      if (!this.provisional) migrateTranscript(folder, this.attachments, id)
      const messages = [...savedMessages(folder)]
      const loaded = { messages, order: [...messages], dirty: new Set<Message>() }
      this.loaded.set(id, loaded)
      if (!this.prepared.has(id)) {
        this.prepared.add(id)
        this.prepare?.(id)
      }
      return loaded
    }
    const record: Record_ = {
      session, agent,
      get messages() { return load().messages },
      set messages(messages) { load().messages = messages },
    }
    if (initial) {
      this.prepared.add(id)
      this.loaded.set(id, { messages: initial, order: [], dirty: new Set() })
    }
    this.records.set(id, record)
    return record
  }

  /** Evict idle, persisted conversations at a point where no request can mutate them. */
  prune(busy: (id: string) => boolean) {
    for (const id of this.loaded.keys()) {
      if (this.provisional || this.pending.has(id) || busy(id)) continue
      this.loaded.delete(id)
    }
  }

  keepAttachment(id: string, file: Attachment) {
    return retainAttachment(this.provisional ? undefined : this.attachments, id, file)
  }

  keepOutput(id: string, output: ToolOutput) {
    return retainOutput(this.provisional ? undefined : this.attachments, id, output)
  }

  all(): Session[] {
    return [...this.records.values()].map((record) => record.session)
  }

  get(id: string): Record_ | undefined {
    return this.records.get(id)
  }

  /** Where one of a session's files is kept (`session.json`, `messages.json`, `agent.json`). */
  fileOf(id: string, name: string): string {
    return join(this.root, id, name)
  }

  /** A session that came whole from another computer. */
  addWhole(record: Record_) {
    this.checkWritable(record.session.id)
    this.keep(record.session, record.agent, join(this.root, record.session.id), record.messages)
    this.save(record.session.id, true)
  }

  /** When its conversation was last written (ms), if it has been. */
  writtenAt(id: string): number | undefined {
    return messageFileTime(join(this.root, id))
  }

  add(session: Session) {
    this.checkWritable(session.id)
    this.keep(session, {}, join(this.root, session.id), [])
    this.save(session.id, true)
  }

  remove(id: string) {
    this.checkWritable(id)
    this.records.delete(id)
    this.loaded.delete(id)
    this.prepared.delete(id)
    clearTimeout(this.pending.get(id))
    this.pending.delete(id)
    rmSync(join(this.root, id), { recursive: true, force: true })
  }

  /** Writes a session's files: at once, or a moment after its last change (a reply streams in). */
  save(id: string, now = false) {
    const loaded = this.loaded.get(id)
    if (loaded) for (const message of loaded.messages) loaded.dirty.add(message)
    this.schedule(id, now)
  }

  /** A streamed checkpoint appends new rows and updates only the messages it touched. */
  progress(id: string, message?: Message, now = false) {
    if (message) this.loaded.get(id)?.dirty.add(message)
    this.schedule(id, now)
  }

  private schedule(id: string, now: boolean) {
    if (this.provisional) {
      this.deferred.add(id)
      return
    }
    clearTimeout(this.pending.get(id))
    if (now) {
      this.pending.delete(id)
      this.write(id)
      return
    }
    this.pending.set(
      id,
      setTimeout(() => {
        this.pending.delete(id)
        this.write(id)
      }, 500),
    )
  }

  commit() {
    this.provisional = false
    for (const id of this.deferred) this.save(id)
    this.deferred.clear()
  }

  /** Everything still waiting to be written, written. */
  flush() {
    for (const id of [...this.pending.keys()]) this.schedule(id, true)
  }

  /** A bridge can give up its state only once every pending write succeeds. */
  flushForTransfer(active: string[]) {
    for (const id of new Set([...this.pending.keys(), ...active])) {
      const record = this.records.get(id)
      if (record) this.writeRecord(id, record, true)
      clearTimeout(this.pending.get(id))
      this.pending.delete(id)
    }
  }

  /** The queue handoff keeps its backup until the bridge has sealed the accepted input. */
  sealPrompt(id: string, record: Record_) {
    this.writeRecord(id, record, true)
  }

  private checkWritable(id: string) {
    try {
      readSession(JSON.parse(readFileSync(this.fileOf(id, "session.json"), "utf8")))
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error
    }
  }

  private writeRecord(id: string, record: Record_, durable = false) {
    this.checkWritable(id)
    const folder = join(this.root, id)
    mkdirSync(folder, { recursive: true })
    const loaded = this.loaded.get(id)
    if (loaded) {
      const initial = !existsSync(join(folder, transcriptDatabase))
      const { messages, order, dirty } = loaded
      function* changed(): Generator<[number, Message]> {
        for (const [index, message] of messages.entries()) {
          if (initial || order[index] !== message || dirty.has(message)) yield [index, message]
        }
      }
      writeTranscript(folder, changed(), messages.length, this.attachments, id, durable)
      loaded.order = [...messages]
      dirty.clear()
    }
    for (const [name, value] of [["session.json", sessionFormat.write(record.session)], ["agent.json", record.agent]] as const) {
      const file = join(folder, name)
      const text = JSON.stringify(value)
      if (durable) writeDurablePrivate(file, text, this.root)
      else {
        writeFileSync(file + ".new", text)
        renameSync(file + ".new", file)
      }
    }
  }

  page(id: string, after: string | undefined, limit: number, ascending: boolean) {
    if (this.loaded.has(id) || !this.prepared.has(id)) return undefined
    return savedPage(join(this.root, id), after, limit, ascending)
  }

  /** The existing portable move format is independent of the bridge's local checkpoints. */
  saved(id: string) {
    return savedMessages(join(this.root, id))
  }

  exportMessages(id: string, file: string, observe?: (text: string) => void) {
    exportTranscript(file, this.saved(id), observe)
  }

  private write(id: string) {
    const record = this.records.get(id)
    if (!record) return
    try {
      this.writeRecord(id, record)
    } catch (error) {
      // Not written now (a full disk, say): the session goes on as it is, and it's tried again in a
      // while, or as everything waiting is written.
      console.error(`session ${id} not written:`, error)
      if (!(error instanceof UnsupportedFormatVersion) && !this.pending.has(id)) this.pending.set(id, setTimeout(() => {
        this.pending.delete(id)
        this.write(id)
      }, 5_000))
    }
  }
}

/** Session metadata is available without opening its conversation. */
export function* readSavedSessionHeaders(root: string) {
  let entries: string[]
  try { entries = readdirSync(root) } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return
    throw error
  }
  for (const id of entries) {
    try {
      const folder = join(root, id)
      const session = readSession(JSON.parse(readFileSync(join(folder, "session.json"), "utf8")))
      let agent: Record<string, unknown> = {}
      try { agent = JSON.parse(readFileSync(join(folder, "agent.json"), "utf8")) } catch {}
      yield { folder, session, agent }
    } catch {
      // A folder left half-written: not a session.
    }
  }
}

/** Consumers read a transcript one message at a time, only after selecting its metadata. */
export function* readSavedSessions(root: string, view: MessageView = "body") {
  for (const { folder, session, agent } of readSavedSessionHeaders(root)) {
    function* messages() {
      try { yield* savedMessages(folder, view) }
      catch (error) { console.error("Unable to read saved conversation", session.id, error) }
    }
    yield { session, messages: messages(), agent }
  }
}
