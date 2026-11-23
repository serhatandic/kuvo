// Your starred models (PROTOCOL.md, "The models offered", `models.starred`): kept setup-wide by the
// setup's main computer, so every computer's picker, from every client, shows the same stars, in the
// order you starred them; a box in no setup that saves setup-wide values keeps its own, as before.
// Real: the box's router and routes, its setup's shared values (saved, seeded, published and taken
// by a member), in the run's home. Stand-ins: the other computer of a setup is a second link state
// written in its turn into the same home (one box per process); its mail is the forwarded write the
// main receives (`writeForPeer`) and the signed document it publishes (`takeShared`).

import "./safety"
import { afterEach, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { MODELS_FILE, setStarred, starredFromKeys, starredModels } from "../src/link/areas/models-starred"
import { ask } from "./route-answer"
import { grantRelayLease, leaseFile } from "./relay-lease"
import { engineHandler } from "../src/link/server/router"
import { K, RUN } from "../src/link/home"
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys"
import { newSigningSeed, readDoc, signingKeyOf } from "../src/link/documents/format"
import { publishedShared, sharedValue, takeShared } from "../src/link/setup/shared"
import { sendSetupCopies, takeSetupCopy } from "../src/link/setup/seed"
import { newSetupID } from "../src/link/setup/record"
import { readLink, writeLink, type LinkState } from "../src/link/state"
import { writeForPeer } from "../src/link/ownership/write"
import { upgradeKuvoFolder } from "../src/shared/folder-version"

const file = () => join(mkdtempSync(join(tmpdir(), "kuvo-starred-")), "models.json")

test("a star is kept in the order given, once, and taken away again", () => {
  const at = file()
  expect(starredModels(at)).toEqual([])
  setStarred({ providerID: "anthropic", id: "claude-opus-5-5" }, true, at)
  setStarred({ providerID: "openai", id: "gpt-6" }, true, at)
  // Starred again (from another client at once): still once, where it was.
  expect(setStarred({ providerID: "anthropic", id: "claude-opus-5-5" }, true, at)).toEqual([
    { providerID: "anthropic", id: "claude-opus-5-5" },
    { providerID: "openai", id: "gpt-6" },
  ])
  expect(setStarred({ providerID: "openai", id: "gpt-6" }, false, at)).toEqual([{ providerID: "anthropic", id: "claude-opus-5-5" }])
  // Read again, as the next client to ask would.
  expect(starredModels(at)).toEqual([{ providerID: "anthropic", id: "claude-opus-5-5" }])
})

test("what else the file keeps stays, and a file that can't be read is no stars", () => {
  const at = file()
  writeFileSync(at, JSON.stringify({ other: 1, starred: [{ providerID: "x", id: "y" }, "nonsense", { providerID: "x", id: "y" }] }))
  expect(starredModels(at)).toEqual([{ providerID: "x", id: "y" }])
  setStarred({ providerID: "a", id: "b" }, true, at)
  expect(JSON.parse(readFileSync(at, "utf8")).other).toBe(1)
  writeFileSync(at, "{ not json")
  expect(starredModels(at)).toEqual([])
})

test("the earlier Mac app's starred models come along, by their provider and model", () => {
  expect(starredFromKeys("anthropic/claude-opus-5-5,openrouter/meta/llama-4,,bad,anthropic/claude-opus-5-5")).toEqual([
    { providerID: "anthropic", id: "claude-opus-5-5" },
    { providerID: "openrouter", id: "meta/llama-4" },
  ])
})

// A computer of a setup, and the setup's link state for it.
function identity() {
  const key = newKey()
  const sign = newSigningSeed()
  return { seed: key.toString("base64url"), key: publicOf(key).toString("base64url"), sign, signer: signingKeyOf(sign), address: addressOf(key).toString("base64url") }
}
type Box = ReturnType<typeof identity>
const rosterOf = (boxes: Box[]) => boxes.map((box, ix) => ({ key: box.key, sign: box.signer, address: box.address, kind: "machine", name: ix ? "Laptop" : "Desk" }))
function setupLink(main: Box, boxes: Box[], role: "primary" | "member" = "primary", own = main): LinkState {
  const id = newSetupID()
  return {
    key: own.seed, sign: own.sign, phones: [], sharedSetup: id,
    setup: { id, epoch: 1, primary: main.key, sign: main.signer, role, managed: true, checkpointed: true },
    shared: { membershipAndAccess: { sequence: 1, value: { boxes: rosterOf(boxes) } } },
  }
}

const noEngine = () => { throw new Error("The engine isn't running.") }
const handler = engineHandler(noEngine as any, { name: "Desk" } as any, noEngine as any)
const star = (providerID: string, id: string, starred = true) => ask(handler, "POST", "kuvo/models/starred", { providerID, id, starred })
const listed = async () => (await ask(handler, "GET", "kuvo/models/starred")).json.starred
const shared = () => sharedValue(readLink(), "setupStarredModels")
const opus = { providerID: "anthropic", id: "claude-opus-5-5" }
const gpt = { providerID: "openai", id: "gpt-6" }
const llama = { providerID: "openrouter", id: "meta/llama-4" }

// (The run's home is shared with other files: its link, lease and models file are put back as they were.)
const kept = [join(RUN, "link.json"), leaseFile, MODELS_FILE]
let before = new Map<string, Buffer | null>()
beforeEach(() => {
  before = new Map(kept.map(file => [file, existsSync(file) ? readFileSync(file) : null]))
  for (const file of kept) rmSync(file, { force: true })
})
afterEach(() => {
  for (const [file, bytes] of before) {
    if (bytes) writeFileSync(file, bytes)
    else rmSync(file, { force: true })
  }
})
const writeModels = (value: unknown) => {
  mkdirSync(dirname(MODELS_FILE), { recursive: true })
  writeFileSync(MODELS_FILE, JSON.stringify(value))
}

test("on a computer alone in its setup, a star is the setup's: saved through the route, read back by every client, and taken away again", async () => {
  const desk = identity()
  writeLink(setupLink(desk, [desk]))
  expect(await listed()).toEqual([])
  expect((await star(opus.providerID, opus.id)).json).toEqual({ starred: [opus] })
  expect((await star(gpt.providerID, gpt.id)).json).toEqual({ starred: [opus, gpt] })
  // Starred again (from another client at once): still once, where it was.
  expect(await star(opus.providerID, opus.id)).toEqual({ status: 200, json: { starred: [opus, gpt] } })
  expect(shared()).toEqual({ starred: [opus, gpt] })
  expect(await listed()).toEqual([opus, gpt])
  // What `kuvo/models/offered` says as `starred`.
  expect(starredModels()).toEqual([opus, gpt])
  // Kept by the setup, not in this box's own file.
  expect(existsSync(MODELS_FILE)).toBe(false)

  expect((await star(gpt.providerID, gpt.id, false)).json).toEqual({ starred: [opus] })
  expect(shared()).toEqual({ starred: [opus] })
  expect(await listed()).toEqual([opus])
  expect(starredModels()).toEqual([opus])
  // Still a 400 for a model not said.
  expect((await ask(handler, "POST", "kuvo/models/starred", { providerID: "openai", starred: true })).status).toBe(400)
})

test("a computer's stars from before it kept them setup-wide seed the setup's list: none lost, and one taken away stays away", async () => {
  const desk = identity()
  writeModels({ starred: [opus, gpt], speeds: { openai: "fast" } })
  writeLink(setupLink(desk, [desk]))
  // Before the setup has a list, the picker shows this computer's own.
  expect(await listed()).toEqual([opus, gpt])
  expect(shared()).toBe(undefined)
  // The first star saved: the setup's list starts from them.
  expect((await star(llama.providerID, llama.id)).json).toEqual({ starred: [opus, gpt, llama] })
  expect(shared()).toEqual({ starred: [opus, gpt, llama] })
  expect((await star(opus.providerID, opus.id, false)).json).toEqual({ starred: [gpt, llama] })
  // The box's next rounds of offering its own copy (as every computer does) bring nothing back.
  await sendSetupCopies(null, Date.now() + 60_000)
  expect(await listed()).toEqual([gpt, llama])
  // Its own file is left as it was, with what else it keeps.
  expect(JSON.parse(readFileSync(MODELS_FILE, "utf8"))).toEqual({ starred: [opus, gpt], speeds: { openai: "fast" } })
  // Its own list changes later (the earlier Mac app's stars brought along): the new one joins; the
  // one taken away in the setup stays away.
  const gemini = { providerID: "google", id: "gemini-3-pro" }
  writeModels({ starred: [opus, gpt, gemini] })
  await sendSetupCopies(null, Date.now() + 90_000)
  expect(await listed()).toEqual([gpt, llama, gemini])
})

test("with no star saved yet, the box's own round seeds the setup's list, and another computer's copy joins it", async () => {
  const desk = identity(), laptop = identity()
  writeModels({ starred: [opus] })
  const link = setupLink(desk, [desk, laptop])
  writeLink(link)
  grantRelayLease(link)
  await sendSetupCopies(null, Date.now() + 120_000)
  expect(shared()).toEqual({ starred: [opus] })
  // The laptop's own stars, offered in its mail.
  const sequence = readLink()!.shared!.setupStarredModels!.sequence
  await takeSetupCopy(laptop.key, { setup: link.setup!.id, epoch: 1, kind: "setupStarredModels", value: { starred: [gpt, opus] }, revision: sequence })
  expect(await listed()).toEqual([opus, gpt])
})

test("a star saved on a member is saved by the main computer, and is what both then read", async () => {
  upgradeKuvoFolder(K)
  const desk = identity(), laptop = identity()
  const main = setupLink(desk, [desk, laptop])
  const member: LinkState = { ...setupLink(desk, [desk, laptop], "member", laptop), setup: { ...main.setup!, role: "member" }, sharedSetup: undefined, shared: undefined }
  writeLink(main)
  grantRelayLease(main)
  // The member's change, as the main receives it from the member's box.
  const [peer] = rosterOf([laptop])
  const saved = await writeForPeer(peer as any, { id: "act_starred1", method: "POST", item: "kuvo/models/starred", body: { value: { ...gpt, starred: true } }, at: Date.now() }, { targets: [], bridge: false })
  expect(saved).toEqual({ status: 200, value: { starred: [gpt] } })
  expect(await listed()).toEqual([gpt])

  // The main publishes the setup's values; the member takes them, and its picker shows the star.
  const doc = publishedShared(readLink()!).map(envelope => readDoc(envelope)!).find(doc => (doc.body.replica as Record<string, unknown> | undefined)?.setupStarredModels)
  expect(doc).toBeDefined()
  takeShared(member, desk.key, doc!)
  writeLink(member)
  expect(await listed()).toEqual([gpt])
  expect(starredModels()).toEqual([gpt])

  // The main out of reach: a star on the member is refused, never kept on the member alone.
  // (Here the member hasn't heard the main take changes from others: what it says then, as for the time zone.)
  expect(await star(opus.providerID, opus.id)).toEqual({ status: 409, json: { message: "This record is on Desk: change it there." } })
  expect(await listed()).toEqual([gpt])
  expect(existsSync(MODELS_FILE)).toBe(false)
})
