import "./safety"
import { expect, test } from "bun:test"
import { askNotice } from "../src/link/areas/asks"
import { sandboxAskNotice } from "../src/link/fence/outside"
import { noticeJSON } from "../src/link/notices/notice"
import { noticeFor, groupNotice } from "../src/link/notices/news"
import { shortNotice } from "../src/link/notices/plain"
import { vaultAskNotice } from "../src/link/vault/asks"
import { workAskWords } from "../src/link/work/asks"

// A notification is in plain words and short: what's asked, never a command line, a path, a flag or
// a code. Those are on the card in the conversation. One long command made a notification a wall
// of text on the lock screen; each kind says it in a line now.

const sessions: Record<string, object> = {
  ses_bot: { id: "ses_bot", title: "Disk Cleaner", metadata: { kuvo: { bot: "bot_1" } }, location: { directory: "/home/me/.kuvo-code/bots/bot_1/workspace" } },
  ses_main: { id: "ses_main", title: "Fix the tests", location: { directory: "/home/me/kuvo-ai" } },
}
const engine = (messages: unknown[] = []) => async (route: string) => {
  if (route.includes("/message?")) return { data: messages }
  const id = decodeURIComponent(route.split("/").pop()!)
  if (!sessions[id]) throw new Error("404")
  return { data: sessions[id] }
}
const permission = (action: string, resources: unknown[], session = "ses_bot") =>
  noticeFor({ type: "permission.asked", data: { id: "per_1", sessionID: session, action, resources } }, engine())

const LONG_COMMAND = [
  'echo "== var/folders/vg C"',
  "du -x -k -d 2 /private/var/folders/vg/ymy_cd692kq_cbj2j1d_44n40000gn/C 2>/dev/null | sort -rn | head -20",
]

test("a long command is one short line naming the program, not the command, and its buttons don't answer it", async () => {
  const notice = (await permission("shell", LONG_COMMAND))!
  expect(notice).toMatchObject({ title: "Disk Cleaner", category: "permission", body: "Wants to run echo and du." })
  expect(notice.body!.length).toBeLessThanOrEqual(60)
  // It says less than it covers, so Allow isn't offered from the notification.
  expect(notice.data?.answerable).toBe("0")
  const sent = JSON.parse(noticeJSON(notice).toString())
  for (const raw of ["var/folders", "-x", "2>/dev/null", "ymy_"]) expect(sent.body).not.toContain(raw)
})

test("each kind of permission says what it would do in a few plain words", async () => {
  const says = async (action: string, resources: unknown[]) => (await permission(action, resources))!.body
  expect(await says("shell", ["git push origin main"])).toBe("Wants to run git.")
  expect(await says("bash", ["/usr/bin/env FOO=1 npm test", "git status"])).toBe("Wants to run env and git.")
  expect(await says("shell", ["FOO=1 make build", "cd /tmp && npm test | tee out.log", "git log", "ls -la"])).toBe("Wants to run make, npm and 2 more.")
  expect(await says("shell", ['"$HOME/bin/odd" --flag'])).toBe("Wants to run a command.")
  expect(await says("shell", ["*"])).toBe("Wants to run a command.")
  expect(await says("edit", ["src/deep/inside/Parser.swift"])).toBe("Wants to edit Parser.swift.")
  expect(await says("write", ["a.txt", "b.txt", "c.txt"])).toBe("Wants to edit 3 files.")
  expect(await says("patch", ["src/**/*.ts"])).toBe("Wants to edit files.")
  expect(await says("edit", ["*"])).toBe("Wants to edit files.")
  expect(await says("read", ["/Users/me/Documents/taxes.pdf"])).toBe("Wants to read taxes.pdf.")
  expect(await says("read", [])).toBe("Wants to read a file.")
  expect(await says("external_directory", ["/Users/me/notes/*", "/opt/data/*"])).toBe("Wants to work outside the project.")
  expect(await says("webfetch", ["https://www.example.com/a/very/long/path?token=abc123"])).toBe("Wants to open example.com.")
  expect(await says("webfetch", ["https://a.dev/x", "https://b.dev/y"])).toBe("Wants to open a web page.")
  expect(await says("task", [])).toBe("Wants to start a helper.")
  expect(await says("linear_create_issue", [])).toBe("Wants to use a tool.")
})

test("a notification's buttons answer a permission only where its words say all it covers", async () => {
  expect((await permission("shell", ["ls"]))!.data?.answerable).toBeUndefined()
  expect((await permission("shell", ["ls", "pwd"]))!.data?.answerable).toBeUndefined()
  expect((await permission("shell", ["ls -la"]))!.data?.answerable).toBe("0")
  expect((await permission("edit", ["notes.md"]))!.data?.answerable).toBe("0")
  expect((await permission("edit", []))!.data?.answerable).toBeUndefined()
})

test("a question is its own words in plain terms, cut at a word with an ellipsis", async () => {
  const ask = (description: string) => noticeFor({ type: "form.created", data: { form: { id: "frm_1", sessionID: "ses_main", metadata: { kind: "question" }, fields: [{ type: "string", description }] } } as never }, engine())
  const short = (await ask("Which database should the new service use?"))!
  expect(short).toMatchObject({ category: "question", body: "Which database should the new service use?" })
  expect(short.data?.answerable).toBeUndefined()
  const long = (await ask("Should I move the files in /Users/me/Projects/kuvo-ai/docs/guides/setup.md into the new folder and update every link that points at them, or leave them where they are?"))!
  expect(long.body!.length).toBeLessThanOrEqual(100)
  expect(long.body).toStartWith("Should I move the files in setup.md into the new folder")
  expect(long.body).toEndWith("…")
  expect(long.body).not.toContain("/Users")
  // Cut, so it doesn't answer what it doesn't fully show.
  expect(long.data?.answerable).toBe("0")
})

test("a finish is its first sentence, its paths as names; a failure's reason only where it's words, not a code", async () => {
  const done = (await noticeFor({ type: "session.execution.succeeded", data: { sessionID: "ses_main" } }, engine([
    { type: "idle" },
    { type: "assistant", content: [{ type: "text", text: "I fixed the parser in `/Users/me/Projects/kuvo-ai/src/parser/Lexer.swift` and reran every test in the suite, and all of them pass now after the change. More below." }] },
    { type: "user", text: "Fix the tests" },
  ])))!
  expect(done).toMatchObject({ category: "finished", title: "Fix the tests" })
  expect(done.body!.length).toBeLessThanOrEqual(100)
  expect(done.body).toStartWith("I fixed the parser in Lexer.swift and reran every test")
  expect(done.body).toEndWith("…")
  const failed = (reason: unknown) => noticeFor({ type: "session.execution.failed", data: { sessionID: "ses_main", error: { message: reason } } }, engine()).then(each => each!.body)
  expect(await failed("Rate limited by the provider.\nTry again in 20 minutes.")).toBe("Stopped with an error: Rate limited by the provider.")
  expect(await failed("429 Too Many Requests {\"error\":{\"type\":\"rate_limit_error\"}}")).toBe("Stopped with an error.")
  expect(await failed("ECONNRESET")).toBe("Stopped with an error.")
  expect(await failed("fetch failed: https://api.example.com/v1/messages?id=req_8f3k2")).toBe("Stopped with an error.")
  expect(await failed("")).toBe("Stopped with an error.")
})

test("a browser ask, a sandbox ask, a vault ask and a project ask say it short, with no command, address or path", () => {
  expect(askNotice({ id: "ask-1", kind: "action", host: "github.com", instruction: "Click Authorize on the page at https://github.com/login/oauth/authorize?client_id=abc123 and then tell me once the page has loaded fully so I can go on." } as never, "Fixer"))
    .toMatchObject({ title: "Your turn on github.com", body: "Click Authorize on the page at github.com and then tell me once the page has loaded fully so I can…" })
  const sandbox = sandboxAskNotice({ id: "sbx_1", command: "rm -rf /Users/me/Library/Caches/some/deep/folder && echo done", why: "To clear the old release outside the project.", sessionName: "Disk Cleaner", at: 1 } as never)
  expect(sandbox).toMatchObject({ title: "Disk Cleaner", body: "Wants to run rm and echo outside the sandbox.", data: { sandboxAsk: "sbx_1", answerable: "0" } })
  expect(sandboxAskNotice({ id: "sbx_2", command: "ls", sessionName: "Disk Cleaner", at: 1 } as never).data?.answerable).toBeUndefined()
  const vault = vaultAskNotice({ id: "vlt_1", kind: "need", botName: "Fixer", need: "the production database password for the billing service in the eu-west region", why: "The migration needs to sign in to the production database with it before the schedule runs tonight.", at: 1 } as never)
  expect(vault.title.length).toBeLessThanOrEqual(60)
  expect(vault.title).toStartWith("Fixer needs the production database")
  expect(vault.body!.length).toBeLessThanOrEqual(100)
  expect(workAskWords({ action: "cloneProject", name: "Fixer", project: "shop", clone: "https://github.com/me/shop.git", folder: "/Users/me/Projects/shop" } as never))
    .toEqual({ title: "Fixer asks to clone a project", body: "shop" })
})

test("whatever its source, a notice goes out with a short title and a body of one sentence", () => {
  const report = "Day 3: 4 steps done (9 of 12 in all), 1 waits for you. ".repeat(4)
  const shown = shortNotice({ title: "A very long conversation title that goes on and on well past what any lock screen shows", body: `# Report\n\n${report}`, category: "report", thread: "ses_1" })
  expect(shown.title!.length).toBeLessThanOrEqual(60)
  expect(shown.title).toEndWith("…")
  expect(shown.body).toBe("Day 3: 4 steps done (9 of 12 in all), 1 waits for you.")
  // What's short as it is stays.
  const short = { title: "Fixer", body: "Done.", category: "finished" as const, thread: "ses_1" }
  expect(shortNotice(short)).toEqual(short)
  const group = groupNotice({ type: "kuvo.group.met", data: { id: "grp_1", name: "Standup", who: "Mira", last: { kind: "say", text: "I looked at /Users/me/Projects/kuvo-ai/docs/plan.md and nothing needs changing." } } })!
  expect(group.body).toBe("I looked at plan.md and nothing needs changing.")
})
