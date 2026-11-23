import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import { atEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { test } from "node:test";
import {
  alive, checkedCredentials, engineBinary, engineVersion, fake, helperBundle, home, homes, k,
  link, primaryHome, read, release, script, sshLog, write,
} from "./computers-add-fixture.js";

// Every SSH installation has its own deadline below. This scenario does several in
// succession, so its outer deadline also covers unlink, wipe and re-enrollment.
test("failure bench: a Linux primary starts fresh, enrolls over SSH and pairs devices", { timeout: 240_000 }, async t => {
  const leases = relayLeaseStandIn();
  const relay = await relayStandIn(link, { request: leases.request });
  atEnd(() => relay.close());
  const primaryBox = path.join(primaryHome, ".kuvo-code");
  const memberHome = homes["primary-member"];
  const memberBox = path.join(memberHome, ".kuvo-code");
  fs.cpSync(release, primaryBox, { recursive: true });
  // On macOS this also exercises the real Mac machine-install and attachment paths.
  if (process.platform === "darwin") fs.cpSync(path.join(home, helperBundle), path.join(primaryBox, helperBundle), { recursive: true });
  write(path.join(primaryBox, "engine", engineVersion, "opencode"), engineBinary, 0o755);
  const env = { ...process.env, HOME: primaryHome, KUVO_HOME: primaryBox, KUVO_TEST_RELAY: relay.url };
  const run = (boxEnv, ...args) => {
    const result = spawnSync(process.execPath, [script, ...args], { env: boxEnv, encoding: "utf8", timeout: 10_000 });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout;
  };
  const start = (boxEnv, ...args) => {
    const child = spawn(process.execPath, [script, ...args], { env: boxEnv, stdio: ["ignore", "ignore", "inherit"] });
    atEnd(() => new Promise(resolve => {
      if (child.exitCode !== null || child.signalCode !== null) return resolve();
      child.once("exit", resolve);
      child.kill("SIGTERM");
    }));
    return child;
  };
  const waitFor = async (description, find, timeout = 30_000) => {
    const until = Date.now() + timeout;
    do {
      const value = await find();
      if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 100));
    } while (Date.now() < until);
    assert.fail(description);
  };
  const inBox = (box, file = "link.json") => read(path.join(box, "run", file));
  const localServer = async (box, boxEnv) => {
    write(path.join(box, "run", "serve.needed"), "");
    start(boxEnv, "serve", "--kept");
    const record = await waitFor("the installed server starts", () => {
      try { const current = inBox(box, "serve.json"); return alive(current.pid) && current; } catch (error) { if (error.code !== "ENOENT") throw error; }
    });
    return async (method, route, body) => {
      const answer = await fetch(`http://127.0.0.1:${record.port}/${route}`, {
        method, signal: AbortSignal.timeout(20_000),
        headers: { authorization: `Basic ${Buffer.from(`opencode:${record.password}`).toString("base64")}`, "x-kuvo-from": "mac" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      return { status: answer.status, body: await answer.json() };
    };
  };

  assert.equal(fs.existsSync(path.join(primaryBox, "run", "link.json")), false);
  run(env, "setup-local");
  const first = inBox(primaryBox);
  const primary = first.setup;
  assert.equal(primary.role, "primary");
  assert.equal(primary.epoch, 1);
  assert.equal(primary.primary, k(link.publicOf(Buffer.from(first.key, "base64url"))));
  assert.equal(primary.sign, link.ownSign(first));
  assert.equal(first.relay, relay.url);
  run(env, "setup-local");
  assert.deepEqual(inBox(primaryBox), first, "a fresh primary keeps its identity on restart");
  const primaryApp = await localServer(primaryBox, env);
  const committed = async (answer, kind, checkReceipt) => {
    if (answer.status === 202) {
      const { setup, operation, proof, epoch, status, pending } = answer.body;
      assert.equal(status, "pending", JSON.stringify(answer.body));
      assert.equal(pending, true);
      const signed = link.readDoc(proof);
      assert.equal(signed?.body.operation, operation);
      assert.equal(signed.body.kind, kind);
      const outcome = await waitFor(`${kind} save ${operation} settles`, () => inBox(primaryBox).sharedOutcomes?.[operation]);
      assert.equal(outcome.status, "committed");
      const finished = await checkReceipt(answer.body);
      assert.equal(finished.status, 200, JSON.stringify(finished.body));
      assert.deepEqual(finished.body, { setup, operation, proof, epoch, status: "committed" });
      return;
    }
    assert.equal(answer.status, 200, JSON.stringify(answer.body));
  };
  const remove = async (key, body, wipe = false) => {
    const method = wipe ? "POST" : "DELETE";
    const route = `kuvo/computers/${key}${wipe ? "/unlink-and-wipe" : ""}`;
    await committed(await primaryApp(method, route, body), "membershipAndAccess", async save => {
      // Reusing the operation also finishes cleanup of the saved SSH destination.
      const { status, body: finished } = await primaryApp(method, route, { ...body, operation: save.operation });
      assert.equal(status, 200, JSON.stringify(finished));
      const { removal, ...receipt } = finished;
      assert.equal(removal.operation, save.operation);
      assert.equal(removal.accessRemoved, true);
      return { status, body: receipt };
    });
    assert.equal(inBox(primaryBox).pendingMachineCleanup?.[key], undefined);
  };
  assert.equal((await primaryApp("GET", "kuvo/computers/add")).body.can, true);

  const begun = await primaryApp("POST", "kuvo/computers/add", { destination: "primary-member", credentials: await checkedCredentials(primaryApp) });
  assert.equal(begun.status, 202, JSON.stringify(begun.body));
  const route = `kuvo/computers/add/${begun.body.try.id}`;
  const tryNow = async () => {
    const listed = await primaryApp("GET", "kuvo/computers/add");
    assert.equal(listed.status, 200, JSON.stringify(listed.body));
    const attempt = listed.body.tries.find(each => each.id === begun.body.try.id);
    assert.ok(attempt);
    return attempt;
  };
  const question = await waitFor("SSH asks to trust the new host key", async () => {
    const attempt = await tryNow();
    assert.notEqual(attempt.state, "failed", JSON.stringify({ attempt, ssh: sshLog() }));
    return attempt.question;
  });
  assert.equal(question.id, "hostKey");
  assert.equal((await primaryApp("POST", `${route}/answer`, { answer: "trust", at: question.at })).status, 200);
  const done = await waitFor("the SSH add finishes", async () => {
    const attempt = await tryNow();
    assert.notEqual(attempt.state, "failed", JSON.stringify({ attempt, ssh: sshLog() }));
    return attempt.state === "done" && attempt;
  }, 60_000);
  const member = done.computer.key;
  assert.equal(done.save.status, "committed");
  assert.ok(inBox(primaryBox).shared.membershipAndAccess.value.boxes.some(peer => peer.key === member));
  const memberEnv = { ...env, HOME: memberHome, KUVO_HOME: memberBox };
  const leadership = link.readDoc(inBox(primaryBox).leadership[primary.epoch]);
  assert.equal(leadership.origin, primary.sign);
  assert.deepEqual(inBox(memberBox).setup, { ...primary, role: "member", leadershipHash: leadership.h }, "SSH installed the signed setup");
  const memberApp = await localServer(memberBox, memberEnv);
  assert.deepEqual((await primaryApp("GET", "kuvo/setup")).body.computers.map(each => each.key), [primary.primary, member]);

  fs.copyFileSync(path.join(home, "run", "engine.json"), path.join(primaryBox, "run", "engine.json"));
  const peers = await waitFor("the member installs the signed roster including its own entry", () => {
    const doc = link.readDoc(inBox(memberBox).docs[`${primary.sign} peers`]);
    return doc?.body.boxes.some(each => each.key === member) && doc;
  });
  assert.equal(peers.origin, primary.sign);
  assert.deepEqual(peers.body.setup, { id: primary.id, epoch: 1, primary: primary.primary });
  assert.deepEqual(peers.body.boxes.map(each => [each.key, each.kind]), [[primary.primary, "machine"], [member, "machine"]]);
  const forged = link.signDoc(link.newSigningSeed(), "peers", peers.n + 1, peers.body);
  assert.equal(link.takeDoc(inBox(memberBox), forged), null, "membership is trusted by key, not by the claimed primary");
  const offered = await waitFor("the member forwards the Add list to the primary", async () => {
    const answer = await memberApp("GET", "kuvo/computers/add");
    return answer.status === 200 && answer.body.can && answer.body;
  });
  assert.deepEqual(offered, (await primaryApp("GET", "kuvo/computers/add")).body);

  const desktop = { key: k(link.publicOf(link.newKey())), name: "New desktop", kind: "desktop" };
  await waitFor("the primary records its relay lease", () => {
    try { return inBox(primaryBox, "setup-relay-lease.json").holder === primary.primary; } catch (error) { if (error.code !== "ENOENT") throw error; }
  });
  const paired = await primaryApp("POST", "kuvo/devices/pair", desktop);
  assert.equal(paired.status, 200, JSON.stringify(paired.body));
  assert.ok(paired.body.pairing);
  await waitFor("the member takes the primary's signed pairing", () => {
    const state = inBox(memberBox);
    const phones = link.readDoc(state.docs[`${primary.sign} phones`]);
    return phones?.origin === primary.sign && phones.body.phones.some(each => each.key === desktop.key && each.pairing === paired.body.pairing)
      && state.phones.some(each => each.key === desktop.key && each.pairing === paired.body.pairing);
  });

  await t.test("a member forwards pairing to the primary", async () => {
    const device = { ...desktop, key: k(link.publicOf(link.newKey())), name: "Member desktop" };
    const made = await waitFor("the member can forward pairing once it hears from the primary", async () => {
      const answer = await memberApp("POST", "kuvo/devices/pair", device);
      assert.ok(answer.status === 200 || answer.status === 503, JSON.stringify(answer));
      return answer.status === 200 && answer.body;
    });
    assert.ok(made.pairing);
    const grant = inBox(primaryBox).phones.find(each => each.key === device.key);
    assert.equal(grant?.pairing, made.pairing);
    assert.equal(grant.by, undefined);
  });

  await t.test("a Mac member's desktop attaches to its existing box as a window", { skip: process.platform !== "darwin" }, async () => {
    const decision = JSON.parse(run({ ...memberEnv, KUVO_HOME: path.join(memberHome, "app", "box") }, "here", ".kuvo-code"));
    assert.equal(decision.use, "attach");
    assert.equal(decision.opens, "window");
    assert.equal(decision.box.primary.key, primary.primary);
    assert.equal((await memberApp("POST", "kuvo/setup/attached")).status, 200);
    assert.equal(inBox(memberBox, "setup-card.json").app, true);
    assert.equal((await memberApp("GET", "kuvo/setup")).body.you.role, "member");
  });
  const addOn = async (ask, destination, replace) => {
    const begun = await ask("POST", "kuvo/computers/add", { destination, ...(replace ? { replace } : {}), credentials: await checkedCredentials(ask) });
    assert.equal(begun.status, 202, JSON.stringify(begun.body));
    const id = begun.body.try.id;
    const current = async () => (await ask("GET", "kuvo/computers/add")).body.tries.find(attempt => attempt.id === id);
    const checked = await waitFor("SSH checks the host key", async () => {
      const attempt = await current();
      return (attempt.question || attempt.state !== "working" || attempt.steps[0].state === "done") && attempt;
    });
    if (checked.question) {
      assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: checked.question.at })).status, 200);
    }
    return current;
  };
  await t.test("unlink revokes both old keys and reinstall restores the computer’s list position", async () => {
    const firstID = "aaaaaaaa-abcd-4444-8888-aaaaaaaaaaaa";
    const otherID = "bbbbbbbb-abcd-4444-8888-bbbbbbbbbbbb";
    write(path.join(homes.rejoin, "hardware-id"), firstID);
    write(path.join(homes["rejoin-clone-name"], "hardware-id"), otherID);
    const add = async (name, replace, failure) => {
      const current = await addOn(primaryApp, name, replace);
      let last;
      const ended = await waitFor(`Add ${name} finishes`, async () => {
        last = await current();
        return (last.state === "done" || last.state === "failed") && last;
      }, 120_000).catch(error => assert.fail(`${error.message}: ${JSON.stringify(last)}`));
      if (failure) {
        assert.equal(ended.state, "failed", JSON.stringify(ended));
        assert.match(ended.failure.message, failure);
      } else assert.equal(ended.state, "done", JSON.stringify(ended.failure));
      return ended.computer;
    };
    const target = path.join(homes.rejoin, ".kuvo-code");
    const first = await add("rejoin");
    const neighbor = await add("rejoin-neighbor");
    const old = inBox(target);
    const oldPeer = { key: first.key, address: k(link.addressOf(Buffer.from(old.secret, "base64url"))), sign: link.ownSign(old) };
    const membership = box => inBox(box).shared.membershipAndAccess.value;
    const keys = async () => (await primaryApp("GET", "kuvo/setup")).body.computers.map(each => each.key);
    const before = await keys();
    assert.deepEqual(before.slice(-2), [first.key, neighbor.key]);
    const signed = link.signDoc(old.sign, "personalities", 50, { personalities: [], removed: [] });
    const targets = [primaryBox, memberBox].map(box => {
      const current = inBox(box);
      const to = { address: k(link.addressOf(Buffer.from(current.secret, "base64url"))), key: k(link.publicOf(Buffer.from(current.key, "base64url"))) };
      const frame = link.mailFor(to, { docs: { give: [signed] } }, old);
      const { body } = link.readFrame(frame.subarray(17));
      assert.equal(link.openMail(body, current)?.sender.key, first.key);
      return { box, body };
    });
    await remove(first.key, { wipe: false });
    for (const { box, body } of targets) {
      assert.ok(membership(box).left.includes(first.key));
      const current = inBox(box);
      assert.equal(link.openMail(body, current), null, "old Noise key");
      assert.equal(link.takeDoc(current, signed), null, "old document signer");
    }

    await waitFor("the removed member finishes unlinking", () => inBox(target).setup?.role === "primary");
    assert.equal(fs.existsSync(path.join(target, "run", ".ssh-fresh-add")), false);
    const stale = path.join(fake, "stale-join-rejoin");
    try {
      write(stale, JSON.stringify(oldPeer));
      await add("rejoin", undefined, /presented membership keys that cannot be used/);
      write(stale, JSON.stringify({ ...oldPeer, key: k(link.publicOf(link.newKey())) }));
      await add("rejoin", undefined, /presented membership keys that cannot be used/);
      assert.deepEqual((await keys()).slice(-1), [neighbor.key]);
    } finally { fs.rmSync(stale, { force: true }); }
    const returned = await add("rejoin");
    const rejoined = inBox(target);
    assert.notEqual(returned.key, first.key);
    assert.notEqual(link.ownSign(rejoined), oldPeer.sign);
    assert.notEqual(rejoined.secret, old.secret);
    assert.equal(inBox(target).setup.id, primary.id);
    assert.deepEqual((await keys()).slice(-2), [returned.key, neighbor.key]);

    // Put the same host name in front of genuinely different hardware.
    write(path.join(fake, "home-rejoin"), homes["rejoin-clone-name"]);
    const distinct = await add("rejoin");
    assert.equal(distinct.name, returned.name);
    assert.notEqual(distinct.key, returned.key);
    assert.deepEqual((await keys()).slice(-3), [returned.key, neighbor.key, distinct.key]);

    // Lose the live completion mail. The next SSH Add can recover the signed receipt
    // from outside the deleted installation and still replace both revoked keys.
    const returnedKey = Buffer.from(returned.key, "base64url");
    const primaryAddress = link.addressOf(Buffer.from(inBox(primaryBox).secret, "base64url"));
    const prefix = Buffer.from("kuvo-removal-v1\0");
    const operation = crypto.randomBytes(16).toString("base64url");
    const releaseDone = relay.intercept(null, primaryAddress, ({ payload }) => {
      const frame = link.readFrame(payload);
      if (frame?.kind !== link.Frame.mail || !frame.body.subarray(0, prefix.length).equals(prefix)) return false;
      const body = frame.body.subarray(prefix.length);
      if (!body.subarray(0, returnedKey.length).equals(returnedKey)) return false;
      const message = link.openOneWay(body.subarray(returnedKey.length), Buffer.from(inBox(primaryBox).key, "base64url"), returnedKey);
      return JSON.parse(message.payload).done === operation;
    });
    let installed;
    try {
      await remove(returned.key, { operation }, true);
      const proof = path.join(homes.rejoin, ".kuvo-code-removal-receipts", operation + ".ack");
      await waitFor("the old installation is erased and its receipt is recoverable", () => !fs.existsSync(target) && fs.existsSync(proof));
      assert.equal(membership(primaryBox).removals[returned.key].delivered, undefined);
      write(path.join(fake, "home-rejoin-installed"), homes.rejoin);
      installed = await add("rejoin-installed");
      assert.equal(membership(primaryBox).removals[returned.key].delivered, true);
    } finally { releaseDone(); }
    assert.notEqual(installed.key, returned.key);
    assert.equal(installed.name, "rejoin-installed");
    assert.deepEqual((await keys()).slice(-3), [installed.key, neighbor.key, distinct.key]);
    const kept = membership(primaryBox);
    assert.ok(kept.left.includes(first.key) && kept.left.includes(returned.key));
    assert.ok(kept.computerHistory.slots.some(slot => slot.key === installed.key && slot.previous.join() === [first.key, returned.key].join()));
    assert.equal(JSON.stringify(kept).includes(firstID.replaceAll("-", "")), false);
    assert.deepEqual(membership(memberBox).computerHistory, kept.computerHistory);

    // Neither installation has a usable platform ID. A deliberate choice still
    // keeps the selected computer's place and records its previous membership.
    const chosen = await add("rejoin-neighbor-return", neighbor.key);
    assert.notEqual(chosen.key, neighbor.key);
    assert.deepEqual((await keys()).slice(-3), [installed.key, chosen.key, distinct.key]);
    assert.ok(membership(primaryBox).computerHistory.slots.some(slot => slot.key === chosen.key && slot.previous.includes(neighbor.key)));
    for (const { key } of [installed, chosen, distinct]) {
      await remove(key);
    }

    if (process.platform === "linux") {
      const image = "cccccccc-abcd-4444-8888-cccccccccccc";
      write(path.join(homes["rejoin-image"], "os-image-id"), image);
      write(path.join(homes["rejoin-image-copy"], "os-image-id"), image);
      const original = await add("rejoin-image");
      await add("rejoin-image-copy", undefined, /Confirm which computer to replace/);
      assert.ok((await keys()).includes(original.key));
      const replacement = await add("rejoin-image-copy", original.key);
      assert.notEqual(replacement.key, original.key);
      assert.equal(membership(primaryBox).computerHistory.slots.find(slot => slot.key === replacement.key)?.fingerprintSource, "os-id");
      assert.ok(membership(primaryBox).left.includes(original.key));
      await remove(replacement.key);
    }
  });


});
