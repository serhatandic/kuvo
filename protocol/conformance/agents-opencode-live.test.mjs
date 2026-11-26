// One real OpenCode Go turn on the OpenCode agent, as the app starts and reads it, in a temp home
// with only a non-renewing OpenCode Go key (copied in for the run) and the pinned OpenCode.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const credential = process.env.KUVO_CONFORMANCE_OPENCODE_GO;
const model = { providerID: "opencode-go", id: "deepseek-v4.1-flash" };
const headers = { "x-kuvo-from": "app" };

test("a real OpenCode Go turn on the OpenCode agent: the app's new conversation, its words, and nothing left running", {
  skip: !(pinned && credential) && "Set KUVO_CONFORMANCE_OPENCODE and KUVO_CONFORMANCE_OPENCODE_GO to isolated test inputs.",
  timeout: 240000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const work = path.join(root, "work");
  privateHome(home);
  fs.mkdirSync(work, { recursive: true });
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const program = path.join(home, "engine", "2.2.6", process.platform === "win32" ? "opencode.exe" : "opencode");
  fs.mkdirSync(path.dirname(program), { recursive: true });
  fs.symlinkSync(fs.realpathSync(pinned), program);
  const env = isolatedEnv(root);
  const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
  for (const folder of Object.values(dirs)) fs.mkdirSync(folder, { recursive: true });
  const relay = await localRelay();
  let seeder, box;
  try {
    const given = JSON.parse(fs.readFileSync(credential, "utf8"));
    if (given.integrationID !== model.providerID || given.value?.type !== "key" || given.value.refresh) {
      throw new Error("Use only a non-renewing OpenCode Go key in the scratch credential file.");
    }
    // The key put in the home's OpenCode as a sign-in would, by a server of its own that then stops.
    const reserve = net.createServer();
    await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
    const port = reserve.address().port;
    await new Promise(resolve => reserve.close(resolve));
    const password = "scratch-seed";
    seeder = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
      { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, work);
    const engine = { port, password };
    await until(async () => (await request(engine, "GET", "api/provider").catch(() => null))?.status === 200, "The seeding OpenCode", 30000);
    const seeded = await request(engine, "POST", "api/credential", { integrationID: model.providerID, label: "Scratch conformance", value: given.value, activate: true });
    assert.ok(seeded.status === 200 || seeded.status === 201, `Seeding returned ${seeded.status}`);
    await seeder.stop();
    seeder = undefined;

    box = startBox({ binary: boxBinary(), home, env: { ...env, KUVO_SANDBOX: "off" }, relay });
    const listener = await box.ready();
    const call = (method, route, body) => request(listener, method, route, body, headers, 60000);
    const started = await call("POST", "kuvo/sessions", { folder: work, model, approval: "askFirst" });
    assert.equal(started.status, 200, started.text);
    const id = started.json.info?.id ?? started.json.id;
    const where = await call("GET", `kuvo/sessions/${id}/location`);
    assert.ok(JSON.stringify(where.json).includes("agents"), where.text);
    const sent = await call("POST", `kuvo/sessions/${id}/send`, { id: `msg_${Date.now().toString(36)}live`, words: "Reply with only the word READY.", delivery: "steer" });
    assert.equal(sent.status, 200, sent.text);
    let rows = [];
    await until(async () => {
      rows = (await call("GET", `kuvo/agents/api/session/${id}/message?limit=50&order=asc`)).json?.data ?? [];
      return rows.some(row => row.type === "idle");
    }, () => `The turn never ended: ${JSON.stringify(rows).slice(-1200)}`, 180000, 1000);
    const said = rows.filter(row => row.type === "assistant").flatMap(row => row.content ?? []).filter(part => part.type === "text").map(part => part.text).join("");
    assert.match(said, /READY/i, said);
    assert.equal(rows.filter(row => row.type === "idle").at(-1).outcome ?? "succeeded", "succeeded");
    // OpenCode ran as the agent, never as the engine's server.
    assert.ok(!fs.existsSync(path.join(home, "run", "engine.json")), "The engine's OpenCode started.");
  } finally {
    await seeder?.stop?.();
    await box?.stop();
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
});
