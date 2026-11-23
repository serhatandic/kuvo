// Real bot streams and route dispatch, with a filesystem watch whose open is held in a worker.
import "./safety"
import { expect, test } from "bun:test"
import { join } from "node:path"

test("20 bot followers share their watches and the box answers while a watch opens", async () => {
  const child = Bun.spawn([process.execPath, "--config=/dev/null", "--no-env-file", join(import.meta.dir, "fixtures", "shared-watches.ts")], {
    stdout: "pipe", stderr: "pipe",
  })
  try {
    const [code, error] = await Promise.all([child.exited, new Response(child.stderr).text()])
    expect({ code, error }).toEqual({ code: 0, error: "" })
  } finally {
    if (child.exitCode === null) child.kill()
  }
})
