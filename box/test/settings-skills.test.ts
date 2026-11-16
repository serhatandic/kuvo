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

test("GitHub install links share check and submit parsing", async () => {
  const { skillInstall } = await import("../src/link/areas/settings-skills")
  expect(skillInstall({ repository: "https://github.com/acme/skills", path: "skills/release" })).toMatchObject({ ready: true, repository: "acme/skills", path: "skills/release" })
  expect(skillInstall({ repository: "https://example.com/acme/skills", path: "" }).ready).toBe(false)
  expect(skillInstall({ repository: "https://github.com/acme/skills/tree/reviewed/release", path: "" }).ready).toBe(false)
  const values = { repository: "not-a-repository", path: "" }
  const at = rows(await page()).find((row: any) => row.id === "install")
  const checked = await ask(handler, "POST", "kuvo/settings/skills/install/install/check", { values })
  const refused = await ask(handler, "POST", "kuvo/settings/skills/install/install", { was: at.actions[0].form.revision, values })
  expect(refused.status).toBe(400)
  expect(refused.json.fields).toEqual(checked.json.fields)
})
test("quoted descriptions round trip and malformed Other Places stays untouched", async () => {
  const { skillAbout } = await import("../src/link/areas/skills")
  const description = 'Use "foo" with C:\\scripts and apostrophes'
  expect(skillAbout(skillDocument("quoted", description, "Text")).description).toBe(description)
  writeFileSync(CONFIG_FILE, JSON.stringify({ skills: "handwritten" }))
  const before = readFileSync(CONFIG_FILE, "utf8")
  const shown = await page()
  expect(shown.groups).toEqual([])
  expect(shown.notes[0].tone).toBe("warning")
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(before)
})

test("discovered skills keep metadata and both lists offer the existing search control", async () => {
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch() { return Response.json({ data: Array.from({ length: 7 }, (_, index) => ({ id: `found-${index}`, name: `Found ${index}`, description: `Use found ${index}`, path: `/builtin/found-${index}/SKILL.md` })) }) } })
  const discovered = engineHandler((() => ({ port: server.port, password: "test" })) as any, { name: "test" }, (() => null) as any)
  try {
    const shown = (await ask(discovered, "GET", "kuvo/settings/skills")).json
    const list = rows(shown).find((row:any) => row.id === "skills")
    expect(list.search.placeholder).toBe("Search skills")
    expect(list.items.find((row:any) => row.id === "skill-found-0")).toMatchObject({ title: "Found 0", line: "Use found 0", badge: { words: "Built in", tone: "quiet" } })
    for (let index = 0; index < 7; index++) {
      const name = `deleted-${index}`
      expect((await ask(handler, "PUT", `kuvo/skills/${name}`, { text: skillDocument(name, "For testing", "Text"), new: true })).status).toBe(200)
      expect((await ask(handler, "DELETE", `kuvo/skills/${name}`)).status).toBe(200)
    }
    expect(rows(await page()).find((row:any) => row.id === "deleted").search).toEqual({ placeholder: "Search recently deleted", empty: "No deleted skill matches “{query}”." })
  } finally { server.stop(true) }
})
