// Shared Rust/TypeScript fixtures. The production mail writer signs and seals every example.
import { createHash } from "node:crypto"
import { mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Frame, frame, readFrame } from "../../src/link/relay/frames"
import { addressOf, publicOf } from "../../src/link/crypto/keys"
import { MAIL_PROLOGUE, openOneWay, sealOneWay } from "../../src/link/crypto/noise"
import { readDoc, signDoc } from "../../src/link/documents/format"
import { DOC_KINDS } from "../../src/shared/documents"
import { sortedJSON } from "../../src/shared/json"

export async function wireVectors() {
  const { mailFor } = await import("../../src/link/peers/known")
  const at = 1760000000000
  const from = Buffer.alloc(32, 11)
  const to = Buffer.alloc(32, 12)
  const ephemeral = Buffer.alloc(32, 13)
  const seed = Buffer.alloc(32, 14)
  const peer = { key: publicOf(to).toString("base64url"), address: addressOf(to).toString("base64url") }
  const link = { key: from.toString("base64url") }
  const id = "ltr_sample"
  const fields = {
    agents: { at, list: [], features: [], owns: [], moved: [] },
    release: { ask: "0.8.0" },
    setupAuthority: { id: "set_sample", epoch: 4 },
    docs: { have: [[peer.key, "peers", 2, "digest"]] },
    phoneSync: { ask: true },
    ask: "agents",
    sessionFollow: { id: "feed", part: { seq: 1 } },
    setupLists: { at, rows: [] },
    activity: { at, total: 1, part: 0, rows: [] },
    zone: { zone: "Europe/Istanbul" },
    letter: { id, from: "ses_from", to: "ses_to", text: "Hello 👋", wait: false, at },
    call: { id, op: "list_sessions" },
    rows: { id, at, leads: [] },
    move: { id, kind: "offer" },
    moving: { moves: [] },
    seat: { id, turn: "turn_sample" },
    seatPart: { id, of: 2, i: 0 },
    limits: { at, accounts: [] },
    workQueue: { memory: true, acted: { id } },
    botEdit: { id, bot: "bot_sample" },
    scopedBotEdit: { id, bot: "bot_sample" },
    botEdited: { id, ok: true },
    delivered: { id, ok: true, answer: false },
    libraryCopy: { at, rows: [] },
    noticeCopy: { at, rows: [] },
    outputCopy: { at, rows: [] },
    archiveCopy: { at, rows: [] },
    attention: { for: 15, sessions: ["ses_to"] },
    peers: [],
    offer: { operation: id },
    review: { operation: id },
    progress: { step: "prepared" },
    sync: 4,
    request: id,
    recorded: id,
    instruction: { operation: id },
    authority: { epoch: 4 },
    authorityPart: { i: 0, of: 2 },
    choice: { operation: id },
    done: "wipe_sample",
    scopedMove: { id, kind: "offer" },
  }
  const removalKinds = new Set(["offer", "review", "progress", "sync", "request", "recorded", "instruction", "authority", "authorityPart", "choice", "done", "scopedMove"])
  const payloads = Object.entries(fields).map(([name, value]) => ({ name, payload: removalKinds.has(name) ? { [name]: value } : { [name]: value, at } }))
  payloads.push({ name: "agents and documents together", payload: { agents: fields.agents, docs: fields.docs, ask: "agents", at } })
  const removalHintBytes = Buffer.byteLength("kuvo-removal-v1\0") + 32
  const mail = payloads.map(({ name, payload }) => {
    const removal = removalKinds.has(name)
    const relayed = mailFor(peer, payload, link, removal, { trackDelivery: false })!
    const body = readFrame(relayed.subarray(17))!.body
    const actual = openOneWay(removal ? body.subarray(removalHintBytes) : body, to, publicOf(from)).payload
    const sealed = sealOneWay(actual, from, publicOf(to), MAIL_PROLOGUE, ephemeral).message
    const fixed = frame(Frame.mail, 0, removal ? Buffer.concat([body.subarray(0, removalHintBytes), sealed]) : sealed)
    return { name, payload, removal, plaintext: actual.toString(), frame: fixed.toString("hex") }
  })
  const documents = DOC_KINDS.map((kind, index) => {
    const n = index + 1
    const body = { z: "first", a: ["second", 2], "10": "ten", "2": "two" }
    const envelope = signDoc(seed.toString("base64url"), kind, n, body)
    return { kind, n, body, envelope, digest: readDoc(envelope)!.h }
  })
  // A document's short exchange hash covers the signed bytes. Canonical
  // digests cover the value and must stay stable across key insertion order.
  const digests = [
    ...documents.map(({ envelope }) => JSON.parse(Buffer.from(envelope.d, "base64url").toString())),
    { same: { z: "é", a: "e\u0301", "00": 0, "0": 0, "1": 1, "4294967294": 2, "4294967295": 3 } },
    { same: { "4294967295": 3, "1": 1, "0": 0, "4294967294": 2, "00": 0, a: "e\u0301", z: "é" } },
    [Number.MIN_VALUE, Number.MAX_VALUE, -1e-7, -1e21, -9007199254740991],
    { empty: {}, list: [], controls: "\b\f\r\t\u0000\u2028\u2029", escapes: "\\/" },
    { z: { b: 1e-7, a: -0 }, a: [1e-6, 1e20, 1e21, 1.5, 9007199254740992] },
    { "\uE000": "BMP", "\u{10000}": "astral", "10": "ten", "2": "two", a: "quote\n\"\\é" },
    [null, true, false, { nested: [{ z: 1, a: 2 }] }],
  ].map(value => {
    const canonical = sortedJSON(value)
    return { value, canonical, sha256: createHash("sha256").update(canonical).digest("base64url") }
  })
  return {
    from: from.toString("hex"), to: to.toString("hex"), ephemeral: ephemeral.toString("hex"),
    fromPublic: publicOf(from).toString("hex"), toPublic: publicOf(to).toString("hex"),
    seed: seed.toString("hex"), mail, documents, digests,
  }
}

if (import.meta.main) {
  const home = mkdtempSync(join(tmpdir(), "kuvo-wire-"))
  const output = new URL("../../../protocol/vectors/wire.json", import.meta.url)
  const draft = new URL(`${output.href}.${process.pid}.tmp`)
  try {
    for (const name of Object.keys(process.env)) {
      if (name.startsWith("KUVO_")) delete process.env[name]
    }
    process.env.HOME = home
    process.env.USERPROFILE = home
    process.env.KUVO_HOME = home
    writeFileSync(draft, JSON.stringify(await wireVectors(), null, 2) + "\n")
    renameSync(draft, output)
    console.log("Wrote protocol/vectors/wire.json")
  } finally {
    rmSync(draft, { force: true })
    rmSync(home, { recursive: true, force: true })
  }
}
