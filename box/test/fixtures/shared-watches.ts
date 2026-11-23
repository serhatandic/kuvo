import "../safety"
import assert from "node:assert/strict"
import fs from "node:fs"
import { EventEmitter } from "node:events"
import threads from "node:worker_threads"

const { engineHandler } = await import("../../src/link/server/router")
const { BOTS, RUN } = await import("../../src/link/home")
const { HELD } = await import("../../src/link/bots/held")
const { SEEN_FOLDER } = await import("../../src/link/bots/seen")
const { ACTIVITY_FOLDER } = await import("../../src/link/work/activity-peers")

const folders = new Set([BOTS, HELD, SEEN_FOLDER, ACTIVITY_FOLDER, RUN])
const counts = new Map<string, number>()
const record = (folder: string) => counts.set(folder, (counts.get(folder) ?? 0) + 1)
const gate = new Int32Array(new SharedArrayBuffer(4))
const allowOpen = () => {
  Atomics.store(gate, 0, 1)
  Atomics.notify(gate, 0)
}
const opening = Promise.withResolvers<void>()
const opened = Promise.withResolvers<void>()
const closed = Promise.withResolvers<void>()
let remaining = folders.size

// Stand in for the filesystem call in either thread. On the old link every follower opens
// its own watches in the dispatcher's thread.
const watch = fs.watch
fs.watch = ((folder, ...args) => {
  if (folders.has(String(folder))) {
    record(String(folder))
    return Object.assign(new EventEmitter(), { close() {} })
  }
  return Reflect.apply(watch, fs, [folder, ...args])
}) as typeof fs.watch
const Worker = threads.Worker
threads.Worker = class extends Worker {
  constructor(code: string, options?: threads.WorkerOptions) {
    const preamble = `
      {
        const fs = require("node:fs");
        const { parentPort, workerData } = require("node:worker_threads");
        const { EventEmitter } = require("node:events");
        const original = fs.watch;
        const folders = new Set(${JSON.stringify([...folders])});
        fs.watch = (folder, ...args) => {
          if (!folders.has(folder)) return original(folder, ...args);
          parentPort.postMessage({ watchTest: "opening", folder });
          if (Atomics.wait(workerData.gate, 0, 0, 10000) === "timed-out") Atomics.store(workerData.gate, 0, 2);
          parentPort.postMessage({ watchTest: "opened" });
          return Object.assign(new EventEmitter(), {
            close() { parentPort.postMessage({ watchTest: "closed" }); },
          });
        };
      }
    `
    if (options?.eval) {
      super(preamble + code, { ...options, workerData: { gate } })
      this.on("message", message => {
        switch (message.watchTest) {
          case "opening": record(message.folder); opening.resolve(); break
          case "opened": if ([...folders].every(folder => counts.has(folder))) opened.resolve(); break
          case "closed": if (--remaining === 0) closed.resolve(); break
        }
      })
      this.on("error", opening.reject)
    } else {
      super(code, options)
    }
  }
}

const noEngine = () => { throw new Error("No engine expected") }
const handler = engineHandler(noEngine, { name: "test" }, noEngine)
function follow() {
  const stop = new AbortController()
  const stream = handler({ method: "GET", path: "kuvo/bots/events", headers: {}, fromApp: true }, undefined, stop.signal)
  return {
    async first() {
      const head = (await stream.next()).value
      assert.ok(head && "head" in head)
      assert.equal(head.head.status, 200)
      const first = (await stream.next()).value
      assert.ok(first && "body" in first)
      assert.ok(Array.isArray(JSON.parse(Buffer.from(first.body).toString()).bots))
    },
    end() { stop.abort(); return stream.return(undefined) },
  }
}
const followers = Array.from({ length: 20 }, follow)
const end = () => Promise.all(followers.map(follower => follower.end()))

try {
  await Promise.all(followers.map(follower => follower.first()))
  assert.ok([...counts.values()].every(count => count === 1), "A folder was opened more than once for 20 bot followers")
  await opening.promise
  const answer = []
  for await (const piece of handler({ method: "GET", path: "kuvo/bots", headers: {}, fromApp: true }, undefined)) answer.push(piece)
  assert.equal(Atomics.load(gate, 0), 0)
  assert.ok("head" in answer[0]! && answer[0].head.status === 200)
  allowOpen()
  await opened.promise
  await end()
  const reconnected = follow()
  try {
    await reconnected.first()
  } finally {
    await reconnected.end()
  }
  await closed.promise
  assert.deepEqual(counts, new Map([...folders].map(folder => [folder, 1])))
} finally {
  allowOpen()
  await end()
}
