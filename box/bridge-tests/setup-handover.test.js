import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
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
import { CODEX_STAND_IN } from "./codex-stand-in.js";
import { relayStandIn } from "./relay-stand-in.js";
import { runningAs } from "./running-as.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { recoveryNeighborBuild, splitLaunchBuild } from "./failure-bench-legacy.js";
import { benchRelease, durablePhase, put, recoveryEngine } from "./recovery-bench.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const TAKEOVER = 180_000;
const submit = (box, primary, epoch) => box.route("POST", "kuvo/setup/handover", { primary: primary.key, epoch });

function peerOwns(box, owner, id) {
  try {
    const owns = JSON.parse(fs.readFileSync(path.join(box.run, "peer-agents.json"), "utf8"))[owner.key]?.owns ?? "";
    const digest = crypto.createHash("sha256").update(id).digest("base64url").slice(0, 6);
    return owns.match(/.{6}/g)?.includes(digest) ?? false;
  } catch { return false; }
}

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
  { beta35Owner = false, macPrimary = false, appOwner = false, acceptedRoster = false, oldPrimary = false, engineModels, prepare, recovery, phaseDiagnostics = false, liveAsks = false, leaseRelay = relayLeaseStandIn({ transferStartsNow: true }), oldMember = false, resident = false, splitIndex } = {}) {
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
  const splitScript = path.join(root, "pre-resident.mjs");
  if (splitIndex !== undefined) fs.writeFileSync(splitScript, splitLaunchBuild());
  if (releaseKey) {
    const source = fs.readFileSync(script, "utf8");
    assert.ok(source.includes(link.UPDATE_KEYS[0]));
    fs.writeFileSync(boxScript, source.replace(link.UPDATE_KEYS[0], releaseKey));
  }
  const oldScript = path.join(root, "old", "kuvo-link.js");
  if (oldPrimary || oldMember) {
    const source = fs.readFileSync(boxScript, "utf8");
    const old = source.replace(/^ +"work-queue\.(?:forward\.memory|labels\.restore\.preview)",\n/gm, "");
    assert.notEqual(old, source, "the old box must omit the new forwarding features");
    fs.mkdirSync(path.dirname(oldScript));
    fs.writeFileSync(oldScript, old);
  }
  const relay = await relayStandIn(link, { request: leaseRelay.request });
  let labelCall;
  let macIdentity;
  const hostPassword = crypto.randomBytes(16).toString("hex");
  const prompts = [];
  const groupMessages = [];
  const bridgePID = owned || movingGroup ? await runningAs("kuvo-agents", t) : null;
  // The real bridge persists groups here, and the link uses those files to announce ownership.
  const groupCopies = new Map();
  const groupFolder = (box, id) => path.join(box.appFolder ?? box.home, "groups", id);
  const saveGroup = (box, chat) => {
    groupCopies.set(box.name, chat);
    put(path.join(groupFolder(box, chat.group.id), "group.json"), JSON.stringify({ kuvo: 1, status: chat.status, group: chat.group }));
    put(path.join(groupFolder(box, chat.group.id), "messages.json"), JSON.stringify(chat.messages));
  };
  let ownedConversationPresent = true;
  let ownedReadFailure = 0;
  let ownedLookupRefused = false;
  let extraMessages = [];
  const ownedEvents = new Set();
  const tellOwned = event => {
    for (const stream of ownedEvents) stream.write(`data: ${JSON.stringify(event)}\n\n`);
  };
  const permissionAsks = new Map();
  const permissionEffects = [];
  const handleRuntime = (request, response) => {
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
      const computer = boxes.find(each => each.name === box);
      if (!computer) return response.writeHead(404).end("{}");
      const chat = groupCopies.get(box);
      const send = (status, data) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify({ data }));
      if (!groupPath[1]) return send(200, chat ? [chat] : []);
      if (request.method === "PUT") {
        const parts = [];
        request.on("data", part => parts.push(part));
        request.on("end", () => { saveGroup(computer, JSON.parse(Buffer.concat(parts))); send(200, true); });
        return;
      }
      if (!chat) return send(404, null);
      if (groupPath[2] === "hold") saveGroup(computer, { ...chat, status: "held" });
      if (groupPath[2] === "activate") saveGroup(computer, { ...chat, status: "active" });
      if (groupPath[2] === "release") {
        fs.rmSync(groupFolder(computer, chat.group.id), { recursive: true, force: true });
        groupCopies.delete(box);
      }
      return send(200, groupCopies.get(box) ?? chat);
    }
    // The bridge lists its running group chats to its link (`/api/kuvo/link/groups`), which
    // tells the other computers which groups live here.
    if (movingGroup && request.url === "/api/kuvo/link/groups") {
      const box = Buffer.from(request.headers.authorization?.slice(6) ?? "", "base64").toString().split(":")[1];
      const chat = groupCopies.get(box);
      const groups = chat?.status === "active" ? [{ id: chat.group.id, name: chat.group.name }] : [];
      return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ groups }));
    }
    if (owned) {
      const local = request.headers.authorization === "Basic " + Buffer.from("opencode:Lab").toString("base64");
      const localAgents = request.headers.authorization === "Basic " + Buffer.from("opencode:Lab-bridge").toString("base64");
      const send = (status, value) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
      if (local && request.url === "/api/event") {
        response.writeHead(200, { "content-type": "text/event-stream" });
        response.flushHeaders();
        ownedEvents.add(response);
        response.on("close", () => ownedEvents.delete(response));
        return;
      }
      const approval = liveAsks && /^\/api\/session\/(ses_owned|ses_native)(?:\/(permission|form)(?:\/([^/]+)\/reply)?)?$/.exec(request.url.split("?", 1)[0]);
      if (approval) {
        const [, session, area, ask] = approval;
        const runner = session === "ses_native" ? "Lab-bridge" : "Lab";
        if (request.headers.authorization !== "Basic " + Buffer.from("opencode:" + runner).toString("base64")) return send(404, {});
        if (!ownedConversationPresent) return send(404, {});
        if (!area) return send(200, { data: { id: session, location: { directory: root } } });
        if (area === "form") return send(200, { data: [] });
        if (request.method === "GET" && !ask) return send(200, { data: permissionAsks.get(session) ?? [] });
        if (request.method === "POST" && ask) {
          const chunks = [];
          request.on("data", chunk => chunks.push(chunk));
          request.on("end", () => {
            const asks = permissionAsks.get(session) ?? [];
            if (!asks.some(pending => pending.id === ask)) return send(404, { message: "That permission is gone." });
            permissionAsks.set(session, asks.filter(pending => pending.id !== ask));
            permissionEffects.push({ session, ask, ...JSON.parse(Buffer.concat(chunks)) });
            send(200, {});
          });
          return;
        }
      }
      const modelList = engineModels?.[request.url.split("?")[0]];
      if (local && modelList !== undefined) return send(200, { data: modelList });
      if (request.url.startsWith("/api/kuvo/link/groups")) {
        if (!local && !localAgents) return send(404, {});
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
        if (ownedLookupRefused && request.url === "/api/session/ses_owned") return send(403, { message: "Conversation lookup withheld." });
        if (!local || !ownedConversationPresent) return send(404, {});
        if (request.url.endsWith("/prompt")) {
          const parts = [];
          request.on("data", part => parts.push(part));
          request.on("end", () => {
            prompts.push(JSON.parse(Buffer.concat(parts)));
            send(200, { data: {} });
          });
          return;
        }
        if (request.url.includes("/message?")) {
          if (ownedReadFailure) return send(ownedReadFailure, { message: "Transcript store isn’t available." });
          return send(200, { data: [...prompts.map(prompt => ({ id: prompt.id, type: "user", text: prompt.text })), ...extraMessages], cursor: {} });
        }
        const model = engineModels?.["/api/model/default"];
        return send(200, { data: { id: "ses_owned", title: "Lab’s conversation", time: { updated: Date.now() }, ...(model ? { model } : {}) } });
      }
      if (request.url.startsWith("/api/session?")) return send(200, { data: local && ownedConversationPresent ? [{ id: "ses_owned", title: "Lab’s conversation", time: { updated: Date.now() } }] : [] });
    }
    const call = request.url.startsWith("/api/session/ses_labels/message") && labelCall;
    const data = call ? [{ content: [{ type: "tool", name: "queue_labels", state: { status: "running", input: call } }] }] : [];
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data }));
  };
  const engine = http.createServer(handleRuntime);
  engine.listen(0, "127.0.0.1");
  await once(engine, "listening");
  const agents = bridgePID ? http.createServer(handleRuntime) : null;
  if (agents) {
    agents.listen(0, "127.0.0.1");
    await once(agents, "listening");
  }
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
    agents?.closeAllConnections();
    await Promise.all([relay.close(), new Promise(resolve => engine.close(resolve)),
      ...(agents ? [new Promise(resolve => agents.close(resolve))] : [])]);
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
      pid: bridgePID, port: agents.address().port, password: owned && name === "Lab" ? "Lab-bridge" : name,
    }));
    const nodeScript = index === splitIndex ? splitScript : name === "Lab" && legacyScript ? legacyScript : recovery && index === 2 ? recoveryScript : boxScript;
    const localRouteScript = path.join(home, "fixture-route.mjs");
    fs.writeFileSync(localRouteScript, `
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
        `);
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
      /** Whether it's the main and holds the relay's lease. */
      lease() {
        const setup = this.read().setup;
        return leaseRelay.lease?.holder === key && setup?.primary === key && setup.role === "primary";
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
        const args = local ? [localRouteScript] : [nodeScript, "box", "route"];
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
      async residentRoute(method, route, body, headers) {
        const { port, password, pid } = read("serve.json");
        assert.equal(pid, this.child.pid);
        const response = await fetch(`http://127.0.0.1:${port}/${route}`, {
          method, headers: { ...headers, authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64"),
            "content-type": "application/json", "x-kuvo-from": "mac" }, body: body === undefined ? undefined : JSON.stringify(body),
        });
        return { status: response.status, value: await response.json() };
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
        const command = mac ? ["serve"] : resident && index !== splitIndex ? ["serve", "--resident"] : ["run"];
        const child = this.child = start(env, [...flags, oldPrimary && index === 0 || oldMember && index === 1 ? oldScript : nodeScript, ...command], [mac ? "pipe" : "ignore", "pipe", "inherit"]);
        if (mac) child.stdin.write(JSON.stringify({ password: "local-test" }) + "\n");
        child.stdout.on("data", chunk => {
          if (this.child !== child) return;
          this.output = (this.output + chunk).slice(-16_000);
          // The app's keeper records the server endpoint when it starts the app-owned Box.
          const port = appFolder && /"url":"http:\/\/127\.0\.0\.1:(\d+)"/.exec(chunk.toString())?.[1];
          if (port) fs.writeFileSync(path.join(run, "serve.json"), JSON.stringify({ pid: child.pid, port: Number(port), password: "local-test" }));
        });
      },
      async stop(signal = "SIGKILL") {
        const stopped = once(this.child, "exit");
        this.child.kill(signal);
        assert.equal((await stopped)[1], signal === "SIGTERM" ? null : signal);
      },
    };
  });
  const [a, b, c] = boxes;
  if (movingGroup) saveGroup(boxes.find(box => box.name === "Lab"), { group: movingGroup, messages: [], status: "active" });
  if (owned) put(path.join(groupFolder(boxes.find(box => box.name === "Lab"), "grp_lab"), "group.json"), JSON.stringify({
    kuvo: 1, status: "active", group: { id: "grp_lab", name: "Lab group", members: [] },
  }));
  const setup = { id: `set_${crypto.randomBytes(16).toString("base64url")}`, epoch: 1, primary: a.key };
  const roster = { boxes: boxes.map(box => box.entry), moved: [], left: [] };
  const accepted = movingGroup || acceptedRoster ? { managed: true, checkpointed: true } : {};
  const bookBody = { setup, ...roster, ...accepted };
  const book = link.signDoc(a.seed, "peers", 1, bookBody);
  const published = link.readDoc(book);
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
        own: { peers: { n: published.n, h: published.h, b: crypto.createHash("sha256").update(JSON.stringify(bookBody)).digest().subarray(0, 8).toString("base64url") } },
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
  // (Read before they start: the main converts its setup as soon as it holds the relay's lease.)
  const prepared = a.read().setup;
  const converted = (prepared?.epoch ?? 0) + 1;
  const leads = prepared?.primary === a.key;
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
  // The main takes the relay's lease and converts the setup in one step, one epoch on; the boxes
  // running this build follow it before a case begins.
  const current = boxes.filter((box, index) => !(legacyScript && box.name === "Lab") && !(recovery && index === 2));
  if (leads) await Promise.all(current.map(box => see(box, converted, a)));
  return { boxes, a, b, c, relay, leaseRelay, see, phone, pairing, prompts, groupMessages, groupCopies, fixtureEngine, phase, tellOwned,
    join: async code => {
      const home = path.join(root, "New Computer");
      fs.mkdirSync(home);
      const env = { HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), PATH: process.env.PATH, TMPDIR: root };
      const child = start(env, [boxScript, "join"], ["pipe", "pipe", "pipe"]);
      let err = "";
      child.stderr.on("data", chunk => { err += chunk; });
      child.stdout.resume();
      child.stdin.end(code + "\n");
      const [status] = await once(child, "close");
      assert.equal(status, 0, err);
      return JSON.parse(fs.readFileSync(path.join(env.KUVO_HOME, "run", "link.json"), "utf8"));
    },
    showOwnedConversation: (present, { deleted = false } = {}) => {
      ownedConversationPresent = present;
      if (deleted) tellOwned({ type: "session.deleted", data: { sessionID: "ses_owned" } });
    },
    breakOwnedRead: status => { ownedReadFailure = status; },
    refuseOwnedLookup: refused => { ownedLookupRefused = refused; },
    setOwnedMessages: messages => { extraMessages = messages; },
    setPermissionAsk: (session, id, sibling) => {
      const ask = (id, resource) => ({ id, sessionID: session, action: "write", resources: [resource] });
      permissionAsks.set(session, sibling ? [ask(id, "fixture.txt"), ask(sibling, "notes.txt")] : [ask(id, "fixture.txt")]);
    },
    permissionEffects,
  };
}

test("failure bench: an older receiver refuses v3 move evidence and ignores evidence handoffs", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-old-evidence-"));
  try {
    const artifact = path.join(root, "neighbor.mjs");
    // Expose the bundled first reader without changing its implementation.
    fs.writeFileSync(artifact, Buffer.concat([recoveryNeighborBuild(), Buffer.from("\nexport { unpack as readMoveFixture };\n")]));
    const home = path.join(root, ".kuvo-code");
    const ran = spawnSync(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import assert from "node:assert/strict";
      import fs from "node:fs";
      import path from "node:path";
      import { pack, unpack } from ${JSON.stringify(new URL("../src/link/moves/bundle.ts", import.meta.url).href)};
      import * as old from ${JSON.stringify(pathToFileURL(artifact).href)};
      const home = process.env.KUVO_HOME;
      fs.mkdirSync(path.join(home, "run"), { recursive: true });
      fs.writeFileSync(path.join(home, "folder-version.json"), '{"version":7}');
      const own = Buffer.alloc(32, 42);
      const sender = { key: old.publicOf(Buffer.alloc(32, 43)).toString("base64url"), name: "Peer", address: "A".repeat(22), kind: "machine" };
      const key = old.publicOf(own).toString("base64url");
      const sign = old.newSigningSeed();
      const signer = old.signingKeyOf(sign);
      const setup = { id: "set_" + "A".repeat(22), primary: key, sign: signer, role: "primary", managed: true, checkpointed: true, epoch: 1 };
      old.writeLink({ key: own.toString("base64url"), sign, setup, sharedSetup: setup.id,
        shared: { membershipAndAccess: { sequence: 1, value: { left: [], boxes: [{ ...sender, key, sign: signer }, sender] } } } });
      const contents = { kind: "group", thing: "grp_fixture", name: "Fixture", exports: [], wasPaused: false };
      await pack("mov_legacy", { ...contents });
      assert.equal((await old.readMoveFixture("mov_legacy")).contents.thing, contents.thing);
      await pack("mov_evidence", { ...contents, forwardEvidence: {} });
      assert.ok((await unpack("mov_evidence")).contents.forwardEvidence);
      await assert.rejects(old.readMoveFixture("mov_evidence"), /manifest format version 3 is unsupported/i);
      const hand = { id: "wqh_abcdefghijklmno", to: "ses_fixture", from: "usr_you",
        item: { id: "wqi_" + "a".repeat(20), title: "Fixture", status: "todo" }, files: [] };
      let admissions = 0;
      const places = () => { admissions++; return { targets: [] }; };
      await old.takeWorkQueueMail(sender, { evidenceHand: { ...hand, evidence: {} } }, null, places);
      assert.equal(admissions, 0, "evidence handoff cannot reach the old admission adapter");
      await old.takeWorkQueueMail(sender, { hand }, null, places);
      assert.equal(admissions, 1, "the same old receiver recognizes the deployed handoff");
      assert.equal(fs.existsSync(path.join(home, "run", "owned-data", "mutations")), false);
    `], { encoding: "utf8", timeout: 30_000, env: { ...process.env, HOME: root, KUVO_HOME: home } });
    assert.equal(ran.status, 0, ran.stderr || String(ran.error ?? ran.signal));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("failure bench: a real idle group move survives source loss and replacement restore", { timeout: 105_000 }, async t => {
  const group = { id: "grp_recovery", name: "Recovered group", members: ["bot_one", "bot_two"], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, groupCopies, relay, phase } = await network(t, false, undefined, false, group, false, false, undefined, { phaseDiagnostics: true });
  const groupPath = "groups/" + group.id;
  const phone = link.newKey();
  const invite = await a.route("POST", "kuvo/phones/pair");
  const paired = await deviceAt(relay, a, phone, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Receipt fixture" });
  assert.ok(paired.greeting);
  t.after(paired.close);
  const at = box => until("paired Work Queue client on " + box.name, async () => {
    const device = await deviceAt(relay, box, phone);
    if (!device.greeting) return false;
    t.after(device.close);
    return device;
  }, t.signal);
  const lister = await at(b);
  phase("paired listing Box");
  const made = await c.route("POST", "kuvo/work-queue/items", { owner: group.id, title: "Before forwarding", operation: "queue-move-create" }, true);
  assert.equal(made.status, 200, JSON.stringify(made));
  const item = made.value.item.id;
  phase("created item in the source group queue");
  const itemPath = `kuvo/work-queue/items/${item}`;
  await until("the listing Box can read the group queue item", async () => (await lister.ask("GET", itemPath)).status === 200, t.signal);
  const headers = { "x-kuvo-operation": "queue-move-patch" };
  const intent = { title: "Accepted before move" };
  phase("listing Box can see the queued item");
  const forwarded = await lister.ask("PATCH", itemPath, intent, headers);
  assert.equal(forwarded.status, 200, JSON.stringify(forwarded));
  const saved = crypto.createHash("sha256").update(JSON.stringify(["scoped-forward", c.read().setup.id, headers["x-kuvo-operation"]])).digest("base64url");
  const savedPath = box => path.join(box.run, "owned-data", "mutations", saved + ".json");
  assert.equal(JSON.parse(fs.readFileSync(savedPath(c), "utf8")).forward.kind, "forwardedQueueDecisions");
  phase("source accepted the forwarding decision");
  const folder = fs.realpathSync(c.env.TMPDIR) + "/backups";
  fs.mkdirSync(folder);
  const destination = { kind: "disk", path: folder };
  const source = "b_" + crypto.randomBytes(16).toString("base64url");
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  put(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: recoveryKey, dest: destination,
    disk: fs.statSync(folder).dev, projects: [],
  }));
  const backup = await c.route("POST", "kuvo/box/backups/now", {}, true);
  assert.equal(backup.status, 200, JSON.stringify(backup.value));
  const point = backup.value.point?.id;
  assert.ok(point);
  phase("captured the source backup");
  const moved = await c.route("POST", "kuvo/moves", { kind: "group", id: group.id, to: a.key, receipt: true }, true);
  assert.equal(moved.status, 202, JSON.stringify(moved.value));
  const receipt = path.join(c.run, "moves", moved.value.move.id + ".outcome.json");
  await until("move completed", () => fs.existsSync(receipt), t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active", JSON.stringify(JSON.parse(fs.readFileSync(receipt))));
  assert.equal(groupCopies.has("Lab"), false);
  phase("group move committed");
  assert.equal(JSON.parse(fs.readFileSync(savedPath(a), "utf8")).forward.owner, c.key);
  const destinationClient = await at(a);
  const retitled = await destinationClient.ask("PATCH", itemPath, { title: "New title at destination" });
  assert.equal(retitled.status, 200, JSON.stringify(retitled));
  const replay = await destinationClient.ask("PATCH", itemPath, intent, headers);
  assert.equal(replay.status, 200, JSON.stringify(replay));
  assert.equal(replay.value.item.title, intent.title);
  assert.equal((await destinationClient.ask("GET", itemPath)).value.item.title, "New title at destination");
  phase("destination replay recovered the old reply without changing the later title");
  await c.stop("SIGTERM");
  fs.rmSync(c.env.HOME, { recursive: true });
  assert.equal((await a.route("DELETE", "kuvo/computers/" + c.key)).status, 200);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: c.key, replacement: b.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  await until("replacement ownership replica", () => b.read().shared?.ownershipRecovery?.value?.["replacement:" + c.key], t.signal);

  phase("replacement assignment verified");
  await until("the replacement heard the destination announce the group", () => peerOwns(b, a, group.id), t.signal);
  const recovered = await b.route("POST", "kuvo/box/backups/restore", { destination, source, id: point, recoveryKey, confirm: true }, true);
  assert.equal(recovered.status, 200, JSON.stringify(recovered.value));
  assert.ok(recovered.value.unavailable.some(entry => entry.entry === groupPath + "/group.json" && entry.reason === "moved"));
  assert.equal(fs.existsSync(path.join(b.env.HOME, groupPath, "group.json")), false);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(fs.existsSync(savedPath(b)), false, "restore did not install an outcome for the moved item");
  assert.ok(recovered.value.held.includes(`home/run/owned-data/mutations/${saved}.json`));
});

test("failure bench: an away owner leaves saved reads and accepts an unsent message on return", { timeout: 90_000 }, async t => {
  const { a, b, c, relay, prompts, groupMessages } = await network(t, false, undefined, true);
  await until("both computers know the conversation and group owner", () =>
    [a, b].every(box => peerOwns(box, c, "ses_owned") && peerOwns(box, c, "grp_lab")), t.signal);
  const read = (box, query = "") => box.route("GET", `kuvo/data/owner?kind=conversations&id=ses_owned&computer=${c.key}${query}`);
  const before = await read(b);
  assert.equal(before.status, 200);
  assert.equal(before.value.owner.key, c.key);
  assert.equal(before.value.owner.name, "Lab");
  assert.equal(before.value.owner.reachable, true);
  assert.equal(before.value.state, "received");
  assert.equal(before.value.refresh, "live");
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
  assert.equal(stale.value.refresh, "unavailable");
  assert.equal(stale.value.lastReceivedAt, before.value.lastReceivedAt);
  assert.deepEqual(stale.value.unavailable, { files: ["plan.md"], messages: ["msg_unknown"] });
  const unseen = await read(a);
  assert.equal(unseen.value.state, "neverReceived");
  assert.equal(unseen.value.refresh, "unavailable");
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

test("owner read distinguishes deletion, failed refresh and unavailable files on pc4", { timeout: 60_000 }, async t => {
  const { a, b, c, relay, see, showOwnedConversation, breakOwnedRead, refuseOwnedLookup, setOwnedMessages } = await network(t, true, undefined, true);
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  // A completed move keeps the owner known even after both runtimes stop listing the session.
  const owner = { session: "ses_owned", key: c.key, at: Date.now() };
  put(path.join(b.run, "moves", "moved.json"), JSON.stringify([owner]));
  put(path.join(c.run, "moves", "moved.json"), JSON.stringify([{ ...owner, arrived: true }]));
  const query = `kuvo/data/owner?kind=conversations&id=ses_owned&computer=${c.key}`;
  const read = async (box = b, extra = "") => {
    const reply = await box.route("GET", query + extra);
    assert.equal(reply.status, 200, JSON.stringify(reply.value));
    return reply.value;
  };
  const empty = await read();
  assert.deepEqual(empty.data.messages, []);
  assert.equal(empty.owner.key, c.key);
  assert.equal(empty.state, "received");
  assert.equal(empty.refresh, "live");
  assert.equal(empty.stale, false);
  assert.ok(empty.lastReceivedAt > 0);

  const config = await rename(a, c, "Archive Lab");
  assert.equal(config.status, 200, JSON.stringify(config.value));
  await until("pc4 has the accepted name", () => b.read().shared?.setupNames?.value?.[c.key] === "Archive Lab", t.signal);
  const managed = path.join(c.home, "attachments", "ses_owned", "fixture", "managed.txt");
  put(managed, "managed bytes");
  const messages = [{ id: "msg_kept", files: [{ name: "inline.txt", source: { type: "inline" }, data: "aGk=" }, { name: "missing.txt" }],
    content: [{ type: "tool", state: { status: "completed", content: [{ type: "file", uri: pathToFileURL(managed).href }] } }] }];
  setOwnedMessages(messages);
  const refs = "&file=arbitrary&message=msg_kept&message=msg_elsewhere";
  const saved = await read(b, refs);
  assert.equal(saved.owner.name, "Archive Lab");
  assert.deepEqual(saved.unavailable, { files: ["inline.txt", "missing.txt", "managed.txt", "arbitrary"], messages: ["msg_elsewhere"] });
  const local = await read(c, refs);
  assert.deepEqual(local.unavailable.files, ["missing.txt", "arbitrary"]);
  fs.unlinkSync(managed);
  assert.deepEqual((await read(c, refs)).unavailable.files, ["missing.txt", "managed.txt", "arbitrary"]);
  await read(b, "&offset=older");

  const withheld = (await b.route("GET", query.replace("conversations", "pendingSessionWork"))).value;
  assert.equal(withheld.refresh, "withheld");
  assert.equal(withheld.lastReceivedAt, null);
  breakOwnedRead(503);
  const failed = await read(b, refs);
  assert.equal(failed.refresh, "failed");
  assert.equal(failed.owner.reachable, true);
  assert.equal(failed.stale, true);
  assert.equal(failed.lastReceivedAt, saved.lastReceivedAt);
  assert.deepEqual(failed.data.messages, messages);
  breakOwnedRead(403);
  const blocked = await read();
  assert.equal(blocked.refresh, "withheld");
  assert.equal(blocked.lastReceivedAt, null);
  assert.equal(Object.hasOwn(blocked, "data"), false);
  breakOwnedRead(503);
  const failedAfterRefusal = await read(b, "&offset=older");
  assert.equal(failedAfterRefusal.refresh, "failed");
  assert.equal(failedAfterRefusal.state, "neverReceived");

  breakOwnedRead(0);
  await read();
  refuseOwnedLookup(true);
  const refusedLookup = await read();
  // Its owner and conversation are already known, so a lookup refusal cannot withhold
  // an otherwise live messages response.
  assert.equal(refusedLookup.refresh, "live");
  assert.deepEqual(refusedLookup.data.messages, messages);
  refuseOwnedLookup(false);

  showOwnedConversation(false, { deleted: true });
  const absent = await until("owner confirms deletion", async () => {
    const value = await read();
    return value.state === "absent" && value;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(8_000)]));
  assert.equal(absent.state, "absent");
  assert.equal(absent.refresh, "live");
  assert.equal(absent.stale, false);
  assert.ok(absent.lastReceivedAt >= saved.lastReceivedAt);
  assert.equal(Object.hasOwn(absent, "data"), false);
  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const staleAbsence = await read(b, "&offset=older&file=missing.txt&message=msg_kept");
  assert.equal(staleAbsence.state, "absent");
  assert.equal(staleAbsence.refresh, "unavailable");
  assert.equal(staleAbsence.owner.name, "Archive Lab");
  assert.equal(staleAbsence.owner.reachable, false);
  assert.equal(staleAbsence.stale, true);
  assert.equal(staleAbsence.lastReceivedAt, absent.lastReceivedAt);
  assert.deepEqual(staleAbsence.unavailable, { files: ["missing.txt"], messages: ["msg_kept"] });

  showOwnedConversation(true);
  breakOwnedRead(0);
  c.start();
  reconnect();
  await until("pc5 is back and serves a new page", async () => (await read()).refresh === "live", t.signal);
  breakOwnedRead(503);
  const oldOffset = await read(b, "&offset=older");
  assert.equal(oldOffset.state, "neverReceived");
  assert.equal(oldOffset.refresh, "failed");
  assert.equal(oldOffset.lastReceivedAt, null);
});

for (const old of [false, true]) test(
  old ? "an updated sender uses a real beta.35 owner" : "a behind owner accepts relayed conversation prompts and forwarded Work Queue mutations",
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
  const { b, c, prompts } = await network(t, false, undefined, true, undefined, false, false, undefined, { beta35Owner: old, prepare });
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
  const create = { owner: "ses_owned", title: "Mixed-version queue", operation: "fleet-admission-queue" };
  const queued = await b.route("POST", "kuvo/work-queue/items", create, true, headers);
  assert.equal(queued.status, 200, JSON.stringify(queued));
  if (old) return; // beta.35 supports queue forwarding, before the scoped prompt wire format.
  const message = { id: "msg_fleetadmission", words: "Run on the owner while it updates." };
  const sent = await b.route("POST", "kuvo/sessions/ses_owned/send", message, true, headers);
  assert.equal(sent.status, 202, JSON.stringify(sent));
  const receipt = () => b.route("GET", `kuvo/data/messages/${sent.value.operation}`, undefined, true, headers);
  await until("the prompt commits while the owner runs the older release", async () => (await receipt()).value.status === "committed", t.signal);
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
});

test("failure bench: an offline member follows two handovers and the former primary can be unlinked", { timeout: 50_000 }, async t => {
  const { a, b, c, see, phone, pairing } = await network(t);
  assert.deepEqual(await submit(c, b, 2), { status: 202, value: { primary: b.key, epoch: 3 } });
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
  assert.equal(b.read().phones.find(device => device.key === phone)?.pairing, pairing);
  assert.equal(a.read().phones.find(device => device.key === phone)?.mac, b.key);
  assert.equal(b.read().vouchBlocked, undefined);
  const transition = link.readDoc(b.read().leadership[3]);
  const stale = c.read();
  assert.equal(link.takeDoc(stale, b.read().leadership[3]), null);
  // The former main can't sign a certificate of its own: the relay's lease is the new main's.
  const forged = link.signDoc(a.seed, "leadership", 4, { ...transition.body, previous: b.key, setup: { ...transition.body.setup, epoch: 4, primary: a.key } });
  assert.equal(link.takeDoc(stale, forged), null);
  assert.equal(stale.setup.primary, b.key);
  const staleRevoke = link.signDoc(a.seed, "phones", 40, { phones: [], removed: [{ key: phone, pairing }] });
  assert.equal(link.takeDoc(stale, staleRevoke), null);
  assert.deepEqual(await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }), {
    status: 400, value: { message: "Only a device paired with your setup can remove devices." },
  });

  await c.stop("SIGTERM");
  assert.deepEqual(await submit(b, a, 3), { status: 202, value: { primary: a.key, epoch: 4 } });
  await Promise.all([see(a, 4, a), see(b, 4, a)]);
  assert.deepEqual(await submit(a, b, 4), { status: 202, value: { primary: b.key, epoch: 5 } });
  await Promise.all([see(a, 5, b), see(b, 5, b)]);
  assert.equal(c.read().setup.epoch, 3);
  c.start();
  await see(c, 5, b);
  assert.ok(c.read().leadership[5]);
  const setupID = b.read().setup.id;
  assert.equal((await b.route("DELETE", `kuvo/computers/${a.key}`)).status, 200);
  await until("removal received", () => a.read().setup?.id !== setupID, t.signal);
  assert.equal(a.read().macPeers, undefined);
  await see(b, 5, b);
});

test("failure bench: an SSH-enrolled member advertises its accepted epoch and catches up", { timeout: 35_000 }, async t => {
  const { a, b, c, see } = await network(t);
  assert.equal((await submit(a, b, 2)).status, 202);
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
  assert.equal((await submit(b, a, 3)).status, 202);
  await Promise.all([see(a, 4, a), see(b, 4, a), see(c, 4, a)]);
  await c.stop("SIGTERM");

  const primary = a.read();
  const gift = link.ownDoc(primary, "peers", link.primaryPeersBody(primary));
  const joined = { ...c.identity, mac: a.key, macs: [a.key], macSigns: { [a.key]: a.sign }, phones: [] };
  // SSH gives a fresh member the current signed roster, without the preceding epoch's authority.
  link.docsWork({ link: joined, library: { personalities: [], removed: [] } }, a.entry, { give: [gift.envelope] }, { stay: a.key });
  assert.equal(joined.setup.epoch, 4);
  assert.equal(joined.leadership, undefined);
  assert.ok(link.digestOf(joined).some(entry => entry[1] === "leadership" && entry[2] === 4));
  fs.writeFileSync(path.join(c.run, "link.json"), JSON.stringify(joined));

  assert.equal((await submit(a, b, 4)).status, 202);
  await Promise.all([see(a, 5, b), see(b, 5, b)]);
  c.start();
  await see(c, 5, b);
  assert.ok(c.read().leadership[5]);
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
  assert.equal((await submit(a, b, 2)).status, 202);
  assert.equal(typeof reconnect, "function");
  // (The relay moved its lease: it takes over, and once back the others follow it.)
  reconnect();
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
});

test("failure bench: a primary killed after committing a lease transfer resumes as a member", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  // The lease moves at the relay. Only the successor can publish the new epoch.
  const resumeWitness = relay.pause(b.address, c.address);
  let resumeFormerMain;
  const unhook = relay.intercept(b.address, a.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), a.read())?.payload;
    if (message?.workQueue?.acted?.value?.sign !== b.sign) return false;
    resumeFormerMain = relay.pause(b.address, a.address);
    unhook();
    return false;
  });
  assert.equal((await submit(a, b, 2)).status, 202);
  await a.stop();
  await see(b, 3, b);
  assert.equal(c.read().setup.epoch, 2);
  assert.equal(typeof resumeFormerMain, "function");
  resumeFormerMain();
  resumeWitness();
  a.start();
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
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
  assert.equal((await submit(a, b, 2)).status, 503);
  assert.equal(typeof reconnect, "function");
  await Promise.all([see(a, 2, a), see(b, 2, a), see(c, 2, a)]);
  assert.deepEqual(Object.keys(a.read().leadership), ["2"]);
  reconnect();
});

// Three computers whose main holds the relay's lease, the second one always on.
async function leasedNetwork(t, releaseKey) {
  const net = await network(t, true, releaseKey);
  await until("the main holds the relay lease", () => net.a.lease(), t.signal);
  return net;
}

test("failure bench: unlink retries credential cleanup before acknowledging completion", { timeout: 45_000 }, async t => {
  const { a, b, c, see } = await leasedNetwork(t);
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

// A computer's own pairing is independent work: SSH refuses that computer, and it joins through a
// setup merge (installer-enrollment.test.js). A member keeps a pairing it made before joining aside
// (`preJoinPairings`) until its main grants it; re-enrolling it takes an explicit decision.
test("an unmatched pre-join pairing requires an explicit enrollment decision", { timeout: 45_000 }, async t => {
  // The main grants this phone with `pairing`, which the member paired on its own before joining.
  const { a, c, phone, pairing, relay } = await network(t);
  await c.stop("SIGTERM");
  const file = path.join(c.run, "link.json");
  const local = { ...c.read(), preJoinPairings: [{ key: phone, pairing, from: "pair", local: true }] };
  fs.writeFileSync(file, JSON.stringify(local));
  // Keep this CLI fixture from launching a second, untracked bridge.
  fs.mkdirSync(path.join(c.run, ".updating"));
  fs.writeFileSync(path.join(c.run, ".updating", "pid"), String(process.pid));
  const token = "guarded-" + crypto.randomBytes(24).toString("hex");
  fs.symlinkSync(token, path.join(path.dirname(c.home), ".kuvo-code-adding"));
  const signed = link.signDoc(a.seed, "phones", 1, { phones: [{ key: phone, pairing: "different-pairing" }], removed: [] });
  const refused = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, docs: [signed] }));
  assert.equal(refused.status, 5, refused.err);
  assert.match(refused.err, /Confirm discarding unmatched local device pairings/);
  assert.deepEqual(c.read(), local);

  const roster = link.signDoc(a.seed, "peers", 2, { setup: { id: a.read().setup.id, epoch: 1, primary: a.key },
    boxes: [a.entry, c.entry], moved: [], left: [] });
  const matching = link.signDoc(a.seed, "phones", 2, { phones: [{ key: phone, pairing }], removed: [] });
  const joined = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, relay: relay.url, docs: [roster, matching] }));
  assert.equal(joined.status, 0, joined.err);
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.pairing, pairing);
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.from, "mac");
  assert.equal(c.read().preJoinPairings, undefined, "the main's grant replaced the pairing kept aside");
});

test("failure bench: local uninstall refuses a primary before clearing identity or stopping it", { timeout: 30_000 }, async t => {
  const { a } = await leasedNetwork(t);
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

test("failure bench: access routes return committed receipts from the main and through a member", { timeout: 90_000 }, async t => {
  const { a, c } = await leasedNetwork(t);
  const device = name => ({ key: link.publicOf(link.newKey()).toString("base64url"), name, kind: "desktop" });
  const pair = (box, asking) => box.route("POST", "kuvo/devices/pair", asking, true);
  const remove = (box, asking) => box.route("POST", "kuvo/phones/remove", { phones: [{ key: asking.key }] }, true);
  const access = async request => {
    const reply = await request();
    assert.equal(reply.status, 200, JSON.stringify(reply));
    assertReceipt(a, "membershipAndAccess", "committed", reply.value);
    return reply.value;
  };
  const local = device("Local");
  const member = device("Member");
  await access(() => pair(a, local));
  assert.equal((await access(() => pair(c, member))).paired, true);
  await access(() => remove(a, local));
  await access(() => remove(c, member));
  await access(() => a.route("DELETE", `kuvo/computers/${c.key}`));
});

test("failure bench: forwarded computer changes carry receipts through the member route", { timeout: 45_000 }, async t => {
  const { a, b, c } = await leasedNetwork(t);
  const named = await rename(b, c, "Forwarded");
  assert.equal(named.status, 200, JSON.stringify(named));
  assertReceipt(a, "setupNames", "committed", named.value);
  const removed = await b.route("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(removed.status, 200, JSON.stringify(removed));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
});

test("a scoped rename cannot overwrite a later name after primary handover", { timeout: 105_000 }, async t => {
  const { a, b, c, relay, see } = await leasedNetwork(t);
  const key = link.newKey();
  await enrollDesktop(a, key, "Receipt fixture");
  const at = box => until("paired rename client on " + box.name, async () => {
    const device = await deviceAt(relay, box, key, { client: "desktop" });
    if (!device.greeting) return false;
    t.after(device.close);
    return device;
  }, t.signal);
  const listing = await at(c);
  const route = `kuvo/computers/${c.key}`;
  const headers = { "x-kuvo-operation": "stable-forwarded-rename" };
  const intent = { name: "First accepted name" };
  const first = await listing.ask("PATCH", route, intent, headers);
  assert.equal(first.status, 200, JSON.stringify(first));
  const digest = purpose => crypto.createHash("sha256").update(JSON.stringify([purpose, a.read().setup.id, headers["x-kuvo-operation"]])).digest("base64url");
  const readReceipt = (box, family, purpose) => JSON.parse(fs.readFileSync(path.join(box.run, "owned-data", family, digest(purpose) + ".json"), "utf8"));
  assert.equal(readReceipt(c, "forward-origins", "forward-origin").forward.kind, "forwardedOperationOrigins");
  assert.equal(readReceipt(a, "mutations", "scoped-forward").forward.kind, "forwardedPrimaryDecisions");
  const changed = await submit(a, b, 2);
  assert.equal(changed.status, 202, JSON.stringify(changed));
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  await until("the new main holds the relay lease", () => b.lease(), t.signal);
  const direct = await at(b);
  assert.equal((await direct.ask("PATCH", route, intent, headers)).status, 409);
  const renamed = await rename(b, c, "Name after handover");
  assert.ok([200, 202].includes(renamed.status), JSON.stringify(renamed));
  if (renamed.status === 202) await until("newer name committed", () => b.read().sharedOutcomes?.[renamed.value.operation]?.status === "committed", t.signal);
  assert.equal((await listing.ask("PATCH", route, intent, headers)).status, 409);
  assert.equal((await direct.ask("PATCH", route, intent, headers)).status, 409);
  assert.equal(b.read().shared.setupNames.value[c.key], "Name after handover");
});

async function enrollDesktop(box, key, name) {
  const device = { key: link.publicOf(key).toString("base64url"), name, kind: "desktop" };
  const enrolled = await box.route("POST", "kuvo/devices/pair", device, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled));
  return enrolled.value;
}

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
    address: client.address,
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

test("live permission replies from paired clients expire across pc5 reconnect", { timeout: 80_000 }, async t => {
  const { a, b, c, relay, see, setPermissionAsk, permissionEffects } =
    await network(t, true, undefined, true, undefined, false, false, undefined, { liveAsks: true });
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const managerKey = link.newKey();
  const deskKey = link.newKey();
  const desktop = { key: link.publicOf(deskKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  await enrollDesktop(a, managerKey, "Primary desktop");
  const primaryDesktop = await deviceAt(relay, a, managerKey, { client: "desktop" });
  assert.ok(primaryDesktop.greeting);
  t.after(primaryDesktop.close);
  const enrolled = await a.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled.value));
  const open = async (box, key, hello = { client: "desktop" }) => {
    const client = await until("current pairing at " + box.name, async () => {
      const client = await deviceAt(relay, box, key, hello);
      return client.greeting && client;
    }, t.signal);
    t.after(client.close);
    return client;
  };
  const manager = await open(c, managerKey);
  const desk = await open(c, deskKey, { client: "desktop" });
  const lister = await open(b, managerKey);
  const apps = [manager, desk];
  const sessions = ["ses_owned", "ses_native"];
  const native = (session, id) => `${session === "ses_native" ? "kuvo/agents/" : ""}api/session/${session}/permission/${id}/reply`;
  const askRoute = (session, id) => `kuvo/sessions/${session}/asks/${id}`;
  for (const [i, client] of apps.entries()) {
    for (const session of sessions) {
      const id = `per_live_${i}_${session}`;
      setPermissionAsk(session, id);
      const path = i === 0 ? askRoute(session, id) : native(session, id);
      const body = i === 0 ? { answer: "once" } : { decision: "once" };
      assert.equal((await client.ask("POST", path, body)).status, 200);
      assert.equal((await client.ask("POST", path, body)).status, 404);
    }
  }
  assert.equal(permissionEffects.length, 4);

  const settings = path.join(c.home, "box", "opencode.json");
  put(settings, JSON.stringify({ permissions: [{ action: "write", resource: "fixture.txt", effect: "deny" }] }));
  for (const session of sessions) {
    setPermissionAsk(session, "per_localdeny");
    const denied = await desk.ask("POST", native(session, "per_localdeny"), { decision: "always" });
    assert.equal(denied.status, 403, JSON.stringify(denied.value));
    assert.equal((await manager.ask("POST", askRoute(session, "per_localdeny"), { answer: "once" })).status, 403);
    setPermissionAsk(session, "per_denied_sibling", "per_allowed_sibling");
    assert.equal((await desk.ask("POST", native(session, "per_allowed_sibling"), { decision: "always" })).status, 403);
    assert.equal((await manager.ask("POST", askRoute(session, "per_allowed_sibling"), { answer: "always" })).status, 403);
  }
  assert.equal(permissionEffects.length, 4);
  fs.unlinkSync(settings);

  setPermissionAsk("ses_owned", "per_old_engine");
  setPermissionAsk("ses_native", "per_old_bridge");
  assert.equal((await lister.ask("POST", askRoute("ses_owned", "per_old_engine"), { answer: "once" })).status, 409);
  const held = [];
  const release = apps.map(client => relay.intercept(client.address, c.address, packet => { held.push(packet); return true; }));
  release.forEach(stop => t.after(stop));
  const pending = [
    manager.ask("POST", native("ses_owned", "per_old_engine"), { decision: "once" }),
    desk.ask("POST", native("ses_native", "per_old_bridge"), { decision: "always" }),
  ].map(answer => answer.catch(() => ({ closed: true })));
  await until("both live replies are held in transport", () => held.length >= 2, t.signal);
  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const revoked = await primaryDesktop.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: enrolled.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked.value));
  setPermissionAsk("ses_owned", "per_replacement_engine");
  setPermissionAsk("ses_native", "per_replacement_bridge");
  c.start();
  reconnect();
  await until("pc5 welcomed", () => relay.has(c.address), t.signal);
  release.forEach(stop => stop());
  held.forEach(packet => relay.replay(packet));
  await until("pc5 received revocation", () => c.read().removed?.some(entry => entry.key === desktop.key), t.signal);
  const refused = await deviceAt(relay, c, deskKey, { client: "desktop" });
  assert.equal(refused.greeting, undefined);
  const connected = await open(c, managerKey);
  for (const [session, id] of [["ses_owned", "per_old_engine"], ["ses_native", "per_old_bridge"]]) {
    assert.equal((await connected.ask("POST", native(session, id), { decision: "once" })).status, 404);
    assert.equal((await connected.ask("POST", askRoute(session, id), { answer: "once" })).status, 404);
  }
  for (const result of await Promise.all(pending)) assert.ok(result.closed, JSON.stringify(result));
  assert.equal(permissionEffects.length, 4);

  // A committed departure leaves old engine bytes accessible to the fixture.
  put(path.join(c.run, "moves", "moved.json"), JSON.stringify([{ session: "ses_owned", key: b.key, at: Date.now() }]));
  const moved = await connected.ask("POST", "api/session/%73es_owned/permission/per_replacement_engine/reply", { decision: "once" });
  assert.equal(moved.status, 409, JSON.stringify(moved.value));
  assert.equal(permissionEffects.length, 4);
});

test("failure bench: clients share machine changes and revocations across three boxes", { timeout: 90_000 }, async t => {
  const { a, b, c, relay, see } = await leasedNetwork(t);
  await see(c, 2, a);
  const open = async (box, key, hello = { client: "desktop" }) => {
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
  const managerKey = link.newKey();
  const managerPublic = link.publicOf(managerKey).toString("base64url");
  await enrollDesktop(a, managerKey, "Managing desktop");
  const managerA = await open(a, managerKey);
  assert.ok(managerA.greeting);
  const managerB = await pairedAt(b, managerKey);
  const managerC = await pairedAt(c, managerKey);

  const launchKey = link.newKey();
  const desktop = { key: link.publicOf(launchKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const save = await b.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(save.status, 200, JSON.stringify(save));
  assertReceipt(a, "membershipAndAccess", "committed", save.value);
  const deskA = await pairedAt(a, launchKey, { client: "desktop" });
  const deskB = await pairedAt(b, launchKey, { client: "desktop" });
  const deskC = await pairedAt(c, launchKey, { client: "desktop" });
  for (const device of [deskA, deskB, deskC, managerA, managerB, managerC]) {
    const roster = await device.ask("GET", "kuvo/computers");
    assert.equal(roster.status, 200);
    assert.deepEqual(new Set(roster.value.computers.map(peer => peer.key)), new Set([a.key, b.key, c.key]));
  }

  const renamed = await managerB.ask("PATCH", `kuvo/computers/${c.key}`, { name: "Shared name" });
  assert.equal(renamed.status, 200, JSON.stringify(renamed));
  assertReceipt(a, "setupNames", "committed", renamed.value);
  for (const device of [deskA, deskB, deskC]) {
    await byClient("the desktop reads the shared name", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.find(peer => peer.key === c.key)?.name === "Shared name");
  }

  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await until("the third computer goes offline", () => !relay.has(c.address), t.signal);
  const revoked = await managerB.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: save.value.pairing }] });
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
    assert.ok(box.read().phones.some(device => device.key === managerPublic));
  }

  const removed = await managerB.ask("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(removed.status, 200, JSON.stringify(removed));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  for (const device of [managerA, managerB]) {
    await byClient("the removed machine is absent from the roster", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.every(peer => peer.key !== c.key));
    assert.equal((await device.ask("POST", `kuvo/computers/${c.key}/connect`)).status, 404);
  }
  await until("the removed computer received its unlink", () => !c.read().phones.some(device => device.key === managerPublic), t.signal);
  assert.deepEqual(await open(c, managerKey), { refused: true });
});

const personality = (i, length = 40) => ({ id: `custom.personal-${i}`, name: `Personality ${i}`, description: "x".repeat(length), updated: i + 1 });

test("failure bench: the SSH personalities command saves at the main", { timeout: 35_000 }, async t => {
  const { a, b } = await leasedNetwork(t);
  const file = path.join(a.env.HOME, "bots", "personalities.json");
  const oldBytes = () => fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
  const before = oldBytes();
  const document = { personalities: [personality(1)], removed: [] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.status, 0, saved.err);
  assert.equal(saved.value?.status, 200, JSON.stringify(saved));
  assert.equal(saved.value.save?.status, "committed");
  const { setup, operation, proof } = saved.value.save;
  assert.equal(link.readDoc(proof)?.body.operation, operation);
  assert.equal(link.readDoc(proof)?.body.id, setup);
  await until("the member keeps the main's decision", () => b.read().sharedOutcomes?.[operation]?.status === "committed", t.signal);
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
  assert.equal(oldDesktop.status, 200, JSON.stringify(oldDesktop.value));
  const emptyLabels = { labels: [], removed: [] };
  const shared = box => box.read().shared?.queueLabels;
  const labels = box => shared(box)?.value ?? emptyLabels;
  // The member copies the main's labels as soon as they're saved.
  const sameCopy = what => until(what, () => JSON.stringify(shared(b)) === JSON.stringify(shared(a)), t.signal);
  for (const field of ["key", "secret", "sign"]) assert.equal(a.read()[field], undefined);
  assert.deepEqual(labels(a), emptyLabels);
  assert.deepEqual(labels(b), emptyLabels);
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
    const outdated = await restore(a, "--dry-run");
    assert.equal(outdated.status, 0, outdated.err);
    assert.deepEqual(labels(a), emptyLabels);
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
  // (Leaving out the relay lease record, which the box refreshes by itself every few seconds.)
  const snapshot = box => Object.fromEntries(fs.readdirSync(box.home, { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile() && !entry.name.startsWith("setup-relay-lease.json")).flatMap(entry => {
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
    // (Before the first restore there's no shared library yet: the preview says it uses the main's labels.)
    const syncing = "The shared label library is syncing. This preview uses the main computer’s current labels.\n";
    assert.equal(await run(a, "--dry-run"), syncing + "Dry run: would restore 10, already present 0, conflicts 0.");
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
    assert.equal(await preview, syncing + "Dry run: would restore 10, already present 0, conflicts 0.");
    assert.deepEqual(snapshot(a), snapshots[0], "the primary's files are byte-identical after the forwarded dry run");
    assert.deepEqual(snapshot(b), snapshots[1], "the member's files are byte-identical after the forwarded dry run");
  } finally {
    releaseA();
    releaseB();
  }
  assert.deepEqual(labels(a), emptyLabels);
  assert.deepEqual(labels(b), emptyLabels);
  assert.equal(pendingOperation(a, "queueLabels"), undefined);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(b), "Restored 10, already present 0, conflicts 0.");
  assert.deepEqual(labels(a), library);
  await sameCopy("the member copied the restored labels");
  const restoredAt = shared(a).sequence;
  const after = await readView();
  assert.deepEqual(after.labels, definitions);
  const byId = new Map(after.items.map(item => [item.id, item.labels]));
  for (const item of items) assert.deepEqual(byId.get(item.id), item.labels);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(a), "Restored 0, already present 10, conflicts 0.");
  assert.deepEqual(labels(a), library);
  assert.equal(shared(a).sequence, restoredAt);
  await sameCopy("the member kept the main's labels");
  assert.equal(pendingOperation(a, "queueLabels"), undefined);

  const [removed, kept] = definitions;
  const deletion = await macRequest("POST", "kuvo/work-queue/labels", { remove: removed.id });
  assert.equal(deletion.status, 200, JSON.stringify(deletion.value));
  assert.deepEqual(labels(a), { labels: definitions.slice(1), removed: [removed.id] });
  assert.equal(shared(a).sequence, restoredAt + 1);
  // A forced restore may clear the tombstone, but any existing definition still wins.
  fs.writeFileSync(file, JSON.stringify({ labels: [removed, { ...kept, name: "Different name", color: "pink" }], removed: [kept.id] }));
  assert.equal(await run(a),
    `Restored 0, already present 0, conflicts 2.\n${removed.id}: This label was removed. Use --force to restore it.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, restoredAt + 1);
  await sameCopy("the member kept the main's labels");
  assert.equal(await run(a, "--force", "--dry-run"),
    `Dry run: would restore 1, already present 0, conflicts 1.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, restoredAt + 1);
  await sameCopy("the member kept the main's labels");

  // With the member cut off, the main saves alone; the member copies it once back.
  const resume = relay.pause(a.address, b.address);
  try {
    const forced = await restore(a, "--force");
    assert.equal(forced.status, 0, forced.err);
    assert.equal(shared(a).sequence, restoredAt + 2);
    assert.equal(shared(b).sequence, restoredAt + 1);
  } finally { resume(); }
  assert.deepEqual(labels(a), { labels: [...definitions.slice(1), removed], removed: [] });
  await sameCopy("the member copied the forced restore");
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

const rename = (a, target, name) => a.route("PATCH", `kuvo/computers/${target.key}`, { name });
const setupName = async (box, key) => (await box.route("GET", "kuvo/setup")).value.computers.find(each => each.key === key)?.name;

test("failure bench: a saved change survives killing the main before it publishes to everyone", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, leaseRelay, phone, pairing } = await leasedNetwork(t);
  // Prevent the third computer from learning the change from the old main.
  const reconnect = [relay.pause(a.address, c.address), relay.pause(b.address, c.address)];
  for (const [box, name] of [[b, "Renamed Desk"], [c, "Accepted name"]]) {
    const done = await rename(a, box, name);
    assert.equal(done.status, 200, JSON.stringify(done.value));
    assert.equal(done.value.status, "committed");
  }
  await until("the other member copied the names", () => b.read().shared?.setupNames?.value?.[c.key] === "Accepted name", t.signal);
  assert.equal(await setupName(c, c.key), c.name);
  assert.equal((await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true)).status, 200);
  await until("the other member copied the revocation", () => b.read().shared?.membershipAndAccess?.value?.devices?.removed?.some(device => device.key === phone), t.signal);
  await a.stop();
  await b.stop("SIGTERM");
  const saved = b.read();
  // Drop the ordinary roster publication while retaining the certificate and the copied
  // setup state. The takeover must carry the accepted names.
  delete saved.docs?.[`${a.sign} peers`];
  delete saved.docs?.[`${a.sign} phones`];
  fs.writeFileSync(path.join(b.run, "link.json"), JSON.stringify(saved));
  b.start();
  reconnect.forEach(resume => resume());
  const lateClaim = leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  lateClaim();
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

test("failure bench: number blocks stay unique across a takeover", { timeout: 55_000 }, async t => {
  const { a, b, c, relay, see, leaseRelay } = await leasedNetwork(t);
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
  const first = await make(c, "Numbered by the old main");
  const firstNumber = await numbered(c, first.id);

  // The main stops; the third computer keeps numbering from its block while cut off from the new main.
  const reconnect = [relay.pause(b.address, c.address), relay.pause(c.address, b.address)];
  await a.stop();
  const offline = await make(c, "Uses the existing block offline");
  assert.equal(offline.number, firstNumber + 1);
  // (The always-on member takes over: the third computer is kept off the relay meanwhile.)
  const lateClaim = leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  lateClaim();
  const next = await make(b, "Numbered by the new main");
  const nextNumber = await numbered(b, next.id);

  reconnect.forEach(resume => resume());
  await see(c, 3, b);
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
  const managerKey = link.newKey();
  const desktopKey = link.newKey();
  await enrollDesktop(a, managerKey, "Managing desktop");
  const managerA = await deviceAt(relay, a, managerKey, { client: "desktop" });
  assert.ok(managerA.greeting);
  t.after(managerA.close);
  const desktop = { key: link.publicOf(desktopKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const enrolled = await a.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled));
  async function open(box, key, hello = { client: "desktop" }) {
    return until("fresh device session", async () => {
      const connection = await deviceAt(relay, box, key, hello);
      if (!connection.greeting) return false;
      t.after(connection.close);
      return connection;
    }, t.signal);
  }
  const manager = await open(b, managerKey);
  const desk = await open(b, desktopKey, { client: "desktop" });
  const configuration = path.join(b.home, "backup", "configuration.json");
  fs.mkdirSync(path.dirname(configuration), { recursive: true });
  fs.writeFileSync(configuration, JSON.stringify({ version: 1, enabled: true, source: "private-source", key: "private-key",
    dest: { kind: "disk", path: "/private/path" }, projects: ["/private/project"], last: { id: "private-id", at: 1234, version: 1 },
    error: "private error" }));
  for (const device of [manager, desk]) {
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
  assert.equal((await manager.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "Different grant" })).status, 409);
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
  const revoked = await managerA.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: enrolled.value.pairing }] });
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
  await until("the restored session owns its queue again", async () => (await editRead(c, alreadyRef)).status === 200,
    AbortSignal.any([t.signal, AbortSignal.timeout(12_000)]));
  assert.deepEqual((await editRead(c, deniedRef)).value.fields, {});
  await until("held bot receipt preserves the revoked action", async () => {
    const listed = await b.route("GET", "kuvo/bots");
    return listed.value.held?.find(entry => entry.bot === bot)?.waits?.includes("submitting device no longer has access");
  }, t.signal);
  assert.ok(fs.existsSync(path.join(botFolder, "bot.json")));
  assert.ok(!fs.existsSync(path.join(c.home, "attachments", "ses_owned")));
  assert.deepEqual((await editRead(c, oldRef)).value.fields, {});
  assert.deepEqual(prompts.map(e => e.id), ["msg_beforerevocation"]);

  const managerAgain = await open(b, managerKey);
  // An old managed draft stays visible until a deliberate edit replaces it with its own grant.
  const heldFile = path.join(b.run, "bots-held", bot + ".json");
  const oldBot = { id: "bte_abcdefghijklmnop", bot, to: c.key, change: { name: "Original draft", jobLine: "Old text" },
    times: { name: 1, jobLine: 1 }, since: 1, at: 1 };
  const key = crypto.createHmac("sha256", Buffer.from(b.read().secret, "base64url")).update("kuvo bot edits held").digest();
  const sealed = [oldBot.id, bot, c.key, null, oldBot.change, oldBot.times, false, 1, 1];
  fs.writeFileSync(heldFile, JSON.stringify({ ...oldBot, seal: crypto.createHmac("sha256", key).update(JSON.stringify(sealed)).digest("base64url") }));
  const reviewed = await managerAgain.ask("GET", "kuvo/bots");
  assert.match(reviewed.value.held.find(entry => entry.bot === bot).waits, /predates device tracking/);
  const resume = relay.pause(b.address, c.address);
  t.after(resume);
  const replaced = await managerAgain.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "New edit" });
  assert.equal(replaced.status, 202, JSON.stringify(replaced));
  assert.deepEqual(replaced.value.held.change, { name: "New edit" });
});

test("two desktops submit complete drafts to the conversation owner", { timeout: 110_000 }, async t => {
  const engineModels = {
    "/api/provider": [{ id: "deepseek" }, { id: "opencode" }],
    "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4", enabled: true, status: "active", cost: [{ input: 1, output: 2 }] }],
    "/api/integration": [{ id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential", method: "key" }] }],
    "/api/model/default": { providerID: "deepseek", id: "v4" },
  };
  const { a, b, c, relay, prompts, see, showOwnedConversation, tellOwned } = await network(t, true, undefined, true, undefined, false, false, undefined, { engineModels });
  await see(c, 2, a);
  await until("primary lease", () => a.lease(2, b), t.signal);
  const manager = link.newKey();
  const desk = link.newKey();
  await enrollDesktop(a, manager, "Managing desktop");
  const managerA = await deviceAt(relay, a, manager, { client: "desktop" });
  assert.ok(managerA.greeting);
  t.after(managerA.close);
  const paired = await a.route("POST", "kuvo/devices/pair", { key: link.publicOf(desk).toString("base64url"), name: "Desk", kind: "desktop" }, true);
  assert.equal(paired.status, 200, JSON.stringify(paired));
  const route = "kuvo/sessions/ses_owned/draft";
  const provider = engineModels["/api/integration"][0];
  const credentials = provider.connections;
  const available = engineModels["/api/provider"];
  const waitForModel = (device, runs) => until(`owner's model readiness is ${runs}`, async () => {
    const reply = await device.ask("GET", "kuvo/sessions/models?sessions=ses_owned");
    return reply.value?.sessions?.ses_owned?.runs === runs;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(12_000)]));
  for (const [key, hello, id] of [[manager, { client: "desktop" }, "msg_draftfirst"], [desk, { client: "desktop" }, "msg_draftdesk"]]) {
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
    provider.connections = [];
    engineModels["/api/provider"] = available.filter(each => each.id !== provider.id);
    tellOwned({ type: "credential.updated" });
    await waitForModel(device, false);
    const refused = await device.ask("POST", route, { id: id + "stale", text: "Did readiness change?", message: {} });
    assert.equal(refused.value.answer?.restore?.text, "Did readiness change?", JSON.stringify(refused));
    provider.connections = credentials;
    engineModels["/api/provider"] = available;
    tellOwned({ type: "credential.updated" });
    await waitForModel(device, true);
  }
  const member = await until("paired session on the listing Box", async () => {
    const connection = await deviceAt(relay, b, manager, { client: "desktop" });
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
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftfirst", "msg_draftdesk"]);
  permit();
  await until("the owner accepts the queued full draft", async () => {
    const receipt = await b.route("GET", `kuvo/data/messages/${waits.value.operation}`);
    return receipt.value.status === "committed";
  }, t.signal);
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftfirst", "msg_draftdesk", "msg_ownerdraft"]);
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
  const recovered = await managerA.ask("GET", recover + "/" + pinned.submission);
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
  const changed = await managerA.ask("POST", submit, { ...pinned, text: "Changed intent on another lister" });
  assert.equal(changed.status, 202, JSON.stringify(changed));
  let changedReceipt;
  await until("changed intent refused by the original owner", async () => {
    const receipt = await managerA.ask("GET", "kuvo/data/messages/" + changed.value.operation);
    if (receipt.value.status !== "interrupted") return false;
    changedReceipt = receipt.value;
    return true;
  }, t.signal);
  const conflict = await managerA.ask("GET", recover + "/" + pinned.submission);
  assert.equal(conflict.value.outcomes[0]?.conflict, true);
  const different = await managerA.ask("GET", recover + "/" + pinned.submission + "?digest=" + changedReceipt.digest);
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
  const revoked = await managerA.ask("POST", "kuvo/phones/remove", { phones: [{ key: link.publicOf(desk).toString("base64url"), pairing: paired.value.pairing }] });
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
  const hidden = await managerA.ask("GET", recover + "/" + parked.submission);
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

test("failure bench: setup preferences merge at the main and survive a takeover", { timeout: 55_000 }, async t => {
  const { a, b, c, leaseRelay } = await leasedNetwork(t);
  const ref = { kind: "setupUsageDisplay" };
  const read = await editRead(c, ref);
  assert.equal(read.status, 200, JSON.stringify(read));
  const { revision: baseRevision } = read.value;
  const first = await editSubmit(c, ref, { operation: "usage-card", baseRevision, fields: { totalSpend: { show: true } } });
  assert.ok(first.status === 200 || first.status === 202, JSON.stringify(first));
  assert.equal((await acceptedEdit(c, first.value.operation, t)).receipt.status, "committed");
  const second = await editSubmit(c, ref, { operation: "usage-model", baseRevision, fields: { codexFallback: "gpt-5.4" } });
  assert.equal((await acceptedEdit(c, second.value.operation, t)).receipt.status, "committed");
  const actual = await editRead(a, ref);
  assert.deepEqual(actual.value.fields, { codexFallback: "gpt-5.4", totalSpend: { show: true } });
  const third = await editSubmit(c, ref, { operation: "usage-other-model", baseRevision, fields: { codexFallback: "gpt-5.3-codex" } });
  const conflict = (await acceptedEdit(c, third.value.operation, t)).receipt;
  assert.equal(conflict.status, "choice");
  await until("the members copied the choice", () => [b, c].every(box => box.read().shared?.setupUsageDisplay?.offlineEdits?.choices?.length), t.signal);
  const copies = [a, b, c].map(box => box.read().shared?.setupUsageDisplay?.offlineEdits?.choices);
  assert.equal(copies[0][0].id, conflict.choices[0]);
  assert.deepEqual(copies[0], copies[1]);
  const current = await editRead(c, ref);
  assert.deepEqual(current.value.choices, copies[0]);
  await a.stop();
  leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await until("the always-on member takes over with the accepted preference choices", () => b.read().setup?.role === "primary" && b.read().setup?.epoch === 3, t.signal);
  const after = await editRead(b, ref);
  assert.deepEqual(after.value.fields, actual.value.fields);
  assert.deepEqual(after.value.choices, copies[0]);
});

test("failure bench: a bot's offline name and instructions are applied only at its home", { timeout: 45_000 }, async t => {
  const id = "bot_Lab";
  const { a, b, c, relay } = await network(t, false, undefined, true, undefined, false, false, undefined, {
    prepare: ([, , home]) => {
      const folder = path.join(home.home, "bots", id);
      fs.mkdirSync(path.join(home.home, "bin"), { recursive: true });
      fs.copyFileSync(new URL("../src/shared/bot-words.json", import.meta.url), path.join(home.home, "bin", "bot-words.json"));
      put(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name: "Lab bot" }));
      put(path.join(folder, "session"), "ses_owned\n");
      put(path.join(folder, "job.json"), JSON.stringify({ line: "Report status", details: "Check the queue." }));
    },
  });
  const ref = { kind: "botSettings", id, computer: c.key };
  await until("both computers know the bot's home", () => [a, b].every(box => peerOwns(box, c, id)), t.signal);
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

function separateExportRoot(t, source) {
  if (process.platform === "darwin") {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-export-"));
    const image = path.join(root, "archive.dmg");
    const mount = path.join(root, "mounted");
    let attached = false;
    const diskImage = args => {
      const ran = spawnSync("/usr/bin/hdiutil", args, { encoding: "utf8", timeout: 20_000 });
      assert.equal(ran.status, 0, ran.error?.message || ran.stderr || String(ran.signal));
    };
    t.after(() => {
      if (attached) diskImage(["detach", mount]);
      fs.rmSync(root, { recursive: true, force: true });
    });
    fs.mkdirSync(mount);
    // APFS avoids hdiutil's deprecated HFS+ creation path.
    diskImage(["create", "-size", "64m", "-fs", "APFS", "-volname", "Kuvo Export Fixture", image]);
    diskImage(["attach", "-nobrowse", "-noautoopen", "-owners", "off", "-mountpoint", mount, image]);
    attached = true;
    assert.notEqual(fs.statSync(mount).dev, fs.statSync(source.home).dev);
    return fs.realpathSync(mount);
  }
  const root = fs.mkdtempSync("/dev/shm/kuvo-removal-fixture-");
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  assert.notEqual(fs.statSync(root).dev, fs.statSync(source.home).dev);
  return root;
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

for (const scenario of ["headless member", "app-owned Mac with a shutdown write", "app-owned Mac with a lost export"]) test(
  `failure bench: revoked Relay-only ${scenario} accepts a verified removal export after reconnect`,
  { timeout: 160_000 }, async t => {
    const appOwner = scenario.startsWith("app-owned Mac");
    const lostExport = scenario.endsWith("lost export");
    const { a, b: receiver, c: source, relay } = await network(t, false, undefined, false, undefined, true, false, undefined, { appOwner, acceptedRoster: true });
    const backup = async (action, body) => {
      const route = "kuvo/box/backups/" + action;
      if (!appOwner) {
        const reply = await source.command(["box", "backups", action], JSON.stringify(body ?? {}));
        assert.equal(reply.status, 0, reply.err);
        assert.equal(reply.value.recoveryKey, undefined);
        return { status: reply.value.status, value: reply.value };
      }
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
    const configFile = path.join(source.home, "backup", "configuration.json");
    assert.equal(fs.statSync(configFile).mode & 0o777, 0o600);
    const ownedConfig = JSON.parse(fs.readFileSync(configFile, "utf8"));
    assert.equal(typeof ownedConfig.key, "string");
    const phone = link.newKey();
    const invite = await a.route("POST", "kuvo/phones/pair");
    const first = await deviceAt(relay, a, phone, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Removal fixture" });
    assert.ok(first.greeting);
    t.after(first.close);
    const open = box => until("paired phone reaches " + box.name, async () => {
      const client = await deviceAt(relay, box, phone);
      return client.greeting && client;
    }, t.signal);
    let initiator = await open(receiver);
    t.after(() => initiator.close());
    const sourceClient = await open(source);
    const remoteHealth = await sourceClient.ask("GET", "kuvo/box/backups/status");
    assert.deepEqual(remoteHealth.value, { enabled: true, destinationKind: "computer", destinationAvailable: true,
      lastVerifiedAt: daily.value.point.at, attention: false });
    assert.equal((await sourceClient.ask("POST", "kuvo/box/backups/destination", { destination, confirm: true })).status, 403);
    assert.equal((await sourceClient.ask("GET", "kuvo/box/backups")).status, 403);
    sourceClient.close();
    const setup = a.read().setup.id;
    const deposit = path.join(receiver.env.HOME, ".kuvo-backups", "relay", setup, source.key, configured.value.source, daily.value.point.id + ".kuvo");
    const original = fs.readFileSync(deposit);
    fs.writeFileSync(data[0], "saved after the daily backup");
    await source.stop();
    await until("source offline", () => !relay.has(source.address), t.signal);
    let operation = operationID();
    const started = await initiator.ask("POST", wipeRoute(source), { operation });
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
    const diskParent = separateExportRoot(t, source);
    const target = { kind: "disk", path: path.join(diskParent, "new-export") };
    if (appOwner) source.onShutdown = `
      import fs from "node:fs";
      process.once("SIGTERM", () => {
        if (fs.existsSync(${JSON.stringify(target.path)})) {
          ${lostExport ? `fs.renameSync(${JSON.stringify(target.path)}, ${JSON.stringify(target.path + "-away")});` :
            `fs.writeFileSync(${JSON.stringify(data[0])}, "saved while stopping");`}
        }
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
      AbortSignal.any([t.signal, AbortSignal.timeout(35_000)]));
    const pending = await a.route("POST", wipeRoute(source), { operation: started.value.operation });
    assert.equal(pending.value.removal.state, "review");
    assert.equal(pending.value.removal.nextAction, "choose-export");
    assert.match(pending.value.message, /Relay.*separate disk or SSH/);
    assert.equal((await receiver.route("GET", "kuvo/computers")).value.wipePending[0].nextAction, "choose-export");
    assert.ok(data.every(file => fs.existsSync(file)));
    assert.equal((await deviceAt(relay, source, phone)).refused, true);
    let supersedes;
    if (!appOwner) {
      const disabled = await backup("destination", { enabled: false });
      assert.equal(disabled.value.enabled, false);
      assert.deepEqual(fs.readFileSync(deposit), original);
      const originalGrant = plainGrant;
      assert.ok(originalGrant);
      const after = relay.delivered.length;
      relay.replay(originalGrant);
      await until("fresh plain wipe instruction still held", () => plainGrant !== originalGrant, t.signal);
      await until("source repeated its refusal", () => relay.delivered.slice(after).some(packet =>
        packet.to.equals(a.address) && removalMessage(packet, a, source)?.review === "choose-export"), t.signal);
      assert.ok(data.every(file => fs.existsSync(file)));
      assert.equal(source.read().localWipe, undefined);
      const sameVolume = fs.mkdtempSync(path.join(source.env.HOME, "same-disk-"));
      const rejected = await a.route("POST", wipeRoute(source), { operation, prepare: {
        export: { destination: { kind: "disk", path: sameVolume }, recoveryKey: crypto.randomBytes(32).toString("base64url") },
      } }, true);
      assert.equal(rejected.status, 200);
      const refusal = await until("same-disk export refused", () => removalOf(a, source)?.error, t.signal);
      assert.match(refusal, /separate disk/);
      assert.deepEqual(fs.readdirSync(sameVolume), []);
      supersedes = operation;
      operation = operationID();
    }
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    const remoteRevision = await initiator.ask("POST", wipeRoute(source), { operation, prepare: { export: { destination: target, recoveryKey } } });
    assert.equal(remoteRevision.status, 403);
    const select = () => a.route("POST", wipeRoute(source), { operation, supersedes, prepare: { export: { destination: target, recoveryKey } } }, true);
    assert.equal((await select()).value.removal.state, "exporting");
    await until("unavailable destination reported", () => removalOf(a, source)?.error, t.signal);
    assert.match(removalOf(a, source).error, /selected export disk is unavailable/);
    assert.equal(removalOf(a, source).error.includes(target.path), false);
    assert.ok(data.every(file => fs.existsSync(file)));
    await a.stop();
    await until("primary connection gone", () => !relay.has(a.address), t.signal);
    a.start();
    await until("primary restarted", () => relay.has(a.address), t.signal);
    assert.equal(removalOf(a, source)?.operation, operation);
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
      await until("shutdown invalidated readiness", () =>
        (lostExport ? /selected export disk is unavailable/ : /Source work changed/).test(removalOf(a, source)?.error ?? ""), t.signal);
      assert.equal(removalOf(a, source).delivered, undefined);
      assert.equal(fs.readFileSync(data[0], "utf8"), lostExport ? "saved after the daily backup" : "saved while stopping");
      if (lostExport) fs.renameSync(target.path + "-away", target.path);
      const again = await a.route("POST", wipeRoute(source), { operation, retryPreparation: true }, true);
      assert.equal(again.status, 200, JSON.stringify(again.value));
    }
    await waitForWipe(source, t.signal, 30_000);
    await until("completion sent after erase", () => done, t.signal);
    assert.equal(removalOf(a, source).delivered, undefined);
    if (lostExport) {
      await a.stop();
      await until("primary offline after detach", () => !relay.has(a.address), t.signal);
      a.start();
      await until("primary resumed the removal", () => relay.has(a.address), t.signal);
      assert.equal(removalOf(a, source)?.operation, operation);
      assert.equal(removalOf(a, source)?.delivered, undefined);
    }
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
    assert.equal(link.peersOf(receiver.read()).some(peer => peer.key === source.key), false);
    const ended = await a.route("POST", wipeRoute(source), { operation });
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
    assert.equal(bytes.value["messages.json"], Buffer.from(appOwner && !lostExport ? "saved while stopping" : "saved after the daily backup").toString("base64"));
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
  const review = await a.route("POST", wipeRoute(source), { operation: started.value.operation });
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
  assert.deepEqual(await a.route("POST", wipeRoute(b), { operation: started.value.operation }), lost);
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
  // What each computer's Remove offers is the box's: unlink or wipe for a member, and for the main
  // of more, who takes its role first; the last computer can only be wiped.
  const offered = async key => (await a.route("GET", "kuvo/computers")).value.computers.find(each => each.key === key)?.actions.find(action => action.id === "remove")?.removal;
  assert.deepEqual(await offered(b.key), { choices: ["unlink", "wipe"] });
  const main = await offered(a.key);
  assert.deepEqual(main?.choices, ["unlink", "wipe"]);
  assert.match(main?.handOver ?? "", / becomes the main computer first\. /);
  for (const peer of [b, c]) {
    const result = await a.route("POST", `kuvo/computers/${peer.key}/unlink`);
    assert.equal(result.status, 200, JSON.stringify(result.value));
    assert.equal(result.value.offer, undefined);
  }
  phase("both member unlink decisions saved");
  await until("both unlinks delivered", () => removalOf(a, b)?.delivered && removalOf(a, c)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(8500)]));
  phase("both member unlink deliveries acknowledged");
  assert.deepEqual(await offered(a.key), { choices: ["wipe"], last: true });
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
  assert.deepEqual(receipt, { setup, operation: last.value.operation, formatVersion: 1 });
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
  assert.equal((await a.route("POST", wipeRoute(a), { operation: first.value.operation })).value.removal.state, "cancelled");
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
  await until("candidate learns the pending wipe before taking over", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
  assert.equal((await submit(a, c, 2)).status, 202);
  await Promise.all([see(a, 3, c), see(c, 3, c)]);
  await until("new primary installs the pending wipe", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
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
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
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
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
  assert.equal(groupCopies.get("Studio")?.group.id, group.id);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an offline Relay source completes its accepted moves and export on reconnect", { timeout: 150_000 }, async t => {
  const group = { id: "grp_offlinemove", name: "Offline move", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, relay, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
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
  const review = await a.route("POST", wipeRoute(c), { operation: first }, true);
  assert.equal(review.status, 200, JSON.stringify(review.value));
  assert.equal(review.value.removal.nextAction, "revise-export");
  assert.equal(groupCopies.get("Lab")?.status, "active");
  const operation = operationID();
  const destination = { kind: "disk", path: separateExportRoot(t, c) };
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
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
  assert.equal(groupCopies.get("Desk")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an encrypted export survives wipe and imports through restore", { timeout: 120_000 }, async t => {
  // The outer deadline covers setup, real uploads, prepare, detach and a fresh restore.
  // Each destructive handoff below still has its own tighter deadline.
  const startedAt = performance.now();
  const { a, b: replacement, c: owner, prompts, phase } = await network(t, false, undefined, true, undefined, true, false, undefined,
    { phaseDiagnostics: true });
  const milestone = description => phase(`${description} (+${Math.round(performance.now() - startedAt)} ms)`);
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
  milestone("session and queue uploads saved and their source bytes verified");
  const destination = { kind: "disk", path: path.join(owner.env.TMPDIR, "manual-export") };
  fs.mkdirSync(destination.path);
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(owner), { operation, prepare: { export: { destination, recoveryKey } } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "exporting");
  milestone("removal plan saved with export requested");
  await waitForPreparedRemoval(a, owner, operation, AbortSignal.any([t.signal, AbortSignal.timeout(25_000)]));
  milestone("prepared export acknowledged by the primary");
  // Allow the next Relay offer, export readback and the worker's writer-stop window.
  await waitForWipe(owner, t.signal, 30_000);
  milestone("source installation detached");
  assert.ok(data.every(file => !fs.existsSync(file)));
  await until("verified wipe accepted", () => removalOf(a, owner)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]));
  milestone("source wipe completion recorded by the primary");
  const retry = await a.route("POST", wipeRoute(owner), { operation });
  assert.equal(retry.status, 200, JSON.stringify(retry.value));
  const poll = await a.route("POST", wipeRoute(owner), { operation: start.value.operation });
  assert.deepEqual(poll, retry);
  const { source, id } = retry.value.export;
  const archive = fs.readFileSync(path.join(destination.path, "kuvo-backups", source, id + ".kuvo"));
  assert.ok(archive.subarray(0, 14).equals(Buffer.from("KUVO backup 1\n")));
  assert.equal(archive.includes("local work"), false);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: owner.key, replacement: replacement.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  milestone("primary committed the replacement match");
  await until("replacement ownership replica", () => replacement.read().shared?.ownershipRecovery?.value?.["replacement:" + owner.key],
    AbortSignal.any([t.signal, AbortSignal.timeout(8_000)]));
  milestone("approved replacement mapping replicated");
  const restored = await replacement.route("POST", "kuvo/box/backups/restore", { destination, source, id, recoveryKey, confirm: true }, true);
  assert.equal(restored.status, 200, JSON.stringify(restored.value));
  assert.equal(fs.readFileSync(path.join(replacement.home, "bots", "bot_retained", "notes.md"), "utf8"), "local work");
  assert.deepEqual(restored.value.unavailable.filter(({ entry }) => entry === queueItem ||
    entry.startsWith("attachments/") || entry.startsWith("home/run/work-queue/files/")), []);
  for (const [file, bytes] of promised) assert.deepEqual(fs.readFileSync(path.join(replacement.home, file)), bytes, file);
  milestone("replacement restored and all promised bytes verified");
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
    const receipt = updateAccess(link => {
      const record = link.memberRemovals[${JSON.stringify(source.key)}];
      record.generation = ${JSON.stringify(message.progress.generation)};
      record.export = ${JSON.stringify(message.progress.export)};
      delete record.error;
      return link;
    }, { checkpoint: true }).save;
    // The main saves alone: the receipt is final.
    console.log(JSON.stringify(receipt));
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
  assert.match(failure, /source or export path is unavailable/);
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
  await until("candidate learns the export preparation before taking over", () => removalOf(c, b)?.operation === operation, t.signal);
  const transfer = await submit(a, c, 2);
  assert.equal(transfer.status, 202, JSON.stringify(transfer.value));
  await see(c, 3, c);
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

test("failure bench: move-only wipe waits for an explicitly revised, byte-verified queue plan", { timeout: 120_000 }, async t => {
  // Setup and route subprocesses consume part of the outer budget on slower hosts.
  const startedAt = performance.now();
  const group = { id: "grp_badfiles", name: "Files for removal", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, relay, phase } = await network(t, false, undefined, false, group, true, false, undefined,
    { phaseDiagnostics: true });
  const milestone = description => phase(`${description} (+${Math.round(performance.now() - startedAt)} ms)`);
  milestone("setup connected");
  const bytes = Buffer.of(0, 255, 1, 7);
  const { id: item, paths: [attachment] } = await uploadQueueFiles(c, { "payload.lock": bytes }, group.id);
  milestone("queue attachment uploaded");
  const source = path.join(c.home, attachment);
  const planRoute = `kuvo/computers/${c.key}/removal-plan`;
  for (const corrupted of [bytes.subarray(0, 1), Buffer.of(0, 255, 1, 8)]) {
    fs.writeFileSync(source, corrupted);
    const plan = await a.route("POST", planRoute, {}, true);
    assert.equal(plan.status, 200, JSON.stringify(plan.value));
    assert.equal(plan.value.plan.complete, false);
    assert.deepEqual(plan.value.plan.files.filter(file => file.status === "unavailable"), [{ item: group.id, name: "payload.lock", status: "unavailable" }]);
  }
  milestone("both corrupt previews marked the attachment unavailable");

  const operation = operationID();
  const prepare = { moveTo: a.key };
  assert.equal((await a.route("POST", wipeRoute(c), { operation, prepare }, true)).status, 200);
  const refused = await until("unavailable attachment blocks removal", () => {
    const kept = removalOf(a, c);
    return kept?.error && kept;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  assert.deepEqual(refused.unresolved, [group.id]);
  assert.equal(refused.generation, undefined);
  assert.equal(c.read().localWipe, undefined);
  assert.equal(fs.existsSync(source), true);
  milestone("first removal refused with the source intact");

  fs.writeFileSync(source, bytes);
  const revised = operationID();
  const readinessChecks = new Set();
  let checkedBeforeErase = false;
  const stopWatching = relay.intercept(c.address, a.address, packet => {
    const check = removalMessage(packet, a, c);
    const nonce = check?.moveFilesCheck;
    if (checkedBeforeErase || check?.operation !== revised || typeof nonce !== "string" || readinessChecks.has(nonce)) return false;
    // Separate preparation retries from the check made by the approved wipe worker.
    const wipe = c.read().localWipe;
    if (!wipe) readinessChecks.add(nonce);
    else if (wipe.operation === revised && wipe.prepared) checkedBeforeErase = true;
    return false;
  });
  t.after(stopWatching);
  const accepted = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare }, true);
  assert.equal(accepted.status, 200, JSON.stringify(accepted.value));
  milestone("revised removal accepted for preparation");
  await waitForPreparedRemoval(a, c, revised, AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  assert.ok(readinessChecks.size > 0, "readiness checked the destination bytes");
  milestone("revised removal prepared after the destination check");
  await waitForWipe(c, t.signal, 30_000);
  assert.equal(fs.existsSync(source), false);
  assert.ok(checkedBeforeErase, "the wipe worker checked the destination using a fresh nonce");
  milestone("source erased after a fresh destination check");

  const opened = await a.command(["box", "route"], JSON.stringify({ method: "GET", path: `kuvo/work-queue/items/${item}/files/payload.lock` }));
  assert.equal(opened.status, 0, opened.err);
  assert.equal(opened.value.status, 200, JSON.stringify(opened.value));
  assert.deepEqual(Buffer.from(opened.value.bodyBase64, "base64"), bytes);
  milestone("original attachment bytes reopened from the destination");
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
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
  const data = seedLocalWork(c, { botWork: false });
  const operation = operationID();
  const original = path.join(c.env.TMPDIR, "missing-disk");
  const replacement = fs.mkdtempSync(path.join(c.env.TMPDIR, "replacement-disk-"));
  const prepare = folder => ({ export: { destination: { kind: "disk", path: folder }, recoveryKey: crypto.randomBytes(32).toString("base64url") } });
  const first = await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key, ...prepare(original) } }, true);
  assert.equal(first.status, 200);
  await until("first recipe stalls after the transfer", () => removalOf(a, c)?.error, t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.deepEqual(preparation(c, "source", operation).moves, [{ id: group.id, kind: "group", completed: true, verified: true, sessions: [] }]);
  const revised = operationID();
  const second = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare: prepare(replacement) }, true);
  assert.equal(second.status, 200, JSON.stringify(second.value));
  const poll = await a.route("POST", wipeRoute(c), { operation });
  assert.equal(poll.value.removal.state, "cancelled");
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  await waitForWipe(c, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
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

function writeLocalPersonalities(box, document) {
  const folder = path.join(box.home, "bots");
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, "personalities.json"), JSON.stringify({ kuvo: 1, ...document }));
}

for (const populated of [false, true]) test(
  `failure bench: retired pc5 Insights origin survives restore into a ${populated ? "populated" : "fresh"} index`,
  { timeout: 105_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const { a, b, c, boxes: [, , , replacement], relay } = await network(t, false, undefined, false, undefined,
      false, false, ["pc2", "pc4", "Original pc5", "Replacement"], { acceptedRoster: true });
    const backup = async (box, action, body) => {
      const answer = await box.route("POST", "kuvo/box/backups/" + action, body, true);
      assert.equal(answer.status, 200, JSON.stringify(answer.value));
      return answer.value;
    };
    const wait = (what, fn, timeout = 15_000) => until(what, fn, AbortSignal.any([t.signal, AbortSignal.timeout(timeout)]));
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
    const checkHistory = async (box, afterRestore = false) => {
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
      if (afterRestore) {
        assert.deepEqual(detail.destination, { status: "unavailable" }, "a restore match alone does not locate the archived conversation");
        assert.equal(detail.calls[0].attributed, true);
      }
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
    await wait("pc4 services knows the current pc5 peer", async () =>
      (await overview(surviving)).computers.some(row => row.key === c.key), 35_000);
    // Seed the survivor through the authenticated wire, independently of whether the
    // two newly started readers have both subscribed before their first automatic pull.
    await deliver(sendPacket(c, b, c.read(), { kind: "insights", action: "page", after: 0, next: 1,
      epoch: "original", more: false, records: [legacy] }), c, b);
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
      return state.shared?.ownershipRecovery?.value?.[`replacement:${c.key}`]?.computer === replacement.key &&
        link.seatPeers(state, {}).insightOrigins?.some(origin => origin.key === c.key);
    }));
    const restored = await backup(replacement, "restore", { destination, source, id: point.id, recoveryKey, confirm: true });
    assert.equal(restored.unavailable.some(entry => entry.entry === "agents/usage/insights.sqlite"), false);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", record.key).computer, c.key);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", clonedAlias.key), undefined);
    if (populated) assert.equal(cursor(replacement), 90, "restore preserves the live index's wire cursor for scoped rewind");

    await checkHistory(surviving);
    const recovering = await services(a);
    // The link can be in its discovery backoff when a services reader starts after the restore.
    await wait("pc2 backfilled its skipped survivor page", async () => (await overview(recovering)).totals.calls === 1, 35_000);
    assert.notEqual(cursor(a), 90);
    await checkHistory(recovering);

    // The provider readings below are scripted. The idle readers in the fixture
    // would otherwise advertise their empty sign-in lists during reconnection.
    const idleLimits = payload => payload?.limits || payload?.seat?.kind === "limits";
    const unintercept = [
      relay.intercept(a.address, replacement.address, packet => idleLimits(opened(packet, replacement))),
      relay.intercept(b.address, replacement.address, packet => {
        const payload = opened(packet, replacement);
        return idleLimits(payload) || payload?.seat?.kind === "insights";
      }),
    ];
    let recovered = await services(replacement);
    await wait("replacement services knows retired pc5", async () => (await overview(recovered)).computers.some(row => row.key === c.key));
    await wait("scanner reconsidered the restored clone transcript", () =>
      query(replacement, "SELECT key FROM event WHERE session = ? AND computer = ?", cloneSession, alias.out));
    assert.equal((await overview(recovered, `&session=${cloneSession}`)).calls.length, 0);
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
    await checkHistory(recovered, true);

    const checked = Date.now() - 1_000;
    const limit = (used, read) => ({ kind: "limits", at: Date.now(), accounts: [{ provider: "claude-code", service: "claude",
      identity: "aabbccddeeff0011", checked: read, windows: [{ id: "five-hour", used, read, minutes: 300 }] }] });
    await deliver(sendPacket(a, replacement, a.read(), limit(0.2, checked - 1000)), a, replacement);
    await deliver(sendPacket(b, replacement, b.read(), limit(0.6, checked)), b, replacement);
    relay.replay(sendPacket(c, replacement, oldCredential, limit(0.9, checked + 500)));
    const checkLimits = async () => {
      const row = (await recovered.get("limits")).rows.find(row => row.id === record.account);
      assert.ok(row, "current peers still supply the provider reading");
      assert.equal(row.usage.updatedAt, checked);
      assert.equal(row.usage.meters[0].share, 0.6);
      assert.deepEqual(row.usage.computers, [{ name: b.name, here: false }]);
      assert.ok(row.boxes.every(box => box.key !== c.key));
    };
    await checkLimits();
    await recovered.stop();
    await replacement.stop();
    replacement.start();
    recovered = await services(replacement);
    await wait("restarted replacement rejoined the relay", () => relay.has(replacement.address));
    await wait("restarted replacement services knows the accepted history", async () =>
      (await overview(recovered)).computers.some(row => row.key === c.key), 35_000);
    await deliver(second, b, replacement);
    assert.equal(cursor(replacement), 2);
    await checkHistory(recovered, true);
    await checkLimits();
    unintercept.forEach(resume => resume());
  });

test("failure bench: a mixed-build update cut and lost move reply survive disk loss, restore, and replay",
  { timeout: 210_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const session = "ses_owned";
    const recovery = { build: recoveryNeighborBuild(), session };
    const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
    const pin = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
    const { a, b, c, relay, see, leaseRelay, fixtureEngine, phase } = await network(t, true, pin, false, undefined, false, false,
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
    await until("pc4 keeps the main's decision", () => b.read().sharedOutcomes?.[saved.operation]?.status === "committed", t.signal);
    const receiptFile = path.join(root, "client-receipt.json");
    put(receiptFile, JSON.stringify(saved));
    const bytes = Buffer.of(0, 255, 3, 128);
    const { id: item, paths: [attachment] } = await uploadQueueFiles(a, { "recovery.lock": bytes });
    await until("pc2 has listed its session", () => peerOwns(b, a, session), t.signal);
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
      const first = !inbound.has(move.op);
      if (first) inbound.set(move.op, packet);
      // pc4 activates from pc2's first switch: the hand-over is between the two computers.
      // Every switch delivered after that comes from the replay harness, so its answers are causal.
      return move.op === "switch" && !first;
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
    put(path.join(a.home, "bin", "kuvo-start"), `#!/bin/sh\n# Kuvo release switch v1\nV=${release.engine}\nexit 0\n`);
    const runner = path.join(a.home, "engine", release.engine, "opencode");
    put(runner, `#!/bin/sh\nBUN_BE_BUN=1 exec '${bun.replaceAll("'", "'\\''")}' "$@"\n`);
    fs.chmodSync(runner, 0o700);
    put(path.join(a.run, "engine.json"), JSON.stringify({ ...read(path.join(a.run, "engine.json")), version: release.engine }));
    // Cut the updater as the verified release lands beside the running one, before it says it's
    // ready for the main's switch. (The updater runs on the engine's Bun, as the keeper starts it.)
    const cut = path.join(root, "update-cut.mjs");
    put(cut, `
      import fs from "node:fs";
      import path from "node:path";
      const rename = fs.renameSync;
      fs.renameSync = (from, to) => {
        rename(from, to);
        if (/\\/versions\\/${release.version.replaceAll(".", "\\.")}-[0-9a-f]{12}\\/bin$/.test(String(to))) {
          const dir = fs.openSync(path.dirname(String(to)), "r");
          fs.fsyncSync(dir);
          fs.closeSync(dir);
          process.kill(process.pid, "SIGKILL");
        }
      };
    `);
    const interrupted = await a.evaluate(bun, [
      "--config=/dev/null", "--no-env-file", "--preload", cut,
      script, "update", "--now", "--from", release.folder, "--key", release.key,
    ]);
    assert.equal(interrupted.signal, "SIGKILL", JSON.stringify(interrupted));
    assert.ok(fs.readdirSync(path.join(a.home, "versions")).some(name => name.startsWith(release.version + "-")));
    assert.equal(fs.readFileSync(path.join(a.home, "bin", "box-version"), "utf8"), "0.8.0", "pc2 still runs its own version");
    assert.equal(fs.existsSync(path.join(a.run, "previous")), false, "nothing switched");
    assert.equal(read(staged).complete, true);
    phase("signed release cut while it was prepared beside the running one, with the move still held");

    relay.replay(arrived);
    await until("pc2 sent its switch and pc4's activation reply was dropped",
      () => inbound.has("switch") && switchedReplies > 0, t.signal);
    const decisionFile = path.join(b.run, "moves", "decisions", move.id + ".json");
    const decision = read(decisionFile);
    assert.equal(decision.as, "switched");
    const owner = () => read(path.join(b.run, "moves", "moved.json")).find(entry => entry.session === session);
    assert.equal(owner()?.key, b.key);
    assert.equal(owner()?.arrived, true);
    assert.equal(b.read().shared?.ownershipAssignments?.value?.[`session computer:${session}`], undefined);
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
    phase("pc2 disk erased with its prepared release and move journals unresolved");
    leaseRelay.advance(125_000);
    await see(b, 3, b);
    const decided = b.read().sharedOutcomes?.[read(receiptFile).operation];
    assert.equal(decided?.status, "committed", JSON.stringify(decided));
    assert.equal(decided.kind, "setupNames");
    assert.equal(await setupName(b, c.key), name);
    phase("pc4 elected and the client receipt resolved from its durable replica");

    unpause.forEach(resume => resume());
    // A build from before the relay lease takes a certificate only from its main (or that main's
    // standby, which the converted setup no longer has), so it can't follow a takeover. pc5 follows
    // pc4 once it runs a build that knows the lease.
    await c.stop();
    fs.copyFileSync(path.join(root, "kuvo-link.mjs"), path.join(root, "neighbor.mjs"));
    c.start();
    await until("pc5 follows the replacement primary", () => c.read().setup.primary === b.key && b.lease(), t.signal);
    phase("pc5, on this build now, follows pc4 at its relay term");
    good(await b.route("DELETE", `kuvo/computers/${a.key}`));
    const restore = { destination, source, id: point.id, recoveryKey, confirm: true };
    assert.match(good(await backup(c, "restore", restore), 409).message, /Confirm the replacement on the main computer/);
    good(await backup(b, "match", { source, previous: a.key, replacement: c.key }));
    await until("pc5 sees its approved replacement mapping", () => c.read().shared?.ownershipRecovery?.value?.["replacement:" + a.key], t.signal);
    await until("pc5 heard pc4 announce the conversation it owns", () => peerOwns(c, b, session), t.signal);
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
    // pc2 is a historical origin on pc5 now, so pc5 reads pc4's stream again from zero.
    const reset = insightsPage(c, 0, 2, "reindexed", true);
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
    // The new main numbers from its own counter, with pc2 gone.
    const next = await make(b, "numbered-on-successor");
    const nextNumber = await numbered(b, next.id);
    assert.ok(![itemNumber, firstNumber, offline.number].includes(nextNumber));
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
    assert.equal(owner()?.key, b.key);
    phase("replayed move and edits kept one import, one wake, the deletion, and distinct numbers");
  });

async function relayLeaseNetwork(t, { ranked = false, standby = true, oldMember = false, resident = false } = {}) {
  const leaseRelay = relayLeaseStandIn();
  const net = await network(t, true, undefined, false, undefined, false, true, undefined, {
    leaseRelay,
    oldMember,
    resident,
    acceptedRoster: true,
    prepare([a, b]) {
      for (const box of [a, b]) {
        const current = box.read();
        if (standby) Object.assign(current.setup, { standby: { key: b.key, sign: b.sign }, numberCeiling: 500 });
        if (ranked) current.setup.successorChain = true;
        current.shared.queueNumberAllocation = { sequence: 1, value: { next: 401 } };
        fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify(current));
      }
    },
  });
  return { ...net, leaseRelay };
}

test("relay lease: a two-computer main pairs a phone and adds a computer with the other stopped", { timeout: 35_000 }, async t => {
  const { a, b, relay, join } = await relayLeaseNetwork(t);
  await b.stop();
  a.clock(65_000);

  const pairing = await a.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200, JSON.stringify(pairing.value));
  const token = new URL(pairing.value.link).searchParams.get("token");
  const key = link.newKey();
  const phone = await deviceAt(relay, a, key, { name: "New phone", pair: token });
  assert.equal(phone.greeting?.save?.status, "committed", JSON.stringify(phone));
  t.after(() => phone.close());
  const pairedKey = link.publicOf(key).toString("base64url");
  assert.ok(a.read().shared.membershipAndAccess.value.devices.phones.some(phone => phone.key === pairedKey));

  const invitation = await a.route("POST", "kuvo/computers/add/code", {}, true);
  assert.equal(invitation.status, 201, JSON.stringify(invitation.value));
  const enrolled = await join(invitation.value.code);
  assert.equal(enrolled.setup.id, a.read().setup.id);
  assert.equal(enrolled.setup.primary, a.key);
  assert.ok(a.read().shared.membershipAndAccess.value.boxes.some(box => box.sign === link.signingKeyOf(enrolled.sign)));
  assert.equal(b.child.signalCode, "SIGKILL");
});

test("relay lease: restart grace keeps the same main even after its old grant expires", { timeout: 22_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  await until("the main's first relay grant", () => leaseRelay.lease, AbortSignal.any([t.signal, AbortSignal.timeout(5_000)]));
  assert.match((await a.route("GET", "kuvo/setup")).value.leadershipWarning, /also runs Kuvo's relay/);
  const reconnect = leaseRelay.disconnect(a.key);
  const initial = leaseRelay.lease;
  leaseRelay.advance(125_000);
  leaseRelay.restart();
  relay.drop();
  const before = leaseRelay.calls.length;
  await until("a member checks the restarted relay", () => leaseRelay.calls.slice(before).some(call => call.computer === b.key), t.signal);
  await delay(600);
  await see(b, 2, a);
  assert.equal(leaseRelay.lease.term, initial.term);
  assert.equal(leaseRelay.lease.expires, initial.expires);

  reconnect();
  await until("the returning main renews the same relay term", () => leaseRelay.lease.expires > initial.expires, t.signal);
  await see(a, 2, a);
  await see(b, 2, a);
  assert.equal(leaseRelay.lease.term, initial.term);
});

test("relay lease: a cut-off main fences changes and steps down when the always-on member takes over", { timeout: 25_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  const initial = await until("the main's first relay grant", () => leaseRelay.lease, AbortSignal.any([t.signal, AbortSignal.timeout(5_000)]));
  const returnLease = leaseRelay.disconnect(a.key);
  const returnMail = relay.disconnect(a.address);
  a.clock(65_000);

  const blocked = await a.route("POST", "kuvo/phones/pair");
  assert.equal(blocked.status, 409);
  assert.match(blocked.value.message, /Kuvo's relay can't be reached/);
  const work = await a.route("POST", "kuvo/work-queue/items", { title: "Everyday offline work" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(work.status, 200, JSON.stringify(work.value));

  const previousHigh = leaseRelay.lease.high;
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  assert.equal(leaseRelay.lease.term, initial.term + 1);
  assert.ok(previousHigh >= 500);
  const made = await b.route("POST", "kuvo/work-queue/items", { title: "Numbered after takeover" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(made.status, 200, JSON.stringify(made.value));
  const number = await until("the new main allocates above the old main's high mark", async () => {
    const reply = await b.route("GET", `kuvo/work-queue/items/${made.value.item.id}`);
    return reply.value.item?.number;
  }, t.signal);
  assert.ok(number > previousHigh && number <= leaseRelay.lease.high);

  returnLease();
  returnMail();
  await see(a, 3, b);
  assert.equal((await a.route("GET", "kuvo/setup")).value.you.role, "member");
  assert.equal(leaseRelay.lease.holder, b.key);
});

test("main saves alone: after unlinking one of two computers new items get numbers", { timeout: 20_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t, { ranked: true, standby: false });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  const removal = await a.route("DELETE", `kuvo/computers/${b.key}`, { wipe: false });
  assert.equal(removal.status, 200, JSON.stringify(removal.value));
  const created = await a.route("POST", "kuvo/work-queue/items", { title: "After unlink" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(created.status, 200, JSON.stringify(created.value));
  await until("item gets a number from the remaining main", async () => {
    const reply = await a.route("GET", `kuvo/work-queue/items/${created.value.item.id}`);
    return Number.isInteger(reply.value.item?.number);
  }, AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]));
});

test("main saves alone: the new main pairs a phone after a two-computer takeover", { timeout: 20_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await a.stop();
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  const pairing = await b.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200, JSON.stringify(pairing.value));
  const token = new URL(pairing.value.link).searchParams.get("token");
  const key = link.newKey();
  const phone = await deviceAt(relay, b, key, { name: "After takeover", pair: token });
  t.after(() => phone.close());
  assert.equal(phone.greeting?.save?.status, "committed");
  const pairedKey = link.publicOf(key).toString("base64url");
  assert.ok(b.read().shared.membershipAndAccess.value.devices.phones.some(phone => phone.key === pairedKey));
  const label = await b.route("POST", "kuvo/work-queue/labels", { add: "After takeover" }, true);
  assert.equal(label.status, 200, JSON.stringify(label.value));
  assert.ok(b.read().shared.queueLabels.value.labels.some(label => label.name === "After takeover"));
});

test("main saves alone: a dead standby is unlinked in one request", { timeout: 20_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t);
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await b.stop();
  const removal = await a.route("DELETE", `kuvo/computers/${b.key}`, { wipe: false });
  assert.equal(removal.status, 200, JSON.stringify(removal.value));
  assert.equal(a.read().setup.epoch, 2);
  assert.equal(a.read().setup.standby, undefined);
  assert.ok(!a.read().shared.membershipAndAccess.value.boxes.some(peer => peer.key === b.key));
});

test("main saves alone: a resident member catches up to a save committed while it was down", { timeout: 40_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t, { standby: false, resident: true });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await b.stop();
  const { port, password, pid } = JSON.parse(fs.readFileSync(path.join(a.run, "serve.json"), "utf8"));
  assert.equal(pid, a.child.pid);
  const result = await fetch(`http://127.0.0.1:${port}/kuvo/computers/${b.key}`, {
    method: "PATCH",
    headers: { authorization: "Basic " + Buffer.from("opencode:" + password).toString("base64"),
      "x-kuvo-from": "mac", "content-type": "application/json" },
    body: JSON.stringify({ name: "Renamed while asleep" }),
    signal: t.signal,
  });
  const receipt = await result.json();
  assert.equal(result.status, 200, JSON.stringify(receipt));
  assertReceipt(a, "setupNames", "committed", receipt);
  assert.equal(a.read().shared.setupNames.value[b.key], "Renamed while asleep");
  b.start();
  await until("member copies the committed name and its receipt", () =>
    b.read().shared?.setupNames?.value[b.key] === "Renamed while asleep" &&
    b.read().sharedOutcomes?.[receipt.operation]?.status === "committed", t.signal);
});

for (const splitIndex of [0, 1]) {
  test(`mixed resident pilot: ${splitIndex === 0 ? "split" : "resident"} main hands over with receipts and a direct move`, { timeout: 80_000 }, async t => {
    const group = { id: "grp_pilot", name: "Pilot group", members: ["bot_one", "bot_two"], created: "2026-11-21T00:00:00Z", seats: {}, heard: {} };
    const { a, b, see, leaseRelay, groupCopies } = await network(t, true, undefined, false, group, false, true, ["Studio", "Lab"], {
      resident: true, splitIndex, acceptedRoster: true,
    });
    const split = [a, b][splitIndex];
    const ask = (box, ...args) => box === split ? box.route(...args) : box.residentRoute(...args);
    const term = leaseRelay.lease.term;
    assert.equal(leaseRelay.lease.holder, a.key);

    const saveName = async (main, member, name) => {
      const receipt = await ask(member, "PATCH", `kuvo/computers/${member.key}`, { name });
      assert.equal(receipt.status, 200, JSON.stringify(receipt));
      assertReceipt(main, "setupNames", "committed", receipt.value);
      const operation = receipt.value.operation;
      await until("the member receives the committed receipt", () => member.read().sharedOutcomes?.[operation]?.status === "committed", t.signal);
      assert.equal(member.read().shared.setupNames.value[member.key], name);
      return operation;
    };
    const before = await saveName(a, b, "Before handover");
    const handover = await ask(a, "POST", "kuvo/setup/handover", { primary: b.key, epoch: 2 });
    assert.ok([200, 202].includes(handover.status), JSON.stringify(handover));
    await Promise.all([see(a, 3, b), see(b, 3, b)]);
    assert.equal(leaseRelay.lease.holder, b.key);
    assert.equal(leaseRelay.lease.term, term + 1);
    assert.equal(b.read().sharedOutcomes[before].status, "committed");
    await saveName(b, a, "After handover");

    const folder = path.join(b.home, "groups", group.id);
    fs.mkdirSync(folder, { recursive: true });
    fs.writeFileSync(path.join(folder, "group.json"), JSON.stringify({ kuvo: 1, status: "active", group }));
    fs.writeFileSync(path.join(folder, "messages.json"), "[]");
    const moved = await ask(b, "POST", "kuvo/moves", { kind: "group", id: group.id, to: a.key, receipt: true });
    assert.equal(moved.status, 202, JSON.stringify(moved));
    const receiptRoute = `kuvo/moves/receipt?id=${moved.value.move.id}`;
    const done = await until("the source reports its move outcome", async () => {
      const receipt = await ask(b, "GET", receiptRoute);
      return receipt.status === 200 && receipt.value.state && receipt.value.state !== "moving" && receipt;
    }, t.signal);
    assert.equal(done.value.state, "done", JSON.stringify(done));
    assert.equal(done.value.move.to, a.key);
    assert.equal(groupCopies.get("Studio")?.status, "active");
    await until("the source exports and releases its old copy", () => !groupCopies.has("Lab"), t.signal);
  });
}

test("unlinking the main of two computers hands its role to the other, on an older version, and unlinks it", { timeout: 30_000 }, async t => {
  const { a, b, leaseRelay, see } = await relayLeaseNetwork(t, { oldMember: true });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  const listed = (await a.route("GET", "kuvo/computers")).value.computers.find(computer => computer.key === a.key);
  assert.match(listed.actions.find(action => action.id === "remove")?.confirm?.message ?? "", /Desk becomes the main computer first/);

  const unlinked = await a.route("POST", `kuvo/computers/${a.key}/unlink`);
  assert.equal(unlinked.status, 202, JSON.stringify(unlinked.value));
  assert.deepEqual(unlinked.value, { primary: b.key, epoch: 3, unlinking: true, removal: { state: "handing-over", accessRemoved: false },
    message: "Desk is becoming the main computer. Then Studio is unlinked." });
  assert.equal(leaseRelay.lease.holder, b.key);
  await see(b, 3, b);
  // The relay starts the new main's grant when the old one ends.
  leaseRelay.advance(65_000);
  b.clock(65_000);
  await until("the new main unlinks the old one", () => !b.read().shared.membershipAndAccess.value.boxes.some(peer => peer.key === a.key), t.signal);
  assert.ok(b.read().shared.membershipAndAccess.value.left.includes(a.key));
  await until("the new main forgets the finished unlink", () => b.read().unlinkAfterHandover === undefined, t.signal);
  await until("the old main lets the setup go", () => a.read().setup?.primary !== a.key, t.signal);
});
