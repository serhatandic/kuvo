import assert from "node:assert/strict";
import { createHash, createHmac } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { addSshStandIn } from "../../box/bridge-tests/add-ssh-stand-in.js";
import { boxBinary, boxCommand, holdWindowsFile, isolatedEnv, launch, privateHome, readJSON, relayAddress, request, scratch, typescriptRuntime, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";

const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));

async function loadWire(home, env) {
  const fixtureEnv = { ...env, KUVO_HOME: home };
  const previous = Object.fromEntries(Object.keys(fixtureEnv).map(key => [key, process.env[key]]));
  Object.assign(process.env, fixtureEnv);
  try {
    return await import(pathToFileURL(wireFixture).href);
  } finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

test("a transient addressed SSH Add refuses start, answer and cancel after authenticating", {
  skip: process.platform !== "darwin" || !fs.existsSync(wireFixture) && "Build the reference Noise fixture",
  timeout: 100_000,
}, async t => {
  const binary = boxBinary();
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const home = path.join(root, "box");
  privateHome(home);
  const children = [];
  const helper = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const send = (status, value) => res.writeHead(status, { "content-type": "application/json", "x-kuvo-host-version": "1" }).end(JSON.stringify(value));
    if (req.url === "/hello") {
      const { nonce } = JSON.parse(Buffer.concat(chunks));
      return send(200, { proof: createHmac("sha256", password).update(`kuvo-host-hello\0${nonce}\0${req.headers.host}`).digest("hex") });
    }
    if (req.url === "/link/identity" && req.headers.authorization === `Bearer ${password}`) return send(200, identity);
    return send(403, {});
  });
  t.after(async () => {
    await Promise.all(children.map(child => child.stop()));
    helper.closeAllConnections();
    await new Promise(resolve => helper.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  });
  const relay = { url: "ws://127.0.0.1:1" };
  const wire = await loadWire(home, env);
  const device = Buffer.alloc(32, 81);
  const pairedDevice = { key: wire.publicOf(device).toString("base64url"), name: "Paired desktop", kind: "desktop", pairing: "fixture" };
  const { main } = seedPeerSetup(home, relay.url, 1, 2, roster => {
    roster.boxes[0].kind = "mac";
    roster.devices.phones.push(pairedDevice);
  });
  const linkFile = path.join(home, "run", "link.json");
  const { key, secret, sign, ...link } = readJSON(linkFile);
  link.phones = [{ ...pairedDevice, from: "pair", added: 1 }];
  const identity = { key, secret, sign };
  write(linkFile, JSON.stringify(link), { mode: 0o600 });
  write(path.join(home, "box", "kind"), "mac\n");
  write(path.join(home, "run", "mac-link.json"), JSON.stringify({ phones: true, relay: relay.url }));
  const password = "transient-helper";
  await new Promise(resolve => helper.listen(0, "127.0.0.1", resolve));
  write(path.join(home, "run", "host.json"), JSON.stringify({ formatVersion: 1, password, url: `http://127.0.0.1:${helper.address().port}` }), { mode: 0o600 });

  const overSsh = async (given, sender = device) => {
    const command = binary.typescript ? ["box", "route-to"] : ["route-to"];
    const process = boxCommand(binary, home, { ...env, KUVO_KEYS_ON_HOST: "1" }, relay, command, {}, "pipe");
    children.push(process);
    const lines = createInterface({ input: process.child.stdout })[Symbol.asyncIterator]();
    const reply = async () => {
      const { value } = await lines.next();
      assert.ok(value, `SSH closed without a reply: ${process.log()}`);
      return JSON.parse(value);
    };
    const noise = new wire.IKInitiator(sender, Buffer.from(main.key, "base64url"), Buffer.from("kuvo-ssh v1"));
    const hello = noise.writeMessage1(Buffer.from('{"v":1,"client":"desktop"}')).toString("base64url");
    process.child.stdin.write(JSON.stringify({ computer: main.key, hello }) + "\n");
    const first = await reply();
    if (!first.hello) {
      process.child.stdin.end();
      return first;
    }
    const secure = noise.readMessage2(Buffer.from(first.hello, "base64url"));
    const allowed = JSON.parse(secure.payload).allowed;
    assert.equal(allowed, sender === device);
    process.child.stdin.end(allowed ? JSON.stringify({ request: secure.transport.seal(Buffer.from(JSON.stringify(given))).toString("base64url") }) + "\n" : undefined);
    return reply();
  };
  const routes = [
    { method: "POST", path: "kuvo/computers/add", body: { destination: "lab" } },
    { method: "POST", path: "kuvo/computers/add/add_fixture/answer", body: { answer: "trust", at: 1 } },
    { method: "DELETE", path: "kuvo/computers/add/add_fixture" },
  ];
  const [unpaired, ...paired] = await Promise.all([overSsh(routes[0], Buffer.alloc(32, 82)), ...routes.map(route => overSsh(route))]);
  assert.equal(unpaired.status, binary.typescript ? 403 : 503, unpaired.body);
  for (const answer of paired) assert.deepEqual(answer, {
    status: 409, routeVersion: "1", type: "application/json", computer: main.key,
    body: '{"message":"Ask this through Kuvo running on this computer: a single request can’t stay to do it."}',
  });
  assert.deepEqual(readJSON(linkFile), link, "The transient request must leave the setup unchanged.");
});

test("SSH Add trusts the selected host, copies the running Rust release and starts an enrolled resident", {
  skip: boxBinary().typescript || process.platform === "win32",
  timeout: 65000,
}, async t => {
  let remote;
  t.after(() => {
    const place = remote && path.join(remote, "box", "run", "serve.json");
    if (place && fs.existsSync(place)) {
      const owned = readJSON(place).pid;
      if (Number.isSafeInteger(owned) && owned > 1) {
        try { process.kill(owned, "SIGTERM"); } catch (error) {
          if (error.code !== "ESRCH") throw error;
        }
      }
    }
  });
  const { computer, leases, relay } = await bench(t);
  const key = Buffer.concat([Buffer.from("0000000b7373682d6564323535313900000020", "hex"), Buffer.alloc(32, 8)]).toString("base64");
  const main = await computer("Studio", boxBinary(), folder => {
    const fake = path.join(folder, "ssh");
    remote = path.join(folder, "remote");
    fs.mkdirSync(remote);
    const installed = path.join(folder, "box", "bin");
    fs.mkdirSync(installed, { recursive: true });
    fs.copyFileSync(boxBinary().file, path.join(installed, "kuvo-box"));
    const program = path.join(fake, "ssh");
    write(program, addSshStandIn, { mode: 0o700 });
    write(path.join(fake, "hostkey"), "ssh-ed25519 " + key);
    write(path.join(folder, "account", ".ssh", "config"), "Host lab\nHost *.example !ignore\nHost second\n");
    return { KUVO_TEST_SSH: program, KUVO_TEST_SSH_ROOT: "box", FAKE_SSH: fake, FAKE_SSH_HOME: remote };
  });
  const ask = main.connect(await main.start().ready());
  const setup = (await ask("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const page = () => ask("GET", "kuvo/computers/add");
  assert.ok(fs.statSync(path.join(main.home, "bin", "kuvo-box")).isFile(), "The installed test release is in the settled home.");
  assert.deepEqual((await page()).json, { can: true, hosts: ["lab", "second"], tries: [] });
  const invalid = await ask("POST", "kuvo/computers/add", { destination: "-oProxyCommand=sh" }, headers);
  assert.equal(invalid.status, 400, invalid.text);
  const selectedKey = "KUVO_KEY_SSH_SAMPLE";
  write(path.join(main.home, "box", "keys", selectedKey), "selected fixture key", { mode: 0o600 });
  write(path.join(main.home, "box", "keys", "KUVO_KEY_UNSELECTED"), "leave this key on Studio", { mode: 0o600 });
  const catalogKey = "KUVO_KEY_GITHUB_TOKEN";
  write(path.join(main.home, "box", "keys", catalogKey), "selected catalog key", { mode: 0o600 });
  write(path.join(main.home, "bin", "integrations.json"), JSON.stringify({ integrations: [{
    id: "github", variables: { TOKEN: catalogKey }, settings: {
      type: "remote", url: "https://api.githubcopilot.com/mcp/", oauth: false, codemode: false,
      headers: { Authorization: `Bearer {file:~/.kuvo-code/box/keys/${catalogKey}}` },
    },
  }] }));
  write(path.join(main.home, "box", "opencode.json"), JSON.stringify({ model: "anthropic/claude", shell: "/private/local-shell", worktree: "/private/worktree", mcp: {
    github: { type: "remote", url: "https://untrusted.invalid", disabled: true, headers: { Authorization: "inline-secret" } },
    permitted: { type: "local", command: ["fixture"], environment: { TOKEN: `{file:/source/home/keys/${selectedKey}}` } },
    excluded: { type: "local", command: ["fixture"], environment: { TOKEN: "inline-secret-do-not-copy" } },
  } }));
  main.engine.integrations.push({ id: "anthropic" }, { id: "github-copilot" }, { id: "openai" }, { id: "notion", metadata: { source: "mcp" } });
  main.engine.credentials.push(
    { id: "fixture-key", integrationID: "anthropic", label: "Selected", active: true, value: { type: "api", key: "selected fixture sign-in" } },
    { id: "fixture-stale", integrationID: "anthropic", label: "Inactive", active: false, value: { type: "api", key: "inactive fixture sign-in" } },
    { id: "fixture-renews", integrationID: "openai", label: "Renewing", active: true, value: { type: "oauth", access: "fixture", refresh: "fixture" } },
    { id: "fixture-mcp", integrationID: "notion", label: "MCP", active: true, value: { type: "oauth", access: "fixture" } },
    { id: "fixture-stable", integrationID: "github-copilot", label: "Copilot", active: true, value: { type: "oauth", access: "fixture", refresh: "stable fixture" } },
  );
  const checklist = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(checklist.status, 200, checklist.text);
  const credentials = checklist.json.items.filter(item => [selectedKey, catalogKey, "Selected", "Copilot"].includes(item.label)).map(item => item.id);
  assert.equal(credentials.length, 4);
  assert.equal(checklist.json.renewing, 1);
  assert.equal(checklist.json.items.some(item => ["Inactive", "Renewing", "MCP"].includes(item.label)), false);
  const begun = await ask("POST", "kuvo/computers/add", { destination: "lab", credentials }, headers);
  assert.equal(begun.status, 202, begun.text);
  const id = begun.json.try.id;
  const readTry = async () => (await page()).json.tries.find(attempt => attempt.id === id);
  const found = await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "asking" && attempt;
  }, "The host key was never shown.");
  assert.equal(found.question.id, "hostKey");
  assert.equal(found.question.title, "Is this lab?");
  assert.match(found.question.facts[1].value, /^SHA256:/);
  assert.deepEqual(fs.readdirSync(remote), []);
  const engineCopy = stageEngine(t, path.join(remote, "box"));
  const oldCache = path.join(remote, "Library", "Application Support", "run.kuvo.code", "Caches", "old-cache");
  write(oldCache, "cache from an earlier install");
  const answer = at => ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at }, headers);
  assert.equal((await answer(found.question.at + 1)).status, 409);
  assert.equal(fs.existsSync(path.join(main.home, "ssh", "known_hosts")), false);
  const fake = path.join(path.dirname(remote), "ssh");
  const hold = path.join(fake, "hold-lab");
  const marker = path.join(remote, "box", "run", "ssh-adding");
  write(hold, "Publish a new executable");
  t.after(() => fs.rmSync(hold, { force: true }));
  assert.equal((await answer(found.question.at)).status, 200);
  const claimed = await until(() => {
    try { return fs.readlinkSync(marker); } catch (error) { if (error.code !== "ENOENT") throw error; }
  }, "The first Add did not reserve the remote account.");
  fs.appendFileSync(path.join(main.home, "ssh", "known_hosts"), `busy-alias ssh-ed25519 ${key}\n`);
  const busy = await ask("POST", "kuvo/computers/add", { destination: "busy-alias" }, headers);
  assert.equal(busy.status, 202, busy.text);
  const other = await until(async () => {
    const attempt = (await page()).json.tries.find(attempt => attempt.id === busy.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "The second Add did not honor the first account reservation.");
  assert.match(other.failure.message, /^Another Kuvo Code change is in progress/);
  assert.equal(fs.readlinkSync(marker), claimed);
  assert.equal(fs.existsSync(path.join(remote, "box", "bin", "kuvo-box")), false);
  const known = path.join(main.home, "ssh", "known_hosts");
  fs.writeFileSync(known, fs.readFileSync(known, "utf8").split("\n").filter(line => !line.startsWith("busy-alias ")).join("\n"));
  fs.unlinkSync(hold);
  const done = await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "done" && attempt;
  }, "SSH Add did not complete.", 35000);
  assert.equal(done.computer.name, "lab");
  assert.deepEqual(done.credentials, credentials);
  const engine = readJSON(engineCopy.imported);
  assert.equal(engine.joined, false, "the guarded engine imports before the installer writes membership");
  assert.equal(engine.keys, "selected fixture key");
  assert.deepEqual(engine.credentials, [
    { integrationID: "anthropic", label: "Selected", value: { type: "api", key: "selected fixture sign-in" }, activate: true },
    { integrationID: "github-copilot", label: "Copilot", value: { type: "oauth", access: "fixture", refresh: "stable fixture" }, activate: true },
  ]);
  assert.equal(engine.settings.model, "anthropic/claude");
  assert.deepEqual(Object.keys(engine.settings.mcp).sort(), ["github", "permitted"]);
  assert.equal(engine.settings.mcp.github.url, "https://api.githubcopilot.com/mcp/");
  assert.equal(engine.settings.mcp.github.disabled, true);
  assert.ok(engine.settings.mcp.github.headers.Authorization.endsWith(`/box/keys/${catalogKey}}`));
  assert.equal(fs.readFileSync(path.join(remote, "box", "box", "keys", catalogKey), "utf8"), "selected catalog key");
  assert.ok(engine.settings.mcp.permitted.environment.TOKEN.endsWith(`/box/keys/${selectedKey}}`));
  assert.equal(engine.settings.shell, undefined);
  assert.equal(engine.settings.worktree, undefined);
  assert.equal(fs.existsSync(path.join(remote, "box/box/keys/KUVO_KEY_UNSELECTED")), false);
  assert.equal(fs.existsSync(path.join(remote, "box/run/.ssh-fresh-add")), false);
  assert.equal(fs.readFileSync(oldCache, "utf8"), "cache from an earlier install");
  assert.equal(fs.readFileSync(path.join(main.home, "ssh", "known_hosts"), "utf8"), `lab ssh-ed25519 ${key}\n`);
  assert.deepEqual((await page()).json.hosts, ["second"]);
  const identity = fs.readFileSync(path.join(remote, "box", "machine-id"), "utf8");
  assert.match(identity, /^[a-f0-9]{32}$/);
  assert.deepEqual(readJSON(path.join(main.home, "machines.json")), [{ destination: "lab", identity }]);
  const host = readJSON(path.join(main.home, "phone-link.json")).machines.lab;
  assert.equal(host.key, done.computer.key);
  assert.equal(host.name, done.computer.name);
  const service = readJSON(path.join(remote, "box", "run", "serve.json"));
  const member = await request(service, "GET", "kuvo/setup");
  assert.equal(member.status, 200, member.text);
  assert.equal(member.json.setup, setup);
  assert.equal(member.json.you.key, done.computer.key);
  assert.equal(member.json.you.role, "member");

  const oldWork = process.platform === "darwin"
    ? path.join(remote, "Library", "Application Support", "run.kuvo.code", "Chats", "legacy", "notes.md")
    : undefined;
  if (oldWork) write(oldWork, "separate earlier work");
  fs.appendFileSync(path.join(main.home, "ssh", "known_hosts"), `second ssh-ed25519 ${key}\n`);
  const trace = path.join(fake, "trace");
  const failRelease = path.join(fake, "fail-release-second");
  write(trace, "");
  write(failRelease, "");
  const pending = await ask("POST", "kuvo/computers/add", { destination: "second" }, headers);
  assert.equal(pending.status, 202, pending.text);
  const failure = await until(async () => {
    const attempt = (await page()).json.tries.find(attempt => attempt.id === pending.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "The committed alias did not report the failed release.");
  assert.match(failure.failure.message, /finished the change but couldn’t clear the account/);
  assert.deepEqual(failure.computer, done.computer);
  const originalReservation = fs.readlinkSync(marker);
  assert.match(originalReservation, /^guarded-[0-9a-f]{48}$/);
  fs.unlinkSync(failRelease);

  const recoverAlias = async () => {
    const started = await ask("POST", "kuvo/computers/add", { destination: "second" }, headers);
    assert.equal(started.status, 202, started.text);
    const id = started.json.try.id;
    const waiting = await until(async () => {
      const attempt = (await page()).json.tries.find(attempt => attempt.id === id);
      if (attempt?.state === "failed") throw new Error(attempt.failure.message);
      return attempt?.state === "asking" && attempt;
    }, "The alias did not offer to recover its reservation.");
    assert.equal(waiting.question.id, "recoverAccount");
    return waiting;
  };
  const cancel = await recoverAlias();
  assert.equal((await ask("POST", `kuvo/computers/add/${cancel.id}/answer`, { answer: "cancel", at: cancel.question.at }, headers)).status, 200);
  await until(async () => !(await page()).json.tries.some(attempt => attempt.id === cancel.id), "The recovery attempt was not cancelled.");
  assert.equal(fs.readlinkSync(marker), originalReservation);
  const recover = await recoverAlias();
  assert.equal((await ask("POST", `kuvo/computers/add/${recover.id}/answer`, { answer: "recover", at: recover.question.at }, headers)).status, 200);
  fs.unlinkSync(trace);
  const aliasID = recover.id;
  const remembered = await until(async () => {
    const attempt = (await page()).json.tries.find(attempt => attempt.id === aliasID);
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "done" && attempt;
  }, "The known computer was not found at its other address.");
  assert.equal(remembered.known, true);
  assert.equal(remembered.title, "second is lab, already one of your computers");
  assert.deepEqual(remembered.computer, done.computer);
  assert.equal(fs.lstatSync(marker, { throwIfNoEntry: false }), undefined);
  assert.deepEqual(readJSON(path.join(main.home, "machines.json")), [{ destination: "lab", identity, otherAddresses: ["second"] }]);
  assert.equal((await ask("GET", "kuvo/setup")).json.computers.length, 2);
  if (oldWork) assert.equal(fs.readFileSync(oldWork, "utf8"), "separate earlier work");

  const copied = path.join(path.dirname(remote), "copied");
  write(path.join(fake, "home-copied"), copied);
  fs.appendFileSync(path.join(main.home, "ssh", "known_hosts"), `copied ssh-ed25519 ${key}\n`);
  write(path.join(copied, "box", "machine-id"), identity);
  write(path.join(copied, "box", "run", "setup-card.json"), JSON.stringify({ formatVersion: 1,
    self: { key: Buffer.alloc(32, 14).toString("base64url"), name: "Copy", kind: "machine" } }));
  const refusedCopy = await ask("POST", "kuvo/computers/add", { destination: "copied" }, headers);
  assert.equal(refusedCopy.status, 202, refusedCopy.text);
  await until(async () => (await page()).json.tries.some(attempt => attempt.id === refusedCopy.json.try.id && attempt.state === "failed"),
    "Copying a machine ID must not let another resident claim an alias.");
  assert.deepEqual(readJSON(path.join(main.home, "machines.json"))[0].otherAddresses, ["second"]);

  const wsl1 = path.join(path.dirname(remote), "wsl1");
  fs.mkdirSync(wsl1);
  write(path.join(fake, "home-windows-no-wsl2"), wsl1);
  write(path.join(fake, "windows-windows-no-wsl2"), "");
  const fromMember = (method, route, value) => request(service, method, route, value, headers);
  const forwardedPage = await fromMember("GET", "kuvo/computers/add");
  assert.equal(forwardedPage.status, 200, forwardedPage.text);
  assert.equal(forwardedPage.json.can, true);
  assert.equal(forwardedPage.json.tries.find(attempt => attempt.id === aliasID).known, true);
  const roster = readJSON(path.join(main.home, "run", "link.json")).shared.membershipAndAccess.value.boxes;
  const source = roster.find(each => each.key !== done.computer.key);
  const lab = roster.find(each => each.key === done.computer.key);
  let lost = false;
  const pause = relay.intercept(Buffer.from(source.address, "base64url"), Buffer.from(lab.address, "base64url"), () => {
    lost ||= readJSON(path.join(main.home, "run", "adding.json")).some(each => each.destination === "windows-no-wsl2");
    return true;
  });
  const forwarded = fromMember("POST", "kuvo/computers/add", { destination: "windows-no-wsl2" });
  let windows;
  try {
    await until(() => lost, "The main did not start the forwarded Add.");
  } finally {
    pause();
    windows = await forwarded;
  }
  assert.equal(windows.status, 202, windows.text);
  const windowsID = windows.json.try.id;
  const windowsTry = async () => (await page()).json.tries.find(attempt => attempt.id === windowsID);
  const windowsQuestion = await until(async () => {
    const attempt = await windowsTry();
    return attempt?.state === "asking" && attempt.question;
  }, "The Windows host key was never shown.");
  const trusted = await fromMember("POST", `kuvo/computers/add/${windowsID}/answer`, { answer: "trust", at: windowsQuestion.at });
  assert.equal(trusted.status, 200, trusted.text);
  const noWsl2 = await until(async () => {
    const attempt = await windowsTry();
    return attempt?.state === "failed" && attempt.failure;
  }, "The Windows account without WSL 2 should be refused.");
  assert.equal(noWsl2.message, "Install Ubuntu with WSL 2 on desktop-test, then open Ubuntu once and try again.");
  assert.deepEqual(fs.readdirSync(wsl1), []);
  const gone = await fromMember("DELETE", `kuvo/computers/add/${windowsID}`);
  assert.equal(gone.status, 200, gone.text);
  assert.ok(!(await page()).json.tries.some(attempt => attempt.id === windowsID));

  const heldAlias = "held-alias";
  const heldCheck = path.join(fake, `hold-${heldAlias}`);
  fs.appendFileSync(path.join(main.home, "ssh", "known_hosts"), `${heldAlias} ssh-ed25519 ${key}\n`);
  write(heldCheck, "card %s");
  const heldMail = relay.intercept(Buffer.from(source.address, "base64url"), Buffer.from(lab.address, "base64url"), () => true);
  t.after(() => {
    heldMail();
    fs.rmSync(heldCheck, { force: true });
  });
  const holding = await ask("POST", "kuvo/computers/add", { destination: heldAlias }, headers);
  assert.equal(holding.status, 202, holding.text);
  await until(() => fs.lstatSync(marker, { throwIfNoEntry: false })?.isSymbolicLink(),
    "The alias did not reach its account probe.");
  const unlinked = await ask("POST", `kuvo/computers/${done.computer.key}/unlink`, {}, headers);
  assert.equal(unlinked.status, 200, unlinked.text);
  fs.unlinkSync(heldCheck);
  const ended = await until(async () => {
    const attempt = (await page()).json.tries.find(attempt => attempt.id === holding.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "The alias was not refused after the computer was removed.");
  assert.match(ended.failure.message, /was removed from your computers/);
  assert.deepEqual(readJSON(path.join(main.home, "machines.json")), []);
  assert.equal((await ask("GET", "kuvo/setup")).json.computers.some(computer => computer.key === done.computer.key), false);
});

const key = byte => Buffer.concat([
  Buffer.from("0000000b7373682d6564323535313900000020", "hex"), Buffer.alloc(32, byte),
]).toString("base64");

function addRelease(installed, implementation) {
  if (implementation.typescript) {
    for (const file of ["bot-words.json", "integrations-mac.json", "integrations.json",
      "kuvo-agents.js", "kuvo-browser.js", "kuvo-launch", "kuvo-link.js", "kuvo-npx",
      "kuvo-setsid", "kuvo-start", "kuvo-wake", "mcp-catalog.json", "runtime/node", ".kuvo-boot"]) {
      write(path.join(installed, "bin", file), "");
    }
    write(path.join(installed, "config", "opencode", "plugins", "kuvo-code.js"), "");
    const release = path.join(installed, "bin", "box-version");
    write(release, "1.2.3\n");
    return release;
  }
  fs.mkdirSync(path.join(installed, "bin"), { recursive: true });
  const release = path.join(installed, "bin", process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box");
  fs.copyFileSync(implementation.file, release);
  return release;
}

function stageEngine(t, home, { holdReady = false, holdImport = false, verifiedProcess = false } = {}) {
  const version = readJSON(new URL("../../box/src/shared/engine-pin.json", import.meta.url)).version;
  const folder = fs.realpathSync(scratch());
  const fixture = path.join(folder, "ssh-engine.mjs");
  const started = path.join(folder, "ssh-engine-pids");
  const imported = path.join(folder, "imported.json");
  const program = path.join(home, "engine", version, "opencode");
  const gate = path.join(folder, "hold-engine-ready");
  const importing = path.join(folder, "importing");
  const importGate = path.join(folder, "hold-engine-import");
  if (holdReady) write(gate, "");
  if (holdImport) write(importGate, "");
  const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
  write(fixture, `import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
if (process.argv.includes("--version")) {
  console.log(${JSON.stringify(version)});
  process.exit(0);
}
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
const home = process.env.KUVO_HOME;
const credentials = [];
const server = http.createServer(async (req, res) => {
  if (req.headers.authorization !== "Basic " + Buffer.from("opencode:" + process.env.OPENCODE_PASSWORD).toString("base64")) return res.writeHead(401).end();
  if (req.url === "/api/session" && fs.existsSync(${JSON.stringify(gate)})) return res.writeHead(503).end();
  if (req.method === "POST" && req.url === "/api/credential") {
    const input = [];
    for await (const chunk of req) input.push(chunk);
    const credential = JSON.parse(Buffer.concat(input));
    fs.writeFileSync(${JSON.stringify(importing)}, "");
    while (fs.existsSync(${JSON.stringify(importGate)})) await new Promise(resolve => setTimeout(resolve, 25));
    const data = path.join(home, "data/opencode");
    fs.mkdirSync(data, { recursive: true });
    const db = new DatabaseSync(path.join(data, "opencode.db"));
    try {
      db.exec("CREATE TABLE IF NOT EXISTS credential (value TEXT); CREATE TABLE IF NOT EXISTS session (id TEXT); CREATE TABLE IF NOT EXISTS message (id TEXT); CREATE TABLE IF NOT EXISTS part (id TEXT)");
      db.prepare("INSERT INTO credential (value) VALUES (?)").run(JSON.stringify(credential));
    } finally { db.close(); }
    credentials.push(credential);
    fs.writeFileSync(${JSON.stringify(imported)}, JSON.stringify({ credentials,
      joined: fs.existsSync(path.join(home, "run/link.json")),
      keys: fs.readFileSync(path.join(home, "box/keys/KUVO_KEY_SSH_SAMPLE"), "utf8"),
      settings: JSON.parse(fs.readFileSync(path.join(home, "box/opencode.json"), "utf8")),
    }));
  }
  res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data: req.url === "/api/credential" ? credentials : [] }));
});
server.listen(port, "127.0.0.1", () => fs.appendFileSync(${JSON.stringify(started)}, process.pid + "\\n"));
`);
  if (verifiedProcess) {
    // Rollback starts in a new process and checks the spawned binary's identity.
    fs.mkdirSync(path.dirname(program), { recursive: true });
    execFileSync(typescriptRuntime(), ["build", "--compile", fixture, "--outfile", program], { cwd: folder, env: isolatedEnv(folder) });
  } else {
    write(program, `#!/bin/sh
if [ "$1" = --version ]; then printf '%s\\n' ${quote(version)}; exit 0; fi
exec ${quote(process.execPath)} ${quote(fixture)} "$@"
`, { mode: 0o700 });
  }
  t.after(() => {
    const pids = fs.existsSync(started) ? fs.readFileSync(started, "utf8").trim().split("\n").map(Number) : [];
    for (const pid of pids) {
      if (!Number.isSafeInteger(pid) || pid < 2) continue;
      let command;
      try { command = execFileSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8" }); }
      catch { continue; }
      if (command.includes(verifiedProcess ? program : fixture)) {
        try { process.kill(pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; }
      }
    }
    fs.rmSync(folder, { recursive: true, force: true });
  });
  return {
    started, importing, imported,
    release: () => fs.rmSync(gate, { force: true }),
    finishImport: () => fs.rmSync(importGate, { force: true }),
  };
}

async function addBench(t, options) {
  const { computer, leases, relay } = await bench(t, options);
  const implementation = boxBinary();
  let fake, remote, account, releaseMarker;
  const main = await computer("Studio", implementation, folder => {
    fake = path.join(folder, "ssh");
    remote = path.join(folder, "remote");
    account = path.join(folder, "account");
    fs.mkdirSync(remote);
    releaseMarker = addRelease(path.join(folder, "box"), implementation);
    const program = path.join(fake, "ssh");
    write(program, addSshStandIn, { mode: 0o700 });
    write(path.join(fake, "hostkey"), "ssh-ed25519 " + key(8));
    return { KUVO_TEST_SSH: program, FAKE_SSH: fake, FAKE_SSH_HOME: remote };
  });
  const ask = main.connect(await main.start().ready());
  const setup = (await ask("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "Studio did not claim a lease.");
  const headers = { "x-kuvo-from": "mac" };
  const page = async () => {
    const answer = await ask("GET", "kuvo/computers/add");
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  return { computer, main, ask, account, remote, fake, headers, page, relay, releaseMarker };
}

async function selectedSshCredentials({ main, ask, headers }) {
  const selectedKey = "KUVO_KEY_SSH_SAMPLE";
  write(path.join(main.home, "box", "keys", selectedKey), "selected fixture key", { mode: 0o600 });
  main.engine.integrations.push({ id: "anthropic" });
  main.engine.credentials.push({ id: "fixture-key", integrationID: "anthropic", label: "Fixture sign-in", active: true,
    value: { type: "api", key: "fixture sign-in only" } });
  const checklist = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(checklist.status, 200, checklist.text);
  const credentials = checklist.json.items.filter(item => [selectedKey, "Fixture sign-in"].includes(item.label)).map(item => item.id);
  assert.equal(credentials.length, 2);
  return { selectedKey, credentials };
}

test("SSH Add stops sending sign-ins when its paired device is revoked during engine startup", {
  skip: process.platform === "win32" || boxBinary().typescript || !fs.existsSync(wireFixture), timeout: 65_000,
}, async t => {
  const fixture = await addBench(t);
  const { main, ask, remote, page, relay } = fixture;
  const target = path.join(remote, ".kuvo-code");
  const engine = stageEngine(t, target, { holdReady: true });
  const { selectedKey, credentials } = await selectedSshCredentials(fixture);

  const wire = await loadWire(main.home, main.env);
  const secret = Buffer.alloc(32, 82);
  const deviceKey = wire.publicOf(secret).toString("base64url");
  const paired = await ask("POST", "kuvo/devices/pair", { key: deviceKey, kind: "desktop", name: "Add caller" }, { "x-kuvo-from": "app" });
  assert.equal(paired.status, 200, paired.text);
  const link = readJSON(path.join(main.home, "run", "link.json"));
  const address = relayAddress(Buffer.from(link.secret, "base64url"));
  const computer = Buffer.from((await ask("GET", "kuvo/setup")).json.you.key, "base64url");
  const device = relay.device();
  t.after(() => device.leave());
  const session = 412;
  const send = (kind, body) => device.send(address, wire.frame(kind, session, body));
  const receive = async kind => {
    const message = wire.readRelay(await device.next());
    assert.equal(message.kind, "deliver");
    const frame = wire.readFrame(message.payload);
    assert.equal(frame.session, session);
    assert.equal(frame.kind, kind);
    return frame.body;
  };
  const initiator = new wire.IKInitiator(secret, computer, wire.PROLOGUE);
  send(wire.Frame.handshake1, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, app: "1.0.0", client: "desktop", pairing: paired.json.pairing }))));
  const opened = initiator.readMessage2(await receive(wire.Frame.handshake2));
  let requestId = 0;
  const fromDevice = async (route, body) => {
    const id = ++requestId;
    const header = { t: "req", id, m: "POST", p: route, h: { "content-type": "application/json" } };
    for (const part of wire.pieces(header, Buffer.from(JSON.stringify(body)), true)) {
      send(wire.Frame.transport, opened.transport.seal(part));
    }
    const head = wire.decodeMessage(opened.transport.open(await receive(wire.Frame.transport)));
    assert.equal(head.head.id, id);
    const chunks = [head.body];
    let last = head;
    while (last.head.end !== true) {
      last = wire.decodeMessage(opened.transport.open(await receive(wire.Frame.transport)));
      assert.equal(last.head.id, id);
      chunks.push(last.body);
    }
    return { status: head.head.s, json: JSON.parse(Buffer.concat(chunks)) };
  };
  const begun = await fromDevice("kuvo/computers/add", { destination: "lab", credentials });
  assert.equal(begun.status, 202, JSON.stringify(begun.json));
  const id = begun.json.try.id;
  const current = async () => (await page()).tries.find(attempt => attempt.id === id);
  const question = await until(async () => (await current())?.question, "SSH did not show the host key.");
  assert.equal(question.id, "hostKey");
  const trusted = await fromDevice(`kuvo/computers/add/${id}/answer`, { answer: "trust", at: question.at });
  assert.equal(trusted.status, 200, JSON.stringify(trusted.json));
  try {
    await until(() => fs.existsSync(engine.started), "The staged engine did not start.", 30_000);
    assert.equal(fs.readFileSync(path.join(target, "box", "keys", selectedKey), "utf8"), "selected fixture key");
    assert.equal(fs.existsSync(engine.imported), false);
    const revoked = await ask("POST", "kuvo/phones/remove", { phones: [{ key: deviceKey }] }, { "x-kuvo-from": "app" });
    assert.equal(revoked.status, 200, revoked.text);
  } finally {
    engine.release();
  }
  const failed = await until(async () => {
    const attempt = await current();
    return attempt?.state === "failed" && attempt;
  }, "The revoked device's Add did not stop.", 25_000);
  assert.match(failed.failure.message, /device that started this add is no longer paired/);
  assert.match(failed.failure.message, /keys and sign-ins copied there were deleted/);
  assert.equal(fs.existsSync(engine.imported), false);
  for (const relative of ["box/keys", "run/link.json", "run/.ssh-fresh-add", "run/ssh-engine.json"]) {
    assert.equal(fs.existsSync(path.join(target, relative)), false, relative);
  }
});

for (const disconnect of ["SIGTERM", "255"]) {
  test(`SSH Add recovers only after a credential writer disconnected by ${disconnect} settles`, {
    skip: process.platform === "win32" || boxBinary().typescript, timeout: 125_000,
  }, async t => {
    const fixture = await addBench(t);
    const { main, ask, remote, fake, headers, page } = fixture;
    const target = path.join(remote, ".kuvo-code");
    const engine = stageEngine(t, target, { holdImport: true });
    const { credentials } = await selectedSshCredentials(fixture);
    const trace = path.join(fake, "trace");
    const cut = path.join(fake, "drop-rust-seed-lab");
    const writerEnded = path.join(fake, "writer-ended");
    const reservation = path.join(remote, ".kuvo-code-adding");
    const fresh = path.join(target, "run", ".ssh-fresh-add");
    write(trace, "");
    write(cut, disconnect);
    write(path.join(main.home, "ssh", "known_hosts"), ["lab", "alias"].map(host => `${host} ssh-ed25519 ${key(8)}\n`).join(""));
    write(path.join(fake, "drop-rust-seed"), `#!${process.execPath}
const fs = require("node:fs");
const { spawn } = require("node:child_process");
const remote = spawn("/bin/sh", ["-c", 'sh "$1"; printf done > "$2"', "credential-writer", process.argv[2], ${JSON.stringify(writerEnded)}],
  { detached: true, stdio: ["pipe", "pipe", "ignore"] });
process.stdin.pipe(remote.stdin);
remote.stdout.pipe(process.stdout);
const timer = setInterval(() => {
  if (!fs.existsSync(${JSON.stringify(engine.importing)})) return;
  if (process.argv[3] === "255") process.exit(255);
  process.kill(process.pid, "SIGTERM");
}, 25);
remote.once("exit", () => { clearInterval(timer); process.exit(1); });
remote.stdin.on("error", () => {});
`, { mode: 0o700 });
    t.after(() => engine.finishImport());

    const attempt = async (destination, chosen = credentials) => {
      const reply = await ask("POST", "kuvo/computers/add", { destination, credentials: chosen }, headers);
      assert.equal(reply.status, 202, reply.text);
      return reply.json.try.id;
    };
    const waitFor = (id, predicate, why) => until(async () => {
      const entry = (await page()).tries.find(tried => tried.id === id);
      return entry && predicate(entry) && entry;
    }, why, 35_000);
    const failure = entry => entry.state === "failed";
    const first = await waitFor(await attempt("lab"), failure, "The disconnected writer did not stop Add.");
    assert.match(first.failure.message, /lost contact.*during a change/i);
    assert.deepEqual(fs.readFileSync(trace, "utf8").trim().split("\n").filter(line => /^lab (seed|join|uninstall|release)$/.test(line)), ["lab seed"]);
    assert.match(fs.readFileSync(fresh, "utf8"), / copying\n$/);
    const original = fs.readlinkSync(reservation);
    const busy = await waitFor(await attempt("alias"), failure, "A second address did not report the remote writer.");
    assert.match(busy.failure.message, /change is in progress/);
    assert.equal(fs.readlinkSync(reservation), original);
    assert.equal(fs.existsSync(writerEnded), false);

    engine.finishImport();
    await until(() => fs.existsSync(writerEnded), "The original remote writer did not finish.");
    fs.unlinkSync(cut);
    const different = disconnect === "255";
    const recovered = await waitFor(await attempt("lab", different ? [] : credentials), entry => entry.question || failure(entry), "Add did not offer recovery.");
    assert.equal(recovered.question?.id, "recoverAccount", recovered.failure?.message);
    assert.equal(fs.readlinkSync(reservation), original);
    const continued = await ask("POST", `kuvo/computers/add/${recovered.id}/answer`, { answer: "recover", at: recovered.question.at }, headers);
    assert.equal(continued.status, 200, continued.text);
    let final = recovered.id;
    if (different) {
      const refused = await waitFor(recovered.id, failure, "Recovery did not refuse the changed selection.");
      assert.match(refused.failure.message, /different choice of sign-ins and keys/);
      assert.deepEqual(refused.credentials, first.credentials);
      assert.equal(fs.lstatSync(reservation, { throwIfNoEntry: false }), undefined);
      final = await attempt("lab");
    }
    const finished = await waitFor(final, entry => entry.state === "done" || failure(entry), "The original selection did not finish enrollment.");
    assert.equal(finished.state, "done", finished.failure?.message);
    for (const file of [reservation, fresh]) assert.equal(fs.lstatSync(file, { throwIfNoEntry: false }), undefined);
    assert.equal(readJSON(engine.imported).joined, false);
    const db = new DatabaseSync(path.join(target, "data", "opencode", "opencode.db"), { readOnly: true });
    try { assert.equal(db.prepare("SELECT COUNT(*) AS count FROM credential").get().count, 1); }
    finally { db.close(); }
    const service = readJSON(path.join(target, "run", "serve.json"));
    t.after(() => { try { process.kill(service.pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; } });
    assert.equal((await request(service, "GET", "kuvo/setup")).json.you.key, finished.computer.key);
  });
}

for (const shouldStop of [true, false]) test(shouldStop
  ? "stopping SSH Add waits for the credential writer and rolls back the staged engine and keys"
  : "SSH Add rolls back the staged engine and credentials if enrollment fails", {
  skip: process.platform === "win32" || boxBinary().typescript, timeout: 65_000,
}, async t => {
  const fixture = await addBench(t);
  const { main, ask, remote, fake, headers, page } = fixture;
  const target = path.join(remote, ".kuvo-code");
  const engine = stageEngine(t, target, { holdImport: true, verifiedProcess: true });
  const { selectedKey, credentials } = await selectedSshCredentials(fixture);
  write(path.join(main.home, "ssh", "known_hosts"), `lab ssh-ed25519 ${key(8)}\n`);
  const added = await ask("POST", "kuvo/computers/add", { destination: "lab", credentials }, headers);
  assert.equal(added.status, 202, added.text);
  const id = added.json.try.id;
  const current = async () => (await page()).tries.find(attempt => attempt.id === id);
  const reservation = path.join(remote, ".kuvo-code-adding");
  let pid;
  try {
    await until(() => fs.existsSync(engine.importing), "The credential writer did not reach the staged engine.", 30_000);
    pid = Number(fs.readFileSync(engine.started, "utf8").trim());
    assert.ok(Number.isSafeInteger(pid) && pid > 1);
    assert.equal(fs.readFileSync(path.join(target, "box", "keys", selectedKey), "utf8"), "selected fixture key");
    await until(async () => (await current()).steps.some(step => step.id === "seeding" && step.state === "now"),
      "The Add page did not reach the credential copy.");
    if (shouldStop) {
      const stopped = await ask("DELETE", `kuvo/computers/add/${id}`, undefined, headers);
      assert.equal(stopped.status, 200, stopped.text);
    } else {
      write(path.join(fake, "fail"), "ssh-enroll");
    }
    assert.equal(fs.lstatSync(reservation).isSymbolicLink(), true);
    assert.doesNotThrow(() => process.kill(pid, 0));
  } finally {
    engine.finishImport();
  }
  await until(() => !fs.lstatSync(reservation, { throwIfNoEntry: false }), "Stop did not release the account after rollback.", 25_000);
  if (shouldStop) {
    await until(async () => (await current()) === undefined, "The stopped Add did not leave the page.");
  } else {
    const failed = await until(async () => {
      const attempt = await current();
      return attempt?.state === "failed" && attempt;
    }, "The failed enrollment did not report its rollback.");
    assert.match(failed.failure.message, /keys and sign-ins copied there were deleted/);
  }
  for (const relative of ["box/keys", "run/link.json", "run/.ssh-fresh-add", "run/ssh-engine.json"]) {
    assert.equal(fs.existsSync(path.join(target, relative)), false, relative);
  }
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
  const db = new DatabaseSync(path.join(target, "data", "opencode", "opencode.db"), { readOnly: true });
  try { assert.equal(db.prepare("SELECT COUNT(*) AS count FROM credential").get().count, 0); }
  finally { db.close(); }
});

test("a failed SSH Add leaves a previously uninstalled computer's credentials and settings in place", {
  skip: process.platform === "win32" || boxBinary().typescript, timeout: 30_000,
}, async t => {
  const fixture = await addBench(t);
  const { main, ask, remote, fake, headers, page } = fixture;
  const { selectedKey, credentials } = await selectedSshCredentials(fixture);
  const target = path.join(remote, ".kuvo-code");
  const retained = {
    "machine-id": "0123456789abcdef0123456789abcdef",
    "folder-version.json": '{"version":23}',
    "run/off": "",
    "box/opencode.json": '{"model":"its/own"}',
    "box/keys/KUVO_KEY_ITS_OWN": "its own key",
    "agents/codex/auth.json": "its own sign-in",
  };
  for (const [file, value] of Object.entries(retained)) write(path.join(target, file), value, { mode: 0o600 });
  write(path.join(main.home, "ssh", "known_hosts"), `lab ssh-ed25519 ${key(8)}\n`);
  write(path.join(fake, "fail"), "ssh-enroll");
  const begun = await ask("POST", "kuvo/computers/add", { destination: "lab", credentials }, headers);
  assert.equal(begun.status, 202, begun.text);
  const failed = await until(async () => {
    const entry = (await page()).tries.find(each => each.id === begun.json.try.id);
    return entry?.state === "failed" && entry;
  }, "The target's enrollment failure did not reach the source.", 20_000);
  assert.equal(failed.steps.find(step => step.id === "seeding").detail, "It keeps its own");
  assert.doesNotMatch(failed.failure.message, /sign-ins copied there were deleted/);
  for (const [file, value] of Object.entries(retained)) assert.equal(fs.readFileSync(path.join(target, file), "utf8"), value);
  for (const file of [`box/keys/${selectedKey}`, "run/.ssh-fresh-add", "run/engine.json"]) {
    assert.equal(fs.existsSync(path.join(target, file)), false, file);
  }
});

test("SSH Add recovers a selected integration after the target saved its settings", {
  skip: process.platform === "win32" || boxBinary().typescript, timeout: 45_000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const target = path.join(remote, ".kuvo-code");
  const catalogKey = "KUVO_KEY_GITHUB_TOKEN";
  const trusted = "https://api.githubcopilot.com/mcp/";
  const catalog = { integrations: [{ id: "github", variables: { TOKEN: catalogKey }, settings: {
    type: "remote", url: trusted, oauth: false, codemode: false,
    headers: { Authorization: `Bearer {file:~/.kuvo-code/box/keys/${catalogKey}}` },
  } }] };
  write(path.join(main.home, "bin", "integrations.json"), JSON.stringify(catalog));
  write(path.join(main.home, "box", "opencode.json"), JSON.stringify({ model: "source/model", mcp: {
    github: { disabled: true, url: "https://not-the-catalog.invalid" },
  } }));
  write(path.join(main.home, "box", "keys", catalogKey), "new fixture key", { mode: 0o600 });
  write(path.join(main.home, "ssh", "known_hosts"), `lab ssh-ed25519 ${key(8)}\n`);
  const checklist = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(checklist.status, 200, checklist.text);
  const choice = checklist.json.items.find(item => item.label === catalogKey);
  assert.ok(choice);
  const { setup, you } = (await ask("GET", "kuvo/setup")).json;
  const digest = data => createHash("sha256").update(JSON.stringify(data)).digest("hex");
  write(path.join(target, "folder-version.json"), '{"version":23}');
  write(path.join(target, "run", ".ssh-fresh-add"), `v1 ${digest([setup, you.key])} ${digest([choice.id])} copying\n`);
  write(path.join(target, "bin", "integrations.json"), JSON.stringify(catalog));
  const retained = { type: "local", command: ["target-owned"] };
  write(path.join(target, "box", "opencode.json"), JSON.stringify({ model: "target/model", mcp: { servers: { retained } } }));
  const keyFile = path.join(target, "box", "keys", catalogKey);
  write(keyFile, "key from the interrupted copy", { mode: 0o600 });
  const engine = stageEngine(t, target);
  const started = await ask("POST", "kuvo/computers/add", { destination: "lab", credentials: [choice.id] }, headers);
  assert.equal(started.status, 202, started.text);
  const result = await until(async () => {
    const entry = (await page()).tries.find(each => each.id === started.json.try.id);
    return ["done", "failed"].includes(entry?.state) && entry;
  }, "The interrupted key-only Add did not settle.", 35_000);
  assert.equal(result.state, "done", result.failure?.message);
  const service = readJSON(path.join(target, "run", "serve.json"));
  t.after(() => { try { process.kill(service.pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; } });
  const settings = readJSON(path.join(target, "box", "opencode.json"));
  assert.equal(settings.model, "target/model");
  assert.deepEqual(settings.mcp.servers.retained, retained);
  assert.equal(settings.mcp.servers.github?.url, trusted);
  assert.equal(settings.mcp.servers.github?.disabled, true);
  assert.equal(fs.readFileSync(keyFile, "utf8"), "key from the interrupted copy");
  assert.equal(fs.existsSync(engine.imported), false, "A key-only recovery sent no sign-ins.");
});

test("an SSH-enrolled member follows the next primary after reconnecting", {
  skip: process.platform === "win32" || boxBinary().typescript, timeout: 120_000,
}, async t => {
  const { computer, main, ask, remote, relay, headers, page } = await addBench(t, { handover: true });
  const delegate = await computer("Delegate");
  const invitation = await ask("POST", "kuvo/computers/add/code", {}, headers);
  assert.equal(invitation.status, 201, invitation.text);
  const enrolled = await delegate.join(invitation.json.code);
  assert.equal(enrolled.code, 0, enrolled.log);
  const other = delegate.connect(await delegate.start().ready());
  const readSetup = async from => (await from("GET", "kuvo/setup")).json;
  const first = await readSetup(ask);
  const firstKey = first.you.key;
  const secondKey = (await readSetup(other)).you.key;
  const handover = async (from, next) => {
    const old = await readSetup(from);
    const result = await from("POST", "kuvo/setup/handover", { primary: next, epoch: old.epoch }, headers);
    assert.equal(result.status, 202, result.text);
    for (const each of [ask, other]) {
      await until(async () => {
        const state = await readSetup(each);
        return state.epoch === old.epoch + 1 && state.primary.key === next;
      }, "The existing residents did not accept the new primary.", 20_000);
    }
    return old.epoch + 1;
  };
  await handover(ask, secondKey);
  const before = await handover(other, firstKey);
  const target = path.join(remote, ".kuvo-code");
  stageEngine(t, target);
  write(path.join(main.home, "ssh", "known_hosts"), `lab ssh-ed25519 ${key(8)}\n`);
  const added = await ask("POST", "kuvo/computers/add", { destination: "lab" }, headers);
  assert.equal(added.status, 202, added.text);
  const done = await until(async () => {
    const entry = (await page()).tries.find(each => each.id === added.json.try.id);
    if (entry?.state === "failed") assert.fail(entry.failure.message);
    return entry?.state === "done" && entry;
  }, "The SSH-enrolled resident did not start.", 35_000);
  const info = readJSON(path.join(target, "run", "serve.json"));
  t.after(() => { try { process.kill(info.pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; } });
  const fromTarget = (...args) => request(info, ...args);
  assert.equal((await readSetup(fromTarget)).epoch, before);
  assert.equal((await readSetup(fromTarget)).you.key, done.computer.key);
  const local = () => readJSON(path.join(target, "run", "link.json"));
  const reconnect = relay.disconnect(relayAddress(Buffer.from(local().secret, "base64url")));
  let after;
  try {
    after = await handover(ask, secondKey);
    assert.equal((await readSetup(fromTarget)).epoch, before);
  } finally {
    reconnect();
  }
  await until(async () => {
    const state = await readSetup(fromTarget);
    return state.epoch === after && state.primary.key === secondKey && state.you.key === done.computer.key && state.you.role === "member";
  }, "The SSH-enrolled resident did not accept the next epoch on reconnect.", 30_000);
  assert.ok(local().leadership[after]);
});

test("SSH Add keeps its selected checklist and refuses a missing key before writing", {
  skip: process.platform === "win32", timeout: 20000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const selected = path.join(main.home, "box", "keys", "KUVO_KEY_SAMPLE");
  write(selected, "one selected key", { mode: 0o600 });
  const list = await ask("GET", "kuvo/computers/add/credentials", undefined, headers);
  assert.equal(list.status, 200, list.text);
  const choice = list.json.items.find(item => item.label === "KUVO_KEY_SAMPLE");
  assert.ok(choice);
  const begin = credentials => ask("POST", "kuvo/computers/add", { destination: "lab", credentials }, headers);
  const notified = await begin([choice.id]);
  assert.equal(notified.status, 202, notified.text);
  const id = notified.json.try.id;
  const current = async () => (await page()).tries.find(attempt => attempt.id === id);
  const waiting = await until(async () => {
    const attempt = await current();
    return attempt?.question?.id === "hostKey" && attempt;
  }, "SSH did not ask for the new computer’s host key.");
  assert.equal((await begin([choice.id, choice.id])).json.try.id, id);
  const competing = await begin([]);
  assert.equal(competing.status, 409, competing.text);
  assert.equal(competing.json.message, "An add with a different choice of sign-ins and keys is already in progress for this computer.");
  fs.unlinkSync(selected);
  const approved = await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: waiting.question.at }, headers);
  assert.equal(approved.status, 200, approved.text);
  const failed = await until(async () => {
    const attempt = await current();
    return attempt?.state === "failed" && attempt;
  }, "SSH Add did not refuse the removed key.");
  assert.equal(failed.failure.message, "Some selected sign-ins or keys are no longer available. Open the checklist again and retry.");
  for (const root of ["box", ".kuvo-code"]) {
    for (const relative of ["bin", "box/keys", "run/link.json", "run/.ssh-fresh-add"]) {
      assert.equal(fs.existsSync(path.join(remote, root, relative)), false, relative);
    }
  }
});

test("SSH Add asks again before replacing a changed key that Kuvo trusted", {
  skip: process.platform === "win32", timeout: 20000,
}, async t => {
  const { main, ask, account, remote, fake, headers, page } = await addBench(t);
  const file = path.join(main.home, "ssh", "known_hosts");
  const localKeys = path.join(account, ".ssh", "known_hosts");
  const previous = `lab ssh-ed25519 ${key(7)}\nlab.lan ssh-ed25519 ${key(7)}\n`;
  const userOwned = `other ssh-ed25519 ${key(9)}\n`;
  write(file, previous, { mode: 0o600 });
  write(localKeys, userOwned, { mode: 0o600 });
  const begin = async () => {
    const reply = await ask("POST", "kuvo/computers/add", { destination: "lab" }, headers);
    assert.equal(reply.status, 202, reply.text);
    const id = reply.json.try.id;
    const current = async () => (await page()).tries.find(each => each.id === id);
    const question = await until(async () => {
      const attempt = await current();
      if (attempt?.state === "failed") throw new Error(attempt.failure.message);
      return attempt?.question;
    }, "The changed saved key was not shown.");
    assert.equal(question.id, "hostKeyChanged");
    assert.equal(question.title, "lab’s host key changed");
    assert.equal(question.facts[1].label, "Fingerprint (ED25519)");
    assert.match(question.facts[1].value, /^SHA256:/);
    assert.deepEqual(question.answers.map(({ id, label, kind }) => [id, label, kind]), [
      ["cancel", "Cancel", "plain"], ["trust", "Trust New Key", "danger"],
    ]);
    const answer = choice => ask("POST", `kuvo/computers/add/${id}/answer`, { answer: choice, at: question.at }, headers);
    return { current, answer };
  };
  const cancelled = await begin();
  assert.equal((await cancelled.answer("cancel")).status, 200);
  await until(async () => !await cancelled.current(), "The declined trust question was not closed.");
  assert.equal(fs.readFileSync(file, "utf8"), previous);
  const stale = await begin();
  const newer = previous.replace(`lab ssh-ed25519 ${key(7)}`, `lab ssh-ed25519 ${key(6)}`);
  write(file, newer, { mode: 0o600 });
  write(path.join(fake, "refuse"), "");
  assert.equal((await stale.answer("trust")).status, 200);
  await until(async () => (await stale.current())?.state === "failed", "Approval of a stale key did not fail.");
  assert.equal(fs.readFileSync(file, "utf8"), newer, "An old approval must not replace a different saved key.");
  write(file, previous, { mode: 0o600 });
  fs.unlinkSync(path.join(fake, "refuse"));
  const approved = await begin();
  write(path.join(fake, "refuse"), "");
  assert.equal((await approved.answer("trust")).status, 200);
  await until(async () => (await approved.current())?.state === "failed", "The next command did not try the explicitly trusted key.");
  assert.deepEqual(fs.readFileSync(file, "utf8").trim().split("\n").sort(), [
    `lab ssh-ed25519 ${key(8)}`, `lab.lan ssh-ed25519 ${key(7)}`,
  ]);
  assert.equal(fs.readFileSync(localKeys, "utf8"), userOwned);
  assert.deepEqual(fs.readdirSync(remote), []);
});

test("SSH Add explains connection failures and leaves the account and host trust alone", {
  skip: process.platform === "win32",
  timeout: 45000,
}, async t => {
  const { main, ask, account, remote, fake, headers, page } = await addBench(t);
  assert.equal((await page()).can, true);
  const start = async destination => {
    const began = await ask("POST", "kuvo/computers/add", { destination }, headers);
    assert.equal(began.status, 202, began.text);
    return began.json.try.id;
  };
  const when = (id, state) => until(async () => {
    const attempt = (await page()).tries.find(item => item.id === id);
    return attempt?.state === state && attempt;
  }, `Try ${id} did not reach ${state}`);

  const nowhere = await start("nowhere");
  const unreachable = await when(nowhere, "failed");
  assert.deepEqual(unreachable.steps.map(item => item.state), ["failed", ...Array(5).fill("waiting")]);
  assert.deepEqual(unreachable.failure, {
    title: "Couldn’t add nowhere",
    message: "Couldn’t reach nowhere. Make sure it’s on and connected, then Try Again.",
    again: true,
  });

  const users = path.join(account, ".ssh", "known_hosts");
  const known = path.join(main.home, "ssh", "known_hosts");
  const saved = `changed ssh-ed25519 ${key(9)}\n`;
  write(users, saved);
  const changed = await when(await start("changed"), "failed");
  assert.equal(changed.failure.title, "changed’s host key changed");
  assert.match(changed.failure.message, /remove the old key in a terminal on Studio with ssh-keygen -R 'changed', then try again/);
  assert.equal(fs.readFileSync(users, "utf8"), saved);
  assert.equal(fs.existsSync(known), false);

  write(path.join(fake, "refuse"), "");
  const denied = await start("denied");
  const asked = await when(denied, "asking");
  assert.equal(asked.question.id, "hostKey");
  assert.equal(fs.existsSync(known), false);
  const trusted = await ask("POST", `kuvo/computers/add/${denied}/answer`, { answer: "trust", at: asked.question.at }, headers);
  assert.equal(trusted.status, 200, trusted.text);
  const refused = await when(denied, "failed");
  assert.equal(refused.failure.title, "denied refused the connection");
  assert.match(refused.failure.message, /didn’t accept Studio’s SSH keys/);
  assert.equal(fs.readFileSync(known, "utf8"), `denied ssh-ed25519 ${key(8)}\n`);
  assert.deepEqual(fs.readdirSync(remote), []);
  assert.equal((await ask("DELETE", `kuvo/computers/add/${nowhere}`, undefined, headers)).status, 200);
  assert.equal((await page()).tries.some(item => item.id === nowhere), false);
});

test("uninstall from a saved SSH address commits a signed wipe and waits for confirmation", {
  skip: process.platform === "win32", timeout: 60000,
}, async t => {
  const { computer, relay, leases } = await bench(t);
  const implementation = boxBinary();
  const studio = await computer("Studio", implementation, folder => {
    addRelease(path.join(folder, "box"), implementation);
    return {};
  });
  const lab = await computer("Lab", implementation, undefined, { canonical: true });
  const { member, spare, save } = seedPeerSetup(studio.home, relay.url, 1, 3);
  seedPeerSetup(lab.home, relay.url, 2, 3);
  const work = path.join(lab.home, "agents", "sessions", "ses_retired", "messages.json");
  write(work, "saved work on the departing member");
  const labAddress = { destination: "test-lab", identity: "4".repeat(32) };
  const spareAddress = { destination: "test-spare", identity: "5".repeat(32) };
  save("machines.json", [labAddress, spareAddress]);
  const primary = studio.connect(await studio.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim its relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const route = `kuvo/computers/${member.key}/uninstall`;
  const missing = await primary("POST", route, {}, headers);
  assert.equal(missing.status, 404, missing.text);
  save("phone-link.json", { machines: {
    "test-lab": { key: member.key, name: member.name, address: member.address },
    "test-spare": { key: spare.key, name: spare.name, address: spare.address },
  } });
  const result = await primary("POST", route, {}, headers);
  assert.equal(result.status, 202, result.text);
  const attempt = result.json.try;
  assert.equal(attempt.kind, "uninstall");
  assert.equal(attempt.destination, "test-lab");
  const state = () => readJSON(path.join(studio.home, "run", "link.json")).shared.membershipAndAccess.value;
  await until(() => state().removals?.[member.key]?.wipe, "The signed wipe was not saved.");
  assert.ok(state().left.includes(member.key));
  assert.equal(fs.readFileSync(work, "utf8"), "saved work on the departing member");
  await until(async () => (await primary("GET", "kuvo/computers")).json.wipePending?.[0]?.key === member.key,
    "The computers list did not show the committed pending wipe.");
  const addresses = () => readJSON(path.join(studio.home, "machines.json"));
  await until(() => addresses().length === 1,
    "The saved SSH address was not cleared after revocation.");
  assert.deepEqual(addresses(), [spareAddress]);
  const tried = async () => (await primary("GET", "kuvo/computers/add")).json.tries.find(each => each.id === attempt.id);
  await until(async () => (await tried())?.save?.proof?.s, "The uninstall did not expose the committed receipt.");
  assert.equal((await primary("DELETE", `kuvo/computers/add/${attempt.id}`, undefined, headers)).status, 200);
  assert.equal((await tried()).state, "working");
  fs.rmSync(path.join(lab.home, "run", "engine.json"), { force: true });
  lab.start();
  const done = await until(async () => {
    const found = await tried();
    if (found?.state === "failed") assert.fail(found.failure.message);
    return found?.state === "done" && found;
  }, "The uninstall did not record a completed wipe.", 17000);
  assert.equal(done.title, "Lab no longer has Kuvo Code running");
  assert.equal(done.save.operation, state().removals[member.key].operation);
  assert.equal(state().removals[member.key].delivered, true);
  assert.equal(fs.existsSync(work), false);

  const missingDisk = await primary("POST", `kuvo/computers/${spare.key}/uninstall`, {}, headers);
  assert.equal(missingDisk.status, 202, missingDisk.text);
  const lostAttempt = missingDisk.json.try.id;
  const lostTry = async () => (await primary("GET", "kuvo/computers/add")).json.tries.find(each => each.id === lostAttempt);
  const operation = await until(async () => (await lostTry())?.save?.operation,
    "The offline computer was not revoked.");
  const lost = await primary("POST", `kuvo/computers/${spare.key}/unlink-and-wipe`, { operation, resolve: "lost" }, headers);
  assert.equal(lost.status, 200, lost.text);
  const closed = await until(async () => {
    const found = await lostTry();
    return found?.state === "failed" && found;
  }, "The unconfirmed erase was reported as complete.");
  assert.equal(closed.failure.again, false);
  assert.equal(closed.failure.message, "The removal was closed without confirmation that the data was erased.");
  assert.equal(closed.save.operation, operation);
  assert.ok(state().left.includes(spare.key));
  assert.deepEqual(addresses(), []);
});

test("unlink keeps its signed revocation when forgetting the SSH address fails", {
  skip: process.getuid?.() === 0, timeout: 30000,
}, async t => {
  const { computer, relay, leases } = await bench(t);
  const studio = await computer("Studio");
  const { member, save } = seedPeerSetup(studio.home, relay.url);
  const addresses = [{ destination: "lab", identity: "4".repeat(32) }, { destination: "other" }];
  save("machines.json", addresses);
  save("phone-link.json", { machines: { lab: { key: member.key, name: member.name, address: member.address } } });
  const running = studio.start();
  const primary = studio.connect(await running.ready());
  const initial = (await primary("GET", "kuvo/setup")).json;
  await until(() => leases.leaseFor(initial.setup), "The main computer did not claim its relay lease.");
  const headers = { "x-kuvo-from": "mac" };
  const route = `kuvo/computers/${member.key}/unlink`;
  const operation = Buffer.alloc(16, 67).toString("base64url");
  const link = () => readJSON(path.join(studio.home, "run", "link.json"));
  let restore;
  if (process.platform === "win32") {
    restore = await holdWindowsFile(t, path.join(studio.home, "machines.json"), path.join(studio.home, "run", "address-lock-ready"), studio.env);
  } else {
    const mode = fs.statSync(studio.home).mode & 0o777;
    fs.chmodSync(studio.home, mode & ~0o200);
    restore = () => fs.chmodSync(studio.home, mode);
  }
  try {
    const blocked = await primary("POST", route, { operation }, headers);
    assert.equal(blocked.status, 503, blocked.text);
    assert.equal(blocked.json.operation, operation);
    assert.equal(typeof blocked.json.proof.s, "string");
    assert.deepEqual(readJSON(path.join(studio.home, "machines.json")), addresses);
  } finally {
    await restore();
  }
  const saved = link().shared.membershipAndAccess;
  assert.ok(saved.value.left.includes(member.key));
  assert.equal(link().pendingMachineCleanup[member.key], "lab");
  await running.stop();
  const restarted = studio.connect(await studio.start().ready());
  const replay = await restarted("POST", route, { operation }, headers);
  assert.equal(replay.status, 200, replay.text);
  assert.deepEqual(link().shared.membershipAndAccess, saved);
  assert.equal(link().pendingMachineCleanup, undefined);
  assert.deepEqual(readJSON(path.join(studio.home, "machines.json")), [addresses[1]]);
});

test("SSH Add suggests configured addresses and refuses input that cannot name a computer", {
  skip: process.platform === "win32" && boxBinary().typescript,
  timeout: 15000,
}, async t => {
  const { main, ask, account, fake, headers, page, releaseMarker } = await addBench(t);
  const config = path.join(account, ".ssh", "config");
  const included = path.join(account, ".ssh", "conf.d");
  write(config, [
    "# my machines", "Host studio mini", "  HostName 10.0.0.5", "host=gpu # comments",
    "Host *.internal !bastion web-?", "Include conf.d/*.conf", "Include ~/elsewhere",
    `Include "${path.join(account, "absolute")}"`, "Host studio", "Host bad:port",
  ].join("\n"));
  write(path.join(included, "b.conf"), "Host late\n");
  write(path.join(included, "a.conf"), "Host early\nInclude conf.d/a.conf\n");
  write(path.join(included, "skipped.txt"), "Host never\n");
  write(path.join(account, "elsewhere"), 'Host "quoted"\n');
  write(path.join(account, "absolute"), "Host last\n");
  const saved = path.join(main.home, "machines.json");
  write(saved, JSON.stringify([{ destination: "Studio", otherAddresses: ["late"] }]));
  assert.deepEqual(await page(), { can: true, hosts: ["mini", "gpu", "early", "quoted", "last"], tries: [] });
  fs.renameSync(releaseMarker, releaseMarker + ".test");
  assert.deepEqual(await page(), {
    can: false, hosts: [], tries: [],
    why: "Kuvo Code on Studio has no files of its own to give another computer yet. Update Kuvo Code here, then try again.",
  });
  fs.renameSync(releaseMarker + ".test", releaseMarker);
  fs.unlinkSync(config);
  assert.deepEqual((await page()).hosts, []);

  const syntax = "Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name.";
  for (const [destination, message] of [
    ...["-oProxyCommand=sh", "host:2222", "ssh://host/path", "::1", "two words", "a\nb", "me@you@host", "@host", "me@", "$(reboot)", "a;b", "a'b"].map(address => [address, syntax]),
    ["", "Give a host name, or user@host."],
    [12, "Give a host name, or user@host."],
    ["This-Mac.localhost", "That’s this computer."],
    ["a".repeat(400), "That's too long for a host's name."],
  ]) {
    const refusal = await ask("POST", "kuvo/computers/add", { destination }, headers);
    assert.equal(refusal.status, 400, refusal.text);
    assert.equal(refusal.json.message, message);
  }
  assert.equal(fs.existsSync(path.join(fake, "log")), false);
});

test("SSH Add preserves an unreadable machine list and recognizes a missing list with saved links", {
  skip: process.platform === "win32" && boxBinary().typescript,
  timeout: 15000,
}, async t => {
  const { main, ask, fake, headers, page } = await addBench(t);
  assert.deepEqual(await page(), { can: true, hosts: [], tries: [] });
  const list = path.join(main.home, "machines.json");
  const links = path.join(main.home, "phone-link.json");
  const saved = JSON.stringify({ machines: { lab: { name: "Lab" } } });
  write(links, saved);
  const why = "Studio’s list of machines can’t be read right now.";
  for (const contents of [null, "{ invalid JSON", '[{"identity":"machine"}]']) {
    if (contents !== null) write(list, contents);
    assert.deepEqual(await page(), { can: false, why, hosts: [], tries: [] });
    const refused = await ask("POST", "kuvo/computers/add", { destination: "lab" }, headers);
    assert.equal(refused.status, 503, refused.text);
    assert.equal(refused.json.message, why);
    assert.equal(fs.existsSync(list) ? fs.readFileSync(list, "utf8") : null, contents);
    assert.equal(fs.readFileSync(links, "utf8"), saved);
  }
  assert.equal(fs.existsSync(path.join(fake, "log")), false);
});

test("SSH Add leaves an older account reservation in place", {
  skip: process.platform === "win32" && "The older reservation is a Unix symlink.",
  timeout: 20000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const destination = "old-reservation";
  const marker = path.join(remote, ".kuvo-code-adding");
  const previous = "0123456789abcdef".repeat(3);
  fs.symlinkSync(previous, marker);
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
  assert.equal(started.status, 202, started.text);
  const failed = await until(async () => {
    const attempt = (await page()).tries.find(attempt => attempt.id === started.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "SSH Add did not report the older reservation.");
  assert.equal(failed.failure.message, "An older version of Kuvo Code left an unfinished change on old-reservation. Restart old-reservation to end that change, then sign in to the same OS account and run rm -f ~/.kuvo-code-adding before trying again.");
  assert.equal(fs.readlinkSync(marker), previous);
  assert.equal(fs.existsSync(path.join(remote, ".kuvo-code")), false);
});

test("SSH Add waits for an active account writer and leaves its reservation alone", {
  skip: process.platform === "win32", timeout: 35000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const destination = "busy-account";
  const marker = path.join(remote, ".kuvo-code-adding");
  const token = `guarded-${"51".repeat(24)}`;
  fs.symlinkSync(token, marker);
  const lock = `${marker}.lock`;
  write(lock, "", { mode: 0o600 });
  const entered = path.join(remote, "writer-entered");
  const release = path.join(remote, "release-writer");
  const child = launch("/bin/sh", ["-c", `
    (
      case "$(uname -s)" in
        Darwin) /usr/bin/lockf -t 0 9 ;;
        *) flock -n 9 ;;
      esac || exit 1
      : >"$2"
      while [ ! -f "$3" ]; do sleep 0.1; done
    ) 9<>"$1"
  `, "kuvo-test-writer", lock, entered, release], { PATH: process.env.PATH }, remote);
  t.after(async () => {
    write(release, "");
    await child.stop();
  });
  await until(() => fs.existsSync(entered), child.log);
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
  assert.equal(started.status, 202, started.text);
  const failed = await until(async () => {
    const attempt = (await page()).tries.find(attempt => attempt.id === started.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "SSH Add did not report the account writer.");
  assert.equal(failed.failure.message, "Another Kuvo Code change is in progress on busy-account. Try again once it finishes.");
  assert.equal(fs.readlinkSync(marker), token);
  assert.equal(fs.existsSync(path.join(remote, ".kuvo-code")), false);
});

test("SSH Add recovery can only replace the reservation shown when asked", {
  skip: process.platform === "win32", timeout: 35000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const destination = "interrupted-account";
  const marker = path.join(remote, ".kuvo-code-adding");
  const previous = `guarded-${"53".repeat(24)}`;
  const next = `guarded-${"71".repeat(24)}`;
  fs.symlinkSync(previous, marker);
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
  assert.equal(started.status, 202, started.text);
  const id = started.json.try.id;
  const readTry = async () => (await page()).tries.find(attempt => attempt.id === id);
  const waiting = await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "asking" && attempt;
  }, "SSH Add did not offer recovery.");
  assert.equal(waiting.question.id, "recoverAccount");
  assert.deepEqual(waiting.question.answers.map(answer => answer.id), ["cancel", "recover"]);
  assert.equal(fs.readlinkSync(marker), previous);
  const answer = input => ask("POST", `kuvo/computers/add/${id}/answer`, input, headers);
  assert.equal((await answer({ answer: "recover", at: waiting.question.at + 1 })).status, 409);
  const replacement = `${marker}.replacement`;
  fs.symlinkSync(next, replacement);
  fs.renameSync(replacement, marker);
  assert.equal((await answer({ answer: "recover", at: waiting.question.at })).status, 200);
  const failed = await until(async () => {
    const attempt = await readTry();
    return attempt?.state === "failed" && attempt;
  }, "SSH Add did not refuse the replaced reservation.");
  assert.equal(failed.failure.message, "Another Kuvo Code change is in progress on interrupted-account. Try again once it finishes.");
  assert.equal(fs.readlinkSync(marker), next);
  assert.equal(fs.existsSync(path.join(remote, ".kuvo-code")), false);
});

test("SSH Add reports that it cannot prepare an invalid account lock", {
  skip: process.platform === "win32", timeout: 30000,
}, async t => {
  const { main, ask, remote, headers, page } = await addBench(t);
  const destination = "invalid-account-lock";
  const marker = path.join(remote, ".kuvo-code-adding");
  const sentinel = path.join(remote, "fixture-file");
  write(sentinel, "do not change this file");
  fs.symlinkSync(sentinel, `${marker}.lock`);
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
  assert.equal(started.status, 202, started.text);
  const failed = await until(async () => {
    const attempt = (await page()).tries.find(attempt => attempt.id === started.json.try.id);
    return attempt?.state === "failed" && attempt;
  }, "SSH Add did not report the failed account reservation.");
  assert.equal(failed.failure.message, "Kuvo Code couldn’t prepare the OS account on invalid-account-lock. Check that you can write to your home folder there, then try again.");
  assert.equal(fs.readlinkSync(`${marker}.lock`), sentinel);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "do not change this file");
  assert.equal(fs.existsSync(marker), false);
  assert.equal(fs.existsSync(path.join(remote, ".kuvo-code")), false);
});

test("SSH Add keeps older Mac data in place and asks that Mac to update", {
  skip: process.platform === "win32", timeout: 35000,
}, async t => {
  const { main, ask, remote, fake, headers, page } = await addBench(t);
  const destinationOf = [
    ["old07", ["opencode", "opencode.db"]],
    ["oldkeeper", ["box", "run", "keeper.json"]],
  ];
  write(path.join(main.home, "ssh", "known_hosts"), destinationOf.map(([destination]) => `${destination} ssh-ed25519 ${key(8)}\n`).join(""));
  for (const [destination, file] of destinationOf) {
    const account = path.join(remote, destination);
    write(path.join(fake, `home-${destination}`), account);
    const oldFile = path.join(account, "Library", "Application Support", "run.kuvo.code", ...file);
    write(oldFile, "kept exactly");
    const before = fs.readdirSync(account, { recursive: true }).sort();
    const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
    assert.equal(started.status, 202, started.text);
    const refused = await until(async () => {
      const attempt = (await page()).tries.find(attempt => attempt.id === started.json.try.id);
      return attempt?.state === "failed" && attempt;
    }, "Add did not stop before changing the older Mac's work.");
    assert.equal(refused.failure.message, `${destination} still has Kuvo Code 0.7’s sessions. Update it to Kuvo first — its sessions move over then — and add it again.`);
    assert.equal(refused.failure.again, false);
    const lock = path.join(account, ".kuvo-code-adding.lock");
    assert.equal(fs.statSync(lock).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(account, { recursive: true }).sort(), [".kuvo-code-adding.lock", ...before]);
    assert.equal(fs.readFileSync(oldFile, "utf8"), "kept exactly");
    assert.equal(fs.existsSync(path.join(account, ".kuvo-code")), false);
  }
});

test("SSH Add preserves an independent installation that appeared after the probe", {
  skip: boxBinary().typescript || process.platform === "win32", timeout: 45000,
}, async t => {
  const fixture = await addBench(t);
  const { computer, main, ask, remote, fake, headers, page } = fixture;
  const independent = await computer("Independent");
  const existing = independent.connect(await independent.start().ready());
  const setup = (await existing("GET", "kuvo/setup")).json.setup;
  assert.notEqual(setup, (await ask("GET", "kuvo/setup")).json.setup);
  const destination = "enrolled-during-add";
  const installed = path.join(remote, ".kuvo-code");
  const engine = stageEngine(t, installed);
  const { selectedKey } = await selectedSshCredentials(fixture);
  write(path.join(main.home, "box", "opencode.json"), JSON.stringify({
    model: "fixture/model", headers: { Authorization: "an inline token" }, mcp: {
      local: { type: "local", command: ["fixture"], environment: { TOKEN: `{file:/source/home/keys/${selectedKey}}` } },
      remote: { type: "remote", url: "https://fixture.invalid", headers: { Authorization: "an inline MCP token" } },
    },
  }));
  const hold = path.join(fake, `hold-${destination}`);
  write(hold, "ssh-enroll");
  t.after(() => fs.rmSync(hold, { force: true }));
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination, credentials: [] }, headers);
  assert.equal(started.status, 202, started.text);
  const readTry = async () => (await page()).tries.find(attempt => attempt.id === started.json.try.id);
  await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.steps.some(step => step.id === "joining" && step.state === "now");
  }, "SSH Add did not reach enrollment.", 30000);
  assert.deepEqual(readJSON(path.join(installed, "box", "opencode.json")), { model: "fixture/model" });
  assert.equal(fs.existsSync(engine.imported), false);
  assert.equal(fs.existsSync(path.join(installed, "box", "keys", selectedKey)), false);
  const state = readJSON(path.join(independent.home, "run", "link.json"));
  assert.equal(state.setup.id, setup);
  const link = path.join(installed, "run", "link.json");
  const kept = path.join(installed, "box", "keys", "KUVO_KEY_EXISTING");
  write(link, JSON.stringify(state));
  write(kept, "existing key");
  fs.unlinkSync(hold);
  const refused = await until(async () => {
    const attempt = await readTry();
    return attempt?.state === "failed" && attempt;
  }, "The second installation was not refused.");
  assert.match(refused.failure.message, /connect over SSH to your own OS account/);
  assert.equal(refused.failure.again, false);
  assert.deepEqual(readJSON(link), state);
  assert.equal(fs.readFileSync(kept, "utf8"), "existing key");
  assert.equal((await existing("GET", "kuvo/setup")).json.setup, setup);
});
