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
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
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
