import "../safety"
import { expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { Store } from "../../src/agents/store"
import { noTokens, type Session } from "../../src/agents/wire"
import { checkFolderVersion } from "../../src/shared/folder-version"

test("legacy sessions acquire the additive stamp on write, and future sessions stay intact", () => {
  const root = mkdtempSync(join(process.env.HOME!, "store-version-"))
  try {
    const session: Session = { id: "ses_a", projectID: "project", time: { created: 1, updated: 1 }, location: { directory: root }, cost: 0, tokens: noTokens() }
    const store = new Store(root)
    const file = store.fileOf(session.id, "session.json")
    mkdirSync(join(root, session.id))
    writeFileSync(file, JSON.stringify(session))
    const current = new Store(root)
    expect(current.get(session.id)?.session).toEqual(session)
    current.save(session.id, true)
    expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ ...session, formatVersion: 1 })

    const future = JSON.stringify({ ...session, formatVersion: 2, extra: "keep this" })
    writeFileSync(file, future)
    expect(new Store(root).get(session.id)).toBeUndefined()
    expect(() => current.addWhole({ session, messages: [], agent: {} })).toThrow("format version 2")
    expect(() => current.remove(session.id)).toThrow("format version 2")
    expect(readFileSync(file, "utf8")).toBe(future)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("a handover can check a future folder layout without writing to it", () => {
  const root = mkdtempSync(join(process.env.HOME!, "folder-version-"))
  try {
    const file = join(root, "folder-version.json")
    const future = '{"version":22,"upgradingTo":23}\n'
    writeFileSync(file, future)
    expect(() => checkFolderVersion(root)).toThrow("format version 23")
    expect(readFileSync(file, "utf8")).toBe(future)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
