import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { test } from "node:test";
import {
  added, alive, ask, base, checkedCredentials, fake, home, homes, k, link, over, pidsThere, read,
  tools, until, write,
} from "./computers-add-fixture.js";

test("an account enrolled after the probe keeps its data when the second setup is refused", async () => {
  const kuvo = path.join(homes.race, ".kuvo-code");
  const hold = path.join(fake, "hold");
  fs.writeFileSync(hold, 'kuvo-link.js" setup');
  let joining;
  const saved = path.join(kuvo, "run", "link.json");
  const keyFile = path.join(kuvo, "box", "keys", "KUVO_KEY_EXISTING");
  const mac = crypto.randomBytes(32).toString("base64url");
  const state = { mac, macs: [mac], phones: [], sign: link.newSigningSeed(), formatVersion: 1 };
  try {
    joining = await added("race", found => found?.steps.find(step => step.id === "joining")?.state === "now");
    write(saved, JSON.stringify(state));
    write(keyFile, "existing-key", 0o600);
  } finally {
    fs.rmSync(hold);
  }
  const refused = await over(joining.id);
  assert.equal(refused.state, "failed");
  assert.match(refused.failure.message, /connect over SSH to your own OS account/);
  assert.deepEqual(read(saved), state);
  assert.equal(fs.readFileSync(keyFile, "utf8"), "existing-key");
});

test("a second SSH add cannot install or seed an account reserved by the first", async () => {
  const folder = homes.claim;
  const hold = path.join(fake, "hold-claim");
  write(path.join(fake, "home-claim-alias"), folder);
  write(hold, "tar xf - -C");
  let first;
  try {
    first = await added("claim", found => found?.steps.find(step => step.id === "files")?.state === "now");
    const second = await added("claim-alias");
    assert.equal(second.state, "failed");
    assert.match(second.failure.message, /change is in progress/);
    assert.equal(fs.existsSync(path.join(folder, ".kuvo-code", "box", "keys")), false);
    assert.equal(fs.existsSync(path.join(folder, "engine-credentials.json")), false);
  } finally {
    fs.rmSync(hold);
    if (first) await over(first.id);
  }
  assert.equal((await over(first.id)).state, "done");
  assert.equal(fs.lstatSync(path.join(folder, ".kuvo-code-adding"), { throwIfNoEntry: false }), undefined);
  const uninstall = await ask("POST", `kuvo/computers/${(await over(first.id)).computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
});

const traceFile = path.join(fake, "trace");
const remoteCalls = () => fs.readFileSync(traceFile, "utf8").trim().split("\n");
const reservationAt = folder => path.join(folder, ".kuvo-code-adding");
const retryAdd = async destination => over((await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() })).body.try.id);
const answerRecovery = (attempt, answer) => ask("POST", `kuvo/computers/add/${attempt.id}/answer`, { answer, at: attempt.question.at });
const waitFor = async found => {
  for (let i = 0; i < 200; i++) {
    if (found()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail("The remote command did not reach the test hold.");
};

for (const [destination, disconnected] of [["interrupted", "SIGTERM"], ["network-drop", "255"]]) {
  test(`failure bench: a credential write interrupted by SSH ${disconnected} stops Add and recovery waits for the remote writer`, async () => {
    const folder = homes[destination];
    const alias = `${destination}-alias`;
    const hold = path.join(folder, "hold-seed");
    const drop = path.join(fake, `drop-seed-${destination}`);
    write(path.join(fake, `home-${alias}`), folder);
    write(traceFile, "");
    write(hold, "");
    write(drop, disconnected);
    try {
      const stopped = await added(destination);
      const writes = remoteCalls().filter(call => call.startsWith(`${destination} `) && / (seed|join|uninstall|release)$/.test(call));
      assert.deepEqual(writes, [`${destination} seed`], "one seed call and no join, uninstall or release");
      assert.equal(stopped.state, "failed");
      assert.match(stopped.failure.message, /lost contact.*during a change/);
      assert.equal(stopped.steps.find(step => step.id === "joining").state, "waiting");
      const original = fs.readlinkSync(reservationAt(folder));
      const whileWriting = await added(alias);
      assert.equal(whileWriting.state, "failed");
      assert.match(whileWriting.failure.message, /change is in progress/);
      assert.equal(fs.readlinkSync(reservationAt(folder)), original);

      fs.rmSync(hold);
      await waitFor(() => fs.existsSync(path.join(folder, "writer-ended")));
      fs.rmSync(drop);
      const mismatched = disconnected === "255";
      const recovery = mismatched
        ? await over((await ask("POST", "kuvo/computers/add", { destination, credentials: [] })).body.try.id)
        : await retryAdd(destination);
      assert.equal(recovery.question?.id, "recoverAccount");
      assert.equal(recovery.question.title, `Continue adding ${destination}?`);
      assert.equal(recovery.question.message, `An earlier attempt to add ${destination} didn't finish, and it's no longer running. Kuvo can clear it and start again.`);
      assert.deepEqual(recovery.question.answers.map(answer => answer.label), ["Cancel", "Clear and Continue"]);
      assert.equal(fs.readlinkSync(reservationAt(folder)), original, "recovery requires an answer");
      assert.equal((await answerRecovery(recovery, "recover")).status, 200);
      let recovered = await over(recovery.id);
      if (mismatched) {
        assert.equal(recovered.state, "failed");
        assert.match(recovered.failure.message, /different choice of sign-ins and keys/);
        assert.deepEqual(recovered.credentials, stopped.credentials);
        recovered = await retryAdd(destination);
      }
      assert.equal(recovered.state, "done", JSON.stringify(recovered.failure));
      assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
      assert.equal(read(path.join(folder, "engine-credentials.json")).length, 2);
      assert.equal(remoteCalls().filter(call => call === `${destination} seed`).length, 2, "the resumed add completes the selected seed step");
      assert.ok(!fs.existsSync(path.join(folder, ".kuvo-code/run/.ssh-fresh-add")));
    } finally {
      fs.rmSync(hold, { force: true });
      fs.rmSync(drop, { force: true });
      fs.rmSync(traceFile);
    }
  });
}

test("a failed final release is reported and an abandoned reservation can be cleared explicitly", async () => {
  const folder = homes.release;
  const fail = path.join(fake, "fail-release-release");
  const move = path.join(tools, "mv");
  const replaced = path.join(folder, "replaced-reservation");
  write(traceFile, "");
  write(fail, "");
  try {
    const result = await added("release");
    assert.equal(result.state, "failed");
    assert.match(result.failure.message, /finished the change but couldn’t clear the account/);
    assert.ok(result.computer?.key, "the failed release reports the committed computer");
    const original = fs.readlinkSync(reservationAt(folder));
    fs.rmSync(fail);
    const question = await retryAdd("release");
    assert.equal(question.question?.id, "recoverAccount");
    assert.equal((await answerRecovery(question, "cancel")).status, 200);
    assert.equal(await over(question.id), null);
    assert.equal(fs.readlinkSync(reservationAt(folder)), original);
    const recovery = await retryAdd("release");
    assert.equal(recovery.question?.id, "recoverAccount");
    write(move, `#!/bin/sh
for last do :; done
if [ "$last" = "$HOME/.kuvo-code-adding" ]; then
  [ -L "$last" ] || exit 1
  readlink "$last" > "$HOME/replaced-reservation"
fi
exec /bin/mv "$@"
`, 0o755);
    assert.equal((await answerRecovery(recovery, "recover")).status, 200);
    const done = await over(recovery.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.equal(done.computer.key, result.computer.key);
    assert.equal(fs.readFileSync(replaced, "utf8").trim(), original, "the old reservation still exists at replacement");
    assert.deepEqual(fs.readdirSync(folder).filter(name => name.startsWith(".kuvo-code-adding.recovery.")), []);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    fs.rmSync(move, { force: true });
    fs.rmSync(fail, { force: true });
    fs.rmSync(traceFile);
  }
});

test("a recovery answer only clears the reservation shown to the user", async () => {
  const folder = homes.prepare;
  const marker = reservationAt(folder);
  const interrupted = "guarded-" + crypto.randomBytes(24).toString("hex");
  const newer = "guarded-" + crypto.randomBytes(24).toString("hex");
  fs.symlinkSync(interrupted, marker);
  try {
    const recovery = await added("prepare");
    assert.equal(recovery.question?.id, "recoverAccount");
    fs.rmSync(marker);
    fs.symlinkSync(newer, marker);
    assert.equal((await answerRecovery(recovery, "recover")).status, 200);
    assert.equal((await over(recovery.id)).state, "failed");
    assert.equal(fs.readlinkSync(marker), newer);
  } finally {
    fs.rmSync(marker);
  }
});

test("an old reservation is kept and Add explains how to clear it after a restart", async () => {
  const marker = reservationAt(homes["old-reservation"]);
  const original = "0123456789abcdef".repeat(3);
  fs.symlinkSync(original, marker);
  const result = await added("old-reservation");
  assert.equal(result.state, "failed");
  assert.equal(result.failure.message, "An older version of Kuvo Code left an unfinished change on old-reservation. Restart old-reservation to end that change, then sign in to the same OS account and run rm -f ~/.kuvo-code-adding before trying again.");
  assert.equal(fs.readlinkSync(marker), original);
});

test("SSH uninstall cannot probe ownership while Add holds the account", async () => {
  const folder = homes["add-first"];
  const installed = await added("add-first");
  assert.equal(installed.state, "done");
  const hold = path.join(fake, "hold-add-first-alias");
  const keyFile = path.join(folder, ".kuvo-code", "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  write(path.join(fake, "home-add-first-alias"), folder);
  write(traceFile, "");
  write(hold, "uname -sm");
  try {
    const adding = await added("add-first-alias", found => found?.state === "working" && !found.question);
    await waitFor(() => remoteCalls().includes("add-first-alias probe"));
    const begun = await ask("POST", `kuvo/computers/${installed.computer.key}/uninstall`);
    const uninstallSecond = await over(begun.body.try.id);
    assert.equal(uninstallSecond.state, "failed");
    assert.equal(remoteCalls().includes("add-first probe"), false, "uninstall first needs the reservation");
    assert.match(uninstallSecond.failure.message, /change is in progress/);
    assert.ok(fs.existsSync(keyFile));
    fs.rmSync(hold);
    assert.equal((await over(adding.id)).state, "done");
  } finally {
    fs.rmSync(hold, { force: true });
    fs.rmSync(traceFile);
  }
});

test("SSH Add is blocked from the time uninstall probes ownership through deletion", async () => {
  const folder = homes["uninstall-first"];
  const installed = await added("uninstall-first");
  assert.equal(installed.state, "done");
  const probeHold = path.join(fake, "hold-uninstall-first");
  const deleteHold = path.join(folder, "hold-uninstall");
  const keyFile = path.join(folder, ".kuvo-code", "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  write(path.join(fake, "home-uninstall-first-alias"), folder);
  write(traceFile, "");
  write(probeHold, "uname -sm");
  write(deleteHold, "");
  try {
    const begun = await ask("POST", `kuvo/computers/${installed.computer.key}/uninstall`);
    await waitFor(() => remoteCalls().includes("uninstall-first probe"));
    const beforeDelete = fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false });
    fs.rmSync(probeHold);
    await waitFor(() => fs.existsSync(path.join(folder, "deleting")));
    const addSecond = await added("uninstall-first-alias");
    assert.equal(addSecond.state, "failed");
    assert.equal(remoteCalls().includes("uninstall-first-alias probe"), false);
    assert.match(addSecond.failure.message, /change is in progress/);
    assert.ok(beforeDelete?.isSymbolicLink(), "uninstall must hold the reservation before its probe");
    assert.ok(fs.existsSync(keyFile));
    fs.rmSync(deleteHold);
    assert.equal((await over(begun.body.try.id)).state, "done");
    assert.equal(fs.existsSync(keyFile), false);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    fs.rmSync(probeHold, { force: true });
    fs.rmSync(deleteHold, { force: true });
    fs.rmSync(traceFile);
  }
});

test("a reservation creation error says the account couldn't be prepared and logs the SSH detail", async () => {
  const folder = homes["prepare-error"];
  const refuse = path.join(folder, "refuse-reservation");
  write(refuse, "");
  const logged = [];
  const log = console.error;
  console.error = (...args) => logged.push(args.join(" "));
  try {
    const result = await added("prepare-error");
    assert.equal(result.state, "failed");
    assert.equal(result.failure.message, "Kuvo Code couldn’t prepare the OS account on prepare-error. Check that you can write to your home folder there, then try again.");
    assert.match(logged.join("\n"), /ln: test account can't create the reservation/);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    console.error = log;
    fs.rmSync(refuse);
  }
});

const macbookAlias = "macbook.lan";
const macbookAddress = destination => destination === "macbook" || destination === macbookAlias;
const macbookPeer = () => link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine" && macbookAddress(each.id));

async function holdAliasAdd(phone, during) {
  const host = path.join(fake, "hold");
  fs.writeFileSync(host, "kuvo kind");
  let id;
  try {
    const started = await ask("POST", "kuvo/computers/add", { destination: macbookAlias }, phone);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps[0]?.state === "now");
    await during();
  } finally {
    fs.rmSync(host);
  }
  return over(id);
}

test("an alias Add honours only its original pairing and cannot restore a key removed during the Add", async () => {
  const installed = await added("macbook");
  assert.equal(installed.state, "done", JSON.stringify(installed.failure));
  const original = macbookPeer();
  const otherMachines = read(path.join(home, "machines.json")).filter(each => !macbookAddress(each.destination));
  fs.appendFileSync(link.KNOWN_HOSTS, `${macbookAlias} ${fs.readFileSync(path.join(fake, "hostkey"), "utf8").trim()}\n`);
  fs.writeFileSync(path.join(fake, `home-${macbookAlias}`), homes.macbook);
  const key = link.publicOf(link.newKey());
  const phone = { key, pairing: "first" };
  link.updateLink(state => {
    link.pairPhone(state, { key: k(key), pairing: phone.pairing, name: "Add caller", from: "pair" });
    return state;
  });
  const revokedCaller = await holdAliasAdd(phone, async () => {
    assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
    link.updateLink(state => {
      link.pairPhone(state, { key: k(key), pairing: "second", name: "Add caller", from: "pair" });
      return state;
    });
  });
  const authorized = { key, pairing: "second" };
  const removedPeer = await holdAliasAdd(authorized, async () => {
    assert.equal((await ask("DELETE", `kuvo/computers/${original.key}`)).status, 200);
  });
  assert.deepEqual([revokedCaller.state, removedPeer.state], ["failed", "failed"]);
  assert.match(revokedCaller.failure.message, /device that started this add is no longer paired/);
  assert.match(removedPeer.failure.message, /was removed from your computers/);
  assert.equal(macbookPeer(), undefined);
  assert.deepEqual(read(path.join(home, "machines.json")), otherMachines);

  const fresh = await ask("POST", "kuvo/computers/add", { destination: macbookAlias }, authorized);
  const done = await over(fresh.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.notEqual(done.computer.key, original.key, "a fresh Add must use a new key");
  assert.ok(link.readLink().removedHere.includes(original.key), "the old key stays revoked");
});

test("uninstall revokes the computer even when its desktop identity folder cleanup fails", async () => {
  // The preceding test leaves this box joined through the alias.
  const computer = macbookPeer();
  const appKey = link.publicOf(link.newKey());
  link.updateLink(state => {
    link.pairDevice(state, { key: k(appKey), name: "MacBook desktop" });
    return state;
  });
  const pairing = link.readLink().phones.find(each => each.key === k(appKey)).pairing;
  const kuvo = path.join(homes.macbook, ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "paired identity", 0o600);
  assert.equal(link.authorizePhone(appKey, {}), true);
  const rm = path.join(tools, "rm");
  write(rm, '#!/bin/sh\nfor arg do [ "$arg" != "$HOME/.kuvo-code/desktop" ] || exit 1; done\nexec /bin/rm "$@"\n', 0o755);
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202);
    result = await over(started.body.try.id);
  } finally {
    fs.rmSync(rm);
  }
  assert.equal(result.state, "failed");
  assert.equal(fs.readFileSync(identity, "utf8"), "paired identity");
  assert.deepEqual(pidsThere(kuvo).filter(alive), [], "the service stopped");
  assert.equal(macbookPeer(), undefined, "the computer membership was revoked");
  assert.equal(link.authorizePhone(appKey, {}), true, "the separate device grant is still valid");
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(appKey), pairing }] })).status, 200);
  assert.equal(link.authorizePhone(appKey, {}), false, "the device removal revokes the surviving identity");
  assert.ok(link.ownPhones(link.readLink()).removed.some(each => each.key === k(appKey) && each.pairing === pairing));
  assert.match(result.failure.message, /access to your other computers was revoked.*desktop identity folder/i);
});

test("uninstall logs a link-write failure without putting the local path in its result", async () => {
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  const computer = macbookPeer();
  const rename = fs.renameSync;
  const linkFile = path.join(home, "run", "link.json");
  fs.renameSync = (from, to) => {
    if (to === linkFile) throw new Error(`Can't write ${linkFile}`);
    return rename(from, to);
  };
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    result = await over(started.body.try.id);
  } finally {
    fs.renameSync = rename;
  }
  assert.equal(result.state, "failed");
  assert.equal(result.failure.message, "Kuvo Code couldn’t confirm that access was revoked. Try uninstalling again.");
  assert.equal(macbookPeer().key, computer.key);
  assert.ok(read(path.join(home, "machines.json")).some(each => each.destination === "macbook"), "the saved address is still available for retry");
});

test("uninstall revokes access before SSH cleanup and reports a lost reply without its diagnostic", async () => {
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  const computer = macbookPeer();
  const key = link.publicOf(link.newKey());
  link.updateLink(state => { link.pairDevice(state, { key: k(key), name: "MacBook desktop" }); return state; });
  const marker = path.join(fake, "lose-uninstall-reply");
  fs.writeFileSync(marker, "");
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    result = await over(started.body.try.id);
  } finally {
    fs.rmSync(marker);
  }
  assert.deepEqual({
    state: result.state, message: result.failure.message,
    letsIn: link.authorizePhone(key, {}), present: Boolean(macbookPeer()),
  }, {
    state: "failed", message: "macbook’s access to your other computers was revoked. Kuvo Code couldn’t confirm whether cleanup finished on that computer. Run the local uninstall command (kuvo-link.js uninstall) on that computer to finish cleanup.",
    letsIn: true, present: false,
  });
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
  assert.deepEqual(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive), []);
});

for (const [destination, heldScript, step, expected] of [
  ["early-seed", "kuvo kind", "checking", ["probe"]],
  ["late-seed", 'bin/kuvo-start" start', "starting", ["probe", "start"]],
]) {
  test(`a revoked Add stops sending credentials after ${step}`, async () => {
    const folder = path.join(base, destination);
    homes[destination] = folder;
    fs.mkdirSync(folder);
    write(path.join(fake, `home-${destination}`), folder);
    for (const tool of ["pkill", "caffeinate"]) write(path.join(folder, ".kuvo-code", "bin", tool), "#!/bin/sh\nexit 0\n", 0o755);
    fs.appendFileSync(link.KNOWN_HOSTS, `${destination} ${fs.readFileSync(path.join(fake, "hostkey"), "utf8").trim()}\n`);
    const key = link.publicOf(link.newKey());
    link.updateLink(state => { link.pairPhone(state, { key: k(key), name: "Add caller", from: "pair", pairing: "caller" }); return state; });
    const track = path.join(fake, "track");
    const hold = path.join(fake, "hold");
    fs.writeFileSync(track, "");
    fs.writeFileSync(hold, heldScript);
    let id;
    try {
      const started = await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() }, { key, pairing: "caller" });
      assert.equal(started.status, 202, JSON.stringify(started.body));
      id = started.body.try.id;
      await until(id, found => found?.steps.find(each => each.id === step)?.state === "now");
      const scriptStarted = () => fs.readFileSync(track, "utf8").split("\n").includes(expected.at(-1));
      const deadline = Date.now() + 15_000;
      while (!scriptStarted() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
      assert.ok(scriptStarted(), "the remote held script was reached before revoking the caller");
      assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
    } finally {
      fs.rmSync(hold);
    }
    const ended = await over(id);
    const ran = fs.readFileSync(track, "utf8").trim().split("\n");
    fs.rmSync(track);
    assert.equal(ended.state, "failed");
    assert.match(ended.failure.message, /device that started this add is no longer paired/);
    assert.deepEqual(ran, expected);
  });
}

test("SSH uninstall leaves membership intact when the remote pre-wipe backup fails", async () => {
  const installed = await added("backup-uninstall");
  assert.equal(installed.state, "done", JSON.stringify(installed.failure));
  const { computer } = installed;
  const kuvo = path.join(homes["backup-uninstall"], ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "keep until backup succeeds", 0o600);
  write(path.join(kuvo, "backup", "configuration.json"), '{"version":999}');
  const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const ended = await over(started.body.try.id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /Unsupported backup configuration/);
  assert.ok(link.readLink().macPeers.some(peer => peer.key === computer.key));
  assert.equal(fs.readFileSync(identity, "utf8"), "keep until backup succeeds");
});

test("SSH uninstall stops if the refreshed peer list drops the target during the probe", async () => {
  const addedForProbe = await added("probe-removal");
  assert.equal(addedForProbe.state, "done", JSON.stringify(addedForProbe.failure));
  const computer = addedForProbe.computer;
  const kuvo = path.join(homes["probe-removal"], ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "still here", 0o600);
  const key = link.publicOf(link.newKey());
  link.updateLink(state => { link.pairDevice(state, { key: k(key), name: "MacBook desktop", by: computer.key }); return state; });
  const hold = path.join(fake, "hold");
  fs.writeFileSync(hold, "kuvo kind");
  let id;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps[0]?.state === "now");
    const peers = link.readLink().macPeers.filter(peer => peer.key !== computer.key);
    assert.equal((await ask("PUT", "kuvo/link/peers", { peers })).status, 200);
  } finally {
    fs.rmSync(hold);
  }
  const ended = await over(id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /couldn’t confirm that access was revoked/);
  assert.equal(link.authorizePhone(key, {}), true, "mere absence isn't revocation");
  assert.equal(fs.readFileSync(identity, "utf8"), "still here");
  assert.equal(fs.existsSync(path.join(kuvo, "run", "off")), false);
});

test("Stop after committed revocation retains the unconfirmed SSH cleanup and local instructions", async () => {
  const addedForStop = await added("stop-uninstall");
  assert.equal(addedForStop.state, "done", JSON.stringify(addedForStop.failure));
  const computer = addedForStop.computer;
  const kuvo = path.join(homes["stop-uninstall"], ".kuvo-code");
  write(path.join(kuvo, "desktop", "identity"), "still here", 0o600);
  const marker = path.join(fake, "lose-uninstall-reply");
  const entered = path.join(fake, "uninstall-entered");
  const release = path.join(fake, "uninstall-continue");
  fs.writeFileSync(marker, "before");
  let id;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps.find(step => step.id === "removing")?.state === "now");
    const deadline = Date.now() + 15_000;
    while (!fs.existsSync(entered) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
    assert.ok(fs.existsSync(entered), "the SSH command is outstanding");
    assert.equal((await ask("DELETE", `kuvo/computers/add/${id}`)).status, 200);
  } finally {
    fs.writeFileSync(release, "");
  }
  const ended = await over(id);
  fs.rmSync(marker);
  assert.equal(ended?.state, "failed");
  assert.match(ended.failure.message, /revoked.*couldn’t confirm.*Run the local uninstall command \(kuvo-link\.js uninstall\)/);
  assert.equal(link.macPeersBody(link.readLink()).boxes.some(peer => peer.key === computer.key), false);
  assert.equal(fs.readFileSync(path.join(kuvo, "desktop", "identity"), "utf8"), "still here");
  assert.equal(fs.existsSync(path.join(kuvo, "run", "off")), false);
});
