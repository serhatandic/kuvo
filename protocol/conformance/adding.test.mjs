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

test("installer enrollment copies only the sign-ins and keys selected for that code", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const remote = await computer("Desk");
  const keyName = "KUVO_KEY_NOTION_TOKEN";
  const unselected = "KUVO_KEY_UNSELECTED_TOKEN";
  const selectedKey = "selected-test-key".repeat(2600);
  write(path.join(main.home, "box", "keys", keyName), selectedKey);
  write(path.join(main.home, "box", "keys", unselected), "unselected-test-key");
  main.engine.integrations.push({ id: "anthropic" }, { id: "notion", metadata: { source: "mcp" } });
  main.engine.credentials.push(
    { id: "selected", integrationID: "anthropic", label: "Work", value: { type: "api", key: "selected-test-credential" }, active: true },
    { id: "inactive", integrationID: "anthropic", label: "Old", value: { type: "api", key: "inactive-test-token" }, active: false },
    { id: "notion", integrationID: "notion", label: "Integration", value: { type: "api", key: "unsent-test-token" }, active: true },
    { id: "refreshing", integrationID: "anthropic", label: "Refreshes", value: { refresh: "unsent-refresh-token" }, active: true },
  );
  const ask = main.connect(await main.start().ready());
  const setup = (await ask("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const checklist = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(checklist.status, 200, checklist.text);
  assert.deepEqual(checklist.json.unavailable, []);
  assert.equal(checklist.json.renewing, 1);
  assert.ok(!checklist.json.items.some(item => item.label === "Refreshes"));
  const chosen = checklist.json.items.filter(item => item.label === keyName || item.label === "Work").map(item => item.id);
  assert.equal(chosen.length, 2);
  assert.doesNotMatch(checklist.text, /selected-test-key|selected-test-credential/);
  const issued = await ask("POST", "kuvo/computers/add/code", { credentials: chosen }, headers);
  assert.equal(issued.status, 201, issued.text);
  const mainLink = () => readJSON(path.join(main.home, "run", "link.json"));
  const invite = () => Object.values(mainLink().installerInvites)[0];
  assert.equal(typeof invite().credentialSeed, "string");
  assert.doesNotMatch(JSON.stringify(mainLink()), /selected-test-key|selected-test-credential/);
  remote.engine.blockCredentialImport = true;
  const interrupted = await remote.join(issued.json.code);
  assert.equal(interrupted.code, 1, interrupted.log);
  assert.match(interrupted.log, /same code/);
  assert.equal(readJSON(path.join(remote.home, "run", "link.json")).installerClaim.credentialsPending, true);
  assert.equal(typeof invite().credentialSeed, "string");
  remote.engine.blockCredentialImport = false;
  const joined = await remote.join(issued.json.code);
  assert.equal(joined.code, 0, joined.log);
  const keys = path.join(remote.home, "box", "keys");
  assert.deepEqual(fs.readdirSync(keys), [keyName]);
  const digest = bytes => createHash("sha256").update(bytes).digest("hex");
  assert.equal(digest(fs.readFileSync(path.join(keys, keyName))), digest(selectedKey));
  assert.deepEqual(remote.engine.credentials, [
    { integrationID: "anthropic", label: "Work", value: { type: "api", key: "selected-test-credential" }, activate: true },
  ]);
  await until(() => invite().credentialsCompleted && invite().credentialSeed === undefined,
    "The main computer did not acknowledge credential completion.");
  assert.equal((await remote.join(issued.json.code)).code, 0);
  assert.equal(remote.engine.credentials.length, 1);
});

test("a member lists the main computer’s keys and issues an installer code through it", { timeout: 120000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Workshop");
  const applicant = await computer("Laptop");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const first = await primary("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(first.status, 201, first.text);
  const enrolled = await member.join(first.json.code);
  assert.equal(enrolled.code, 0, enrolled.log);
  const ask = member.connect(await member.start().ready());
  const key = "KUVO_KEY_STUDIO_TOKEN";
  write(path.join(main.home, "box", "keys", key), "studio-test-key");
  write(path.join(member.home, "box", "keys", "KUVO_KEY_WORKSHOP_TOKEN"), "workshop-test-key");
  const checklist = await until(async () => {
    const answer = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
    if (answer.status === 200 && answer.json.items.some(item => item.label === key)) return answer.json;
    if (answer.status === 200) throw new Error("The member answered with its own checklist.");
  }, "The main computer did not answer the member’s checklist.", 25000);
  assert.deepEqual(checklist.items.map(item => item.label), [key]);
  const mainLink = () => readJSON(path.join(main.home, "run", "link.json"));
  const roster = mainLink().shared.membershipAndAccess.value.boxes;
  const studio = roster.find(computer => computer.name === "Studio");
  const workshop = roster.find(computer => computer.name === "Workshop");
  let droppedAfterOffer = false;
  const drop = relay.intercept(Buffer.from(studio.address, "base64url"), Buffer.from(workshop.address, "base64url"), () => {
    droppedAfterOffer ||= Object.keys(mainLink().installerInvites).length === 2;
    return true;
  });
  let granted;
  const forwarded = ask("POST", "kuvo/computers/add/code", { credentials: [checklist.items[0].id] }, headers);
  try {
    await until(() => droppedAfterOffer,
      "The main did not save the forwarded invitation before sending its reply.");
    fs.unlinkSync(path.join(main.home, "box", "keys", key));
  } finally {
    drop();
    granted = await forwarded;
  }
  assert.equal(granted.status, 201, granted.text);
  const recorded = mainLink().installerInvites;
  assert.equal(Object.keys(recorded).length, 2);
  const token = JSON.parse(Buffer.from(granted.json.code.split('.')[1], "base64url")).token;
  assert.ok(recorded[createHash("sha256").update(token).digest("base64url")]);
  assert.ok(!JSON.stringify(mainLink()).includes(token));
  assert.equal(Object.keys(readJSON(path.join(member.home, "run", "link.json")).installerInvites ?? {}).length, 0);
  const adopted = await applicant.join(granted.json.code);
  assert.equal(adopted.code, 0, adopted.log);
  assert.equal(fs.readFileSync(path.join(applicant.home, "box", "keys", key), "utf8"), "studio-test-key");
});

for (const confirmed of [false, true]) {
  test(confirmed
    ? "a saved installer claim cancels its provisional member after local work appears"
    : "a saved installer claim fences a delayed confirmation after local work appears",
  { timeout: 90000 }, async t => {
    const { computer, leases, relay } = await bench(t);
    const main = await computer("Studio");
    const applicant = await computer("Workshop");
    const stranger = await computer("Spare");
    if (!boxBinary().typescript) {
      const independent = applicant.start();
      const ready = applicant.connect(await independent.ready());
      const own = await ready("GET", "kuvo/setup");
      assert.equal(own.json.you.role, "primary", own.text);
      await independent.stop();
    }
    const ask = main.connect(await main.start().ready());
    const original = (await ask("GET", "kuvo/setup")).json;
    await until(() => leases.leaseFor(original.setup), "The main computer did not claim its lease.");
    const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
    assert.equal(invite.status, 201, invite.text);
    const ticket = JSON.parse(Buffer.from(invite.json.code.split(".")[1], "base64url"));
    const id = createHash("sha256").update(ticket.token).digest("base64url");
    const issuer = Buffer.from(ticket.address, "base64url");
    const prefix = Buffer.from("kuvo installer v1\0");
    const mainLink = () => readJSON(path.join(main.home, "run", "link.json"));
    const applicantLink = () => readJSON(path.join(applicant.home, "run", "link.json"));
    const roster = () => mainLink().shared.membershipAndAccess.value;
    let delayed, releaseReply;
    const releaseRequest = relay.intercept(null, issuer, packet => {
      if (!packet.payload.includes(prefix)) return false;
      if (confirmed) {
        releaseReply ??= relay.intercept(issuer, packet.from,
          reply => reply.payload.includes(prefix) && roster().boxes.length === 2);
      } else if (applicantLink().installerClaim?.invite === id) {
        delayed ??= packet;
        return true;
      }
      return false;
    });
    let held, independentSetup, signingKey;
    try {
      const interrupted = await applicant.join(invite.json.code, { afterStart: async child => {
        await until(() => confirmed ? roster().boxes.length === 2 : delayed,
          () => `The enrollment did not reach its saved claim.\n${child.log()}`);
        const current = applicantLink();
        held = current.installerClaim;
        independentSetup = current.setup.id;
        assert.equal(current.setup.role, "primary");
        if (confirmed) signingKey = mainLink().installerInvites[id].claim.sign;
        assert.equal(held.invite, id);
        assert.notEqual(held.adopted, true);
        await child.stop();
      } });
      assert.notEqual(interrupted.code, 0, interrupted.log);
    } finally {
      releaseRequest();
      releaseReply?.();
    }

    const project = path.join(applicant.home, "bots", "bot_independent", "bot.json");
    const work = '{"id":"bot_independent","name":"Kept"}';
    write(project, work);
    const before = mainLink().shared.membershipAndAccess.sequence;
    if (confirmed) {
      // Drop the cancellation acknowledgement once retirement is durable.
      let releaseAck;
      const releaseCancellation = relay.intercept(null, issuer, packet => {
        if (packet.payload.includes(prefix) && !releaseAck) {
          releaseAck = relay.intercept(issuer, packet.from,
            reply => reply.payload.includes(prefix) && roster().left?.includes(held.recovery.key));
        }
        return false;
      });
      try {
        await applicant.join(invite.json.code, { afterStart: async child => {
          await until(() => roster().left?.includes(held.recovery.key),
            "The main computer did not retire the provisional computer.");
          assert.equal(applicantLink().installerClaim.cancelPending, true);
          await child.stop();
        } });
      } finally {
        releaseCancellation();
        releaseAck?.();
      }
    }
    const rejected = await applicant.join(invite.json.code);
    assert.equal(rejected.code, 1, rejected.log);
    assert.match(rejected.log, /The enrollment was cancelled\. This computer kept its own setup\./);
    assert.equal(applicantLink().installerClaim, undefined);
    assert.equal(applicantLink().setup.id, independentSetup);
    assert.equal(applicantLink().setup.role, "primary");
    assert.equal(fs.readFileSync(project, "utf8"), work);
    assert.equal(mainLink().installerInvites[id].cancelled, true);
    if (delayed) relay.replay(delayed);

    const reused = await stranger.join(invite.json.code);
    assert.equal(reused.code, 1, reused.log);
    assert.match(reused.log, /This enrollment was cancelled/);
    assert.deepEqual(roster().boxes.map(peer => peer.key), [original.you.key]);
    assert.deepEqual(roster().left ?? [], confirmed ? [held.recovery.key] : []);
    if (confirmed) assert.ok(roster().computerHistory.retiredSigns.includes(signingKey));
    assert.equal(mainLink().shared.membershipAndAccess.sequence, before + Number(confirmed));
  });
}
