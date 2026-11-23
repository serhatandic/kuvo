// Installer codes run through two real residents and a private relay. The same
// scenario can pair two different box binaries during the switch-over.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, readJSON, scratch, until, write } from "./adapter.mjs";
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

test("the main retires only the provisional computer named by an installer cancellation receipt", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const implementation = process.env.KUVO_CONFORMANCE_JOIN_BIN
    ? boxBinary(process.env.KUVO_CONFORMANCE_JOIN_BIN) : boxBinary();
  const applicant = await computer("Workshop", implementation);
  const ask = main.connect(await main.start().ready());
  const original = (await ask("GET", "kuvo/setup")).json;
  await until(() => leases.leaseFor(original.setup), "The main computer did not claim a relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const invite = await ask("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(invite.status, 201, invite.text);
  const code = JSON.parse(Buffer.from(invite.json.code.split(".")[1], "base64url"));
  const issuer = Buffer.from(code.address, "base64url");
  const prefix = Buffer.from("kuvo installer v1\0");
  const mainLink = () => readJSON(path.join(main.home, "run", "link.json"));
  const roster = () => mainLink().shared.membershipAndAccess.value;
  const saved = () => roster().boxes.length === 2;
  let releaseReply;
  const releaseRequest = relay.intercept(null, issuer, ({ from, payload }) => {
    if (!releaseReply && payload.includes(prefix)) {
      releaseReply = relay.intercept(issuer, from, ({ payload }) => saved() && payload.includes(prefix));
    }
    return false;
  });
  let issued;
  try {
    await applicant.join(invite.json.code, { afterStart: async child => {
      await until(saved, "The main computer did not save the provisional entry.");
      const state = readJSON(path.join(applicant.home, "run", "link.json"));
      const { setup, invite, recovery } = state.installerClaim;
      issued = { setup, invite, ...recovery };
      assert.equal(typeof issued.secret, "string");
      await child.stop();
    } });
  } finally {
    releaseRequest();
    releaseReply?.();
  }
  const receipt = value => "kuvo-cancel-1." + Buffer.from(JSON.stringify(value)).toString("base64url");
  const applicantLink = path.join(applicant.home, "run", "link.json");
  const stopped = readJSON(applicantLink);
  stopped.installerClaim.until = Date.now() - 1;
  write(applicantLink, JSON.stringify(stopped));
  const prepared = await applicant.command(["installer-recovery", "issue"]);
  assert.equal(prepared.code, 0, prepared.log);
  assert.equal(prepared.text.trim(), receipt(issued));
  assert.equal(readJSON(applicantLink).installerClaim.cancelPending, true);
  const apply = (value, from = headers) => ask("POST", "kuvo/setup/installer-recovery", { receipt: receipt(value) }, from);
  const previous = mainLink().shared.membershipAndAccess.sequence;
  const remote = await apply(issued, {});
  assert.equal(remote.status, 403, remote.text);
  const future = await ask("POST", "kuvo/setup/installer-recovery", { receipt: "kuvo-cancel-2.e30" }, headers);
  assert.equal(future.status, 409, future.text);
  assert.match(future.json.message, /kuvo-cancel format version 2/);
  const forged = await apply({ ...issued, key: original.you.key });
  assert.equal(forged.status, 409, forged.text);
  assert.match(forged.json.message, /no matching installer recovery entry/);
  assert.equal(mainLink().shared.membershipAndAccess.sequence, previous);
  const removed = await main.command(["installer-recovery", "apply"], prepared.text);
  assert.equal(removed.code, 0, removed.log);
  assert.equal(removed.text.trim(), "The provisional installer entry was retired.");
  assert.deepEqual(roster().boxes.map(peer => peer.key), [original.you.key]);
  assert.deepEqual(roster().left, [issued.key]);
  const replay = await apply(issued);
  assert.equal(replay.status, 200, replay.text);
  assert.equal(replay.json.message, "The provisional installer entry was already retired.");
  assert.equal(mainLink().shared.membershipAndAccess.sequence, previous + 1);
  assert.equal(mainLink().installerInvites[issued.invite].cancelled, true);
  const clean = await ask("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(clean.status, 201, clean.text);
  const joined = await applicant.join(clean.json.code);
  assert.equal(joined.code, 0, joined.log);
  assert.notEqual(readJSON(applicantLink).key, stopped.key);
  const archived = await applicant.command(["installer-recovery", "issue"]);
  assert.equal(archived.code, 0, archived.log);
  assert.equal(archived.text, prepared.text);
});

test("a seed keeps existing sign-ins and keys and takes portable credentials just once", async t => {
  const { computer } = await bench(t);
  const remote = await computer("Desk");
  const own = { integrationID: "anthropic", label: "Local", value: { key: "existing" } };
  remote.engine.credentials.push(own);
  const key = path.join(remote.home, "box", "keys", "KUVO_KEY_GITHUB_TOKEN");
  const marker = path.join(remote.home, "run", "seeded.json");
  const seed = async given => {
    const result = await remote.command(["box", "seed"], JSON.stringify(given));
    assert.equal(result.code, 0, result.log);
    return JSON.parse(result.text);
  };
  const credentials = [
    { integrationID: "anthropic", label: "Source", value: { key: "should-stay-local" } },
    { integrationID: "openai", label: "Renewing", value: { refresh: "rotating" } },
    { integrationID: "github-copilot", label: "Portable", value: { refresh: "shared" } },
    { integrationID: "../data", label: "Invalid account", value: { key: "invalid" } },
  ];
  const keys = { KUVO_KEY_GITHUB_TOKEN: "example-fixture-value" };
  const future = await seed({ formatVersion: 2, credentials, keys });
  assert.equal(future.status, 400);
  assert.match(future.message, /Box seed format version 2/);
  assert.equal(fs.existsSync(marker), false);
  assert.equal(fs.existsSync(key), false);
  assert.deepEqual(remote.engine.credentials, [own]);

  write(path.join(remote.home, "bin", "integrations.json"), JSON.stringify({ integrations: [{
    id: "github", variables: { TOKEN: "KUVO_KEY_GITHUB_TOKEN" },
    settings: { type: "remote", url: "https://api.githubcopilot.com/mcp/", oauth: false,
      headers: { Authorization: "Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_GITHUB_TOKEN}" }, codemode: false },
  }] }));
  const copy = await seed({ credentials, keys, integrations: [{ id: "github", disabled: true, url: "https://outside.invalid" }] });
  assert.deepEqual(copy, { status: 200, taken: 1, keys: 1 });
  assert.deepEqual(remote.engine.credentials, [own, { ...credentials[2], activate: true }]);
  assert.equal(fs.readFileSync(key, "utf8"), keys.KUVO_KEY_GITHUB_TOKEN);
  const integration = readJSON(path.join(remote.home, "box", "opencode.json")).mcp.github;
  assert.equal(integration.disabled, true);
  assert.equal(integration.url, "https://api.githubcopilot.com/mcp/");
  assert.ok(integration.headers.Authorization.endsWith("/KUVO_KEY_GITHUB_TOKEN}"));
  assert.equal(readJSON(marker).taken, 1);
  const retry = await seed({ formatVersion: 1, credentials: [
    { integrationID: "poe", value: { key: "not-another-copy" } },
  ], keys: { KUVO_KEY_GITHUB_TOKEN: "do-not-overwrite" } });
  assert.deepEqual(retry, { status: 200, taken: 0, keys: 1 });
  assert.equal(remote.engine.credentials.length, 2);
  assert.equal(fs.readFileSync(key, "utf8"), keys.KUVO_KEY_GITHUB_TOKEN);
});
