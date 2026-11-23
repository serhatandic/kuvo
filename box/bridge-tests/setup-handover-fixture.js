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
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { relayStandIn } from "./relay-stand-in.js";
import { runningAs } from "./running-as.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { splitLaunchBuild } from "./failure-bench-legacy.js";
import { durablePhase, put, recoveryEngine } from "./recovery-bench.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const submit = (box, primary, epoch) => box.route("POST", "kuvo/setup/handover", { primary: primary.key, epoch });

function peerOwns(box, owner, id) {
  try {
    const owns = JSON.parse(fs.readFileSync(path.join(box.run, "peer-agents.json"), "utf8"))[owner.key]?.owns ?? "";
    const digest = crypto.createHash("sha256").update(id).digest("base64url").slice(0, 6);
    return owns.match(/.{6}/g)?.includes(digest) ?? false;
  } catch { return false; }
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

async function leasedNetwork(t, releaseKey) {
  const net = await network(t, true, releaseKey);
  await until("the main holds the relay lease", () => net.a.lease(), t.signal);
  return net;
}

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

const rename = (a, target, name) => a.route("PATCH", `kuvo/computers/${target.key}`, { name });
const setupName = async (box, key) => (await box.route("GET", "kuvo/setup")).value.computers.find(each => each.key === key)?.name;

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

export {
  acceptedEdit, assertReceipt, deviceAt, editAddress, editPath, editRead, editSubmit, enrollDesktop,
  itemID, leasedNetwork, link, network, peerOwns, pendingOperation, rename, script, setupName,
  submit, until,
};
