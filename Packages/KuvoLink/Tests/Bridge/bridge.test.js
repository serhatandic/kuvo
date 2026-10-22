// The bridge, checked against the shared vectors and against a phone and an engine in this
// process. Run with `node --test Packages/KuvoLink/Tests/Bridge/*.test.js`.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

// The bridge keeps its state in KUVO_HOME, so give it a folder of its own before loading it.
const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-test-"));
process.env.KUVO_HOME = home;
after(() => fs.rmSync(home, { recursive: true, force: true }));
const link = await import("../../Sources/KuvoLink/Resources/kuvo-link.js");

const vectors = name => JSON.parse(fs.readFileSync(new URL(`../../Vectors/${name}`, import.meta.url), "utf8"));
const hex = s => Buffer.from(s, "hex");

test("Noise IK matches the vectors", () => {
  for (const v of vectors("noise.json").vectors.filter(v => v.protocol_name === "Noise_IK_25519_AESGCM_SHA256")) {
    const initiator = new link.IKInitiator(hex(v.init_static), hex(v.init_remote_static), hex(v.init_prologue), hex(v.init_ephemeral));
    const responder = new link.IKResponder(hex(v.resp_static), hex(v.resp_prologue), hex(v.resp_ephemeral));
    const [m0, m1, ...rest] = v.messages;
    const first = initiator.writeMessage1(hex(m0.payload));
    assert.equal(first.toString("hex"), m0.ciphertext);
    assert.equal(responder.readMessage1(first).toString("hex"), m0.payload);
    const { message, transport: responderSide } = responder.writeMessage2(hex(m1.payload));
    assert.equal(message.toString("hex"), m1.ciphertext);
    const { payload, transport: initiatorSide } = initiator.readMessage2(message);
    assert.equal(payload.toString("hex"), m1.payload);
    assert.equal(initiatorSide.hash.toString("hex"), v.handshake_hash);
    rest.forEach((m, i) => {
      const [from, to] = i % 2 === 0 ? [initiatorSide, responderSide] : [responderSide, initiatorSide];
      assert.equal(from.seal(hex(m.payload)).toString("hex"), m.ciphertext);
      assert.equal(to.open(hex(m.ciphertext)).toString("hex"), m.payload);
    });
  }
});

test("Noise K matches the vectors", () => {
  for (const v of vectors("noise.json").vectors.filter(v => v.protocol_name === "Noise_K_25519_AESGCM_SHA256")) {
    const [m0, ...rest] = v.messages;
    const sealed = link.sealOneWay(hex(m0.payload), hex(v.init_static), hex(v.init_remote_static), hex(v.init_prologue), hex(v.init_ephemeral));
    assert.equal(sealed.message.toString("hex"), m0.ciphertext);
    const opened = link.openOneWay(sealed.message, hex(v.resp_static), link.publicOf(hex(v.init_static)), hex(v.resp_prologue));
    assert.equal(opened.payload.toString("hex"), m0.payload);
    for (const m of rest) {
      assert.equal(sealed.following.encrypt(Buffer.alloc(0), hex(m.payload)).toString("hex"), m.ciphertext);
      assert.equal(opened.following.decrypt(Buffer.alloc(0), hex(m.ciphertext)).toString("hex"), m.payload);
    }
  }
});

test("addresses, relay messages, frames and messages match the vectors", () => {
  const v = vectors("link.json");
  for (const { secret, address } of v.addresses) assert.equal(link.addressOf(hex(secret)).toString("hex"), address);
  for (const r of v.relay) {
    const bytes = hex(r.bytes);
    if (r.name === "hello") assert.deepEqual(link.relayHello(bytes.subarray(1)), bytes);
    if (r.name === "send") assert.deepEqual(link.relaySend(hex(r.to), hex(r.payload)), bytes);
    if (r.name === "welcome") assert.equal(link.readRelay(bytes).address.toString("hex"), r.address);
    if (r.name === "deliver") assert.deepEqual(link.readRelay(bytes), { kind: "deliver", address: hex(r.address), payload: hex(r.payload) });
    if (r.name === "unreachable") assert.equal(link.readRelay(bytes).kind, "unreachable");
    if (r.name === "pushed") assert.equal(link.readRelay(bytes).status, r.status);
  }
  for (const f of v.frames) {
    assert.equal(link.frame(f.kind, f.session, hex(f.body)).toString("hex"), f.bytes);
    assert.deepEqual(link.readFrame(hex(f.bytes)), { kind: f.kind, session: f.session, body: hex(f.body) });
  }
  for (const r of v.routes) assert.equal(link.allowed(r.path, r.method ?? "GET"), r.allowed, `${r.path} ${r.method ?? "GET"}`);
  for (const r of v.sockets) assert.equal(link.socketAllowed(r.path), r.allowed, r.path);
  for (const r of v.agentRoutes) assert.equal(link.agentsAllowed(r.path, r.method ?? "GET"), r.allowed, `agents: ${r.path} ${r.method ?? "GET"}`);
  for (const r of v.signInReturns) assert.equal(link.signInReturn(r.url), r.loads ?? null, r.url);
  for (const m of v.messages) {
    assert.equal(link.encodeMessage(m.head, hex(m.body)).toString("hex"), m.bytes);
    assert.deepEqual(link.decodeMessage(hex(m.bytes)), { head: m.head, body: hex(m.body) });
  }
});

test("long bodies split into messages that fit", () => {
  const body = crypto.randomBytes(200_000);
  const parts = link.pieces({ t: "res", id: 3, s: 200, h: { "content-type": "x" } }, body, true).map(link.decodeMessage);
  assert.equal(parts.length, 4);
  assert.deepEqual(parts.map(p => p.head.t), ["res", "data", "data", "data"]);
  assert.deepEqual(parts.map(p => p.head.end), [false, false, false, true]);
  assert.deepEqual(Buffer.concat(parts.map(p => p.body)), body);
});

// ---- A phone talking to a LinkHost in this process ----

function phoneFor(host, hostKey) {
  const key = link.newKey();
  const address = crypto.randomBytes(16);
  const session = crypto.randomInt(1, 2 ** 32);
  const inbox = [];
  const waiting = [];
  // What the host sends goes straight to the phone.
  host.send = bytes => {
    const message = link.readRelay(Buffer.concat([Buffer.from([2]), bytes.subarray(1, 17), bytes.subarray(17)]));
    const frame = link.readFrame(message.payload);
    const resolve = waiting.shift();
    if (resolve) resolve(frame); else inbox.push(frame);
  };
  const next = () => new Promise((resolve, reject) => {
    if (inbox.length) return resolve(inbox.shift());
    waiting.push(resolve);
    setTimeout(() => reject(new Error("nothing arrived")), 2000).unref();
  });
  let transport;
  return {
    key,
    address,
    async open(hello) {
      const initiator = new link.IKInitiator(key, link.publicOf(hostKey), link.PROLOGUE);
      await host.receive(address, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, name: "Test iPhone", app: "0", ...hello })))));
      const answer = await next();
      if (answer.kind !== link.Frame.handshake2) return { refused: true };
      const opened = initiator.readMessage2(answer.body);
      transport = opened.transport;
      return JSON.parse(opened.payload);
    },
    send(head, body = Buffer.alloc(0)) {
      for (const piece of link.pieces(head, body, head.end ?? true)) host.receive(address, link.frame(link.Frame.transport, session, transport.seal(piece)));
    },
    async receive() {
      const frame = await next();
      if (frame.kind !== link.Frame.transport) return { closed: true };
      return link.decodeMessage(transport.open(frame.body));
    },
    // A whole response: its head, and its body up to the message marked end.
    async response() {
      const first = await this.receive();
      const body = [first.body];
      for (let last = first; last.head.end !== true;) {
        last = await this.receive();
        body.push(last.body);
      }
      return { head: first.head, body: Buffer.concat(body) };
    },
  };
}

test("the host lets in a phone with the pairing token once, then by its key", async () => {
  const hostKey = link.newKey();
  link.writeLink({ relay: "wss://relay.test", secret: "", key: hostKey.toString("base64url"), phones: [], pairing: { token: Buffer.alloc(16, 7).toString("base64url"), expires: Date.now() + 60_000 } });
  const host = new link.LinkHost({ key: hostKey, hello: { v: 1, kind: "machine", name: "my-server", app: "0", engine: "2.0.30" }, authorize: link.authorizePhone, confirmed: link.confirmPhone, handle: async function* () {} });

  const stranger = phoneFor(host, hostKey);
  assert.deepEqual(await stranger.open({ pair: Buffer.alloc(16, 8).toString("base64url") }), { refused: true });

  const phone = phoneFor(host, hostKey);
  const hello = await phone.open({ pair: Buffer.alloc(16, 7).toString("base64url") });
  assert.equal(hello.kind, "machine");
  assert.equal(hello.name, "my-server");
  const saved = link.readLink();
  assert.equal(saved.pairing, undefined);
  assert.deepEqual(saved.phones.map(p => [p.key, p.name]), [[link.publicOf(phone.key).toString("base64url"), "Test iPhone"]]);
  // Where its notifications go.
  assert.equal(saved.phones[0].address, phone.address.toString("base64url"));
  assert.equal((fs.statSync(link.LINK_FILE).mode & 0o777).toString(8), "600");

  // The token is used up; the paired phone comes back without it.
  const late = phoneFor(host, hostKey);
  assert.deepEqual(await late.open({ pair: Buffer.alloc(16, 7).toString("base64url") }), { refused: true });
  const returning = phoneFor(host, hostKey);
  returning.key.set(phone.key);
  assert.equal((await returning.open({})).name, "my-server");
  // Somewhere else now: its notifications go there once its session proves its own (a replayed
  // first handshake message can't), not on the handshake.
  assert.equal(link.readLink().phones[0].address, phone.address.toString("base64url"));
  returning.send({ t: "ping" });
  assert.equal((await returning.receive()).head.t, "pong");
  assert.equal(link.readLink().phones[0].address, returning.address.toString("base64url"));
});

// An engine on a free port that answers like opencode does, and records what it was asked.
async function fakeEngine() {
  const asked = [];
  const server = http.createServer((request, response) => {
    asked.push({ method: request.method, url: request.url, auth: request.headers.authorization, directory: request.headers["x-opencode-directory"] });
    if (request.url === "/api/event") {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write('data: {"type":"server.connected"}\n\n');
      const timer = setInterval(() => response.write('data: {"type":"server.heartbeat"}\n\n'), 10);
      request.on("close", () => { clearInterval(timer); asked.push({ closed: request.url }); });
      return;
    }
    if (request.url === "/api/credential" && request.method === "GET") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ data: [
        { id: "cred_1", integrationID: "anthropic", label: "Work", active: true, value: { type: "api", key: "sk-ant-secret" } },
        { id: "cred_2", integrationID: "github-copilot", label: "me", active: false, value: { type: "oauth", refresh: "ghr_secret" } },
      ] }));
      return;
    }
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      response.writeHead(200, { "content-type": "application/json", "set-cookie": "nope" });
      response.end(JSON.stringify({ path: request.url, body }));
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { asked, port: server.address().port, close: () => { server.closeAllConnections(); server.close(); } };
}

test("requests go to the engine with its password, all of its API; paths that aren't what they seem don't", async t => {
  const engine = await fakeEngine();
  t.after(engine.close);
  const hostKey = link.newKey();
  const handle = link.engineHandler(() => ({ pid: process.pid, port: engine.port, password: "s3cret", version: "2.0.30" }));
  const host = new link.LinkHost({ key: hostKey, hello: { v: 1, kind: "machine", name: "m", app: "0" }, authorize: () => true, handle });
  const phone = phoneFor(host, hostKey);
  await phone.open({});

  phone.send({ t: "req", id: 1, m: "POST", p: "api/session/ses_1/prompt?x=1", h: { "Content-Type": "application/json", "X-Opencode-Directory": "/home/me/p", Authorization: "Basic forged" } }, Buffer.from('{"text":"hi"}'));
  const answer = await phone.response();
  assert.equal(answer.head.t, "res");
  assert.equal(answer.head.s, 200);
  assert.equal(answer.head.h["content-type"], "application/json");
  assert.equal(answer.head.h["set-cookie"], undefined);
  // A message from the phone says so, at its end.
  assert.deepEqual(JSON.parse(answer.body), { path: "/api/session/ses_1/prompt?x=1", body: JSON.stringify({ text: `hi\n\n${link.FROM_PHONE}` }) });
  const call = engine.asked.at(-1);
  assert.equal(call.auth, `Basic ${Buffer.from("opencode:s3cret").toString("base64")}`);
  assert.equal(call.directory, "/home/me/p");

  // The phone is yours: the engine's settings and sign-ins too.
  for (const [id, p] of [[2, "api/credential"], [3, "api/config"]]) {
    phone.send({ t: "req", id, m: "GET", p, h: {} });
    assert.equal((await phone.response()).head.s, 200, p);
  }
  for (const [id, p] of [[4, "api/session/../credential"], [5, "kuvo/nothing"], [6, "api/session#x"]]) {
    phone.send({ t: "req", id, m: "GET", p, h: {} });
    assert.equal((await phone.response()).head.s, 403, p);
  }
  assert.ok(!engine.asked.some(a => a.url?.includes("..") || a.url?.includes("nothing")));

  // An event stream runs until the phone cancels it, and then the engine sees it close.
  phone.send({ t: "req", id: 9, m: "GET", p: "api/event", h: { Accept: "text/event-stream" } });
  const head = await phone.receive();
  assert.equal(head.head.h["content-type"], "text/event-stream");
  assert.match(head.body.toString(), /server.connected/);
  await phone.receive();
  phone.send({ t: "cancel", id: 9 });
  for (let i = 0; i < 50 && !engine.asked.some(a => a.closed === "/api/event"); i++) await new Promise(r => setTimeout(r, 20));
  assert.ok(engine.asked.some(a => a.closed === "/api/event"));
});

// What a phone's request gets from the handler, read whole.
async function answer(handle, method, p, body) {
  const parts = [];
  for await (const part of handle({ method, path: p, headers: {}, body: Buffer.from(body ?? "") }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  return { status: parts[0].head.status, json: text ? JSON.parse(text) : undefined };
}

test("a sign-in that returns to the machine is loaded there, at its own address only, without following it further", async t => {
  const asked = [];
  const server = http.createServer((request, response) => {
    asked.push(request.url);
    response.writeHead(302, { location: "http://example.com/done" });
    response.end();
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  const handle = link.engineHandler(() => ({ port: 1, password: "p" }));
  const port = server.address().port;
  const loaded = await answer(handle, "POST", "kuvo/sign-in/return", JSON.stringify({ url: `http://127.0.0.1:${port}/mcp/oauth/callback?code=c&state=s#_=_` }));
  assert.deepEqual([loaded.status, loaded.json], [200, { status: 302 }]);
  assert.deepEqual(asked, ["/mcp/oauth/callback?code=c&state=s"]);
  for (const url of ["http://example.com:80/cb?code=c", `http://127.0.0.1:${port}/cb`, `https://127.0.0.1:${port}/cb?code=c`]) {
    assert.equal((await answer(handle, "POST", "kuvo/sign-in/return", JSON.stringify({ url }))).status, 400, url);
  }
  assert.equal(asked.length, 1);
  // Nothing there to take it.
  const closed = http.createServer();
  await new Promise(resolve => closed.listen(0, "127.0.0.1", resolve));
  const gone = closed.address().port;
  await new Promise(resolve => closed.close(resolve));
  assert.equal((await answer(handle, "POST", "kuvo/sign-in/return", JSON.stringify({ url: `http://127.0.0.1:${gone}/cb?code=c` }))).status, 502);
  assert.equal((await answer(handle, "GET", "kuvo/sign-in/return")).status, 405);
});

test("a machine's settings are changed on its Mac; its engine restarts as it was started", async () => {
  const handle = link.engineHandler(() => ({ pid: 999999, port: 1, password: "p" }));
  for (const p of ["kuvo/config", "kuvo/mcp/catalog", "kuvo/mcp/catalog/linear"]) {
    const answered = await answer(handle, p.endsWith("linear") ? "POST" : "GET", p);
    assert.equal(answered.status, 409, p);
    assert.match(answered.json.message, /Mac/);
  }
  // Never started from a Mac here (no record of how): it says to.
  assert.equal((await answer(handle, "POST", "kuvo/engine/restart")).status, 409);
  assert.equal((await answer(link.engineHandler(() => { throw new Error("none"); }), "POST", "kuvo/engine/restart")).status, 409);
  // Started from a Mac: `kuvo-start` starts it again, once the answer's out.
  const bin = path.join(home, "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  fs.writeFileSync(path.join(home, "run", "launch.env"), "PATH=/usr/bin\n");
  const started = path.join(home, "run", "started-again");
  fs.writeFileSync(path.join(bin, "kuvo-start"), `#!/bin/sh\necho "bun=\${BUN_BE_BUN-}" >"${started}"\n`);
  // Without what starts the engine (`kuvo-launch`), nothing is stopped.
  assert.equal((await answer(handle, "POST", "kuvo/engine/restart")).status, 409);
  fs.writeFileSync(path.join(bin, "kuvo-launch"), "#!/bin/sh\n");
  // The bridge runs as Bun; the engine it starts again mustn't.
  process.env.BUN_BE_BUN = "1";
  try {
    assert.equal((await answer(handle, "POST", "kuvo/engine/restart")).status, 202);
    for (let i = 0; i < 100 && !fs.existsSync(started); i++) await new Promise(r => setTimeout(r, 50));
  } finally {
    delete process.env.BUN_BE_BUN;
  }
  assert.equal(fs.readFileSync(started, "utf8").trim(), "bun=");
  assert.equal((await answer(handle, "GET", "kuvo/engine/restart")).status, 405);
  for (const name of ["bin/kuvo-start", "bin/kuvo-launch", "run/launch.env", "run/started-again"]) fs.rmSync(path.join(home, name));
});

test("kuvo/file sends a file on the machine by its file:// address, with its type", async () => {
  const handle = link.engineHandler(() => ({}));
  const folder = fs.mkdtempSync(path.join(home, "files-"));
  const picture = path.join(folder, "a chart #1.png");
  const bytes = crypto.randomBytes(100 * 1024);
  fs.writeFileSync(picture, bytes);
  const ask = async uri => {
    const parts = [];
    for await (const part of handle({ method: "GET", path: `kuvo/file?uri=${encodeURIComponent(uri)}`, headers: {}, body: Buffer.alloc(0) }, {}, undefined)) parts.push(part);
    return { head: parts[0].head, body: Buffer.concat(parts.filter(part => part.body).map(part => part.body)) };
  };
  const sent = await ask(pathToFileURL(picture).href);
  assert.equal(sent.head.status, 200);
  assert.equal(sent.head.headers["content-type"], "image/png");
  assert.ok(sent.body.equals(bytes));
  const gone = await ask(pathToFileURL(path.join(folder, "gone.png")).href);
  assert.equal(gone.head.status, 404);
  assert.ok(JSON.parse(gone.body).message);
  assert.equal((await ask(pathToFileURL(folder).href)).head.status, 404, "a folder isn't a file");
  assert.equal((await ask("https://example.com/a.png")).head.status, 400);
  const big = path.join(folder, "big.bin");
  fs.writeFileSync(big, "");
  fs.truncateSync(big, link.FILE_LIMIT + 1);
  assert.equal((await ask(pathToFileURL(big).href)).head.status, 413);
  fs.rmSync(folder, { recursive: true });
});

test("kuvo/info says when the engine running now came up, which a restart changes", async () => {
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  const record = path.join(home, "run", "engine.json");
  fs.writeFileSync(record, JSON.stringify({ pid: 4242, port: 1, password: "p", version: "2.0.32" }));
  const then = new Date("2026-10-22T10:00:00Z");
  fs.utimesSync(record, then, then);
  const handle = link.engineHandler(() => JSON.parse(fs.readFileSync(record, "utf8")));
  const info = await answer(handle, "GET", "kuvo/info");
  assert.equal(info.json.started, then.getTime() / 1000);
  assert.equal(info.json.engine, "2.0.32");
  // None running: none started.
  assert.equal((await answer(link.engineHandler(() => { throw new Error("none"); }), "GET", "kuvo/info")).json.started, null);
  fs.rmSync(record);
});

test("plan accounts: listed, signed in to and switched from a phone", async t => {
  const engine = await fakeEngine();
  t.after(engine.close);
  const hostKey = link.newKey();
  const handle = link.engineHandler(() => ({ pid: process.pid, port: engine.port, password: "s3cret", version: "2.0.30" }));
  const host = new link.LinkHost({ key: hostKey, hello: { v: 1, kind: "machine", name: "m", app: "0" }, authorize: () => true, handle });
  const phone = phoneFor(host, hostKey);
  await phone.open({});

  // Its sign-ins, by name and which is in use; never what they're signed in with.
  phone.send({ t: "req", id: 1, m: "GET", p: "kuvo/accounts/credentials", h: {} });
  const listed = await phone.response();
  assert.equal(listed.head.s, 200);
  assert.deepEqual(JSON.parse(listed.body), { credentials: [
    { id: "cred_1", integrationID: "anthropic", label: "Work", active: true },
    { id: "cred_2", integrationID: "github-copilot", label: "me", active: false },
  ] });
  assert.ok(!listed.body.toString().includes("secret"));

  // A key goes in; another sign-in is switched to; the list itself never goes out.
  phone.send({ t: "req", id: 2, m: "POST", p: "api/integration/anthropic/connect/key", h: { "Content-Type": "application/json" } }, Buffer.from('{"key":"sk-new"}'));
  assert.equal((await phone.response()).head.s, 200);
  phone.send({ t: "req", id: 3, m: "POST", p: "api/credential/cred_2/activate", h: {} });
  assert.equal((await phone.response()).head.s, 200);
  // A sign-in page, started with its method's hidden answers (OpenCode's server), as the Mac sends them.
  const start = '{"methodID":"console","answer":{"server":"https://opencode.ai"}}';
  phone.send({ t: "req", id: 7, m: "POST", p: "api/integration/opencode/connect/oauth", h: { "Content-Type": "application/json" } }, Buffer.from(start));
  const started = await phone.response();
  assert.equal(started.head.s, 200);
  assert.equal(JSON.parse(started.body).body, start);
  assert.deepEqual(engine.asked.filter(a => a.url?.startsWith("/api/")).map(a => `${a.method} ${a.url}`),
    ["GET /api/credential", "POST /api/integration/anthropic/connect/key", "POST /api/credential/cred_2/activate", "POST /api/integration/opencode/connect/oauth"]);
});

// A terminal on a free port as the engine serves one: a WebSocket that first says where its
// output starts, then answers each line. Short frames only, which is all this needs.
async function fakeTerminal() {
  const asked = [];
  const server = http.createServer();
  server.on("upgrade", (request, socket) => {
    asked.push({ url: request.url, auth: request.headers.authorization, directory: request.headers["x-opencode-directory"] });
    const accept = crypto.createHash("sha1").update(request.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    const send = (opcode, payload) => socket.write(Buffer.concat([Buffer.from([0x80 | opcode, payload.length]), payload]));
    send(2, Buffer.concat([Buffer.from([0]), Buffer.from('{"cursor":0}')]));
    let buffered = Buffer.alloc(0);
    socket.on("data", chunk => {
      buffered = Buffer.concat([buffered, chunk]);
      while (buffered.length >= 6) {
        const opcode = buffered[0] & 0x0f;
        const length = buffered[1] & 0x7f;
        if (buffered.length < 6 + length) return;
        const mask = buffered.subarray(2, 6);
        const payload = Buffer.from(buffered.subarray(6, 6 + length).map((byte, i) => byte ^ mask[i % 4]));
        buffered = buffered.subarray(6 + length);
        if (opcode === 1) send(1, Buffer.concat([Buffer.from("echo:"), payload]));
        if (opcode === 8) socket.end();
      }
    });
    socket.on("close", () => asked.push({ closed: request.url }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { asked, port: server.address().port, close: () => { server.closeAllConnections(); server.close(); } };
}

test("a socket reaches the engine's terminal with its password, both ways, until the phone closes it", async t => {
  const terminal = await fakeTerminal();
  t.after(terminal.close);
  const hostKey = link.newKey();
  const engine = () => ({ pid: process.pid, port: terminal.port, password: "s3cret", version: "2.0.30" });
  const host = new link.LinkHost({ key: hostKey, hello: { v: 1, kind: "machine", name: "m", app: "0" }, authorize: () => true, handle: link.engineHandler(engine), connect: link.engineConnect(engine) });
  const phone = phoneFor(host, hostKey);
  await phone.open({});

  phone.send({ t: "req", id: 1, m: "GET", p: "api/pty/pty_1/connect?cursor=0", h: { upgrade: "websocket", "x-opencode-directory": "/home/me/p" } });
  const opened = await phone.receive();
  assert.deepEqual([opened.head.t, opened.head.s, opened.head.end], ["res", 101, false]);
  const note = await phone.receive();
  assert.equal(note.head.h.type, "binary");
  assert.deepEqual(note.body, Buffer.concat([Buffer.from([0]), Buffer.from('{"cursor":0}')]));
  const call = terminal.asked[0];
  assert.equal(call.url, "/api/pty/pty_1/connect?cursor=0");
  assert.equal(call.auth, `Basic ${Buffer.from("opencode:s3cret").toString("base64")}`);
  assert.equal(call.directory, "/home/me/p");

  phone.send({ t: "data", id: 1, end: false }, Buffer.from("ls\r"));
  const answer = await phone.receive();
  assert.equal(answer.head.t, "data");
  assert.equal(answer.head.h, undefined);
  assert.equal(answer.body.toString(), "echo:ls\r");

  phone.send({ t: "cancel", id: 1 });
  for (let i = 0; i < 50 && !terminal.asked.some(a => a.closed); i++) await new Promise(r => setTimeout(r, 20));
  assert.ok(terminal.asked.some(a => a.closed));

  // Anything but a terminal's socket is turned away, without reaching the engine.
  phone.send({ t: "req", id: 2, m: "GET", p: "api/event", h: { upgrade: "websocket" } });
  const refused = await phone.response();
  assert.equal(refused.head.s, 502);
  assert.equal(terminal.asked.length, 2);
});

test("a relay that isn't there ends the attempt, so the bridge tries again", async () => {
  const host = { reset() {}, send() {} };
  const started = Date.now();
  assert.equal(await link.connectOnce("ws://127.0.0.1:9", crypto.randomBytes(32), host), false);
  assert.ok(Date.now() - started < 5000);
});

test("a session the host doesn't know is closed, and tampering ends one", async () => {
  const hostKey = link.newKey();
  const host = new link.LinkHost({ key: hostKey, hello: { v: 1, kind: "machine", name: "m", app: "0" }, authorize: () => true, handle: async function* () {} });
  const sent = [];
  host.send = bytes => sent.push(link.readFrame(bytes.subarray(17)));
  await host.receive(crypto.randomBytes(16), link.frame(link.Frame.transport, 99, Buffer.from("junk")));
  assert.equal(sent.at(-1).kind, link.Frame.close);
  assert.equal(sent.at(-1).session, 99);
});

test("the self-test passes here", () => {
  assert.ok(link.selftest());
});

test("setup keeps the machine's keys and its own phones, starts the bridge, and stops it with no phones left", async () => {
  const { spawnSync } = await import("node:child_process");
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-setup-"));
  fs.mkdirSync(path.join(folder, "workspace"));
  const script = fileURLToPath(new URL("../../Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
  const setup = input => {
    const result = spawnSync(process.execPath, [script, "setup"], { input: JSON.stringify(input), env: { ...process.env, KUVO_HOME: folder }, encoding: "utf8" });
    return JSON.parse(result.stdout);
  };
  const pidFile = path.join(folder, "run", "link.pid");
  try {
    // A relay nobody listens on: the bridge keeps trying, which is all this needs.
    const first = setup({ relay: "ws://127.0.0.1:9", name: "test", phones: [{ key: "AAAA", name: "A" }] });
    assert.match(first.address, /^[A-Za-z0-9_-]{22}$/);
    // The file exists briefly before its PID is written; setup also needs its version file.
    let pid = 0;
    let ready = false;
    for (let i = 0; i < 50; i++) {
      try {
        pid = Number(fs.readFileSync(pidFile, "utf8"));
        ready = Number.isInteger(pid) && pid > 0 && fs.readFileSync(path.join(folder, "run", "link.version"), "utf8").trim() === link.BRIDGE_VERSION;
        if (ready) break;
      } catch {}
      await new Promise(r => setTimeout(r, 50));
    }
    assert.ok(ready, "the bridge writes its PID and version before setup runs again");
    process.kill(pid, 0);

    // Its keys stay; the phones are what the Mac says now; the same bridge carries on.
    const second = setup({ relay: "ws://127.0.0.1:9", name: "test", phones: [{ key: "AAAA", name: "A" }, { key: "BBBB", name: "B" }] });
    assert.deepEqual(second, first);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(folder, "run", "link.json"), "utf8")).phones.map(p => p.key), ["AAAA", "BBBB"]);
    assert.equal(Number(fs.readFileSync(pidFile, "utf8")), pid);

    // A phone paired with the machine itself stays when the Mac's list changes.
    const linkFile = path.join(folder, "run", "link.json");
    const saved = JSON.parse(fs.readFileSync(linkFile, "utf8"));
    fs.writeFileSync(linkFile, JSON.stringify({ ...saved, phones: [...saved.phones, { key: "CCCC", name: "C", added: 1, from: "pair" }] }));
    setup({ relay: "ws://127.0.0.1:9", name: "test", phones: [{ key: "BBBB", name: "B" }] });
    assert.deepEqual(JSON.parse(fs.readFileSync(linkFile, "utf8")).phones.map(p => p.key), ["BBBB", "CCCC"]);

    // Nothing left to answer: the bridge stops, and the machine keeps its keys; no phone keeps
    // the engine up any more.
    const seen = path.join(folder, "run", "link.seen");
    fs.writeFileSync(seen, "");
    fs.writeFileSync(linkFile, JSON.stringify({ ...saved, phones: [] }));
    assert.deepEqual(setup({ relay: "ws://127.0.0.1:9", phones: [] }), {});
    const kept = JSON.parse(fs.readFileSync(linkFile, "utf8"));
    assert.equal(kept.secret, saved.secret);
    assert.deepEqual(kept.phones, []);
    assert.equal(fs.existsSync(seen), false);
    let stopped = false;
    for (let i = 0; i < 50 && !stopped; i++) {
      try { process.kill(pid, 0); await new Promise(r => setTimeout(r, 50)); } catch { stopped = true; }
    }
    assert.ok(stopped);
  } finally {
    try {
      const pid = Number(fs.readFileSync(pidFile, "utf8"));
      if (Number.isInteger(pid) && pid > 0) process.kill(pid, "SIGKILL");
    } catch {}
    fs.rmSync(folder, { recursive: true, force: true });
  }
});

// ---- Notifications ----

test("a notification fits a push, its body cut", () => {
  const long = "Wörds and more words. ".repeat(400);
  const json = link.noticeJSON({ title: "Fixer", subtitle: "kuvo-ai", body: long, category: "finished", thread: "ses_1", data: { session: "ses_1" } });
  assert.ok(json.length <= link.NOTICE_LIMIT);
  const notice = JSON.parse(json);
  assert.equal(notice.t, "notify");
  assert.ok(notice.body.endsWith("…"));
  assert.ok(long.startsWith(notice.body.slice(0, -1)));
  assert.ok(notice.body.length > 2000);
  // Short ones go as they are, keys sorted as the Mac writes them.
  assert.equal(link.noticeJSON({ title: "Fixer", body: "Done.", category: "report", thread: "ses_1", data: { session: "ses_1" } }).toString(),
    '{"body":"Done.","category":"report","data":{"session":"ses_1"},"t":"notify","thread":"ses_1","title":"Fixer"}');
});

test("a push is sealed for the phone it goes to", () => {
  const hostKey = link.newKey(), phoneKey = link.newKey();
  const address = crypto.randomBytes(16);
  const json = link.noticeJSON({ title: "Fixer", body: "Done.", category: "report", thread: "ses_1", data: {} });
  const bytes = link.pushTo({ key: link.publicOf(phoneKey).toString("base64url"), address: address.toString("base64url") }, json, hostKey);
  assert.equal(bytes[0], 5);
  assert.deepEqual(bytes.subarray(1, 17), address);
  const mail = link.readFrame(bytes.subarray(17));
  assert.equal(mail.kind, link.Frame.mail);
  assert.equal(mail.session, 0);
  assert.deepEqual(link.openOneWay(mail.body, phoneKey, link.publicOf(hostKey)).payload, json);
  assert.throws(() => link.openOneWay(mail.body, link.newKey(), link.publicOf(hostKey)));
});

test("engine events become notifications, as the Mac words them", async () => {
  const sessions = {
    ses_main: { id: "ses_main", title: "Fix the tests", location: { directory: "/home/me/kuvo-ai" } },
    ses_sub: { id: "ses_sub", parentID: "ses_main", title: "Explore", location: { directory: "/home/me/kuvo-ai" } },
    ses_bot: { id: "ses_bot", title: "Fixer", metadata: { kuvo: { bot: "bot_1" } }, location: { directory: "/home/me/.kuvo-code/bots/bot_1/workspace" } },
  };
  const messages = { data: [{ type: "assistant", content: [{ type: "text", text: "**All** green:\n 42 tests pass." }] }, { type: "user", content: [] }] };
  // ses_main is at work, with a tool of the "linear" server running.
  const working = { data: [{ type: "assistant", content: [{ type: "tool", name: "linear_create_issue", state: { status: "running", input: {} } }] }] };
  const get = async route => {
    if (route === "api/session/active") return { data: ["ses_main"] };
    if (route.endsWith("/message?limit=8&order=desc")) return route.includes("ses_main") ? working : { data: [] };
    if (route.endsWith("/message?limit=6&order=desc")) return messages;
    const id = decodeURIComponent(route.split("/").pop());
    if (!sessions[id]) throw new Error("404");
    return { data: sessions[id] };
  };

  // A subagent's permission shows in the session that started it.
  assert.deepEqual(await link.noticeFor({ type: "permission.asked", data: { id: "per_1", sessionID: "ses_sub", action: "shell", resources: ["npm test"] } }, get), {
    title: "Fix the tests", subtitle: "kuvo-ai", body: "Wants to run a command: npm test", category: "permission", thread: "ses_main",
    data: { session: "ses_main", request: "per_1", requestSession: "ses_sub" },
  });
  const question = { id: "frm_1", sessionID: "ses_main", title: "Pick", metadata: { kind: "question" }, fields: [{ key: "a", type: "string", description: "Which database?" }] };
  // As the engine sends it: the form under `form`.
  const asked = await link.noticeFor({ type: "form.created", data: { form: question } }, get);
  assert.equal(asked.category, "question");
  assert.equal(asked.body, "Which database?");
  assert.deepEqual(asked.data, { session: "ses_main", request: "frm_1" });
  // An MCP server's form is filed under no session: it's the one of its folder using that server.
  const elicitation = { id: "frm_2", sessionID: "global", metadata: { kind: "mcp-elicitation", server: "linear", message: "Which team?" }, fields: [] };
  const fromServer = await link.noticeFor({ type: "form.created", location: { directory: "/home/me/kuvo-ai" }, data: { form: elicitation } }, get);
  assert.equal(fromServer.body, "Which team?");
  assert.deepEqual(fromServer.data, { session: "ses_main", request: "frm_2", requestSession: "global" });
  // In a folder where nothing uses it: no notice.
  assert.equal(await link.noticeFor({ type: "form.created", location: { directory: "/home/me/other" }, data: { form: elicitation } }, get), null);

  // A bot's run: its news, with what it said last, without its folder.
  const report = await link.noticeFor({ type: "session.execution.succeeded", data: { sessionID: "ses_bot" } }, get);
  assert.deepEqual(report, { title: "Fixer", subtitle: undefined, body: "All green: 42 tests pass.", category: "report", thread: "ses_bot", data: { session: "ses_bot" } });
  assert.equal((await link.noticeFor({ type: "session.execution.succeeded", data: { sessionID: "ses_main" } }, get)).category, "finished");
  const failed = await link.noticeFor({ type: "session.execution.failed", data: { sessionID: "ses_main" } }, get);
  assert.equal(failed.category, "failed");
  assert.equal(failed.body, "Stopped with an error.");

  // A subagent's run is its parent's business; a stopped run and the rest aren't news.
  assert.equal(await link.noticeFor({ type: "session.execution.succeeded", data: { sessionID: "ses_sub" } }, get), null);
  assert.equal(await link.noticeFor({ type: "session.execution.interrupted", data: { sessionID: "ses_main" } }, get), null);
  assert.equal(await link.noticeFor({ type: "session.text.delta", data: { sessionID: "ses_main" } }, get), null);
});

test("a Mac that said it's here in the last few minutes keeps the bridge quiet", () => {
  const folder = fs.mkdtempSync(path.join(home, "present-"));
  try {
    assert.equal(link.macPresent(path.join(folder, "none")), false);
    assert.equal(link.macPresent(folder), false);
    fs.writeFileSync(path.join(folder, "mac-1"), "");
    assert.equal(link.macPresent(folder), true);
    const old = new Date(Date.now() - 10 * 60_000);
    fs.utimesSync(path.join(folder, "mac-1"), old, old);
    assert.equal(link.macPresent(folder), false);
  } finally {
    fs.rmSync(folder, { recursive: true, force: true });
  }
});

test("a phone list the Mac mails through the relay is taken from it alone, a newer one only: a phone removed there loses its way in", () => {
  const machine = link.newKey(), mac = link.newKey(), stranger = link.newKey();
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  link.writeLink({
    secret: crypto.randomBytes(32).toString("base64url"), key: machine.toString("base64url"), relay: "wss://relay.test", name: "m",
    mac: link.publicOf(mac).toString("base64url"), macAt: 1000,
    phones: [{ key: "lost", name: "Lost iPhone", added: 1, from: "mac" }, { key: "own", name: "Paired here", added: 2, from: "pair" }],
  });
  const mail = (from, phones, at) => link.sealOneWay(Buffer.from(JSON.stringify({ phones, at })), from, link.publicOf(machine)).message;
  const ended = [];
  const host = { keepOnly: keys => ended.push([...keys].sort()) };
  const keys = () => link.readLink().phones.map(phone => `${phone.from}:${phone.key}`);

  link.phonesFromMac(mail(stranger, [], 2000), host);
  link.phonesFromMac(mail(mac, [{ key: "new", name: "New iPhone" }], 900), host);
  assert.deepEqual(keys(), ["mac:lost", "pair:own"], "not from the Mac, or older: nothing changes");
  assert.deepEqual(ended, []);

  link.phonesFromMac(mail(mac, [{ key: "new", name: "New iPhone" }], 2000), host);
  assert.deepEqual(keys(), ["mac:new", "pair:own"], "the Mac's list now, and the machine's own pairing kept");
  assert.deepEqual(ended, [["new", "own"]], "the removed phone's sessions end");
  assert.equal(link.authorizePhone(Buffer.from("lost"), {}, undefined), false);
  // The same mail again changes nothing.
  link.phonesFromMac(mail(mac, [], 2000), host);
  assert.deepEqual(keys(), ["mac:new", "pair:own"]);
  fs.rmSync(path.join(home, "run", "link.json"));
});
