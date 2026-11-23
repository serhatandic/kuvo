// Installer codes run through two real residents and a private relay. The same
// scenario can pair two different box binaries during the switch-over.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, readJSON, scratch, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("an installer code binds one computer, survives a retry, and keeps its identity", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const memberImplementation = process.env.KUVO_CONFORMANCE_JOIN_BIN
    ? boxBinary(process.env.KUVO_CONFORMANCE_JOIN_BIN) : boxBinary();
  const newComputer = await computer("Workshop", memberImplementation);
  const stranger = await computer("Spare", memberImplementation);
  const ask = main.connect(await main.start().ready());
  const original = await ask("GET", "kuvo/setup");
  assert.equal(original.status, 200, original.text);
  await until(() => leases.leaseFor(original.json.setup), "The main computer did not claim a relay lease.");

  const denied = await ask("POST", "kuvo/computers/add/code", { credentials: "all" }, { "x-kuvo-from": "mac" });
  assert.equal(denied.status, 400, denied.text);
  assert.deepEqual(denied.json, { message: "Choose sign-ins and keys by their checklist IDs." });

  const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const { code, expires } = invite.json;
  const format = "kuvo-add-1.";
  assert.ok(code.startsWith(format));
  const { token, ...scope } = JSON.parse(Buffer.from(code.slice(format.length), "base64url"));
  assert.equal(scope.expires, expires);
  assert.equal(scope.setup, original.json.setup);
  assert.equal(scope.primary, original.json.you.key);
  const id = createHash("sha256").update(token).digest("base64url");
  const mainLink = () => readJSON(path.join(main.home, "run", "link.json"));
  assert.deepEqual(mainLink().installerInvites[id], { setup: scope.setup, epoch: 1, expires });
  assert.equal(JSON.stringify(mainLink()).includes(token), false);

  const applicant = () => readJSON(path.join(newComputer.home, "run", "link.json"));
  // Drop only the installer reply after the signed roster has been saved. The
  // applicant never sees the result and is stopped while its claim is on disk.
  const prefix = Buffer.from("kuvo installer v1\0");
  const mainAddress = Buffer.from(scope.address, "base64url");
  const committed = () => mainLink().shared.membershipAndAccess.value.boxes.length === 2;
  let releaseResponse;
  const stopWatching = relay.intercept(null, mainAddress, ({ from, payload }) => {
    if (!releaseResponse && payload.includes(prefix)) {
      releaseResponse = relay.intercept(mainAddress, from, ({ payload }) => committed() && payload.includes(prefix));
    }
    return false;
  });
  let reserved;
  try {
    const interrupted = await newComputer.join(code, { afterStart: async child => {
      await until(committed, "The main computer did not save membership before sending the answer.");
      reserved = applicant();
      assert.equal(reserved.installerClaim.invite, id);
      assert.notEqual(reserved.installerClaim.adopted, true);
      await child.stop();
    }});
    assert.notEqual(interrupted.code, 0);
  } finally {
    stopWatching();
    releaseResponse?.();
  }
  const joined = await newComputer.join(code);
  assert.equal(joined.code, 0, joined.log);
  const first = applicant();
  assert.equal(first.key, reserved.key);
  assert.equal(first.sign, reserved.sign);
  assert.equal(first.secret, reserved.secret);
  assert.equal(first.installerClaim.invite, id);
  assert.equal(first.installerClaim.adopted, true);
  assert.equal(first.installerClaim.activationPending, undefined);
  assert.equal(mainLink().installerInvites[id].activated, true);
  const retry = await newComputer.join(code);
  assert.equal(retry.code, 0, retry.log);
  const replayed = applicant();
  assert.equal(replayed.key, first.key);
  assert.equal(replayed.sign, first.sign);
  assert.equal(replayed.secret, first.secret);

  const secondUse = await stranger.join(code);
  assert.notEqual(secondUse.code, 0);
  assert.match(secondUse.log, /This installer code is no longer available/);
  const member = newComputer.connect(await newComputer.start().ready());
  const theirs = await member("GET", "kuvo/setup");
  assert.equal(theirs.status, 200, theirs.text);
  assert.equal(theirs.json.you.role, "member");
  assert.equal(theirs.json.setup, scope.setup);
  const primaryView = await ask("GET", "kuvo/setup");
  const roster = setup => setup.computers.map(({ key, name, role }) => ({ key, name, role }));
  assert.deepEqual(roster(primaryView.json), [
    { key: scope.primary, name: "Studio", role: "primary" },
    { key: theirs.json.you.key, name: "Workshop", role: "member" },
  ]);
  assert.deepEqual(roster(theirs.json), roster(primaryView.json));
});

test("an unsupported installer code leaves the new computer’s home untouched", async t => {
  const implementation = boxBinary();
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  fs.mkdirSync(home);
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const version of [0, 2]) {
    const args = implementation.typescript ? ["join"] : ["install", "--join"];
    const process = boxCommand(implementation, home, isolatedEnv(root), { url: "ws://127.0.0.1:1" }, args);
    t.after(() => process.stop());
    process.child.stdin.end(`kuvo-add-${version}.e30\n`);
    assert.equal((await process.exited).code, 1);
    assert.match(process.log(), new RegExp(`kuvo-add format version ${version} is unsupported`));
    assert.deepEqual(fs.readdirSync(home), []);
  }
});

test("a Rust applicant leaves an older home for the move", {
  skip: boxBinary().typescript,
}, async t => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  fs.mkdirSync(home);
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const layout = '{"version":22}\n';
  fs.writeFileSync(path.join(home, "folder-version.json"), layout);
  const code = {
    setup: "set_" + Buffer.alloc(16, 1).toString("base64url"),
    expires: Date.now() + 60000,
    relay: "ws://127.0.0.1:1",
    address: Buffer.alloc(16, 2).toString("base64url"),
    primary: Buffer.alloc(32, 3).toString("base64url"),
    token: Buffer.alloc(24, 4).toString("base64url"),
  };
  const process = boxCommand(boxBinary(), home, isolatedEnv(root), { url: code.relay }, ["install", "--join"]);
  t.after(() => process.stop());
  process.child.stdin.end("kuvo-add-1." + Buffer.from(JSON.stringify(code)).toString("base64url") + "\n");
  assert.equal((await process.exited).code, 1);
  assert.match(process.log(), /Kuvo folder layout 22 requires the move to layout 23/);
  assert.deepEqual(fs.readdirSync(home), ["folder-version.json"]);
  assert.equal(fs.readFileSync(path.join(home, "folder-version.json"), "utf8"), layout);
});
