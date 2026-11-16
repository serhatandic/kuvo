import "./safety"
import { beforeEach, expect, test } from "bun:test"
import { mkdirSync, writeFileSync, rmSync, readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { ask, follow } from "./route-answer"
const { engineHandler } = await import("../src/link/server/router")
const { CONFIG_FILE } = await import("../src/link/areas/config")
const { SKILLS_FOLDER } = await import("../src/link/areas/skills")
const { skillDocument } = await import("../src/link/areas/settings-skills")
const handler = engineHandler((() => { throw Error("offline") }) as any, { name: "test" }, (() => null) as any)
const rows = (page: any) => page.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])]))
const page = async () => (await ask(handler, "GET", "kuvo/settings/skills")).json
beforeEach(() => {
  mkdirSync(dirname(CONFIG_FILE), { recursive: true })
  writeFileSync(CONFIG_FILE, "{}")
  rmSync(SKILLS_FOLDER, { recursive: true, force: true })
  mkdirSync(join(SKILLS_FOLDER, "release"), { recursive: true })
  writeFileSync(join(SKILLS_FOLDER, "release", "SKILL.md"), skillDocument("release", "Write releases", "Instructions"))
})
test("Skills lists editable skills and the Swift editor words", async () => {
  const shown = await page()
  const skill = rows(shown).find((row: any) => row.id === "skill-release")
  expect(skill.value).toBe(true)
  expect(skill.actions[0].form.fields.map((field: any) => field.title)).toEqual(["Use it when", "Instructions"])
  expect(shown.groups[0].title).toBe("Skills the Agent Can Use")
  expect(shown.groups.find((group: any) => group.id === "other-places").note).toBe("A catalog is a web address with an index.json listing its skills.")
})
test("check and refused submit share name warnings and do not write", async () => {
  const at = rows(await page()).find((row: any) => row.id === "skills")
  const body = { was: at.add.form.revision, values: { name: "release", description: "Test", instructions: "Text" } }
  const check = await ask(handler, "POST", "kuvo/settings/skills/skills/add/check", body)
  expect(check.json.ready).toBe(false)
  const submit = await ask(handler, "POST", "kuvo/settings/skills/skills/add", body)
  expect(submit.status).toBe(400)
  expect(submit.json.fields).toEqual(check.json.fields)
})
test("edit preserves unknown header keys and rejects stale forms", async () => {
  const at = rows(await page()).find((row: any) => row.id === "skill-release")
  const body = { was: at.actions[0].form.revision, values: { description: "Changed", instructions: "New" } }
  writeFileSync(join(SKILLS_FOLDER, "release", "SKILL.md"), skillDocument("release", "Outside", "Instructions"))
  expect((await ask(handler, "POST", "kuvo/settings/skills/skill-release/edit", body)).status).toBe(409)
  expect(skillDocument("a", "b", "c", "---\nname: a\ndescription: old\nlicense: MIT\n---\nold")).toContain("license: MIT")
})
test("wrong switch type is 400 and unrelated config change does not stale the skill", async () => {
  const at = rows(await page()).find((row: any) => row.id === "skill-release")
  writeFileSync(CONFIG_FILE, JSON.stringify({ unrelated: true }))
  expect((await ask(handler, "PATCH", "kuvo/settings/skills", { row: at.id, was: at.revision, value: "no" })).status).toBe(400)
  expect((await ask(handler, "PATCH", "kuvo/settings/skills", { row: at.id, was: at.revision, value: false })).status).toBe(200)
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).unrelated).toBe(true)
})
test("held permissions refuse changes without touching the file", async () => {
  writeFileSync(CONFIG_FILE, JSON.stringify({ permissions: "handwritten" }))
  const before = readFileSync(CONFIG_FILE, "utf8")
  const at = rows(await page()).find((row: any) => row.id === "skill-release")
  expect(at.can.why).toBeTruthy()
  expect((await ask(handler, "PATCH", "kuvo/settings/skills", { row: at.id, was: at.revision, value: false })).status).toBe(403)
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(before)
})
test("another skill's change does not stale this editor, rewording keeps switch revision", async () => {
  const at = rows(await page()).find((row: any) => row.id === "skill-release")
  mkdirSync(join(SKILLS_FOLDER, "other"), { recursive: true })
  writeFileSync(join(SKILLS_FOLDER, "other", "SKILL.md"), skillDocument("other", "Other", "Other"))
  const next = rows(await page()).find((row: any) => row.id === at.id)
  expect(next.actions[0].form.revision).toBe(at.actions[0].form.revision)
  expect((await ask(handler, "POST", "kuvo/settings/skills/skill-release/edit", { was: at.actions[0].form.revision, values: { description: "Changed", instructions: "New instructions" } })).status).toBe(200)
})
test("legacy skill writes reject a changed SKILL.md via was", async () => {
  const result = await ask(handler, "PUT", "kuvo/skills/release", { was: "old", text: "new" })
  expect(result.status).toBe(409)
  expect(result.json.message).toBe("It changed since you read it.")
})

test("events follow authored files, permissions and other places", async () => {
  const lines = follow(handler, "kuvo/settings/skills/events")
  try {
    expect(rows(await lines.next()).some((row: any) => row.id === "skill-release")).toBe(true)
    writeFileSync(join(SKILLS_FOLDER, "release", "SKILL.md"), skillDocument("release", "Changed externally", "New"))
    expect(rows(await lines.next()).find((row: any) => row.id === "skill-release").line).toBe("Changed externally")
    writeFileSync(CONFIG_FILE, JSON.stringify({ permissions: [{ action: "skill", resource: "release", effect: "deny" }], skills: ["/example"] }))
    const next = rows(await lines.next())
    expect(next.find((row: any) => row.id === "skill-release").value).toBe(false)
    expect(next.find((row: any) => row.id === "place-0").title).toBe("/example")
  } finally { lines.stop() }
})
