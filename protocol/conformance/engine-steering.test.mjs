// The old engine's native step inbox with the pinned, unmodified OpenCode and
// a labelled loopback model: the box makes the same next-step request it makes
// for the other agent kinds, and the next model call receives it.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import test from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { STAND_IN, standInModel, standInSettings } from "./opencode-model.mjs";

const pin = process.env.KUVO_CONFORMANCE_OPENCODE;
test("an engine conversation receives its waiting steer after a tool completes, while still working", {
  timeout: 90000, skip: !pin && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode.",
}, async t => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  privateHome(home);
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const model = await standInModel([
    { tools: [{ name: "question", args: { questions: [{ question: "Which size?", header: "Size", options: [{ label: "Small", description: "A little" }, { label: "Large", description: "A lot" }] }] } }] },
    messages => JSON.stringify(messages).includes("Also check the notes.") ? { text: "Heard the steer." } : { hang: true },
  ]);
  const config = path.join(root, "config");
  write(path.join(config, "opencode", "opencode.json"), JSON.stringify(standInSettings(model.url)));
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const password = "scratch-engine";
  const env = { ...isolatedEnv(root), KUVO_SANDBOX: "off", OPENCODE_DISABLE_MODELS_FETCH: "1" };
  const worker = launch(fs.realpathSync(pin), ["serve", "--port", String(port), "--hostname", "127.0.0.1"], {
    ...env, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password,
    XDG_DATA_HOME: path.join(root, "data"), XDG_CONFIG_HOME: config, XDG_CACHE_HOME: path.join(root, "cache"), XDG_STATE_HOME: path.join(root, "state"),
  }, home);
  const relay = await localRelay();
  const engine = { port, password };
  let box;
  t.after(async () => {
    await box?.stop();
    await worker.stop();
    await relay.close();
    await model.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  });
  await until(async () => (await request(engine, "GET", "api/session?limit=1").catch(() => null))?.status === 200, "Pinned engine readiness", 30000);
  write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version: "2.2.6" }));
  const made = await request(engine, "POST", "api/session", { title: "Steering", location: { directory: home }, model: STAND_IN });
  assert.equal(made.status, 200, made.text);
  const id = made.json.data.id;
  box = startBox({ binary: boxBinary(), home, env, relay });
  const listener = await box.ready();
  const base = `kuvo/sessions/${id}`;
  const call = (method, suffix, body) => request(listener, method, base + suffix, body, { "x-kuvo-from": "app" });
  assert.equal((await call("POST", "/send", { id: "msg_first", words: "Ask for a size.", delivery: "steer" })).status, 200);
  const ask = await until(async () => (await call("GET", "/asks")).json?.asks?.find(each => each.kind === "question"), "The question never came", 30000);
  const sent = await call("POST", "/send", { id: "msg_second", words: "Also check the notes.", delivery: "steer" });
  assert.equal(sent.status, 200, sent.text);
  assert.ok(!model.requests.some(asked => JSON.stringify(asked.messages).includes("Also check the notes.")));
  const answered = await call("POST", `/asks/${encodeURIComponent(ask.id)}`, { answers: { [ask.questions[0].key]: "Large" } });
  assert.ok(answered.status < 300, answered.text);
  await until(() => model.requests.some(asked => JSON.stringify(asked.messages).includes("Also check the notes.")), "The next model step did not hear the steer", 30000);
});
