// OpenCode conversations made before OpenCode was a coding agent here: a home where the reference
// box made one on its engine (the pinned OpenCode, a labelled stand-in model), then the box under
// test on a copy of that home. The Rust box lists it from OpenCode's database without starting
// OpenCode, reads it from OpenCode's export the first time it's opened, and continues it on the
// OpenCode agent; the reference does all three on its engine. The app must see the same.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { STAND_IN, standInModel, standInSettings } from "./opencode-model.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const reference = process.env.KUVO_CONFORMANCE_REFERENCE ?? new URL("../../box/dist/kuvo-link.js", import.meta.url).pathname;
const version = "2.2.6";
const headers = { "x-kuvo-from": "app" };
const skip = (!pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode (used in place).")
  || (!fs.existsSync(reference) && "Build the reference (scripts/build-built.sh): it makes the home.")
  || (process.platform === "win32" && "The reference makes the home, and it doesn't run on Windows.");

// (The engine's rows are `{ info, parts }`, the agents' `{ type, content }`: the app reads both.)
const role = row => row.info?.role ?? row.role ?? row.type;
const parts = row => row.parts ?? row.content ?? [];
// (OpenCode's own rows keep a message's words in `text` too.)
const text = (rows, who) => rows.filter(row => role(row) === who)
  .flatMap(row => [...(typeof row.text === "string" ? [row.text] : []), ...parts(row).filter(part => part.type === "text").map(part => part.text)])
  .join("|");

describe("OpenCode conversations from before", { skip }, () => {
  const binary = boxBinary();
  let root, made, home, work, model, relay, env, box, listener, engine;
  let id, earlier;
  const script = [];

  /** OpenCode's server on `at`, as the reference's box runs it. */
  async function engineOn(at) {
    const reserve = net.createServer();
    await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
    const port = reserve.address().port;
    await new Promise(resolve => reserve.close(resolve));
    const password = "scratch-engine";
    const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(at, sub)]));
    for (const folder of [...Object.values(dirs), path.join(at, "workspace")]) fs.mkdirSync(folder, { recursive: true });
    const worker = launch(path.join(at, "engine", version, "opencode"), ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
      { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, path.join(at, "workspace"));
    await until(async () => (await request({ port, password }, "GET", "api/session?limit=1").catch(() => null))?.status === 200, "The reference's engine", 30000);
    write(path.join(at, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version }));
    return worker;
  }

  /** `which` box on `at`, with an engine for the reference. */
  async function serve(which, at) {
    const worker = which.typescript ? await engineOn(at) : null;
    const started = startBox({ binary: which, home: at, env, relay });
    return { box: started, listener: await started.ready(), worker };
  }

  const call = (method, route, body, timeout = 30000) => request(listener, method, route, body, headers, timeout);

  async function prefix(of) {
    let found;
    await until(async () => (found = await call("GET", `kuvo/sessions/${of}/location`)).status !== 503,
      () => `Its location never answered: ${found?.text}`, 30000, 200);
    assert.equal(found.status, 200, found.text);
    return JSON.stringify(found.json).includes("agents") ? "kuvo/agents/" : "";
  }

  async function messages(of) {
    const read = await call("GET", `${await prefix(of)}api/session/${of}/message?limit=50&order=asc`);
    assert.equal(read.status, 200, read.text);
    return read.json.data ?? read.json;
  }

  async function send(of, words) {
    const sent = await call("POST", `kuvo/sessions/${of}/send`, { id: `msg_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, words, delivery: "steer" });
    assert.equal(sent.status, 200, sent.text);
  }

  async function answered(of, words) {
    let rows = [];
    await until(async () => text(rows = await messages(of), "assistant").includes(words),
      () => `It never said "${words}": ${JSON.stringify(rows).slice(-1200)}\n${box?.log?.() ?? ""}`, 90000, 250);
    return rows;
  }

  /** OpenCode's processes running on `at`, by their command lines. */
  const openCodeOn = at => execFileSync("ps", ["-ax", "-o", "command="], { encoding: "utf8" })
    .split("\n").filter(line => line.includes(at) && /opencode/.test(line) && !line.includes("kuvo-box"));

  before(async () => {
    root = fs.realpathSync(scratch());
    made = path.join(root, "made");
    home = path.join(root, "box");
    work = path.join(root, "work");
    privateHome(made);
    fs.mkdirSync(work, { recursive: true });
    write(path.join(made, "folder-version.json"), '{"version":23}\n');
    const program = path.join(made, "engine", version, "opencode");
    fs.mkdirSync(path.dirname(program), { recursive: true });
    fs.symlinkSync(fs.realpathSync(pinned), program);
    model = await standInModel(script);
    write(path.join(made, "config", "opencode", "opencode.json"), JSON.stringify(standInSettings(model.url)));
    write(path.join(made, "box", "opencode.json"), "{}\n");
    env = { ...isolatedEnv(root), KUVO_SANDBOX: "off", OPENCODE_DISABLE_MODELS_FETCH: "1" };
    relay = await localRelay();

    // The reference makes the conversation on its engine: two turns.
    ({ box, listener, worker: engine } = await serve(boxBinary(reference), made));
    const started = await call("POST", "kuvo/sessions", { folder: work, model: STAND_IN });
    assert.equal(started.status, 200, started.text);
    id = started.json.info?.id ?? started.json.id;
    script.push({ text: "First answer." }, { text: "Second answer." });
    await send(id, "First message.");
    await answered(id, "First answer.");
    await send(id, "Second message.");
    earlier = await answered(id, "Second answer.");
    await until(async () => !JSON.stringify((await call("GET", "kuvo/sessions/list")).json).includes('"running":true'), "its turn ended", 30000);
    await box.stop();
    await engine.stop();
    engine = null;
    fs.rmSync(path.join(made, "run", "engine.json"), { force: true });

    // The box under test on a copy of that home.
    fs.cpSync(made, home, { recursive: true, verbatimSymlinks: true });
    ({ box, listener, worker: engine } = await serve(binary, home));
  });

  after(async () => {
    await box?.stop();
    for (const at of [made, home]) if (at) await stopFixtureHolders(at);
    await engine?.stop?.();
    await relay?.close();
    await model?.close();
    if (root) fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  });

  test("it's listed with its title, and listing starts no OpenCode", { timeout: 60000 }, async () => {
    let list;
    await until(async () => JSON.stringify((list = await call("GET", "kuvo/sessions/list")).json).includes(id),
      () => `It isn't listed: ${list?.text?.slice(0, 1500)}`, 30000, 250);
    if (!binary.typescript) {
      assert.deepEqual(openCodeOn(home), [], "Listing started OpenCode.");
      assert.ok(!fs.existsSync(path.join(home, "run", "engine.json")), "Listing started OpenCode's server.");
    }
  });

  test("it opens with its whole transcript", { timeout: 90000 }, async () => {
    const rows = await messages(id);
    assert.equal(text(rows, "user"), text(earlier, "user"));
    assert.equal(text(rows, "assistant"), text(earlier, "assistant"));
    assert.equal(rows.length, earlier.length);
    assert.deepEqual(rows.map(row => row.id ?? row.info?.id), earlier.map(row => row.id ?? row.info?.id));
    if (!binary.typescript) assert.deepEqual(openCodeOn(home), [], "Reading it left OpenCode running.");
  });

  test("it goes on with a turn that has all that came before", { timeout: 120000 }, async () => {
    script.push({ text: "Third answer." });
    await send(id, "Third message.");
    const rows = await answered(id, "Third answer.");
    assert.ok(text(rows, "user").includes("Third message."));
    const heard = JSON.stringify(model.requests.filter(asked => asked.tools?.length).at(-1)?.messages ?? []);
    for (const earlier of ["First message.", "First answer.", "Second message.", "Second answer.", "Third message."]) {
      assert.ok(heard.includes(earlier), `The model didn't hear "${earlier}": ${heard.slice(0, 1200)}`);
    }
    assert.ok(text((await messages(id)), "assistant").includes("First answer."), "The older turns went.");
  });
});
