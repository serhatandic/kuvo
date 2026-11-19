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
import { NO_BUN, testBun } from "./bun-for-tests.js";
import { relayStandIn } from "./relay-stand-in.js";
import { runningAs } from "./running-as.js";

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

function assertReceipt(box, kind, status, save) {
  assert.ok(save, `${kind} ${status} reply has a receipt`);
  assert.equal(save.status, status, kind);
  assert.equal(save.pending, status === "pending" ? true : undefined, kind);
  assert.equal(save.setup, box.read().setup.id, kind);
  assert.equal(save.epoch, box.read().setup.epoch, kind);
  const proof = link.readDoc(save.proof);
  assert.equal(proof?.origin, box.sign, kind);
  assert.equal(proof.body.kind, kind);
  assert.equal(proof.body.operation, save.operation, kind);
  if (status === "pending") assert.equal(save.operation, pendingOperation(box, kind), kind);
  else assert.equal(box.read().sharedOutcomes?.[save.operation]?.status, status, kind);
}

async function until(what, read, signal) {
  while (!signal.aborted) {
    const value = read();
    if (value) return value;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${what}`);
}

async function network(t, takeover = false, releaseKey, owned = false) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-handover-"));
  const relay = await relayStandIn(link);
  const boxScript = releaseKey ? path.join(root, "kuvo-link.mjs") : script;
  if (releaseKey) {
    const source = fs.readFileSync(script, "utf8");
    assert.ok(source.includes(link.UPDATE_KEYS[0]));
    fs.writeFileSync(boxScript, source.replace(link.UPDATE_KEYS[0], releaseKey));
  }
  let labelCall;
  const prompts = [];
  const groupMessages = [];
  const bridgePID = owned ? await runningAs("kuvo-agents", t) : null;
  const engine = http.createServer((request, response) => {
    if (owned) {
      const local = request.headers.authorization === "Basic " + Buffer.from("opencode:Lab").toString("base64");
      const send = (status, value) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
      if (request.url.startsWith("/api/kuvo/link/groups")) {
        if (!local) return send(404, {});
        if (request.url.endsWith("/groups")) return send(200, { groups: [{ id: "grp_lab" }] });
        if (request.method === "POST") {
          const parts = [];
          request.on("data", part => parts.push(part));
          request.on("end", () => {
            groupMessages.push(JSON.parse(Buffer.concat(parts)));
            send(200, {});
          });
          return;
        }
        return send(200, { messages: groupMessages });
      }
      if (request.url.startsWith("/api/session/ses_owned")) {
        if (!local) return send(404, {});
        if (request.url.endsWith("/prompt")) {
          const parts = [];
          request.on("data", part => parts.push(part));
          request.on("end", () => {
            prompts.push(JSON.parse(Buffer.concat(parts)));
            send(200, { data: {} });
          });
          return;
        }
        if (request.url.includes("/message?")) return send(200, { data: prompts.map(prompt => ({ id: prompt.id, type: "user", text: prompt.text })), cursor: {} });
        return send(200, { data: { id: "ses_owned", title: "Lab’s conversation", time: { updated: Date.now() } } });
      }
      if (request.url.startsWith("/api/session?")) return send(200, { data: local ? [{ id: "ses_owned", title: "Lab’s conversation", time: { updated: Date.now() } }] : [] });
    }
    const call = request.url.startsWith("/api/session/ses_labels/message") && labelCall;
    const data = call ? [{ content: [{ type: "tool", name: "queue_labels", state: { status: "running", input: call } }] }] : [];
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data }));
  });
  engine.listen(0, "127.0.0.1");
  await once(engine, "listening");
  const children = new Set();
  const start = (env, args, io, command = process.execPath) => {
    const child = spawn(command, args, { env, stdio: io });
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
    fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ port: engine.address().port, password: owned ? name : "test" }));
    if (bridgePID && name === "Lab") fs.writeFileSync(path.join(run, "bridge.json"), JSON.stringify({
      pid: bridgePID, port: engine.address().port, password: name,
    }));
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
      command(args, input = "") { return this.evaluate(process.execPath, [boxScript, ...args], input); },
      async evaluate(command, args, input = "") {
        const child = start(env, args, ["pipe", "pipe", "pipe"], command);
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
        `] : [boxScript, "box", "route"];
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
      async labelTool(args) {
        labelCall = args;
        return (await this.route("POST", "kuvo/work-queue/tools", { name: "queue_labels", arguments: args, session: "ses_labels" }, true)).value;
      },
      async updateFrom(from) {
        assert.ok(releaseKey);
        const args = ["update", "--now", "--key", releaseKey, "--url", `http://127.0.0.1:${engine.address().port}/no-release`];
        const { status, value, err } = await this.command(from ? [...args, "--from", from] : args);
        assert.equal(status, 0, err);
        return value;
      },
      start() {
        this.output = "";
        const child = this.child = start(env, [boxScript, "run"], ["ignore", "pipe", "inherit"]);
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
  return { boxes, a, b, c, relay, see, phone, pairing, prompts, groupMessages };
}

test("failure bench: an away owner leaves saved reads and accepts an unsent message on return", { timeout: 50_000 }, async t => {
  const { a, b, c, relay, prompts, groupMessages } = await network(t, false, undefined, true);
  const read = (box, query = "") => box.route("GET", `kuvo/data/owner?kind=conversations&id=ses_owned&computer=${c.key}${query}`);
  const before = await read(b);
  assert.equal(before.status, 200);
  assert.equal(before.value.owner.key, c.key);
  assert.equal(before.value.owner.name, "Lab");
  assert.equal(before.value.owner.reachable, true);
  assert.equal(before.value.state, "received");
  assert.deepEqual(before.value.data.messages, []);
  assert.ok(before.value.lastReceivedAt > 0);
  const group = "kuvo/data/owner?kind=groupTranscript&id=grp_lab";
  const groupRead = await b.route("GET", group);
  assert.equal(groupRead.value.owner.key, c.key);
  assert.deepEqual(groupRead.value.data.messages, []);

  const rejoin = relay.disconnect(c.address);
  await c.stop("SIGTERM");
  const stale = await read(b, "&file=plan.md&message=msg_unknown");
  assert.equal(stale.value.owner.reachable, false);
  assert.equal(stale.value.stale, true);
  assert.equal(stale.value.state, "received");
  assert.equal(stale.value.lastReceivedAt, before.value.lastReceivedAt);
  assert.deepEqual(stale.value.unavailable, { files: ["plan.md"], messages: ["msg_unknown"] });
  const unseen = await read(a);
  assert.equal(unseen.value.state, "neverReceived");
  assert.equal(unseen.value.lastReceivedAt, null);
  assert.equal(Object.hasOwn(unseen.value, "data"), false);

  const message = { id: "msg_owned", words: "This belongs to Lab." };
  const sent = await b.route("POST", "kuvo/sessions/ses_owned/send", message);
  assert.equal(sent.status, 202);
  const operation = sent.value.operation;
  const check = () => b.route("GET", `kuvo/data/messages/${operation}`);
  assert.equal((await check()).value.status, "pending");
  assert.deepEqual(prompts, []);
  const groupSend = await b.route("POST", `kuvo/data/messages?group=grp_lab&computer=${c.key}`, { id: "msg_group", text: "This belongs in Lab’s group." });
  assert.equal(groupSend.status, 202);
  const groupReceipt = () => b.route("GET", `kuvo/data/messages/${groupSend.value.operation}`);
  assert.equal((await groupReceipt()).value.status, "pending");
  assert.deepEqual(groupMessages, []);

  // The sender restarts, and the owner accepts without the primary in the network.
  await b.stop("SIGTERM");
  await a.stop("SIGTERM");
  b.start();
  assert.equal((await check()).value.status, "pending");
  rejoin();
  const releaseReceipt = relay.pause(c.address, b.address);
  c.start();
  await until("owner accepted the prompt", () => prompts.length === 1, t.signal);
  assert.equal((await check()).value.status, "pending", "the sender has no receipt yet");
  await c.stop("SIGTERM");
  releaseReceipt();
  c.start();
  let receipt;
  while (!t.signal.aborted) {
    receipt = (await check()).value;
    if (receipt.status !== "pending") break;
    await delay(150);
  }
  assert.equal(receipt.status, "committed");
  assert.equal(receipt.operation, operation);
  assert.equal(receipt.owner, c.key);
  assert.ok(receipt.acceptedAt >= before.value.lastReceivedAt);
  assert.equal((await b.route("POST", "kuvo/sessions/ses_owned/send", message)).value.status, "committed");
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
  assert.deepEqual((await read(b)).value.data.messages.map(prompt => prompt.id), [message.id]);
  let groupOutcome;
  while (!t.signal.aborted) {
    groupOutcome = (await groupReceipt()).value;
    if (groupOutcome.status !== "pending") break;
    await delay(150);
  }
  assert.equal(groupOutcome.status, "committed");
  assert.deepEqual(groupMessages.map(message => message.id), ["msg_group"]);
});

test("failure bench: a member runs its own conversation and Work Queue while the primary is away", { timeout: 35_000 }, async t => {
  const { a, b, c, prompts } = await network(t, false, undefined, true);
  await a.stop("SIGTERM");
  assert.equal(c.read().setup.primary, a.key);
  assert.equal((await c.route("POST", "kuvo/sessions/ses_owned/send", { id: "msg_local", words: "Keep going." })).status, 200);
  assert.deepEqual(prompts.map(prompt => prompt.id), ["msg_local"]);
  const work = await c.route("POST", "kuvo/work-queue/items", { owner: "ses_owned", title: "Continue on Lab" }, true);
  assert.equal(work.status, 200);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${work.value.item.id}`)).value.item.title, "Continue on Lab");
  const forwarded = await b.route("POST", "kuvo/work-queue/items", { owner: "ses_owned", title: "Desk sends to Lab" }, true);
  assert.equal(forwarded.status, 200);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${forwarded.value.item.id}`)).value.item.title, "Desk sends to Lab");
});

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

test("failure bench: an SSH-enrolled member advertises its accepted epoch and catches up", { timeout: 35_000 }, async t => {
  const { a, b, c, see } = await network(t);
  assert.equal((await submit(a, b, 1)).status, 202);
  await Promise.all([see(a, 2, b), see(b, 2, b), see(c, 2, b)]);
  assert.equal((await submit(b, a, 2)).status, 202);
  await Promise.all([see(a, 3, a), see(b, 3, a), see(c, 3, a)]);
  await c.stop("SIGTERM");

  const primary = a.read();
  const gift = link.ownDoc(primary, "peers", link.primaryPeersBody(primary));
  const joined = { ...c.identity, mac: a.key, macs: [a.key], macSigns: { [a.key]: a.sign }, phones: [] };
  // SSH gives a fresh member the current signed roster, without the preceding epoch's authority.
  link.docsWork({ link: joined, library: { personalities: [], removed: [] } }, a.entry, { give: [gift.envelope] }, { stay: a.key });
  assert.equal(joined.setup.epoch, 3);
  assert.equal(joined.leadership, undefined);
  assert.ok(link.digestOf(joined).some(entry => entry[1] === "leadership" && entry[2] === 3));
  fs.writeFileSync(path.join(c.run, "link.json"), JSON.stringify(joined));

  assert.equal((await submit(a, b, 3)).status, 202);
  await Promise.all([see(a, 4, b), see(b, 4, b)]);
  c.start();
  await see(c, 4, b);
  assert.ok(c.read().leadership[4]);
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

async function standbyNetwork(t, releaseKey) {
  const net = await network(t, true, releaseKey);
  const { a, b, see } = net;
  await see(a, 2, a);
  assert.equal((await a.route("GET", "kuvo/setup")).value.standby, b.key);
  await until("primary leased", () => a.lease(2, b), t.signal);
  return net;
}

async function releaseNetwork(t) {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const pin = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
  const net = await standbyNetwork(t, pin);
  const { a, b, see } = net;
  await see(b, 2, a);
  const root = path.dirname(a.run);
  const release = path.join(root, "release");
  const manifestPath = path.join(release, "boxes.json");
  const pack = Buffer.from("This fixture is deliberately not a tarball.");
  const manifestFor = version => Buffer.from(JSON.stringify({
    kuvo: "box/1", version, engine: "1.0.0",
    pack: { url: "https://example.invalid/pack.tgz", sha256: crypto.createHash("sha256").update(pack).digest("hex") },
  }));
  fs.mkdirSync(release);
  fs.writeFileSync(path.join(release, "pack.tgz"), pack);
  const offer = version => {
    const manifest = manifestFor(version);
    fs.writeFileSync(manifestPath, JSON.stringify({
      manifest: manifest.toString("base64"), key: pin,
      signature: crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), privateKey).toString("base64"),
    }));
  };
  fs.mkdirSync(path.join(root, "bin"), { recursive: true });
  fs.writeFileSync(path.join(root, "bin", "box-version"), "0.9.0");
  return { ...net, release, manifestPath, offer };
}

test("failure bench: a local release retry returns its committed receipt after the manifest is restored", { timeout: 45_000 }, async t => {
  const { a, b, relay, release, manifestPath, offer } = await releaseNetwork(t);
  const kind = "setupSecurityAndReleaseChoice";
  offer("0.10.0");
  const resume = relay.pause(a.address, b.address);
  t.after(resume);
  const first = await a.updateFrom(release);
  assert.equal(first.result, "waiting");
  assertReceipt(a, kind, "pending", first.save);
  resume();
  await until("the release choice committed", () => a.read().sharedOutcomes?.[first.save.operation]?.status === "committed", t.signal);

  const manifest = fs.readFileSync(manifestPath);
  fs.unlinkSync(manifestPath);
  const unreadable = await a.updateFrom(release);
  assert.equal(unreadable.result, "failed");
  assert.equal(unreadable.message, "That release can't be read.");
  assert.equal(unreadable.save, undefined);
  fs.writeFileSync(manifestPath, manifest);

  const recovered = await a.updateFrom(release);
  assert.equal(recovered.result, "failed"); // The deliberately invalid pack can't be unpacked.
  assertReceipt(a, kind, "committed", recovered.save);
  assert.equal(recovered.save.operation, first.save.operation);
  assert.deepEqual(recovered.save.proof, first.save.proof);
  const repeated = await a.updateFrom(release);
  assert.equal(repeated.result, "failed");
  assert.equal(repeated.save, undefined);
});

test("failure bench: normal save callers carry proofs to the successor", { timeout: 45_000 }, async t => {
  const bun = testBun();
  assert.ok(bun, NO_BUN);
  const { a, b, c, relay, see, release, offer } = await releaseNetwork(t);
  const settings = fileURLToPath(new URL("../src/agents/usage/settings.ts", import.meta.url));
  const usage = async (box, method, rest, body) => {
    const { status, value, err } = await box.evaluate(bun, ["-e", `
      import { UsageSettings, settingsRoutes } from ${JSON.stringify(settings)};
      const settings = new UsageSettings(undefined, undefined, undefined, ${JSON.stringify(box.run)});
      const reply = await settingsRoutes(settings, () => ["gpt-test", "gpt-next"], ${JSON.stringify(method)}, ${JSON.stringify(rest)}, async () => (${JSON.stringify(body)}));
      console.log(JSON.stringify({ status: reply.status, value: await reply.json() }));
    `]);
    assert.equal(status, 0, err);
    return value;
  };

  const changeZone = async value => {
    const clock = await a.route("GET", "kuvo/settings/clock", undefined, true);
    assert.equal(clock.status, 200);
    const row = clock.value.groups.flatMap(group => group.rows).find(row => row.id === "time-zone");
    assert.ok(row);
    return a.route("PATCH", "kuvo/settings/clock", { row: row.id, was: row.revision, value }, true);
  };
  const takeLibrary = document => a.route("POST", "kuvo/personalities/take", { document }, true);
  const editPersonality = name => a.route("POST", "kuvo/personalities", { name, description: "A receipt-backed edit." }, true);
  offer("0.9.9");
  const [savedDisplay, savedLabel, savedName, savedZone, savedLibrary, savedRelease] = await Promise.all([
    usage(a, "PUT", [], { codexFallback: "gpt-test" }),
    a.route("POST", "kuvo/work-queue/labels", { add: "Online label" }, true),
    rename(a, c, "Online name"),
    changeZone("America/New_York"),
    takeLibrary({ personalities: [personality(2)], removed: [] }),
    a.updateFrom(release),
  ]);
  for (const answer of [savedDisplay, savedLabel, savedName, savedZone, savedLibrary]) {
    assert.equal(answer.status, 200, JSON.stringify(answer));
  }
  // The release choice commits before unpacking. The fixture's invalid tarball stops installation.
  assert.equal(savedRelease.result, "failed", JSON.stringify(savedRelease));
  const savedTool = await a.labelTool({ add: "Online tool label" });
  assert.equal(savedTool.isError, false);
  const savedPersonality = await editPersonality("Online personality");
  assert.equal(savedPersonality.status, 200, JSON.stringify(savedPersonality));
  for (const [kind, save] of [
    ["setupUsageDisplay", savedDisplay.value.data.save], ["queueLabels", savedLabel.value], ["queueLabels", savedTool.save],
    ["setupNames", savedName.value], ["setupTimeZone", savedZone.value], ["setupPersonalities", savedLibrary.value],
    ["setupPersonalities", savedPersonality.value], ["setupSecurityAndReleaseChoice", savedRelease.save],
  ]) assertReceipt(a, kind, "committed", save);

  // The individual edit and the library merge share a kind. Settle the edit before staging the merge.
  const holdEdit = relay.pause(a.address, b.address);
  t.after(holdEdit);
  const pendingEdit = await editPersonality("Outage personality");
  assert.equal(pendingEdit.status, 202, JSON.stringify(pendingEdit));
  assertReceipt(a, "setupPersonalities", "pending", pendingEdit.value);
  holdEdit();
  await until("the edit settled", () => a.read().sharedOutcomes?.[pendingEdit.value.operation]?.status === "committed", t.signal);

  offer("0.10.0");
  const resume = relay.pause(a.address, b.address);
  t.after(resume);
  const args = { add: "Outage label" };
  const document = { personalities: [personality(1)], removed: [] };
  const [display, tool, update, label, name, removal, zone, library, sshLibrary] = await Promise.all([
    usage(a, "PUT", [], { codexFallback: "gpt-next" }),
    a.labelTool(args),
    a.updateFrom(release),
    a.route("POST", "kuvo/work-queue/labels", args, true),
    rename(a, c, "Outage name"),
    a.route("DELETE", `kuvo/computers/${c.key}`),
    changeZone("Asia/Tokyo"),
    takeLibrary(document),
    a.command(["box", "personalities"], JSON.stringify(document)),
  ]);
  assert.equal(display.status, 202);
  assert.equal(update.result, "waiting", JSON.stringify(update));
  assert.equal(tool.isError, false);
  for (const answer of [label, name, removal, zone, library]) assert.equal(answer.status, 202, JSON.stringify(answer));
  assert.equal(sshLibrary.status, 0, sshLibrary.err);
  assert.equal(sshLibrary.value.status, 202, JSON.stringify(sshLibrary.value));
  const repeated = await a.updateFrom(release);
  assert.deepEqual(repeated.save?.proof, update.save?.proof);
  assert.equal(repeated.save?.operation, update.save?.operation);
  const automatic = await a.updateFrom();
  assert.equal(automatic.result, "waiting", JSON.stringify(automatic));
  assert.deepEqual(automatic.save, update.save);
  await a.stop();
  b.clock(3 * HOUR + 5_000);
  await see(b, 3, b);

  const receipts = [
    ["setupUsageDisplay", display.value], ["queueLabels", tool.save], ["queueLabels", label.value],
    ["setupSecurityAndReleaseChoice", update.save], ["setupNames", name.value], ["membershipAndAccess", removal.value],
    ["setupTimeZone", zone.value], ["setupPersonalities", library.value],
    ["setupPersonalities", sshLibrary.value.save],
  ];
  for (const [kind, save] of receipts) {
    assertReceipt(a, kind, "pending", save);
    assert.equal((await link.askSetupStatus(b.run, { id: save.setup, operation: save.operation })).status, 404, kind);
  }
  assert.ok(tool.text.endsWith(JSON.stringify(tool.save)));
  const queriedUsage = await usage(b, "POST", ["save"], display.value);
  assert.equal(queriedUsage.value.status, "interrupted");
  assert.equal(queriedUsage.value.operation, display.value.operation);
  const queriedTool = await b.labelTool({ save: tool.save });
  assert.equal(queriedTool.save?.status, "interrupted");
  assert.deepEqual(queriedTool.save.proof, tool.save.proof);
  for (const [kind, save] of receipts) {
    const outcome = await link.askSetupOutcome(b.run, { setup: save.setup, operation: save.operation, proof: save.proof });
    assert.equal(outcome.status, 200, kind);
    assert.equal(outcome.receipt.status, "interrupted", kind);
  }
});

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
  // The primary is still partitioned. Ask the successor using the receipt from the save.
  assert.equal(blocked.value.operation, interrupted);
  const lostOnSuccessor = await link.askSetupOutcome(b.run, { setup: stale.setup.id, operation: interrupted, proof: blocked.value.proof });
  assert.equal(lostOnSuccessor.status, 200);
  assert.equal(lostOnSuccessor.receipt.status, "interrupted");
  assert.equal(lostOnSuccessor.receipt.operation, interrupted);
  const checked = link.readDoc(blocked.value.proof);
  const forgedProof = link.signDoc(c.seed, "setup-shared", checked.n, checked.body);
  assert.equal((await link.askSetupOutcome(b.run, { setup: stale.setup.id, operation: interrupted, proof: forgedProof })).status, 404);
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
  await see(c, 4, b);
  assert.equal((await link.askSetupStatus(c.run, { id: stale.setup.id, operation: interrupted })).outcome, "interrupted");
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
  const lost = await link.askSetupStatus(a.run, { id: stale.setup.id, operation: interrupted });
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

test("failure bench: access routes return receipts with and without a standby acknowledgement", { timeout: 90_000 }, async t => {
  const { a, b, c, relay } = await standbyNetwork(t);
  const device = name => ({ key: link.publicOf(link.newKey()).toString("base64url"), name, kind: "desktop" });
  const pair = (box, asking) => box.route("POST", "kuvo/devices/pair", asking, true);
  const remove = asking => a.route("POST", "kuvo/phones/remove", { phones: [{ key: asking.key }] }, true);
  const allow = () => a.route("POST", `kuvo/computers/${c.key}/allow-devices`, undefined, true);
  const access = async (status, request) => {
    const resume = status === "pending" ? relay.pause(a.address, b.address) : () => {};
    t.after(resume);
    const reply = await request();
    assert.equal(reply.status, status === "pending" ? 202 : 200, JSON.stringify(reply));
    assertReceipt(a, "membershipAndAccess", status, reply.value);
    resume();
    if (status === "pending") {
      await until("access change settled", () => a.read().sharedOutcomes?.[reply.value.operation]?.status === "committed", t.signal);
    }
    return reply.value;
  };

  // The fixture starts with this voucher blocked. Removing its app blocks it again.
  for (const status of ["pending", "committed"]) {
    await access(status, allow);
    const local = device(`Local ${status}`);
    const member = device(`Vouched ${status}`);
    await access(status, () => pair(a, local));
    const vouched = await access(status, () => pair(c, member));
    if (status === "committed") assert.equal(vouched.paired, true);
    await access(status, () => remove(local));
    await access(status, () => remove(member));
  }
  await access("committed", () => a.route("DELETE", `kuvo/computers/${c.key}`));
});

test("failure bench: forwarded computer changes carry receipts through the member route", { timeout: 75_000 }, async t => {
  for (const status of ["pending", "committed"]) {
    await t.test(status, async t => {
      const { a, b, c, relay } = await standbyNetwork(t);
      // Keep both directions of forwarding open while holding the primary's proposal to the standby.
      const release = status === "pending" ? relay.intercept(a.address, b.address, packet => {
        const give = link.openMail(link.readFrame(packet.payload).body, b.read())?.payload?.docs?.give;
        return give?.some(envelope => {
          const doc = link.readDoc(envelope);
          return doc?.kind === "setup-shared" && !doc.body.published && !doc.body.receipt
            && ["setupNames", "membershipAndAccess"].includes(doc.body.kind);
        });
      }) : () => {};
      t.after(release);
      for (const [kind, reply] of [
        ["setupNames", await rename(b, c, `Forwarded ${status}`)],
        ["membershipAndAccess", await b.route("DELETE", `kuvo/computers/${c.key}`)],
      ]) {
        assert.equal(reply.status, status === "pending" ? 202 : 200, JSON.stringify(reply));
        assertReceipt(a, kind, status, reply.value);
      }
    });
  }
});

test("failure bench: a QR handshake carries its durable pairing receipt to the successor", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  const pairing = await a.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200);
  const qr = new URL(pairing.value.link);
  const newKey = link.newKey();
  const deviceKey = link.publicOf(newKey).toString("base64url");
  async function open(box, key, hello, session) {
    const device = relay.device();
    const initiator = new link.IKInitiator(key, Buffer.from(box.key, "base64url"), link.PROLOGUE);
    device.send(box.address, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from(JSON.stringify({
      v: 1, name: "New QR phone", ...hello,
    })))));
    const frame = link.readFrame(link.readRelay(await device.next(15_000)).payload);
    if (frame.kind !== link.Frame.handshake2) return { frame: frame.kind };
    const { payload, transport } = initiator.readMessage2(frame.body);
    return { hello: JSON.parse(payload), transport, device };
  }
  const receipts = [];
  const release = relay.intercept(b.address, a.address, packet => {
    // Keep leases flowing while holding the signed access receipt.
    const envelope = link.openMail(link.readFrame(packet.payload).body, a.read())?.payload?.docs?.give?.[0];
    if (link.readDoc(envelope)?.body.receipt) { receipts.push(packet); return true; }
    return false;
  });
  t.after(release);
  const pending = await open(a, newKey, { pair: qr.searchParams.get("token") }, 44);
  const save = pending.hello.save;
  assert.equal(pending.hello.v, 1);
  assert.ok(pending.hello.waiting);
  assert.equal(save.status, "pending");
  assert.equal(save.pending, true);
  assert.equal(save.operation, pendingOperation(a, "membershipAndAccess"));
  assert.equal(link.readDoc(save.proof).origin, a.sign);
  assert.ok(receipts.length);
  assert.ok(b.read().phones.some(phone => phone.key === deviceKey));
  assert.ok(!a.read().phones.some(phone => phone.key === deviceKey));
  pending.device.send(a.address, link.frame(link.Frame.transport, 44, pending.transport.seal(
    link.encodeMessage({ t: "req", id: 1, m: "GET", p: "kuvo/setup", end: true }, Buffer.alloc(0)),
  )));
  assert.equal(link.readFrame(link.readRelay(await pending.device.next(5_000)).payload).kind, link.Frame.close);
  const recovered = await open(a, newKey, { pair: qr.searchParams.get("token") }, 45);
  assert.deepEqual(recovered.hello.save, save);

  await a.stop();
  b.clock(3 * HOUR + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  const forged = await open(b, link.newKey(), { save }, 46);
  assert.equal(forged.frame, link.Frame.close);
  const committed = await open(b, newKey, { save }, 47);
  assert.equal(committed.hello.waiting, undefined);
  assert.deepEqual(committed.hello.save, {
    setup: save.setup, epoch: save.epoch, operation: save.operation, proof: save.proof, status: "committed",
  });
  assert.ok(b.read().phones.some(phone => phone.key === deviceKey));
});

test("failure bench: member uninstall keeps its pending revocation and retries the same save", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  await see(c, 2, a);
  const identity = path.join(c.env.HOME, "desktop", "identity");
  fs.mkdirSync(path.dirname(identity));
  fs.writeFileSync(identity, "keep this identity while pending");
  const restore = relay.pause(a.address, b.address);
  t.after(restore);
  const pending = await c.command(["uninstall"]);
  assert.equal(pending.status, 1, pending.err);
  assert.equal(pending.value.result, "pending");
  const save = pending.value.save;
  assert.equal(save.status, "pending");
  assert.equal(save.operation, pendingOperation(a, "membershipAndAccess"));
  assert.equal(link.readDoc(save.proof).origin, a.sign);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(c.run, "uninstall-save.json"), "utf8")), save);
  assert.equal(fs.readFileSync(identity, "utf8"), "keep this identity while pending");
  assert.equal(c.child.exitCode, null);
  assert.equal(fs.existsSync(path.join(c.run, "off")), false);
  const stillPending = await c.command(["uninstall"]);
  assert.equal(stillPending.status, 1, stillPending.err);
  assert.deepEqual(stillPending.value.save, save);
  restore();
  await until("revocation committed", () => a.read().sharedOutcomes?.[save.operation]?.status === "committed", t.signal);
  const finished = await c.command(["uninstall"]);
  assert.equal(finished.status, 0, finished.err);
  assert.equal(finished.value.result, "uninstalled");
  assert.deepEqual(finished.value.save, {
    setup: save.setup, epoch: save.epoch, operation: save.operation, proof: save.proof, status: "committed",
  });
  assert.equal(fs.existsSync(identity), false);
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
  assert.equal(saved.value.save?.status, "committed");
  const { setup, operation, proof } = saved.value.save;
  assert.equal(link.readDoc(proof)?.body.operation, operation);
  assert.equal((await link.askSetupOutcome(b.run, { setup, operation, proof })).receipt.status, "committed");
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
  for (const [box, name, sequence] of [[b, "Renamed Desk", 1], [c, "Accepted name", 2]]) {
    const done = await rename(a, box, name);
    const { setup, sharedOutcomes } = b.read();
    const operation = Object.keys(sharedOutcomes).find(id => sharedOutcomes[id].kind === "setupNames"
      && sharedOutcomes[id].sequence === sequence);
    assert.ok(operation, "the standby durably accepted the operation");
    const proof = link.signDoc(a.seed, "setup-shared", sequence, {
      id: setup.id, epoch: 2, operation, kind: "setupNames", baseRevision: sequence - 1, proposal: sharedOutcomes[operation].proposal,
    });
    assert.deepEqual(done, { status: 200, value: { name, setup: setup.id, epoch: 2, operation, proof, status: "committed" } });
  }
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
  assert.ok(display.receipt.operation);
  assert.equal(pendingOperation(a, usage.kind), display.receipt.operation);
  assert.equal((await link.askSetupStatus(a.run, { id: usage.id, operation: display.receipt.operation })).outcome, "pending");
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
  assert.equal((await link.askSetupStatus(a.run, { id: usage.id, operation: display.receipt.operation })).outcome, "committed");
  await until("display preferences published to an ordinary member", () => c.read().shared?.setupUsageDisplay?.sequence === 1, t.signal);
  const later = await link.askSetupSave(a.run, { ...usage, sequence: 1, value: { units: {}, imported: false } });
  assert.equal(later.status, 200);
  assert.notEqual(later.receipt.operation, display.receipt.operation);
  assert.equal((await link.askSetupStatus(b.run, { id: usage.id, operation: display.receipt.operation })).outcome, "committed");
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

const editAddress = (owner, id) => ({ kind: "queueItems", queue: "ses_owned", id, computer: owner.key });
const editPath = ref => `kuvo/data/edits?${new URLSearchParams(ref)}`;
const editRead = (box, ref) => box.route("GET", editPath(ref), undefined, true);
const editSubmit = (box, ref, edit) => box.route("POST", "kuvo/data/edits", { ref, ...edit }, true);
async function acceptedEdit(box, operation, t) {
  while (!t.signal.aborted) {
    const result = await box.route("GET", `kuvo/data/edits/${operation}`, undefined, true);
    assert.equal(result.status, 200);
    if (result.value.receipt.status !== "pending") return result.value;
    await delay(100);
  }
  assert.fail(`No terminal receipt for ${operation}`);
}
const itemID = () => `wqi_${crypto.randomBytes(15).toString("base64url")}`;

test("failure bench: offline fields and notes converge in either reconnect order, and a clash is recorded once", { timeout: 85_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, true);
  for (const [first, second] of [[a, b], [b, a]]) {
    const ref = editAddress(c, itemID());
    const create = { operation: `create-${ref.id}`, baseRevision: null, create: true, fields: { title: "Original" },
      notes: [{ id: "draft", text: "Alpha red. Beta green." }] };
    const allowCreate = relay.pause(first.address, c.address);
    const made = await editSubmit(first, ref, create);
    assert.equal(made.status, 202, JSON.stringify(made));
    assert.deepEqual((await editRead(c, ref)).value.fields, {});
    allowCreate();
    assert.equal((await acceptedEdit(first, made.value.operation, t)).receipt.status, "committed");
    assert.equal((await editSubmit(first, ref, create)).value.status, "committed");
    const [left, right] = await Promise.all([editRead(first, ref), editRead(second, ref)]);
    assert.equal(left.status, 200, JSON.stringify(left));
    assert.equal(right.status, 200, JSON.stringify(right));
    const baseRevision = left.value.revision;
    assert.equal(right.value.revision, baseRevision);

    const reconnectFirst = relay.pause(first.address, c.address);
    const reconnectSecond = relay.pause(second.address, c.address);
    const attachment = { id: "same-file", file: { uri: "data:text/plain;base64,cXVldWUgYXR0YWNobWVudA==", name: "queue.txt" } };
    const one = { operation: `first-${ref.id}`, baseRevision, fields: { title: first.name, "note:draft": "Alpha blue. Beta green." },
      notes: [{ id: "first-note", text: "First addition" }], attachments: [attachment] };
    const two = { operation: `second-${ref.id}`, baseRevision, fields: { title: second.name, status: "waiting", "note:draft": "Alpha red. Beta orange." },
      notes: [{ id: "second-note", text: "Second addition" }], attachments: [attachment] };
    const [sentOne, sentTwo] = await Promise.all([editSubmit(first, ref, one), editSubmit(second, ref, two)]);
    assert.equal(sentOne.status, 202, JSON.stringify(sentOne));
    assert.equal(sentTwo.status, 202, JSON.stringify(sentTwo));
    assert.equal((await editRead(c, ref)).value.fields.title, "Original");
    reconnectFirst();
    assert.equal((await acceptedEdit(first, sentOne.value.operation, t)).receipt.status, "committed");
    const releaseReceipt = relay.pause(c.address, second.address);
    reconnectSecond();
    let owner;
    while (!t.signal.aborted) {
      owner = await editRead(c, ref);
      if (owner.value.choices?.length) break;
      await delay(100);
    }
    assert.equal(owner.status, 200);
    assert.equal(owner.value.choices.length, 1);
    assert.equal(owner.value.choices[0].accepted, first.name);
    assert.equal(owner.value.choices[0].proposed, second.name);
    assert.equal(owner.value.fields.status, "waiting");
    assert.equal(owner.value.fields["note:draft"], "Alpha blue. Beta orange.");
    assert.equal(owner.value.fields["note:first-note"], "First addition");
    assert.equal(owner.value.fields["note:second-note"], "Second addition");

    await c.stop("SIGTERM");
    releaseReceipt();
    c.start();
    const clash = (await acceptedEdit(second, sentTwo.value.operation, t)).receipt;
    assert.equal(clash.status, "choice");
    assert.deepEqual(clash.choices, [owner.value.choices[0].id]);
    const both = await Promise.all([editRead(first, ref), editRead(second, ref)]);
    for (const result of both) {
      assert.equal(result.status, 200, JSON.stringify(result));
      assert.equal(result.value.stale, false);
      assert.deepEqual(result.value.choices, owner.value.choices);
    }
    const item = await c.route("GET", `kuvo/work-queue/items/${ref.id}`);
    assert.equal(item.status, 200);
    assert.deepEqual(item.value.item.files.map(file => file.name), ["queue.txt"]);
    const resolved = await editSubmit(c, ref, { operation: `resolve-${ref.id}`, baseRevision: owner.value.revision,
      resolve: { id: clash.choices[0], use: "proposed" } });
    assert.equal(resolved.value.status, "committed", JSON.stringify(resolved));
    const current = (await editRead(c, ref)).value;
    assert.equal(current.fields.title, second.name);
    assert.deepEqual(current.choices, []);
  }
});

test("failure bench: deletion recovers late text and a long-absent copy must refresh", { timeout: 55_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, true);
  const ref = editAddress(c, itemID());
  const edit = { operation: "creation", create: true, baseRevision: null, fields: { title: "Original" } };
  const made = await editSubmit(c, ref, edit);
  assert.equal(made.value.status, "committed", JSON.stringify(made));
  const baseRevision = (await editRead(b, ref)).value.revision;
  assert.equal((await editRead(a, ref)).value.revision, baseRevision);
  const reconnectB = relay.pause(b.address, c.address);
  const reconnectA = relay.pause(a.address, c.address);
  const first = await editSubmit(b, ref, { operation: "late", baseRevision, fields: { body: "Text to recover" } });
  const last = await editSubmit(a, ref, { operation: "months-late", baseRevision, fields: { body: "Text from the old copy" } });
  assert.equal(first.status, 202);
  assert.equal(last.status, 202);
  assert.equal((await editSubmit(c, ref, { operation: "delete", baseRevision, delete: true })).value.status, "committed");
  reconnectB();
  const deleted = await acceptedEdit(b, first.value.operation, t);
  assert.equal(deleted.receipt.status, "deleted");
  assert.deepEqual(deleted.recovery, { body: "Text to recover" });
  const outbox = await b.route("GET", "kuvo/data/edits/outbox", undefined, true);
  assert.equal(outbox.status, 200);
  assert.ok(outbox.value.entries.some(entry => entry.ref.id === ref.id && entry.receipt.operation === first.value.operation));
  assert.equal((await editRead(c, ref)).value.deleted, true);

  // Move the owner’s deletion past the supported window without changing any writer's clock.
  // Stand-in homes are the only files touched.
  const queue = path.join(c.run, "work-queue", "queues", "ses_owned.json");
  const kept = JSON.parse(fs.readFileSync(queue, "utf8"));
  kept.items.find(item => item.id === ref.id).removed.at -= 90 * 86_400_000;
  fs.writeFileSync(queue + ".bench", JSON.stringify(kept));
  fs.renameSync(queue + ".bench", queue);
  const snapshot = await editRead(c, ref);
  assert.equal(snapshot.value.deleted, true);
  assert.deepEqual(snapshot.value.fields, {});
  reconnectA();
  const old = await acceptedEdit(a, last.value.operation, t);
  assert.equal(old.receipt.status, "refresh");
  assert.deepEqual(old.recovery, { body: "Text from the old copy" });
  const fresh = await editRead(a, ref);
  assert.equal(fresh.value.stale, false);
  assert.equal(fresh.value.revision, old.receipt.revision);
  assert.equal(fresh.value.deleted, true);
  const resurrect = await editSubmit(c, ref, { ...edit, operation: "create-again" });
  assert.equal(resurrect.value.status, "refresh");
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${ref.id}`)).status, 404);
});

test("failure bench: setup preferences merge at the primary and await the standby receipt", { timeout: 55_000 }, async t => {
  const { a, b, c, relay } = await standbyNetwork(t);
  const ref = { kind: "setupUsageDisplay" };
  const read = await editRead(c, ref);
  assert.equal(read.status, 200, JSON.stringify(read));
  const { revision: baseRevision } = read.value;
  const release = relay.pause(b.address, a.address);
  const first = await editSubmit(c, ref, { operation: "usage-card", baseRevision, fields: { totalSpend: { show: true } } });
  assert.equal(first.status, 202);
  await until("standby accepted the first preference", () => b.read().shared?.setupUsageDisplay?.value?.units?.totalSpend?.value?.show, t.signal);
  const pending = await c.route("GET", `kuvo/data/edits/${first.value.operation}`, undefined, true);
  assert.equal(pending.value.receipt.status, "pending");
  release();
  assert.equal((await acceptedEdit(c, first.value.operation, t)).receipt.status, "committed");
  const second = await editSubmit(c, ref, { operation: "usage-model", baseRevision, fields: { codexFallback: "gpt-5.4" } });
  assert.equal((await acceptedEdit(c, second.value.operation, t)).receipt.status, "committed");
  const actual = await editRead(a, ref);
  assert.deepEqual(actual.value.fields, { codexFallback: "gpt-5.4", totalSpend: { show: true } });
  const third = await editSubmit(c, ref, { operation: "usage-other-model", baseRevision, fields: { codexFallback: "gpt-5.3-codex" } });
  const conflict = (await acceptedEdit(c, third.value.operation, t)).receipt;
  assert.equal(conflict.status, "choice");
  const copies = [a, b, c].map(box => box.read().shared?.setupUsageDisplay?.offlineEdits?.choices);
  assert.equal(copies[0][0].id, conflict.choices[0]);
  assert.deepEqual(copies[0], copies[1]);
  const current = await editRead(c, ref);
  assert.deepEqual(current.value.choices, copies[0]);
  await a.stop();
  b.clock(3 * HOUR + 5_000);
  await until("standby promoted with accepted preference choices", () => b.read().setup?.role === "primary" && b.read().setup?.epoch === 3, t.signal);
  const after = await editRead(b, ref);
  assert.deepEqual(after.value.fields, actual.value.fields);
  assert.deepEqual(after.value.choices, copies[0]);
});

test("failure bench: a bot's offline name and instructions are applied only at its home", { timeout: 45_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, true);
  const ref = { kind: "botSettings", id: "bot_Lab", computer: c.key };
  const home = path.dirname(c.run);
  const folder = path.join(home, "bots", ref.id);
  fs.mkdirSync(folder, { recursive: true });
  fs.mkdirSync(path.join(home, "bin"), { recursive: true });
  fs.copyFileSync(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/bot-words.json", import.meta.url), path.join(home, "bin", "bot-words.json"));
  fs.writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: ref.id, name: "Lab bot" }));
  fs.writeFileSync(path.join(folder, "session"), "ses_owned\n");
  fs.writeFileSync(path.join(folder, "job.json"), JSON.stringify({ line: "Report status", details: "Check the queue." }));
  const { value: before, status } = await editRead(a, ref);
  assert.equal(status, 200, JSON.stringify(before));
  const baseRevision = before.revision;
  const reconnect = relay.pause(a.address, c.address);
  const name = await editSubmit(a, ref, { operation: "name", baseRevision, fields: { name: "Ready bot" } });
  const instructions = await editSubmit(b, ref, { operation: "instructions", baseRevision, fields: { jobDetails: "Check the queue and summarize." } });
  assert.equal(name.status, 202);
  assert.equal((await acceptedEdit(b, instructions.value.operation, t)).receipt.status, "committed");
  assert.equal((await editRead(c, ref)).value.fields.name, "Lab bot");
  reconnect();
  assert.equal((await acceptedEdit(a, name.value.operation, t)).receipt.status, "committed");
  const { value: after } = await editRead(b, ref);
  assert.equal(after.fields.name, "Ready bot");
  assert.equal(after.fields.jobDetails, "Check the queue and summarize.");
  assert.deepEqual(after.choices, []);
  assert.equal(fs.existsSync(path.join(path.dirname(a.run), "bots", ref.id)), false);
  assert.equal(fs.existsSync(path.join(path.dirname(b.run), "bots", ref.id)), false);
});
