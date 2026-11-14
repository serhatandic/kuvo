import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"

// The Work Queue's items moving from categories to statuses on the disk a box keeps them on
// (PROTOCOL.md, "The Work Queue", "From categories"), as it goes on a Mac and on Linux alike: the
// copy made first, in one step; each queue's file written whole; the same end after a run cut off.
// (The scenarios with a client and agents: bridge-tests/work-queue-statuses.test.js.)

const { WORK_QUEUE, allItems, changeQueues } = await import("../src/link/work-queue/store")
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
