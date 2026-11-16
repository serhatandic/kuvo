import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ask, follow } from "./route-answer"

// Settings › Vault as the box describes it (PROTOCOL.md, "A box's settings", Vault): what agents
// wait on you for, the items with their forms, who may use each, Pause, and the activity, all read
// and changed through the vault's own routes. The vault is real, in a home of the test's own (a
// machine's: its own encrypted store); nothing here is a stand-in but the engine, which isn't asked.
// No value may ever be on a page, in a page followed, in a check's words or in a refusal.

const { engineHandler } = await import("../src/link/server/router")
const { holdVault } = await import("../src/link/vault/holder")
const { VAULT_FOLDER, BOTS } = await import("../src/link/home")
const store = await import("../src/link/vault/store")
const asks = await import("../src/link/vault/asks")

const handler = engineHandler((() => { throw new Error("No engine here.") }) as any, { name: "lokum" }, (() => null) as any)
const held = holdVault(handler, () => () => {})
afterAll(() => held.close())

const SECRET = "hunter2-correct-horse"
const here = process.platform === "darwin" ? "this Mac" : "this computer"

function bot(id: string, name: string) {
  mkdirSync(join(BOTS, id), { recursive: true })
  writeFileSync(join(BOTS, id, "bot.json"), JSON.stringify({ kuvo: 1, id, name }))
  writeFileSync(join(BOTS, id, "session"), `ses_${id}`)
}

beforeEach(() => {
  client = {}
  rmSync(VAULT_FOLDER, { recursive: true, force: true })
  rmSync(BOTS, { recursive: true, force: true })
  store.forgetKeys()
  for (const each of asks.listAsks()) asks.answer(each.id, { answer: "no" })
})

/** The client asking: one that says it makes sure it's you first (Touch ID) before what lets
 * something use what's yours from now on, or (as it starts each test) one that doesn't. */
const CONFIRMS = { "x-kuvo-confirms-you": "1" }
let client: Record<string, string> = {}
const page = async (fromApp = true) => {
  const { status, json } = await ask(handler, "GET", "kuvo/settings/vault", undefined, { fromApp, headers: client })
  expect(status).toBe(200)
  expect(JSON.stringify(json)).not.toContain(SECRET)
  return json
}
const rowOf = (shown: any, id: string): any => shown.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const actionOf = (row: any, id: string) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find((each: any) => each.id === id)
const act = (row: any, action: string, values?: unknown, was = actionOf(row, action)?.form?.revision ?? row.revision) =>
  ask(handler, "POST", `kuvo/settings/vault/${encodeURIComponent(row.id)}/${action}`, { was, ...(values === undefined ? {} : { values }) }, { headers: client })
const check = (row: any, action: string, values: unknown) => ask(handler, "POST", `kuvo/settings/vault/${encodeURIComponent(row.id)}/${action}/check`, { was: actionOf(row, action).form.revision, values }, { headers: client })
const change = (row: any, value: unknown, was = row.revision) => ask(handler, "PATCH", "kuvo/settings/vault", { row: row.id, value, was }, { headers: client })
const opening = (row: any, action: string) => Object.fromEntries(actionOf(row, action).form.fields.filter((field: any) => "value" in field).map((field: any) => [field.id, field.value]))
/** What a client sends in a check for a secret: a dot for each character. */
const masked = (text: string) => "•".repeat(Array.from(text).length)

/** A machine's vault made, with a sign-in in it. */
async function made() {
  store.makeVault()
  return store.addItem({ kind: "login", name: "GitHub", sites: ["github.com"], username: "octo", values: { password: SECRET }, signInOnly: true })
}
const activity = () => JSON.parse(`[${require("node:fs").readFileSync(join(VAULT_FOLDER, "activity.jsonl"), "utf8").trim().split("\n").join(",")}]`)

test("it's one of this computer's sections, and before there's a vault it says where its first item is added", async () => {
  const { json: listed } = await ask(handler, "GET", "kuvo/settings")
  expect(listed.sections.find((each: any) => each.id === "vault")).toMatchObject({ title: "Vault", group: "Agents & Tools", icon: "shield" })
  const shown = await page()
  const items = rowOf(shown, "items")
  expect(items).toMatchObject({ kind: "list", items: [], empty: "Nothing yet. Add a sign-in, a key, a file or a note for your agents to use." })
  // A page can't carry the key a machine's new vault is unlocked with: its first item is added
  // where that key is kept, and adding it here is refused as the page says.
  expect(items.add.can.why).toBe(`There’s no vault on ${here} yet. This app can’t make one: it doesn’t keep the key that unlocks a vault after its computer restarts.`)
  expect(rowOf(shown, "pause")).toBeUndefined()
  const refused = await act(items, "add", { kind: "note", name: "Contact", text: "Call me maybe" })
  expect(refused.status).toBe(403)
  expect(store.isMade()).toBe(false)
})

test("the items are listed by name with what each is and how it's used, and never a value", async () => {
  const github = await made()
  bot("bot_scout", "Scout")
  store.setGrant(github.id, { bot: "bot_scout" }, "own")
  await store.addItem({ kind: "key", name: "App Store", sites: [], values: { value: "key-value-1", issuer_id: "issuer-1" } })
  await store.addItem({ kind: "note", name: "Contact", values: { text: "a note to read" }, readable: true })
  const shown = await page()
  const items = rowOf(shown, "items")
  expect(items.items.map((each: any) => [each.title, each.line, each.badge?.words ?? null])).toEqual([
    ["App Store", "API Key · issuer_id, value", null],
    ["Contact", "Note · Bots may read it", null],
    ["GitHub", "Sign-in · octo · github.com", "On its own"],
  ])
  expect(items.items[2].actions.map((each: any) => each.title)).toEqual(["Edit…", "Who Can Use It…", "Delete…"])
  expect(shown.groups.find((group: any) => group.id === "items").note).toContain("encrypted, with a key only the Kuvo that made it keeps")
  expect(rowOf(shown, "pause")).toMatchObject({ kind: "switch", value: false, can: true })
  for (const secret of ["key-value-1", "issuer-1", "a note to read"]) expect(JSON.stringify(shown)).not.toContain(secret)
})

test("Add Item is checked as it's typed without the values themselves, refused in the same words, and adds an item", async () => {
  await made()
  const items = rowOf(await page(), "items")
  const add = actionOf(items, "add")
  expect(add.form.fields.map((field: any) => [field.id, field.kind, field.shown?.is ?? null])).toEqual([
    ["kind", "choice", null], ["name", "text", null], ["site", "text", ["login"]], ["username", "text", ["login"]], ["password", "secret", ["login"]], ["signInOnly", "switch", ["login"]],
    ["hosts", "text", ["key"]], ["value", "secret", ["key"]], ["more", "pairs", ["key"]], ["file", "file", ["file"]], ["text", "secret", ["note"]], ["readable", "switch", ["note"]],
  ])
  const opened = opening(items, "add")
  // What a client checks with: the secret masked, so the box says only what's missing or short.
  let checked = (await check(items, "add", { ...opened, name: "github", site: "github.com", password: masked("abc") })).json
  expect(checked.ready).toBe(false)
  expect(checked.fields).toContainEqual({ id: "name", words: "There’s an item called “github” already.", tone: "warning" })
  expect(checked.fields).toContainEqual({ id: "password", words: "A value needs at least 4 characters, so it can be hidden from agents.", tone: "warning" })
  checked = (await check(items, "add", { ...opened, name: "Linear", site: "not a site!", password: "" })).json
  expect(checked.fields).toContainEqual({ id: "site", words: "“site!” isn’t a site.", tone: "warning" })
  expect(checked.fields).toContainEqual({ id: "password", words: "Its password.", tone: "quiet" })
  checked = (await check(items, "add", { ...opened, name: "Linear", site: "linear.app", password: masked("linear-pass") })).json
  expect(checked).toEqual({ fields: [], ready: true })

  // A short value sent is refused in the same words, without saying it.
  const refused = await act(items, "add", { ...opened, name: "Linear", site: "linear.app", password: "abc" })
  expect(refused.status).toBe(400)
  expect(refused.json.message).toBe("A value needs at least 4 characters, so it can be hidden from agents.")
  expect(JSON.stringify(refused.json)).not.toContain("\"abc\"")

  const added = await act(items, "add", { ...opened, name: "Linear", site: "https://linear.app/login", username: "me", password: SECRET })
  expect(added.status).toBe(200)
  expect(JSON.stringify(added.json)).not.toContain(SECRET)
  const linear = store.readItems().find(each => each.name === "Linear")!
  expect(linear).toMatchObject({ kind: "login", sites: ["linear.app"], username: "me", fields: ["password"], signInOnly: true })
  expect(await store.valuesOf(linear.id)).toEqual({ password: SECRET })
  expect(activity().at(-1)).toMatchObject({ what: "added", itemName: "Linear", client: "Mac" })
  expect(JSON.stringify(activity())).not.toContain(SECRET)

  // A key with more than one part, and a file of the client's.
  const more = await act(rowOf(await page(), "items"), "add", { ...opened, kind: "key", name: "Store Key", hosts: "api.example.com", value: "the-key-value", more: [{ key: "Issuer ID", value: "issuer-value" }] })
  expect(more.status).toBe(200)
  const key = store.readItems().find(each => each.name === "Store Key")!
  expect(key).toMatchObject({ sites: ["api.example.com"], fields: ["issuer_id", "value"] })
  const big = (await check(rowOf(await page(), "items"), "add", { ...opened, kind: "file", name: "Big", file: { name: "big.p8", size: 900_000 } })).json
  expect(big.fields).toContainEqual({ id: "file", words: "That file is too big for the vault (at most about 700 KB).", tone: "warning" })
  const file = await act(rowOf(await page(), "items"), "add", { ...opened, kind: "file", name: "Signing Key", file: { name: "AuthKey.p8", data: Buffer.from("-----BEGIN KEY-----").toString("base64") } })
  expect(file.status).toBe(200)
  const kept = store.readItems().find(each => each.name === "Signing Key")!
  expect(kept).toMatchObject({ fileName: "AuthKey.p8", fields: ["file"] })
  expect(rowOf(file.json, `item-${kept.id}`).line).toBe("File · AuthKey.p8")
})

test("Edit… opens empty for each value, shows one only through the vault's values route, and saves only what changed", async () => {
  const github = await made()
  let row = rowOf(await page(), `item-${github.id}`)
  const edit = actionOf(row, "edit")
  const password = edit.form.fields.find((field: any) => field.id === "password")
  expect(password).toEqual({ id: "password", title: "Password", kind: "secret", placeholder: "Unchanged", shows: { route: `kuvo/vault/items/${github.id}/values`, field: "password" } })
  expect(edit.form.fields.map((field: any) => field.id)).toEqual(["name", "site", "username", "password", "signInOnly"])

  // Show: the vault's own route, which notes that it was shown.
  const shown = await ask(handler, "GET", password.shows.route)
  expect(shown.json.values.password).toBe(SECRET)
  expect(activity().at(-1)).toMatchObject({ what: "shown", itemName: "GitHub" })

  // Saved as it opened (or with the value shown typed back): nothing is written.
  const lines = activity().length
  const changed = store.readItems()[0]!.changed
  expect((await act(row, "edit", { ...opening(row, "edit"), password: "" })).status).toBe(200)
  expect((await act(row, "edit", { ...opening(row, "edit"), password: SECRET })).status).toBe(200)
  expect(activity().length).toBe(lines)
  expect(store.readItems()[0]!.changed).toBe(changed)

  // A new password and username are saved; the rest stays.
  const saved = await act(row, "edit", { ...opening(row, "edit"), username: "octocat", password: "new-password-1" })
  expect(saved.status).toBe(200)
  expect(store.readItems()[0]).toMatchObject({ username: "octocat", sites: ["github.com"], signInOnly: true })
  expect(await store.valuesOf(github.id)).toEqual({ password: "new-password-1" })
  expect(activity().at(-1)).toMatchObject({ what: "changed", itemName: "GitHub" })

  // The form read before that change is stale: refused with the page now, nothing written.
  const stale = await act(row, "edit", { ...opening(row, "edit"), name: "GitHub 2" })
  expect(stale.status).toBe(409)
  expect(rowOf(stale.json.page, `item-${github.id}`).title).toBe("GitHub")
  // Another item's change doesn't make it stale.
  row = rowOf(await page(), `item-${github.id}`)
  await store.addItem({ kind: "note", name: "Other", values: { text: "other note" } })
  expect((await act(row, "edit", { ...opening(row, "edit"), name: "GitHub Work" })).status).toBe(200)
  expect(store.itemByID(github.id)!.name).toBe("GitHub Work")
})

test("Who Can Use It… takes access away from any client, and lets it be used more only from one that makes sure it's you", async () => {
  const github = await made()
  bot("bot_scout", "Scout")
  bot("bot_ranger", "Ranger")
  store.setGrant(github.id, { bot: "bot_scout" }, "ask")
  let row = rowOf(await page(), `item-${github.id}`)
  let who = actionOf(row, "who").form
  // A client that doesn't: what each is now and less; more says why, and is refused.
  expect(who.fields.map((field: any) => [field.id, field.value, field.choices.map((choice: any) => choice.id), field.can?.why ?? null])).toEqual([
    ["bot-bot_scout", "ask", ["ask", "no"], null],
    ["everyone", "no", ["no"], "This needs Touch ID or your Mac’s password. Update Kuvo to do it here."],
    ["add-bot", "none", ["none", "bot_ranger"], "This needs Touch ID or your Mac’s password. Update Kuvo to do it here."],
  ])
  expect(who.note).toBe("Only on github.com. Agents never see the password: Kuvo fills it in.")
  expect((await act(row, "who", { ...opening(row, "who"), "bot-bot_scout": "own" })).json.message).toBe("This needs Touch ID or your Mac’s password. Update Kuvo to do it here.")
  expect((await act(row, "who", { ...opening(row, "who"), "add-bot": "bot_ranger" })).status).toBe(403)
  expect(store.readGrants()).toMatchObject([{ who: { bot: "bot_scout" }, how: "ask" }])
  // Taken away: from anywhere.
  expect((await act(row, "who", { ...opening(row, "who"), "bot-bot_scout": "no" })).status).toBe(200)
  expect(store.readGrants()).toEqual([])

  // A client that makes sure it's you: every choice, those that let it be used more `verified`
  // (it asks before one is sent; taking access away asks nothing).
  client = CONFIRMS
  row = rowOf(await page(), `item-${github.id}`)
  who = actionOf(row, "who").form
  expect(who.fields.map((field: any) => [field.id, field.title, field.can ?? null, field.choices.map((choice: any) => [choice.id, choice.verified ?? false])])).toEqual([
    ["everyone", "Every agent", null, [["own", true], ["ask", true], ["no", false]]],
    ["add-bot", "Let a Bot Use It", null, [["none", false], ["bot_ranger", true], ["bot_scout", true]]],
  ])
  expect(who.fields[1].choices.map((choice: any) => choice.title)).toEqual(["No One Else", "Ranger", "Scout"])
  expect((await act(row, "who", { ...opening(row, "who"), "add-bot": "bot_scout" })).status).toBe(200)
  row = rowOf(await page(), `item-${github.id}`)
  expect(row.badge).toEqual({ words: "Asks", tone: "quiet" })
  expect((await act(row, "who", { ...opening(row, "who"), "bot-bot_scout": "own" })).status).toBe(200)
  expect(store.readGrants()).toMatchObject([{ who: { bot: "bot_scout" }, how: "own" }])
  expect(activity().map((each: any) => each.what).slice(-3)).toEqual(["ungranted", "granted", "granted"])
  // Someone it can't name is refused.
  row = rowOf(await page(), `item-${github.id}`)
  expect((await act(row, "who", { everyone: "always" })).status).toBe(400)
})

test("a key's other field is taken away from Edit…, and a new file is added only with its bytes", async () => {
  store.makeVault()
  const key = await store.addItem({ kind: "key", name: "Store Key", sites: [], values: { value: "the-key-value", isuer_id: "typo-value" } })
  let row = rowOf(await page(), `item-${key.id}`)
  const edit = actionOf(row, "edit").form
  expect(edit.fields.find((field: any) => field.id === "remove")).toMatchObject({ kind: "choice", value: "none", choices: [{ id: "none", title: "None" }, { id: "isuer_id", title: "Isuer Id" }] })
  expect((await act(row, "edit", { ...opening(row, "edit"), remove: "isuer_id" })).status).toBe(200)
  expect(store.itemByID(key.id)!.fields).toEqual(["value"])
  expect(await store.valuesOf(key.id)).toEqual({ value: "the-key-value" })
  // A new file sent as a check is (its name and size): refused, nothing added.
  const items = rowOf(await page(), "items")
  const refused = await act(items, "add", { ...opening(items, "add"), kind: "file", name: "Signing Key", file: { name: "AuthKey.p8", size: 1234 } })
  expect(refused.status).toBe(400)
  expect(store.readItems().map(each => each.name)).toEqual(["Store Key"])
})

test("Delete… takes the item away with its values and grants", async () => {
  const github = await made()
  const row = rowOf(await page(), `item-${github.id}`)
  expect(actionOf(row, "delete")).toMatchObject({ style: "destructive", confirm: { title: "Delete “GitHub” from the vault?", button: "Delete" } })
  const done = await act(row, "delete")
  expect(done.status).toBe(200)
  expect(store.readItems()).toEqual([])
  expect(rowOf(done.json, `item-${github.id}`)).toBeUndefined()
})

test("what agents wait on you for is answered from the page: once from any client, always and from now on only from one that makes sure it's you", async () => {
  const github = await made()
  const using = asks.ask({ kind: "use", item: github.id, itemName: "GitHub", bot: "bot_scout", botName: "Scout", why: "To open a pull request", always: true })
  const seat = asks.ask({ kind: "use", item: github.id, itemName: "GitHub", botName: "Scout", group: "grp_1" })
  const needing = asks.ask({ kind: "item", botName: "Ranger", need: "an npm token", why: "To publish" })
  const shown = await page()
  const group = shown.groups[0]
  expect(group.title).toBe("Waiting for You")
  expect(group.rows.map((each: any) => [each.title, each.line ?? null, each.actions.map((action: any) => action.title)])).toEqual([
    ["Scout wants to use “GitHub”", "To open a pull request", ["Don’t Allow", "Allow Once", "Always Allow…"]],
    ["Scout wants to use “GitHub”", null, ["Don’t Allow", "Allow Once"]],
    ["Ranger needs an npm token", "To publish", ["Don’t Allow", "Give…"]],
  ])
  expect(group.rows[0].actions.map((action: any) => action.verified ?? false)).toEqual([false, false, true])
  expect(group.rows[0].actions[2].confirm).toEqual({ title: "Let Scout use “GitHub” on its own?", body: "From now on it uses it without asking you. Change that in its Who Can Use It.", button: "Always Allow" })
  // To a client that doesn't make sure it's you: there, with why, and refused.
  expect(group.rows[0].actions[2].can).toEqual({ why: "This needs Touch ID or your Mac’s password. Update Kuvo to do it here." })
  expect((await act(group.rows[0], "always")).status).toBe(403)
  let give = rowOf(await page(), group.rows[2].id)
  expect(actionOf(give, "give").form.fields[1]).toMatchObject({ id: "how", can: { why: "This needs Touch ID or your Mac’s password. Update Kuvo to do it here." } })
  expect((await act(give, "give", { item: github.id, how: "always" })).status).toBe(403)
  expect(asks.listAsks().length).toBe(3)

  // To one that does: offered, and done.
  client = CONFIRMS
  const offered = rowOf(await page(), group.rows[0].id)
  expect(offered.actions[2].can).toBeUndefined()
  expect((await act(offered, "always")).status).toBe(200)
  expect(await using).toMatchObject({ answer: "always", client: "Mac" })
  // A seat's is only ever once.
  const seatRow = rowOf(await page(), group.rows[1].id)
  expect((await act(seatRow, "always", undefined, seatRow.revision)).status).toBe(404)
  expect((await act(seatRow, "once")).status).toBe(200)
  expect(await seat).toMatchObject({ answer: "once" })
  give = rowOf(await page(), group.rows[2].id)
  expect(actionOf(give, "give").form.fields.map((field: any) => [field.id, field.value, field.choices.map((choice: any) => choice.verified ?? false)])).toEqual([["item", "", [false]], ["how", "once", [false, true]]])
  expect((await act(give, "give", { item: github.id, how: "always" })).status).toBe(200)
  expect(await needing).toMatchObject({ answer: "always", item: github.id })
  // One answered elsewhere meanwhile is gone from the page: refused, nothing answered.
  const gone = asks.ask({ kind: "use", item: github.id, itemName: "GitHub", botName: "Scout" })
  const row = (await page()).groups[0].rows[0]
  asks.answer(asks.listAsks()[0]!.id, { answer: "no" })
  await gone
  expect((await act(row, "once")).status).toBe(404)
})

test("Pause stops every agent's use; a paused bot is let go on", async () => {
  await made()
  bot("bot_scout", "Scout")
  store.setPaused(true, "bot_scout", "It filled a sign-in on the wrong site.")
  let shown = await page()
  expect(rowOf(shown, "paused-bot_scout")).toMatchObject({ title: "Scout is paused", line: "It filled a sign-in on the wrong site.", actions: [{ id: "go-on", title: "Let Go On" }] })
  shown = (await change(rowOf(shown, "pause"), true)).json
  expect(store.vaultState().paused).toBe(true)
  expect(rowOf(shown, "pause").value).toBe(true)
  shown = (await act(rowOf(shown, "paused-bot_scout"), "go-on")).json
  expect(store.vaultState().pausedBots).toEqual({})
  expect(rowOf(shown, "paused-bot_scout")).toBeUndefined()
  // (Its bot named as it's called now: the vault notes only who it was.)
  expect(rowOf(shown, "activity").items.map((each: any) => each.title)).toEqual(["Scout’s use of the vault goes on again", "The vault was paused"])
})

test("the activity says what happened in a sentence, yours plainly, and never a value", async () => {
  const github = await made()
  await ask(handler, "GET", `kuvo/vault/items/${github.id}/values`)
  const titles = rowOf(await page(), "activity").items.map((each: any) => each.title)
  // Done by this client: said plainly.
  expect(titles).toEqual(["Looked at “GitHub”"])
})

test("a locked vault says so and shows nothing else; a request that isn't a client's reads nothing", async () => {
  await made()
  store.forgetKeys()
  const shown = await page()
  expect(shown.groups).toEqual([])
  expect(shown.notes).toEqual([{ words: `${here.charAt(0).toUpperCase() + here.slice(1)}’s vault is locked since it started again, so its agents can’t use it. Unlocking it takes the key it was made with, which this app doesn’t keep.`, tone: "warning" }])
  // An agent's command on this computer, say: not a client.
  const outside = await page(false)
  expect(outside.groups).toEqual([])
  expect(outside.notes[0].words).toBe("Only Kuvo on your devices can open lokum’s vault.")
})

test("the page followed is sent again as the vault changes, without a value", async () => {
  const github = await made()
  const followed = follow(handler, "kuvo/settings/vault/events")
  try {
    const first = await followed.next()
    expect(rowOf(first, `item-${github.id}`).title).toBe("GitHub")
    const row = rowOf(first, `item-${github.id}`)
    expect((await act(row, "edit", { ...opening(row, "edit"), name: "GitHub Work", password: "another-secret-9" })).status).toBe(200)
    const next = await followed.next()
    expect(rowOf(next, `item-${github.id}`).title).toBe("GitHub Work")
    expect(JSON.stringify(next)).not.toContain("another-secret-9")
  } finally {
    followed.stop()
  }
})

test("no section's form can carry what's secret, nor read a value but through the vault's own route", async () => {
  const { safeField, SHOWS_ROUTE, item: kitItem } = await import("../src/link/areas/settings-kit")
  // Whatever a section puts there.
  const hostile = kitItem({
    id: "thing", title: "Thing", kind: "actions",
    actions: [{ id: "edit", title: "Edit…", form: { title: "Edit", submit: "Save", fields: [
      { id: "key", title: "Key", kind: "secret", value: SECRET, shows: { route: "kuvo/config", field: "key" } },
      { id: "kept", title: "Kept", kind: "secret", shows: { route: "kuvo/vault/items/v_abcdefghij/values", field: "password" } },
      { id: "env", title: "Env", kind: "pairs", secret: true, value: [{ key: "TOKEN", value: SECRET }] },
      { id: "file", title: "File", kind: "file", value: { name: "a.p8", data: Buffer.from(SECRET).toString("base64") } },
    ] } }],
  })
  const fields = hostile.actions![0]!.form!.fields
  expect(JSON.stringify(hostile)).not.toContain(SECRET)
  expect(JSON.stringify(hostile)).not.toContain(Buffer.from(SECRET).toString("base64"))
  expect(fields.map(field => [field.id, field.value ?? null, field.shows?.route ?? null])).toEqual([
    ["key", null, null], ["kept", null, "kuvo/vault/items/v_abcdefghij/values"], ["env", [{ key: "TOKEN", value: "" }], null], ["file", { name: "a.p8" }, null],
  ])
  expect(safeField({ id: "name", title: "Name", kind: "text", value: "kept as it is" }).value).toBe("kept as it is")

  // And every section this box describes, as a client that makes sure it's you reads it (the most it's offered).
  await made()
  bot("bot_scout", "Scout")
  void asks.ask({ kind: "item", botName: "Scout", need: "a token" })
  client = CONFIRMS
  const { json: listed } = await ask(handler, "GET", "kuvo/settings", undefined, { headers: client })
  for (const section of listed.sections) {
    const { json: shown } = await ask(handler, "GET", `kuvo/settings/${section.id}`, undefined, { headers: client })
    const rows = (shown.groups ?? []).flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])]))
    const forms = rows.flatMap((row: any) => [...(row.actions ?? []), ...(row.add ? [row.add] : []), ...(row.flow?.actions ?? [])]).flatMap((action: any) => (action.form ? [action.form] : []))
    for (const field of forms.flatMap((form: any) => form.fields)) {
      if (field.kind === "secret") expect("value" in field).toBe(false)
      if (field.kind === "pairs" && field.secret) expect((field.value ?? []).every((pair: any) => pair.value === "")).toBe(true)
      if (field.kind === "file") expect(Object.keys(field.value ?? {}).every(key => key === "name")).toBe(true)
      if (field.shows) expect(field.shows.route).toMatch(SHOWS_ROUTE)
    }
    expect(JSON.stringify(shown)).not.toContain(SECRET)
  }
})
