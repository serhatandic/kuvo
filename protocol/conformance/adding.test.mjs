// Installer codes run through two real residents and a private relay. The same
// scenario can pair two different box binaries during the switch-over.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, launch, privateHome, readJSON, scratch, until, write } from "./adapter.mjs";
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

  const denied = await ask("POST", "kuvo/computers/add/code", { credentials: "all" }, { "x-kuvo-from": "mac" });
  assert.equal(denied.status, 400, denied.text);
  assert.deepEqual(denied.json, { message: "Choose sign-ins and keys by their checklist IDs." });

  const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(original.json.setup), "The main computer did not claim a relay lease.");
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
  privateHome(home);
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const version of [0, 2]) {
    const args = implementation.typescript ? ["join"] : ["install", "--join"];
    const process = boxCommand(implementation, home, isolatedEnv(root), { url: "ws://127.0.0.1:1" }, args);
    t.after(() => process.stop());
    process.child.stdin.end(`kuvo-add-${version}.e30\n`);
    assert.equal((await process.exited).code, 1);
    assert.match(process.log(), new RegExp(`kuvo-add format version ${version} is unsupported`));
    assert.deepEqual(fs.readdirSync(home), []);
    if (!implementation.typescript) {
      const fresh = path.join(root, `fresh-${version}`);
      const going = launch(implementation.file, args, { ...isolatedEnv(root), KUVO_HOME: fresh }, root);
      t.after(() => going.stop());
      going.child.stdin.end(`kuvo-add-${version}.e30\n`);
      assert.equal((await going.exited).code, 1);
      assert.match(going.log(), new RegExp(`kuvo-add format version ${version} is unsupported`));
      assert.equal(fs.existsSync(fresh), false, "a rejected code does not make an installation directory");
    }
  }
});

test("ordinary installer and SSH enrollment leave a used independent setup intact", { timeout: 45_000 }, async t => {
  const { computer, leases } = await bench(t);
  const issuer = await computer("Studio");
  const independent = await computer("Independent", undefined, undefined, { canonical: true });
  const resident = independent.start();
  const own = await independent.connect(await resident.ready())("GET", "kuvo/setup");
  assert.equal(own.status, 200, own.text);
  assert.equal(own.json.you.role, "primary");
  await resident.stop();
  const session = '{"id":"ses_here"}\n';
  const work = path.join(independent.home, "agents", "sessions", "ses_here", "session.json");
  write(work, session, { mode: 0o600 });
  const link = path.join(independent.home, "run", "link.json");
  const before = readJSON(link);
  const ask = issuer.connect(await issuer.start().ready());
  const original = await ask("GET", "kuvo/setup");
  assert.notEqual(original.json.setup, own.json.setup);
  // The lease is claimed on demand: issuing the code starts it.
  const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "app" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(original.json.setup), "Studio did not claim its lease.");

  const ts = boxBinary().typescript;
  const result = await independent.command(ts ? ["join"] : ["install", "--join"], invite.json.code + "\n");
  assert.equal(result.code, 1, result.log);
  assert.match(result.log, /setup merge/i);
  if (!ts) {
    const ssh = await independent.command(["ssh-enroll", "Independent"], invite.json.code + "\n");
    assert.equal(ssh.code, 1, ssh.log);
    assert.match(ssh.log, /setup merge/i);
  } else if (process.platform !== "win32") {
    const from = readJSON(path.join(issuer.home, "run", "link.json"));
    const token = `guarded-${"56".repeat(24)}`;
    fs.symlinkSync(token, path.join(path.dirname(independent.home), ".kuvo-code-adding"));
    const ssh = await independent.command(["setup", token], JSON.stringify({ mac: from.setup.primary, macSign: from.setup.sign, relay: from.relay }));
    assert.notEqual(ssh.code, 0, ssh.log);
    assert.match(ssh.log, /already has a Kuvo setup/i);
  }
  assert.deepEqual(readJSON(link), before);
  assert.equal(fs.readFileSync(work, "utf8"), session);
  assert.deepEqual((await ask("GET", "kuvo/setup")).json.computers, original.json.computers);
});

test("a Rust applicant leaves an older home for the move", {
  skip: boxBinary().typescript,
}, async t => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  privateHome(home);
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
  const headers = { "x-kuvo-from": "mac" };
  const invite = await ask("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(original.setup), "The main computer did not claim a relay lease.");
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
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const remote = await computer("Desk");
  const keyName = "KUVO_KEY_NOTION_TOKEN";
  const unselected = "KUVO_KEY_UNSELECTED_TOKEN";
  const selectedKey = "selected-test-key".repeat(2600);
  write(path.join(main.home, "box", "keys", keyName), selectedKey);
  write(path.join(main.home, "box", "keys", unselected), "unselected-test-key");
  main.engine.integrations.push({ id: "anthropic" }, { id: "github-copilot" }, { id: "notion", metadata: { source: "mcp" } });
  const copilot = { type: "oauth", refresh: "reusable-test-token", access: "reusable-test-token" };
  main.engine.credentials.push(
    { id: "selected", integrationID: "anthropic", label: "Work", value: { type: "api", key: "selected-test-credential" }, active: true },
    { id: "inactive", integrationID: "anthropic", label: "Old", value: { type: "api", key: "inactive-test-token" }, active: false },
    { id: "notion", integrationID: "notion", label: "Integration", value: { type: "api", key: "unsent-test-token" }, active: true },
    { id: "refreshing", integrationID: "anthropic", label: "Refreshes", value: { refresh: "unsent-refresh-token" }, active: true },
    { id: "copilot", integrationID: "github-copilot", label: "Copilot", value: copilot, active: true },
  );
  const ask = main.connect(await main.start().ready());
  const setup = (await ask("GET", "kuvo/setup")).json.setup;
  const headers = { "x-kuvo-from": "mac" };
  const checklist = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(checklist.status, 200, checklist.text);
  assert.deepEqual(checklist.json.unavailable, []);
  assert.equal(checklist.json.renewing, 1);
  assert.deepEqual(checklist.json.items.filter(item => item.kind === "sign-in").map(item => item.label), ["Work", "Copilot"]);
  const chosen = checklist.json.items.filter(item => [keyName, "Work", "Copilot"].includes(item.label)).map(item => item.id);
  assert.equal(chosen.length, 3);
  assert.doesNotMatch(checklist.text, /selected-test-key|selected-test-credential|reusable-test-token/);
  const issued = await ask("POST", "kuvo/computers/add/code", { credentials: chosen }, headers);
  assert.equal(issued.status, 201, issued.text);
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
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
  main.engine.credentials[0].value.key = "source-key-rotated-after-consent";
  write(path.join(main.home, "box", "keys", keyName), "source-integration-key-rotated-after-consent");
  remote.engine.blockCredentialImport = false;
  if (process.platform !== "win32") {
    const issuer = mainLink().shared.membershipAndAccess.value.boxes.find(peer => peer.name === "Studio");
    const prefix = Buffer.from("kuvo installer v1\0");
    const address = Buffer.from(issuer.address, "base64url");
    const replies = new Map();
    const watch = relay.intercept(null, address, ({ from, payload }) => {
      const sender = from.toString("base64url");
      if (payload.includes(prefix) && !replies.has(sender)) {
        replies.set(sender, relay.intercept(address, from,
          ({ payload: reply }) => invite().credentialsCompleted && reply.includes(prefix)));
      }
      return false;
    });
    try {
      const cut = await remote.join(issued.json.code, { afterStart: async installer => {
        await until(() => invite().credentialsCompleted, "The issuer did not retire the consumed credential copy.", 20000);
        installer.child.kill("SIGTERM");
      }});
      assert.equal(cut.signal, "SIGTERM", cut.log);
      assert.equal(readJSON(path.join(remote.home, "run", "link.json")).installerClaim.credentialsAckPending, true);
    } finally {
      watch();
      for (const resume of replies.values()) resume();
    }
  }
  const joined = await remote.join(issued.json.code);
  assert.equal(joined.code, 0, joined.log);
  const keys = path.join(remote.home, "box", "keys");
  assert.deepEqual(fs.readdirSync(keys), [keyName]);
  const digest = bytes => createHash("sha256").update(bytes).digest("hex");
  assert.equal(digest(fs.readFileSync(path.join(keys, keyName))), digest(selectedKey));
  assert.deepEqual(remote.engine.credentials, [
    { integrationID: "anthropic", label: "Work", value: { type: "api", key: "selected-test-credential" }, activate: true },
    { integrationID: "github-copilot", label: "Copilot", value: copilot, activate: true },
  ]);
  await until(() => invite().credentialsCompleted && invite().credentialSeed === undefined,
    "The main computer did not acknowledge credential completion.");
  assert.equal(readJSON(path.join(remote.home, "run", "link.json")).installerClaim.credentialsAckPending, undefined);
  assert.equal((await remote.join(issued.json.code)).code, 0);
  assert.equal(remote.engine.credentials.length, 2);
});

test("a member lists the main computer’s keys and issues an installer code through it", { timeout: 120000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Workshop");
  const applicant = await computer("Laptop");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  const headers = { "x-kuvo-from": "mac" };
  const first = await primary("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(first.status, 201, first.text);
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
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
    {
      // Its own resident ran first: the empty stores it opened aren't setup data.
      const independent = applicant.start();
      const ready = applicant.connect(await independent.ready());
      const own = await ready("GET", "kuvo/setup");
      assert.equal(own.json.you.role, "primary", own.text);
      await independent.stop();
    }
    const ask = main.connect(await main.start().ready());
    const original = (await ask("GET", "kuvo/setup")).json;
    const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
    assert.equal(invite.status, 201, invite.text);
    await until(() => leases.leaseFor(original.setup), "The main computer did not claim its lease.");
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
    if (confirmed) assert.match(rejected.log, /This computer could not join\./);
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

test("a new main can retire a provisional installer computer from its replicated claim", { timeout: 120000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const issuer = await computer("Studio");
  const successor = await computer("Lab");
  const observer = await computer("Observer");
  const applicant = await computer("Workshop");
  const replacement = await computer("Replacement");
  const stateFile = box => path.join(box.home, "run", "link.json");
  const state = box => readJSON(stateFile(box));
  const clock = (box, offset) => write(path.join(box.home, "run", "test-setup-clock.json"), JSON.stringify({ offset }));
  for (const box of [issuer, successor, observer]) clock(box, 0);
  write(path.join(successor.home, "setup-takeover.json"), '{"alwaysOn":true}');
  const runningIssuer = issuer.start();
  const ask = issuer.connect(await runningIssuer.ready());
  const view = (await ask("GET", "kuvo/setup")).json;
  const headers = { "x-kuvo-from": "mac" };
  const desktop = Buffer.alloc(32, 74).toString("base64url");
  const paired = await ask("POST", "kuvo/devices/pair", { key: desktop, kind: "desktop", name: "Retired desktop" },
    { "x-kuvo-from": "app" });
  assert.equal(paired.status, 200, paired.text);
  const denial = { key: desktop, pairing: paired.json.pairing };
  const revoked = await ask("POST", "kuvo/phones/remove", { phones: [denial] }, headers);
  assert.equal(revoked.status, 200, revoked.text);
  const issue = async (at = ask) => {
    const reply = await at("POST", "kuvo/computers/add/code", {}, headers);
    assert.equal(reply.status, 201, reply.text);
    return reply.json.code;
  };
  const first = await issue();
  await until(() => leases.leaseFor(view.setup), "The issuer did not acquire the initial lease.");
  const joined = await successor.join(first);
  assert.equal(joined.code, 0, joined.log);
  const runningSuccessor = successor.start();
  const receive = successor.connect(await runningSuccessor.ready());
  const lab = (await receive("GET", "kuvo/setup")).json.you.key;
  const received = await observer.join(await issue());
  assert.equal(received.code, 0, received.log);
  const observe = observer.connect(await observer.start().ready());
  const observed = (await observe("GET", "kuvo/setup")).json.you.key;
  await until(() => {
    const roster = leases.leaseFor(view.setup)?.roster;
    return roster?.[lab] && roster[observed];
  }, "The relay lease did not include both members.", 30000);

  const code = await issue();
  const ticket = JSON.parse(Buffer.from(code.split(".")[1], "base64url"));
  const issuerAddress = Buffer.from(ticket.address, "base64url");
  const prefix = Buffer.from("kuvo installer v1\0");
  const recoveries = box => {
    const current = state(box);
    return current.shared.ownershipRecovery?.value?.["installer recovery"]
      ?? current.shared.ownershipAssignments?.value?.["installer recovery"]
      ?? current.shared.membershipAndAccess.value.installerRecoveries;
  };
  const roster = box => state(box).shared.membershipAndAccess.value;
  const claimed = () => state(applicant).installerClaim;
  let releaseReply, pending, delayed;
  const releaseRequest = relay.intercept(null, issuerAddress, packet => {
    if (packet.payload.includes(prefix)) {
      if (claimed()?.recovery?.key) delayed = packet;
      releaseReply ??= relay.intercept(issuerAddress, packet.from, ({ payload }) =>
        payload.includes(prefix) && !!recoveries(issuer)?.[claimed()?.recovery?.key]);
    }
    return false;
  });
  try {
    const interrupted = await applicant.join(code, { afterStart: async child => {
      await until(() => {
        const entry = recoveries(issuer)?.[claimed()?.recovery?.key];
        return entry?.commitment && recoveries(successor)?.[claimed()?.recovery?.key]?.commitment === entry.commitment;
      }, "Lab did not receive the provisional recovery commitment.", 25000);
      pending = claimed();
      await child.stop();
    } });
    assert.notEqual(interrupted.code, 0, interrupted.log);
  } finally {
    releaseRequest();
    releaseReply?.();
  }
  const applicantKey = pending.recovery.key;
  const original = recoveries(issuer)[applicantKey];
  const unchanged = await applicant.command(["installer-recovery", "issue"]);
  assert.equal(unchanged.code, 1, unchanged.log);
  const expired = state(applicant);
  expired.installerClaim.until = Date.now() - 1;
  write(stateFile(applicant), JSON.stringify(expired));
  const issued = await applicant.command(["installer-recovery", "issue"]);
  assert.equal(issued.code, 0, issued.log);
  assert.match(issued.text, /^kuvo-cancel-1\./);
  write(stateFile(replacement), JSON.stringify(state(applicant)));
  const localWork = path.join(applicant.home, "agents", "sessions", "local.json");
  const savedWork = '{"title":"Saved after the claim"}';
  write(localWork, savedWork);
  await runningIssuer.stop();
  const allowIssuer = leases.disconnect(view.you.key);
  const allowObserver = leases.disconnect(observed);
  try {
    leases.advance(125000);
    clock(successor, 125000);
    clock(observer, 125000);
    await until(() => state(successor).setup.role === "primary",
      () => "Lab did not take over while Studio was off.\n" + runningSuccessor.log(), 30000);
    allowObserver();
    assert.equal(leases.leaseFor(view.setup).holder, lab);
    assert.deepEqual(recoveries(successor)[applicantKey], original);
    assert.equal(Object.hasOwn(state(successor).installerInvites ?? {}, pending.invite), false);
    const before = roster(successor);
    assert.ok(before.boxes.some(peer => peer.key === applicantKey));
    const forged = JSON.parse(Buffer.from(issued.text.trim().split(".")[1], "base64url"));
    const badReceipt = "kuvo-cancel-1." + Buffer.from(JSON.stringify({ ...forged, key: observed })).toString("base64url") + "\n";
    const denied = await successor.command(["installer-recovery", "apply"], badReceipt);
    assert.equal(denied.code, 1, denied.log);
    assert.match(denied.log, /no matching installer recovery entry/);
    const result = await successor.command(["installer-recovery", "apply"], issued.text);
    assert.equal(result.code, 0, result.log);
    assert.match(result.text, /provisional installer entry was retired/);
    const after = roster(successor);
    assert.deepEqual(after.boxes.map(peer => peer.key).sort(), before.boxes.map(peer => peer.key).filter(key => key !== applicantKey).sort());
    assert.deepEqual(after.left, [applicantKey]);
    assert.ok(after.computerHistory.retiredSigns.includes(original.sign));
    assert.equal(recoveries(successor)?.[applicantKey], undefined);
    assert.ok(after.devices.removed.some(entry => entry.key === denial.key && entry.pairing === denial.pairing));
    const savedSlots = new Set(before.computerHistory.slots.map(slot => slot.key));
    assert.deepEqual(after.computerHistory.slots.filter(slot => savedSlots.has(slot.key)), before.computerHistory.slots);
    assert.ok(after.computerHistory.slots.some(slot => slot.key === applicantKey));
    await until(() => state(observer).setup.primary === lab && roster(observer).left?.includes(applicantKey),
      "The observer did not apply the new main's signed retirement.", 30000);
    const replay = await successor.command(["installer-recovery", "apply"], issued.text);
    assert.equal(replay.code, 0, replay.log);
    assert.match(replay.text, /provisional installer entry was already retired/);

    const freshCode = await issue(receive);
    const refused = await applicant.join(freshCode);
    assert.equal(refused.code, 1, refused.log);
    assert.match(refused.log, /Use a setup merge/);
    assert.equal(fs.readFileSync(localWork, "utf8"), savedWork);
    const enrolled = await replacement.join(freshCode);
    assert.equal(enrolled.code, 0, enrolled.log);
    const returned = state(replacement);
    assert.equal(returned.setup.id, view.setup);
    for (const field of ["key", "sign", "secret"]) assert.notEqual(returned[field], expired[field], field);
    const archived = await replacement.command(["installer-recovery", "issue"]);
    assert.equal(archived.code, 0, archived.log);
    assert.equal(archived.text, issued.text);

    allowIssuer();
    clock(issuer, 125000);
    await issuer.start().ready();
    await until(() => state(issuer).setup.primary === lab && roster(issuer).left?.includes(applicantKey),
      "The former issuer did not follow the retirement.", 30000);
    assert.ok(delayed, "The original confirmation was captured.");
    let answered = false;
    const watch = relay.intercept(issuerAddress, delayed.from, ({ payload }) => {
      answered ||= payload.includes(prefix);
      return false;
    });
    try {
      relay.replay(delayed);
      await until(() => answered, "The former issuer did not answer the old confirmation.");
    } finally {
      watch();
    }
    assert.equal(roster(issuer).boxes.some(peer => peer.key === applicantKey), false);
    assert.deepEqual(roster(issuer).devices.removed, after.devices.removed);
  } finally {
    allowIssuer();
    allowObserver();
  }
});
