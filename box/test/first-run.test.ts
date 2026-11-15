// Whether you've had Kuvo Code's first run with a box (#733, PROTOCOL.md "Your setup", `welcomed`):
// kept on the box, so every client of it agrees, and a client installed again doesn't welcome you
// again. Through the box's own routes, as a client asks them.

import "./safety"
import { expect, test } from "bun:test"
import { existsSync } from "node:fs"
import { join } from "node:path"
import { RUN } from "../src/link/home"

test("the first run is done once a client says so, for every client of the box, and can be had again", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const handler = engineHandler(() => { throw new Error("no engine") }, { name: "test" }, () => { throw new Error("none") })
  const ask = async (method: string, path: string, body?: unknown, from: { fromApp?: boolean } = {}) => {
    let status = 0, text = ""
    for await (const piece of handler({ method, path, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), ...from }, undefined)) {
      if ("head" in piece) status = piece.head.status
      if ("body" in piece) text += Buffer.from(piece.body).toString()
    }
    return { status, body: JSON.parse(text || "{}") }
  }
  const welcomed = async () => (await ask("GET", "kuvo/setup")).body.welcomed

  expect(await welcomed()).toBe(false)
  // The desktop app finishes its first run: the phone, asking the same box, isn't welcomed again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true }, { fromApp: true })).toEqual({ status: 200, body: { welcomed: true } })
  expect(await welcomed()).toBe(true)
  expect(existsSync(join(RUN, "welcomed.json"))).toBe(true)
  // Said again, it stays as it was.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true })).toEqual({ status: 200, body: { welcomed: true } })
  // Refused: nothing said, or not a yes or no; and only POST.
  expect((await ask("POST", "kuvo/setup/welcomed", {})).status).toBe(400)
  expect((await ask("POST", "kuvo/setup/welcomed", { welcomed: "yes" })).status).toBe(400)
  expect((await ask("GET", "kuvo/setup/welcomed")).status).toBe(405)
  expect(await welcomed()).toBe(true)
  // A client following the setup hears the change as it's said.
  const following = new AbortController()
  const lines: any[] = []
  const heard = (async () => {
    let text = ""
    for await (const piece of handler({ method: "GET", path: "kuvo/setup/events", headers: {}, body: undefined }, undefined, following.signal)) {
      if (!("body" in piece)) continue
      text += Buffer.from(piece.body).toString()
      const done = text.split("\n").slice(0, -1)
      text = text.slice(text.lastIndexOf("\n") + 1)
      lines.push(...done.filter(Boolean).map(line => JSON.parse(line)))
      if (lines.length === 1) await ask("POST", "kuvo/setup/welcomed", { welcomed: false })
      if (lines.length >= 2) break
    }
    following.abort()
  })()
  await heard
  expect(lines.map(line => line.welcomed)).toEqual([true, false])
  expect(await welcomed()).toBe(false)
  // Said again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: true })).toEqual({ status: 200, body: { welcomed: true } })
  // To be had again.
  expect(await ask("POST", "kuvo/setup/welcomed", { welcomed: false })).toEqual({ status: 200, body: { welcomed: false } })
  expect(await welcomed()).toBe(false)
  expect(existsSync(join(RUN, "welcomed.json"))).toBe(false)
})
