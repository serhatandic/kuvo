import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const link = await import(script);
const submit = (box, primary, epoch) => box.route("POST", "kuvo/setup/handover", { primary: primary.key, epoch });

async function until(what, read, signal) {
  while (!signal.aborted) {
    const value = read();
    if (value) return value;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${what}`);
}

async function network(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-handover-"));
  const relay = await relayStandIn(link);
  const engine = http.createServer((_request, response) => response.writeHead(200, { "content-type": "application/json" }).end('{"data":[]}'));
  engine.listen(0, "127.0.0.1");
  await once(engine, "listening");
  const children = new Set();
  const start = (env, args, io) => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: io });
    children.add(child);
    child.once("exit", () => children.delete(child));
    return child;
  };
  t.after(async () => {
    const stops = [...children].map(async child => {
      const exited = once(child, "exit");
      child.kill("SIGTERM");
      await exited;
    });
    await Promise.all(stops);
    engine.closeAllConnections();
    await Promise.all([relay.close(), new Promise(resolve => engine.close(resolve))]);
    fs.rmSync(root, { recursive: true, force: true });
  });

  const boxes = ["Studio", "Desk", "Lab"].map(name => {
    const home = path.join(root, name);
    const secret = crypto.randomBytes(32);
    const privateKey = link.newKey();
    const seed = link.newSigningSeed();
    const address = link.addressOf(secret);
    const key = link.publicOf(privateKey).toString("base64url");
    const sign = link.signingKeyOf(seed);
    const env = { HOME: home, KUVO_HOME: home, PATH: process.env.PATH, TMPDIR: root };
    const run = path.join(home, "run");
    fs.mkdirSync(path.join(run, "work-queue"), { recursive: true });
    fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ port: engine.address().port, password: "test" }));
    const read = file => JSON.parse(fs.readFileSync(path.join(run, file), "utf8"));
    return {
      key, sign, seed, address, name, run, env,
      entry: { key, sign, address: address.toString("base64url"), name, kind: "machine" },
      identity: { key: privateKey.toString("base64url"), sign: seed, secret: secret.toString("base64url"), name, relay: relay.url },
      read: () => read("link.json"),
      heard: key => {
        try { return read("peer-agents.json")[key]?.features?.includes("setup.handover"); } catch { return false; }
      },
      async route(method, path, body) {
        const child = start(env, ["box", "route"], ["pipe", "pipe", "pipe"]);
        let out = "", err = "";
        child.stdout.on("data", chunk => { out += chunk; });
        child.stderr.on("data", chunk => { err += chunk; });
        child.stdin.end(JSON.stringify({ method, path, body }));
        const [status] = await once(child, "close");
        assert.equal(status, 0, err);
        const wire = JSON.parse(out.trim().split("\n").at(-1));
        return { status: wire.status, value: JSON.parse(wire.body) };
      },
      start() { this.child = start(env, ["run"], ["ignore", "ignore", "inherit"]); },
      async stop(signal = "SIGKILL") {
        const stopped = once(this.child, "exit");
        this.child.kill(signal);
        assert.equal((await stopped)[1], signal === "SIGTERM" ? null : signal);
      },
    };
  });
  const [a, b, c] = boxes;
  const setup = { id: `set_${crypto.randomBytes(16).toString("base64url")}`, epoch: 1, primary: a.key };
  const book = link.signDoc(a.seed, "peers", 1, { setup, boxes: boxes.map(box => box.entry), moved: [], left: [] });
  const phone = link.publicOf(link.newKey()).toString("base64url");
  const pairing = crypto.randomBytes(16).toString("base64url");
  for (const box of boxes) {
    fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify({
      ...box.identity,
      setup: { ...setup, role: box === a ? "primary" : "member", sign: a.sign },
      ...(box === a ? {
        macPeers: [b.entry, c.entry], phones: [{ key: phone, pairing, from: "pair", name: "Device" }],
        vouchBlocked: [{ key: c.key, device: "Revoked", at: 1 }],
      } : { mac: a.key, macs: [a.key], macSigns: { [a.key]: a.sign }, docs: { [`${a.sign} peers`]: book } }),
    }));
  }
  fs.writeFileSync(path.join(a.run, "work-queue", "numbers.json"), JSON.stringify({ primary: "self", next: 401, leases: [] }));
  // Start the members first so the primary can immediately hear their feature advertisement.
  for (const box of [b, c, a]) box.start();
  await until("connected setup", () => boxes.every(box => relay.has(box.address)) && a.heard(b.key) && a.heard(c.key) && c.heard(a.key) && b.heard(a.key), t.signal);
  const see = (box, epoch, primary) => until(`${box.name} at epoch ${epoch}`, () => {
    const setup = box.read().setup;
    return setup.epoch === epoch && setup.primary === primary.key && setup.role === (box === primary ? "primary" : "member") && setup;
  }, t.signal);
  return { boxes, a, b, c, relay, see, phone, pairing };
}

test("failure bench: an offline member follows two handovers and the former primary can be unlinked", { timeout: 50_000 }, async t => {
  const { a, b, c, see, phone, pairing } = await network(t);
  const refused = { status: 409, value: { message: "Choose another primary first." } };
  assert.deepEqual(await c.route("DELETE", `kuvo/computers/${a.key}`), refused);
  assert.deepEqual(await submit(c, b, 1), { status: 202, value: { primary: b.key, epoch: 2 } });
  await Promise.all([see(a, 2, b), see(b, 2, b), see(c, 2, b)]);
  assert.equal(b.read().phones.find(device => device.key === phone)?.pairing, pairing);
  assert.equal(a.read().phones.find(device => device.key === phone)?.mac, b.key);
  assert.equal(b.read().vouchBlocked[0].key, c.key);
  const transition = link.readDoc(b.read().leadership[2]);
  assert.equal(transition.body.numbers.next, 401);
  const stale = c.read();
  assert.equal(link.takeDoc(stale, b.read().leadership[2]), null);
  const forged = link.signDoc(a.seed, "leadership", 3, { ...transition.body, previous: b.key, setup: { ...transition.body.setup, epoch: 3, primary: a.key } });
  assert.equal(link.takeDoc(stale, forged), null);
  assert.equal(stale.setup.primary, b.key);
  const staleRevoke = link.signDoc(a.seed, "phones", 40, { phones: [], removed: [{ key: phone, pairing }] });
  assert.equal(link.takeDoc(stale, staleRevoke), null);
  assert.deepEqual(await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }), {
    status: 409, value: { message: "Remove devices from Desk.", primary: b.key },
  });
  assert.deepEqual(await a.route("DELETE", `kuvo/computers/${b.key}`), refused);

  await c.stop("SIGTERM");
  assert.deepEqual(await submit(b, a, 2), { status: 202, value: { primary: a.key, epoch: 3 } });
  await Promise.all([see(a, 3, a), see(b, 3, a)]);
  assert.deepEqual(await submit(a, b, 3), { status: 202, value: { primary: b.key, epoch: 4 } });
  await Promise.all([see(a, 4, b), see(b, 4, b)]);
  assert.equal(c.read().setup.epoch, 2);
  c.start();
  await see(c, 4, b);
  assert.deepEqual(Object.keys(c.read().leadership), ["2", "3", "4"]);
  assert.equal((await b.route("DELETE", `kuvo/computers/${a.key}`)).status, 200);
  await until("removal received", () => !a.read().setup || a.read().setup.id !== b.read().setup.id, t.signal);
});

test("failure bench: a successor that goes unreachable during handover follows the committed epoch when back", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  let reconnect;
  const unhook = relay.intercept(b.address, a.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), a.read())?.payload;
    if (message?.workQueue?.acted?.value?.sign !== b.sign) return false;
    relay.replay(packet);
    reconnect = relay.disconnect(b.address);
    unhook();
    return true;
  });
  assert.equal((await submit(a, b, 1)).status, 202);
  assert.equal(typeof reconnect, "function");
  await Promise.all([see(a, 2, b), see(c, 2, b)]);
  assert.equal(b.read().setup.epoch, 1);
  reconnect();
  await see(b, 2, b);
});

test("failure bench: a primary killed after commit and before publishing resumes as a member", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  const resumeWitness = relay.pause(a.address, c.address);
  // Let the primary's readiness request pass, then block its publication to both members.
  let resumeTarget;
  const unhook = relay.intercept(b.address, a.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), a.read())?.payload;
    if (message?.workQueue?.acted?.value?.sign !== b.sign) return false;
    resumeTarget = relay.pause(a.address, b.address);
    unhook();
    return false;
  });
  assert.equal((await submit(a, b, 1)).status, 202);
  await a.stop();
  assert.equal(a.read().setup.role, "member");
  assert.equal(b.read().setup.epoch, 1);
  assert.equal(c.read().setup.epoch, 1);
  assert.equal(typeof resumeTarget, "function");
  resumeTarget();
  resumeWitness();
  a.start();
  await Promise.all([see(a, 2, b), see(b, 2, b), see(c, 2, b)]);
});

test("failure bench: losing the successor before readiness keeps the old primary", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  let reconnect;
  const unhook = relay.intercept(a.address, b.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), b.read())?.payload;
    if (message?.workQueue?.act?.area !== "setup") return false;
    reconnect = relay.disconnect(b.address);
    unhook();
    return true;
  });
  assert.equal((await submit(a, b, 1)).status, 503);
  assert.equal(typeof reconnect, "function");
  await Promise.all([see(a, 1, a), see(b, 1, a), see(c, 1, a)]);
  assert.equal(a.read().leadership, undefined);
  reconnect();
});
