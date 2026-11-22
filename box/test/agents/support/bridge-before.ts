// The bridge before an update, as a process of its own (bridge-update.test.ts): it starts its agent
// under a holder in one session, and leaves it at work in a command (`working`) or between turns
// (`idle`); told to, it makes way for the next bridge as a bridge does, and ends.
//
// usage: bun bridge-before.ts <codex|claude|antigravity|grok> <folder> <launcher> <working|idle>

// (Not ../../safety, which gives a process a home of its own: it runs in its test's, as it's given.)
import { existsSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { agentHoldFolder } from "../../../src/agents/hold/adopt"
import { heldIn } from "../../../src/agents/hold/client"
import { bridge, goOn, modelOf, type Kind } from "./update-standins"

const [kind, folder, launcher, scenario] = process.argv.slice(2) as [Kind, string, string, "working" | "idle"]
const { agent, sessions } = bridge(kind, folder, launcher)
const hold = agentHoldFolder(kind)!
const id = sessions.create({ title: "Update", model: modelOf(kind, agent), location: { directory: folder } }).id
sessions.prompt(id, { id: "msg_update_turn_00001", text: "run the command", files: [], delivery: "steer" })

async function until(condition: () => boolean) {
  while (!condition()) await new Promise((resolve) => setTimeout(resolve, 10))
}
// Under way in its agent, as a turn running a command is: for Codex, once its turn has its id; for
// Claude Code, once it has said its version, as it does as it starts.
const turnNoted = () => heldIn(hold).some((record) => (kind !== "claude" || record.meta.version !== undefined) && Object.values((record.meta.turns ?? {}) as Record<string, { session: string; turn?: string }>).some((note) => note.session === id && (kind !== "codex" || note.turn !== undefined)))
const sent = () => (existsSync(join(folder, "sent.jsonl")) ? readFileSync(join(folder, "sent.jsonl"), "utf8") : "")
await until(() => sent().includes(kind === "codex" ? '"turn/start"' : kind === "claude" ? '"type":"user"' : '"session/prompt"') && turnNoted())
if (scenario === "idle") {
  await goOn(folder, 1)
  await until(() => sessions.messages(id).some((message) => message.type === "idle") && !turnNoted())
}
console.log(JSON.stringify({ session: id }))
process.stdin.once("data", () => void sessions.makeWay().finally(() => process.exit(0)))
