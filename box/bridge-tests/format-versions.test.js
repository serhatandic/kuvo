import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-format-"));
removeAtEnd(home);
process.env.KUVO_HOME = home;
const script = LINK_BUNDLE;
const link = await import(script);

test("authenticated peer mail accepts the old payload and emits a version older boxes can ignore", t => {
  const mine = link.newKey();
  const theirs = link.newKey();
  const source = { key: mine.toString("base64url") };
  const peer = { key: link.publicOf(theirs).toString("base64url"), address: Buffer.alloc(16).toString("base64url") };
  const destination = { key: theirs.toString("base64url"), mac: link.publicOf(mine).toString("base64url") };
  const payload = { ask: "agents", at: 2 };
  const sent = link.readFrame(link.mailFor(peer, payload, source).subarray(17)).body;
  assert.deepEqual(JSON.parse(link.openOneWay(sent, theirs, link.publicOf(mine)).payload), { ...payload, formatVersion: 1 });
  assert.deepEqual(link.openMail(sent, destination).payload, payload);
  const seal = body => link.sealOneWay(Buffer.from(JSON.stringify(body)), mine, link.publicOf(theirs)).message;
  assert.deepEqual(link.openMail(seal(payload), destination).payload, payload);
  const warning = t.mock.method(console, "warn", () => {});
  assert.equal(link.openMail(seal({ ...payload, formatVersion: 2 }), destination), null);
  assert.match(warning.mock.calls[0].arguments[0], /Peer mail format version 2 is unsupported/);
});

test("SSH route answers carry their version and future requests are refused before dispatch", () => {
  const header = "x-kuvo-route-version";
  function route(given) {
    const child = spawnSync(process.execPath, [fileURLToPath(script), "box", "route"], {
      input: JSON.stringify({ method: "GET", path: "kuvo/info", headers: given }), encoding: "utf8", timeout: 10_000,
    });
    assert.equal(child.status, 0, child.stderr);
    return JSON.parse(child.stdout);
  }
  const legacy = route({});
  assert.equal(legacy.status, 200);
  assert.equal(legacy.routeVersion, "1");
  assert.equal(route({ [header]: "1" }).status, 200);
  assert.equal(route({ "X-Kuvo-Route-Version": "1" }).status, 200);
  for (const headers of [
    { [header]: "2" },
    { "X-Kuvo-Route-Version": "2" },
    { [header]: "1", "X-Kuvo-Route-Version": "2" },
    { "X-Kuvo-Route-Version": "2", [header]: "1" },
  ]) {
    const future = route(headers);
    assert.equal(future.status, 400, JSON.stringify(headers));
    assert.equal(future.routeVersion, "1");
    assert.equal(JSON.parse(future.body).code, "unsupportedFormatVersion");
  }
});

test("future ledger and link files are refused without making replacement data", () => {
  const ledger = path.join(home, "run", "work-queue", "numbers.json");
  const future = '{"leases":[[41,42]],"formatVersion":2,"next":43}';
  fs.mkdirSync(path.dirname(ledger), { recursive: true });
  fs.writeFileSync(ledger, future);
  assert.throws(() => link.askToSend(), /Work Queue number ledger format version 2 is unsupported/);
  assert.equal(fs.readFileSync(ledger, "utf8"), future);

  const linkFile = path.join(home, "run", "link.json");
  fs.writeFileSync(linkFile, future);
  assert.throws(() => link.updateLink(state => ({ ...state, phones: [] })), /Link state format version 2 is unsupported/);
  assert.equal(fs.readFileSync(linkFile, "utf8"), future);
});

test("HTTP refusal is sent before the Box route can change state", async () => {
  const auth = "Basic " + Buffer.from("opencode:pw").toString("base64");
  const server = link.serveBox("pw");
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const response = await fetch(`http://127.0.0.1:${server.address().port}/kuvo/info`, {
      headers: { authorization: auth, "x-kuvo-route-version": "2" },
    });
    assert.equal(response.status, 400);
    assert.equal(response.headers.get("x-kuvo-route-version"), "1");
    assert.equal((await response.json()).code, "unsupportedFormatVersion");
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});
