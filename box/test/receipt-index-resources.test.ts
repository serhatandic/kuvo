import "./safety"
import { expect, test } from "bun:test"
import { readdirSync, readlinkSync } from "node:fs"
import { join } from "node:path"
import { RUN } from "../src/link/home"
import { receiptPageKeys } from "../src/link/ownership/receipt-index"

// These are real receipt-page reads from the box's SQLite index. A completed page must release
// its file descriptor immediately so periodic maintenance and further reads can keep answering.
test("receipt pages leave no open index handles behind", () => {
  const file = join(RUN, "owned-data", "receipt-index-v1", "addresses.sqlite")
  const scope = { setup: "set_test", session: "ses_test", author: "device:test", after: "", issuers: ["test"] }
  for (let n = 0; n < 64; n++) {
    expect(receiptPageKeys(scope)).toEqual({ owner: [], listing: [], complete: true })
  }
  const handles = process.platform === "linux"
    ? readdirSync("/proc/self/fd").flatMap(fd => {
      try { return [readlinkSync(`/proc/self/fd/${fd}`)] } catch { return [] }
    })
    : Bun.spawnSync(["/usr/sbin/lsof", "-a", "-p", String(process.pid), "-Fn"]).stdout.toString()
      .split("\n").filter(line => line.startsWith("n")).map(line => line.slice(1))
  expect(handles.filter(path => path === file)).toHaveLength(0)
})
