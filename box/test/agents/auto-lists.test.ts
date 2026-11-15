import "../safety"
import { expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Auto } from "../../src/agents/auto"

test("two clients keep different lists; stale changes to one list are refused, including after reopening", () => {
  const root = mkdtempSync(join(tmpdir(), "auto-lists-"))
  try {
    const auto = new Auto(root)
    expect(auto.change("bugs", undefined, { name: "Bugs", entries: [] }, true)).toBe(true)
    const [first, bugs] = auto.all()
    expect(auto.change(first.id, first.revision, { name: "Everyday" })).toBe(true)
    expect(auto.change(bugs.id, bugs.revision, { entries: [{ providerID: "codex", modelID: "gpt" }] })).toBe(true)
    expect(auto.change(first.id, first.revision, { name: "Stale" })).toBe(false)
    expect(auto.change(bugs.id, bugs.revision, { entries: [] })).toBe(false)
    expect(auto.all().map(list => list.name)).toEqual(["Everyday", "Bugs"])
    expect(new Auto(root).all()).toEqual(auto.all())
    const was = auto.all().map(list => list.id)
    expect(auto.reorder([...was].reverse(), was)).toBe(true)
    expect(auto.reorder(was, was)).toBe(false)
    expect(auto.all()[0].entries).toEqual([{ providerID: "codex", modelID: "gpt" }])
    expect(auto.change("bugs", undefined, { name: "Duplicate" }, true)).toBe(false)
  } finally { rmSync(root, { recursive: true, force: true }) }
})

import { handler } from "../../src/agents/server"
import { Sessions } from "../../src/agents/sessions"
import { Store } from "../../src/agents/store"
import type { Roster } from "../../src/agents/roster"

test("Auto routes add, rename, set contents, order and refuse stale client writes", async () => {
  const root = mkdtempSync(join(tmpdir(), "auto-routes-"))
  try {
    const auto = new Auto(root)
    const roster = { find: () => undefined } as unknown as Roster
    const sessions = new Sessions(new Store(join(root, "sessions")), roster, auto)
    const answer = handler({ auto, sessions, roster, password: "pw" } as any)
    const ask = (method: string, tail = "", body?: unknown) => answer(new Request("http://box/api/kuvo/auto" + tail, {
      method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    }))
    expect((await ask("POST", "", { id: "bugs", name: "Bugs", entries: [] })).status).toBe(200)
    const read = await (await ask("GET")).json() as any
    const [first, bugs] = read.data.lists
    expect((await ask("PUT", "/auto", { revision: first.revision, name: "Everyday" })).status).toBe(200)
    expect((await ask("PUT", "/bugs", { revision: bugs.revision, entries: [{ providerID: "codex", modelID: "gpt" }] })).status).toBe(200)
    expect((await ask("PUT", "/bugs", { revision: bugs.revision, entries: [] })).status).toBe(409)
    expect((await ask("PUT", "/auto", { name: "Missing revision" })).status).toBe(409)
    expect((await ask("PUT", "/order", { ids: ["bugs", "auto"], was: ["auto", "bugs"] })).status).toBe(200)
    expect((await ask("PUT", "/order", { ids: ["auto", "bugs"], was: ["auto", "bugs"] })).status).toBe(409)
    expect((await ask("DELETE", "/order", { ids: ["auto", "bugs"], was: ["bugs", "auto"] })).status).toBe(404)
    expect((await ask("PUT", "/bugs/extra", { revision: auto.all()[0].revision, name: "Wrong path" })).status).toBe(404)
    expect((await ask("PUT", "", { lists: [] })).status).toBe(404)
    const lists = (await (await ask("GET")).json() as any).data.lists
    expect(lists.map((list: any) => list.name)).toEqual(["Bugs", "Everyday"])
    expect(lists[0].entries).toEqual([{ providerID: "codex", modelID: "gpt" }])
    expect((await ask("DELETE", "/bugs", { revision: lists[0].revision })).status).toBe(200)
    expect((await ask("DELETE", "/auto", { revision: lists[1].revision })).status).toBe(400)
  } finally { rmSync(root, { recursive: true, force: true }) }
})

test("an edit that waits for an agent's model alias refuses a revision changed while waiting", async () => {
  const root = mkdtempSync(join(tmpdir(), "auto-alias-"))
  try {
    const auto = new Auto(root)
    auto.set([{ providerID: "agent", modelID: "default" }])
    let entered!: () => void
    let release!: (id: string) => void
    const waiting = new Promise<void>(resolve => { entered = resolve })
    const resolved = new Promise<string>(resolve => { release = resolve })
    const roster = { find: () => ({ concreteModel: () => { entered(); return resolved } }) } as unknown as Roster
    const sessions = new Sessions(new Store(join(root, "sessions")), roster, auto)
    const answer = handler({ auto, sessions, roster, password: "pw" } as any)
    const before = auto.all()[0]
    const save = answer(new Request("http://box/api/kuvo/auto/auto", {
      method: "PUT", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" },
      body: JSON.stringify({ revision: before.revision, entries: [{ providerID: "agent", modelID: "concrete" }] }),
    }))
    await waiting
    expect(auto.change("auto", before.revision, { name: "Changed while resolving" })).toBe(true)
    release("concrete")
    expect((await save).status).toBe(409)
    expect(auto.all()[0].name).toBe("Changed while resolving")
    expect(auto.all()[0].entries).toEqual([{ providerID: "agent", modelID: "default" }])
  } finally { rmSync(root, { recursive: true, force: true }) }
})
