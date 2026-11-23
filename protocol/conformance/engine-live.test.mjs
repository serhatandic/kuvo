import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import http from "node:http";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, request, scratch, startBox, until, write } from "./adapter.mjs";

// The pinned OpenCode handles the queue. Only the upstream model is a fixture.
const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
test("a busy OpenCode conversation keeps an edited queue across a resident restart", {
  skip: !pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned binary.",
  timeout: 60000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  fs.mkdirSync(home);
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  const env = isolatedEnv(root);
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const password = "scratch-engine";
  const pin = fs.realpathSync(pinned);
  let modelCalled = false;
  const model = http.createServer((req, res) => {
    if (req.method !== "POST" || req.url !== "/v1/chat/completions") {
      res.writeHead(404).end();
      return;
    }
    req.resume();
    req.on("end", () => {
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.flushHeaders();
      modelCalled = true;
    });
  });
  await new Promise(resolve => model.listen(0, "127.0.0.1", resolve));
  const worker = launch(pin, ["serve", "--port", String(port), "--hostname", "127.0.0.1"], {
    ...env, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password, OPENCODE_DISABLE_MODELS_FETCH: "1",
    OPENCODE_CONFIG: path.join(root, "opencode.json"), XDG_DATA_HOME: path.join(root, "opencode-data"),
    XDG_CONFIG_HOME: path.join(root, "opencode-config"), XDG_CACHE_HOME: path.join(root, "opencode-cache"),
  }, home);
  write(path.join(root, "opencode.json"), JSON.stringify({ provider: { lab: {
    name: "Test", npm: "@ai-sdk/openai-compatible",
    options: { baseURL: "http://127.0.0.1:" + model.address().port + "/v1", apiKey: "scratch" },
    models: { "lab-one": { name: "Held test model" } },
  } } }));
  const relay = await localRelay();
  const engine = { port, password };
  let box;
  try {
    await until(async () => (await request(engine, "GET", "api/session?limit=1").catch(() => null))?.status === 200,
      () => "Pinned OpenCode readiness: " + worker.log(), 20000);
    write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version: "2.2.6" }));
    const binary = boxBinary();
    const start = () => startBox({ binary, home, env: { ...env, KUVO_SANDBOX: "off" }, relay });
    box = start();
    let record = await box.ready();
    const started = await request(engine, "POST", "api/session", {
      title: "Scratch live queue", location: { directory: home }, model: { providerID: "lab", id: "lab-one" },
    });
    assert.equal(started.status, 200, started.text);
    const id = started.json.data.id;
    const base = "kuvo/sessions/" + id;
    const send = async (number, words, delivery) => {
      const reply = await request(record, "POST", base + "/send", { id: "msg_live" + number, words, delivery });
      assert.equal(reply.status, 200, reply.text);
    };
    const inbox = async expected => {
      let rows;
      await until(async () => {
        const reply = await request(record, "GET", base + "/queue?items=1");
        assert.equal(reply.status, 200, reply.text);
        rows = reply.json.items?.map(row => row.id);
        return rows?.join() === expected.join();
      }, () => `Expected queue ${expected}, last read ${rows}`);
    };
    await send(1, "Start a held step", "steer");
    await until(() => modelCalled, () => "the local test model to be called: " + worker.log(), 15000);
    await send(2, "Wrong second message", "queue");
    await send(3, "Unneeded third message", "queue");
    await inbox(["msg_live2", "msg_live3"]);
    assert.equal((await request(record, "POST", base + "/queue/msg_live2/steer")).status, 200);
    const edited = await request(record, "DELETE", base + "/queue/msg_live2?receipt=msg_edit");
    assert.equal(edited.status, 200, edited.text);
    await send(4, "Correct second message", "queue");
    const removed = await request(record, "DELETE", base + "/queue/msg_live3?receipt=msg_remove");
    assert.equal(removed.status, 200, removed.text);
    await box.stop();
    box = start();
    record = await box.ready();
    await send(4, "Correct second message", "queue");
    await inbox(["msg_live4"]);
    assert.equal((await request(record, "POST", base + "/stop")).status, 200);
  } finally {
    try { await box?.stop(); }
    finally {
      await worker.stop();
      await relay.close();
      model.closeAllConnections();
      await new Promise(resolve => model.close(resolve));
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
    }
  }
});
