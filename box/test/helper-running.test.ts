// A Mac's own box whose Kuvo Helper isn't running is started by the app: the installer's verdict
// counts the helper in place only while launchd has it running, not merely loaded. (Seen on a Mac
// whose helper had ended cleanly: launchd starts it again only after a crash, the app found it
// "in place", ran nothing, and waited for a box that never answered.)
import "./safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { helperProgram } from "../src/link/helper"
import { helperRuns } from "../src/link/install/command"
import { LAUNCH_AGENT } from "../src/link/marks"

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-helper-running-")))
const saved = { HOME: process.env.HOME, tools: process.env.KUVO_TEST_UNINSTALL_TOOLS }
afterEach(() => {
  process.env.HOME = saved.HOME
  if (saved.tools === undefined) delete process.env.KUVO_TEST_UNINSTALL_TOOLS
  else process.env.KUVO_TEST_UNINSTALL_TOOLS = saved.tools
})
afterAll(() => fs.rmSync(root, { recursive: true, force: true }))

/** launchd, as a stand-in: the job loaded, and what `launchctl print` says of it (`state`). */
function launchd(state: string | null) {
  const home = path.join(root, "home"), tools = path.join(root, "tools")
  fs.mkdirSync(path.join(home, "Library", "LaunchAgents"), { recursive: true })
  fs.mkdirSync(tools, { recursive: true })
  fs.writeFileSync(path.join(home, "Library", "LaunchAgents", `${LAUNCH_AGENT}.plist`), `<plist><dict><key>ProgramArguments</key><array><string>${helperProgram()}</string></array></dict></plist>\n`)
  fs.writeFileSync(path.join(tools, "launchctl"), state === null ? "#!/bin/sh\nexit 113\n" : `#!/bin/sh\n[ "$1" = print ] && printf '%s\\n' "gui/501/${LAUNCH_AGENT} = {" "	state = ${state}" "}"\nexit 0\n`, { mode: 0o755 })
  process.env.HOME = home
  process.env.KUVO_TEST_UNINSTALL_TOOLS = tools
}

test.if(process.platform === "darwin")("a helper launchd has loaded but that has ended isn't in place, so the app's installer starts it", () => {
  launchd("not running")
  expect(helperRuns()).toBe(false)
})

test.if(process.platform === "darwin")("a running helper is in place: nothing is started again", () => {
  launchd("running")
  expect(helperRuns()).toBe(true)
})

test.if(process.platform === "darwin")("one launchd doesn't have at all isn't in place either", () => {
  launchd(null)
  expect(helperRuns()).toBe(false)
})
