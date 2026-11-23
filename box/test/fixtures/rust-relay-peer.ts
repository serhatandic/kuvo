// A localhost production relay. The Rust test owns this process, the TypeScript box that uses
// it, and both scratch homes.
import assert from "node:assert/strict"
import { writeSync } from "node:fs"
import { createRelay } from "../../../relay/src/relay"
import { Registrations } from "../../../relay/src/registrations"

assert(process.env.KUVO_HOME?.endsWith("/ts-peer"), "requires a private peer home")
const relay = createRelay({ registrations: new Registrations() })
await new Promise<void>(done => relay.server.listen(0, "127.0.0.1", done))
const address = relay.server.address()
assert(address && typeof address !== "string")
writeSync(1, `ws://127.0.0.1:${address.port}\n`)
process.stdin.resume()
await new Promise<void>(done => process.stdin.once("end", done))
await relay.close()
