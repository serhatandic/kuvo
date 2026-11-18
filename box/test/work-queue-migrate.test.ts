import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"

// The Work Queue's items moving from categories to statuses on the disk a box keeps them on
// (PROTOCOL.md, "The Work Queue", "From categories"), as it goes on a Mac and on Linux alike: the
// copy made first, in one step; each queue's file written whole; the same end after a run cut off.
// (The scenarios with a client and agents: bridge-tests/work-queue-statuses.test.js.)

const { WORK_QUEUE, allItems, changeQueues, readQueueSettingAside, Refusal } = await import("../src/link/work-queue/store")
const { addLabel } = await import("../src/link/work-queue/labels")
const { forgetMigrated } = await import("../src/link/work-queue/migrate")
const { labels } = await import("../src/link/work-queue/labels")

const before = path.join(WORK_QUEUE, "before-statuses")
const fresh = () => {
  fs.rmSync(WORK_QUEUE, { recursive: true, force: true })
  forgetMigrated()
}
beforeEach(fresh)
afterAll(fresh)

const item = (n: number, category: string, more: Record<string, unknown> = {}) => ({
  id: `wqi_${String(n).padStart(20, "0")}`, number: n, owner: "ses_a", title: `Item ${n}`, body: `Body ${n}`, category, order: n,
  notes: [{ at: n, by: { user: true }, text: `Note ${n}` }], files: [], links: [{ url: `https://example.com/${n}` }], sessions: [],
  created: { at: n, by: { user: true } }, updated: { at: n, by: { user: true } }, ...more,
})
function keep(): Record<string, string> {
  const write = (name: string, value: unknown) => {
    fs.mkdirSync(path.dirname(path.join(WORK_QUEUE, name)), { recursive: true })
    fs.writeFileSync(path.join(WORK_QUEUE, name), JSON.stringify(value))
    return [name, JSON.stringify(value)] as const
  }
  return Object.fromEntries([
    write("queues/ses_a.json", { owner: "ses_a", items: [item(1, "next"), item(2, "later"), item(3, "parked"), item(4, "done", { doneAt: 9 }), item(5, "cat_tests-tooling"), item(6, "cat_waiting-on-you")] }),
    write("queues/ses_b.json", { owner: "ses_b", items: [item(7, "cat_edge-cases", { owner: "ses_b" }), item(8, "next", { owner: "ses_b", order: 0 })] }),
    write("categories.json", { custom: [{ id: "cat_tests-tooling", name: "Tests & tooling" }, { id: "cat_edge-cases", name: "Edge cases" }, { id: "cat_waiting-on-you", name: "Waiting on you" }] }),
  ])
}
const stands = () => Object.fromEntries(allItems().map(each => [each.number, [each.status, each.labels, each.order]]))
const END = { 1: ["todo", [], 1], 2: ["backlog", [], 0], 3: ["backlog", ["lbl_parked"], 2], 4: ["done", [], 0], 5: ["todo", ["cat_tests-tooling"], 2], 6: ["needs-you", [], 0], 7: ["backlog", ["cat_edge-cases"], 1], 8: ["todo", [], 0] }

test("what was kept is copied whole before anything is written, and every item keeps what it held", () => {
  const kept = keep()
  expect(stands()).toEqual(END)
  for (const [name, text] of Object.entries(kept)) expect(fs.readFileSync(path.join(before, name), "utf8")).toBe(text)
  for (const each of allItems()) {
    expect([each.title, each.body, each.notes[0].text, each.links[0].url]).toEqual([`Item ${each.number}`, `Body ${each.number}`, `Note ${each.number}`, `https://example.com/${each.number}`])
  }
  expect(labels().map(each => each.name)).toEqual(["Tests & tooling", "Edge cases", "Parked"])
  expect(JSON.parse(fs.readFileSync(path.join(WORK_QUEUE, "model.json"), "utf8")).model).toBe(2)
  expect(JSON.parse(fs.readFileSync(path.join(WORK_QUEUE, "queues", "ses_a.json"), "utf8")).formatVersion).toBe(1)
  expect(fs.existsSync(path.join(WORK_QUEUE, "categories.json"))).toBe(false)
  // (Only its own folders and files there: nothing left of the copy's making.)
  expect(fs.readdirSync(WORK_QUEUE).filter(name => !name.startsWith(".")).sort()).toEqual(["before-statuses", "labels.json", "model.json", "queues"])
})

test("a run cut off after the copy, one queue written, ends as an uninterrupted one; a change made first isn't lost to it", () => {
  keep()
  expect(stands()).toEqual(END)
  const whole = fs.readFileSync(path.join(WORK_QUEUE, "queues", "ses_b.json"), "utf8")
  const copy = fs.mkdtempSync(path.join(path.dirname(WORK_QUEUE), "copy-"))
  try {
    fs.cpSync(before, copy, { recursive: true })
    fresh()
    keep()
    fs.cpSync(copy, before, { recursive: true })
    fs.writeFileSync(path.join(WORK_QUEUE, "queues", "ses_b.json"), whole)
    // (A part of a copy left by a run cut off sooner still lies beside it.)
    fs.mkdirSync(path.join(WORK_QUEUE, "before-statuses.00ff00ff00ff", "queues"), { recursive: true })
    expect(stands()).toEqual(END)
    expect(fs.readFileSync(path.join(WORK_QUEUE, "queues", "ses_b.json"), "utf8")).toBe(whole)
    // The first change after it is kept with statuses, and nothing is moved over twice.
    changeQueues(book => { book.find({ number: 1 })!.status = "in-review" })
    forgetMigrated()
    expect(stands()[1]).toEqual(["in-review", [], 1])
    expect(JSON.parse(fs.readFileSync(path.join(before, "queues", "ses_a.json"), "utf8")).items[0].category).toBe("next")
  } finally {
    fs.rmSync(copy, { recursive: true, force: true })
  }
})

test("where the copy can't be made, items still read (each said anew) and nothing is changed until it can", () => {
  const kept = keep()
  // (The folder can't be written in: a full disk does the same to the copy.)
  fs.chmodSync(WORK_QUEUE, 0o500)
  try {
    expect(Object.fromEntries(allItems().map(each => [each.number, each.status]))).toEqual({ 1: "todo", 2: "backlog", 3: "backlog", 4: "done", 5: "todo", 6: "needs-you", 7: "backlog", 8: "todo" })
    expect(() => changeQueues(book => { book.find({ number: 1 })!.title = "Changed" })).toThrow(Refusal)
    expect(() => changeQueues(() => {})).toThrow(/couldn't be moved over to statuses on this computer .*so nothing was changed/)
    expect(addLabel("Bug")).toEqual({ refused: expect.stringContaining("couldn't be moved over to statuses") })
    for (const [name, text] of Object.entries(kept)) expect(fs.readFileSync(path.join(WORK_QUEUE, name), "utf8")).toBe(text)
    expect(fs.existsSync(path.join(WORK_QUEUE, "model.json"))).toBe(false)
  } finally {
    fs.chmodSync(WORK_QUEUE, 0o700)
  }
  // Once it can: moved over as ever, by their categories' names.
  expect(stands()).toEqual(END)
  expect(labels().map(each => each.name)).toEqual(["Tests & tooling", "Edge cases", "Parked"])
})


test("a future owner file stays in place before and after the status migration", () => {
  keep()
  const file = path.join(WORK_QUEUE, "queues", "ses_a.json")
  const future = JSON.stringify({ formatVersion: 2, work: [{ id: "future" }] })
  fs.writeFileSync(file, future)
  expect(() => allItems()).toThrow(/Work Queue owner file format version 2 is unsupported/)
  expect(fs.existsSync(before)).toBe(false)
  expect(fs.readFileSync(file, "utf8")).toBe(future)

  fs.writeFileSync(path.join(WORK_QUEUE, "model.json"), JSON.stringify({ model: 2 }))
  expect(() => readQueueSettingAside("ses_a")).toThrow(/Work Queue owner file format version 2 is unsupported/)
  expect(() => changeQueues(() => {})).toThrow(/Work Queue owner file format version 2 is unsupported/)
  expect(fs.readFileSync(file, "utf8")).toBe(future)
})

test("legacy labels keep their order and ownership when written with a version", () => {
  keep()
  fs.writeFileSync(path.join(WORK_QUEUE, "model.json"), JSON.stringify({ model: 2 }))
  const file = path.join(WORK_QUEUE, "labels.json")
  const old = { labels: [{ id: "lbl_existing", name: "Existing", color: "sky" }], fromPrimary: ["lbl_existing"], primary: "peer" }
  fs.writeFileSync(file, JSON.stringify(old))
  expect(labels()).toEqual(old.labels)
  expect(addLabel("New", "mint")).toEqual({ done: { id: "lbl_new", name: "New", color: "mint" } })
  expect(JSON.parse(fs.readFileSync(file, "utf8"))).toEqual({
    ...old,
    labels: [...old.labels, { id: "lbl_new", name: "New", color: "mint" }],
    formatVersion: 1,
  })
})

test("future labels and status models survive an older box attempting an edit", () => {
  const original = keep()
  const file = path.join(WORK_QUEUE, "labels.json")
  const future = JSON.stringify({ formatVersion: 2, labels: [], additional: ["keep me"] })
  fs.writeFileSync(file, future)
  expect(() => labels()).toThrow(/Work Queue labels format version 2/)
  expect(() => addLabel("New", "mint")).toThrow(/Work Queue labels format version 2/)
  expect(fs.readFileSync(file, "utf8")).toBe(future)
  for (const [name, text] of Object.entries(original)) expect(fs.readFileSync(path.join(WORK_QUEUE, name), "utf8")).toBe(text)

  const model = path.join(WORK_QUEUE, "model.json")
  const futureModel = JSON.stringify({ formatVersion: 1, model: 3 })
  fs.writeFileSync(model, futureModel)
  expect(() => allItems()).toThrow(/Work Queue status model format version 3/)
  expect(fs.readFileSync(model, "utf8")).toBe(futureModel)
})

test("peer summaries adopt legacy fields and retain a future file even when the peer is removed", async () => {
  const { TOLD, forgetTold, keepTold, readTold } = await import("../src/link/work-queue/told")
  const peer = "a".repeat(43)
  const file = path.join(TOLD, `${peer}.json`)
  const summary = { id: "wqt_abcdefgh", at: 7, items: [], owners: [], labels: [], kind: "mac" as const }
  fs.mkdirSync(TOLD, { recursive: true })
  fs.writeFileSync(file, JSON.stringify({ ...summary, heard: 8 }))
  expect(readTold(peer)).toEqual({ ...summary, heard: 8 })
  expect(keepTold(peer, summary, 9)).toBe(true)
  expect(JSON.parse(fs.readFileSync(file, "utf8"))).toEqual({ ...summary, heard: 9, formatVersion: 1 })
  const future = JSON.stringify({ ...summary, formatVersion: 2 })
  fs.writeFileSync(file, future)
  expect(() => keepTold(peer, summary, 10)).toThrow(/Work Queue peer summary format version 2/)
  expect(() => forgetTold(peer)).toThrow(/Work Queue peer summary format version 2/)
  expect(fs.readFileSync(file, "utf8")).toBe(future)
})
