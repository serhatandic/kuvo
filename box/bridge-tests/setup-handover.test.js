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

function checkpoint(box, epoch) {
  const transition = link.readDoc(box.read().leadership[epoch]);
  assert.equal(transition.body.boxes, undefined);
  assert.equal(transition.body.devices, undefined);
  assert.equal(transition.body.shared, undefined);
  assert.equal(transition.body.numbers, undefined);
  const file = path.join(box.run, "setup-checkpoints", transition.body.checkpoint + ".json");
  const envelope = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.equal(crypto.createHash("sha256").update(envelope.d).digest("base64url"), transition.body.checkpoint);
  const saved = link.readDoc(envelope);
  assert.equal(saved.kind, "setup-checkpoint");
  assert.equal(saved.n, epoch);
  return saved.body;
}

const pendingOperation = (box, kind) => Object.entries(box.read().sharedPending ?? {})
  .find(([, envelope]) => link.readDoc(envelope).body.kind === kind)?.[0];

async function until(what, read, signal) {
  while (!signal.aborted) {
    const value = read();
    if (value) return value;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${what}`);
}

async function network(t, takeover = false) {
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
    const clock = offset => {
      const file = path.join(run, "test-setup-clock.json");
      fs.writeFileSync(file + ".new", JSON.stringify({ offset }));
      fs.renameSync(file + ".new", file);
    };
    if (takeover) {
      clock(0);
      fs.writeFileSync(path.join(home, "setup-takeover.json"), JSON.stringify({ alwaysOn: name === "Desk" }));
    }
    return {
      key, sign, seed, address, name, run, env,
      entry: { key, sign, address: address.toString("base64url"), name, kind: "machine" },
      identity: { key: privateKey.toString("base64url"), sign: seed, secret: secret.toString("base64url"), name, relay: relay.url },
      read: () => read("link.json"),
      clock,
      lease(epoch, standby) {
        try { return read("setup-lease.json").stamp === `${this.read().setup.id}:${epoch}:${standby.key}`; } catch { return false; }
      },
      heard: key => {
        try { return read("peer-agents.json")[key]?.features?.includes("setup.handover"); } catch { return false; }
      },
      async command(args, input = "") {
        const child = start(env, args, ["pipe", "pipe", "pipe"]);
        let out = "", err = "";
        child.stdout.on("data", chunk => { out += chunk; });
        child.stderr.on("data", chunk => { err += chunk; });
        child.stdin.end(input);
        const [status] = await once(child, "close");
        return { status, value: JSON.parse(out.trim().split("\n").at(-1)), err };
      },
      async route(method, path, body, local = false) {
        const args = local ? ["--input-type=module", "-e", `
          import { engineHandler } from ${JSON.stringify(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url).href)};
          const chunks = [];
          for await (const chunk of process.stdin) chunks.push(chunk);
          const given = JSON.parse(Buffer.concat(chunks));
          const request = { ...given, fromApp: true, served: true, headers: {}, body: Buffer.from(JSON.stringify(given.body ?? {})) };
          let status, body = [];
          for await (const part of engineHandler()(request, {})) {
            if (part.head) status = part.head.status;
            if (part.body) body.push(part.body);
          }
          console.log(JSON.stringify({ status, body: Buffer.concat(body).toString() }));
        `] : [script, "box", "route"];
        const child = spawn(process.execPath, args, { env, stdio: ["pipe", "pipe", "pipe"] });
        children.add(child);
        child.once("exit", () => children.delete(child));
        let out = "", err = "";
        child.stdout.on("data", chunk => { out += chunk; });
        child.stderr.on("data", chunk => { err += chunk; });
        child.stdin.end(JSON.stringify({ method, path, body }));
        const [status] = await once(child, "close");
        assert.equal(status, 0, err);
        const wire = JSON.parse(out.trim().split("\n").at(-1));
        return { status: wire.status, value: JSON.parse(wire.body) };
      },
      start() {
        this.output = "";
        const child = this.child = start(env, ["run"], ["ignore", "pipe", "inherit"]);
        child.stdout.on("data", chunk => {
          if (this.child === child) this.output = (this.output + chunk).slice(-16_000);
        });
      },
      observing(epoch) { return this.output.includes(`Standby watching epoch ${epoch} `); },
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
  assert.equal(checkpoint(b, 2).shared.queueNumberAllocation.value.next, 401);
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
  const setupID = b.read().setup.id;
  assert.equal((await b.route("DELETE", `kuvo/computers/${a.key}`)).status, 200);
  await until("removal received", () => a.read().setup?.id !== setupID, t.signal);
  assert.equal(a.read().macPeers, undefined);
  await see(b, 4, b);
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

const HOUR = 3_600_000;

async function standbyNetwork(t) {
  const net = await network(t, true);
  const { a, b, see } = net;
  await see(a, 2, a);
  assert.equal((await a.route("GET", "kuvo/setup")).value.standby, b.key);
  await until("primary leased", () => a.lease(2, b), t.signal);
  return net;
}

test("failure bench: only the standby takes over across a split and the old primary rejoins", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, phone, pairing } = await standbyNetwork(t);
  const reconnect = [b, c].flatMap(box => [relay.pause(a.address, box.address), relay.pause(box.address, a.address)]);
  for (const box of [a, b, c]) box.clock(2 * HOUR);
  await delay(800);
  await see(b, 2, a);
  const blocked = await a.route("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(blocked.status, 202);
  assert.equal(blocked.value.pending, true);
  const interrupted = pendingOperation(a, "membershipAndAccess");
  assert.ok(interrupted);
  const work = await a.route("POST", "kuvo/work-queue/items", { title: "Work survives the split" }, true);
  assert.equal(work.status, 200);
  const stale = a.read();

  for (const box of [a, b, c]) box.clock(3 * HOUR + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  assert.equal(a.read().setup.epoch, 2);
  const winning = link.readDoc(b.read().leadership[3]);
  assert.equal(winning.origin, b.sign);
  assert.equal(winning.body.takeover, true);
  assert.equal(checkpoint(b, 3).shared.queueNumberAllocation.value.next, stale.setup.numberCeiling + 1);
  const fake = link.signDoc(c.seed, "leadership", 3, winning.body);
  assert.equal(link.takeDoc(stale, fake), null);
  const oldPrimaryFork = link.signDoc(a.seed, "leadership", 3, {
    ...winning.body, takeover: false, setup: { ...winning.body.setup, primary: c.key },
  });
  assert.equal(link.takeDoc(stale, oldPrimaryFork), null);
  assert.equal(link.takeDoc(b.read(), a.read().leadership[2]), null);
  await see(b, 4, b);
  assert.equal(b.read().setup.standby.key, c.key);
  await until("new primary leased", () => b.lease(4, c), t.signal);
  assert.equal((await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] })).status, 409);

  // Simulate shared state kept only by the old primary, separately from its local item.
  await a.stop("SIGTERM");
  const interim = a.read();
  interim.peerNames = { [c.key]: "Only heard by the old primary" };
  interim.removed = [{ key: phone, pairing }];
  interim.phones = [];
  fs.writeFileSync(path.join(a.run, "link.json"), JSON.stringify(interim));
  reconnect.forEach(resume => resume());
  a.start();
  await see(a, 4, b);
  assert.equal(a.read().peerNames, undefined);
  const lost = await link.askSetupSave(a.run, { id: stale.setup.id, operation: interrupted });
  assert.equal(lost.status, 200);
  assert.equal(lost.outcome, "interrupted");
  assert.equal(a.read().sharedOutcomes[interrupted].baseRevision, 1);
  assert.equal(a.read().phones.find(device => device.key === phone)?.pairing, pairing);
  assert.equal((await a.route("GET", `kuvo/work-queue/items/${work.value.item.id}`)).value.item.title, "Work survives the split");
});

test("failure bench: losing both primary and standby never promotes another member", { timeout: 35_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  await b.stop("SIGTERM");
  c.clock(HOUR);
  await delay(600);
  await see(c, 2, a);
  assert.equal((await a.route("GET", "kuvo/setup")).value.epoch, 2);
  // Primary-side authority expires if the standby stays away.
  a.clock(HOUR);
  assert.equal((await a.route("POST", "kuvo/phones/pair")).status, 409);
  await a.stop("SIGTERM");
  c.clock(5 * HOUR);
  await delay(900);
  await see(c, 2, a);
  assert.equal(c.read().leadership[3], undefined);
});

test("failure bench: only the old standby can approve a standby replacement and a handover", { timeout: 45_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  const selected = await a.route("POST", "kuvo/setup/standby", { standby: c.key, epoch: 2 });
  assert.deepEqual(selected, { status: 202, value: { primary: a.key, epoch: 3 } });
  await Promise.all([see(b, 3, a), see(c, 3, a)]);
  assert.equal(link.readDoc(c.read().leadership[3]).origin, b.sign);
  await until("replacement granted lease", () => a.lease(3, c), t.signal);
  assert.deepEqual(await submit(a, b, 3), { status: 202, value: { primary: b.key, epoch: 4 } });
  await Promise.all([see(b, 4, b), see(c, 4, b)]);
  assert.equal(link.readDoc(b.read().leadership[4]).origin, c.sign);
  assert.equal(b.read().setup.standby.key, c.key);
});

test("failure bench: a lost primary is replaced after the configured delay", { timeout: 40_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  fs.writeFileSync(path.join(b.env.HOME, "setup-takeover.json"), JSON.stringify({ delayMinutes: 60, alwaysOn: true }));
  await a.stop("SIGKILL");
  b.clock(3_000);
  await delay(700);
  await see(b, 2, a);
  b.clock(HOUR + 1_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  assert.equal(a.read().setup.epoch, 2);
});

test("failure bench: standby retirement keeps the live standby until a replacement is ready", { timeout: 45_000 }, async t => {
  const { a, b, c, see, relay } = await standbyNetwork(t);
  const wake = relay.disconnect(c.address);
  try {
    const refused = await a.route("DELETE", `kuvo/computers/${b.key}`);
    assert.equal(refused.status, 503, JSON.stringify(refused.value));
    assert.equal(a.read().setup.epoch, 2);
    assert.equal(a.read().setup.standby.key, b.key);
    assert.equal(a.read().leadershipPending, undefined);
  } finally {
    wake();
  }
  await until("candidate reconnects", () => relay.has(c.address), t.signal);
  const done = await a.route("DELETE", `kuvo/computers/${b.key}`);
  assert.equal(done.status, 200, JSON.stringify(done.value));
  await see(a, 3, a);
  await see(c, 3, a);
  assert.equal(a.read().setup.standby.key, c.key);
  assert.equal(link.readDoc(c.read().leadership[3]).origin, b.sign);
  assert.ok(a.read().left.includes(b.key));
  await until("standby removal received", () => b.read().setup?.id !== a.read().setup.id, t.signal);
});

test("failure bench: local uninstall refuses a primary before clearing identity or stopping it", { timeout: 30_000 }, async t => {
  const { a } = await standbyNetwork(t);
  const identity = path.join(a.env.HOME, "desktop", "identity");
  fs.mkdirSync(path.dirname(identity));
  fs.writeFileSync(identity, "keep this identity");
  const before = a.read().setup;
  const done = await a.command(["uninstall"]);
  assert.equal(done.status, 1, done.err);
  assert.deepEqual(done.value, { result: "refused", message: "Choose another primary first." });
  assert.deepEqual(a.read().setup, before);
  assert.equal(fs.readFileSync(identity, "utf8"), "keep this identity");
  assert.equal(a.child.exitCode, null);
  const home = path.join(a.env.HOME, "standalone");
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  const child = spawn(process.execPath, [script, "uninstall"], { env: { ...a.env, HOME: home, KUVO_HOME: home }, stdio: "ignore" });
  assert.equal((await once(child, "exit"))[0], 0);
  assert.ok(fs.existsSync(path.join(home, "run", "off")));
});

test("failure bench: a QR handshake waits for its own durable pairing receipt", { timeout: 35_000 }, async t => {
  const { a, b, relay } = await standbyNetwork(t);
  const pairing = await a.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200);
  const qr = new URL(pairing.value.link);
  const device = relay.device();
  const newKey = link.newKey();
  const deviceKey = link.publicOf(newKey).toString("base64url");
  const initiator = new link.IKInitiator(newKey, Buffer.from(qr.searchParams.get("key"), "base64url"), link.PROLOGUE);
  const receipts = [];
  const release = relay.intercept(b.address, a.address, packet => {
    // Keep leases flowing while holding the signed access receipt.
    const envelope = link.openMail(link.readFrame(packet.payload).body, a.read())?.payload?.docs?.give?.[0];
    if (link.readDoc(envelope)?.body.receipt) { receipts.push(packet); return true; }
    return false;
  });
  t.after(release);
  device.send(a.address, link.frame(link.Frame.handshake1, 44, initiator.writeMessage1(Buffer.from(JSON.stringify({
    v: 1, name: "New QR phone", pair: qr.searchParams.get("token"),
  })))));
  let frame;
  const reply = device.next(15_000).then(bytes => { frame = link.readFrame(link.readRelay(bytes).payload); });
  // Register the rejection handler before assertions so a failed test can also close the relay.
  reply.catch(() => {});
  await until("the standby persists the phone and tries to send its receipt", () => receipts[0], t.signal);
  await delay(3_250);
  assert.equal(frame, undefined, "the box has sent neither a refusal nor a session");
  assert.ok(b.read().phones.some(phone => phone.key === deviceKey));
  assert.ok(!a.read().phones.some(phone => phone.key === deviceKey));
  release();
  relay.replay(receipts[0]);
  await reply;
  assert.equal(frame.kind, link.Frame.handshake2);
  assert.equal(JSON.parse(initiator.readMessage2(frame.body).payload).v, 1);
  assert.ok(a.read().phones.some(phone => phone.key === deviceKey));
});

const personality = (i, length = 40) => ({ id: `custom.personal-${i}`, name: `Personality ${i}`, description: "x".repeat(length), updated: i + 1 });

test("failure bench: the SSH personalities command saves through the standby", { timeout: 35_000 }, async t => {
  const { a, b } = await standbyNetwork(t);
  const file = path.join(a.env.HOME, "bots", "personalities.json");
  const oldBytes = () => fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
  const before = oldBytes();
  const document = { personalities: [personality(1)], removed: [] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.status, 0, saved.err);
  assert.equal(saved.value.status, 200, JSON.stringify(saved.value));
  assert.deepEqual((await a.route("GET", "kuvo/personalities")).value.personalities.map(each => each.id), ["custom.personal-1"]);
  assert.deepEqual(b.read().shared.setupPersonalities.value.personalities.map(each => each.id), ["custom.personal-1"]);
  assert.equal(oldBytes(), before, "the SSH save did not write to the legacy file");
});

test("failure bench: first standby assignment waits for the candidate to install the entire personality library", { timeout: 35_000 }, async t => {
  const { a, b, c, relay } = await network(t);
  const document = { personalities: [personality(1), personality(2)], removed: [{ id: "custom.retired", removed: 5 }] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.value.status, 200);
  const accepted = [];
  const releases = [b, c].map(from => relay.intercept(from.address, a.address, packet => {
    const give = link.openMail(link.readFrame(packet.payload).body, a.read())?.payload?.docs?.give;
    if (give?.some(envelope => link.readDoc(envelope)?.body.standbyAcceptance === true && link.readDoc(envelope).body.approval)) {
      accepted.push(packet);
      return true;
    }
    return false;
  }));
  const release = () => releases.forEach(resume => resume());
  t.after(release);
  const selecting = a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  selecting.catch(() => {});
  await until("the new standby tries to confirm its durable replica", () => accepted[0], t.signal);
  assert.equal(a.read().setup.epoch, 1);
  assert.equal(a.read().setup.standby, undefined);
  const blocked = await a.command(["box", "personalities"], JSON.stringify({ personalities: [personality(3)] }));
  assert.equal(blocked.value.status, 503, "the legacy library is frozen while its replica is waiting");
  assert.equal(b.read().setup.standby.key, b.key);
  assert.deepEqual(b.read().shared.setupPersonalities.value, {
    personalities: saved.value.personalities, removed: saved.value.removed,
  });
  release();
  relay.replay(accepted[0]);
  assert.equal((await selecting).status, 202);
  assert.equal(a.read().setup.standby.key, b.key);
});

test("failure bench: a first standby refuses a legacy library beyond replica capacity", { timeout: 35_000 }, async t => {
  const { a, b } = await network(t);
  const document = { personalities: Array.from({ length: 103 }, (_, i) => personality(i, 2_000)), removed: [] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.value.status, 200);
  assert.equal(saved.value.personalities.length, document.personalities.length);
  const refused = await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  assert.equal(refused.status, 413, JSON.stringify(refused.value));
  assert.match(refused.value.message, /personality library.*too large/);
  assert.equal(a.read().setup.standby, undefined);
  assert.equal(a.read().setup.epoch, 1);
  assert.equal((await a.route("GET", "kuvo/personalities")).value.personalities.length, document.personalities.length);
});

const rename = (a, target, name) => a.route("PATCH", `kuvo/computers/${target.key}`, { name });
const setupName = async (box, key) => (await box.route("GET", "kuvo/setup")).value.computers.find(each => each.key === key)?.name;

test("failure bench: a saved change survives killing the primary before it publishes", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, phone, pairing } = await standbyNetwork(t);
  // Prevent the third computer from learning the change from the old primary.
  const reconnect = [relay.pause(a.address, c.address), relay.pause(b.address, c.address)];
  const standby = await rename(a, b, "Renamed Desk");
  assert.deepEqual(standby, { status: 200, value: { name: "Renamed Desk" } });
  const done = await rename(a, c, "Accepted name");
  assert.deepEqual(done, { status: 200, value: { name: "Accepted name" } });
  assert.equal(b.read().shared.setupNames.value[c.key], "Accepted name");
  assert.equal(await setupName(c, c.key), c.name);
  assert.equal((await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true)).status, 200);
  assert.deepEqual(b.read().shared.membershipAndAccess.value.devices.removed, [{ key: phone, pairing }]);
  await a.stop();
  await b.stop("SIGTERM");
  const saved = b.read();
  // Drop the ordinary roster publication while retaining the signed leadership
  // certificate and the durable replica. Takeover must recover the accepted name.
  delete saved.docs?.[`${a.sign} peers`];
  delete saved.docs?.[`${a.sign} phones`];
  fs.writeFileSync(path.join(b.run, "link.json"), JSON.stringify(saved));
  b.start();
  await until("restarted standby has begun observing epoch 2", () => b.observing(2), t.signal);
  reconnect.forEach(resume => resume());
  b.clock(3 * HOUR + 5_000);
  await see(b, 3, b);
  await see(c, 3, b);
  assert.equal(await setupName(b, c.key), "Accepted name");
  assert.equal(await setupName(c, c.key), "Accepted name");
  assert.equal(await setupName(b, b.key), "Renamed Desk");
  assert.equal((await b.route("GET", "kuvo/computers")).value.computers.find(box => box.key === b.key)?.name, "Renamed Desk");
  assert.equal(await setupName(c, b.key), "Renamed Desk");
  const discovery = await b.route("GET", "kuvo/hosts");
  assert.equal(discovery.status, 200);
  assert.deepEqual(discovery.value.hosts.map(({ key, name }) => ({ key, name })),
    [{ key: c.key, name: "Accepted name" }, { key: a.key, name: a.name }]);
  assert.deepEqual(await c.route("GET", "kuvo/hosts"), discovery);
  assert.equal(b.read().phones.some(device => device.key === phone), false);
  assert.ok(b.read().removed.some(device => device.key === phone && device.pairing === pairing));
});

test("failure bench: a save waits durably through standby and primary restarts", { timeout: 45_000 }, async t => {
  const { a, b, c } = await standbyNetwork(t);
  await b.stop("SIGTERM");
  const usage = {
    kind: "setupUsageDisplay", id: a.read().setup.id, epoch: 2, sequence: 0,
    value: { units: { codexFallback: { value: "gpt-test", stamp: { v: 1, by: a.key } } }, imported: true },
  };
  assert.equal((await a.route("POST", "kuvo/setup/save-local", usage, true)).status, 403);
  const [answer, display, label] = await Promise.all([
    rename(a, c, "Waits on disk"), link.askSetupSave(a.run, usage),
    a.route("POST", "kuvo/work-queue/labels", { add: "Saved label" }, true),
  ]);
  assert.equal(label.status, 202);
  assert.equal(label.value.pending, true);
  assert.equal(display.status, 202);
  assert.equal(a.read().shared?.setupUsageDisplay, undefined);
  assert.equal(answer.status, 202);
  assert.equal(answer.value.pending, true);
  assert.ok(pendingOperation(a, "setupNames"));
  assert.ok(display.operation);
  assert.equal(pendingOperation(a, usage.kind), display.operation);
  assert.equal((await link.askSetupSave(a.run, { id: usage.id, operation: display.operation })).outcome, "pending");
  assert.equal(await setupName(a, c.key), c.name);
  const item = await a.route("POST", "kuvo/work-queue/items", { title: "Everyday work" }, true);
  assert.equal(item.status, 200);
  assert.equal(item.value.item.number, undefined);
  await a.stop();
  a.start();
  b.start();
  await until("saved on both boxes", () => {
    const saved = box => box.read().shared?.setupNames?.value[c.key] === "Waits on disk";
    return saved(a) && saved(b) && !pendingOperation(a, "setupNames")
      && a.read().shared?.setupUsageDisplay?.sequence === 1 && b.read().shared?.setupUsageDisplay?.sequence === 1
      && a.read().shared?.queueLabels?.sequence === 2 && b.read().shared?.queueLabels?.sequence === 2;
  }, t.signal);
  assert.equal(await setupName(a, c.key), "Waits on disk");
  assert.deepEqual(b.read().shared.setupUsageDisplay.value, usage.value);
  assert.equal((await link.askSetupSave(a.run, { id: usage.id, operation: display.operation })).outcome, "committed");
  await until("display preferences published to an ordinary member", () => c.read().shared?.setupUsageDisplay?.sequence === 1, t.signal);
  const later = await link.askSetupSave(a.run, { ...usage, sequence: 1, value: { units: {}, imported: false } });
  assert.equal(later.status, 200);
  assert.notEqual(later.operation, display.operation);
  assert.equal((await link.askSetupSave(b.run, { id: usage.id, operation: display.operation })).outcome, "committed");
  assert.equal((await a.route("GET", "kuvo/work-queue/labels")).value.labels[0].name, "Saved label");
  assert.equal((await link.askSetupSave(c.run, usage)).status, 409);
});

test("failure bench: number blocks stay unique across a takeover", { timeout: 55_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  const make = async (box, title) => {
    const answer = await box.route("POST", "kuvo/work-queue/items", { title }, true);
    assert.equal(answer.status, 200);
    return answer.value.item;
  };
  const numbered = async (box, id) => {
    while (!t.signal.aborted) {
      const answer = await box.route("GET", `kuvo/work-queue/items/${id}`);
      if (Number.isInteger(answer.value.item?.number)) return answer.value.item.number;
      await delay(150);
    }
    assert.fail("Timed out waiting for item number");
  };
  const first = await make(c, "Numbered by the old primary");
  assert.equal(first.number, undefined);
  const firstNumber = await numbered(c, first.id);
  const ledger = b.read().shared.queueNumberAllocation.value;
  const block = Object.values(ledger.given).find(grant => grant.by === c.key);
  assert.ok(block && block.from <= firstNumber && firstNumber < block.to);

  const reconnect = [relay.pause(b.address, c.address), relay.pause(c.address, b.address)];
  await a.stop();
  const offline = await make(c, "Uses the existing block offline");
  assert.equal(offline.number, firstNumber + 1);
  b.clock(3 * HOUR + 5_000);
  await see(b, 3, b);
  assert.deepEqual(checkpoint(b, 3).shared.queueNumberAllocation.value.given, ledger.given);
  const next = await make(b, "Must wait for a new block");
  assert.equal(next.number, undefined);

  reconnect.forEach(resume => resume());
  c.clock(3 * HOUR + 5_000);
  await see(c, 4, b);
  const nextNumber = await numbered(b, next.id);
  assert.ok(nextNumber > block.to);
  const stillOffline = await make(c, "Keeps the remainder of the old block");
  assert.equal(stillOffline.number, offline.number + 1);
  assert.equal(new Set([firstNumber, offline.number, nextNumber, stillOffline.number]).size, 4);
});
