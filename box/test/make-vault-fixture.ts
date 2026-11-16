// Writes the desktop app's stand-in Vault (gpui/kuvo/src/stand_in/settings/vault.json) from the
// box's own code: a machine's vault (lokum), as Kuvo on your other devices reads it, with sample
// items, a bot that may use one, one paused, what agents wait for, and some activity; the page
// locked; and what its forms' checks answer. Never a value: the stand-in makes up its own for Show.
//
// Run with `bun test/make-vault-fixture.ts` from box/ (it uses a home of its own, and removes it).
import "./safety"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const { engineHandler } = await import("../src/link/server/router")
const { holdVault } = await import("../src/link/vault/holder")
const { BOTS } = await import("../src/link/home")
const store = await import("../src/link/vault/store")
const asks = await import("../src/link/vault/asks")
const { note } = await import("../src/link/vault/activity")

const handler = engineHandler((() => { throw new Error("No engine here.") }) as any, { name: "lokum" }, (() => null) as any)
const held = holdVault(handler, () => () => {})
const phone = { key: Buffer.alloc(32, 7) }

async function asked(method: string, path: string, body?: unknown) {
  let status = 0, text = ""
  for await (const piece of handler({ method, path, headers: {}, ...(body === undefined ? {} : { body: Buffer.from(JSON.stringify(body)) }) }, phone as any)) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
  }
  if (status !== 200) throw new Error(`${method} ${path}: ${status} ${text}`)
  return JSON.parse(text)
}

function bot(id: string, name: string) {
  mkdirSync(join(BOTS, id), { recursive: true })
  writeFileSync(join(BOTS, id, "bot.json"), JSON.stringify({ kuvo: 1, id, name }))
  writeFileSync(join(BOTS, id, "session"), `ses_${id}`)
}

bot("bot_scout", "Scout")
bot("bot_ranger", "Ranger")
store.makeVault()
const value = () => `made-up-${Math.random().toString(36).slice(2)}`
const github = await store.addItem({ kind: "login", name: "GitHub", sites: ["github.com"], username: "octo@example.com", values: { password: value() }, signInOnly: true })
const npm = await store.addItem({ kind: "key", name: "npm Token", sites: ["registry.npmjs.org"], values: { value: value() } })
const asc = await store.addItem({ kind: "key", name: "App Store Connect", values: { value: value(), issuer_id: value() } })
await store.addItem({ kind: "file", name: "Signing Key", fileName: "AuthKey_ABC123.p8", values: { file: Buffer.from(value()).toString("base64") } })
await store.addItem({ kind: "note", name: "App Review Contact", values: { text: value() }, readable: true })
store.setGrant(github.id, { bot: "bot_scout" }, "own")
store.setGrant(npm.id, { everyone: true }, "ask")
store.setPaused(true, "bot_ranger", "It tried to fill a sign-in on a site that isn't one of its sites.")
const at = Date.parse("2026-11-16T09:00:00Z")
note({ what: "added", client: "iPhone", by: `phone:${phone.key.toString("base64url")}`, item: github.id, itemName: "GitHub", kind: "login" }, at)
note({ what: "used", item: github.id, itemName: "GitHub", kind: "login", bot: "bot_scout", botName: "Scout", site: "github.com", how: "own" }, at + 3_600_000)
note({ what: "refused", item: asc.id, itemName: "App Store Connect", kind: "key", bot: "bot_ranger", botName: "Ranger", site: "evil.example", how: "wrong site" }, at + 7_200_000)
note({ what: "shown", client: "Mac", by: "mac", item: npm.id, itemName: "npm Token", kind: "key" }, at + 9_000_000)
void asks.ask({ kind: "use", item: asc.id, itemName: "App Store Connect", bot: "bot_scout", botName: "Scout", why: "To upload the build to TestFlight", always: true })
void asks.ask({ kind: "item", botName: "Ranger", need: "a Sentry token", why: "To read the crash reports" })

const section = (await asked("GET", "kuvo/settings")).sections.find((each: any) => each.id === "vault")
const page = await asked("GET", "kuvo/settings/vault")
const rowOf = (id: string): any => page.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const items = rowOf("items")
const add = items.add
const opened = Object.fromEntries(add.form.fields.filter((field: any) => "value" in field).map((field: any) => [field.id, field.value]))
const check = (row: any, action: string, values: unknown) => asked("POST", `kuvo/settings/vault/${encodeURIComponent(row.id)}/${action}/check`, { was: [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find((each: any) => each.id === action).form.revision, values })
const checks = {
  "items/add": await check(items, "add", opened),
  "items/add/short": await check(items, "add", { ...opened, name: "GitHub", site: "github.com", password: "•••" }),
  "items/add/ready": await check(items, "add", { ...opened, name: "Linear", site: "linear.app", password: "••••••••••" }),
}
store.forgetKeys()
const locked = await asked("GET", "kuvo/settings/vault")

const out = join(import.meta.dir, "../../gpui/kuvo/src/stand_in/settings/vault.json")
writeFileSync(out, JSON.stringify({ section, page, locked, checks, items: Object.fromEntries(items.items.map((each: any) => [each.id, each.title])) }, null, 1) + "\n")
for (const each of asks.listAsks()) asks.answer(each.id, { answer: "no" })
held.close()
console.log(`Wrote ${out}`)
process.exit(0)
