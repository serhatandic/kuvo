// Read the recorded beta inventory through the resident and the pinned OpenCode API.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { generateCurrent } from "./compat/generate.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
test("the recorded beta session pages and saved queue open through the resident", {
  skip: !pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned binary.",
  timeout: 60000,
}, async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 }));
  const { home } = generateCurrent(path.join(root, "source"));
  privateHome(home);
  const env = isolatedEnv(root);
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const password = "golden-inventory";
  const worker = launch(fs.realpathSync(pinned), ["serve", "--port", String(port), "--hostname", "127.0.0.1"], {
    ...env, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password, OPENCODE_DISABLE_MODELS_FETCH: "1",
    XDG_DATA_HOME: path.join(home, "data"), XDG_CONFIG_HOME: path.join(root, "config"),
    XDG_CACHE_HOME: path.join(root, "cache"), XDG_STATE_HOME: path.join(root, "state"),
  }, home);
  const relay = await localRelay();
  let box;
  try {
    await until(async () => (await request({ port, password }, "GET", "api/session?limit=1").catch(() => null))?.status === 200,
      () => "Pinned OpenCode readiness: " + worker.log(), 20000);
    write(path.join(home, "run/engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version: "2.2.6" }));
    box = startBox({ binary: boxBinary(), home, env, relay });
    const resident = await box.ready();
    const queue = await request(resident, "GET", "kuvo/sessions/ses_golden/queue");
    assert.equal(queue.status, 200, queue.text);
    assert.equal(queue.json.paused, true);
    assert.deepEqual(queue.json.held, []);
    assert.deepEqual(queue.json.waiting.map(item => item.id), ["msg_goldenqueued"]);

    let route = "api/session?limit=1&order=desc";
    const titles = [];
    for (let page = 0; page < 5; page++) {
      const reply = await request(resident, "GET", route);
      assert.equal(reply.status, 200, `Page ${page + 1}: ${reply.text}`);
      assert.equal(reply.json.data.length, 1);
      const [session] = reply.json.data;
      assert.ok(path.isAbsolute(session.location.directory), session.location.directory);
      titles.push(session.title);
      route = "api/session?limit=1&cursor=" + encodeURIComponent(reply.json.cursor.next);
    }
    assert.deepEqual(titles, ["plain", "in worktree", "chat (fork #1)", "git edit", "chat"]);
  } finally {
    try { await box?.stop(); }
    finally {
      await worker.stop();
      await relay.close();
    }
  }
});
