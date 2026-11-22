// A real private box services endpoint answers while the artifact journal's disk read is stalled.
import "./safety"
import { expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { createHash } from "node:crypto"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { servicesAuthorization } from "../src/shared/box-services"

async function lineFrom(stream: ReadableStream<Uint8Array>) {
  const reader = stream.getReader()
  let text = ""
  try {
    while (!text.includes("\n")) {
      const next = await reader.read()
      if (next.done) throw new Error("The child stopped before it was ready.")
      text += Buffer.from(next.value).toString("utf8")
    }
    return text.split("\n")[0]!
  } finally { reader.releaseLock() }
}

test("saved usage is available while the old artifact journal is still being read", async () => {
  const box = mkdtempSync(join(tmpdir(), "startup-backfill-"))
  const run = join(box, "run")
  const artifacts = join(box, "artifacts")
  const journal = join(artifacts, "log", "2026-01.jsonl")
  mkdirSync(run)
  mkdirSync(join(box, "bots"))
  mkdirSync(join(artifacts, "log"), { recursive: true })
  expect(Bun.spawnSync(["mkfifo", journal]).exitCode).toBe(0)
  const report = join(box, "report.txt")
  const text = "Archived report"
  const digest = (value: string) => createHash("sha256").update(value).digest("hex")
  const hash = digest(text)
  const id = "art_" + digest(report).slice(0, 24)
  const copies = join(artifacts, "copies", hash.slice(0, 2))
  mkdirSync(copies, { recursive: true })
  writeFileSync(join(copies, hash), text)

  const script = join(import.meta.dir, "../src/link/services.ts")
  const child = Bun.spawn([process.execPath, "--config=/dev/null", "--no-env-file", "-e", `
    import { serveBoxServices } from ${JSON.stringify(script)}
    import { readServices } from ${JSON.stringify(join(import.meta.dir, "../src/shared/box-services.ts"))}
    serveBoxServices(${JSON.stringify({ box, run, artifacts, home: join(box, "agents"), bots: join(box, "bots") })})
    console.log(JSON.stringify(readServices(${JSON.stringify(run)})))
  `], {
    env: { ...process.env, KUVO_HOME: box },
    stdout: "pipe",
    stderr: "inherit",
  })
  // Opening the pipe can block for good in a broken run. Keep both ends in child processes
  // so their exit releases the rendezvous as well as the waiting read.
  const writer = Bun.spawn(["/bin/sh", "-c", 'exec 3>"$1"; printf "open\n"; exec cat >&3', "journal", journal], {
    stdin: "pipe",
    stdout: "pipe",
  })
  try {
    const { port, password } = JSON.parse(await lineFrom(child.stdout))
    const request = (route: string) => fetch(`http://127.0.0.1:${port}/api/kuvo/${route}`, {
      headers: { authorization: servicesAuthorization(password) },
      signal: AbortSignal.timeout(2_000),
    })
    // The rendezvous happens only when the journal reader opens the pipe. Keep it open so that
    // replay cannot make progress until after the real Usage request gets its answer.
    expect(await lineFrom(writer.stdout)).toBe("open")
    const usage = await request("usage")
    expect(usage.status).toBe(200)
    expect(await usage.json()).toHaveProperty("data")

    writer.stdin.write(JSON.stringify({
      a: { id, path: report, name: "report.txt", kind: "document", type: "text/plain",
        where: { bots: [], sessions: ["ses_saved"], groups: [] }, pinned: null, gone: null, updated: "2026-01-01T00:00:00.000Z" },
      v: { n: 1, at: "2026-01-01T00:00:00.000Z", by: { kind: "agent" }, via: ["show_file"], status: "added",
        edits: 1, size: Buffer.byteLength(text), hash: "sha256:" + hash, kept: true },
    }) + "\n")
    await writer.stdin.end()
    expect(await writer.exited).toBe(0)

    const catalog = await request("link/artifacts?session=ses_saved")
    expect(catalog.status).toBe(200)
    expect(await catalog.json()).toMatchObject({ artifacts: [{ id, name: "report.txt" }] })
    const storage = await request("link/artifacts/storage")
    expect(await storage.json()).toMatchObject({ copies: Buffer.byteLength(text) })
    const library = await request("link/library")
    expect(library.status).toBe(200)
    const item = { id: "lib_" + digest(report).slice(0, 24), name: "report.txt", how: "shown", there: true }
    expect(await library.json()).toMatchObject({ items: [item] })
    expect(await (await request(`link/library/${item.id}/file`)).text()).toBe(text)
  } finally {
    if (writer.exitCode === null) writer.kill("SIGTERM")
    if (child.exitCode === null) child.kill("SIGTERM")
    await Promise.all([writer.exited, child.exited])
    rmSync(box, { recursive: true, force: true })
  }
}, 15_000)
