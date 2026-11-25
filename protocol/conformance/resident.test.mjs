// Local clients discover and authenticate to a resident, follow its line, and reattach on restart.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openLine, request } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("resident publishes usable credentials for kuvo/info", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("info");
  const record = await here.start().ready();
  const denied = await request({ ...record, password: "bad password" }, "GET", "kuvo/info");
  assert.equal(denied.status, 401);
  const info = await request(record, "GET", "kuvo/info");
  assert.equal(info.status, 200);
  assert.equal(info.json.kind, "machine");
  assert.equal(info.json.chats, path.join(here.home, "chats"));
  assert.ok(info.json.features.includes("info"));
});

test("resident has one owner, streams numbered changes and publishes new credentials after restart", { timeout: 90000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("resident");
  const first = here.start();
  const record = await first.ready();
  const duplicate = here.start();
  const refused = await duplicate.exited;
  assert.equal(refused.code, 1, duplicate.log());
  const watch = Buffer.from(JSON.stringify({ example: "api/event" })).toString("base64url");
  const line = await openLine(record, "?watch=" + watch);
  try {
    assert.equal(line.status, 200);
    assert.match(line.type, /ndjson/);
    assert.equal(Number(fs.readFileSync(path.join(here.home, "run", "link.pid"), "utf8")), record.pid);
    const frames = line.frames();
    const { value: { hello } } = await frames.next();
    assert.equal(hello.fresh, true);
    assert.equal(typeof hello.line, "string");
    const { value: initial } = await frames.next();
    assert.deepEqual(initial, { t: "example", d: { type: "server.connected" }, n: 1 });
    here.engine.emit({ type: "conformance.event", data: { text: "Hello" } });
    const changed = { t: "example", d: { type: "conformance.event", data: { text: "Hello" } }, n: 2 };
    assert.deepEqual((await frames.next()).value, changed);
    const resumed = await openLine(record, `?watch=${watch}&line=${hello.line}&after=1`);
    try {
      const replay = resumed.frames();
      assert.deepEqual((await replay.next()).value.hello, { ...hello, fresh: false });
      assert.deepEqual((await replay.next()).value, changed);
    } finally {
      resumed.close();
    }
  } finally {
    line.close();
  }
  const stopped = await first.stop();
  if (process.platform === "win32") {
    // Node's Windows SIGTERM forcibly terminates the child; unlike a Unix
    // signal it cannot run the resident's shutdown handler. Reopening a home
    // with that stale discovery record is the Windows restart case here.
    assert.equal(stopped.signal, "SIGTERM", first.log());
  } else {
    assert.equal(stopped.code, 0, first.log());
    assert.equal(fs.existsSync(path.join(here.home, "run", "serve.json")), false);
  }
  const restarted = await here.start().ready();
  assert.notEqual(restarted.password, record.password);
  assert.equal((await request(restarted, "GET", "kuvo/info")).status, 200);
});
