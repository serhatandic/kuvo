import "./safety"
import { afterAll, expect, setDefaultTimeout, test } from "bun:test"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { askedArgs, GIT_QUIET, ownArgs } from "../src/shared/git"

// A repository whose own settings (and a file they include) name a program for everything git can
// start: hooks (a hooks folder and settings), the fsmonitor, the pager and editor, a diff program
// and a diff and merge driver with a converter, a filter, a credential helper, an `ext::` remote,
// an ssh command, signing and an alias. What the box runs by itself through ownArgs starts none of
// them; what you ask for (askedArgs) runs no fsmonitor, pager, diff program or signature check.
// The Rust layer has the same case (`kuvo_services::git::safe`, `hostile`).

setDefaultTimeout(30_000)
const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-git-hostile-")))
afterAll(() => rmSync(root, { recursive: true, force: true }))
const none = join(root, "no-settings")
writeFileSync(none, "")
const AUTHOR = { GIT_AUTHOR_NAME: "Test", GIT_AUTHOR_EMAIL: "test@example.com", GIT_COMMITTER_NAME: "Test", GIT_COMMITTER_EMAIL: "test@example.com" }
const ENV = { ...process.env, ...AUTHOR, GIT_CONFIG_GLOBAL: none, GIT_CONFIG_NOSYSTEM: "1" }
function git(folder: string, ...args: string[]): string {
  const ran = spawnSync("git", ["-c", "init.defaultBranch=main", "-c", "core.hooksPath=/dev/null", "-c", "hook.planted.enabled=false", "-c", "core.fsmonitor=false", ...args], { cwd: folder, env: ENV, encoding: "utf8" })
  if (ran.status !== 0) throw new Error(`git ${args.join(" ")}: ${ran.stderr}`)
  return ran.stdout.trim()
}

let made = 0
function planted() {
  const at = join(root, `${++made}`)
  const [top, programs, marks] = ["project", "programs", "marks"].map(name => join(at, name))
  for (const folder of [at, top, programs, marks, join(programs, "hooks")]) mkdirSync(folder)
  git(top, "init", "-q")
  writeFileSync(join(top, "a.txt"), "one\n")
  writeFileSync(join(top, "b.txt"), "one\n")
  writeFileSync(join(top, ".gitattributes"), "a.txt filter=evil diff=evil merge=evil\n")
  git(top, "add", "-A")
  git(top, "commit", "-qm", "first")
  const program = (name: string, then: string) => {
    const file = join(programs, name)
    writeFileSync(file, `#!/bin/sh\ntouch "${marks}/${name.replace("/", "-")}"\n${then}\n`)
    chmodSync(file, 0o755)
    return file
  }
  for (const hook of ["pre-commit", "prepare-commit-msg", "commit-msg", "post-commit", "post-checkout", "pre-auto-gc", "reference-transaction", "post-index-change", "pre-push"]) program(`hooks/${hook}`, "exit 0")
  const config = (key: string, value: string) => git(top, "config", "--add", key, value)
  config("core.fsmonitor", program("fsmonitor", "exit 0"))
  config("core.hooksPath", join(programs, "hooks"))
  config("hook.planted.command", program("hook-setting", "exit 0"))
  for (const event of ["reference-transaction", "post-index-change", "pre-commit", "post-checkout"]) config("hook.planted.event", event)
  config("core.pager", program("pager", "cat"))
  config("core.editor", program("editor", "exit 0"))
  config("diff.external", program("external", "exit 0"))
  config("log.showSignature", "true")
  config("gpg.program", program("gpg", "exit 1"))
  config("commit.gpgSign", "true")
  config("tag.gpgSign", "true")
  config("filter.evil.clean", program("clean", "cat"))
  config("filter.evil.smudge", program("smudge", "cat"))
  config("filter.evil.required", "true")
  config("merge.evil.driver", program("merge", "exit 1"))
  config("credential.helper", program("credential", "exit 0"))
  config("core.askPass", program("askpass", "exit 0"))
  config("protocol.ext.allow", "always")
  config("remote.origin.url", `ext::${program("ext", "exit 1")}`)
  config("core.sshCommand", program("ssh", "exit 1"))
  config("remote.far.url", "ssh://example.invalid/x")
  config("alias.st", `!${program("alias", "exit 0")}`)
  // A local path its settings rewrite to an `ext::` remote, and one to an ssh one.
  config(`url.ext::${program("rewritten", "exit 1")}.insteadOf`, join(at, "rewritten"))
  config("url.ssh://example.invalid/.insteadOf", join(at, "rewritten-ssh"))
  // And a repository at a local path, for a fetch the box does make.
  git(at, "init", "-q", "source")
  git(join(at, "source"), "commit", "-q", "--allow-empty", "-m", "from elsewhere")
  writeFileSync(join(programs, "more.config"), `[diff "evil"]\n\ttextconv = ${program("textconv", 'cat "$1"')}\n\tcommand = ${program("command", "exit 0")}\n`)
  config("include.path", join(programs, "more.config"))
  writeFileSync(join(top, "a.txt"), "one\ntwo, longer\n")
  writeFileSync(join(top, "b.txt"), "one\ntwo\n")
  const run = (args: string[], input = "") => spawnSync("git", args, { cwd: top, env: { ...ENV, ...GIT_QUIET }, input, stdio: ["pipe", "ignore", "ignore"] })
  return { at, top, run, ran: () => readdirSync(marks).sort(), forget: () => { rmSync(marks, { recursive: true }); mkdirSync(marks) } }
}

const OWN_COMMANDS = [
  ["status", "--porcelain=v1", "-z", "--untracked-files=all"], ["diff"], ["diff", "HEAD", "--stat"], ["log", "-p", "-2"], ["show", "HEAD"], ["blame", "a.txt"], ["grep", "two"],
  ["ls-files", "-m"], ["rev-parse", "HEAD"], ["for-each-ref"], ["remote", "get-url", "origin"], ["add", "-A"], ["diff", "--cached"], ["write-tree"], ["stash", "list"],
  ["branch", "feature"], ["update-ref", "refs/kuvo/kept", "HEAD"], ["commit", "-qm", "second"], ["tag", "-a", "-m", "kept", "kept"], ["checkout", "-q", "feature"], ["gc", "--auto"], ["credential", "fill"],
]

test("the repository's programs are really there: git itself runs them", () => {
  const { top, ran } = planted()
  for (const args of [["status", "--porcelain"], ["diff"], ["branch", "control"], ["commit", "--allow-empty", "-qm", "signed"], ["st"], ["ls-remote", "origin"], ["ls-remote", "far"], ["credential", "fill"], ["-c", "credential.helper=", "credential", "fill"]])
    spawnSync("git", args, { cwd: top, env: { ...process.env, GIT_CONFIG_GLOBAL: none, GIT_TERMINAL_PROMPT: "0" }, input: "protocol=https\nhost=example.com\n\n", stdio: ["pipe", "ignore", "ignore"] })
  expect(ran()).toEqual(expect.arrayContaining(["alias", "askpass", "clean", "command", "fsmonitor", "gpg", "ext", "ssh"]))
})

test("what the box runs by itself starts none of them", () => {
  const { top, run, ran } = planted()
  for (const command of OWN_COMMANDS) {
    const args = ownArgs(command, top)
    expect(args).toBeDefined()
    run(args!, "protocol=https\nhost=example.com\n\n")
    expect([command.join(" "), ran()]).toEqual([command.join(" "), []])
  }
  expect(git(top, "log", "-1", "--format=%s", "main")).toBe("second")
  expect(git(top, "tag")).toBe("kept")
})

test("what you ask for runs its hooks but no fsmonitor, pager, diff program or signature check", () => {
  const { top, run, ran, forget } = planted()
  for (const command of [["status", "--porcelain"], ["diff"], ["log", "-p", "-1"], ["show"]]) run(askedArgs(command, top)!)
  for (const never of ["fsmonitor", "pager", "external", "command", "gpg"]) expect(ran()).not.toContain(never)
  forget()
  run(askedArgs(["commit", "-qam", "yours", "--no-gpg-sign"], top)!)
  expect(ran()).toContain("hooks-pre-commit")
  expect(ran()).not.toContain("fsmonitor")
  expect(ran()).not.toContain("pager")
})

test("what reaches another repository by itself, only a local path, and never through a transport its settings name", () => {
  const { at, top, run, ran } = planted()
  // A remote its settings name, ext:: and ssh among them, or none: refused.
  for (const command of [["fetch", "origin"], ["fetch", "far"], ["fetch"], ["ls-remote", "origin"], ["pull", "origin"], ["push", "far", "HEAD"], ["fetch", "-q", "ssh://example.invalid/x"],
    ["clone", "--quiet", "ext::sh -c true", join(at, "c")], ["remote", "update"], ["remote", "show", "origin"], ["submodule", "update", "--init"], ["request-pull", "HEAD", "origin"], ["archive", "--remote=origin", "HEAD"], ["maintenance", "run", "--task=prefetch"]])
    expect([command.join(" "), ownArgs(command, top)]).toEqual([command.join(" "), undefined])
  // A local path is fetched, but where its settings rewrite one to ext:: or ssh, nothing runs.
  for (const path of ["rewritten", "rewritten-ssh"]) {
    const args = ownArgs(["fetch", "-q", join(at, path)], top)
    expect(args).toBeDefined()
    run(args!)
  }
  run(ownArgs(["fetch", "-q", "--no-tags", join(at, "source"), "HEAD:refs/kuvo/fetched"], top)!)
  expect(git(top, "log", "-1", "--format=%s", "refs/kuvo/fetched")).toBe("from elsewhere")
  run(ownArgs(["clone", "-q", "--bare", "--local", join(at, "source"), join(at, "copy")], top)!)
  expect(git(join(at, "copy"), "log", "-1", "--format=%s")).toBe("from elsewhere")
  expect(ran()).toEqual([])
  // (What you ask for goes where you said, as your terminal would.)
  expect(askedArgs(["push", "far", "HEAD"], top)).toBeDefined()
})
