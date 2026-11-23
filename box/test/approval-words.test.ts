import "./safety"
import { expect, test } from "bun:test"
import { ask } from "./route-answer"

// The modes' words (PROTOCOL.md, "Approval", `options`): the box names each mode and says what it
// does in one line, so the model picker's permissions on every client show the same words. The
// box's code is real, in a home of the test's own; there's no engine or bridge.

const { engineHandler } = await import("../src/link/server/router")

const noEngine = () => { throw new Error("The engine isn't running.") }
const handler = engineHandler(noEngine as any, { name: "devbox" }, noEngine as any)

test("each mode a new conversation can start in comes with its name and one line on what it does", async () => {
  const here = (await ask(handler, "GET", "kuvo/approval")).json
  for (const side of ["engine", "agents"]) {
    const words = here.options[side].map((option: any) => [option.approval, option.title, option.detail])
    expect(words).toEqual([
      ["askFirst", "Ask First", "Asks before it edits files or runs commands."],
      ["acceptEdits", "Accept Edits", "Edits files in the project on its own; asks before commands."],
      ["auto", "Auto", "Works on its own inside the sandbox; asks before anything beyond it."],
      ["fullAccess", "Full Access", "Runs everything outside the sandbox and never asks."],
    ])
  }
})
