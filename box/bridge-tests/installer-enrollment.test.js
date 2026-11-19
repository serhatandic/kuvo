import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const link = await import(script);
const temp = fs.realpathSync(fs.mkdtempSync("/tmp/kuvo-enroll-"));
removeAtEnd(temp);

const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const keyOf = value => link.publicOf(Buffer.from(value.key, "base64url")).toString("base64url");
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(description, read, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  do {
    const value = await read();
    if (value) return value;
    await wait(50);
  } while (Date.now() < deadline);
  assert.fail(description);
}

function box(name, relay) {
  const home = path.join(temp, name);
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(home, { recursive: true });
  const ssh = path.join(home, "fake-ssh");
  fs.writeFileSync(ssh, '#!/bin/sh\nprintf "asked\\n" >> "$HOME/ssh-called"\nprintf "ssh: connect to host from-member.invalid port 22: Connection refused\\n" >&2\nexit 255\n', { mode: 0o700 });
  const env = { ...process.env, HOME: home, KUVO_HOME: folder, KUVO_TEST_RELAY: relay.url, KUVO_TEST_SSH: ssh };
  const file = name => path.join(folder, "run", name);
  const launch = (args, input) => spawnSync(process.execPath, [script, ...args], { input, encoding: "utf8", timeout: 12_000, env });
  const initialized = launch(["setup-local"]);
  assert.equal(initialized.status, 0, initialized.stderr);
  const start = (args, input) => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: [input ? "pipe" : "ignore", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", data => { out += data; });
    child.stderr.on("data", data => { err += data; });
    const done = new Promise(resolve => child.once("close", (status, signal) => resolve({ status, signal, out, err })));
    atEnd(async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await done;
    });
    if (input) child.stdin.end(input);
    return { child, done };
  };
  const state = () => read(file("link.json"));
  const request = async (method, name, body) => {
    const serve = await until("local server did not start", () => fs.existsSync(file("serve.json")) && read(file("serve.json")));
    const reply = await fetch(`http://127.0.0.1:${serve.port}/${name}`, {
      method, headers: { authorization: "Basic " + Buffer.from(`opencode:${serve.password}`).toString("base64"), "x-kuvo-from": "mac" },
      signal: AbortSignal.timeout(25_000), body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: reply.status, value: await reply.json() };
  };
  const startServer = () => {
    fs.writeFileSync(file("serve.needed"), "");
    return start(["serve", "--kept"]);
  };
  const release = () => {
    for (const [name] of link.RELEASE_FILES) {
      const target = path.join(folder, name);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, name === "bin/box-version" ? "0.8.0-beta.35\n" : "test release file");
    }
  };
  return { home, folder, file, start, state, request, startServer, release };
}

test("failure bench: installer retries a saved join after a lost answer and rejects code reuse by a different computer", { timeout: 55_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const engine = http.createServer((_request, response) => response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data: [] })));
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  atEnd(() => { engine.closeAllConnections(); engine.close(); });
  const attachEngine = target => fs.writeFileSync(target.file("engine.json"), JSON.stringify({ port: engine.address().port, password: "fake" }));
  const primary = box("primary", relay);
  const first = primary.state();
  // A pending pairing keeps the normal keeper bridge online before the first computer joins.
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...first, pairing: { token: "test", expires: Date.now() + 120_000 } }));
  attachEngine(primary);
  primary.release();
  primary.startServer();
  primary.start(["run"]);
  const address = link.addressOf(Buffer.from(first.secret, "base64url"));
  await until("primary is on the relay", () => relay.has(address));

  const created = await primary.request("POST", "kuvo/computers/add/code");
  assert.equal(created.status, 201, JSON.stringify(created.value));
  const { code } = created.value;
  assert.ok(created.value.expires > Date.now());
  const token = JSON.parse(Buffer.from(code.slice("kuvo-add-1.".length), "base64url")).token;
  assert.ok(!JSON.stringify(primary.state()).includes(token), "the primary stores the hash of the invitation");
  const joining = box("joining", relay);
  attachEngine(joining);
  const identity = joining.state();
  const key = keyOf(identity);
  const committed = () => primary.state().shared?.membershipAndAccess?.value?.boxes?.some(peer => peer.key === key);
  let release;
  const watch = relay.intercept(null, address, ({ from, payload }) => {
    if (!release && payload.includes(Buffer.from("kuvo installer v1"))) {
      release = relay.intercept(address, from, ({ payload }) => committed() && payload.includes(Buffer.from("kuvo installer v1")));
    }
    return false;
  });
  const initial = joining.start(["join"], code + "\n");
  await until("the primary saved accepted membership", committed);
  assert.equal(joining.state().setup.role, "primary", "the installer has not heard the result");
  initial.child.kill();
  const cut = await initial.done;
  assert.equal(cut.signal, "SIGTERM");
  watch();
  release();

  // Keep the applicant's bridge as a test-owned child even after the installer returns.
  fs.writeFileSync(joining.file("link.json"), JSON.stringify({ ...joining.state(), pairing: { token: "test", expires: Date.now() + 120_000 } }));
  joining.start(["run"]);
  await until("the applicant's bridge is running", () => relay.has(link.addressOf(Buffer.from(identity.secret, "base64url"))));
  const retry = await joining.start(["join"], code + "\n").done;
  assert.equal(retry.status, 0, retry.err);
  assert.deepEqual(JSON.parse(retry.out), { setup: first.setup.id, primary: first.setup.primary });
  assert.equal(joining.state().setup.role, "member");
  assert.equal(joining.state().setup.id, first.setup.id);
  assert.equal(keyOf(joining.state()), key);
  assert.ok(link.readDoc(joining.state().docs[`${first.setup.sign} peers`]));
  assert.equal(primary.state().shared.membershipAndAccess.value.boxes.filter(peer => peer.key === key).length, 1);

  const second = box("second", relay);
  const prefix = "kuvo-add-1.";
  const expired = JSON.parse(Buffer.from(code.slice(prefix.length), "base64url"));
  expired.expires = Date.now() - 1;
  const stale = await second.start(["join"], prefix + Buffer.from(JSON.stringify(expired)).toString("base64url") + "\n").done;
  assert.equal(stale.status, 1, stale.err);
  assert.match(stale.err, /installer code has expired/);
  assert.equal(second.state().installerClaim, undefined);
  const used = await second.start(["join"], code + "\n").done;
  assert.equal(used.status, 1, used.err);
  assert.match(used.err, /installer code is no longer available/);
  assert.equal(second.state().setup.role, "primary");
  assert.equal(second.state().installerClaim, undefined);
  assert.equal(committed(), true);

  const withData = second.state();
  withData.sharedSetup = withData.setup.id;
  withData.shared = { setupTimeZone: { sequence: 1, value: { zone: "Etc/UTC" } } };
  fs.writeFileSync(second.file("link.json"), JSON.stringify(withData));
  const refused = await second.start(["join"], code + "\n").done;
  assert.equal(refused.status, 1, refused.err);
  assert.match(refused.err, /This computer has setup data/);
  assert.deepEqual(second.state(), withData);

  // The same request reaches the primary through an SSH one-shot on the member.
  joining.startServer();
  await until("the member hears the primary's add feature", async () => {
    const result = await joining.request("GET", "kuvo/computers/add");
    return result.value.can;
  });
  const forwardedCode = await joining.request("POST", "kuvo/computers/add/code");
  assert.equal(forwardedCode.status, 201, JSON.stringify(forwardedCode.value));
  const privateCode = forwardedCode.value.code;
  const privateToken = JSON.parse(Buffer.from(privateCode.slice(prefix.length), "base64url")).token;
  const receipts = fs.readFileSync(primary.file("work-queue/acted.json"), "utf8");
  assert.ok(!receipts.includes(privateCode) && !receipts.includes(privateToken), "portable receipts do not contain a bearer code");
  const command = await joining.start(["box", "route"], JSON.stringify({ method: "POST", path: "kuvo/computers/add", body: { destination: "--non-host" } })).done;
  assert.equal(command.status, 0, command.err);
  const forwarded = JSON.parse(command.out);
  assert.equal(forwarded.status, 400, forwarded.body);
  assert.match(JSON.parse(forwarded.body).message, /Give a host name/);
  const added = await joining.request("POST", "kuvo/computers/add", { destination: "from-member.invalid" });
  assert.equal(added.status, 202, JSON.stringify(added.value));
  const attempt = await until("only the primary runs the forwarded SSH attempt", async () => {
    const primaryList = await primary.request("GET", "kuvo/computers/add");
    return primaryList.value.tries.find(attempt => attempt.id === added.value.try.id && attempt.state === "failed");
  });
  assert.equal(attempt.destination, "from-member.invalid");
  const memberList = await joining.request("GET", "kuvo/computers/add");
  assert.deepEqual(memberList.value.tries.find(attempt => attempt.id === added.value.try.id), attempt);
  assert.ok(fs.existsSync(path.join(primary.home, "ssh-called")), JSON.stringify(attempt.failure));
  assert.equal(fs.existsSync(path.join(joining.home, "ssh-called")), false);

  // A saved SSH address also uses this owner when a one-shot client asks to set it up again.
  fs.writeFileSync(path.join(primary.folder, "machines.json"), JSON.stringify([{ destination: "from-member.invalid" }]));
  fs.writeFileSync(path.join(primary.folder, "phone-link.json"), JSON.stringify({ machines: { "from-member.invalid": { key } } }));
  const rerun = await primary.start(["box", "route"], JSON.stringify({ method: "POST", path: `kuvo/computers/${key}/setup` })).done;
  assert.equal(rerun.status, 0, rerun.err);
  const reply = JSON.parse(rerun.out);
  assert.equal(reply.status, 202, reply.body);
  const operation = JSON.parse(reply.body).try;
  assert.equal(operation.kind, "setup");
  const seen = await primary.request("GET", "kuvo/computers/add");
  assert.ok(seen.value.tries.some(attempt => attempt.id === operation.id));
});
