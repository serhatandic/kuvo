import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { NO_BUN, testBun } from "./bun-for-tests.js";
import { relayStandIn } from "./relay-stand-in.js";
import { runningAs } from "./running-as.js";
import { recoveryNeighborBuild } from "./failure-bench-legacy.js";
import { benchRelease, durablePhase, put, recoveryEngine } from "./recovery-bench.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const link = await import(script);
const TAKEOVER = 180_000;
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
    const value = await read();
    if (value) return value;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${what}`);
}

async function network(t, takeover = false, releaseKey, owned = false, movingGroup, disposableInstall = false, pairOnly = false, names,
  { beta35Owner = false, macPrimary = false, appOwner = false, acceptedRoster = false, oldPrimary = false, engineModels, prepare, recovery, phaseDiagnostics = false } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), disposableInstall ? "kd-" : "kuvo-handover-"));
  const phase = recovery || phaseDiagnostics ? durablePhase(t, root) : undefined;
  const recoveryScript = recovery && path.join(root, "neighbor.mjs");
  const fixtureEngine = recovery && recoveryEngine(root, "pc2", recovery.session);
  if (recovery) fs.writeFileSync(recoveryScript, recovery.build);
  const boxScript = releaseKey ? path.join(root, "kuvo-link.mjs") : script;
  const legacyScript = beta35Owner ? path.join(root, "beta35.mjs") : null;
  if (legacyScript) {
    const archived = fileURLToPath(new URL("./.beta35-owner.mjs", import.meta.url));
    if (fs.existsSync(archived)) {
      fs.copyFileSync(archived, legacyScript);
    } else {
      const old = spawnSync("git", ["--no-optional-locks", "show", "fccda2de1:swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"], { maxBuffer: 16_000_000 });
      assert.equal(old.status, 0, old.stderr.toString());
      fs.writeFileSync(legacyScript, old.stdout);
    }
  }
  if (releaseKey) {
    const source = fs.readFileSync(script, "utf8");
    assert.ok(source.includes(link.UPDATE_KEYS[0]));
    fs.writeFileSync(boxScript, source.replace(link.UPDATE_KEYS[0], releaseKey));
  }
  const oldScript = path.join(root, "old", "kuvo-link.js");
  if (oldPrimary) {
    const source = fs.readFileSync(boxScript, "utf8");
    const old = source.replace(/^ +"work-queue\.(?:forward\.memory|labels\.restore\.preview)",\n/gm, "");
    assert.notEqual(old, source, "the old box must omit the new forwarding features");
    fs.mkdirSync(path.dirname(oldScript));
    fs.writeFileSync(oldScript, old);
  }
  const relay = await relayStandIn(link);
  let labelCall;
  let macIdentity;
  const hostPassword = crypto.randomBytes(16).toString("hex");
  const prompts = [];
  const draftEffects = [];
  const groupMessages = [];
  const bridgePID = owned || movingGroup ? await runningAs("kuvo-agents", t) : null;
  const groupCopies = new Map(movingGroup ? [["Lab", { group: movingGroup, messages: [], status: "active" }]] : []);
  let ownedConversationPresent = true;
  const engine = http.createServer((request, response) => {
    if (fixtureEngine) {
      void fixtureEngine.handle(request, response).then(handled => {
        if (!handled) response.writeHead(404).end("{}");
      }, () => response.writeHead(500).end("{}"));
      return;
    }
    if ((macPrimary || appOwner) && request.url === "/hello") {
      request.once("data", data => {
        const { nonce } = JSON.parse(data);
        const proof = crypto.createHmac("sha256", hostPassword)
          .update(`kuvo-host-hello\0${nonce}\x00127.0.0.1:${engine.address().port}`).digest("hex");
        response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ proof }));
      });
      return;
    }
    if ((macPrimary || appOwner) && request.url === "/link/identity" && request.headers.authorization === `Bearer ${hostPassword}`) {
      request.resume();
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(macIdentity));
      return;
    }
    if (appOwner && request.url === "/keys/names" && request.headers.authorization === `Bearer ${hostPassword}`) {
      request.resume();
      if (fs.existsSync(path.join(root, "keeper-stopped"))) return response.writeHead(503).end("{}");
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ names: [] }));
      return;
    }
    const groupPath = movingGroup && /^\/api\/kuvo\/groups(?:\/(grp_[a-z0-9]+)(?:\/(hold|activate|release))?)?$/.exec(request.url);
    if (groupPath) {
      const box = Buffer.from(request.headers.authorization?.slice(6) ?? "", "base64").toString().split(":")[1];
      const chat = groupCopies.get(box);
      const send = (status, data) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify({ data }));
      if (!groupPath[1]) return send(200, chat ? [chat] : []);
      if (request.method === "PUT") {
        const parts = [];
        request.on("data", part => parts.push(part));
        request.on("end", () => { groupCopies.set(box, JSON.parse(Buffer.concat(parts))); send(200, true); });
        return;
      }
      if (!chat) return send(404, null);
      if (groupPath[2] === "hold") chat.status = "held";
      if (groupPath[2] === "activate") chat.status = "active";
      if (groupPath[2] === "release") groupCopies.delete(box);
      return send(200, chat);
    }
    if (owned) {
      const local = request.headers.authorization === "Basic " + Buffer.from("opencode:Lab").toString("base64");
      const send = (status, value) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
      const modelList = engineModels?.[request.url.split("?")[0]];
      if (local && modelList !== undefined) return send(200, { data: modelList });
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
        if (!local || !ownedConversationPresent) return send(404, {});
        const effect = engineModels && /\/(command|shell)$/.exec(request.url)?.[1];
        if (effect && request.method === "POST") {
          const parts = [];
          request.on("data", part => parts.push(part));
          request.on("end", () => {
            draftEffects.push({ effect, body: JSON.parse(Buffer.concat(parts)) });
            send(200, { data: {} });
          });
          return;
        }
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
        const model = engineModels?.["/api/model/default"];
        return send(200, { data: { id: "ses_owned", title: "Lab’s conversation", time: { updated: Date.now() }, ...(model ? { model } : {}) } });
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

  const boxes = (names ?? (pairOnly ? ["Studio", "Desk"] : ["Studio", "Desk", "Lab"])).map((name, index) => {
    const account = path.join(root, name);
    const appFolder = appOwner && index === 2 ? path.join(account, "Library", "Application Support", "run.kuvo.code.fixture") : null;
    const home = appFolder ? path.join(appFolder, "box") : disposableInstall ? path.join(account, ".kuvo-code") : account;
    const secret = crypto.randomBytes(32);
    const privateKey = link.newKey();
    const seed = link.newSigningSeed();
    const address = link.addressOf(secret);
    const key = link.publicOf(privateKey).toString("base64url");
    const sign = link.signingKeyOf(seed);
    const env = { HOME: account, KUVO_HOME: home, PATH: process.env.PATH, TMPDIR: root };
    if (appFolder) {
      const tools = path.join(account, "uninstall-tools");
      fs.mkdirSync(tools, { recursive: true });
      fs.writeFileSync(path.join(tools, "security"), '#!/bin/sh\nprintf "%s\\n" "$*" >> "$HOME/keychain-cleared"\n', { mode: 0o700 });
      Object.assign(env, { KUVO_BOX: "mac", KUVO_KEYS_ON_HOST: "1", KUVO_BOTS: path.join(appFolder, "bots"),
        KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_ENGINE_CONFIG: path.join(appFolder, "opencode", "config"),
        KUVO_MAC_ATTACHMENTS: path.join(appFolder, "Attachments"), KUVO_TEST_UNINSTALL_TOOLS: tools });
    }
    const mac = macPrimary && index === 0 || appFolder;
    const run = path.join(home, "run");
    fs.mkdirSync(path.join(run, "work-queue"), { recursive: true });
    fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ port: engine.address().port, password: owned || recovery ? name : "test" }));
    if (bridgePID && (name === "Lab" || movingGroup && name === "Studio")) fs.writeFileSync(path.join(run, "bridge.json"), JSON.stringify({
      pid: bridgePID, port: engine.address().port, password: name,
    }));
    const nodeScript = name === "Lab" && legacyScript ? legacyScript : recovery && index === 2 ? recoveryScript : boxScript;
    if (nodeScript === legacyScript) {
      fs.mkdirSync(path.join(home, "bin"), { recursive: true });
      fs.writeFileSync(path.join(home, "bin", "box-version"), "0.8.0-beta.35");
    }
    const read = file => JSON.parse(fs.readFileSync(path.join(run, file), "utf8"));
    const clock = offset => {
      const file = path.join(run, "test-setup-clock.json");
      fs.writeFileSync(file + ".new", JSON.stringify({ offset }));
      fs.renameSync(file + ".new", file);
    };
    if (takeover || prepare && index > 0) clock(0);
    if (takeover || prepare) fs.writeFileSync(path.join(home, "setup-takeover.json"), JSON.stringify({ alwaysOn: takeover && index === 1 }));
    return {
      key, sign, seed, address, name, home, run, env, appFolder,
      entry: { key, sign, address: address.toString("base64url"), name, kind: mac ? "mac" : "machine" },
      identity: { key: privateKey.toString("base64url"), sign: seed, secret: secret.toString("base64url"), name, relay: relay.url },
      read: () => read("link.json"),
      clock,
      lease(epoch, standby) {
        try { return read("setup-lease.json").stamp === `${this.read().setup.id}:${epoch}:${standby.key}`; } catch { return false; }
      },
      heard: key => {
        try { return read("peer-agents.json")[key]?.features?.includes("setup.handover"); } catch { return false; }
      },
      command(args, input = "") { return this.evaluate(process.execPath, [nodeScript, ...args], input); },
      async evaluate(command, args, input = "", json = true) {
        const child = start(env, args, ["pipe", "pipe", "pipe"], command);
        let out = "", err = "";
        child.stdout.on("data", chunk => { out += chunk; });
        child.stderr.on("data", chunk => { err += chunk; });
        child.stdin.end(input);
        const [status, signal] = await once(child, "close");
        return { status, signal, value: json && out.trim() ? JSON.parse(out.trim().split("\n").at(-1)) : null, out: out.trim(), err };
      },
      async route(method, path, body, local = false, headers = {}, { freezeClockAt } = {}) {
        const args = local ? ["--input-type=module", "-e", `
          import { engineHandler } from ${JSON.stringify(recovery ? pathToFileURL(nodeScript).href : pathToFileURL(script).href)};
          const chunks = [];
          for await (const chunk of process.stdin) chunks.push(chunk);
          const given = JSON.parse(Buffer.concat(chunks));
          const request = { ...given, fromApp: true, served: true, headers: given.headers, body: Buffer.from(JSON.stringify(given.body ?? {})) };
          let status, body = [];
          for await (const part of engineHandler()(request, {})) {
            if (part.head) status = part.head.status;
            if (part.body) body.push(part.body);
          }
          console.log(JSON.stringify({ status, body: Buffer.concat(body).toString() }));
        `] : [nodeScript, "box", "route"];
        if (freezeClockAt) {
          const clock = `
            import { existsSync } from "node:fs";
            const now = Date.now;
            let frozen;
            Date.now = () => {
              if (frozen === undefined && existsSync(${JSON.stringify(freezeClockAt)})) frozen = now();
              return frozen ?? now();
            };
          `;
          args.unshift("--import", `data:text/javascript,${encodeURIComponent(clock)}`);
        }
        const child = spawn(process.execPath, args, { env, stdio: ["pipe", "pipe", "pipe"] });
        children.add(child);
        child.once("exit", () => children.delete(child));
        let out = "", err = "";
        child.stdout.on("data", chunk => { out += chunk; });
        child.stderr.on("data", chunk => { err += chunk; });
        child.stdin.end(JSON.stringify({ method, path, body, headers }));
        const [status] = await once(child, "close");
        assert.equal(status, 0, err);
        const wire = JSON.parse(out.trim().split("\n").at(-1));
        return { status: wire.status, value: JSON.parse(wire.body) };
      },
      async labelTool(args) {
        labelCall = args;
        return (await this.route("POST", "kuvo/work-queue/tools", { name: "queue_labels", arguments: args, session: "ses_labels" }, true)).value;
      },
      async updateFrom(from, url = `http://127.0.0.1:${engine.address().port}/no-release`) {
        assert.ok(releaseKey);
        const args = ["update", "--now", "--key", releaseKey, "--url", url];
        const { status, value, err } = await this.command(from ? [...args, "--from", from] : args);
        assert.equal(status, 0, err);
        return value;
      },
      launch(command, args) { return start(env, args, ["ignore", "ignore", "inherit"], command); },
      start() {
        this.output = "";
        const flags = this.onShutdown ? ["--import", `data:text/javascript,${encodeURIComponent(this.onShutdown)}`] : [];
        const child = this.child = start(env, [...flags, oldPrimary && index === 0 ? oldScript : nodeScript, mac ? "serve" : "run"], [mac ? "pipe" : "ignore", "pipe", "inherit"]);
        if (mac) child.stdin.write(JSON.stringify({ password: "local-test" }) + "\n");
        child.stdout.on("data", chunk => {
          if (this.child !== child) return;
          this.output = (this.output + chunk).slice(-16_000);
          // The app's keeper records the server endpoint when it starts the app-owned Box.
          const port = appFolder && /"url":"http:\/\/127\.0\.0\.1:(\d+)"/.exec(chunk.toString())?.[1];
          if (port) fs.writeFileSync(path.join(run, "serve.json"), JSON.stringify({ pid: child.pid, port: Number(port), password: "local-test" }));
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
  const roster = { boxes: boxes.map(box => box.entry), moved: [], left: [] };
  const accepted = movingGroup || acceptedRoster ? { managed: true, checkpointed: true } : {};
  const book = link.signDoc(a.seed, "peers", 1, { setup, ...roster, ...accepted });
  const phone = link.publicOf(link.newKey()).toString("base64url");
  const pairing = crypto.randomBytes(16).toString("base64url");
  for (const box of boxes) {
    const mac = macPrimary && box === a || box.appFolder;
    if (mac) {
      const { key, secret, sign } = box.identity;
      macIdentity = { key, secret, sign };
      if (!box.appFolder) {
        fs.mkdirSync(path.join(box.home, "box"), { recursive: true });
        fs.writeFileSync(path.join(box.home, "box", "kind"), "mac\n");
      }
      fs.writeFileSync(path.join(box.run, "mac-link.json"), JSON.stringify({ name: box.name, relay: relay.url, phones: true }));
      fs.writeFileSync(path.join(box.run, "host.json"), JSON.stringify({ url: `http://127.0.0.1:${engine.address().port}`, password: hostPassword }));
    }
    fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify({
      ...(mac ? {} : box.identity),
      setup: { ...setup, ...accepted, role: box === a ? "primary" : "member", sign: a.sign },
      ...(accepted.checkpointed ? { sharedSetup: setup.id, shared: { membershipAndAccess: { sequence: 1, value: roster } } } : {}),
      ...(box === a ? {
        macPeers: boxes.slice(1).map(peer => peer.entry), phones: [{ key: phone, pairing, from: "pair", name: "Device" }],
        vouchBlocked: c ? [{ key: c.key, device: "Revoked", at: 1 }] : [],
      } : { mac: a.key, macs: [a.key], macSigns: { [a.key]: a.sign }, docs: { [`${a.sign} peers`]: book } }),
    }));
  }
  fs.writeFileSync(path.join(a.run, "work-queue", "numbers.json"), JSON.stringify({ primary: "self", next: 401, leases: [] }));
  if (movingGroup) for (const bot of movingGroup.members) {
    const dir = path.join(a.home, "bots", bot);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: bot }));
    fs.writeFileSync(path.join(dir, "session"), "ses_" + bot.slice(4));
  }
  // Start the members first so the primary can immediately hear their feature advertisement.
  prepare?.(boxes);
  phase?.("disposable fleet stores prepared");
  for (const box of [...boxes.slice(1), a]) box.start();
  try {
    await until("connected setup", () => boxes.every(box => relay.has(box.address)) && boxes.slice(1).every(box => a.heard(box.key) && box.heard(a.key)),
      appOwner ? AbortSignal.any([t.signal, AbortSignal.timeout(18_000)]) : t.signal);
  } catch (error) {
    if (!appOwner) throw error;
    throw new Error(boxes.map(box => `${box.name} connected=${relay.has(box.address)}: ${box.output}`).join("\n"), { cause: error });
  }
  const see = (box, epoch, primary) => until(`${box.name} at epoch ${epoch}`, () => {
    const setup = box.read().setup;
    return setup.epoch === epoch && setup.primary === primary.key && setup.role === (box === primary ? "primary" : "member") && setup;
  }, t.signal);
  return { boxes, a, b, c, relay, see, phone, pairing, prompts, draftEffects, groupMessages, groupCopies, fixtureEngine, phase,
    showOwnedConversation: present => { ownedConversationPresent = present; },
  };
}

test("failure bench: a real idle group move survives source loss and replacement restore", { timeout: 55_000 }, async t => {
  const group = { id: "grp_recovery", name: "Recovered group", members: ["bot_one", "bot_two"], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, groupCopies } = await network(t, false, undefined, false, group);
  const groupPath = "groups/" + group.id;
  const oldGroup = path.join(c.env.HOME, groupPath);
  fs.mkdirSync(oldGroup, { recursive: true });
  fs.writeFileSync(path.join(oldGroup, "group.json"), JSON.stringify({ kuvo: 1, status: "active", group }));
  fs.writeFileSync(path.join(oldGroup, "messages.json"), "[]");
  const folder = path.join(c.env.TMPDIR, "backups");
  fs.mkdirSync(folder);
  const destination = { kind: "disk", path: folder };
  const source = "b_" + crypto.randomBytes(16).toString("base64url");
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const point = "2026-11-19T000000.000Z-daily-0123456789abcdef";
  const backup = await c.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
    import { captureBackup, scratchFolder } from ${JSON.stringify(new URL("../src/link/backup/engine.ts", import.meta.url).href)};
    import { destinationStore } from ${JSON.stringify(new URL("../src/link/backup/destination.ts", import.meta.url).href)};
    const destination = ${JSON.stringify(destination)};
    const source = ${JSON.stringify(source)};
    const point = ${JSON.stringify(point)};
    const scratch = scratchFolder();
    const file = scratch + "/archive";
    const manifest = captureBackup(file, ${JSON.stringify(recoveryKey)}, source, point, Date.now(), [], scratch);
    await destinationStore(destination, source).put(point, file);
    console.log(JSON.stringify(manifest));
  `]);
  assert.equal(backup.status, 0, backup.err);
  assert.equal(backup.value.paired, true);

  const moved = await c.route("POST", "kuvo/moves", { kind: "group", id: group.id, to: a.key, receipt: true }, true);
  assert.equal(moved.status, 202, JSON.stringify(moved.value));
  const receipt = path.join(c.run, "moves", moved.value.move.id + ".outcome.json");
  await until("move completed", () => fs.existsSync(receipt), t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active", JSON.stringify(JSON.parse(fs.readFileSync(receipt))));
  assert.equal(groupCopies.has("Lab"), false);
  await c.stop("SIGTERM");
  fs.rmSync(c.env.HOME, { recursive: true });
  assert.equal((await a.route("DELETE", "kuvo/computers/" + c.key)).status, 200);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: c.key, replacement: b.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  await until("replacement ownership replica", () => b.read().shared?.ownershipAssignments?.value?.["replacement:" + c.key], t.signal);

  const recovered = await b.route("POST", "kuvo/box/backups/restore", { destination, source, id: point, recoveryKey, confirm: true }, true);
  assert.equal(recovered.status, 200, JSON.stringify(recovered.value));
  assert.ok(recovered.value.unavailable.some(entry => entry.entry === groupPath + "/group.json" && entry.reason === "moved"));
  assert.equal(fs.existsSync(path.join(b.env.HOME, groupPath, "group.json")), false);
  assert.equal(groupCopies.get("Studio")?.status, "active");
});

test("failure bench: an away owner leaves saved reads and accepts an unsent message on return", { timeout: 90_000 }, async t => {
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
  await Promise.all([b.stop("SIGTERM"), a.stop("SIGTERM")]);
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
  // Wait for the durable receipt without starting a new CLI process on every poll.
  const savedReceipt = async (id, description, readStatus) => {
    const file = path.join(b.run, "owned-data", "outbox", id + ".json");
    await until(description, () => JSON.parse(fs.readFileSync(file, "utf8")).receipt.status !== "pending",
      AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
    return (await readStatus()).value;
  };
  const receipt = await savedReceipt(operation, "conversation acceptance to reach the sender", check);
  assert.equal(receipt.status, "committed");
  assert.equal(receipt.operation, operation);
  assert.equal(receipt.owner, c.key);
  assert.ok(receipt.acceptedAt >= before.value.lastReceivedAt);
  assert.equal((await b.route("POST", "kuvo/sessions/ses_owned/send", message)).value.status, "committed");
  assert.equal((await c.route("POST", "kuvo/sessions/ses_owned/send", message)).status, 200);
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
  assert.deepEqual((await read(b)).value.data.messages.map(prompt => prompt.id), [message.id]);
  const groupOutcome = await savedReceipt(groupSend.value.operation, "group acceptance to reach the sender", groupReceipt);
  assert.equal(groupOutcome.status, "committed");
  assert.deepEqual(groupMessages.map(message => message.id), ["msg_group"]);
});

test("failure bench: a member runs its own conversation and Work Queue while the primary is away", { timeout: 35_000 }, async t => {
  const { a, b, c, prompts } = await network(t, false, undefined, true);
  await a.stop("SIGTERM");
  assert.equal(c.read().setup.primary, a.key);
  assert.equal((await c.route("POST", "kuvo/sessions/ses_owned/send", { id: "msg_local", words: "Keep going." })).status, 200);
  assert.deepEqual(prompts.map(prompt => prompt.id), ["msg_local"]);
  const work = await c.route("POST", "kuvo/work-queue/items", { owner: "ses_owned", operation: "continue-on-lab", title: "Continue on Lab" }, true);
  assert.equal(work.status, 200);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${work.value.item.id}`)).value.item.title, "Continue on Lab");
  const create = { owner: "ses_owned", title: "Desk sends to Lab", operation: "one-create-from-desk" };
  const forwarded = await b.route("POST", "kuvo/work-queue/items", create, true);
  assert.equal(forwarded.status, 200);
  assert.deepEqual(await b.route("POST", "kuvo/work-queue/items", create, true), forwarded);
  assert.deepEqual(await c.route("POST", "kuvo/work-queue/items", create, true), forwarded);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${forwarded.value.item.id}`)).value.item.title, "Desk sends to Lab");
});

for (const old of [false, true]) test(
  old ? "an updated sender holds new work away from a real beta.35 owner" : "a behind owner defers relayed conversation prompts and refuses forwarded Work Queue mutations",
  { timeout: 45_000 }, async t => {
  const version = "0.10.0", sha256 = "a".repeat(64);
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify({ clientProtocol: 1 })).toString("base64") })).toString("base64");
  const prepare = ([a, b, c]) => {
    // Give each closed store its accepted snapshot before any keeper can write over it.
    for (const box of [a, b, c]) {
      const state = box.read();
      state.setup.checkpointed = true;
      state.sharedSetup = state.setup.id;
      state.shared = { ...state.shared, setupSecurityAndReleaseChoice: { sequence: 1, value: { version, sha256, signed } } };
      fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify(state));
      const bin = path.join(box.home, "bin");
      fs.mkdirSync(bin, { recursive: true });
      fs.writeFileSync(path.join(bin, "box-version"), box === c ? "0.8.0-beta.35" : version);
      if (box !== c) {
        fs.mkdirSync(path.join(box.home, "updates"), { recursive: true });
        fs.writeFileSync(path.join(box.home, "updates", "installed.json"), JSON.stringify({ version, sha256 }));
      }
    }
  };
  const { b, c, relay, prompts } = await network(t, false, undefined, true, undefined, false, false, undefined, { beta35Owner: old, prepare });
  await until("the sender knows the conversation owner", () => {
    try {
      const peer = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[c.key];
      return peer?.list.some(entry => entry.session === "ses_owned");
    } catch { return false; }
  }, t.signal);
  if (old) {
    await until("the sender heard the real beta.35 owner", () => {
      try {
        const peer = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[c.key];
        return peer?.computer?.kuvo === "0.8.0-beta.35" && !peer.features.includes("update.fleet");
      } catch { return false; }
    }, t.signal);
  }
  const headers = { "x-kuvo-client-version": version };
  const message = { id: "msg_fleetadmission", words: "Run once the owner has updated." };
  const sent = await b.route("POST", "kuvo/sessions/ses_owned/send", message, true, headers);
  assert.equal(sent.status, 202, JSON.stringify(sent));
  const gotPrompt = () => relay.delivered.some(packet => {
    if (!packet.from.equals(b.address) || !packet.to.equals(c.address)) return false;
    const body = link.readFrame(packet.payload)?.body;
    return body && link.openMail(body, c.read())?.payload?.workQueue?.scopedOwnerMessage?.operation === sent.value.operation;
  });
  if (!old) await until("the prompt reached the behind owner", gotPrompt, t.signal);
  const create = { owner: "ses_owned", title: "Wait for the owner", operation: "fleet-admission-queue" };
  const queued = await b.route("POST", "kuvo/work-queue/items", create, true, headers);
  assert.equal(queued.status, 503, JSON.stringify(queued));
  assert.equal(queued.value.reason, "computer-update-required");
  assert.deepEqual(prompts, []);
  if (old) {
    await delay(5_500); // Give the sender a full outbox retry to expose an unexpected send.
    assert.equal(gotPrompt(), false);
    assert.deepEqual(prompts, []);
  }
  const receipt = () => b.route("GET", `kuvo/data/messages/${sent.value.operation}`, undefined, true, headers);
  assert.equal((await receipt()).value.status, "pending");

  if (old) return; // The real beta.35 owner remains behind for the duration of this test.
  fs.mkdirSync(path.join(c.home, "updates"), { recursive: true });
  fs.writeFileSync(path.join(c.home, "updates", "installed.json"), JSON.stringify({ version, sha256 }));
  fs.writeFileSync(path.join(c.home, "bin", "box-version"), version);
  const retried = await b.route("POST", "kuvo/work-queue/items", create, true, headers);
  assert.equal(retried.status, 200, JSON.stringify(retried));
  await until("the pending prompt commits after the owner updates", async () => (await receipt()).value.status === "committed", t.signal);
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
});

test("failure bench: an offline member follows two handovers and the former primary can be unlinked", { timeout: 50_000 }, async t => {
  const { a, b, c, see, phone, pairing } = await network(t);
  const refused = { status: 409, value: { message: "Choose another main computer first." } };
  assert.deepEqual(await c.route("DELETE", `kuvo/computers/${a.key}`), refused);
  assert.deepEqual(await submit(c, b, 1), { status: 202, value: { primary: b.key, epoch: 2 } });
  await Promise.all([see(a, 2, b), see(b, 2, b), see(c, 2, b)]);
  assert.equal(b.read().phones.find(device => device.key === phone)?.pairing, pairing);
  assert.equal(a.read().phones.find(device => device.key === phone)?.mac, b.key);
  assert.equal(b.read().vouchBlocked, undefined);
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
    status: 400, value: { message: "Only a device paired with your setup can remove devices." },
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


async function standbyNetwork(t, releaseKey) {
  const net = await network(t, true, releaseKey);
  const { a, b, see } = net;
  await see(a, 2, a);
  assert.equal((await a.route("GET", "kuvo/setup")).value.standby, b.key);
  await until("primary leased", () => a.lease(2, b), t.signal);
  return net;
}

async function watchingRanks(t, relay, primary, followers) {
  await until("all successors surveying the accepted epoch", () => {
    const state = primary.read();
    return followers.every(box => relay.delivered.some(packet => {
      if (!packet.from.equals(box.address) || !packet.to.equals(primary.address)) return false;
      const body = link.readFrame(packet.payload)?.body;
      const authority = body && link.openMail(body, state)?.payload?.setupAuthority;
      return authority?.kind === "tip" && authority.epoch === state.setup.epoch;
    }));
  }, t.signal);
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
  fs.mkdirSync(release);
  const offer = (version, { layout, url = "https://example.invalid/pack.tgz", released = "2021-01-01T00:00:00Z" } = {}) => {
    const files = path.join(release, "files");
    fs.mkdirSync(path.join(files, "bin"), { recursive: true });
    if (layout) fs.writeFileSync(path.join(files, "bin", "kuvo-link.js"), `// Kuvo folder layout ${layout} abcdef123456\n`);
    // Verify layout and commit the choice first. The missing version file then makes installation fail.
    fs.writeFileSync(path.join(files, "box.json"), JSON.stringify({ version, engine: "1.0.0", files: {}, released }));
    const packPath = path.join(release, "pack.tgz");
    assert.equal(spawnSync("tar", ["czf", packPath, "-C", files, "."], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
    const manifest = Buffer.from(JSON.stringify({
      kuvo: "box/1", version, engine: "1.0.0", spreads: true,
      released, ...(layout ? { layout } : {}),
      pack: { url, sha256: crypto.createHash("sha256").update(fs.readFileSync(packPath)).digest("hex") },
    }));
    fs.writeFileSync(manifestPath, JSON.stringify({
      manifest: manifest.toString("base64"), key: pin,
      signature: crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), privateKey).toString("base64"),
    }));
  };
  fs.mkdirSync(path.join(root, "bin"), { recursive: true });
  fs.writeFileSync(path.join(root, "bin", "box-version"), "0.9.0");
  return { ...net, release, manifestPath, offer };
}

test("the main leaves the accepted choice unchanged when a member cannot read the new pack layout", { timeout: 45_000 }, async t => {
  const { a, b, c, release, offer } = await releaseNetwork(t);
  const kind = "setupSecurityAndReleaseChoice";
  const previous = a.read().shared?.[kind];
  const facts = { [b.key]: { computer: { layout: 18 } }, [c.key]: { computer: { layout: 19 } } };
  fs.writeFileSync(path.join(a.run, "peer-agents.json"), JSON.stringify(facts));
  offer("0.10.0", { layout: 19 });
  const answer = await a.updateFrom(release);
  assert.equal(answer.result, "blocked");
  assert.match(answer.message, /support Kuvo folder layout 19/);
  assert.equal(answer.save, undefined);
  assert.deepEqual(a.read().shared?.[kind], previous);
  assert.ok([b, c].every(box => box.read().shared?.[kind]?.value?.version !== "0.10.0"));
});

test("the main discovers a successor while the selected pack waits in backoff", { timeout: 45_000 }, async t => {
  const { a, b, c, release, manifestPath, offer } = await releaseNetwork(t);
  const kind = "setupSecurityAndReleaseChoice";
  offer("0.10.0");
  const first = await a.updateFrom(release);
  assert.equal(first.result, "failed");
  assertReceipt(a, kind, "committed", first.save);
  const oldManifest = fs.readFileSync(manifestPath);
  const oldPack = fs.readFileSync(path.join(release, "pack.tgz"));
  // Leave an accepted offer in place and the failed pack within its retry cooldown.
  const offered = path.join(path.dirname(a.run), "updates", "offered");
  fs.mkdirSync(offered, { recursive: true });
  fs.writeFileSync(path.join(offered, "boxes.json"), oldManifest);
  fs.writeFileSync(path.join(offered, "pack.tgz"), oldPack);
  let manifestReads = 0, packReads = 0;
  const server = http.createServer((request, response) => {
    if (request.url === "/boxes.json") {
      manifestReads++;
      response.end(fs.readFileSync(manifestPath));
    } else if (request.url === "/pack.tgz") {
      packReads++;
      response.end(fs.readFileSync(path.join(release, "pack.tgz")));
    } else { response.writeHead(404); response.end(); }
  });
  server.listen(0, "127.0.0.1");
  t.after(() => server.close());
  await once(server, "listening");
  const url = `http://127.0.0.1:${server.address().port}`;
  assert.equal((await a.updateFrom(undefined, `${url}/boxes.json`)).result, "waiting");
  assert.equal(packReads, 0, "the selected failed pack still rests");
  offer("0.10.1", { url: `${url}/pack.tgz` });
  const next = await a.updateFrom(undefined, `${url}/boxes.json`);
  assert.equal(next.result, "failed"); // Both fixtures stop after selecting the release.
  assertReceipt(a, kind, "committed", next.save);
  assert.equal(manifestReads, 2);
  assert.equal(packReads, 1);
  await until("members observed the newer release", () => [a, b, c].every(box => box.read().shared?.[kind]?.value?.version === "0.10.1"), t.signal);
  const previous = a.read().shared[kind].value.sha256;
  const previousManifest = fs.readFileSync(manifestPath), previousPack = fs.readFileSync(path.join(release, "pack.tgz"));
  fs.mkdirSync(offered, { recursive: true });
  fs.writeFileSync(path.join(offered, "boxes.json"), previousManifest);
  fs.writeFileSync(path.join(offered, "pack.tgz"), previousPack);
  offer("0.10.1", { url: `${url}/pack.tgz`, released: "2022-01-01T00:00:00Z" });
  const rebuilt = await a.updateFrom(undefined, `${url}/boxes.json`);
  assertReceipt(a, kind, "committed", rebuilt.save);
  const rebuiltHash = a.read().shared[kind].value.sha256;
  assert.notEqual(rebuiltHash, previous);
  assert.equal(fs.existsSync(path.join(offered, "boxes.json")), false);
  fs.writeFileSync(manifestPath, previousManifest);
  fs.writeFileSync(path.join(release, "pack.tgz"), previousPack);
  const superseded = await a.updateFrom(undefined, `${url}/boxes.json`);
  assert.equal(superseded.result, "waiting");
  assert.equal(superseded.save, undefined);
  assert.equal(a.read().shared[kind].value.sha256, rebuiltHash);
});

test("failure bench: a local release retry returns its committed receipt after the manifest is restored", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, release, manifestPath, offer } = await releaseNetwork(t);
  const kind = "setupSecurityAndReleaseChoice";
  offer("0.10.0");
  const resume = relay.pause(a.address, b.address);
  t.after(resume);
  const first = await a.updateFrom(release);
  assert.equal(first.result, "waiting");
  assertReceipt(a, kind, "pending", first.save);
  resume();
  await until("the release choice committed", () => a.read().sharedOutcomes?.[first.save.operation]?.status === "committed", t.signal);
  await until("both members accepted the same release choice", () => [b, c].every(box => box.read().shared?.[kind]?.value?.version === "0.10.0"), t.signal);

  const manifest = fs.readFileSync(manifestPath);
  fs.unlinkSync(manifestPath);
  const unreadable = await a.updateFrom(release);
  assert.equal(unreadable.result, "failed");
  assert.equal(unreadable.message, "That release can't be read.");
  assert.equal(unreadable.save, undefined);
  fs.writeFileSync(manifestPath, manifest);

  const recovered = await a.updateFrom(release);
  assert.equal(recovered.result, "failed"); // The pack lacks the required box version file.
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
  const settings = fileURLToPath(new URL("../src/usage/settings.ts", import.meta.url));
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
  // The release choice commits after it checks the folder layout. The missing version file stops installation.
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
  b.clock(TAKEOVER + 5_000);
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
  const queriedName = await b.route("POST", "kuvo/computers/name-save", { save: name.value });
  assert.equal(queriedName.status, 409);
  assert.equal(queriedName.value.status, "interrupted");
  assert.equal(queriedName.value.operation, name.value.operation);
  for (const [kind, save] of receipts) {
    const outcome = await link.askSetupOutcome(b.run, { setup: save.setup, operation: save.operation, proof: save.proof });
    assert.equal(outcome.status, 200, kind);
    assert.equal(outcome.receipt.status, "interrupted", kind);
  }
});

test("failure bench: only the standby takes over across a split and the old primary rejoins", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, phone, pairing } = await standbyNetwork(t);
  const reconnect = [b, c].flatMap(box => [relay.pause(a.address, box.address), relay.pause(box.address, a.address)]);
  for (const box of [a, b, c]) box.clock(90_000);
  await delay(800);
  await see(b, 2, a);
  const blocked = await a.route("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(blocked.status, 202);
  assert.equal(blocked.value.pending, true);
  const interrupted = pendingOperation(a, "membershipAndAccess");
  assert.ok(interrupted);
  const work = await a.route("POST", "kuvo/work-queue/items", { operation: "work-survives-split", title: "Work survives the split" }, true);
  assert.equal(work.status, 200);
  const stale = a.read();

  for (const box of [a, b, c]) box.clock(TAKEOVER + 5_000);
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
  assert.equal((await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true)).status, 409);

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

test("failure bench: a two-computer standby takes over and the returning primary steps down", { timeout: 40_000 }, async t => {
  const { a, b, see } = await network(t, true, undefined, false, undefined, false, true);
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  await until("primary leased", () => a.lease(2, b), t.signal);
  await until("standby watching", () => b.observing(2), t.signal);
  await a.stop("SIGTERM");
  b.clock(TAKEOVER + 5_000);
  await see(b, 3, b);
  assert.equal(b.read().leadership[3] ? link.readDoc(b.read().leadership[3])?.origin : undefined, b.sign);
  a.start();
  await see(a, 3, b);
  assert.equal(a.read().setup.role, "member");
});

test("failure bench: signed membership carries every computer's successor rank", { timeout: 35_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  await Promise.all([see(b, 2, a), see(c, 2, a)]);
  const order = checkpoint(a, 2).shared.membershipAndAccess.value.successorOrder;
  assert.deepEqual(new Set(order), new Set([a.key, b.key, c.key]));
  assert.equal(order.length, 3);
  assert.ok(order.indexOf(b.key) < order.indexOf(c.key), "the configured always-on machine ranks ahead of the nonpreferred machine");
  for (const box of [a, b, c]) assert.deepEqual(box.read().shared.membershipAndAccess.value.successorOrder, order);
});

test("failure bench: losing both primary and standby promotes the next ranked computer", { timeout: 35_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  await see(c, 2, a);
  await watchingRanks(t, relay, a, [c]);
  assert.equal(c.read().setup.successorChain, true);
  await a.stop("SIGTERM");
  await b.stop("SIGTERM");
  c.clock(TAKEOVER + 5_000);
  await see(c, 3, c);
  const third = link.readDoc(c.read().leadership[3]);
  assert.equal(third.origin, c.sign);
  assert.equal(third.body.takeover, true);
});

test("failure bench: split successors converge to the better rank at the same epoch", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  await Promise.all([see(b, 2, a), see(c, 2, a)]);
  await watchingRanks(t, relay, a, [b, c]);
  const resumes = [relay.pause(b.address, c.address), relay.pause(c.address, b.address)];
  await a.stop("SIGTERM");
  b.clock(TAKEOVER + 5_000);
  c.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, c)]);
  assert.equal(b.read().setup.role, "primary");
  assert.equal(c.read().setup.role, "primary");
  resumes.forEach(resume => resume());
  await see(c, 3, b);
  assert.equal(b.read().setup.primary, b.key);
  assert.deepEqual(c.read().leadership[3], b.read().leadership[3]);
});

test("failure bench: a newer split epoch wins over an earlier, better-ranked successor", { timeout: 50_000 }, async t => {
  const { a, b, boxes, relay, see } = await network(t, true, undefined, false, undefined, false, false,
    ["Studio", "Desk", "Lab", "Office"]);
  await Promise.all(boxes.map(box => see(box, 2, a)));
  await until("primary leased", () => a.lease(2, b), t.signal);
  await watchingRanks(t, relay, a, boxes.slice(1));
  const order = checkpoint(a, 2).shared.membershipAndAccess.value.successorOrder;
  const [fallback, partner] = order.filter(key => key !== a.key && key !== b.key).map(key => boxes.find(box => box.key === key));
  const resumes = [fallback, partner].flatMap(box => [relay.pause(b.address, box.address), relay.pause(box.address, b.address)]);
  await a.stop("SIGTERM");
  for (const box of boxes.slice(1)) box.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 3, b), see(fallback, 4, fallback)]);
  resumes.forEach(resume => resume());
  await see(b, 4, fallback);
  assert.deepEqual(b.read().leadership[4], fallback.read().leadership[4]);
});

test("failure bench: a pending handover is superseded when its approver is lost across restart", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, phone, pairing } = await standbyNetwork(t);
  await see(c, 2, a);
  const release = relay.intercept(a.address, b.address, packet => {
    const mail = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), b.read())?.payload;
    return mail?.docs?.give?.some(envelope => link.readDoc(envelope)?.body.proposal === true);
  });
  const freezeClockAt = path.join(a.run, "handover-wait-clock");
  const handover = a.route("POST", "kuvo/setup/handover", { primary: c.key, epoch: 2 }, false, {}, { freezeClockAt });
  const pending = await until("signed pending handover", () => a.read().leadershipPending, t.signal);
  // The route has passed its lease checks. Only its reply clock stops during the test's restart.
  fs.writeFileSync(freezeClockAt, "");
  assert.equal(b.read().setup.epoch, 2);
  await b.stop("SIGTERM");
  release();
  await a.stop("SIGTERM");
  a.start();
  await Promise.all([see(a, 3, a), see(c, 3, a)]);
  assert.equal(link.readDoc(a.read().leadership[3]).body.supersedes, pending.s);
  assert.equal(a.read().setup.standby.key, c.key);
  assert.equal((await handover).status, 409);
  await until("replacement lease", () => a.lease(3, c), t.signal);
  const saved = await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true);
  assert.equal(saved.status, 200);
  await until("replacement acknowledges a save", () => c.read().removed?.some(device => device.key === phone), t.signal);
});

test("failure bench: a promoted primary cannot hand over before its standby grants a lease", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  const unblock = relay.intercept(c.address, b.address, packet => {
    const mail = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), b.read())?.payload;
    return mail?.setupAuthority?.kind === "available";
  });
  await a.stop("SIGTERM");
  b.clock(TAKEOVER + 5_000);
  c.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  assert.equal(b.read().setup.standby, undefined);
  assert.equal((await submit(b, c, 3)).status, 503);
  assert.equal(b.read().setup.epoch, 3);
  unblock();
  await see(b, 4, b);
  await until("primary leased", () => b.lease(4, c), t.signal);
  assert.equal((await submit(b, c, 4)).status, 202);
  await see(c, 5, c);
});

test("failure bench: choosing a first standby after takeover records the pin only after acceptance and lease", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  let acceptance;
  const release = relay.intercept(c.address, b.address, packet => {
    const payload = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), b.read())?.payload;
    if (payload?.docs?.give?.some(envelope => link.readDoc(envelope)?.body.standbyAcceptance && link.readDoc(envelope).body.approval)) {
      acceptance = packet;
      return true;
    }
    return payload?.setupAuthority?.kind === "available" || payload?.setupAuthority?.kind === "lease";
  });
  await a.stop("SIGTERM");
  b.clock(TAKEOVER + 5_000);
  c.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  const choosing = b.route("POST", "kuvo/setup/standby", { standby: c.key, epoch: 3 });
  await until("candidate signed the first assignment", () => acceptance, t.signal);
  assert.equal(b.read().setup.standby, undefined);
  assert.equal(c.read().shared.membershipAndAccess.value.successorPin, undefined);
  assert.equal(b.read().shared.membershipAndAccess.value.successorPin, undefined);
  release();
  relay.replay(acceptance);
  assert.equal((await choosing).status, 202);
  await until("first lease", () => b.lease(4, c), t.signal);
  await until("the standby acknowledged its pin", () => b.read().shared.membershipAndAccess.value.successorPin === c.key, t.signal);
  assert.equal(b.read().setup.epoch, 4);
  assert.equal(c.read().shared.membershipAndAccess.value.successorPin, c.key);
});

test("failure bench: a fenced primary replaces an unavailable standby before saving again", { timeout: 45_000 }, async t => {
  const { a, b, c, see, phone, pairing } = await standbyNetwork(t);
  await see(c, 2, a);
  const pin = await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 2 });
  assert.deepEqual(pin, { status: 200, value: { primary: a.key, epoch: 2 } });
  await b.stop("SIGTERM");
  a.clock(TAKEOVER + 5_000);
  c.clock(TAKEOVER + 5_000);
  await delay(1000);
  assert.equal((await a.route("POST", "kuvo/phones/pair")).status, 409);
  a.clock(2 * TAKEOVER);
  c.clock(2 * TAKEOVER);
  await Promise.all([see(a, 3, a), see(c, 3, a)]);
  assert.equal(a.read().setup.standby.key, c.key);
  assert.equal(a.read().shared.membershipAndAccess.value.successorPin, b.key);
  assert.equal(a.read().shared.membershipAndAccess.value.successorOrder[0], b.key);
  await until("new standby renews the lease", () => a.lease(3, c), t.signal);
  const remove = await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true);
  assert.equal(remove.status, 200);
  await until("replacement acknowledged the save", () => c.read().removed.some(device => device.key === phone && device.pairing === pairing), t.signal);
  b.clock(2 * TAKEOVER);
  b.start();
  await Promise.all([see(a, 4, a), see(b, 4, a), see(c, 4, a)]);
  assert.equal(a.read().setup.standby.key, b.key, "the returning first ranked computer resumes as standby");
  await until("returning standby renewed the lease", () => a.lease(4, b), t.signal);
});

test("failure bench: a device revoked on a losing branch stays revoked on both sides", { timeout: 70_000 }, async t => {
  const { a, b, boxes, relay, see, phone, pairing, phase } = await network(t, true, undefined, false, undefined, false, false,
    ["Studio", "Desk", "Lab", "Office", "Workshop"], { phaseDiagnostics: true });
  await Promise.all(boxes.map(box => see(box, 2, a)));
  await until("primary leased", () => a.lease(2, b), t.signal);
  await watchingRanks(t, relay, a, boxes.slice(1));
  phase("original primary leased and all successors observing epoch 2");
  const order = checkpoint(a, 2).shared.membershipAndAccess.value.successorOrder;
  const [fallback, partner, remaining] = order.filter(key => key !== a.key && key !== b.key).map(key => boxes.find(box => box.key === key));
  const resumes = [b, remaining].flatMap(left => [fallback, partner].flatMap(right => [
    relay.pause(left.address, right.address), relay.pause(right.address, left.address),
  ]));
  await a.stop("SIGTERM");
  for (const box of boxes.slice(1)) box.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 4, b), see(fallback, 4, fallback)]);
  await until("losing branch leased", () => fallback.lease(4, partner), t.signal);
  phase("both partitions elected and the losing branch leased");
  assert.ok(b.read().phones.some(device => device.key === phone));
  const branchName = "Only on the losing branch";
  const branchSave = await rename(fallback, partner, branchName);
  assert.equal(branchSave.status, 200, JSON.stringify(branchSave.value));
  assertReceipt(fallback, "setupNames", "committed", branchSave.value);
  assert.equal(partner.read().sharedOutcomes?.[branchSave.value.operation]?.status, "committed");
  assert.equal(await setupName(fallback, partner.key), branchName);
  assert.equal(await setupName(b, partner.key), partner.name);

  phase("losing branch saved the ordinary edit through its standby");
  const remove = await fallback.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true);
  assert.equal(remove.status, 200);
  await until("losing branch revocation accepted", () => fallback.read().removed.some(device => device.key === phone && device.pairing === pairing), t.signal);
  phase("losing branch committed its device revocation");
  resumes.forEach(resume => resume());
  await Promise.all([see(fallback, 4, b), see(partner, 4, b)]);
  assert.equal(await setupName(fallback, partner.key), partner.name);
  assert.equal(await setupName(b, partner.key), partner.name);
  const branchOutcome = await link.askSetupOutcome(b.run, branchSave.value, "setupNames");
  assert.equal(branchOutcome.status, 404);
  assert.equal(branchOutcome.receipt, undefined, "a supplied receipt is not a new attestation by the winner");
  phase("winning branch discarded the edit and refused its receipt");
  const kept = fallback.read().discardedRevocations;
  assert.deepEqual(kept.certificates.map(envelope => link.readDoc(envelope).n), [3, 4]);
  for (const box of boxes.slice(1)) {
    await until(`revocation on ${box.name}`, () => box.read().removed.some(device => device.key === phone && device.pairing === pairing), t.signal);
    assert.ok(!box.read().phones.some(device => device.key === phone));
  }
  phase("verified revocation converged on all survivors");
});

test("failure bench: only the old standby can approve a standby replacement and a handover", { timeout: 45_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  const selected = await a.route("POST", "kuvo/setup/standby", { standby: c.key, epoch: 2 });
  assert.deepEqual(selected, { status: 202, value: { primary: a.key, epoch: 3 } });
  await Promise.all([see(b, 3, a), see(c, 3, a)]);
  assert.equal(link.readDoc(c.read().leadership[3]).origin, b.sign);
  const ranked = c.read().shared.membershipAndAccess.value;
  assert.equal(ranked.successorPin, c.key);
  assert.equal(ranked.successorOrder[0], c.key);
  await until("replacement granted lease", () => a.lease(3, c), t.signal);
  assert.deepEqual(await submit(a, b, 3), { status: 202, value: { primary: b.key, epoch: 4 } });
  await Promise.all([see(b, 4, b), see(c, 4, b)]);
  assert.equal(link.readDoc(b.read().leadership[4]).origin, c.sign);
  assert.equal(b.read().setup.standby.key, c.key);
});

test("failure bench: the takeover deadline ignores a retained delay override", { timeout: 40_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  fs.writeFileSync(path.join(b.env.HOME, "setup-takeover.json"), JSON.stringify({ delayMinutes: 60, alwaysOn: true }));
  await a.stop("SIGKILL");
  b.clock(3_000);
  await delay(700);
  await see(b, 2, a);
  b.clock(TAKEOVER + 5_000);
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  assert.equal(a.read().setup.epoch, 2);
});

test("removing the final standby requires its durable retirement approval", { timeout: 45_000 }, async t => {
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true);
  await see(a, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const resume = relay.pause(b.address, a.address);
  try {
    const removing = a.route("DELETE", `kuvo/computers/${b.key}`, { operation: "final-standby-unlink-test" });
    await until("standby installed retirement", () => b.read().setup.epoch === 3, t.signal);
    assert.ok(a.read().leadershipPending);
    assert.equal(a.read().setup.standby.key, b.key);
    assert.equal((await a.route("POST", "kuvo/phones/pair")).status, 409);
    assert.equal((await removing).status, 503);
  } finally { resume(); }

  await see(a, 3, a);
  const retired = link.readDoc(a.read().leadership[3]);
  assert.equal(retired.origin, b.sign);
  assert.equal(retired.body.retiredStandby, b.key);
  assert.equal(a.read().setup.sharedSaves, undefined);
  const removed = await a.route("DELETE", `kuvo/computers/${b.key}`, { operation: "final-standby-unlink-test" });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assert.equal(removed.value.operation, "final-standby-unlink-test");
  await until("only the primary is left", () => a.read().left.includes(b.key), t.signal);
  assert.equal((await a.route("POST", "kuvo/phones/pair")).status, 200);
});

test("failure bench: takeover waits two minutes after the primary lease expires", { timeout: 30_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  await Promise.all([see(b, 2, a), see(c, 2, a)]);
  await until("standby watches", () => b.observing(2), t.signal);
  await a.stop();
  await until("primary offline", () => !relay.has(a.address), t.signal);
  b.clock(155_000);
  await delay(900);
  assert.equal(b.read().setup.epoch, 2);
  b.clock(185_000);
  await see(b, 3, b);
});

test("failure bench: unlink retries credential cleanup before acknowledging completion", { timeout: 45_000 }, async t => {
  const { a, b, c, see } = await standbyNetwork(t);
  await see(c, 2, a);
  const folder = path.join(c.home, "data", "opencode");
  const database = path.join(folder, "opencode.db");
  const next = path.join(folder, "opencode-next.db");
  const auth = path.join(folder, "auth.json");
  const agentAuth = path.join(c.home, "agents", "codex", "accounts", "test-account", "auth.json");
  const otherFolder = path.join(c.home, "engine-data-fixture");
  const other = path.join(otherFolder, "opencode-preview.db");
  c.env.KUVO_ENGINE_DB_FOLDER = otherFolder;
  const keys = path.join(c.home, "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  const bot = path.join(c.home, "bots", "bot_fixture", "bot.json");
  const native = path.join(c.env.HOME, ".codex", "auth.json");
  for (const file of [database, keys, bot, native, auth, agentAuth]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "fixture");
  }
  const operation = "restart-unlink-cleanup-test";
  const removed = await a.route("DELETE", `kuvo/computers/${c.key}`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  await until("unlink journal persisted", () => c.read().localUnlink?.operation === operation, t.signal);
  const record = () => a.read().shared.membershipAndAccess.value.removals[c.key];
  assert.equal(record().delivered, undefined);
  assert.equal(c.read().setup.id, a.read().setup.id);
  assert.ok(fs.existsSync(keys));

  await c.stop("SIGTERM");
  fs.unlinkSync(database);
  fs.mkdirSync(otherFolder);
  for (const file of [database, next, other]) {
    const db = new DatabaseSync(file);
    db.exec("CREATE TABLE credential (value TEXT); INSERT INTO credential VALUES ('fixture'); CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('session');");
    db.close();
  }
  const keychain = new Set(["KUVO_KEY_NOTION_TOKEN", "KUVO_KEY_NOTION_TOKEN__BOT_LEGACY"]);
  const hostPassword = crypto.randomBytes(24).toString("hex");
  const host = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const given = JSON.parse(Buffer.concat(chunks));
    if (request.url === "/hello") {
      const proof = crypto.createHmac("sha256", hostPassword)
        .update(`kuvo-host-hello\0${given.nonce}\0${request.headers.host}`).digest("hex");
      return response.end(JSON.stringify({ proof }));
    }
    if (request.headers.authorization !== `Bearer ${hostPassword}`) return response.writeHead(401).end();
    if (request.url === "/keys/names") return response.end(JSON.stringify({ names: [...keychain] }));
    if (request.url === "/keys/delete") {
      keychain.delete(given.name);
      return response.end("{}");
    }
    response.writeHead(404).end();
  });
  host.listen(0, "127.0.0.1");
  await once(host, "listening");
  t.after(() => { host.closeAllConnections(); return new Promise(resolve => host.close(resolve)); });
  c.env.KUVO_KEYS_ON_HOST = "1";
  fs.writeFileSync(path.join(c.run, "host.json"), JSON.stringify({ url: `http://127.0.0.1:${host.address().port}`, password: hostPassword }));
  c.start();
  await until("primary receives completed unlink", () => record().delivered === true, t.signal);
  await until("source receives the acknowledgement", () => !c.read().localUnlink, t.signal);
  assert.notEqual(c.read().setup.id, a.read().setup.id);
  for (const file of [database, next, other]) {
    const remaining = new DatabaseSync(file);
    assert.equal(remaining.prepare("SELECT COUNT(*) AS n FROM credential").get().n, 0, path.basename(file));
    assert.equal(remaining.prepare("SELECT id FROM session").get().id, "session");
    remaining.close();
  }
  for (const file of [auth, agentAuth]) assert.equal(fs.existsSync(file), false);
  assert.equal(keychain.size, 0);
  assert.equal(fs.existsSync(keys), false);
  for (const file of [bot, native]) assert.equal(fs.readFileSync(file, "utf8"), "fixture");
});

test("an unmatched pre-join pairing requires an explicit enrollment decision", { timeout: 20_000 }, async t => {
  const { a, c, phone, relay } = await network(t);
  await c.stop("SIGTERM");
  const file = path.join(c.run, "link.json");
  const local = { ...c.identity, phones: [{ key: phone, pairing: "independent-pairing", from: "pair" }] };
  fs.writeFileSync(file, JSON.stringify(local));
  // Keep this CLI fixture from launching a second, untracked bridge.
  fs.mkdirSync(path.join(c.run, ".updating"));
  fs.writeFileSync(path.join(c.run, ".updating", "pid"), String(process.pid));
  const token = "pairing-decision";
  fs.symlinkSync(token, path.join(path.dirname(c.home), ".kuvo-code-adding"));
  const signed = link.signDoc(a.seed, "phones", 1, { phones: [{ key: phone, pairing: "different-pairing" }], removed: [] });
  const refused = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, docs: [signed] }));
  assert.equal(refused.status, 5, refused.err);
  assert.match(refused.err, /Confirm discarding unmatched local device pairings/);
  assert.deepEqual(c.read(), local);

  const roster = link.signDoc(a.seed, "peers", 2, { setup: { id: a.read().setup.id, epoch: 1, primary: a.key },
    boxes: [a.entry, c.entry], moved: [], left: [] });
  const matching = link.signDoc(a.seed, "phones", 2, { phones: [{ key: phone, pairing: "independent-pairing" }], removed: [] });
  const joined = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, relay: relay.url, docs: [roster, matching] }));
  assert.equal(joined.status, 0, joined.err);
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.pairing, "independent-pairing");
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.from, "mac");
});

test("failure bench: standby retirement keeps the live standby until a replacement is ready", { timeout: 45_000 }, async t => {
  const { a, b, c, see, relay } = await standbyNetwork(t);
  assert.equal((await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 2 })).status, 200);
  assert.equal(a.read().shared.membershipAndAccess.value.successorPin, b.key);
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
  assert.equal(a.read().shared.membershipAndAccess.value.successorPin, undefined);
  assert.equal(c.read().shared.membershipAndAccess.value.successorPin, undefined);
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
  assert.deepEqual(done.value, { result: "refused", message: "Choose another main computer first." });
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
  const remove = (box, asking) => box.route("POST", "kuvo/phones/remove", { phones: [{ key: asking.key }] }, true);
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

  for (const status of ["pending", "committed"]) {
    const local = device(`Local ${status}`);
    const member = device(`Member ${status}`);
    await access(status, () => pair(a, local));
    const vouched = await access(status, () => pair(c, member));
    if (status === "committed") assert.equal(vouched.paired, true);
    await access(status, () => remove(a, local));
    await access(status, () => remove(c, member));
  }
  await access("committed", () => a.route("DELETE", `kuvo/computers/${c.key}`));
});

test("failure bench: forwarded computer changes carry receipts through the member route", { timeout: 105_000 }, async t => {
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
      const named = await rename(b, c, `Forwarded ${status}`);
      assert.equal(named.status, status === "pending" ? 202 : 200, JSON.stringify(named));
      assertReceipt(a, "setupNames", status, named.value);
      const poll = save => b.route("POST", "kuvo/computers/name-save", { save });
      const checked = await poll({ ...named.value, epoch: named.value.epoch + 10 });
      assert.equal(checked.status, status === "pending" ? 202 : 200, JSON.stringify(checked));
      assert.equal(checked.value.status, status);
      assert.equal(checked.value.epoch, named.value.epoch);
      const forged = { ...named.value, proof: { ...named.value.proof, s: "forged" } };
      assert.equal((await poll(forged)).status, 404);

      const removed = await b.route("DELETE", `kuvo/computers/${c.key}`);
      assert.equal(removed.status, status === "pending" ? 202 : 200, JSON.stringify(removed));
      assertReceipt(a, "membershipAndAccess", status, removed.value);
      release();
      if (status === "pending") {
        await b.stop("SIGTERM");
        b.start();
        await until("the listing box restarts", () => relay.has(b.address), t.signal);
      }
      const final = await until("the lister recovers the rename outcome", async () => {
        const reply = await poll(named.value);
        return reply.status === 200 && reply;
      }, t.signal);
      assert.equal(final.value.operation, named.value.operation);
      assert.equal(final.value.status, "committed");
    });
  }
});

// Connect as an ordinary relay device and send normal client routes to a running box.
async function deviceAt(relay, box, key, hello = {}) {
  const client = relay.device();
  const session = crypto.randomInt(1, 2 ** 30);
  const initiator = new link.IKInitiator(key, Buffer.from(box.key, "base64url"), link.PROLOGUE);
  client.send(box.address, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, ...hello })))));
  const frame = link.readFrame(link.readRelay(await client.next(12_000)).payload);
  if (frame.kind !== link.Frame.handshake2) {
    client.leave();
    return { refused: true };
  }
  const { payload, transport } = initiator.readMessage2(frame.body);
  const greeting = JSON.parse(payload);
  if (greeting.waiting) {
    client.leave();
    return { waiting: greeting.waiting };
  }
  let id = 0;
  return {
    greeting,
    close: () => client.leave(),
    async ask(method, path, body, headers = {}) {
      const h = { ...(body === undefined ? {} : { "content-type": "application/json" }), ...headers };
      const data = body === undefined ? Buffer.alloc(0) : Buffer.from(JSON.stringify(body));
      for (const part of link.pieces({ t: "req", id: ++id, m: method, p: path, h }, data, true)) {
        client.send(box.address, link.frame(link.Frame.transport, session, transport.seal(part)));
      }
      const chunks = [];
      let status;
      for (;;) {
        const next = link.readFrame(link.readRelay(await client.next(12_000)).payload);
        if (next.kind !== link.Frame.transport) return { closed: true };
        const answer = link.decodeMessage(transport.open(next.body));
        chunks.push(answer.body);
        status ??= answer.head.s;
        if (answer.head.end === true) return { status, value: JSON.parse(Buffer.concat(chunks).toString()) };
      }
    },
  };
}

test("failure bench: clients share machine changes and revocations across three boxes", { timeout: 90_000 }, async t => {
  const { a, b, c, relay, see } = await standbyNetwork(t);
  await see(c, 2, a);
  const open = async (box, key, hello) => {
    const device = await deviceAt(relay, box, key, hello);
    if (!device.greeting) return device;
    t.after(device.close);
    return device;
  };
  async function byClient(label, read) {
    while (!t.signal.aborted) {
      const value = await read();
      if (value) return value;
      await delay(25);
    }
    throw new Error(`Timed out waiting for ${label}`);
  }
  const pairedAt = (box, key, hello) => byClient(`the paired device reaches ${box.name}`, async () => {
    const device = await open(box, key, hello);
    return device.greeting && device;
  });
  const phoneKey = link.newKey();
  const phonePublic = link.publicOf(phoneKey).toString("base64url");
  const invite = await a.route("POST", "kuvo/phones/pair");
  assert.equal(invite.status, 200);
  const phoneA = await open(a, phoneKey, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Phone" });
  assert.ok(phoneA.greeting);
  const phoneB = await pairedAt(b, phoneKey);
  const phoneC = await pairedAt(c, phoneKey);

  const launchKey = link.newKey();
  const desktop = { key: link.publicOf(launchKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const save = await b.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(save.status, 200, JSON.stringify(save));
  assertReceipt(a, "membershipAndAccess", "committed", save.value);
  const deskA = await pairedAt(a, launchKey, { client: "desktop" });
  const deskB = await pairedAt(b, launchKey, { client: "desktop" });
  const deskC = await pairedAt(c, launchKey, { client: "desktop" });
  for (const device of [deskA, deskB, deskC, phoneA, phoneB, phoneC]) {
    const roster = await device.ask("GET", "kuvo/computers");
    assert.equal(roster.status, 200);
    assert.deepEqual(new Set(roster.value.computers.map(peer => peer.key)), new Set([a.key, b.key, c.key]));
  }

  const renamed = await phoneB.ask("PATCH", `kuvo/computers/${c.key}`, { name: "Shared name" });
  assert.equal(renamed.status, 200, JSON.stringify(renamed));
  assertReceipt(a, "setupNames", "committed", renamed.value);
  for (const device of [deskA, deskB, deskC]) {
    await byClient("the desktop reads the shared name", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.find(peer => peer.key === c.key)?.name === "Shared name");
  }

  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await until("the third computer goes offline", () => !relay.has(c.address), t.signal);
  const revoked = await phoneB.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: save.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  assertReceipt(a, "membershipAndAccess", "committed", revoked.value);
  assert.ok(c.read().phones.some(device => device.key === desktop.key));
  const unpause = relay.pause(a.address, c.address);
  t.after(unpause);
  reconnect();
  await until("the third computer reconnects with cached grants", () => relay.has(c.address), t.signal);
  assert.equal((await open(c, launchKey, { client: "desktop" })).waiting, "Checking this app with your other computers…");
  unpause();
  await until("the third computer installs the tombstone", () => c.read().removed.some(entry => entry.key === desktop.key), t.signal);
  for (const box of [a, b, c]) {
    assert.deepEqual(await open(box, launchKey, { client: "desktop" }), { refused: true });
    assert.ok(box.read().phones.some(device => device.key === phonePublic));
  }

  const removed = await phoneB.ask("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(removed.status, 200, JSON.stringify(removed));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  for (const device of [phoneA, phoneB]) {
    await byClient("the removed machine is absent from the roster", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.every(peer => peer.key !== c.key));
    assert.equal((await device.ask("POST", `kuvo/computers/${c.key}/connect`)).status, 404);
  }
  await until("the removed computer received its unlink", () => !c.read().phones.some(device => device.key === phonePublic), t.signal);
  assert.deepEqual(await open(c, phoneKey), { refused: true });
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
  b.clock(TAKEOVER + 5_000);
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
  const desktopSecret = link.newKey();
  const desktop = link.publicOf(desktopSecret).toString("base64url");
  const identityText = `${desktopSecret.toString("base64url")}.${crypto.randomBytes(32).toString("base64url")}`;
  fs.writeFileSync(identity, identityText);
  assert.equal((await c.route("POST", "kuvo/devices/pair", { key: desktop, name: "Desktop", kind: "desktop" }, true)).status, 200);
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
  assert.equal(fs.readFileSync(identity, "utf8"), identityText);
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

test("box labels restore recovers 344 items' original labels through a two-computer setup", { timeout: 100_000 }, async t => {
  const definitions = [
    ["cat_architecture", "Architecture", "coral"],
    ["cat_security-privacy", "Security & privacy", "orange"],
    ["cat_tests-tooling", "Tests & tooling", "mint"],
    ["cat_features", "Features", "indigo"],
    ["cat_edge-cases", "Edge cases", "coral"],
    ["cat_correctness-sweep", "Correctness sweep", "mint"],
    ["lbl_parked", "Parked", "graphite"],
    ["lbl_my-feedbax", "My Feedbax", "amber"],
    ["lbl_gpui-migration-leftovers", "GPUI Migration Leftovers", "lime"],
    ["lbl_my-feedbacks", "My Feedbacks", "coral"],
  ].map(([id, name, color]) => ({ id, name, color }));
  const owner = "ses_recovered";
  const items = Array.from({ length: 344 }, (_, i) => ({
    id: `wqi_${String(i).padStart(20, "0")}`, title: `Item ${i + 1}`, number: i + 1, status: "todo", order: i + 1,
    labels: [definitions[i % definitions.length].id],
  }));
  const queueOf = box => path.join(box.run, "work-queue", "queues", owner + ".json");
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, {
    macPrimary: true,
    prepare([primary]) {
      const queue = queueOf(primary);
      fs.mkdirSync(path.dirname(queue), { recursive: true });
      fs.writeFileSync(queue, JSON.stringify({ owner, items }));
    },
  });
  const version = "0.10.0", sha256 = "e".repeat(64);
  const macRequest = async (method, route, body, clientVersion = version) => {
    const { port, password } = JSON.parse(fs.readFileSync(path.join(a.run, "serve.json"), "utf8"));
    const response = await fetch(`http://127.0.0.1:${port}/${route}`, {
      method,
      headers: { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}`, "x-kuvo-from": "app", "x-kuvo-client-version": clientVersion },
      ...(body ? { body: JSON.stringify(body) } : {}),
      signal: t.signal,
    });
    return { status: response.status, value: await response.json() };
  };
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify({ clientProtocol: 1 })).toString("base64") })).toString("base64");
  for (const box of [a, b]) {
    fs.mkdirSync(path.join(box.home, "bin"), { recursive: true });
    fs.mkdirSync(path.join(box.home, "updates"), { recursive: true });
    fs.writeFileSync(path.join(box.home, "bin", "box-version"), version);
    fs.writeFileSync(path.join(box.home, "updates", "installed.json"), JSON.stringify({ version, sha256 }));
    const state = box.read();
    state.shared.setupSecurityAndReleaseChoice = { sequence: 1, value: { version, sha256, signed } };
    const file = path.join(box.run, "link.json");
    fs.writeFileSync(file + ".fleet", JSON.stringify(state));
    fs.renameSync(file + ".fleet", file);
  }
  const oldDesktop = await macRequest("GET", "kuvo/info", undefined, "0.8.0-beta.35");
  assert.equal(oldDesktop.status, 426, JSON.stringify(oldDesktop.value));
  assert.equal(oldDesktop.value.reason, "client-update-required");
  assert.equal(oldDesktop.value.target, version);
  const emptyLabels = { labels: [], removed: [] };
  const seedRevision = crypto.createHash("sha256").update(JSON.stringify(emptyLabels)).digest("base64url");
  const empty = { sequence: 1, value: emptyLabels, offlineEdits: {
    versions: [], choices: [], receipts: [], additions: {
      "setup-seed:": "1", [`setup-seed:${a.key}`]: seedRevision, [`setup-seed:${b.key}`]: seedRevision,
      [`setup-seed:fields:${seedRevision}`]: "{}",
    },
  } };
  const shared = box => box.read().shared.queueLabels;
  for (const field of ["key", "secret", "sign"]) assert.equal(a.read()[field], undefined);
  assert.deepEqual(shared(a), empty);
  assert.deepEqual(shared(b), empty);
  const library = { labels: definitions, removed: [] };
  const file = path.join(a.env.TMPDIR, "saved-labels.json");
  const restore = (box, ...flags) => box.evaluate(process.execPath, [script, "box", "labels", "restore", file, ...flags], "", false);
  const run = async (box, ...flags) => {
    const answer = await restore(box, ...flags);
    assert.equal(answer.status, 0, answer.err);
    return answer.out;
  };
  fs.writeFileSync(file, JSON.stringify(library));
  const versionFile = path.join(a.home, "bin", "box-version");
  try {
    fs.writeFileSync(versionFile, "0.8.0-beta.35");
    const outdated = await restore(a);
    assert.equal(outdated.status, 1);
    assert.equal(outdated.out, "");
    assert.match(outdated.err, /This computer needs to update to 0\.10\.0 before it can be used\./);
    assert.deepEqual(shared(a), empty);
  } finally {
    fs.writeFileSync(versionFile, version);
  }

  const queue = queueOf(a);
  const readView = async () => {
    const view = await macRequest("GET", `kuvo/work-queue?owner=${owner}&glance=1`);
    assert.equal(view.status, 200, JSON.stringify(view.value));
    assert.equal(view.value.items.length, items.length);
    return view.value;
  };
  const before = await readView();
  assert.deepEqual(before.labels, []);
  assert.ok(before.items.every(item => item.labels.length === 0));
  const unchanged = fs.readFileSync(queue, "utf8");
  await until("initial queue numbering", () => [a, b].every(box => {
    try { return JSON.parse(fs.readFileSync(path.join(box.run, "work-queue", "numbers.json"), "utf8")).since; }
    catch { return false; }
  }), t.signal);

  for (const box of [a, b]) {
    const response = box === a ? await macRequest("POST", "kuvo/work-queue/labels/restore", { library })
      : await b.route("POST", "kuvo/work-queue/labels/restore", { library }, true, { "x-kuvo-client-version": version });
    assert.equal(response.status, 403);
  }
  const snapshot = box => Object.fromEntries(fs.readdirSync(box.home, { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile()).flatMap(entry => {
      const file = path.join(entry.parentPath, entry.name);
      try { return [[path.relative(box.home, file), fs.readFileSync(file)]]; }
      catch (error) {
        if (error.code === "ENOENT") return [];
        throw error;
      }
    }));
  // Keep periodic replication out of the snapshot window. Let the preview and the previous
  // forwarding envelopes through so the old implementation would leave its durable receipt.
  let request;
  let reply;
  const allowRead = to => packet => {
    const wire = link.readFrame(packet.payload);
    const message = wire && link.openMail(wire.body, { ...to.read(), ...to.identity })?.payload;
    const queue = message?.workQueue;
    if (queue?.act?.item === "kuvo/work-queue/labels/restore") request = queue;
    if (queue?.acted?.id && queue.acted.id === request?.act.id) {
      reply = { queue, packet };
      return true;
    }
    return !queue?.act && !queue?.acted;
  };
  const releaseA = relay.intercept(b.address, a.address, allowRead(a));
  const releaseB = relay.intercept(a.address, b.address, allowRead(b));
  try {
    await delay(500);
    fs.writeFileSync(path.join(a.run, "peer-reach-checked.json"), JSON.stringify({ [b.key]: Date.now() }));
    fs.writeFileSync(path.join(b.run, "peer-reach-checked.json"), JSON.stringify({ [a.key]: Date.now() }));
    const snapshots = [snapshot(a), snapshot(b)];
    assert.equal(await run(a, "--dry-run"), "Dry run: would restore 10, already present 0, conflicts 0.");
    assert.deepEqual(snapshot(a), snapshots[0], "the primary's files are byte-identical after its local dry run");
    const preview = run(b, "--dry-run");
    preview.catch(() => {});
    await until("the forwarded preview reply", () => reply, t.signal);
    assert.equal(request.act.method, "GET");
    assert.equal(request.act.area, "ownership");
    assert.equal(request.memory, true);
    assert.equal(reply.queue.memory, true);
    assert.deepEqual(snapshot(a), snapshots[0], "the primary has no in-flight preview files");
    assert.deepEqual(snapshot(b), snapshots[1], "the member has no in-flight preview files");
    relay.replay(reply.packet);
    assert.equal(await preview, "Dry run: would restore 10, already present 0, conflicts 0.");
    assert.deepEqual(snapshot(a), snapshots[0], "the primary's files are byte-identical after the forwarded dry run");
    assert.deepEqual(snapshot(b), snapshots[1], "the member's files are byte-identical after the forwarded dry run");
  } finally {
    releaseA();
    releaseB();
  }
  assert.deepEqual(shared(a), empty);
  assert.deepEqual(shared(b), empty);
  assert.equal(pendingOperation(a, "queueLabels"), undefined);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(b), "Restored 10, already present 0, conflicts 0.");
  assert.deepEqual(shared(a), { ...empty, sequence: 2, value: library });
  assert.deepEqual(shared(b), shared(a));
  const after = await readView();
  assert.deepEqual(after.labels, definitions);
  const byId = new Map(after.items.map(item => [item.id, item.labels]));
  for (const item of items) assert.deepEqual(byId.get(item.id), item.labels);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(a), "Restored 0, already present 10, conflicts 0.");
  assert.deepEqual(shared(a), { ...empty, sequence: 2, value: library });
  assert.deepEqual(shared(b), shared(a));
  assert.equal(pendingOperation(a, "queueLabels"), undefined);

  const [removed, kept] = definitions;
  const deletion = await macRequest("POST", "kuvo/work-queue/labels", { remove: removed.id });
  assert.equal(deletion.status, 200, JSON.stringify(deletion.value));
  assert.deepEqual(shared(a), { ...empty, sequence: 3, value: { labels: definitions.slice(1), removed: [removed.id] } });
  // A forced restore may clear the tombstone, but any existing definition still wins.
  fs.writeFileSync(file, JSON.stringify({ labels: [removed, { ...kept, name: "Different name", color: "pink" }], removed: [kept.id] }));
  assert.equal(await run(a),
    `Restored 0, already present 0, conflicts 2.\n${removed.id}: This label was removed. Use --force to restore it.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, 3);
  assert.deepEqual(shared(b), shared(a));
  assert.equal(await run(a, "--force", "--dry-run"),
    `Dry run: would restore 1, already present 0, conflicts 1.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, 3);
  assert.deepEqual(shared(b), shared(a));

  const resume = relay.pause(a.address, b.address);
  try {
    const pending = await restore(a, "--force");
    assert.equal(pending.status, 1);
    assert.equal(pending.out, "");
    assert.match(pending.err, /Pending until the standby is back/);
    assert.equal(shared(a).sequence, 3);
    assert.deepEqual(shared(b), shared(a));
    assert.ok(pendingOperation(a, "queueLabels"));
  } finally { resume(); }
  await until("the forced restore was acknowledged", () => shared(a).sequence === 4, t.signal);
  assert.deepEqual(shared(a), { ...empty, sequence: 4, value: { labels: [...definitions.slice(1), removed], removed: [] } });
  assert.deepEqual(shared(b), shared(a));
});

test("box labels restore dry run asks for an update before sending to an old primary", { timeout: 100_000 }, async t => {
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, { macPrimary: true, oldPrimary: true });
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  const featureList = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[a.key].features;
  assert.ok(featureList.includes("work-queue.labels.restore"));
  assert.ok(!featureList.includes("work-queue.labels.restore.preview"));
  assert.ok(!featureList.includes("work-queue.forward.memory"));
  const file = path.join(b.env.TMPDIR, "preview-labels.json");
  fs.writeFileSync(file, JSON.stringify([{ id: "cat_tests", name: "Tests", color: "mint" }]));
  let sent = 0;
  const stop = relay.intercept(b.address, a.address, packet => {
    const wire = link.readFrame(packet.payload);
    const payload = wire && link.openMail(wire.body, { ...a.read(), ...a.identity })?.payload;
    if (payload?.workQueue?.act?.item === "kuvo/work-queue/labels/restore") ++sent;
    return false;
  });
  try {
    const reply = await b.evaluate(process.execPath, [script, "box", "labels", "restore", file, "--dry-run"], "", false);
    assert.equal(reply.status, 1);
    assert.equal(reply.out, "");
    assert.equal(reply.err.trim(), "Update the box on the main computer before previewing a restore from another computer.");
    assert.equal(sent, 0);
  } finally { stop(); }
});

test("box labels restore dry run returns promptly when the relay can't reach the primary", { timeout: 100_000 }, async t => {
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, { macPrimary: true });
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  const file = path.join(b.env.TMPDIR, "offline-preview.json");
  fs.writeFileSync(file, JSON.stringify([{ id: "cat_tests", name: "Tests", color: "mint" }]));
  const reach = () => ["peer-reach.json", "peer-reach-checked.json", "phone-trust.json"].map(name => {
    const file = path.join(b.run, name);
    return fs.existsSync(file) ? fs.readFileSync(file) : null;
  });
  let sent = false;
  const stop = relay.intercept(b.address, a.address, packet => {
    const wire = link.readFrame(packet.payload);
    const payload = wire && link.openMail(wire.body, { ...a.read(), ...a.identity })?.payload;
    if (payload?.workQueue?.memory && payload.workQueue.act?.item === "kuvo/work-queue/labels/restore") {
      sent = true;
      return false;
    }
    // Only the preview should generate an unreachable delivery during this check.
    return true;
  });
  const reconnect = relay.disconnect(a.address);
  try {
    await until("the main computer to go offline", () => !relay.has(a.address), t.signal);
    const before = reach();
    const started = performance.now();
    const result = await b.evaluate(process.execPath, [script, "box", "labels", "restore", file, "--dry-run"], "", false);
    assert.equal(result.status, 1);
    assert.equal(result.out, "");
    assert.equal(result.err.trim(), `${a.name} isn't reachable. Try again once it's back.`);
    assert.ok(sent, "the preview went through the relay");
    assert.ok(performance.now() - started < 10_000, "the CLI returned before the 20-second read timeout");
    assert.deepEqual(reach(), before, "the failed delivery leaves reach and phone trust files untouched");
  } finally {
    stop();
    reconnect();
  }
});

test("failure bench: first standby assignment waits for the candidate to install the entire personality library", { timeout: 55_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  const document = { personalities: [personality(1), personality(2)], removed: [{ id: "custom.retired", removed: 5 }] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.value.status, 200);
  // Lose the original acceptance on both paths so it cannot reach A via C.
  const resumeThird = relay.pause(b.address, c.address);
  t.after(resumeThird);
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
  const original = a.read().leadershipPending;
  assert.equal(link.readDoc(original).body.successorChain, true);
  assert.equal(a.read().setup.successorChain, undefined);
  await b.stop("SIGTERM");
  release();
  await a.stop("SIGTERM");
  await selecting;
  a.clock(0);
  c.clock(0);
  a.start();
  await Promise.all([see(a, 2, a), see(c, 2, a)]);
  assert.equal(a.read().setup.standby.key, c.key);
  assert.equal(link.readDoc(a.read().leadership[2]).body.supersedes, original.s);
  assert.deepEqual(c.read().shared.setupPersonalities.value, b.read().shared.setupPersonalities.value);
  assert.equal(a.read().shared.membershipAndAccess.value.successorPin, undefined);
  await until("replacement grants lease", () => a.lease(2, c), t.signal);
  const winner = a.read().leadership[2];
  assert.notDeepEqual(b.read().leadership[2], winner);
  resumeThird();
  b.clock(0);
  b.start();
  await until("superseded standby joins the winning chain", () =>
    b.read().setup?.role === "member" && b.read().setup?.standby?.key === c.key, t.signal);
  for (const box of [a, b, c]) {
    assert.equal(box.read().setup.epoch, 2);
    assert.equal(box.read().setup.primary, a.key);
    assert.deepEqual(box.read().leadership[2], winner);
  }
  assert.ok(a.lease(2, c));
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
  b.clock(TAKEOVER + 5_000);
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
  const item = await a.route("POST", "kuvo/work-queue/items", { operation: "everyday-work", title: "Everyday work" }, true);
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
    const answer = await box.route("POST", "kuvo/work-queue/items", { title, operation: crypto.randomUUID() }, true);
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
  b.clock(TAKEOVER + 5_000);
  await see(b, 3, b);
  assert.deepEqual(checkpoint(b, 3).shared.queueNumberAllocation.value.given, ledger.given);
  const next = await make(b, "Must wait for a new block");
  assert.equal(next.number, undefined);

  reconnect.forEach(resume => resume());
  c.clock(TAKEOVER + 5_000);
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

test("paired devices share ordinary edits and queued grants survive restart and revocation safely", { timeout: 120_000 }, async t => {
  const { a, b, c, relay, prompts, see, showOwnedConversation } = await network(t, true, undefined, true);
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const phoneKey = link.newKey();
  const desktopKey = link.newKey();
  const invite = await a.route("POST", "kuvo/phones/pair");
  const phoneA = await deviceAt(relay, a, phoneKey, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Phone" });
  assert.ok(phoneA.greeting);
  t.after(phoneA.close);
  const desktop = { key: link.publicOf(desktopKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const enrolled = await a.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled));
  async function open(box, key, hello) {
    return until("fresh device session", async () => {
      const connection = await deviceAt(relay, box, key, hello);
      if (!connection.greeting) return false;
      t.after(connection.close);
      return connection;
    }, t.signal);
  }
  const phone = await open(b, phoneKey);
  const desk = await open(b, desktopKey, { client: "desktop" });
  const configuration = path.join(b.home, "backup", "configuration.json");
  fs.mkdirSync(path.dirname(configuration), { recursive: true });
  fs.writeFileSync(configuration, JSON.stringify({ version: 1, enabled: true, source: "private-source", key: "private-key",
    dest: { kind: "disk", path: "/private/path" }, projects: ["/private/project"], last: { id: "private-id", at: 1234, version: 1 },
    error: "private error" }));
  for (const device of [phone, desk]) {
    assert.deepEqual(await device.ask("GET", "kuvo/box/backups/status"), { status: 200, value: {
      enabled: true, destinationKind: "disk", destinationAvailable: false, lastVerifiedAt: 1234, attention: true,
    } });
    assert.equal((await device.ask("GET", "kuvo/box/backups")).status, 403);
    assert.equal((await device.ask("POST", "kuvo/box/backups/destination", { enabled: false })).status, 403);
    assert.equal((await device.ask("POST", "kuvo/setup/save-local", { kind: "setupSecurityAndReleaseChoice", value: {} })).status, 403);

    const ref = editAddress(c, itemID());
    const draft = { operation: ref.id, baseRevision: null, id: ref.id, owner: ref.queue, computer: c.key,
      fields: { title: "Paired edit" } };
    assert.equal((await device.ask("GET", editPath(ref))).status, 200);
    const saved = await device.ask("POST", "kuvo/work-queue/items", draft);
    assert.equal(saved.status, 202, JSON.stringify(saved));
    assert.equal((await acceptedEdit(b, saved.value.operation, t)).receipt.status, "committed");
    assert.deepEqual(await device.ask("POST", "kuvo/work-queue/items", draft),
      { status: 200, value: (await acceptedEdit(b, saved.value.operation, t)).receipt });
    assert.equal((await editRead(c, ref)).value.fields.title, "Paired edit");
    const path = `kuvo/work-queue/items/${ref.id}`;
    const headers = { "x-kuvo-operation": "scoped-" + ref.id };
    const direct = await device.ask("PATCH", path, { title: "Forwarded edit" }, headers);
    assert.equal(direct.status, 200, JSON.stringify(direct));
    const replay = await device.ask("PATCH", path, { title: "Forwarded edit" }, headers);
    assert.equal(replay.status, 200, JSON.stringify(replay));
    assert.equal(replay.value.item.title, "Forwarded edit");
    assert.equal((await device.ask("PATCH", path, { title: "Different edit" }, headers)).status, 409);
    assert.equal((await editRead(c, ref)).value.fields.title, "Forwarded edit");
  }

  const lost = relay.intercept(c.address, b.address, packet => {
    const payload = link.openMail(link.readFrame(packet.payload)?.body, b.read())?.payload?.workQueue;
    return Boolean(payload?.offlineEditReceipt || payload?.ownerMessageReceipt);
  });
  t.after(lost);
  const alreadyRef = editAddress(c, itemID());
  const submit = async (ref, title) => desk.ask("POST", "kuvo/data/edits", {
    ref, operation: ref.id, baseRevision: null, create: true, fields: { title }, submitter: { kind: "computer" },
  });
  const send = (id, files) => desk.ask("POST", `kuvo/data/messages?session=ses_owned&computer=${c.key}`, { id, words: id, files });
  const alreadyEdit = await submit(alreadyRef, "Accepted with a lost reply");
  const alreadySent = await send("msg_beforerevocation");
  assert.equal(alreadyEdit.status, 202, JSON.stringify(alreadyEdit));
  assert.equal(alreadySent.status, 202, JSON.stringify(alreadySent));
  await until("owner accepted both operations before revocation", async () =>
    prompts.length === 1 && (await editRead(c, alreadyRef)).value.fields.title === "Accepted with a lost reply", t.signal);

  // Capture a pre-revocation envelope and deliver it after reconnect, before the owner has access updates.
  const held = [];
  const releaseWork = relay.intercept(b.address, c.address, packet => {
    const mail = link.openMail(link.readFrame(packet.payload)?.body, c.read())?.payload;
    const payload = mail?.workQueue;
    if (!payload?.scopedOfflineEdit && !payload?.scopedOwnerMessage && !mail?.scopedBotEdit) return false;
    held.push({ packet, payload, bot: mail.scopedBotEdit });
    return true;
  });
  t.after(releaseWork);
  const deniedRef = editAddress(c, itemID());
  const deniedEdit = await submit(deniedRef, "Keep this draft");
  const deniedSend = await send("msg_revokeddevice", inlineFiles({ "revoked.txt": Buffer.from("do not keep") }));
  const bot = "bot_Protected";
  const botFolder = path.join(c.home, "bots", bot);
  fs.mkdirSync(botFolder, { recursive: true });
  fs.writeFileSync(path.join(botFolder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Protected" }));
  fs.writeFileSync(path.join(botFolder, "session"), "ses_owned\n");
  const heldDelete = await desk.ask("DELETE", `kuvo/bots/${bot}?on=${c.key}`);
  assert.equal(heldDelete.status, 202, JSON.stringify(heldDelete));
  assert.equal((await phone.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "Different grant" })).status, 409);
  assert.equal(deniedEdit.status, 202);
  assert.equal(deniedSend.status, 202);
  const beforeRevocation = await until("new work in flight", () => {
    const edit = held.find(({ payload }) => payload?.scopedOfflineEdit?.edit.operation === deniedEdit.value.operation);
    // Messages preserve per-conversation order. The first accepted message still lacks its receipt.
    return edit && edit.packet;
  }, t.signal);
  assert.deepEqual(held.find(({ packet }) => packet === beforeRevocation).payload.scopedOfflineEdit.submitter,
    { kind: "paired", key: desktop.key, pairing: enrolled.value.pairing });

  const oldRef = editAddress(c, itemID());
  const old = await submit(oldRef, "Recover the old draft");
  assert.equal(old.status, 202);
  // Previous layout used an unattributed envelope. Preserve its payload and original digest.
  const oldFile = path.join(b.run, "owned-data", "edit-outbox", old.value.operation + ".json");
  const oldRecord = JSON.parse(fs.readFileSync(oldFile, "utf8"));
  const { queuedEdit, submitter, ...record } = oldRecord;
  const { submitter: _unattributed, ...envelope } = queuedEdit;
  fs.writeFileSync(oldFile, JSON.stringify({ ...record, envelope }));

  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const revoked = await phoneA.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: enrolled.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  assert.ok(c.read().phones.some(each => each.key === desktop.key));
  await b.stop("SIGTERM");
  b.start();
  const keepStale = relay.pause(a.address, c.address);
  t.after(keepStale);
  c.start();
  reconnect();
  await until("owner is back with cached grants", () => relay.has(c.address), t.signal);
  relay.replay(beforeRevocation);
  assert.equal((await deviceAt(relay, c, desktopKey, { client: "desktop" })).waiting, "Checking this app with your other computers…");
  assert.deepEqual((await editRead(c, deniedRef)).value.fields, {});
  assert.deepEqual(prompts.map(e => e.id), ["msg_beforerevocation"]);
  keepStale();
  releaseWork();
  showOwnedConversation(false);
  lost();

  const messageReceipt = operation => b.route("GET", `kuvo/data/messages/${operation}`);
  for (const [sent, status] of [[alreadySent, "committed"], [deniedSend, "interrupted"]]) {
    const checked = await until(`message is ${status}`, async () => {
      const reply = await messageReceipt(sent.value.operation);
      return reply.value.status !== "pending" && reply;
    }, t.signal);
    assert.equal(checked.value.status, status, JSON.stringify(checked));
    if (status === "committed") showOwnedConversation(true);
  }
  assert.equal((await acceptedEdit(b, alreadyEdit.value.operation, t)).receipt.status, "committed");
  const denied = await acceptedEdit(b, deniedEdit.value.operation, t);
  assert.equal(denied.receipt.status, "interrupted");
  assert.match(denied.receipt.message, /submitting device no longer has access/);
  assert.deepEqual(denied.recovery, { title: "Keep this draft" });
  const legacy = await acceptedEdit(b, old.value.operation, t);
  assert.equal(legacy.receipt.status, "interrupted");
  assert.match(legacy.receipt.message, /predates device tracking/);
  assert.deepEqual(legacy.recovery, { title: "Recover the old draft" });
  assert.deepEqual((await editRead(c, deniedRef)).value.fields, {});
  await until("held bot receipt preserves the revoked action", async () => {
    const listed = await b.route("GET", "kuvo/bots");
    return listed.value.held?.find(entry => entry.bot === bot)?.waits?.includes("submitting device no longer has access");
  }, t.signal);
  assert.ok(fs.existsSync(path.join(botFolder, "bot.json")));
  assert.ok(!fs.existsSync(path.join(c.home, "attachments", "ses_owned")));
  assert.deepEqual((await editRead(c, oldRef)).value.fields, {});
  assert.deepEqual(prompts.map(e => e.id), ["msg_beforerevocation"]);

  const phoneAgain = await open(b, phoneKey);
  // An old managed draft stays visible until a deliberate edit replaces it with its own grant.
  const heldFile = path.join(b.run, "bots-held", bot + ".json");
  const oldBot = { id: "bte_abcdefghijklmnop", bot, to: c.key, change: { name: "Original draft", jobLine: "Old text" },
    times: { name: 1, jobLine: 1 }, since: 1, at: 1 };
  const key = crypto.createHmac("sha256", Buffer.from(b.read().secret, "base64url")).update("kuvo bot edits held").digest();
  const sealed = [oldBot.id, bot, c.key, null, oldBot.change, oldBot.times, false, 1, 1];
  fs.writeFileSync(heldFile, JSON.stringify({ ...oldBot, seal: crypto.createHmac("sha256", key).update(JSON.stringify(sealed)).digest("base64url") }));
  const reviewed = await phoneAgain.ask("GET", "kuvo/bots");
  assert.match(reviewed.value.held.find(entry => entry.bot === bot).waits, /predates device tracking/);
  const resume = relay.pause(b.address, c.address);
  t.after(resume);
  const replaced = await phoneAgain.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "New edit" });
  assert.equal(replaced.status, 202, JSON.stringify(replaced));
  assert.deepEqual(replaced.value.held.change, { name: "New edit" });
});

test("phone and desktop submit the same complete draft to the conversation owner", { timeout: 110_000 }, async t => {
  const engineModels = {
    "/api/provider": [{ id: "deepseek" }],
    "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4" }],
    "/api/integration": [{ id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential" }] }],
    "/api/model/default": { providerID: "deepseek", id: "v4" },
    "/api/command": [{ name: "init" }],
  };
  const { a, b, c, relay, prompts, draftEffects, see, showOwnedConversation } = await network(t, true, undefined, true, undefined, false, false, undefined, { engineModels });
  await see(c, 2, a);
  await until("primary lease", () => a.lease(2, b), t.signal);
  const phone = link.newKey();
  const desk = link.newKey();
  const pair = await a.route("POST", "kuvo/phones/pair");
  const phoneA = await deviceAt(relay, a, phone, { pair: new URL(pair.value.link).searchParams.get("token"), name: "Phone" });
  assert.ok(phoneA.greeting);
  t.after(phoneA.close);
  const paired = await a.route("POST", "kuvo/devices/pair", { key: link.publicOf(desk).toString("base64url"), name: "Desk", kind: "desktop" }, true);
  assert.equal(paired.status, 200, JSON.stringify(paired));
  const route = "kuvo/sessions/ses_owned/draft";
  for (const [key, hello, id] of [[phone, {}, "msg_draftphone"], [desk, { client: "desktop" }, "msg_draftdesk"]]) {
    const device = await until("owner device session", async () => {
      const connection = await deviceAt(relay, c, key, hello);
      return connection.greeting && connection;
    }, t.signal);
    t.after(device.close);
    const draft = { id, text: "Continue", quotedText: "> Context\n\nContinue", message: {
      delivery: "steer", files: [{ uri: "data:text/plain;base64,Y29udGV4dA==", name: "context.txt" }],
    } };
    const initial = await device.ask("POST", route, draft);
    assert.equal(initial.status, 200, JSON.stringify(initial));
    assert.equal(initial.value.message, id, JSON.stringify(initial));
    assert.equal(initial.value.answer.send.text, "Continue");
    assert.match(prompts.at(-1).text, /> Context/);
    assert.ok(prompts.at(-1).files?.length || prompts.at(-1).text.includes("context.txt"));
    showOwnedConversation(false);
    assert.deepEqual(await device.ask("POST", route, draft), initial);
    assert.equal((await device.ask("POST", route, { ...draft, text: "Changed" })).status, 409);
    showOwnedConversation(true);
    const side = await device.ask("POST", route, { id: id + "side", text: "Aside", side: true, message: {} });
    assert.deepEqual(side, { status: 200, value: { answer: { sideThread: true } } });
    const command = await device.ask("POST", route, { id: id + "slash", text: "/init now", message: {} });
    assert.equal(command.value.answer?.command?.name, "init", JSON.stringify(command));
    const shell = { id: id + "shell", text: "!test", message: {} };
    const ran = await device.ask("POST", route, shell);
    assert.equal(ran.value.answer?.shell, "test", JSON.stringify(ran));
    assert.deepEqual(await device.ask("POST", route, shell), ran);
    engineModels["/api/model/default"] = { providerID: "deepseek", id: "missing" };
    const refused = await device.ask("POST", route, { id: id + "stale", text: "Did readiness change?", message: {} });
    assert.equal(refused.value.answer?.restore?.text, "Did readiness change?", JSON.stringify(refused));
    engineModels["/api/model/default"] = { providerID: "deepseek", id: "v4" };
  }
  assert.deepEqual(draftEffects.map(effect => effect.effect), ["command", "shell", "command", "shell"]);
  const member = await until("paired session on the listing Box", async () => {
    const connection = await deviceAt(relay, b, phone);
    return connection.greeting && connection;
  }, t.signal);
  t.after(member.close);
  const permit = relay.pause(b.address, c.address);
  t.after(permit);
  const queued = { id: "msg_ownerdraft", text: "Send from the listing Box", message: { delivery: "steer" } };
  const waits = await member.ask("POST", route, queued);
  assert.equal(waits.status, 202, JSON.stringify(waits));
  const outbox = JSON.parse(fs.readFileSync(path.join(b.run, "owned-data", "outbox", waits.value.operation + ".json"), "utf8"));
  assert.equal(outbox.queuedMessage, undefined);
  assert.equal(outbox.operation, undefined);
  assert.equal(outbox.queuedDraft.body.text, queued.text);
  assert.equal((await member.ask("POST", route, { ...queued, id: "msg_liveonly", text: "!test" })).status, 400);
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftphone", "msg_draftdesk"]);
  permit();
  await until("the owner accepts the queued full draft", async () => {
    const receipt = await b.route("GET", `kuvo/data/messages/${waits.value.operation}`);
    return receipt.value.status === "committed";
  }, t.signal);
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftphone", "msg_draftdesk", "msg_ownerdraft"]);
  const settled = await member.ask("POST", route, queued);
  assert.equal(settled.value.message, queued.id);
  assert.equal(typeof settled.value.acceptedAt, "number");
  assert.deepEqual(await member.ask("POST", route, queued), settled);

  const submit = "kuvo/sessions/ses_owned/submit";
  const recover = "kuvo/sessions/ses_owned/submissions";
  const scope = await member.ask("GET", recover + "?scope=1");
  assert.equal(scope.value.currentOwner, c.key);
  assert.equal(scope.value.canSubmit, true);
  const pinned = { ...queued, id: "msg_recoverdraft", submission: scope.value.setup + "." + c.key + ".msg_recoverdraft" };
  const loseAnswer = relay.pause(c.address, b.address);
  t.after(loseAnswer);
  const holding = await member.ask("POST", submit, pinned);
  assert.equal(holding.status, 202, JSON.stringify(holding));
  await until("owner prompt before listing's receipt", () => prompts.some(prompt => prompt.id === pinned.id), t.signal);
  const recovered = await phoneA.ask("GET", recover + "/" + pinned.submission);
  assert.equal(recovered.status, 200, JSON.stringify(recovered));
  assert.equal(recovered.value.outcomes[0]?.status, "committed", JSON.stringify(recovered));
  assert.equal(recovered.value.outcomes[0]?.effect.message, pinned.id);
  assert.equal(recovered.value.outcomes[0]?.originalOwner, c.key);
  loseAnswer();
  await until("original listing receipt", async () => {
    const saved = await member.ask("GET", "kuvo/data/messages/" + holding.value.operation);
    return saved.value.status === "committed";
  }, t.signal);
  assert.equal(prompts.filter(prompt => prompt.id === pinned.id).length, 1);
  const changed = await phoneA.ask("POST", submit, { ...pinned, text: "Changed intent on another lister" });
  assert.equal(changed.status, 202, JSON.stringify(changed));
  let changedReceipt;
  await until("changed intent refused by the original owner", async () => {
    const receipt = await phoneA.ask("GET", "kuvo/data/messages/" + changed.value.operation);
    if (receipt.value.status !== "interrupted") return false;
    changedReceipt = receipt.value;
    return true;
  }, t.signal);
  const conflict = await phoneA.ask("GET", recover + "/" + pinned.submission);
  assert.equal(conflict.value.outcomes[0]?.conflict, true);
  const different = await phoneA.ask("GET", recover + "/" + pinned.submission + "?digest=" + changedReceipt.digest);
  assert.equal(different.value.outcomes[0]?.status, "unknown");
  assert.equal(different.value.outcomes[0]?.draft.text, "Changed intent on another lister");
  assert.equal(prompts.filter(prompt => prompt.id === pinned.id).length, 1);

  const deskListing = await until("desktop on the listing Box", async () => {
    const connection = await deviceAt(relay, b, desk, { client: "desktop" });
    return connection.greeting && connection;
  }, t.signal);
  t.after(deskListing.close);
  const acceptedThenRevoked = { ...pinned, id: "msg_beforerevoke", submission: scope.value.setup + "." + c.key + ".msg_beforerevoke" };
  const resumeAccepted = relay.pause(c.address, b.address);
  t.after(resumeAccepted);
  const acceptedReceipt = await deskListing.ask("POST", submit, acceptedThenRevoked);
  assert.equal(acceptedReceipt.status, 202, JSON.stringify(acceptedReceipt));
  const claimFile = path.join(c.run, "owned-data", "submissions", acceptedReceipt.value.operation + ".json");
  const acceptedAt = await until("owner commits the desktop draft before revocation", () => {
    if (!fs.existsSync(claimFile)) return false;
    const claim = JSON.parse(fs.readFileSync(claimFile, "utf8"));
    return claim.answer?.value?.message === acceptedThenRevoked.id && claim.at;
  }, t.signal);
  const parked = { ...pinned, id: "msg_revokeddraft", submission: scope.value.setup + "." + c.key + ".msg_revokeddraft" };
  const resumeDelivery = relay.pause(b.address, c.address);
  t.after(resumeDelivery);
  const waiting = await deskListing.ask("POST", submit, parked);
  assert.equal(waiting.status, 202, JSON.stringify(waiting));
  const revoked = await phoneA.ask("POST", "kuvo/phones/remove", { phones: [{ key: link.publicOf(desk).toString("base64url"), pairing: paired.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  await until("owner has the desktop revocation", () => !c.read().phones.some(device => device.key === link.publicOf(desk).toString("base64url")), t.signal);
  resumeAccepted();
  resumeDelivery();
  const archiveFolder = path.join(b.run, "owned-data", "message-receipts");
  const acceptedFile = path.join(archiveFolder, acceptedReceipt.value.operation + ".json");
  await until("committed receipt survives the revocation", () => fs.existsSync(acceptedFile), t.signal);
  const recoveredAcceptance = JSON.parse(fs.readFileSync(acceptedFile, "utf8")).receipt;
  assert.equal(recoveredAcceptance.status, "committed");
  assert.equal(recoveredAcceptance.acceptedAt, acceptedAt);
  assert.equal(prompts.filter(prompt => prompt.id === acceptedThenRevoked.id).length, 1);
  const retired = path.join(archiveFolder, waiting.value.operation + ".json");
  await until("interrupted receipt from the revoked pairing", () => fs.existsSync(retired), t.signal);
  assert.equal(JSON.parse(fs.readFileSync(retired, "utf8")).receipt.status, "interrupted");
  assert.equal(prompts.filter(prompt => prompt.id === parked.id).length, 0);
  const hidden = await phoneA.ask("GET", recover + "/" + parked.submission);
  assert.equal(hidden.value.outcomes.length, 0);
});

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
  b.clock(TAKEOVER + 5_000);
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

const waitForWipe = async (box, signal, timeout = 12_000) => {
  try {
    await until("box erased", () => !fs.existsSync(box.home), AbortSignal.any([signal, AbortSignal.timeout(timeout)]));
  } catch (error) {
    const log = path.join(box.run, "uninstall.log");
    throw new Error(`${error.message} on ${box.name}: journal=${JSON.stringify(box.read().localWipe)}, log=${fs.existsSync(log) ? fs.readFileSync(log, "utf8") : "(none)"}`, { cause: error });
  }
};
const removalOf = (box, peer) => box.read().shared?.membershipAndAccess?.value.removals?.[peer.key];

function seedLocalWork(box, { botWork = true } = {}) {
  const bot = path.join(box.home, "bots", "bot_retained", "notes.md");
  const session = path.join(box.home, "agents", "sessions", "ses_retained", "messages.json");
  const files = botWork ? [bot, session] : [session];
  for (const file of files) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "local work");
  }
  return files;
}

const inlineFiles = payloads => Object.entries(payloads).map(([name, bytes]) => ({
  name, uri: `data:application/octet-stream;base64,${bytes.toString("base64")}`,
}));

async function uploadSessionFiles(box, id, payloads) {
  const sent = await box.route("POST", "kuvo/sessions/ses_owned/send", {
    id, words: "Open the attached files", files: inlineFiles(payloads),
  });
  assert.equal(sent.status, 200, JSON.stringify(sent.value));
  const folder = "attachments/ses_owned";
  const uploads = fs.readdirSync(path.join(box.home, folder), { withFileTypes: true });
  return uploads.filter(upload => upload.isDirectory()).flatMap(upload => {
    const relative = `${folder}/${upload.name}`;
    const names = fs.readdirSync(path.join(box.home, relative));
    return names.filter(name => Object.hasOwn(payloads, name)).map(name => `${relative}/${name}`);
  });
}

async function uploadQueueFiles(box, payloads, owner = "ses_owned") {
  const added = await box.route("POST", "kuvo/work-queue/items", {
    owner, operation: "add-backup-fixture-attachments", title: "Keep these files", files: inlineFiles(payloads),
  }, true);
  assert.equal(added.status, 200, JSON.stringify(added.value));
  const { id } = added.value.item;
  const read = await box.route("GET", `kuvo/work-queue/items/${id}`);
  assert.equal(read.status, 200, JSON.stringify(read.value));
  const { files } = read.value.item;
  assert.deepEqual(files.map(({ name, size }) => [name, size]), Object.entries(payloads).map(([name, bytes]) => [name, bytes.length]));
  return { id, paths: files.map(({ name }) => `run/work-queue/files/${id}/${name}`) };
}

test("failure bench: online unlink from a member keeps the target’s sessions and bots standalone", { timeout: 25_000 }, async t => {
  const { a, b, c } = await network(t);
  const data = seedLocalWork(b);
  const original = b.read().setup.id;
  const operation = crypto.randomBytes(16).toString("base64url");
  const removed = await c.route("POST", `kuvo/computers/${b.key}/unlink`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  assert.deepEqual(removed.value.removal, { operation, state: "unlinked", accessRemoved: true });
  await until("new standalone setup", () => b.read().setup?.id !== original && b.read().setup?.role === "primary", t.signal);
  await until("unlink reached all members", () => removalOf(a, b)?.delivered && removalOf(c, b)?.delivered, t.signal);
  assert.deepEqual(b.read().phones, []);
  assert.deepEqual(link.peersOf(b.read()), []);
  assert.deepEqual(data.map(file => fs.readFileSync(file, "utf8")), ["local work", "local work"]);
  assert.deepEqual(link.peersOf(c.read()).map(peer => peer.key), [a.key]);
  const retry = await a.route("POST", `kuvo/computers/${b.key}/unlink`, { operation });
  assert.deepEqual(retry.value.proof, removed.value.proof);
});

test("failure bench: an offline member is unlinked at commit and wipes after returning", { timeout: 85_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, false, undefined, true, false, undefined, { acceptedRoster: true });
  const data = seedLocalWork(b);
  const dest = path.join(b.env.HOME, "external-backup");
  fs.mkdirSync(dest);
  fs.mkdirSync(path.join(b.home, "backup"));
  const source = "b_" + crypto.randomBytes(16).toString("base64url");
  fs.writeFileSync(path.join(b.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: crypto.randomBytes(32).toString("base64url"),
    dest: { kind: "disk", path: dest }, disk: fs.statSync(dest).dev, projects: [],
  }));
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = crypto.randomBytes(16).toString("base64url");
  const removed = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  assert.deepEqual(removed.value.removal, { operation, state: "wipe-pending", accessRemoved: true });
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === b.key), false);
  await until("pending wipe on another client’s box", () => removalOf(c, b)?.operation === operation, t.signal);
  assert.deepEqual((await c.route("GET", "kuvo/computers")).value.wipePending, [{ key: b.key, name: b.name, operation }]);
  assert.ok(data.every(file => fs.existsSync(file)));
  b.start();
  await until("target returned to the relay", () => relay.has(b.address) || !fs.existsSync(b.home), t.signal);
  await waitForWipe(b, t.signal, 25_000);
  await until("completion replicated", () => removalOf(a, b)?.delivered && removalOf(c, b)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(8500)]));
  assert.equal((await c.route("GET", "kuvo/computers")).value.wipePending, undefined);
  assert.ok(fs.readdirSync(path.join(dest, "kuvo-backups", source)).some(file => file.includes("-wipe-")), "verified backup survived wipe");
});

for (const appOwner of [false, true]) test(
  `failure bench: revoked Relay-only ${appOwner ? "app-owned Mac" : "member"} accepts a verified removal export after reconnect`,
  { timeout: 160_000 }, async t => {
    const { a, b: receiver, c: source, relay } = await network(t, false, undefined, false, undefined, true, false, undefined, { appOwner, acceptedRoster: true });
    const backup = async (action, body) => {
      const route = "kuvo/box/backups/" + action;
      if (!appOwner) return source.route("POST", route, body, true);
      const { port, password } = JSON.parse(fs.readFileSync(path.join(source.run, "serve.json"), "utf8"));
      const response = await fetch(`http://127.0.0.1:${port}/${route}`, {
        method: "POST", headers: { authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64"), "x-kuvo-from": "app" },
        body: JSON.stringify(body), signal: t.signal,
      });
      return { status: response.status, value: await response.json() };
    };
    const data = seedLocalWork(source, { botWork: false });
    const sibling = source.appFolder && path.join(source.appFolder, "Attachments", "ses_retained", "message", "design.new");
    if (sibling) {
      fs.mkdirSync(path.dirname(sibling), { recursive: true });
      fs.writeFileSync(sibling, Buffer.of(0, 255, 18));
      data.push(sibling);
    }
    const outside = path.join(source.env.HOME, "unrelated");
    fs.writeFileSync(outside, "keep");
    const destination = { kind: "computer", computer: receiver.key };
    const configured = await backup("destination", { destination, confirm: true });
    assert.equal(configured.status, 200, JSON.stringify(configured.value));
    const daily = await backup("now", {});
    assert.equal(daily.status, 200, JSON.stringify(daily.value));
    assert.match(daily.value.point.id, /-daily-/);
    const setup = a.read().setup.id;
    const deposit = path.join(receiver.env.HOME, ".kuvo-backups", "relay", setup, source.key, configured.value.source, daily.value.point.id + ".kuvo");
    const original = fs.readFileSync(deposit);
    fs.writeFileSync(data[0], "saved after the daily backup");
    await source.stop();
    await until("source offline", () => !relay.has(source.address), t.signal);
    const operation = operationID();
    const started = await a.route("POST", wipeRoute(source), { operation });
    assert.equal(started.status, 200, JSON.stringify(started.value));
    assert.deepEqual(started.value.removal, { operation, state: "wipe-pending", accessRemoved: true });
    await until("receiver replicated revocation", () => removalOf(receiver, source)?.operation === operation, t.signal);
    assert.equal(link.peersOf(receiver.read()).some(peer => peer.key === source.key), false);
    const denied = await receiver.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import { relayBackupAct } from ${JSON.stringify(new URL("../src/link/backup/relay-store.ts", import.meta.url).href)};
      const sender = { key: ${JSON.stringify(source.key)} };
      const item = ${JSON.stringify(configured.value.source)};
      const body = { setup: ${JSON.stringify(setup)}, id: ${JSON.stringify(daily.value.point.id)} };
      console.log(JSON.stringify(await Promise.all([
        relayBackupAct(sender, { method: "GET", item, body: { ...body, action: "list" } }),
        relayBackupAct(sender, { method: "GET", item, body: { ...body, action: "read", offset: 0 } }),
        relayBackupAct(sender, { method: "POST", item, body: { ...body, action: "begin", size: 5, sha256: "a".repeat(64) } }),
        relayBackupAct(sender, { method: "DELETE", item, body }),
      ])));
    `]);
    assert.equal(denied.status, 0, denied.err);
    assert.deepEqual(denied.value.map(reply => reply.status), [409, 409, 409, 409]);
    if (appOwner) source.onShutdown = `
      import fs from "node:fs";
      process.once("SIGTERM", () => {
        fs.writeFileSync(${JSON.stringify(data[0])}, "saved while stopping");
        fs.writeFileSync(${JSON.stringify(path.join(source.env.TMPDIR, "keeper-stopped"))}, "");
        process.exit(0);
      });
    `;
    let plainGrant;
    const observeGrant = relay.intercept(a.address, source.address, packet => {
      const instruction = link.readDoc(removalMessage(packet, source, a)?.instruction);
      if (instruction?.body.wipe && !instruction.body.plan) plainGrant = packet;
      return false;
    });
    t.after(observeGrant);
    source.start();
    await until("review replicated after reconnect", () => removalOf(receiver, source)?.needsExport,
      AbortSignal.any([t.signal, AbortSignal.timeout(18_000)]));
    const pending = await a.route("POST", wipeRoute(source), { save: started.value });
    assert.equal(pending.value.removal.state, "review");
    assert.equal(pending.value.removal.nextAction, "choose-export");
    assert.match(pending.value.message, /Relay.*separate disk or SSH/);
    assert.equal((await receiver.route("GET", "kuvo/computers")).value.wipePending[0].nextAction, "choose-export");
    assert.ok(data.every(file => fs.existsSync(file)));
    if (!appOwner) {
      const config = path.join(source.home, "backup", "configuration.json");
      const enabled = fs.readFileSync(config);
      fs.writeFileSync(config, JSON.stringify({ ...JSON.parse(enabled), enabled: false }));
      const originalGrant = plainGrant;
      assert.ok(originalGrant);
      const after = relay.delivered.length;
      relay.replay(originalGrant);
      await until("fresh plain wipe instruction still held", () => plainGrant !== originalGrant, t.signal);
      await until("source repeated its refusal", () => relay.delivered.slice(after).some(packet =>
        packet.to.equals(a.address) && removalMessage(packet, a, source)?.review === "choose-export"), t.signal);
      assert.ok(data.every(file => fs.existsSync(file)));
      assert.equal(source.read().localWipe, undefined);
      fs.writeFileSync(config, enabled);
    }
    const target = { kind: "disk", path: path.join(source.env.HOME, "new-export") };
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    const select = () => a.route("POST", wipeRoute(source), { operation, prepare: { export: { destination: target, recoveryKey } } }, true);
    assert.equal((await select()).value.removal.state, "exporting");
    await until("unavailable destination reported", () => removalOf(a, source)?.error, t.signal);
    assert.ok(data.every(file => fs.existsSync(file)));
    fs.mkdirSync(target.path);
    let done;
    const releaseDone = relay.intercept(null, a.address, packet => {
      if (removalMessage(packet, a, source)?.done !== operation) return false;
      done = packet;
      return true;
    });
    t.after(releaseDone);
    const retry = await a.route("POST", wipeRoute(source), { operation, retryPreparation: true }, true);
    assert.equal(retry.status, 200, JSON.stringify(retry.value));
    if (appOwner) {
      await until("shutdown invalidated readiness", () => /Source work changed/.test(removalOf(a, source)?.error ?? ""), t.signal);
      assert.equal(removalOf(a, source).delivered, undefined);
      assert.equal(fs.readFileSync(data[0], "utf8"), "saved while stopping");
      const again = await a.route("POST", wipeRoute(source), { operation, retryPreparation: true }, true);
      assert.equal(again.status, 200, JSON.stringify(again.value));
    }
    await waitForWipe(source, t.signal, 30_000);
    await until("completion sent after erase", () => done, t.signal);
    assert.equal(removalOf(a, source).delivered, undefined);
    releaseDone();
    relay.replay(done);
    await until("completion replicated", () => removalOf(receiver, source)?.delivered, t.signal);
    assert.ok(data.every(file => !fs.existsSync(file)));
    if (source.appFolder) {
      assert.equal(fs.existsSync(source.appFolder), false);
      if (process.platform === "darwin") assert.deepEqual(fs.readFileSync(path.join(source.env.HOME, "keychain-cleared"), "utf8").trim().split("\n"),
        ["key", "secret", "sign"].map(account => `delete-generic-password -s run.kuvo.code.fixture.phone-link -a ${account}`));
    }
    assert.equal(fs.readFileSync(outside, "utf8"), "keep");
    assert.deepEqual(fs.readFileSync(deposit), original);
    const ended = await a.route("POST", wipeRoute(source), { save: started.value });
    assert.equal(ended.value.removal.state, "wiped");
    assert.equal(ended.value.offer, undefined);
    assert.equal((await receiver.route("GET", "kuvo/computers")).value.wipePending, undefined);
    const point = ended.value.export;
    const archive = path.join(target.path, "kuvo-backups", point.source, point.id + ".kuvo");
    const bytes = await a.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import fs from "node:fs";
      import { archiveContext, openArchive } from ${JSON.stringify(new URL("../src/link/backup/archive.ts", import.meta.url).href)};
      const point = ${JSON.stringify(point)};
      const entries = openArchive(${JSON.stringify(archive)}, ${JSON.stringify(recoveryKey)},
        archiveContext(point.source, point.id), fs.mkdtempSync(process.env.TMPDIR + "/inspect-"));
      console.log(JSON.stringify(Object.fromEntries(entries.filter(({ name }) => name.endsWith("/ses_retained/messages.json") ||
        name.endsWith("/design.new")).map(({ name, file }) => [name.split("/").at(-1), fs.readFileSync(file).toString("base64")] ))));
    `]);
    assert.equal(bytes.status, 0, bytes.err);
    assert.equal(bytes.value["messages.json"], Buffer.from(appOwner ? "saved while stopping" : "saved after the daily backup").toString("base64"));
    if (sibling) assert.equal(bytes.value["design.new"], Buffer.of(0, 255, 18).toString("base64"));
  },
);

test("failure bench: unsupported erase layout stays pending and can cancel without losing work", { timeout: 35_000 }, async t => {
  const { a, b: source } = await network(t);
  const kept = seedLocalWork(source);
  const operation = operationID();
  const started = await a.route("POST", wipeRoute(source), { operation });
  assert.equal(started.value.removal.state, "wipe-pending");
  await until("unsupported layout held for review", () => removalOf(a, source)?.error, t.signal);
  const review = await a.route("POST", wipeRoute(source), { save: started.value });
  assert.equal(review.value.removal.state, "review");
  assert.equal(review.value.removal.nextAction, "cancel-wipe");
  const cancelled = await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.value.removal.state, "cancelled");
  await until("standalone setup retained the work", () => source.read().setup.role === "primary", t.signal);
  assert.ok(kept.every(file => fs.existsSync(file)));
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === source.key), false);
});

test("failure bench: a missing computer stays wipe-pending across primary restart", { timeout: 25_000 }, async t => {
  const { a, b, c, relay } = await network(t);
  await b.stop();
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  await a.stop("SIGTERM");
  a.start();
  await until("primary reconnected", () => relay.has(a.address), t.signal);
  await until("pending removal replicated", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
  assert.deepEqual((await a.route("GET", "kuvo/computers")).value.wipePending, [{ key: b.key, name: b.name, operation: result.value.operation }]);
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === b.key), false);
});

test("failure bench: a lost disk closes a pending wipe and preserves the key revocations", { timeout: 25_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = operationID();
  const started = await a.route("POST", wipeRoute(b), { operation });
  assert.equal(started.status, 200);
  const lost = await a.route("POST", wipeRoute(b), { operation, resolve: "lost" }, true);
  assert.equal(lost.status, 200, JSON.stringify(lost.value));
  assert.deepEqual(lost.value.removal, { operation, state: "lost", accessRemoved: true });
  assert.deepEqual(await a.route("POST", wipeRoute(b), { save: started.value }), lost);
  const membership = a.read().shared.membershipAndAccess.value;
  assert.ok(membership.left.includes(b.key));
  assert.equal(membership.boxes.some(box => box.key === b.key), false);
  assert.equal(membership.removals[b.key].peer.sign, b.sign);
  assert.equal((await a.route("GET", "kuvo/computers")).value.wipePending, undefined);
});

test("failure bench: unlinking the only computer wipes its local setup", { timeout: 25_000 }, async t => {
  const { a, b, c, phase } = await network(t, false, undefined, false, undefined, true, false, undefined,
    { phaseDiagnostics: true });
  const data = seedLocalWork(a);
  const blocked = await a.route("POST", `kuvo/computers/${a.key}/unlink`);
  assert.equal(blocked.status, 409);
  assert.match(blocked.value.message, /Choose another main computer/);
  for (const peer of [b, c]) {
    const result = await a.route("POST", `kuvo/computers/${peer.key}/unlink`);
    assert.equal(result.status, 200, JSON.stringify(result.value));
    assert.equal(result.value.offer, undefined);
  }
  phase("both member unlink decisions saved");
  await until("both unlinks delivered", () => removalOf(a, b)?.delivered && removalOf(a, c)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(8500)]));
  phase("both member unlink deliveries acknowledged");
  const setup = a.read().setup.id;
  const last = await a.route("POST", `kuvo/computers/${a.key}/unlink`);
  assert.equal(last.status, 200, JSON.stringify(last.value));
  assert.deepEqual(last.value.removal, { operation: last.value.operation, state: "wipe-pending", accessRemoved: true });
  assert.equal(last.value.offer, "reinstall-local");
  phase("last-computer wipe accepted with local reinstall offer");
  await waitForWipe(a, t.signal);
  phase("old installation detached");
  assert.ok(data.every(file => !fs.existsSync(file)));
  const marker = path.join(path.dirname(a.home), ".kuvo-code-removal-receipts", "last-setup-wipe");
  const receipt = await until("the detached installation is erased", () => {
    const value = JSON.parse(fs.readFileSync(marker, "utf8"));
    return !value.phase && value;
  }, t.signal);
  phase("detached wipe journal completed");
  assert.deepEqual(receipt, { setup, operation: last.value.operation });
  assert.equal(fs.statSync(marker).mode & 0o777, 0o600);
});

test("failure bench: a sole primary can supersede and cancel a prepared removal", { timeout: 35_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true, true);
  const data = seedLocalWork(a);
  assert.equal((await a.route("POST", `kuvo/computers/${b.key}/unlink`)).status, 200);
  await until("unlink acknowledged", () => removalOf(a, b)?.delivered, t.signal);
  const prepare = name => ({ export: { destination: { kind: "disk", path: path.join(a.env.TMPDIR, name) },
    recoveryKey: crypto.randomBytes(32).toString("base64url") } });
  const operation = operationID();
  const first = await a.route("POST", wipeRoute(a), { operation, prepare: prepare("missing-original") }, true);
  assert.equal(first.status, 200, JSON.stringify(first.value));
  await until("local preparation failed", () => removalOf(a, a)?.error, AbortSignal.timeout(8_000));
  const revised = operationID();
  const second = await a.route("POST", wipeRoute(a), { operation: revised, supersedes: operation, prepare: prepare("missing-replacement") }, true);
  assert.equal(second.status, 200, JSON.stringify(second.value));
  assert.equal((await a.route("POST", wipeRoute(a), { save: first.value })).value.removal.state, "cancelled");
  await until("revised preparation failed", () => removalOf(a, a)?.operation === revised && removalOf(a, a)?.error, AbortSignal.timeout(8_000));
  const cancelled = await a.route("POST", wipeRoute(a), { operation: revised, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200, JSON.stringify(cancelled.value));
  assert.equal(cancelled.value.removal.state, "cancelled");
  assert.equal(a.read().setup.role, "primary");
  assert.equal(a.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  const fresh = operationID();
  const started = await a.route("POST", wipeRoute(a), { operation: fresh, prepare: prepare("missing-fresh") }, true);
  assert.equal(started.status, 200, JSON.stringify(started.value));
  assert.equal((await a.route("POST", wipeRoute(a), { operation: revised })).value.removal.state, "cancelled");
  assert.equal(removalOf(a, a).operation, fresh);
});

test("failure bench: a pending wipe follows the setup through handover", { timeout: 30_000 }, async t => {
  const { a, b, c, see, relay } = await network(t, false, undefined, false, undefined, true);
  seedLocalWork(b);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  assert.equal((await submit(a, c, 1)).status, 202);
  await Promise.all([see(a, 2, c), see(c, 2, c)]);
  assert.equal(removalOf(c, b).operation, result.value.operation);
  assert.equal(removalOf(c, b).delivered, undefined);
  b.start();
  await waitForWipe(b, t.signal);
  await until("new primary records completion", () => removalOf(c, b)?.delivered, t.signal);
});

test("failure bench: a failed pre-wipe backup leaves the removed computer’s data in place", { timeout: 25_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  fs.mkdirSync(path.join(b.home, "backup"));
  fs.writeFileSync(path.join(b.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source: "b_" + crypto.randomBytes(16).toString("base64url"),
    key: crypto.randomBytes(32).toString("base64url"), disk: 1, projects: [],
    dest: { kind: "disk", path: path.join(b.env.HOME, "missing-disk") },
  }));
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  const log = path.join(b.run, "uninstall.log");
  await until("backup attempt failed", () => fs.existsSync(log) && fs.readFileSync(log, "utf8").includes("missing-disk"), t.signal);
  assert.deepEqual(data.map(file => fs.readFileSync(file, "utf8")), ["local work", "local work"]);
  assert.equal(b.read().localWipe.operation, result.value.operation);
  assert.equal(b.read().localWipe.prepared, undefined);
  assert.equal(removalOf(a, b).delivered, undefined);
});

const operationID = () => crypto.randomBytes(16).toString("base64url");
const wipeRoute = box => `kuvo/computers/${box.key}/unlink-and-wipe`;
function preparation(box, role, operation) {
  const dir = path.join(box.run, "wipe-preparation");
  if (!fs.existsSync(dir)) return undefined;
  for (const name of fs.readdirSync(dir)) {
    if (!name.startsWith(role + "-") || !name.endsWith(".json")) continue;
    const state = JSON.parse(fs.readFileSync(path.join(dir, name), "utf8"));
    if (state.operation === operation) return state;
  }
  return undefined;
}
async function waitForPreparedRemoval(primary, source, operation, signal) {
  await until("prepared removal committed", () => {
    const failure = preparation(primary, "intent", operation)?.error ?? preparation(source, "source", operation)?.error;
    assert.equal(failure, undefined);
    const removal = removalOf(primary, source);
    return removal?.operation === operation && removal.generation;
  }, signal);
}

test("failure bench: move all verifies accepted ownership before wiping the source", { timeout: 60_000 }, async t => {
  const group = { id: "grp_beforewipe", name: "Before wipe", members: ["bot_one", "bot_two"], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true);
  const data = seedLocalWork(c, { botWork: false });
  const planned = await a.route("POST", `kuvo/computers/${c.key}/removal-plan`, {}, true);
  assert.equal(planned.status, 200, JSON.stringify(planned.value));
  const plan = planned.value.plan;
  assert.equal(plan.source.key, c.key);
  assert.equal(plan.complete, true);
  assert.deepEqual(plan.movable.map(({ id, destinations }) => [id, destinations.map(place => place.key)]), [[group.id, [a.key]]]);
  assert.ok(plan.stays.includes("native-agent-histories"));
  assert.equal(Object.hasOwn(plan, "projects"), false);
  const temp = c.env.TMPDIR;
  const project = fs.realpathSync(fs.mkdtempSync(path.join(temp, "chosen-project-")));
  c.env.TMPDIR = fs.mkdtempSync(path.join(temp, "source-scratch-"));
  try {
    const selected = await c.route("POST", `kuvo/computers/${c.key}/removal-plan`, { projects: [project] }, true);
    assert.equal(selected.status, 200, JSON.stringify(selected.value));
    assert.deepEqual(selected.value.plan.projects, [project]);
  } finally { c.env.TMPDIR = temp; }
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "moving");
  assert.equal(removalOf(a, c).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  await waitForPreparedRemoval(a, c, operation, AbortSignal.timeout(30_000)).catch(error => {
    const moves = box => {
      const folder = path.join(box.run, "moves");
      return fs.existsSync(folder) ? fs.readdirSync(folder).filter(file => file.endsWith(".json")).map(file => JSON.parse(fs.readFileSync(path.join(folder, file), "utf8"))) : [];
    };
    throw new Error(`${error.message}. Source moves ${JSON.stringify(moves(c))}. Destination moves ${JSON.stringify(moves(a))}.`);
  });
  await waitForWipe(c, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  const assignment = a.read().shared.ownershipAssignments.value["group host:" + group.id];
  assert.equal(assignment.computer, a.key);
  assert.equal(groupCopies.get("Studio")?.group.id, group.id);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an offline Relay source completes its accepted moves and export on reconnect", { timeout: 150_000 }, async t => {
  const group = { id: "grp_offlinemove", name: "Offline move", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, relay, groupCopies } = await network(t, false, undefined, false, group, true);
  const bridge = JSON.parse(fs.readFileSync(path.join(a.run, "bridge.json"), "utf8"));
  fs.writeFileSync(path.join(b.run, "bridge.json"), JSON.stringify({ ...bridge, password: b.name }));
  await until("destination supports accepted moves", () => a.heard(b.key) && c.heard(b.key), t.signal);
  await c.stop("SIGTERM");
  await until("move source offline", () => !relay.has(c.address), t.signal);
  fs.mkdirSync(path.join(c.home, "backup"), { recursive: true });
  fs.writeFileSync(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source: "b_" + operationID(), key: crypto.randomBytes(32).toString("base64url"),
    dest: { kind: "computer", computer: b.key }, projects: [],
  }));
  fs.writeFileSync(path.join(c.run, "peer-reach.json"), JSON.stringify({ [b.key]: Date.now() }));
  // After access is revoked, the destination can't clear the source's stale reachability by ordinary mail.
  const scoped = [];
  const seeRemoval = (packet, target, sender) => {
    const message = removalMessage(packet, target, sender);
    if (message) scoped.push(message.scopedMove?.op ?? (message.moveProbe ? "probe" : message.moveReady ? "ready for removal" : "other"));
    return message;
  };
  t.after(relay.intercept(b.address, c.address, packet => !seeRemoval(packet, c, b)));
  t.after(relay.intercept(c.address, b.address, packet => { seeRemoval(packet, b, c); return false; }));
  const first = operationID();
  const start = await a.route("POST", wipeRoute(c), { operation: first, prepare: { moveTo: b.key } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  c.start();
  await until("move planning requires a separate export", () => removalOf(a, c)?.error,
    AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  const review = await a.route("POST", wipeRoute(c), { save: start.value });
  assert.equal(review.value.removal.nextAction, "revise-export");
  assert.equal(groupCopies.get("Lab")?.status, "active");
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(c.env.TMPDIR, "move-export-")) };
  const newPlan = await a.route("POST", wipeRoute(c), { operation, supersedes: first,
    prepare: { moveTo: b.key, export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } } }, true);
  assert.equal(newPlan.status, 200, JSON.stringify(newPlan.value));
  await waitForPreparedRemoval(a, c, operation, AbortSignal.any([t.signal, AbortSignal.timeout(55_000)])).catch(error => {
    const state = { source: preparation(c, "source", operation), accepted: removalOf(a, c),
      destination: removalOf(b, c), ownership: b.read().shared?.ownershipAssignments, scoped, groups: [...groupCopies] };
    throw new Error(`${error.message}: ${JSON.stringify(state)}`, { cause: error });
  });
  await waitForWipe(c, t.signal);
  const point = removalOf(a, c).export;
  assert.ok(fs.existsSync(path.join(destination.path, "kuvo-backups", point.source, point.id + ".kuvo")));
  assert.ok(scoped.includes("probe"));
  assert.ok(scoped.includes("ready for removal"));
  assert.equal(a.read().shared.ownershipAssignments.value["group host:" + group.id].computer, b.key);
  assert.equal(groupCopies.get("Desk")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an encrypted export survives wipe and imports through restore", { timeout: 50_000 }, async t => {
  const { a, b: replacement, c: owner, prompts, phase } = await network(t, false, undefined, true, undefined, true, false, undefined,
    { phaseDiagnostics: true });
  const data = seedLocalWork(owner);
  const sessionBytes = {
    "readme.txt": Buffer.from("read me"), "yarn.lock": Buffer.of(0, 1, 255), "design.new": Buffer.of(255, 3, 0),
    ".keep": Buffer.alloc(0), "report.tmp": Buffer.of(0, 128, 255), "report.db": Buffer.of(7, 0, 129),
  };
  const queueBytes = {
    "queue.bin": Buffer.of(1, 0), "queue.lock": Buffer.of(255, 1), "queue.new": Buffer.of(128, 1),
    "queue.tmp": Buffer.of(0, 1, 2, 255), "queue.db": Buffer.of(7, 6, 5), "queue.db-wal": Buffer.of(0, 128),
  };
  const sessionFiles = await uploadSessionFiles(owner, "msg_backupfiles", sessionBytes);
  const { id: queueItem, paths: queueFiles } = await uploadQueueFiles(owner, queueBytes);
  assert.equal(prompts.at(-1)?.id, "msg_backupfiles");
  assert.deepEqual(sessionFiles.map(file => path.basename(file)).sort(), Object.keys(sessionBytes).sort());
  const promised = [
    ...sessionFiles.map(file => [file, sessionBytes[path.basename(file)]]),
    ...queueFiles.map(file => [file, queueBytes[path.basename(file)]]),
  ];
  for (const [file, bytes] of promised) {
    assert.deepEqual(fs.readFileSync(path.join(owner.home, file)), bytes);
    assert.equal(fs.existsSync(path.join(replacement.home, file)), false);
  }
  phase("session and queue uploads saved and their source bytes verified");
  const destination = { kind: "disk", path: path.join(owner.env.TMPDIR, "manual-export") };
  fs.mkdirSync(destination.path);
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(owner), { operation, prepare: { export: { destination, recoveryKey } } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "exporting");
  phase("removal plan saved with export requested");
  await waitForPreparedRemoval(a, owner, operation, AbortSignal.any([t.signal, AbortSignal.timeout(25_000)]));
  phase("prepared export acknowledged by the primary");
  await waitForWipe(owner, t.signal);
  phase("source installation detached");
  assert.ok(data.every(file => !fs.existsSync(file)));
  await until("verified wipe accepted", () => removalOf(a, owner)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]));
  phase("source wipe completion recorded by the primary");
  const retry = await a.route("POST", wipeRoute(owner), { operation });
  assert.equal(retry.status, 200, JSON.stringify(retry.value));
  const poll = await a.route("POST", wipeRoute(owner), { save: start.value });
  assert.deepEqual(poll, retry);
  const { source, id } = retry.value.export;
  const archive = fs.readFileSync(path.join(destination.path, "kuvo-backups", source, id + ".kuvo"));
  assert.ok(archive.subarray(0, 14).equals(Buffer.from("KUVO backup 1\n")));
  assert.equal(archive.includes("local work"), false);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: owner.key, replacement: replacement.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  await until("replacement ownership replica", () => replacement.read().shared?.ownershipAssignments?.value?.["replacement:" + owner.key],
    AbortSignal.any([t.signal, AbortSignal.timeout(8_000)]));
  phase("approved replacement mapping replicated");
  const restored = await replacement.route("POST", "kuvo/box/backups/restore", { destination, source, id, recoveryKey, confirm: true }, true);
  assert.equal(restored.status, 200, JSON.stringify(restored.value));
  assert.equal(fs.readFileSync(path.join(replacement.home, "bots", "bot_retained", "notes.md"), "utf8"), "local work");
  assert.deepEqual(restored.value.unavailable.filter(({ entry }) => entry === queueItem ||
    entry.startsWith("attachments/") || entry.startsWith("home/run/work-queue/files/")), []);
  for (const [file, bytes] of promised) assert.deepEqual(fs.readFileSync(path.join(replacement.home, file)), bytes, file);
  phase("replacement restored and all promised bytes verified");
  const queue = JSON.parse(fs.readFileSync(path.join(replacement.run, "work-queue/queues/ses_owned.json"), "utf8"));
  assert.deepEqual(queue.items.find(item => item.id === queueItem)?.files.map(file => file.name), Object.keys(queueBytes));
});

test("failure bench: a missing listed attachment blocks a requested export and wipe", { timeout: 40_000 }, async t => {
  const { a, c: source } = await network(t, false, undefined, true, undefined, true);
  const data = seedLocalWork(source);
  const payloads = { "missing.tmp": Buffer.of(0, 7, 255), "keep.bin": Buffer.of(8, 0, 129) };
  const { paths: [missing, retained] } = await uploadQueueFiles(source, payloads);
  fs.unlinkSync(path.join(source.home, missing));
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(source.env.TMPDIR, "incomplete-export-")) };
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(source), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assert.equal(start.value.removal.state, "exporting");
  const failure = await until("missing attachment reported", () => removalOf(a, source)?.error, t.signal);
  assert.match(failure, /missing\.tmp/);
  assert.equal(removalOf(a, source).generation, undefined);
  assert.equal(removalOf(a, source).export, undefined);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.deepEqual(fs.readFileSync(path.join(source.home, retained)), payloads["keep.bin"]);
  assert.equal(fs.readdirSync(destination.path, { recursive: true }).some(file => file.endsWith(".kuvo")), false);
  const bypass = await a.route("POST", wipeRoute(source), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  const cancelled = await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200, JSON.stringify(cancelled.value));
  assert.equal(cancelled.value.removal.state, "cancelled");
  assert.ok(data.every(file => fs.existsSync(file)));
});

test("failure bench: legacy export readiness cannot authorize a wipe", { timeout: 35_000 }, async t => {
  const { a, b: source, relay } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(source);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(source.env.TMPDIR, "legacy-export-")) };
  let ready;
  t.after(relay.intercept(source.address, a.address, packet => {
    const message = removalMessage(packet, a, source);
    if (message?.operation !== operation || !message.progress?.generation) return false;
    ready ??= { packet, message };
    return true;
  }));
  const start = await a.route("POST", wipeRoute(source), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  const { packet, message } = await until("new readiness message", () => ready, t.signal);
  assert.ok(message.progress.export?.id);
  assert.equal(typeof message.progress.inventoryVersion, "number");
  // Model the same source on an older build, which could authenticate its archive but sent no inventory version.
  delete message.progress.inventoryVersion;
  const legacy = link.readRelay(link.mailFor(a.entry, message, source.read(), true));
  assert.equal(legacy.kind, "deliver");
  relay.replay({ ...packet, payload: legacy.payload });
  const failure = await until("legacy export readiness refused", () => removalOf(a, source)?.error, t.signal);
  assert.match(failure, /Update the source computer to verify all attachments/);
  assert.equal(removalOf(a, source).generation, undefined);
  assert.equal(removalOf(a, source).export, undefined);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));

  // Model a primary upgrade after a grant was accepted under the old rules.
  const saved = await a.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
    import { updateAccess } from ${JSON.stringify(new URL("../src/link/setup/access.ts", import.meta.url).href)};
    import { awaitSharedSave } from ${JSON.stringify(new URL("../src/link/setup/shared.ts", import.meta.url).href)};
    const receipt = updateAccess(link => {
      const record = link.memberRemovals[${JSON.stringify(source.key)}];
      record.generation = ${JSON.stringify(message.progress.generation)};
      record.export = ${JSON.stringify(message.progress.export)};
      delete record.error;
      return link;
    }, { checkpoint: true }).save;
    console.log(JSON.stringify(await awaitSharedSave(receipt)));
  `]);
  assert.equal(saved.status, 0, saved.err);
  assert.equal(saved.value?.status, "committed");
  const withdrawn = await until("saved legacy grant withdrawn", () => {
    const record = removalOf(a, source);
    return record?.withdrawnGeneration && record;
  }, t.signal);
  assert.equal(withdrawn.generation, undefined);
  assert.equal(withdrawn.export, undefined);
  assert.equal(withdrawn.withdrawnGeneration, message.progress.generation);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.equal((await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true)).status, 409);
  const revision = await a.route("POST", wipeRoute(source), { operation: operationID(), supersedes: operation,
    prepare: { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } },
  }, true);
  assert.equal(revision.status, 409);

  // A completion for the same operation can have been in flight before upgrade.
  const done = link.readRelay(link.mailFor(a.entry, {
    setup: message.setup, done: operation, generation: message.progress.generation,
  }, source.read(), true));
  assert.equal(done.kind, "deliver");
  relay.replay({ ...packet, payload: done.payload });
  await until("previously issued wipe completion recorded", () => removalOf(a, source)?.delivered, t.signal);
  assert.equal(removalOf(a, source).error, undefined);
});

test("failure bench: a failed requested export revokes access and keeps source data", { timeout: 50_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "missing-export-folder") };
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(b), { operation, prepare: { export: { destination, recoveryKey } } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  const failure = await until("export error", () => removalOf(a, b)?.error, t.signal);
  assert.match(failure, /ENOENT/);
  const status = await a.route("GET", "kuvo/computers");
  assert.equal(status.value.wipePending?.find(pending => pending.operation === operation)?.error, failure);
  const bypass = await a.route("POST", wipeRoute(b), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  assert.equal(removalOf(a, b).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(b.key));
  assert.equal(b.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  fs.mkdirSync(destination.path);
  const retry = await a.route("POST", wipeRoute(b), { operation, retryPreparation: true }, true);
  assert.equal(retry.status, 200, JSON.stringify(retry.value));
  await until("retry reaches source", () => !preparation(b, "source", operation)?.error, t.signal);
  await waitForPreparedRemoval(a, b, operation, t.signal);
  await waitForWipe(b, t.signal);
});

test("failure bench: an unreachable source keeps a requested export and wipe pending until it returns", { timeout: 50_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "pending-export") };
  fs.mkdirSync(destination.path);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(b), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "exporting");
  assert.equal(removalOf(a, b).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(b.key));
  assert.ok(data.every(file => fs.existsSync(file)));
  await a.stop("SIGTERM");
  a.start();
  await until("primary restarted", () => relay.has(a.address), t.signal);
  const listed = await a.route("GET", "kuvo/computers");
  assert.deepEqual(listed.value.wipePending, [{ key: b.key, name: b.name, operation }]);
  b.start();
  await waitForPreparedRemoval(a, b, operation, t.signal);
  await waitForWipe(b, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.ok(fs.readdirSync(path.join(destination.path, "kuvo-backups")).length);
});

test("failure bench: an offline source’s accepted export preparation survives primary handover", { timeout: 60_000 }, async t => {
  const { a, b, c, relay, see } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "handover-export") };
  fs.mkdirSync(destination.path);
  const operation = operationID();
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  await b.stop();
  await until("source offline", () => !relay.has(b.address), t.signal);
  const start = await a.route("POST", wipeRoute(b), { operation, prepare }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  const membership = a.read().shared.membershipAndAccess.value;
  assert.equal(membership.removals[b.key].operation, operation);
  assert.equal(JSON.stringify(membership).includes(prepare.export.recoveryKey), false);
  const transfer = await submit(a, c, 1);
  assert.equal(transfer.status, 202, JSON.stringify(transfer.value));
  await see(c, 2, c);
  assert.equal(c.read().shared.membershipAndAccess.value.removals[b.key].operation, operation);
  const bypass = await c.route("POST", wipeRoute(b), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  assert.equal(removalOf(c, b).operation, operation);
  assert.ok(data.every(file => fs.existsSync(file)));
  const resumed = await c.route("POST", wipeRoute(b), { operation, prepare }, true);
  assert.equal(resumed.status, 200, JSON.stringify(resumed.value));
  b.start();
  await waitForPreparedRemoval(c, b, operation, t.signal);
  await waitForWipe(b, t.signal);
});

function removalMessage(packet, recipient, sender) {
  const envelope = link.readFrame(packet.payload);
  const prefix = Buffer.from("kuvo-removal-v1\0");
  if (envelope?.kind !== link.Frame.mail || !envelope.body.subarray(0, prefix.length).equals(prefix)) return;
  const body = envelope.body.subarray(prefix.length);
  const key = Buffer.from(sender.key, "base64url");
  if (!body.subarray(0, key.length).equals(key)) return;
  return JSON.parse(link.openOneWay(body.subarray(key.length), Buffer.from(recipient.identity.key, "base64url"), key).payload);
}

test("failure bench: move-only wipe waits for an explicitly revised, byte-verified queue plan", { timeout: 65_000 }, async t => {
  const group = { id: "grp_badfiles", name: "Files for removal", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, relay } = await network(t, false, undefined, false, group, true);
  const bytes = Buffer.of(0, 255, 1, 7);
  const { id: item, paths: [attachment] } = await uploadQueueFiles(c, { "payload.lock": bytes }, group.id);
  const source = path.join(c.home, attachment);
  const planRoute = `kuvo/computers/${c.key}/removal-plan`;
  for (const corrupted of [bytes.subarray(0, 1), Buffer.of(0, 255, 1, 8)]) {
    fs.writeFileSync(source, corrupted);
    const plan = await a.route("POST", planRoute, {}, true);
    assert.equal(plan.status, 200, JSON.stringify(plan.value));
    assert.equal(plan.value.plan.complete, false);
    assert.deepEqual(plan.value.plan.files.filter(file => file.status === "unavailable"), [{ item: group.id, name: "payload.lock", status: "unavailable" }]);
  }

  const operation = operationID();
  const prepare = { moveTo: a.key };
  assert.equal((await a.route("POST", wipeRoute(c), { operation, prepare }, true)).status, 200);
  const refused = await until("unavailable attachment blocks removal", () => {
    const kept = removalOf(a, c);
    return kept?.error && kept;
  }, t.signal);
  assert.deepEqual(refused.unresolved, [group.id]);
  assert.equal(refused.generation, undefined);
  assert.equal(c.read().localWipe, undefined);
  assert.equal(fs.existsSync(source), true);

  fs.writeFileSync(source, bytes);
  const revised = operationID();
  const challenges = new Set();
  const stopWatching = relay.intercept(c.address, a.address, packet => {
    const check = removalMessage(packet, a, c);
    if (check?.operation === revised && typeof check.moveFilesCheck === "string") challenges.add(check.moveFilesCheck);
    return false;
  });
  t.after(stopWatching);
  const accepted = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare }, true);
  assert.equal(accepted.status, 200, JSON.stringify(accepted.value));
  await waitForPreparedRemoval(a, c, revised, t.signal);
  await waitForWipe(c, t.signal);
  assert.equal(fs.existsSync(source), false);
  assert.ok(challenges.size >= 2, "readiness and the stopped wipe worker each checked the destination using a fresh nonce");

  const opened = await a.command(["box", "route"], JSON.stringify({ method: "GET", path: `kuvo/work-queue/items/${item}/files/payload.lock` }));
  assert.equal(opened.status, 0, opened.err);
  assert.equal(opened.value.status, 200, JSON.stringify(opened.value));
  assert.deepEqual(Buffer.from(opened.value.bodyBase64, "base64"), bytes);
});

test("removal preview discloses unselected payloads and missing retained artifacts", { timeout: 35_000 }, async t => {
  const group = { id: "grp_filepreview", name: "Preview", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c } = await network(t, false, undefined, false, group, true);
  const bytes = Buffer.of(0, 255, 9);
  const local = await c.route("POST", "kuvo/work-queue/items", { operation: "local-attachment-preview", title: "Local item",
    files: inlineFiles({ "local.lock": bytes }) }, true);
  assert.equal(local.status, 200, JSON.stringify(local.value));
  assert.equal(local.value.item.owner, "usr_you");
  put(path.join(c.home, "attachments", "legacy", "orphan.lock"), bytes);
  const hex = crypto.createHash("sha256").update(bytes).digest("hex");
  put(path.join(c.home, "artifacts", "log", "2026-11.jsonl"),
    JSON.stringify({ a: { id: "art_preview" }, v: { n: 1, kept: true, size: bytes.length, hash: `sha256:${hex}` } }) + "\n");
  const route = `kuvo/computers/${c.key}/removal-plan`;
  const preview = await a.route("POST", route, {}, true);
  assert.equal(preview.status, 200, JSON.stringify(preview.value));
  assert.equal(preview.value.plan.complete, false);
  assert.deepEqual(preview.value.plan.files.filter(file => file.status === "unavailable"),
    [{ item: "artifacts", name: `Retained artifact version ${hex.slice(0, 12)}`, status: "unavailable" }]);
  assert.ok(preview.value.plan.files.some(file => file.item === "usr_you" && file.status === "export-only"));
  assert.ok(preview.value.plan.files.some(file => file.name === "orphan.lock" && file.status === "export-only"));
  const copy = path.join(c.home, "artifacts", "copies", hex.slice(0, 2), hex);
  put(copy, Buffer.of(0, 255, 8));
  const changed = await a.route("POST", route, {}, true);
  assert.equal(changed.value.plan.complete, false);
  put(copy, bytes);
  const present = await a.route("POST", route, {}, true);
  assert.equal(present.value.plan.complete, true, JSON.stringify(present.value));
  assert.ok(present.value.plan.files.some(file => file.item === "artifacts" && file.status === "export-only"));
});

test("failure bench: an unreadable bot remains unresolved and blocks move-all deletion", { timeout: 35_000 }, async t => {
  const group = { id: "grp_unreadable", name: "Unresolved", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true);
  const data = seedLocalWork(c);
  const planned = await a.route("POST", `kuvo/computers/${c.key}/removal-plan`, {}, true);
  assert.equal(planned.status, 200);
  assert.equal(planned.value.plan.complete, false);
  assert.deepEqual(planned.value.plan.unresolved, ["bot_retained"]);
  const operation = operationID();
  assert.equal((await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key } }, true)).status, 200);
  const failure = await until("accepted unresolved work", () => removalOf(a, c)?.error && removalOf(a, c), t.signal);
  assert.equal(failure.generation, undefined);
  assert.deepEqual(failure.unresolved, ["bot_retained"]);
  assert.equal(c.read().localWipe, undefined);
  assert.equal(groupCopies.get("Lab")?.status, "active");
  assert.ok(data.every(file => fs.existsSync(file)));
  const cancelled = await a.route("POST", wipeRoute(c), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200);
  assert.deepEqual(cancelled.value.removal, { operation, state: "cancelled", accessRemoved: true });
  await until("source became standalone", () => c.read().setup?.role === "primary", t.signal);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
});

test("failure bench: a revised removal keeps completed moves and replaces a failed export", { timeout: 50_000 }, async t => {
  const group = { id: "grp_revised", name: "Keep this move", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true);
  const data = seedLocalWork(c, { botWork: false });
  const operation = operationID();
  const original = path.join(c.env.TMPDIR, "missing-disk");
  const replacement = fs.mkdtempSync(path.join(c.env.TMPDIR, "replacement-disk-"));
  const prepare = folder => ({ export: { destination: { kind: "disk", path: folder }, recoveryKey: crypto.randomBytes(32).toString("base64url") } });
  const first = await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key, ...prepare(original) } }, true);
  assert.equal(first.status, 200);
  await until("first recipe stalls after the transfer", () => removalOf(a, c)?.error, t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.deepEqual(preparation(c, "source", operation).moves, [{ id: group.id, kind: "group", completed: true }]);
  const revised = operationID();
  const second = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare: prepare(replacement) }, true);
  assert.equal(second.status, 200, JSON.stringify(second.value));
  const poll = await a.route("POST", wipeRoute(c), { operation });
  assert.equal(poll.value.removal.state, "cancelled");
  assert.deepEqual(await a.route("POST", wipeRoute(c), { save: first.value }), poll);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  await waitForWipe(c, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.equal(a.read().shared.ownershipAssignments.value["group host:" + group.id].computer, a.key);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(fs.existsSync(original), false);
  assert.ok(fs.readdirSync(path.join(replacement, "kuvo-backups")).length);
});

test("failure bench: a WAL checkpoint preserves an accepted wipe generation", { timeout: 40_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const database = path.join(b.home, "data", "opencode", "opencode.db");
  fs.mkdirSync(path.dirname(database), { recursive: true });
  const db = new DatabaseSync(database);
  let closed = false;
  t.after(() => { if (!closed) db.close(); });
  db.exec("PRAGMA journal_mode=WAL; CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('session');");
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(b.env.TMPDIR, "checkpoint-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  const release = relay.intercept(a.address, b.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, b, a)?.instruction);
    if (!instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation, prepare }, true)).status, 200);
  await until("wipe grant issued", () => grant, t.signal);
  const generation = removalOf(a, b).generation;
  assert.ok(generation);
  assert.equal(db.prepare("PRAGMA wal_checkpoint(TRUNCATE)").get().busy, 0);
  db.close();
  closed = true;
  assert.equal(fs.existsSync(database + "-wal"), false);
  release();
  await waitForWipe(b, t.signal);
  assert.equal(removalOf(a, b).generation, generation);
});

test("failure bench: source work added after readiness invalidates the wipe grant", { timeout: 45_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  seedLocalWork(b);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(b.env.TMPDIR, "generation-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  let hold = true;
  const release = relay.intercept(a.address, b.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, b, a)?.instruction);
    if (!hold || !instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation, prepare }, true)).status, 200);
  await until("wipe grant issued", () => grant, t.signal);
  const originalGeneration = removalOf(a, b).generation;
  assert.ok(originalGeneration);
  const added = path.join(b.home, "bots", "bot_retained", "after-ready.md");
  fs.writeFileSync(added, "new work");
  hold = false;
  relay.replay(grant);
  await until("generation invalidated", () => !removalOf(a, b)?.generation && removalOf(a, b)?.error, t.signal);
  assert.equal(b.read().localWipe, undefined);
  assert.equal(fs.readFileSync(added, "utf8"), "new work");
  const retry = await a.route("POST", wipeRoute(b), { operation, retryPreparation: true }, true);
  assert.equal(retry.status, 200);
  // This accepts the retry request. The source must prepare again before the wipe can start.
  await until("the replacement wipe generation is accepted", () => {
    const generation = removalOf(a, b)?.generation;
    return generation && generation !== originalGeneration;
  }, t.signal);
  await waitForWipe(b, t.signal);
});

for (const change of ["added", "changed"]) test(`failure bench: a session attachment ${change} after readiness invalidates the wipe grant`, { timeout: 45_000 }, async t => {
  const { a, c: owner, relay } = await network(t, false, undefined, true, undefined, true);
  const file = "yarn.lock";
  const [original] = await uploadSessionFiles(owner, "msg_originalfile", { [file]: Buffer.of(0, 255) });
  assert.ok(original);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(owner.env.TMPDIR, "attachment-generation-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  const release = relay.intercept(a.address, owner.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, owner, a)?.instruction);
    if (!instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  const started = await a.route("POST", wipeRoute(owner), { operation, prepare }, true);
  assert.equal(started.status, 200, JSON.stringify(started.value));
  await until("wipe grant issued", () => grant, t.signal);
  assert.ok(removalOf(a, owner).generation);
  const bytes = Buffer.of(255, 0);
  let changed = original;
  if (change === "added") {
    [changed] = await uploadSessionFiles(owner, "msg_latefile", { "later.tmp": bytes });
  } else {
    fs.writeFileSync(path.join(owner.home, original), bytes);
  }
  relay.replay(grant);
  await until("attachment change invalidated generation", () => !removalOf(a, owner)?.generation && removalOf(a, owner)?.error, t.signal);
  assert.equal(owner.read().localWipe, undefined);
  assert.deepEqual(fs.readFileSync(path.join(owner.home, changed)), bytes);
});

test("failure bench: a deleted member retries a lost completion from outside its folder", { timeout: 40_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const operation = operationID();
  let dropped = 0;
  const release = relay.intercept(null, a.address, packet => {
    if (removalMessage(packet, a, b)?.done !== operation) return false;
    dropped++;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation })).status, 200);
  await waitForWipe(b, t.signal);
  await until("completion dropped", () => dropped, t.signal);
  await delay(8_300);
  assert.equal(removalOf(a, b).delivered, undefined);
  const receipt = path.join(b.env.HOME, ".kuvo-code-removal-receipts", operation + ".json");
  const saved = JSON.parse(fs.readFileSync(receipt, "utf8"));
  assert.equal(saved.deleted, true);
  assert.equal(JSON.stringify(saved).includes(b.identity.key), false);
  assert.equal(JSON.stringify(saved).includes(b.identity.sign), false);
  release();
  await until("completion accepted", () => removalOf(a, b)?.delivered, t.signal);
  await until("completion acknowledged", () => !fs.existsSync(receipt), t.signal);
  assert.equal((await a.route("POST", wipeRoute(b), { operation })).value.removal.state, "wiped");
});

const miniLabels = Array.from({ length: 9 }, (_, index) => ({
  id: `${index < 4 ? "cat" : "lbl"}_work-${index}`, name: `Work ${index}`, color: index % 2 ? "sky" : "coral",
}));
const feedbackLabel = { id: "lbl_my-feedbacks", name: "My feedbacks", color: "violet" };
const allIncidentLabels = [...miniLabels, feedbackLabel];

function incidentCopies(boxes, duplicate = false) {
  const [, mini, lokum] = boxes;
  for (const [box, definitions, referenced] of [
    [mini, miniLabels, miniLabels],
    [lokum, duplicate ? [feedbackLabel, miniLabels[0]] : [feedbackLabel], [feedbackLabel]],
  ]) {
    const queue = path.join(box.run, "work-queue");
    fs.writeFileSync(path.join(queue, "labels.json"), JSON.stringify({ kuvo: 1, labels: definitions }));
    fs.mkdirSync(path.join(queue, "queues"));
    box.incidentItem = `wqi_${crypto.randomBytes(15).toString("base64url")}`;
    fs.writeFileSync(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      kuvo: 1, items: [{ id: box.incidentItem, number: box === lokum ? 849 : 713, title: "Existing work", status: "open", labels: referenced.map(label => label.id) }],
    }));
  }
}

async function assertIncidentResolved(box, item, labels) {
  const local = await box.route("GET", `kuvo/work-queue/items/${item}`);
  assert.equal(local.status, 200, JSON.stringify(local.value));
  assert.deepEqual(local.value.item.labels, labels.map(label => label.id));
  const library = await box.route("GET", "kuvo/work-queue/labels");
  assert.equal(library.status, 200, JSON.stringify(library.value));
  const definitions = new Map(library.value.labels.map(each => [each.id, each]));
  assert.deepEqual(local.value.item.labels.map(id => definitions.get(id)), labels);
}

test("failure bench: first standby unions the two members' label libraries with an empty primary", { timeout: 45_000 }, async t => {
  const miniSpend = { show: true, period: "day" };
  const lokumSpend = { show: false, period: "day" };
  const futureFields = Object.fromEntries(Array.from({ length: 20 }, (_, index) => [`futurePart${index}`, index]));
  const { a, b, c, see } = await network(t, false, undefined, false, undefined, false, false, undefined, {
    prepare: boxes => {
      incidentCopies(boxes, true);
      for (const [member, totalSpend, extra] of [
        [boxes[1], miniSpend, Object.fromEntries(Object.entries(futureFields).map(([key, value]) =>
          [key, { value, stamp: { v: 2, by: boxes[1].key } }]))],
        [boxes[2], lokumSpend, {
          codexFallback: { value: null, stamp: { v: 2, by: boxes[2].key } },
          futureMeter: { value: { visible: false }, stamp: { v: 2, by: boxes[2].key } },
          futureSort: { value: "original", stamp: { v: 2, by: boxes[2].key } },
        }],
      ]) {
        const location = path.join(member.home, "agents", "usage");
        fs.mkdirSync(location, { recursive: true });
        fs.writeFileSync(path.join(location, "settings.json"), JSON.stringify({
          kuvo: 1, units: { totalSpend: { value: totalSpend, stamp: { v: 1, by: member.key } }, ...extra },
        }));
      }
    },
  });
  assert.deepEqual((await a.route("GET", "kuvo/work-queue/labels")).value.labels, []);
  const selected = await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  assert.equal(selected.status, 202, JSON.stringify(selected.value));
  await Promise.all([see(a, 2, a), see(b, 2, a), see(c, 2, a)]);
  const seeded = checkpoint(a, 2).shared.queueLabels;
  assert.equal(seeded.sequence, 1);
  assert.deepEqual(seeded.value, { labels: allIncidentLabels, removed: [] });
  for (const box of [a, b, c]) assert.deepEqual(box.read().shared.queueLabels.value, seeded.value);
  await assertIncidentResolved(b, b.incidentItem, miniLabels);
  await assertIncidentResolved(c, c.incidentItem, [feedbackLabel]);
  const display = checkpoint(a, 2).shared.setupUsageDisplay;
  assert.equal(display.sequence, 1);
  assert.deepEqual(display.value.units.totalSpend.value, miniSpend);
  assert.equal(display.value.units.codexFallback.value, null);
  assert.deepEqual(display.value.units.futureMeter.value, { visible: false });
  assert.equal(display.value.units.futureSort.value, "original");
  for (const [key, value] of Object.entries(futureFields)) assert.equal(display.value.units[key].value, value);
  assert.deepEqual(display.offlineEdits.choices.map(({ field, accepted, proposed }) => ({ field, accepted, proposed })),
    [{ field: "totalSpend", accepted: miniSpend, proposed: lokumSpend }]);
  for (const box of [a, b, c]) assert.deepEqual(box.read().shared.setupUsageDisplay, display);

  const { codexFallback: _fallback, ...units } = display.value.units;
  const withoutFallback = { units: { ...units, futureSort: { value: "primary", stamp: { v: 3, by: a.key } } }, imported: false };
  const saved = await link.askSetupSave(a.run, { id: a.read().setup.id, epoch: 2, kind: "setupUsageDisplay",
    sequence: display.sequence, value: withoutFallback });
  assert.equal(saved.status, 200, JSON.stringify(saved));
  const preferences = path.join(c.home, "agents", "usage", "settings.json");
  const local = JSON.parse(fs.readFileSync(preferences, "utf8"));
  const notifications = { underTenPercent: true, healthyToClose: false, closeToRunningOut: true };
  local.units.notifications = { value: notifications, stamp: { v: 3, by: c.key } };
  local.units.futureMeter = { value: { visible: true }, stamp: { v: 3, by: c.key } };
  local.units.futureSort = { value: "member", stamp: { v: 3, by: c.key } };
  fs.writeFileSync(preferences, JSON.stringify(local));
  await until("the late usage edit is accepted", () => [a, b, c].every(box =>
    box.read().shared?.setupUsageDisplay?.value.units.notifications), t.signal);
  for (const box of [a, b, c]) {
    const accepted = box.read().shared.setupUsageDisplay;
    assert.equal(accepted.value.units.codexFallback, undefined);
    assert.deepEqual(accepted.value.units.notifications.value, notifications);
    assert.deepEqual(accepted.value.units.futureMeter, local.units.futureMeter);
    assert.equal(accepted.value.units.futureSort.value, "primary");
    for (const [key, value] of Object.entries(futureFields)) assert.equal(accepted.value.units[key].value, value);
    assert.deepEqual(accepted.offlineEdits.choices.slice(0, 1), display.offlineEdits.choices);
    assert.deepEqual(accepted.offlineEdits.choices.slice(1).map(({ field, accepted, proposed }) => ({ field, accepted, proposed })),
      [{ field: "futureSort", accepted: "primary", proposed: "member" }]);
  }
  const view = await editRead(a, { kind: "setupUsageDisplay" });
  assert.equal(view.status, 200, JSON.stringify(view.value));
  assert.equal(view.value.fields.futureSort, "primary");
  assert.equal(view.value.fields.futureMeter.visible, true);
  for (const [key, value] of Object.entries(futureFields)) assert.equal(view.value.fields[key], value);
  assert.deepEqual(view.value.choices, a.read().shared.setupUsageDisplay.offlineEdits.choices);
});

test("failure bench: an offline seed adds its original label when it rejoins", { timeout: 50_000 }, async t => {
  const unused = { id: "lbl_unused-on-lokum", name: "Unused on lokum", color: "amber" };
  const { a, b, c, see, relay } = await network(t, false, undefined, false, undefined, false, false, undefined, {
    prepare: boxes => {
      incidentCopies(boxes);
      fs.writeFileSync(path.join(boxes[2].run, "work-queue", "labels.json"), JSON.stringify({ kuvo: 1, labels: [feedbackLabel, unused] }));
    },
  });
  await c.stop("SIGTERM");
  await until("offline member disconnected", () => !relay.has(c.address), t.signal);
  const selected = await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  assert.equal(selected.status, 202, JSON.stringify(selected.value));
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  assert.deepEqual(checkpoint(a, 2).shared.queueLabels.value, { labels: miniLabels, removed: [] });
  c.start();
  await see(c, 2, a);
  await until("late label accepted by the standby and published", () => [a, b, c].every(box => box.read().shared?.queueLabels?.sequence === 2), t.signal);
  for (const box of [a, b, c]) assert.deepEqual(box.read().shared.queueLabels.value, { labels: [...allIncidentLabels, unused], removed: [] });
  await assertIncidentResolved(c, c.incidentItem, [feedbackLabel]);
  assert.equal(checkpoint(a, 2).shared.queueLabels.sequence, 1);
});

function writeLocalPersonalities(box, document) {
  const folder = path.join(box.home, "bots");
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, "personalities.json"), JSON.stringify({ kuvo: 1, ...document }));
}

test("failure bench: a reconnecting member's personality tombstone defeats a stale active seed", { timeout: 55_000 }, async t => {
  const active = personality(1);
  const tombstone = { id: active.id, removed: active.updated + 1 };
  const labelFile = box => path.join(box.run, "work-queue", "labels.json");
  const { a, b, c, see, relay } = await network(t, false, undefined, false, undefined, false, false, undefined, {
    prepare: ([primary]) => writeLocalPersonalities(primary, { personalities: [active], removed: [] }),
  });
  await c.stop("SIGTERM");
  await until("the offline member disconnected", () => !relay.has(c.address), t.signal);
  writeLocalPersonalities(c, { personalities: [], removed: [tombstone] });
  fs.writeFileSync(labelFile(c), JSON.stringify({ kuvo: 1, labels: [], removed: [feedbackLabel.id], removedSetup: c.read().setup.id }));
  fs.writeFileSync(labelFile(a), JSON.stringify({ kuvo: 1, labels: [feedbackLabel] }));

  const selected = await a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  assert.equal(selected.status, 202, JSON.stringify(selected.value));
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  assert.deepEqual(checkpoint(a, 2).shared.setupPersonalities.value.personalities.map(entry => entry.id), [active.id]);
  assert.deepEqual(checkpoint(a, 2).shared.queueLabels.value, { labels: [feedbackLabel], removed: [] });

  c.start();
  await see(c, 2, a);
  await until("the standby acknowledges both late tombstones", () => [a, b, c].every(box => {
    const shared = box.read().shared;
    return shared?.setupPersonalities?.value.removed.some(entry => entry.id === active.id)
      && shared?.queueLabels?.value.removed.includes(feedbackLabel.id);
  }), t.signal);
  for (const box of [a, b, c]) {
    assert.deepEqual(box.read().shared.setupPersonalities.value, { personalities: [], removed: [tombstone] });
    assert.deepEqual(box.read().shared.queueLabels.value, { labels: [], removed: [feedbackLabel.id] });
  }
  assert.equal(checkpoint(a, 2).shared.setupPersonalities.value.personalities[0].id, active.id);
});

test("failure bench: a member's local edit after gather is merged after standby acceptance", { timeout: 75_000 }, async t => {
  const { a, b, c, see, relay } = await network(t, false, undefined, false, undefined, false, false, undefined, {
    prepare: incidentCopies,
  });
  const added = { id: "lbl_edited-during-gather", name: "Edited during gather", color: "lime" };
  const recolored = { ...feedbackLabel, color: "orange" };
  let gathered;
  const release = relay.intercept(c.address, a.address, packet => {
    const labels = link.openMail(link.readFrame(packet.payload).body, a.read())?.payload.workQueue?.acted?.value?.copies?.queueLabels;
    if (labels?.labels.some(label => label.id === feedbackLabel.id)) {
      gathered = { packet, labels };
      return true;
    }
    return false;
  });
  t.after(release);
  const selecting = a.route("POST", "kuvo/setup/standby", { standby: b.key, epoch: 1 });
  selecting.catch(() => {});
  await until("the member has answered the gather", () => gathered, t.signal);
  assert.equal(c.read().setup.epoch, 1);
  assert.deepEqual(gathered.labels, { labels: [feedbackLabel], removed: [] });
  fs.writeFileSync(path.join(c.run, "work-queue", "labels.json"), JSON.stringify({ kuvo: 1, labels: [recolored, added] }));
  release();
  const offers = new Map();
  const unhold = relay.intercept(c.address, a.address, packet => {
    const seed = link.openMail(link.readFrame(packet.payload).body, a.read())?.payload.workQueue?.setupCopy;
    if (seed?.kind !== "queueLabels") return false;
    offers.set(seed.revision, packet);
    return true;
  });
  t.after(unhold);
  relay.replay(gathered.packet);

  const selected = await selecting;
  assert.equal(selected.status, 202, JSON.stringify(selected.value));
  await Promise.all([see(a, 2, a), see(b, 2, a), see(c, 2, a)]);
  const initial = checkpoint(a, 2).shared.queueLabels;
  assert.deepEqual(initial.value, { labels: allIncidentLabels, removed: [] });
  const stale = await until("the member proposes its local edit on the first revision", () => offers.get(initial.sequence), t.signal);
  const primaryEdit = await a.route("POST", "kuvo/work-queue/labels", { add: "Primary edit", color: "pink" }, true);
  assert.equal(primaryEdit.status, 200, JSON.stringify(primaryEdit.value));
  const next = initial.sequence + 1;
  relay.replay(stale);
  const fresh = await until("the member reoffers on the new revision", () => offers.get(next), t.signal);
  assert.equal(a.read().shared.queueLabels.sequence, next);
  assert.equal(b.read().shared.queueLabels.sequence, next);
  assert.equal(pendingOperation(a, "queueLabels"), undefined);
  unhold();
  relay.replay(fresh);
  await until("the newer local snapshot is acknowledged", () => [a, b, c].every(box =>
    box.read().shared?.queueLabels?.value.labels.some(label => label.id === added.id)), t.signal);
  for (const box of [a, b, c]) {
    const accepted = box.read().shared.queueLabels;
    assert.equal(accepted.sequence, next + 1);
    assert.deepEqual(accepted.value, { labels: [...miniLabels, recolored, primaryEdit.value.label, added], removed: [] });
    assert.deepEqual(accepted.offlineEdits.choices, []);
  }
});

for (const populated of [false, true]) test(
  `failure bench: retired pc5 Insights origin survives restore into a ${populated ? "populated" : "fresh"} index`,
  { timeout: 80_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const { a, b, c, boxes: [, , , replacement], relay } = await network(t, false, undefined, false, undefined,
      false, false, ["pc2", "pc4", "Original pc5", "Replacement"], { acceptedRoster: true });
    const backup = async (box, action, body) => {
      const answer = await box.route("POST", "kuvo/box/backups/" + action, body, true);
      assert.equal(answer.status, 200, JSON.stringify(answer.value));
      return answer.value;
    };
    const wait = (what, fn) => until(what, fn, AbortSignal.any([t.signal, AbortSignal.timeout(15_000)]));
    const index = box => path.join(box.home, "agents", "usage", "insights.sqlite");
    const query = (box, sql, ...params) => {
      const db = new DatabaseSync(index(box), { readOnly: true });
      try { return db.prepare(sql).get(...params); } finally { db.close(); }
    };
    const cursor = box => JSON.parse(query(box, "SELECT body FROM state WHERE key = ?", `cursor/${b.key}`)?.body ?? "0");
    const record = {
      key: crypto.createHash("sha256").update("pc5-provider-call").digest("hex"),
      computer: c.key, session: "ses_history", call: "provider-call", provider: "claude", attributed: true,
      account: "claude#aabbccddeeff0011", accountObserved: true,
      at: Date.now() - 1_000, tokens: { input: 100, output: 50, cacheRead: 0, cacheWrite5m: 0, cacheWrite1h: 0 },
    };
    const writeIndex = async (box, records, state = {}) => {
      fs.mkdirSync(path.dirname(index(box)), { recursive: true });
      const store = new URL("../src/usage/insights-store.ts", import.meta.url).href;
      const pricing = new URL("../src/usage/pricing.ts", import.meta.url).href;
      const result = await box.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
        import { InsightsStore } from ${JSON.stringify(store)};
        import { PricingStore } from ${JSON.stringify(pricing)};
        const db = new InsightsStore(${JSON.stringify(index(box))});
        db.take(${JSON.stringify(records)}, new PricingStore(undefined, null).current(), Date.now());
        for (const [key, value] of Object.entries(${JSON.stringify(state)})) db.write(key, value);
        db.close();
      `], "", false);
      assert.equal(result.status, 0, result.err);
    };
    const services = async box => {
      const child = box.launch(bun, ["--config=/dev/null", "--no-env-file", script, "box-services"]);
      const file = path.join(box.run, "box-services.json");
      const r = await wait(`${box.name} services`, () => {
        assert.equal(child.exitCode, null);
        const saved = fs.existsSync(file) && JSON.parse(fs.readFileSync(file));
        return saved?.pid === child.pid && saved;
      });
      return {
        async get(route) {
          const result = await fetch(`http://127.0.0.1:${r.port}/api/kuvo/${route}`, {
            headers: { authorization: "Basic " + Buffer.from(`opencode:${r.password}`).toString("base64") },
            signal: AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]),
          });
          assert.equal(result.status, 200, await result.clone().text());
          return (await result.json()).data;
        },
        async stop() {
          const exited = once(child, "exit");
          child.kill();
          await exited;
        },
      };
    };
    const overview = (box, filter = "") => box.get(`insights?period=7&zone=UTC${filter}`);
    const checkHistory = async box => {
      const data = await overview(box);
      assert.equal(data.totals.calls, 1);
      assert.equal(data.totals.tokens, 150);
      assert.deepEqual(data.computers.find(row => row.key === c.key), { key: c.key, name: c.name });
      assert.equal((await overview(box, `&computer=${c.key}`)).totals.calls, 1);
      assert.equal((await overview(box, `&computer=${replacement.key}`)).totals.calls, 0);
      assert.equal((await overview(box, `&account=${encodeURIComponent(record.account)}`)).totals.calls, 1);
      const detail = await box.get(`insights/sessions/${record.session}?period=7&zone=UTC`);
      assert.deepEqual(detail.calls.map(({ key, computer, call, account }) => ({ key, computer, call, account })),
        [{ key: record.key, computer: c.key, call: record.call, account: record.account }]);
      const sessions = await box.get("insights/sessions?period=7&zone=UTC");
      assert.equal(sessions.sessions.find(row => row.session === record.session).computer, c.key);
    };
    const opened = (packet, to) => {
      const frame = link.readFrame(packet.payload);
      return frame && link.openMail(frame.body, to.read())?.payload;
    };
    const sendPacket = (from, to, state, seat) => {
      const seatID = operationID();
      const wire = link.readRelay(link.mailFor(to.entry, { at: Date.now(), seatID, seat }, state));
      return { from: from.address, to: to.address, payload: wire.payload, seatID };
    };
    const deliver = async (packet, from, to) => {
      const start = relay.delivered.length;
      relay.replay(packet);
      await wait("authenticated services page applied", () => relay.delivered.slice(start).some(reply =>
        reply.from.equals(to.address) && reply.to.equals(from.address) &&
          opened(reply, from)?.delivered?.id === packet.seatID && opened(reply, from)?.delivered?.ok));
    };
    const unrelated = { ...record, key: "f".repeat(64), computer: link.publicOf(link.newKey()).toString("base64url"),
      at: record.at + 1, supersedes: record.key };
    const usageSource = new URL("../src/usage/usage.ts", import.meta.url).href;
    const alias = await c.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
      import { ownComputer } from ${JSON.stringify(usageSource)};
      import { createHash } from "node:crypto";
      console.log(createHash("sha256").update(JSON.stringify(["kuvo insight computer", ownComputer()])).digest("hex").slice(0, 32));
    `], "", false);
    assert.equal(alias.status, 0, alias.err);
    assert.match(alias.out, /^[a-f0-9]{32}$/);
    const clonedAlias = { ...unrelated, key: "e".repeat(64), computer: alias.out };
    const legacy = { ...record, attributed: undefined };
    await writeIndex(c, [legacy]);
    const old = await services(c);
    const surviving = await services(b);
    await wait("pc4 synced the call from pc5", async () => (await overview(surviving)).totals.calls === 1);
    assert.equal(query(b, "SELECT sender, seq FROM event WHERE key = ?", record.key).seq, null);
    await old.stop();
    await writeIndex(c, [clonedAlias]);
    const cloneSession = "ses_clone_alias";
    const savedSessions = new URL("../src/agents/store.ts", import.meta.url).href;
    const saved = await c.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
      import { Store } from ${JSON.stringify(savedSessions)};
      const store = new Store(${JSON.stringify(path.join(c.home, "agents", "sessions"))});
      store.addWhole({
        session: { id: ${JSON.stringify(cloneSession)}, projectID: "p", title: "Unbound clone call", cost: 0,
          time: { created: ${record.at}, updated: ${record.at} }, location: { directory: "/tmp" },
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } } },
        messages: [{ id: "cloned-call", type: "assistant", model: { providerID: "claude-code", id: "test" },
          agent: "build", content: [], time: { created: ${record.at}, completed: ${record.at}, origin: ${JSON.stringify(alias.out)} } }],
        agent: {},
      });
    `], "", false);
    assert.equal(saved.status, 0, saved.err);
    const skipped = { [`cursor/${b.key}`]: 90, [`peer-epoch/${b.key}`]: "legacy" };
    await writeIndex(a, [], skipped);
    if (populated) await writeIndex(replacement, [unrelated, { ...record, computer: unrelated.computer }], skipped);
    else assert.equal(fs.existsSync(index(replacement)), false);

    const root = a.env.TMPDIR;
    const destination = { kind: "disk", path: path.join(root, "archive") };
    fs.mkdirSync(destination.path);
    const source = "b_" + operationID();
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    put(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
      version: 1, enabled: true, source, key: recoveryKey, dest: destination,
      disk: fs.statSync(destination.path).dev, projects: [],
    }));
    const { point } = await backup(c, "now");
    const oldCredential = c.read();
    await c.stop();
    fs.rmSync(c.home, { recursive: true });
    assert.equal(fs.existsSync(c.home), false);
    const removed = await a.route("DELETE", `kuvo/computers/${c.key}`);
    assert.ok([200, 202].includes(removed.status), JSON.stringify(removed.value));
    await wait("pc5 is no longer a current peer", () => link.seatPeers(replacement.read(), {}).boxes.every(peer => peer.key !== c.key));
    const oldPacket = sendPacket(c, replacement, oldCredential, { kind: "insights", action: "page", after: 0, next: 999,
      epoch: "retired", more: false, records: [{ ...record, key: "a".repeat(64) }] });
    assert.equal(opened(oldPacket, replacement), undefined, "the old transport credential is still revoked");
    relay.replay(oldPacket);
    await backup(a, "match", { source, previous: c.key, replacement: replacement.key });
    await wait("replacement and survivor have accepted the restore match and historical provenance", () => [replacement, b].every(box => {
      const state = box.read();
      return state.shared?.ownershipAssignments?.value?.[`replacement:${c.key}`]?.computer === replacement.key &&
        link.seatPeers(state, {}).insightOrigins?.some(origin => origin.key === c.key);
    }));
    const restored = await backup(replacement, "restore", { destination, source, id: point.id, recoveryKey, confirm: true });
    assert.equal(restored.unavailable.some(entry => entry.entry === "agents/usage/insights.sqlite"), false);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", record.key).computer, c.key);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", clonedAlias.key), undefined);
    if (populated) assert.equal(cursor(replacement), 90, "restore preserves the live index's wire cursor for scoped rewind");

    await checkHistory(surviving);
    const recovering = await services(a);
    await wait("pc2 backfilled its skipped survivor page", async () => (await overview(recovering)).totals.calls === 1);
    assert.notEqual(cursor(a), 90);
    await checkHistory(recovering);

    const unintercept = relay.intercept(b.address, replacement.address, packet => opened(packet, replacement)?.seat?.kind === "insights");
    let recovered = await services(replacement);
    await wait("replacement services knows retired pc5", async () => (await overview(recovered)).computers.some(row => row.key === c.key));
    await wait("scanner reconsidered the restored clone transcript", () =>
      query(replacement, "SELECT key FROM event WHERE session = ? AND computer = ?", cloneSession, alias.out));
    assert.equal((await overview(recovered, `&session=${cloneSession}`)).totals.calls, 0);
    await checkHistory(recovered);
    const page = (after, next) => sendPacket(b, replacement, b.read(), { kind: "insights", action: "page", after, next,
      epoch: "replay", more: false, records: [record, unrelated] });
    const first = page(0, 1), second = page(1, 2);
    for (const packet of populated ? [second, first, second, first] : [first, second, second, first]) {
      await deliver(packet, b, replacement);
    }
    assert.equal(cursor(replacement), 2);
    assert.equal(query(replacement, "SELECT body FROM state WHERE key = ?", `cursor/${c.key}`), undefined);
    if (!populated) assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", unrelated.key), undefined);
    await checkHistory(recovered);

    const checked = Date.now() - 1_000;
    const limit = (used, read) => ({ kind: "limits", at: Date.now(), accounts: [{ provider: "claude-code", service: "claude",
      identity: "aabbccddeeff0011", checked: read, windows: [{ id: "five-hour", used, read, minutes: 300 }] }] });
    await deliver(sendPacket(a, replacement, a.read(), limit(0.2, checked - 1000)), a, replacement);
    await deliver(sendPacket(b, replacement, b.read(), limit(0.6, checked)), b, replacement);
    relay.replay(sendPacket(c, replacement, oldCredential, limit(0.9, checked + 500)));
    const checkLimits = async () => {
      const row = (await recovered.get("limits")).rows.find(row => row.id === record.account);
      assert.equal(row.usage.updatedAt, checked);
      assert.equal(row.usage.meters[0].share, 0.6);
      assert.deepEqual(row.usage.computers, [{ name: b.name, here: false }]);
      assert.ok(row.boxes.every(box => box.key !== c.key));
    };
    await checkLimits();
    await recovered.stop();
    recovered = await services(replacement);
    await wait("restarted replacement services knows the accepted history", async () =>
      (await overview(recovered)).computers.some(row => row.key === c.key));
    assert.equal(cursor(replacement), 2);
    await checkHistory(recovered);
    await checkLimits();
    unintercept();
  });

test("failure bench: a mixed-build update cut and lost move reply survive disk loss, restore, and replay",
  { timeout: 210_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const session = "ses_owned";
    const recovery = { build: recoveryNeighborBuild(), session };
    const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
    const pin = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
    const { a, b, c, relay, see, fixtureEngine, phase } = await network(t, true, pin, false, undefined, false, false,
      ["pc2", "pc4", "pc5"], { recovery });
    const root = a.env.TMPDIR;
    const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
    const backup = (box, action, body) => box.route("POST", "kuvo/box/backups/" + action, body, true);
    const good = (result, status = 200) => {
      assert.equal(result.status, status, JSON.stringify(result.value));
      return result.value;
    };
    const mail = (packet, recipient, saved = recipient.read()) => {
      const frame = link.readFrame(packet.payload);
      return frame && link.openMail(frame.body, saved)?.payload;
    };
    const name = "Recovered with receipt";
    await see(a, 2, a);
    await see(b, 2, a);
    await see(c, 2, a);
    await until("pc2 holds its lease", () => a.lease(2, b), t.signal);
    phase("epoch 2 leased to the primary with the neighboring build enrolled on pc5");

    // Save the receipt on the client side, outside the disk about to be lost.
    const saved = good(await rename(a, c, name));
    assertReceipt(a, "setupNames", "committed", saved);
    assert.equal(b.read().sharedOutcomes?.[saved.operation]?.status, "committed");
    const receiptFile = path.join(root, "client-receipt.json");
    put(receiptFile, JSON.stringify(saved));
    const bytes = Buffer.of(0, 255, 3, 128);
    const { id: item, paths: [attachment] } = await uploadQueueFiles(a, { "recovery.lock": bytes });
    const deleted = editAddress(a, itemID());
    const created = good(await editSubmit(a, deleted, {
      operation: "create-before-delete", create: true, baseRevision: null, fields: { title: "Delete this item" },
    }));
    const baseRevision = created.revision;
    assert.ok(baseRevision);
    assert.equal(good(await editSubmit(a, deleted, {
      operation: "delete-before-move", baseRevision, delete: true,
    })).status, "committed");
    const wake = `wakes/${session}/wakes/1-abcd`;
    put(path.join(a.home, wake), "One recovered wake");
    put(path.join(a.home, "wakes", session, "session"), session);
    const savedConversation = `agents/sessions/${session}/messages.json`;
    put(path.join(a.home, savedConversation), JSON.stringify(fixtureEngine.messages));
    const [retained] = seedLocalWork(a);
    const retainedFile = path.relative(a.home, retained);
    fs.writeFileSync(retained, Buffer.of(253, 0, 56));

    const usageProcesses = new Map();
    const service = async box => {
      const child = box.launch(bun, ["--config=/dev/null", "--no-env-file", script, "box-services"]);
      usageProcesses.set(box, child);
      const file = path.join(box.run, "box-services.json");
      const record = await until(`${box.name} Box services`, () => {
        assert.equal(child.exitCode, null, `${box.name} Box services exited`);
        return fs.existsSync(file) && read(file).pid === child.pid && read(file);
      }, t.signal);
      const ask = async (route, method = "GET") => {
        const response = await fetch(`http://127.0.0.1:${record.port}/api/kuvo/insights${route}`, {
          method, headers: { authorization: "Basic " + Buffer.from(`opencode:${record.password}`).toString("base64") },
          signal: AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]),
        });
        assert.equal(response.status, method === "GET" ? 200 : 202, await response.clone().text());
        return (await response.json()).data;
      };
      await until(`${box.name} service learned the fleet roster`, async () =>
        (await ask("?period=7")).computers.some(({ key }) => key === b.key), t.signal);
      return ask;
    };
    const callID = "one-provider-call";
    const call = {
      key: crypto.createHash("sha256").update(JSON.stringify(["claude", session, callID])).digest("hex"),
      at: Date.now(), session, computer: b.key, call: callID, provider: "claude", model: "claude-test",
      tokens: { input: 100, output: 50, cacheRead: 0, cacheWrite5m: 0, cacheWrite1h: 0 },
    };
    // Inject the provider's normalized observation at the peer boundary.
    // The enrolled sender's real wire format, Relay ingress, services reader
    // and persistent Insights index are used for both deliveries.
    const insightsPage = (to, after, next, epoch, reset = false) => {
      const seatID = operationID();
      const wire = link.readRelay(link.mailFor(to.entry, {
        at: Date.now(), seatID, seat: { kind: "insights", action: "page", after, next, epoch, reset, more: false, records: [call] },
      }, b.read()));
      return { from: b.address, to: to.address, payload: wire.payload, seatID };
    };
    const sourceInsights = await service(a);
    relay.replay(insightsPage(a, 0, 1, "original"));
    const sourceUsage = await until("pc2's Insights index receives pc4's call", async () => {
      const data = await sourceInsights("?period=7&zone=UTC");
      return data.totals.calls > 0 && data;
    }, AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]));
    assert.deepEqual([sourceUsage.totals.calls, sourceUsage.totals.tokens], [1, 150]);
    phase("one Insights call replicated from pc4 to pc2");
    const numbered = async (box, id) => until("number assigned", async () => {
      const read = await box.route("GET", `kuvo/work-queue/items/${id}`);
      const number = read.value.item?.number;
      return Number.isInteger(number) && number;
    }, t.signal);
    const make = async (box, operation) => good(await box.route("POST", "kuvo/work-queue/items", {
      operation, title: operation,
    }, true)).item;
    const itemNumber = await numbered(a, item);
    const first = await make(c, "before-partition");
    const firstNumber = await numbered(c, first.id);
    const grant = Object.values(b.read().shared.queueNumberAllocation.value.given).find(grant => grant.by === c.key);
    assert.ok(grant && grant.from <= firstNumber && firstNumber < grant.to);
    const unpause = [a, b].flatMap(box => [relay.pause(c.address, box.address), relay.pause(box.address, c.address)]);
    const offline = await make(c, "numbered-offline");
    assert.equal(offline.number, firstNumber + 1);
    assert.equal((await make(c, "numbered-offline")).id, offline.id);
    phase("number grant replicated and consumed once on the partitioned computer");

    // A separate disposable folder models external disk media without mounting a volume.
    const destination = { kind: "disk", path: fs.realpathSync(root) + "/archive" };
    fs.mkdirSync(destination.path);
    const source = "b_" + operationID();
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    put(path.join(a.home, "backup", "configuration.json"), JSON.stringify({
      version: 1, enabled: true, source, key: recoveryKey, dest: destination,
      disk: fs.statSync(destination.path).dev, projects: [],
    }));
    const { point } = good(await backup(a, "now"));
    assert.ok(point?.id);
    phase("verified encrypted recovery point published outside pc2");

    const sender = a.read();
    const inbound = new Map();
    let arrived, switchedReplies = 0;
    const holdArrival = relay.intercept(b.address, a.address, packet => {
      const move = mail(packet, a, sender)?.move;
      if (move?.op === "arrived") { arrived ??= packet; return true; }
      if (move?.op === "switched") { switchedReplies++; return true; }
      return false;
    });
    const recordPackets = relay.intercept(a.address, b.address, packet => {
      const move = mail(packet, b)?.move;
      if (move?.op !== "part" && move?.op !== "switch") return false;
      if (!inbound.has(move.op)) inbound.set(move.op, packet);
      // pc4 activates from the accepted assignment. Every switch delivered
      // after that comes from the replay harness, so its answers are causal.
      return move.op === "switch";
    });
    const { move } = good(await a.route("POST", "kuvo/moves", { kind: "chat", id: session, to: b.key, receipt: true }, true), 202);
    await until("pc4's staged move reply", () => arrived, t.signal);
    const staged = path.join(b.run, "moves", move.id + ".json");
    assert.equal(read(staged).complete, true);
    assert.match(fs.readFileSync(path.join(b.home, "wakes", session, "paused"), "utf8"), /moving/);
    phase("pc4 durably staged the incoming conversation and pc2 has not seen its arrival");

    const fire = async box => {
      const wakeRunner = fileURLToPath(new URL("../src/keeper/kuvo-wake", import.meta.url));
      const result = await box.evaluate("sh", [wakeRunner, "fire", path.join(box.home, "bots"), path.join(box.run, "engine.json")], "", false);
      assert.equal(result.status, 0, result.err);
    };
    await fire(a);
    await fire(b);
    assert.deepEqual(fixtureEngine.wakes, [], "neither staged copy may deliver before transfer");

    const release = benchRelease(root, script, privateKey, pin);
    put(path.join(a.home, "bin", "kuvo-link.js"), fs.readFileSync(script));
    put(path.join(a.home, "bin", "box-version"), "0.8.0");
    put(path.join(a.home, "bin", "kuvo-start"), "#!/bin/sh\nexit 0\n");
    const runner = path.join(a.home, "engine", release.engine, "opencode");
    put(runner, `#!/bin/sh\nBUN_BE_BUN=1 exec '${bun.replaceAll("'", "'\\''")}' "$@"\n`);
    fs.chmodSync(runner, 0o700);
    put(path.join(a.run, "engine.json"), JSON.stringify({ ...read(path.join(a.run, "engine.json")), version: release.engine }));
    const trial = path.join(a.run, "update-trial");
    const cut = `
      import fs from "node:fs";
      const rename = fs.renameSync;
      fs.renameSync = (from, to) => {
        rename(from, to);
        if (to === ${JSON.stringify(trial)}) {
          const fd = fs.openSync(to, "r");
          fs.fsyncSync(fd);
          fs.closeSync(fd);
          const dir = fs.openSync(${JSON.stringify(a.run)}, "r");
          fs.fsyncSync(dir);
          fs.closeSync(dir);
          process.kill(process.pid, "SIGKILL");
        }
      };
    `;
    const interrupted = await a.evaluate(process.execPath, [
      "--import", `data:text/javascript,${encodeURIComponent(cut)}`,
      script, "update", "--now", "--from", release.folder, "--key", release.key,
    ]);
    assert.equal(interrupted.signal, "SIGKILL", JSON.stringify(interrupted));
    assert.equal(fs.readFileSync(trial, "utf8").trim(), release.version);
    assert.equal(read(path.join(a.run, "update.json")).trial.installing, true);
    const updatePoint = read(path.join(a.home, "backup", "configuration.json")).last.id;
    assert.match(updatePoint, /-update-/);
    assert.ok(fs.existsSync(path.join(destination.path, "kuvo-backups", source, updatePoint + ".kuvo")));
    assert.equal(fs.readFileSync(path.join(a.home, "bin", "box-version"), "utf8"), "0.8.0");
    assert.equal(read(staged).complete, true);
    phase("signed updater killed after its durable trial marker, with the move still held");

    relay.replay(arrived);
    await until("pc2 sent its switch and pc4's activation reply was dropped",
      () => inbound.has("switch") && switchedReplies > 0, t.signal);
    const decisionFile = path.join(b.run, "moves", "decisions", move.id + ".json");
    const decision = read(decisionFile);
    assert.equal(decision.as, "switched");
    const assignment = state => state.shared?.ownershipAssignments?.value?.[`session computer:${session}`];
    assert.deepEqual(assignment(b.read()), { computer: b.key, move: move.id, from: a.key });
    assert.equal(read(path.join(a.run, "moves", move.id + ".json")).stage, "switching");
    assert.ok(fs.existsSync(fixtureEngine.file(a.name)));
    await fire(a);
    assert.deepEqual(fixtureEngine.wakes, [], "the stale source remains held after it loses ownership");
    phase("pc4 committed ownership and activated once; its switch reply was dropped");

    // While the old sender is still enrolled, exercise the destination's decision
    // on both orders of the original bundle packets. Replay them after revocation too.
    const bundle = [inbound.get("part"), inbound.get("switch")];
    assert.ok(bundle.every(Boolean));
    const orders = [bundle, [...bundle].reverse()];
    for (const order of orders) {
      const replies = switchedReplies;
      order.forEach(relay.replay);
      await until("pc4 answers a replay from its saved move decision", () => switchedReplies > replies, t.signal);
    }

    const stopService = async box => {
      const services = usageProcesses.get(box);
      const stopped = once(services, "exit");
      services.kill();
      await stopped;
    };
    await stopService(a);
    await a.stop();
    fs.rmSync(a.home, { recursive: true });
    assert.equal(fs.existsSync(a.home), false);
    assert.equal(fs.existsSync(path.join(destination.path, "kuvo-backups", source, point.id + ".kuvo")), true);
    phase("pc2 disk erased with its update and move journals unresolved");
    b.clock(TAKEOVER + 5_000);
    await see(b, 3, b);
    const outcome = await link.askSetupOutcome(b.run, read(receiptFile), "setupNames");
    assert.equal(outcome.status, 200, JSON.stringify(outcome));
    assert.equal(outcome.outcome, "committed", JSON.stringify(outcome));
    assert.equal(outcome.receipt.status, "committed");
    assert.equal(outcome.receipt.operation, saved.operation);
    assert.equal(await setupName(b, c.key), name);
    phase("pc4 elected and the client receipt resolved from its durable replica");

    unpause.forEach(resume => resume());
    c.clock(TAKEOVER + 5_000);
    await until("pc5 follows the replacement primary and grants its lease", () =>
      c.read().setup.primary === b.key && b.lease(b.read().setup.epoch, c), t.signal);
    good(await b.route("DELETE", `kuvo/computers/${a.key}`));
    const restore = { destination, source, id: point.id, recoveryKey, confirm: true };
    assert.match(good(await backup(c, "restore", restore), 409).message, /Confirm the replacement on the main computer/);
    good(await backup(b, "match", { source, previous: a.key, replacement: c.key }));
    await until("pc5 sees its approved replacement mapping", () => c.read().shared?.ownershipAssignments?.value?.["replacement:" + a.key], t.signal);
    const restored = good(await backup(c, "restore", restore));
    assert.ok(restored.unavailable.some(entry => entry.entry === item && entry.reason === "moved or transfer needs review"),
      JSON.stringify(restored.unavailable));
    assert.equal(restored.unavailable.some(entry => entry.entry === "agents/usage/insights.sqlite"), false);
    assert.deepEqual(fs.readFileSync(path.join(c.home, retainedFile)), Buffer.of(253, 0, 56));
    assert.ok(restored.unavailable.some(entry => entry.entry === savedConversation && entry.reason === "moved"));
    assert.equal(fs.existsSync(path.join(c.home, savedConversation)), false);
    assert.equal(fs.existsSync(path.join(c.home, wake)), false);
    assert.equal(fs.existsSync(path.join(c.home, attachment)), false);
    phase("pc5 restored retained bytes and fenced the stale copy of pc4's conversation");

    const replacementInsights = await service(c);
    const overview = () => replacementInsights("?period=7&zone=UTC");
    const restoredUsage = await overview();
    assert.deepEqual([restoredUsage.totals.calls, restoredUsage.totals.tokens], [1, 150]);
    // A reindexed peer resends the call already held in the restored index.
    // Let each delivery reach the reader before checking its durable cursor.
    const deliverPage = async packet => {
      const start = relay.delivered.length;
      relay.replay(packet);
      await until("pc5 processed the Insights page", () => relay.delivered.slice(start).some(reply =>
        reply.from.equals(c.address) && reply.to.equals(b.address) &&
        mail(reply, b)?.delivered?.id === packet.seatID && mail(reply, b)?.delivered?.ok === true), t.signal);
    };
    const index = path.join(c.home, "agents", "usage", "insights.sqlite");
    const usageCursor = () => {
      const db = new DatabaseSync(index, { readOnly: true });
      try {
        return JSON.parse(db.prepare("SELECT body FROM state WHERE key = ?").get(`cursor/${b.key}`)?.body ?? "null");
      } finally {
        db.close();
      }
    };
    const reset = insightsPage(c, 1, 2, "reindexed", true);
    const following = insightsPage(c, 2, 3, "reindexed");
    await deliverPage(following);
    assert.equal(usageCursor(), 1);
    await deliverPage(reset);
    assert.equal(usageCursor(), 2);
    await deliverPage(reset);
    await deliverPage(following);
    assert.equal(usageCursor(), 3);
    const usage = await overview();
    assert.deepEqual([usage.totals.calls, usage.totals.tokens], [1, 150]);
    const usageDetail = await replacementInsights(`?period=7&zone=UTC&session=${session}`);
    assert.deepEqual(usageDetail.calls.map(({ call, computer }) => [call, computer]), [[callID, b.key]]);
    phase("restored Insights index counts the peer call once after duplicate delivery");

    const kept = good(await b.route("GET", `kuvo/work-queue/items/${item}`)).item;
    assert.equal(kept.owner, session);
    assert.equal(kept.number, itemNumber);
    assert.deepEqual(fs.readFileSync(path.join(b.home, attachment)), bytes);
    assert.deepEqual(read(fixtureEngine.file(b.name)).messages, fixtureEngine.messages);
    const next = await make(b, "numbered-on-successor");
    const nextNumber = await numbered(b, next.id);
    assert.ok(nextNumber > grant.to);
    const replayedCreate = await make(c, "numbered-offline");
    assert.equal(replayedCreate.id, offline.id);
    assert.equal(replayedCreate.number, offline.number);
    assert.equal(new Set([itemNumber, firstNumber, offline.number, nextNumber]).size, 4);

    const lateRef = editAddress(b, deleted.id);
    const late = { operation: "old-edit", baseRevision, fields: { body: "Late text" } };
    assert.equal(good(await editSubmit(b, lateRef, late), 409).status, "deleted");
    assert.equal(good(await editSubmit(b, lateRef, late), 409).status, "deleted");
    assert.equal(good(await editRead(b, lateRef)).deleted, true);
    const tombstone = good(await b.route("GET", `kuvo/work-queue/items/${deleted.id}`)).item;
    assert.ok(tombstone.removed?.at);
    assert.equal(tombstone.body, "");
    assert.equal(tombstone.events.filter(event => event.kind === "deleted").length, 1);

    await fire(c);
    await fire(b);
    recordPackets();
    holdArrival();
    for (const order of orders) order.forEach(relay.replay);
    await fire(b);
    await fire(c);
    assert.deepEqual(fixtureEngine.imports, [b.name]);
    assert.deepEqual(fixtureEngine.wakes.map(({ box, text }) => [box, text]), [[b.name, "[Wake-up] One recovered wake"]]);
    assert.deepEqual(read(decisionFile), decision);
    assert.equal(assignment(b.read()).computer, b.key);
    phase("replayed move and edits kept one import, one wake, the deletion, and distinct numbers");
  });
