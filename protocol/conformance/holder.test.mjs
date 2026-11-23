// A bridge hands an in-flight request to its successor over the real holder socket.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, readJSON, scratch, typescriptRuntime, until, write } from "./adapter.mjs";

const id = "0123456789ab";
const key = "fixture-only-key";
const ask = '{"id":17,"method":"fixture/approve"}';
const reply = '{"id":17,"result":{"allow":true}}';
const meta = { kind: "codex", provider: "codex-cli", turns: {} };
const turns = { thread: { session: "ses_held", turn: "turn", from: 0 } };
const boot = { hello: "hold/1", key, generation: 1 };
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

test("hold/1 lets a new controller finish the same agent's turn",
  { timeout: 15000, skip: process.platform === "win32" && "The TypeScript holder uses Unix sockets." }, async () => {
  const binary = boxBinary();
  const root = scratch();
  const folder = path.join(root, "hold");
  const sockets = [];
  const transcript = [];
  let holder;
  try {
    const env = isolatedEnv(root);
    mkdirSync(folder, { mode: 0o700 });
    const worker = path.join(root, "agent.mjs");
    write(worker, agent);
    let command = binary.file;
    let args = ["hold", folder, id, "jsonrpc"];
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
      key, spec: { command: [process.execPath, worker], cwd: root, bridge: process.pid, holder: build, meta, orphan: 8000, linger: 3000 },
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
      const stable = { ...frame };
      if (Object.hasOwn(stable, "pid")) stable.pid = "<holder PID>";
      if (Object.hasOwn(stable, "child")) stable.child = "<agent PID>";
      transcript.push({ controller, direction, line: JSON.stringify(stable) + "\n" });
    }
    capture("original", "holder", ready);

    function connect(controller) {
      const socket = net.createConnection(path.join(folder, id + ".sock"));
      sockets.push(socket);
      socket.setTimeout(3500, () => socket.destroy(new Error("hold/1 socket timed out")));
      socket.on("error", () => {});
      const incoming = frames(socket);
      return {
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
    final.send({ signal: "SIGUSR1" });
    assert.deepEqual(await final.read(), { s: 4, o: "signal received" });
    final.send({ end: true });
    assert.deepEqual(await final.read(), { x: { code: 0, signal: null } });
    final.send({ bye: true });
    await until(() => holder.child.exitCode !== null, "holder did not leave after bye", 3500);
    assert.deepEqual(await holder.exited, { code: 0, signal: null });
    if (process.env.KUVO_CONFORMANCE_HOLDER_TRANSCRIPT) {
      write(process.env.KUVO_CONFORMANCE_HOLDER_TRANSCRIPT, JSON.stringify(transcript, null, 2) + "\n", { flag: "wx" });
    }
  } finally {
    for (const socket of sockets) socket.destroy();
    await holder?.stop();
    rmSync(root, { recursive: true, force: true });
  }
});
