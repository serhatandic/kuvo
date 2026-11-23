import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { addSshStandIn } from "../../box/bridge-tests/add-ssh-stand-in.js";
import { boxBinary, readJSON, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("SSH Add trusts the selected host, copies the running Rust release and starts an enrolled resident", {
  skip: boxBinary().typescript || process.platform === "win32",
  timeout: 45000,
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
  const begun = await ask("POST", "kuvo/computers/add", { destination: "lab" }, headers);
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
  const answer = at => ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at }, headers);
  assert.equal((await answer(found.question.at + 1)).status, 409);
  assert.equal(fs.existsSync(path.join(main.home, "ssh", "known_hosts")), false);
  assert.equal((await answer(found.question.at)).status, 200);
  const done = await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "done" && attempt;
  }, "SSH Add did not complete.", 35000);
  assert.equal(done.computer.name, "lab");
  assert.equal(fs.readFileSync(path.join(main.home, "ssh", "known_hosts"), "utf8"), `lab ssh-ed25519 ${key}\n`);
  assert.deepEqual((await page()).json.hosts, ["second"]);
  const identity = fs.readFileSync(path.join(remote, "box", "machine-id"), "utf8");
  assert.match(identity, /^[a-f0-9]{32}$/);
  assert.deepEqual(readJSON(path.join(main.home, "machines.json")), [{ destination: "lab", identity }]);
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
  const alias = await ask("POST", "kuvo/computers/add", { destination: "second" }, headers);
  assert.equal(alias.status, 202, alias.text);
  const aliasID = alias.json.try.id;
  const remembered = await until(async () => {
    const attempt = (await page()).json.tries.find(attempt => attempt.id === aliasID);
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.state === "done" && attempt;
  }, "The known computer was not found at its other address.");
  assert.equal(remembered.known, true);
  assert.equal(remembered.title, "second is lab, already one of your computers");
  assert.deepEqual(remembered.computer, done.computer);
  assert.deepEqual(readJSON(path.join(main.home, "machines.json")), [{ destination: "lab", identity, otherAddresses: ["second"] }]);
  assert.equal((await ask("GET", "kuvo/setup")).json.computers.length, 2);
  if (oldWork) assert.equal(fs.readFileSync(oldWork, "utf8"), "separate earlier work");

  const fake = path.join(path.dirname(remote), "ssh");
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
  const host = roster.find(each => each.key !== done.computer.key);
  const lab = roster.find(each => each.key === done.computer.key);
  let lost = false;
  const pause = relay.intercept(Buffer.from(host.address, "base64url"), Buffer.from(lab.address, "base64url"), () => {
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
});

const key = byte => Buffer.concat([
  Buffer.from("0000000b7373682d6564323535313900000020", "hex"), Buffer.alloc(32, byte),
]).toString("base64");

async function addBench(t) {
  const { computer, leases } = await bench(t);
  const implementation = boxBinary();
  let fake, remote, account, releaseMarker;
  const main = await computer("Studio", implementation, folder => {
    fake = path.join(folder, "ssh");
    remote = path.join(folder, "remote");
    account = path.join(folder, "account");
    fs.mkdirSync(remote);
    const installed = path.join(folder, "box");
    if (implementation.typescript) {
      for (const file of ["bot-words.json", "integrations-mac.json", "integrations.json",
        "kuvo-agents.js", "kuvo-browser.js", "kuvo-launch", "kuvo-link.js", "kuvo-npx",
        "kuvo-setsid", "kuvo-start", "kuvo-wake", "mcp-catalog.json", "runtime/node", ".kuvo-boot"]) {
        write(path.join(installed, "bin", file), "");
      }
      write(path.join(installed, "config", "opencode", "plugins", "kuvo-code.js"), "");
      releaseMarker = path.join(installed, "bin", "box-version");
      write(releaseMarker, "1.2.3\n");
    } else {
      fs.mkdirSync(path.join(installed, "bin"), { recursive: true });
      releaseMarker = path.join(installed, "bin", process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box");
      fs.copyFileSync(implementation.file, releaseMarker);
    }
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
  return { main, ask, account, remote, fake, headers, page, releaseMarker };
}

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
