// A bridge hands an in-flight request to its successor over the real holder socket.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash, createHmac } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, readJSON, scratch, typescriptRuntime, until, write } from "./adapter.mjs";

const id = "0123456789ab";
const key = "fixture-only-key";
const homeKey = Buffer.from("ab".repeat(32), "hex");
const ask = '{"id":17,"method":"fixture/approve"}';
const reply = '{"id":17,"result":{"allow":true}}';
const meta = { kind: "codex", provider: "codex-cli", turns: {} };
const turns = { thread: { session: "ses_held", turn: "turn", from: 0 } };
const boot = { hello: "hold/1", key, generation: 1 };
const windows = process.platform === "win32";
const agent = `
import { createInterface } from "node:readline";
process.on("SIGUSR1", () => console.log("signal received"));
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "begin") {
    console.log("before");
    console.log(${JSON.stringify(ask)});
  } else if (line === ${JSON.stringify(reply)}) {
    console.log("after");
  } else if (line === "diagnostic") {
    process.stderr.write("fixture diagnostic");
  } else {
    process.exit(2);
  }
}
`;

function frames(stream) {
  return createInterface({ input: stream, crlfDelay: Infinity })[Symbol.asyncIterator]();
}

async function read(frames) {
  const result = await frames.next();
  assert.equal(result.done, false, "holder closed before sending the next frame");
  return JSON.parse(result.value);
}

async function withHolder(t, settings, scenario) {
  const binary = boxBinary();
  if (windows && binary.typescript) {
    t.skip("The TypeScript holder uses Unix sockets.");
    return;
  }
  const root = scratch();
  const folder = path.join(root, "run", "agents", "hold");
  const sockets = [];
  const transcript = [];
  let holder;
  try {
    const env = isolatedEnv(root);
    if (windows) {
      write(path.join(root, "run", "socket.key"), homeKey.toString("hex"));
    } else {
      mkdirSync(folder, { recursive: true, mode: 0o700 });
    }
    const worker = path.join(root, "agent.mjs");
    write(worker, settings.agent ?? agent);
    let command = binary.file;
    let args = ["hold", folder, id, settings.kind ?? "jsonrpc"];
    let build;
    if (binary.typescript) {
      // The bridge concatenates these two sources into its standalone holder.
      const source = readFileSync(new URL("../../box/src/shared/local-ipc.js", import.meta.url), "utf8") + "\n" +
        readFileSync(new URL("../../box/src/agents/hold/program.js", import.meta.url), "utf8");
      build = createHash("sha256").update(source).digest("hex").slice(0, 16);
      const program = path.join(folder, `kuvo-hold-${build}.js`);
      write(program, source, { mode: 0o600 });
      command = typescriptRuntime();
      args = ["--no-env-file", program, ...args.slice(1)];
    }
    holder = launch(command, args, { ...env, KUVO_HOME: root, BUN_BE_BUN: "1" }, root, "pipe");
    const readyFrames = frames(holder.child.stdout);
    holder.child.stdin.end(JSON.stringify({
      key, spec: { command: [process.execPath, worker], cwd: root, bridge: process.pid, holder: build, meta,
        orphan: 8000, linger: 3000, ...settings.spec },
    }));
    const ready = await read(readyFrames);
    assert.deepEqual(ready, { ready: "hold/1", pid: holder.child.pid, child: ready.child });
    assert.ok(Number.isInteger(ready.child) && ready.child > 0);
    const recordPath = path.join(folder, id + ".json");
    const record = readJSON(recordPath);
    assert.equal(record.proto, "hold/1");
    assert.equal(record.key, key);
    assert.equal(record.child, ready.child);

    function capture(controller, direction, frame) {
      if (!settings.capture) return;
      const stable = { ...frame };
      if (Object.hasOwn(stable, "pid")) stable.pid = "<holder PID>";
      if (Object.hasOwn(stable, "child")) stable.child = "<agent PID>";
      transcript.push({ controller, direction, line: JSON.stringify(stable) + "\n" });
    }
    capture("original", "holder", ready);

    const address = windows ? windowsAddress() : path.join(folder, id + ".sock");
    function connect(controller) {
      const socket = net.createConnection(address);
      sockets.push(socket);
      socket.setTimeout(3500, () => socket.destroy(new Error("hold/1 socket timed out")));
      socket.on("error", () => {});
      const incoming = frames(socket);
      return {
        closed: () => incoming.next(),
        disconnect: () => socket.destroy(),
        pause: () => socket.pause(),
        send(frame) {
          capture(controller, "controller", frame);
          socket.write(JSON.stringify(frame) + "\n");
        },
        async read() {
          const frame = await read(incoming);
          capture(controller, "holder", frame);
          return frame;
        },
      };
    }

    await scenario({ ready, connect, folder, root, recordPath, holder, capture });
    if (settings.capture) write(settings.capture, JSON.stringify(transcript, null, 2) + "\n", { flag: "wx" });
  } finally {
    for (const socket of sockets) socket.destroy();
    await holder?.stop();
    rmSync(root, { recursive: true, force: true });
  }
}

test("hold/1 lets a new controller finish the same agent's turn",
  { timeout: 15000 }, t => withHolder(t, { capture: process.env.KUVO_CONFORMANCE_HOLDER_TRANSCRIPT },
    async ({ ready, connect, holder, folder, recordPath, capture }) => {
    function welcome(generation, seq, meta) {
      return { hello: "hold/1", pid: ready.pid, child: ready.child, generation, seq, first: 1,
        gap: false, exited: null, stderr: "", meta };
    }

    const original = connect("original");
    original.send(boot);
    assert.deepEqual(await original.read(), welcome(1, 0, meta));
    original.send({ meta: { turns } });
    original.send({ i: "begin" });
    assert.deepEqual(await original.read(), { s: 1, o: "before" });
    assert.deepEqual(await original.read(), { s: 2, o: ask });
    original.send({ ack: 2 });
    original.send({ prepare: 0 });
    assert.deepEqual(await original.read(), { prepared: true });

    const refused = connect("refused");
    refused.send({ ...boot, key: "fixture-wrong-key" });
    assert.deepEqual(await refused.read(), { refused: "Not this holder's key." });

    const replaying = connect("replaying");
    replaying.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.deepEqual(await original.read(), { fenced: true });
    assert.deepEqual(await replaying.read(), welcome(2, 2, { ...meta, turns }));
    assert.deepEqual(await replaying.read(), { s: 1, o: "before" });
    assert.deepEqual(await replaying.read(), { s: 2, o: ask });
    replaying.send({ unpin: true });

    const successor = connect("successor");
    successor.send({ ...boot, generation: 3 });
    assert.deepEqual(await replaying.read(), { fenced: true });
    assert.deepEqual(await successor.read(), welcome(3, 2, { ...meta, turns }));
    assert.deepEqual(await successor.read(), { s: 2, o: ask, again: true });
    successor.send({ i: reply });
    assert.deepEqual(await successor.read(), { s: 3, o: "after" });
    const stale = connect("stale");
    stale.send({ ...boot, generation: 2 });
    assert.deepEqual(await stale.read(), { refused: "A bridge of generation 3 has this agent." });

    const final = connect("final");
    final.send({ ...boot, generation: 4 });
    assert.deepEqual(await successor.read(), { fenced: true });
    assert.deepEqual(await final.read(), welcome(4, 3, { ...meta, turns }));
    // The earlier ask was answered. Only the unacknowledged output comes back.
    assert.deepEqual(await final.read(), { s: 3, o: "after" });
    final.send({ i: "diagnostic" });
    assert.deepEqual(await final.read(), { e: "fixture diagnostic" });
    if (!windows) {
      final.send({ signal: "SIGUSR1" });
      assert.deepEqual(await final.read(), { s: 4, o: "signal received" });
    }
    const journal = readFileSync(path.join(folder, id + ".journal"), "utf8");
    assert.equal(journal, ["before", ask, "after", ...(!windows ? ["signal received"] : [])].join("\n") + "\n");
    const currentRecord = readJSON(recordPath);
    capture("final", "filesystem", { journal, meta: currentRecord.meta, generation: currentRecord.generation });
    final.send({ end: true });
    assert.deepEqual(await final.read(), { x: { code: 0, signal: null } });
    final.send({ bye: true });
    await until(() => holder.child.exitCode !== null, "holder did not leave after bye", 3500);
    assert.deepEqual(await holder.exited, { code: 0, signal: null });
    const remaining = {
      record: existsSync(recordPath),
      journal: existsSync(path.join(folder, id + ".journal")),
      socket: existsSync(path.join(folder, id + ".sock")),
    };
    assert.deepEqual(remaining, { record: false, journal: false, socket: false });
    capture("final", "filesystem", remaining);
}));

test("a complete replay outside the journal leaves the original controller attached",
  { timeout: 15000 }, t => withHolder(t, {
    spec: { journalBytes: 128 },
    agent: `
import { createInterface } from "node:readline";
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "begin") {
    console.log("before");
    console.log("x".repeat(5_000_000));
    console.log("after");
  } else {
    console.log("still here");
  }
}
`,
  }, async ({ connect, folder }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.send({ i: "begin" });
    assert.deepEqual(await original.read(), { s: 1, o: "before" });
    const large = await original.read();
    assert.equal(large.s, 2);
    assert.equal(large.o.length, 5_000_000);
    assert.match(large.o, /^x+$/);
    assert.deepEqual(await original.read(), { s: 3, o: "after" });
    assert.equal(readFileSync(path.join(folder, id + ".journal"), "utf8"), "after\n");

    const replacement = connect();
    replacement.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.deepEqual(await replacement.read(), { refused: "The turn is no longer in the journal." });
    original.send({ i: "ping" });
    assert.deepEqual(await original.read(), { s: 4, o: "still here" });
    await finish(original);
}));

test("Claude's CLI control requests go to the live controller once and are excluded from replay",
  { timeout: 15000 }, t => withHolder(t, {
    kind: "claude",
    agent: agent.replace(JSON.stringify(ask), JSON.stringify('{"type":"control_request","request_id":"ask"}')),
  }, async ({ connect }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.send({ i: "begin" });
    assert.deepEqual(await original.read(), { s: 1, o: "before" });
    assert.deepEqual(await original.read(), { s: 2, o: '{"type":"control_request","request_id":"ask"}' });

    const replacement = connect();
    replacement.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.deepEqual(await original.read(), { fenced: true });
    assert.equal((await replacement.read()).seq, 2);
    assert.deepEqual(await replacement.read(), { s: 1, o: "before" });
    await finish(replacement); // Next frame is exit, with no repeated control request.
}));

test("a pinned turn replays more than four megabytes without dropping any lines",
  { timeout: 15000 }, t => withHolder(t, {
    spec: { journalBytes: 5_100_000 },
    agent: `
import { createInterface } from "node:readline";
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "begin") {
    for (let n = 0; n < 64; n++) console.log(n + ":" + "x".repeat(80_000));
  } else {
    console.log("next turn");
  }
}
`,
  }, async ({ connect, folder }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.send({ prepare: 0 });
    assert.deepEqual(await original.read(), { prepared: true });
    original.send({ i: "begin" });
    async function burst(stream) {
      for (let n = 0; n < 64; n++) {
        assert.deepEqual(await stream.read(), { s: n + 1, o: n + ":" + "x".repeat(80_000) });
      }
    }
    await burst(original);
    const journalPath = path.join(folder, id + ".journal");
    assert.equal(readFileSync(journalPath, "utf8").split("\n").length, 65);

    const replacement = connect();
    replacement.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.deepEqual(await original.read(), { fenced: true });
    assert.equal((await replacement.read()).first, 1);
    await burst(replacement);
    replacement.send({ unpin: true });
    replacement.send({ i: "next" });
    assert.deepEqual(await replacement.read(), { s: 65, o: "next turn" });
    assert.ok(readFileSync(journalPath).byteLength < 5_100_000);
    await finish(replacement);
}));

test("two simultaneous successors cannot both take one generation",
  { timeout: 15000 }, t => withHolder(t, {}, async ({ connect }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).generation, 1);
    const claimants = [connect(), connect()];
    for (const claimant of claimants) claimant.send({ ...boot, generation: 2 });
    assert.deepEqual(await original.read(), { fenced: true });
    const answers = await Promise.all(claimants.map(claimant => claimant.read()));
    const owner = answers.findIndex(answer => answer.hello === "hold/1");
    assert.notEqual(owner, -1);
    assert.equal(answers[owner].generation, 2);
    assert.deepEqual(answers[1 - owner], { refused: "A bridge of generation 2 has this agent." });
    await finish(claimants[owner]);
}));

test("an orphaned agent closes its stdin and leaves no record or journal",
  { timeout: 15000 }, t => withHolder(t, {
    spec: { orphan: 200 },
  }, async ({ holder, folder, recordPath }) => {
    await until(() => holder.child.exitCode !== null, "orphaned holder did not exit", 5000);
    assert.deepEqual(await holder.exited, { code: 0, signal: null });
    assert.equal(existsSync(recordPath), false);
    assert.equal(existsSync(path.join(folder, id + ".journal")), false);
}));

test("a replacement hears the output and exit of an agent that finished while detached",
  { timeout: 15000 }, t => withHolder(t, {
    agent: `
import { watch } from "node:fs";
const watcher = watch(process.cwd(), (_event, name) => {
  if (name !== "end-turn") return;
  watcher.close();
  console.log("done");
});
console.log("watching");
`,
  }, async ({ connect, folder, root, recordPath }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).generation, 1);
    assert.deepEqual(await original.read(), { s: 1, o: "watching" });
    original.disconnect();
    const lifecycle = path.join(folder, "lifecycle.log");
    await until(() => readFileSync(lifecycle, "utf8").includes("controller disconnected"),
      "holder did not notice the original controller's exit");
    write(path.join(root, "end-turn"), "");
    await until(() => readJSON(recordPath).exited?.code === 0,
      "holder did not journal its agent's exit");

    const successor = connect();
    successor.send({ ...boot, generation: 2, from: 1, complete: true });
    assert.deepEqual((await successor.read()).exited, { code: 0, signal: null });
    assert.deepEqual(await successor.read(), { s: 2, o: "done" });
    assert.deepEqual(await successor.read(), { x: { code: 0, signal: null } });
    successor.send({ bye: true });
    assert.equal((await successor.closed()).done, true);
}));

test("stopping a held agent also stops its subprocess",
  { timeout: 15000 }, t => withHolder(t, {
    agent: `
import { spawn } from "node:child_process";
const nested = spawn(process.execPath, ["-e", "const net = require('node:net'); const server = net.createServer(socket => socket.end()); server.listen(0, '127.0.0.1', () => console.log(server.address().port));"], { stdio: ["ignore", "inherit", "inherit"] });
process.stdin.resume();
`,
  }, async ({ connect }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).generation, 1);
    const port = Number((await original.read()).o);
    assert.ok(port > 0);
    assert.equal(await accepts(port), true);
    original.send({ signal: "SIGTERM" });
    assert.deepEqual(await original.read(), { x: windows
      ? { code: 1, signal: null }
      : { code: null, signal: "SIGTERM" } });
    await until(async () => !await accepts(port), "held subprocess still accepts connections");
    original.send({ bye: true });
    assert.equal((await original.closed()).done, true);
}));

test("a stalled controller cannot hold back a successor's complete replay",
  { timeout: 15000 }, t => withHolder(t, {
    agent: `
import { createInterface } from "node:readline";
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "begin") for (let n = 0; n < 16; n++) console.log(n + ":" + "x".repeat(320_000));
}
`,
  }, async ({ connect, folder }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.pause();
    original.send({ i: "begin" });
    const journalPath = path.join(folder, id + ".journal");
    await until(() => statSync(journalPath).size > 300_000,
      "holder never started sending to the stalled reader");

    const successor = connect();
    successor.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.equal((await successor.read()).generation, 2);
    for (let n = 0; n < 16; n++) {
      assert.deepEqual(await successor.read(), { s: n + 1, o: n + ":" + "x".repeat(320_000) });
    }
    original.disconnect();
    await finish(successor);
}));

test("the raw worker replays blank lines and carriage returns as written",
  { timeout: 15000 }, t => withHolder(t, {
    kind: "raw",
    agent: String.raw`
process.stdin.once("data", () => process.stdout.write("first\r\n\nlast\n"));
process.stdin.resume();
`,
  }, async ({ connect }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.send({ i: "begin" });
    const expected = ["first\r", "", "last"].map((o, i) => ({ s: i + 1, o }));
    for (const frame of expected) assert.deepEqual(await original.read(), frame);
    const successor = connect();
    successor.send({ ...boot, generation: 2, from: 0, complete: true });
    assert.deepEqual(await original.read(), { fenced: true });
    assert.equal((await successor.read()).seq, 3);
    for (const frame of expected) assert.deepEqual(await successor.read(), frame);
    await finish(successor);
}));

test("a pinned journal does not keep an abandoned holder alive",
  { timeout: 15000 }, t => withHolder(t, {
    spec: { orphan: 1000, journalBytes: 128 },
    agent: `
process.stdin.once("data", () => console.log("x".repeat(200)));
process.stdin.resume();
`,
  }, async ({ connect, holder, recordPath }) => {
    const original = connect();
    original.send(boot);
    assert.equal((await original.read()).seq, 0);
    original.send({ prepare: 0 });
    assert.deepEqual(await original.read(), { prepared: true });
    original.send({ i: "begin" });
    assert.deepEqual(await original.read(), { s: 1, o: "x".repeat(200) });
    original.disconnect();
    await until(() => holder.child.exitCode !== null, "holder kept an abandoned journal pinned", 5000);
    assert.deepEqual(await holder.exited, { code: 0, signal: null });
    assert.equal(existsSync(recordPath), false);
}));

async function accepts(port) {
  return new Promise(resolve => {
    const socket = net.createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
}

async function finish(controller) {
  controller.send({ end: true });
  assert.deepEqual(await controller.read(), { x: { code: 0, signal: null } });
  controller.send({ bye: true });
  assert.equal((await controller.closed()).done, true);
}

function windowsAddress() {
  const whoami = path.join(process.env.SystemRoot, "System32", "whoami.exe");
  const user = execFileSync(whoami, ["/user", "/fo", "csv", "/nh"], { encoding: "utf8" });
  const sid = user.match(/"((?:S-1-)(?:\d+-)*\d+)"/)[1];
  return "\\\\.\\pipe\\kuvo-" + createHmac("sha256", homeKey)
    .update(`kuvo-local-ipc-v1\0${sid}\0holder:${id}`).digest("hex");
}
