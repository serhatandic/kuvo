import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { addSshStandIn } from "../../box/bridge-tests/add-ssh-stand-in.js";
import { boxBinary, launch, readJSON, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";

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

async function addBench(t) {
  const { computer, leases } = await bench(t);
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
  return { computer, main, ask, account, remote, fake, headers, page, releaseMarker };
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
  skip: process.platform === "win32" || process.getuid?.() === 0, timeout: 30000,
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
  const mode = fs.statSync(studio.home).mode & 0o777;
  fs.chmodSync(studio.home, mode & ~0o200);
  try {
    const blocked = await primary("POST", route, { operation }, headers);
    assert.equal(blocked.status, 503, blocked.text);
    assert.equal(blocked.json.operation, operation);
    assert.equal(typeof blocked.json.proof.s, "string");
    assert.deepEqual(readJSON(path.join(studio.home, "machines.json")), addresses);
  } finally {
    fs.chmodSync(studio.home, mode);
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
  const { computer, main, ask, remote, fake, headers, page } = await addBench(t);
  const independent = await computer("Independent");
  const existing = independent.connect(await independent.start().ready());
  const setup = (await existing("GET", "kuvo/setup")).json.setup;
  assert.notEqual(setup, (await ask("GET", "kuvo/setup")).json.setup);
  const destination = "enrolled-during-add";
  const hold = path.join(fake, `hold-${destination}`);
  write(hold, "ssh-enroll");
  t.after(() => fs.rmSync(hold, { force: true }));
  write(path.join(main.home, "ssh", "known_hosts"), `${destination} ssh-ed25519 ${key(8)}\n`);
  const started = await ask("POST", "kuvo/computers/add", { destination }, headers);
  assert.equal(started.status, 202, started.text);
  const readTry = async () => (await page()).tries.find(attempt => attempt.id === started.json.try.id);
  await until(async () => {
    const attempt = await readTry();
    if (attempt?.state === "failed") throw new Error(attempt.failure.message);
    return attempt?.steps.some(step => step.id === "joining" && step.state === "now");
  }, "SSH Add did not reach enrollment.", 30000);
  const installed = path.join(remote, ".kuvo-code");
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
