import "./safety"
import { expect, spyOn, test } from "bun:test"
import fs from "node:fs"
import { join } from "node:path"
import { reapplyApprovals } from "../src/link/areas/approval"
import { approvalRules } from "../src/link/work/approval"
import { FULL_ACCESS_FOLDER, markedFullAccess, markFullAccess } from "../src/link/work/full"

// The box reconciles real launcher markers after restart. Only the engine's HTTP API is a stand-in.
test("the box revokes old Full Access for a page of conversations in one pass over the launcher markers", async () => {
  const suffix = crypto.randomUUID().replaceAll("-", "")
  const made = Array.from({ length: 70 }, (_, index) => `ses_approval_round_${suffix}_${index}`)
  const children = made.map(id => id + "_child")
  const unrelated = `ses_unrelated_${suffix}`
  const ids = [...made, ...children, unrelated]
  for (const [index, child] of children.entries()) markFullAccess(child, true, made[index]!)
  // A child can still have a marker even if its root marker went away.
  markFullAccess(made[0]!, true)
  markFullAccess(unrelated, true)
  const data = made.map(id => ({
    id, title: "Check changes", location: { directory: "/project" },
    metadata: { kuvo: { approval: "askFirst", applied: "askFirst" } },
    permissions: approvalRules("askFirst", []),
  }))
  const bridge = Bun.serve({
    port: 0, hostname: "127.0.0.1",
    fetch(request) {
      if (new URL(request.url).pathname === "/api/session") return Response.json({ data })
      return new Response(null, { status: 404 })
    },
  })
  const read = spyOn(fs, "readdirSync")
  try {
    expect(await reapplyApprovals(null, { port: bridge.port!, password: "test" })).toBe(true)
    expect([...made, ...children].filter(id => markedFullAccess(id))).toEqual([])
    expect(markedFullAccess(unrelated)).toBe(true)
    expect(read.mock.calls.filter(([folder]) => folder === FULL_ACCESS_FOLDER)).toHaveLength(1)
  } finally {
    read.mockRestore()
    bridge.stop(true)
    for (const id of ids) fs.rmSync(join(FULL_ACCESS_FOLDER, id), { force: true })
  }
})
