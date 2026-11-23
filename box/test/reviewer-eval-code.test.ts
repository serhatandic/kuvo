import "./safety"
import { afterAll, beforeAll, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { accountHomeForTest } from "../src/shared/account"
import { join } from "node:path"

// The reviewer's eval set (`eval/reviewer/actions.jsonl`): every case it says code settles (a hard
// line: Kuvo Code's own folders, wrecking your home; and the agent's own tools reading outside its
// folder, which go ahead) comes out so here, with no model at all; and none of the others is
// settled by code, so the model gets to judge them. The model's own share is measured
// with `eval/reviewer/run.ts` on real models.

let box: string
let reviewer: typeof import("../src/link/work/reviewer")
// (What's judged here is your account's home, as the code finds it: a home of the test's own, with
// its own Desktop, Documents and Downloads, which HOME and your account's home both name for these.
// Never your real one: macOS may hold whoever opens those until you answer its prompt.)
const runsHome = process.env.HOME
let yourHome: string
beforeAll(async () => {
  box = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-reviewer-eval-")))
  yourHome = join(box, "home")
  for (const folder of ["Desktop", "Documents", "Downloads"]) mkdirSync(join(yourHome, folder), { recursive: true })
  process.env.HOME = yourHome
  accountHomeForTest(yourHome)
  reviewer = await import("../src/link/work/reviewer")
})
afterAll(() => {
  process.env.HOME = runsHome
  accountHomeForTest(undefined)
  rmSync(box, { recursive: true, force: true })
})

interface Case { id: string; kind: "command" | "file" | "tool"; command?: string; fetched?: string; path?: string; access?: "read" | "write"; tool?: string; args?: Record<string, unknown>; said?: string[]; bot?: string; expect: string; by?: string; on?: NodeJS.Platform }
// (A case about one system's places, `on` it only: the walls are each system's own.)
const cases = readFileSync(join(import.meta.dir, "..", "eval", "reviewer", "actions.jsonl"), "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line) as Case)
  .filter(each => !each.on || each.on === process.platform)

// (Each case a test of its own: a hundred and more reviews in one would run over a test's time
// limit on a busy Mac, though each takes a moment.)
test("the eval set has its hundred cases and more", () => {
  expect(cases.length).toBeGreaterThanOrEqual(100)
})

test.each(cases.map(each => [each.id, each] as const))("eval case %s: settled by code as it says, or left to the model", async (_, each) => {
  const project = join(box, "project")
  const action = each.kind === "command" ? { kind: "command" as const, command: each.command!, folder: project }
    : each.kind === "file" ? { kind: "file" as const, path: each.path!.replace(/^~(?=\/)/, yourHome), access: each.access ?? "read", folder: project }
    : { kind: "tool" as const, tool: each.tool!, args: each.args ?? {}, folder: project }
  const context = each.bot ? { session: `eval_${each.id}`, said: [each.bot] } : { session: `eval_${each.id}`, said: each.said ?? [] }
  const verdict = await reviewer.review(action, context, { model: null, folder: join(box, "reviews"), fetch: async () => each.fetched ?? Promise.reject(new Error("not in the eval set")) })
  reviewer.forgetFetched(verdict.fetched)
  const byCode = verdict.by === "code" && !(verdict.decision === "refuse" && verdict.why === "noModel")
  const seen = `${verdict.decision} by ${verdict.by} (${verdict.reason})`
  if (each.by === "code") {
    expect(byCode, seen).toBe(true)
    expect<string>(verdict.decision, seen).toBe(each.expect)
  } else {
    expect(byCode, seen).toBe(false)
  }
})
