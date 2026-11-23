import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { atEnd } from "./at-end.js";
import { test } from "node:test";
import {
  added, alive, ask, base, checkedCredentials, command, crontabFile, engineBinary, engineVersion,
  fake, handle, home, homes, hostKey, k, keyOne, keyTwo, link, mac, over, pidsThere, read,
  release, remote, secrets, sha256, sshLog, steps, there, until, write,
} from "./computers-add-fixture.js";

test("failure bench: addressed SSH cannot start, answer, or cancel an Add in its transient process", async () => {
  const actor = { key: link.publicOf(link.newKey()), kind: "desktop", name: "Paired desktop", pairing: "fixture" };
  for (const [method, route, body] of [
    ["POST", "kuvo/computers/add", { destination: "--invalid" }],
    ["POST", "kuvo/computers/add/add_fixture/answer", { answer: "trust", at: 1 }],
    ["DELETE", "kuvo/computers/add/add_fixture"],
  ]) {
    const parts = [];
    for await (const part of handle({ method, path: route, headers: {}, addressedSsh: true,
      ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, actor)) parts.push(part);
    assert.equal(parts[0].head.status, 409);
    const result = JSON.parse(Buffer.concat(parts.filter(part => part.body).map(part => part.body)));
    assert.match(result.message, /a single request can’t stay to do it/);
  }
});

test("a Darwin fixture keeper with an empty environment reaches only its fake launchctl", { skip: process.platform !== "darwin" }, () => {
  const account = path.join(base, "keeper-scope");
  const box = path.join(account, ".kuvo-code");
  const keeper = path.join(box, "bin", "kuvo-start");
  const source = fs.readFileSync(path.join(release, "bin", "kuvo-start"), "utf8");
  assert.doesNotMatch(source, /\/bin\/launchctl\b/);
  write(keeper, source, 0o755);
  write(path.join(box, "run", "machine-helper"), "");
  const result = spawnSync("/bin/sh", [keeper, "box"], {
    env: { HOME: account, PATH: "/usr/bin:/bin" }, encoding: "utf8", timeout: 5000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 1, result.stderr);
  assert.equal(fs.readFileSync(path.join(account, "launchctl.log"), "utf8"),
    `kickstart gui/${process.getuid()}/run.kuvo.code.machine\nbootstrap gui/${process.getuid()} ${account}/Library/LaunchAgents/run.kuvo.code.machine.plist\n`);
});

test("a box says whether it adds computers, and suggests the hosts in ~/.ssh/config that aren't patterns", async () => {
  const said = await ask("GET", "kuvo/computers/add");
  assert.equal(said.status, 200);
  assert.deepEqual(said.body, { can: true, hosts: ["studio", "gpu", "github.com"], tries: [] });
  // An address ssh would read as something else never reaches it.
  for (const destination of ["-oProxyCommand=sh", "host:22", "a b", "ssh://x/y", "", "me@", "this-mac.localhost"]) {
    const refused = await ask("POST", "kuvo/computers/add", { destination });
    assert.equal(refused.status, 400, destination);
    assert.ok(refused.body.message.length > 10);
  }
  assert.deepEqual(sshLog(), [], "nothing reached ssh");
});

test("adding a computer: the box shows a host key it doesn't know, and on Trust copies itself there, starts it and signs it in", async () => {
  const begun = await ask("POST", "kuvo/computers/add", { destination: "studio", credentials: await checkedCredentials() });
  assert.equal(begun.status, 202);
  const id = begun.body.try.id;
  assert.equal(begun.body.try.title, "Adding studio…");

  // The host isn't known: read without signing in, and asked about, with its fingerprint.
  const asking = await until(id, found => found?.state === "asking");
  assert.equal(asking.question.id, "hostKey");
  assert.equal(asking.question.title, "Is this studio?");
  assert.ok(Number.isInteger(asking.question.at));
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: asking.question.at + 1 })).status, 409, "an answer meant for another ask isn't taken");
  assert.deepEqual(asking.question.facts, [{ label: "Address", value: "studio" }, { label: "Fingerprint (ED25519)", value: link.fingerprintOf(keyOne), code: true }]);
  assert.deepEqual(asking.question.answers.map(each => each.id), ["cancel", "trust"]);
  assert.equal(fs.existsSync(path.join(remote, "bin", "kuvo-link.js")), false, "nothing put there before you answer");
  assert.equal(fs.existsSync(link.KNOWN_HOSTS), false, "and its key not kept yet");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "sure", at: asking.question.at })).status, 409, "only one of its answers");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust" })).status, 409, "nor one that doesn't say which ask it answers");
  assert.equal(fs.existsSync(link.KNOWN_HOSTS), false, "nothing trusted by any of those");

  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: asking.question.at })).status, 200);
  const done = await over(id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.deepEqual(steps(done), ["checking:done", "files:done", "engine:done", "starting:done", "seeding:done", "joining:done"]);
  assert.equal(done.title, "studio is one of your computers");

  // Its key, in the box's own file; yours untouched.
  assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), `studio ${hostKey(keyOne)}\n`);
  assert.equal((fs.statSync(link.KNOWN_HOSTS).mode & 0o777).toString(8), "600");
  assert.equal(fs.existsSync(path.join(base, "here", ".ssh", "known_hosts")), false);

  // This box's own files there, each as it is here, with its mode; and the engine.
  for (const [name, mode] of link.RELEASE_FILES) {
    assert.equal(sha256(fs.readFileSync(path.join(remote, name))), sha256(fs.readFileSync(path.join(home, name))), name);
    assert.equal((fs.statSync(path.join(remote, name)).mode & 0o777).toString(8), mode.toString(8), name);
  }
  assert.equal(fs.readFileSync(path.join(remote, "engine", engineVersion, "opencode"), "utf8"), engineBinary);
  assert.deepEqual(fs.readdirSync(path.join(remote, "tmp")).filter(name => name.startsWith(".kuvo-files")), [], "nothing left of the copy");

  // Started by its own keeper: an engine with a port and password of its own, which never left it.
  const noted = read(path.join(remote, "run", "engine.json"));
  assert.ok(alive(noted.pid) && command(noted.pid).includes(path.join(remote, "engine", engineVersion, "opencode")), "its engine runs, from its own folder");
  assert.ok(Number.isInteger(noted.port) && noted.password);
  assert.equal(noted.notes.macName, "Studio");
  assert.equal(noted.notes.box, "1.0.0");
  const env = fs.readFileSync(path.join(remote, "run", "launch.env"), "utf8");
  assert.match(env, /^KUVO_MACHINE_NAME=studio$/m);
  assert.match(env, /^PATH=/m);
  assert.doesNotMatch(env, /PASSWORD|KUVO_KEY/, "no secret in how it starts again");
  assert.ok(alive(Number(fs.readFileSync(path.join(remote, "run", "box.pid"), "utf8"))), "its keeper keeps it from here on");
  // What it starts with, once: this box's settings and its integration's key, in its own box; and
  // its accounts' key and the sign-in that stays the same, in its own engine. Never the sign-in
  // that renews itself, an integration's own sign-in, a bot's key, or what only this Mac can use.
  assert.equal(done.steps.find(step => step.id === "seeding").detail, "Copied 1 key and 2 accounts. One sign-in renews itself, so sign in to it on studio.");
  const settings = read(path.join(remote, "box", "opencode.json"));
  assert.equal(settings.model, "anthropic/claude-opus-5-5");
  assert.deepEqual(settings.mcp.notion.environment, { NOTION_TOKEN: "{file:~/.kuvo-code/box/keys/KUVO_KEY_NOTION_TOKEN}" });
  assert.deepEqual([settings.shell, settings.$schema, settings.mcp["chrome-devtools"]], [undefined, undefined, undefined]);
  assert.deepEqual(fs.readdirSync(path.join(remote, "box", "keys")), ["KUVO_KEY_NOTION_TOKEN"]);
  assert.equal(fs.readFileSync(path.join(remote, "box", "keys", "KUVO_KEY_NOTION_TOKEN"), "utf8"), "secret-notion-token");
  assert.equal((fs.statSync(path.join(remote, "box", "keys", "KUVO_KEY_NOTION_TOKEN")).mode & 0o777).toString(8), "600");
  const accounts = read(path.join(there, "engine-credentials.json"));
  assert.deepEqual(accounts.map(each => [each.integrationID, each.label, each.value, each.active]), [
    ["anthropic", "Work", { type: "api", key: "sk-ant-secret-key" }, true],
    ["github-copilot", "me", { type: "oauth", refresh: "gho_stays_the_same", access: "gho_stays_the_same", expires: 0 }, true],
  ]);
  // None of it is written anywhere but there: not in what's said of the try, how it starts again, or its logs.
  const said = [fs.readFileSync(path.join(home, "run", "adding.json"), "utf8"), JSON.stringify(done), env, fs.readFileSync(path.join(fake, "log"), "utf8"),
    ...["launch.notes", "engine.log", "link.log"].map(name => { try { return fs.readFileSync(path.join(remote, "run", name), "utf8"); } catch { return ""; } })].join("\n");
  for (const secret of secrets) assert.equal(said.includes(secret), false, secret);
  // And after a restart.
  if (process.platform !== "darwin") assert.match(fs.readFileSync(crontabFile, "utf8"), /^@reboot sh "\$HOME\/\.kuvo-code\/bin\/kuvo-start" >\/dev\/null 2>&1 # kuvo-code: starts its engine again after a restart$/m);
  if (process.platform === "darwin") {
    const plist = fs.readFileSync(path.join(there, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"), "utf8");
    assert.match(plist, /Kuvo Helper<\/string><string>keep-machine<\/string>/);
    assert.doesNotMatch(plist, /\/bin\/sh/);
    assert.match(fs.readFileSync(path.join(fake, "reached"), "utf8"), /codesign .*identifier "run\.kuvo\.code\.box".*subject\.OU.*C8Y523R7T5/);
    assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/);
    const [keeper, helper] = fs.readFileSync(path.join(remote, "run", "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    assert.equal(keeper, Number(fs.readFileSync(path.join(remote, "run", "box.pid"), "utf8")));
    assert.match(command(helper), /Kuvo Helper keep-machine/);
  }

  // Signed in: it knows this box as its Mac, and this box names it among your computers.
  const theirs = read(path.join(remote, "run", "link.json"));
  const own = k(link.publicOf(mac));
  assert.equal(theirs.mac, own);
  assert.equal(theirs.relay, "ws://127.0.0.1:1");
  assert.equal(theirs.id, "studio");
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  assert.equal(peer.name, "studio");
  assert.deepEqual(done.computer, { key: peer.key, name: "studio" });
  assert.deepEqual(read(path.join(home, "machines.json")).map(each => each.destination), ["studio"]);
  assert.equal(read(path.join(home, "machines.json"))[0].identity, fs.readFileSync(path.join(remote, "machine-id"), "utf8"));
  assert.equal(read(path.join(home, "phone-link.json")).machines.studio.key, peer.key);

  // Every client's list has it, with what this box can do with it over SSH; it's suggested no more.
  const listed = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  assert.deepEqual(listed.actions.map(action => action.id), ["rename", "remove", "setup", "uninstall"]);
  assert.match(listed.actions.find(action => action.id === "uninstall").confirm.message, /Studio connects to studio over SSH/);
  assert.deepEqual((await ask("GET", "kuvo/computers/add")).body.hosts, ["gpu", "github.com"]);
  // Every connection, whatever your config says: no host key written to your own known hosts, no X11.
  const options = fs.readFileSync(path.join(fake, "options"), "utf8").split("\n");
  const count = option => options.filter(each => each === option).length;
  assert.ok(count("BatchMode=yes") > 5);
  assert.deepEqual([count("UpdateHostKeys=no"), count("ForwardX11=no"), count("ForwardAgent=no")], [count("BatchMode=yes"), count("BatchMode=yes"), count("BatchMode=yes")]);
  // One connection shared by its scripts, and let go of at the end.
  // (Where a socket's path fits: a Mac's short folder in /tmp; on Linux the box's own `run/ssh`,
  // which a test's deep home can be too long for, and then each script connects by itself.)
  const shares = process.platform === "darwin" || Buffer.byteLength(path.join(home, "run", "ssh")) <= 55;
  assert.equal(sshLog().at(-1), shares ? "studio  exit" : "studio yes run");
});

test("adding a Windows PC with only WSL 1 tells you how to get WSL 2 before copying Kuvo Code", async t => {
  const destination = "windows-no-wsl2";
  const folder = path.join(base, destination);
  fs.mkdirSync(folder);
  write(path.join(fake, `home-${destination}`), folder);
  write(path.join(fake, `windows-${destination}`), "");
  t.after(() => fs.rmSync(path.join(fake, `windows-${destination}`)));
  const begun = await ask("POST", "kuvo/computers/add", { destination });
  assert.equal(begun.status, 202);
  const id = begun.body.try.id;
  const asking = await until(id, found => found?.state === "asking");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { at: asking.question.at, answer: "trust" })).status, 200);
  const failed = await over(id);
  assert.equal(failed.failure?.message, "Install Ubuntu with WSL 2 on desktop-test, then open Ubuntu once and try again.");
  assert.deepEqual(fs.readdirSync(folder), []);
});

// The keeper converts the fresh install to versioned files independently of SSH Add.
async function waitForVersionedFiles() {
  const run = path.join(remote, "run");
  const ready = () => fs.lstatSync(path.join(remote, "bin"), { throwIfNoEntry: false })?.isSymbolicLink() &&
    !fs.existsSync(path.join(run, ".updating"));
  if (ready()) return;
  const watchers = [];
  let deadline;
  try {
    await new Promise((resolve, reject) => {
      const check = () => { if (ready()) resolve(); };
      for (const folder of [remote, run]) watchers.push(fs.watch(folder, check).on("error", reject));
      deadline = setTimeout(() => reject(new Error("Studio did not finish converting its installed files")), 45_000);
      check();
    });
  } finally {
    for (const watcher of watchers) watcher.close();
    clearTimeout(deadline);
  }
}

test("the same computer under another address is the one already here, not a second one", async () => {
  await waitForVersionedFiles();
  fs.appendFileSync(link.KNOWN_HOSTS, `studio.lan ${hostKey(keyOne)}\n`);
  const begun = await ask("POST", "kuvo/computers/add", { destination: "studio.lan" });
  const done = await over(begun.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.known, true);
  assert.equal(done.title, "studio.lan is studio, already one of your computers");
  assert.equal(done.computer.name, "studio");
  assert.deepEqual(read(path.join(home, "machines.json")).map(each => [each.destination, each.otherAddresses]), [["studio", ["studio.lan"]]]);
  assert.equal(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine").length, 1);
});

test("a host that can't be reached, one that refuses your keys, and one whose key changed are each said in words, and nothing is trusted", async () => {
  const fails = async destination => over((await ask("POST", "kuvo/computers/add", { destination })).body.try.id);
  const nowhere = await fails("nowhere");
  assert.equal(nowhere.state, "failed");
  assert.deepEqual(steps(nowhere), ["checking:failed", "files:waiting", "engine:waiting", "starting:waiting", "seeding:waiting", "joining:waiting"]);
  // (In words, with what to do next: what ssh said is in the log alone.)
  assert.equal(nowhere.failure.message, "Couldn’t reach nowhere. Make sure it’s on and connected, then Try Again.");
  assert.equal(nowhere.failure.again, true);

  // A key in your own known hosts that isn't the host's now: never asked about, never replaced.
  fs.writeFileSync(path.join(base, "here", ".ssh", "known_hosts"), `gpu ${hostKey(keyTwo)}\n`);
  const changed = await fails("gpu");
  assert.equal(changed.failure.title, "gpu’s host key changed");
  assert.match(changed.failure.message, /remove the old key in a terminal on Studio with ssh-keygen -R 'gpu', then try again\. If not, someone may be intercepting the connection\./);
  assert.doesNotMatch(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), /^gpu /m);
  fs.rmSync(path.join(base, "here", ".ssh", "known_hosts"));

  // Refused: its key was trusted (you said so), and your keys weren't taken. The vault has none.
  fs.writeFileSync(path.join(fake, "refuse"), "");
  try {
    const begun = await ask("POST", "kuvo/computers/add", { destination: "gpu" });
    const asking = await until(begun.body.try.id, found => found?.state === "asking");
    assert.equal(asking.question.id, "hostKey");
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "trust", at: asking.question.at });
    const refused = await over(begun.body.try.id);
    assert.equal(refused.failure.title, "gpu refused the connection");
    assert.match(refused.failure.message, /didn’t accept Studio’s SSH keys/);
  } finally {
    fs.rmSync(path.join(fake, "refuse"));
  }
  // What failed stays to be read, and is forgotten when you say.
  const kept = (await ask("GET", "kuvo/computers/add")).body.tries.map(each => each.destination);
  assert.deepEqual(kept.sort(), ["gpu", "nowhere", "studio", "studio.lan", "windows-no-wsl2"]);
  assert.equal((await ask("DELETE", `kuvo/computers/add/${nowhere.id}`)).status, 200);
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.id === nowhere.id), false);
});

test("a host key that changed from the one you trusted here is asked about again; Cancel leaves everything as it was", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const before = fs.readFileSync(link.KNOWN_HOSTS, "utf8");
  fs.writeFileSync(path.join(fake, "hostkey"), hostKey(keyTwo));
  try {
    const begun = await ask("POST", `kuvo/computers/${peer.key}/setup`);
    assert.equal(begun.status, 202);
    assert.equal(begun.body.try.title, "Setting up studio…");
    const asking = await until(begun.body.try.id, found => found?.state === "asking");
    assert.equal(asking.question.id, "hostKeyChanged");
    assert.equal(asking.question.facts[1].value, link.fingerprintOf(keyTwo));
    assert.deepEqual(asking.question.answers.map(each => [each.id, each.kind]), [["cancel", "plain"], ["trust", "danger"]]);
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "cancel", at: asking.question.at });
    assert.equal(await over(begun.body.try.id), null, "stopped, and gone from the list");
    assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), before);
    // Trust New Key: the new key takes the old one's line (its other address's stays), and it goes on.
    const again = await ask("POST", `kuvo/computers/${peer.key}/setup`);
    const asked = await until(again.body.try.id, found => found?.state === "asking");
    await ask("POST", `kuvo/computers/add/${again.body.try.id}/answer`, { answer: "trust", at: asked.question.at });
    const done = await over(again.body.try.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.deepEqual(fs.readFileSync(link.KNOWN_HOSTS, "utf8").trim().split("\n").filter(line => line.startsWith("studio")).sort(), [`studio ${hostKey(keyTwo)}`, `studio.lan ${hostKey(keyOne)}`]);
  } finally {
    fs.writeFileSync(link.KNOWN_HOSTS, before);
    fs.writeFileSync(path.join(fake, "hostkey"), hostKey(keyOne));
  }
});

test("setting a computer up again finds its files in place and leaves its running engine alone", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  const begun = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  const done = await over(begun.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.deepEqual(done.steps.map(step => [step.id, step.detail]), [["checking", "Connecting to studio"], ["files", "It has them already"], ["engine", "It has it already"], ["starting", undefined], ["seeding", "It keeps its own"], ["joining", undefined]]);
  assert.equal(read(path.join(remote, "run", "engine.json")).pid, engine, "the engine at work wasn't started over");
  // What's there is its own from then on: nothing of this box's is given again.
  assert.equal(read(path.join(there, "engine-credentials.json")).length, 2);
  assert.equal(link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine").key, peer.key, "the same computer, once");
});

test("a machine whose Kuvo is older is brought up to date by this box whatever is at work there, its running engine left alone; Try Again only once it couldn't be reached for days", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const versionFile = path.join(remote, "bin", "box-version");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  // As new as this box: nothing is said, nothing offered, nothing begun.
  await link.keepUpRound();
  assert.equal((await listed()).update, undefined);
  assert.equal(link.allTries().some(each => each.kind === "update"), false);
  // Older (as a beta from before boxes took releases from a client), with two sessions at work.
  fs.writeFileSync(versionFile, "0.9.0\n");
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  // Not reached over SSH (your keys refused, say): no failed update, and tried again next round.
  fs.writeFileSync(path.join(fake, "refuse"), "");
  try {
    await link.keepUpRound();
  } finally {
    fs.rmSync(path.join(fake, "refuse"), { force: true });
  }
  // (It goes on trying by itself: nothing to press yet.)
  const missed = await listed();
  assert.deepEqual([missed.update.state, missed.update.title], ["waiting", "Kuvo on studio updates once Studio can reach it"]);
  assert.equal(missed.actions.some(each => each.id === "update"), false);
  assert.equal(read(link.KEEP_UP_FILE)[peer.key].failed, undefined);
  // Not reached for three days: said, with Try Again.
  const kept = read(link.KEEP_UP_FILE);
  kept[peer.key].unreachedSince -= 3 * 86_400_000;
  fs.writeFileSync(link.KEEP_UP_FILE, JSON.stringify(kept));
  const days = await listed();
  assert.deepEqual([days.update.state, days.update.title], ["failed", "Couldn’t reach studio"]);
  assert.match(days.update.detail, /^studio didn’t accept Studio’s SSH keys\..* Kuvo also tries again by itself later\.$/);
  assert.deepEqual(days.actions.find(each => each.id === "update")?.label, "Try Again", "a way on, never a card alone");
  // Two sessions at work there: the round brings it up all the same (nothing there stops), its
  // running engine left alone.
  const busy = path.join(there, "busy");
  fs.writeFileSync(busy, "");
  atEnd(() => fs.rmSync(busy, { force: true }));
  await link.keepUpRound();
  const begun = link.allTries().find(each => each.kind === "update" && each.state === "working");
  assert.ok(begun, "begun with something at work there");
  assert.equal(begun.title, "Updating Kuvo on studio…");
  assert.deepEqual((await listed()).update, { state: "updating", title: "Updating Kuvo on studio…", detail: "" });
  const done = await over(begun.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.title, "studio is up to date");
  assert.equal(fs.readFileSync(versionFile, "utf8").trim(), "1.0.0");
  assert.equal(read(path.join(remote, "run", "engine.json")).pid, engine, "the engine at work wasn't started over");
  fs.rmSync(busy, { force: true });
  const after = await listed();
  assert.equal(after.update, undefined);
  assert.equal(after.actions.some(each => each.id === "update"), false);
  // Update Now on one as new already: nothing is put there again.
  const again = await ask("POST", `kuvo/computers/${peer.key}/update`);
  assert.equal(again.status, 202);
  const current = await over(again.body.try.id);
  assert.deepEqual(current.steps.map(step => [step.id, step.state, step.detail]).slice(1), ["files", "engine", "starting", "joining"].map(id => [id, "done", "It’s up to date already"]));
});

test("a machine heard from that never says its version (a box from before they said it) updates in a moment, before any look over SSH, with nothing to press; a look that finds it as new clears it", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  const told = path.join(home, "run", "peer-agents.json");
  const before = fs.existsSync(told) ? fs.readFileSync(told, "utf8") : null;
  const all = before ? JSON.parse(before) : {};
  const { computer: _computer, ...rest } = all[peer.key] ?? {};
  fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...rest, at: Date.now(), heard: Date.now() } }));
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  try {
    const it = await listed();
    assert.equal(it.reach, "connected");
    assert.equal(it.version, undefined, "it never said");
    assert.deepEqual(it.update, { state: "waiting", title: "Kuvo on studio updates in a moment", detail: "" });
    assert.equal(it.actions.some(each => each.id === "update"), false, "it's brought up by itself");
    // One this box didn't set up over SSH (its record didn't come along): it can't be brought up
    // from here until it is, which is what it offers, never nothing.
    const list = path.join(home, "machines.json");
    const saved = fs.readFileSync(list, "utf8");
    fs.writeFileSync(list, "[]");
    try {
      const apart = await listed();
      assert.deepEqual(apart.update, { state: "waiting", title: "Kuvo on studio needs an update", detail: "Studio hasn’t set it up over SSH, so it can’t update it yet. Set it up over SSH once: what runs there stays.", action: { id: "add", label: "Set Up Over SSH…", address: "studio" } });
      assert.deepEqual(apart.actions.find(each => each.id === "add"), { id: "add", label: "Set Up Over SSH…" });
    } finally {
      fs.writeFileSync(list, saved);
    }
    // Looked at over SSH: as new as this box, so nothing more is said.
    await link.keepUpRound();
    const after = await listed();
    assert.equal(after.update, undefined);
    assert.equal(after.actions.some(each => each.id === "update"), false);
  } finally {
    if (before === null) fs.rmSync(told, { force: true }); else fs.writeFileSync(told, before);
  }
});

test("a machine whose address doesn't answer (a home network's while you're away) is reached at another it tells its Mac, its key checked as the one known; with none, it's said in words with Try Again", async () => {
  await waitForVersionedFiles();
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const versionFile = path.join(remote, "bin", "box-version");
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  const told = path.join(home, "run", "peer-agents.json");
  const before = fs.existsSync(told) ? fs.readFileSync(told, "utf8") : null;
  const all = before ? JSON.parse(before) : {};
  // (None of the addresses you added it under answers: the one it was added at, and any other.)
  const own = read(path.join(home, "machines.json")).find(each => each.destination === peer.id);
  const away = [peer.id, ...(own.otherAddresses ?? [])].map(address => path.join(fake, `away-${address}`));
  fs.writeFileSync(versionFile, "0.9.0\n");
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  for (const file of away) fs.writeFileSync(file, "");
  try {
    // Nowhere else to try: in words, with Try Again, and what ssh said in the log alone.
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "0.9.0", working: 0 } } }));
    const failed = await over((await ask("POST", `kuvo/computers/${peer.key}/update`)).body.try.id);
    assert.equal(failed.state, "failed");
    assert.equal(failed.failure.message, `Couldn’t reach ${peer.name}. Make sure it’s on and connected, then Try Again.`);
    const it = await listed();
    assert.deepEqual([it.update.state, it.update.title, it.update.detail], ["failed", `Couldn’t reach ${peer.name}`, "Make sure it’s on and connected, then Try Again. Kuvo also tries again by itself later."]);
    assert.equal(it.actions.find(each => each.id === "update")?.label, "Try Again");
    // It tells its Mac its tailnet's address: Try Again reaches it there, as the user it was set
    // up as, its host key checked against the one known for its first address; nothing is asked.
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "0.9.0", working: 0, addresses: ["100.101.2.3", "bad address", peer.id] } } }));
    assert.deepEqual(link.otherAddressesOf(peer.id), [...(own.otherAddresses ?? []).map(address => ({ address, own: true })), { address: "100.101.2.3", own: false }]);
    fs.rmSync(path.join(fake, "options"), { force: true });
    const done = await over((await ask("POST", `kuvo/computers/${peer.key}/update`)).body.try.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.equal(fs.readFileSync(versionFile, "utf8").trim(), "1.0.0");
    assert.ok(sshLog().some(line => line.startsWith("100.101.2.3 yes run")), "reached at its other address, strictly checked");
    assert.ok(fs.readFileSync(path.join(fake, "options"), "utf8").split("\n").includes(`HostKeyAlias=${peer.id}`));
    // (Started again, it tells its Mac its new version: nothing more is said.)
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "1.0.0", working: 0 } } }));
    assert.equal((await listed()).update, undefined);
  } finally {
    for (const file of away) fs.rmSync(file, { force: true });
    if (before === null) fs.rmSync(told, { force: true }); else fs.writeFileSync(told, before);
  }
});

test("a machine says where else its Macs may reach it: its tailnet's address first, then its name, never a loopback or link-local one", () => {
  const interfaces = { lo0: [{ family: "IPv4", address: "127.0.0.1", internal: true }], en0: [{ family: "IPv4", address: "192.168.1.101", internal: false }, { family: "IPv6", address: "fe80::1", internal: false }], awdl0: [{ family: "IPv4", address: "169.254.3.4", internal: false }], tailscale0: [{ family: "IPv4", address: "100.88.7.6", internal: false }] };
  assert.deepEqual(link.ownAddresses(interfaces, "lokum"), ["100.88.7.6", "lokum", "192.168.1.101"]);
  assert.deepEqual(link.ownAddresses({}, "localhost"), []);
});

test("once this Mac has moved from the old app, the old app opening again stops nothing: it's told it was replaced, and machines are still kept up to date", async () => {
  const config = link.macLink.config;
  const record = path.join(home, "updates", "rehome", "state.json");
  const appConfig = { phones: true, machines: true, relay: "ws://127.0.0.1:1", name: "Studio", app: "0.7.4" };
  const quietly = async run => { const said = console.error; console.error = () => {}; try { return await run(); } finally { console.error = said; } };
  try {
    // Before a move, the old app attached keeps this box from adding machines, and says so.
    assert.equal((await ask("PUT", "kuvo/link/config", appConfig)).status, 200);
    assert.equal(link.oldAppAttached(), true);
    assert.match(link.canAdd().message ?? "", /^Quit the old Kuvo Code app on Studio first/);
    // Moved: the old app is refused with words, takes nothing over, and stops nothing.
    write(record, JSON.stringify({ state: "done", movedAt: Date.now() }));
    link.macLink.config = config;
    const refused = await ask("PUT", "kuvo/link/config", appConfig);
    assert.deepEqual([refused.status, refused.body.message], [410, "Kuvo Code 1.0 has replaced this app on Studio. Quit it and move it to the Trash: everything it did is in Kuvo Code now."]);
    assert.equal(link.macLink.config, config, "what the old app said isn't taken");
    assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [] })).status, 410, "nor its list of machines");
    assert.equal(link.oldAppAttached(), false);
    assert.equal(link.canAdd().can, true);
    await quietly(() => link.keepUpRound());
  } finally {
    fs.rmSync(path.join(home, "updates"), { recursive: true, force: true });
    link.macLink.config = config;
    link.macLink.appSaid = undefined;
  }
});

test("uninstalling while it's being set up waits for the start under way, then nothing runs there and nothing starts it again (#603)", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  // The desktop and phone have independent grants. The desktop keeps its key on this machine.
  write(path.join(remote, "desktop", "identity"), "its-device-key", 0o600);
  const appThere = k(link.publicOf(link.newKey()));
  link.updateLink(state => {
    link.pairDevice(state, { key: appThere, name: "Studio screen" });
    link.pairPhone(state, { key: k(link.publicOf(link.newKey())), name: "My iPhone", added: 1, from: "pair", pairing: "its-own" });
    return state;
  });
  const confirms = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key).actions.find(action => action.id === "uninstall").confirm.message;
  assert.match(confirms, /The sign-ins and keys it keeps are deleted\. Its sessions stay/);
  assert.doesNotMatch(confirms, /taken off your devices/);
  // A Set Up Again, held at its start there (the script that can't be cut short).
  fs.writeFileSync(path.join(fake, "hold"), 'bin/kuvo-start" start');
  const setup = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  await until(setup.body.try.id, found => found?.steps.find(step => step.id === "starting")?.state === "now");
  // Uninstall asked meanwhile: the try stops (said so), and the uninstall waits its turn.
  const listedBusy = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  assert.equal(listedBusy.actions.find(action => action.id === "setup").why, "It’s being set up now.");
  const uninstall = await ask("POST", `kuvo/computers/${peer.key}/uninstall`);
  assert.equal(uninstall.status, 202);
  const tries = (await ask("GET", "kuvo/computers/add")).body.tries;
  assert.equal(tries.find(each => each.id === setup.body.try.id).state, "stopping");
  assert.equal(tries.find(each => each.id === setup.body.try.id).title, "Stopping…");
  assert.deepEqual(steps(tries.find(each => each.id === uninstall.body.try.id)), ["checking:waiting", "removing:waiting", "forgetting:waiting"], "not begun while the start is under way");
  // Asked twice (a second click, another client): the one uninstall, and setting it up waits for it.
  assert.equal((await ask("POST", `kuvo/computers/${peer.key}/uninstall`)).body.try.id, uninstall.body.try.id);
  const meanwhile = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  assert.deepEqual([meanwhile.status, meanwhile.body.message], [409, "Kuvo Code is being uninstalled from studio now. Add it again once that’s done."]);
  const offered = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key).actions;
  assert.equal(offered.find(action => action.id === "uninstall").why, "Kuvo Code is being uninstalled from it now.");
  fs.rmSync(path.join(fake, "hold"));

  const done = await over(uninstall.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.title, "studio no longer has Kuvo Code running");
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.id === setup.body.try.id), false, "the try that was stopped is gone");

  // Nothing of it runs, and nothing starts it again.
  assert.equal(alive(engine), false, "its engine stopped");
  assert.deepEqual(pidsThere().filter(alive), [], "and its keeper, link and bridge");
  assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, "its line in the crontab is gone");
  assert.equal(fs.existsSync(path.join(there, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), false);
  assert.ok(fs.existsSync(path.join(remote, "run", "off")), "marked off, until it's added again");
  // It let this computer go, and this one took it off its list. Its sessions' folder stays.
  assert.deepEqual(read(path.join(remote, "run", "link.json")).macs ?? [], []);
  assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), []);
  assert.deepEqual(read(path.join(home, "machines.json")), []);
  assert.ok(fs.existsSync(path.join(remote, "data")));
  assert.equal((await ask("POST", `kuvo/computers/${peer.key}/uninstall`)).status, 404, "no longer one it set up");
  // Uninstall clears the local key. The primary keeps independent device grants until removed.
  assert.equal(fs.existsSync(path.join(remote, "desktop")), false, "the app's key there is deleted");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name).sort(), ["My iPhone", "Studio screen"]);
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: appThere }] })).status, 200);
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone"]);
  assert.deepEqual(link.ownPhones(link.readLink()).removed.map(each => each.key), [appThere]);
});

const nothingOfOurs = folder => {
  const kuvo = path.join(folder, ".kuvo-code");
  let keys = [];
  try { keys = fs.readdirSync(path.join(kuvo, "box", "keys")); } catch {}
  assert.deepEqual(keys, [], "no key of this box's there");
  for (const file of ["box/opencode.json", "run/engine.json", "run/link.json"]) {
    let text = "";
    try { text = fs.readFileSync(path.join(kuvo, file), "utf8"); } catch {}
    for (const secret of secrets) assert.equal(text.includes(secret), false, `${file}: ${secret}`);
  }
};

test("a Mac with earlier Kuvo Code data must update before being added and keeps its data", async () => {
  for (const [destination, file] of [["old07", "opencode/opencode.db"], ["oldkeeper", "box/run/keeper.json"]]) {
    const folder = path.join(base, destination);
    fs.mkdirSync(folder, { recursive: true });
    write(path.join(fake, `home-${destination}`), folder);
    const oldFile = path.join(folder, "Library", "Application Support", "run.kuvo.code", file);
    write(oldFile, "kept exactly");
    const before = fs.readdirSync(folder, { recursive: true }).sort();
    const done = await added(destination);
    assert.equal(done.state, "failed");
    assert.equal(done.failure.message, `${destination} still has Kuvo Code 0.7’s sessions. Update it to Kuvo first — its sessions move over then — and add it again.`);
    assert.deepEqual(fs.readdirSync(folder, { recursive: true }).sort(), [".kuvo-code-adding.lock", ...before]);
    assert.equal(fs.statSync(path.join(folder, ".kuvo-code-adding.lock")).mode & 0o777, 0o600);
    assert.equal(fs.readFileSync(oldFile, "utf8"), "kept exactly");
    assert.equal(fs.existsSync(path.join(folder, ".kuvo-code")), false);
  }
});

test("a Mac that's a machine's box already, with earlier Kuvo Code data beside it, is added and set up again as before; that data is left as it is", async () => {
  // The installer there leaves both (`two`): only a Mac whose earlier data it would move must update first.
  const folder = path.join(base, "machine07");
  fs.mkdirSync(folder, { recursive: true });
  write(path.join(fake, "home-machine07"), folder);
  for (const tool of ["pkill", "caffeinate"]) write(path.join(folder, ".kuvo-code", "bin", tool), `#!/bin/sh\nexit 0\n`, 0o755);
  write(path.join(folder, ".kuvo-code", "bin", "kuvo-start"), "#!/bin/sh\nexit 0\n", 0o755);
  write(path.join(folder, ".kuvo-code", "box", "kind"), "machine\n");
  const oldFile = path.join(folder, "Library", "Application Support", "run.kuvo.code", "opencode", "opencode.db");
  write(oldFile, "kept exactly");
  const done = await added("machine07");
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  const again = await over((await ask("POST", `kuvo/computers/${done.computer.key}/setup`)).body.try.id);
  assert.equal(again.state, "done", JSON.stringify(again.failure));
  assert.equal(fs.readFileSync(oldFile, "utf8"), "kept exactly");
  // Taken off again, and forgotten, for the tests after.
  const uninstall = await ask("POST", `kuvo/computers/${done.computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
  assert.equal((await ask("DELETE", "kuvo/hosts/machine07")).status, 200);
});

test("a Mac with an existing app folder is added without copying sign-ins or keys", async () => {
  // Only the app's folder, no box of a machine's: judged free, and added with no question.
  fs.mkdirSync(path.join(homes.macbook, "Library", "Application Support", "run.kuvo.code"), { recursive: true });
  const done = await added("macbook");
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.steps.find(step => step.id === "seeding").detail, "It keeps its own");
  nothingOfOurs(homes.macbook);
  assert.equal(fs.existsSync(path.join(homes.macbook, "engine-credentials.json")), false, "nor any account in its engine");
  assert.equal(fs.existsSync(path.join(homes.macbook, ".kuvo-code", "run", "seeded.json")), false);
  // Taken off again, for the tests after.
  const uninstall = await ask("POST", `kuvo/computers/${done.computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
});

test("a computer set up again with keys made anew keeps its independently paired app", async () => {
  // (Its host key is trusted already, from when it was added before: nothing is asked.)
  const done = await over((await ask("POST", "kuvo/computers/add", { destination: "macbook" })).body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  // Kuvo Code's app there has a device grant of its own.
  const appThere = k(link.publicOf(link.newKey()));
  link.updateLink(state => {
    link.pairDevice(state, { key: appThere, name: "MacBook screen" });
    return state;
  });
  // Its box's keys are lost there (stopped, its link's file gone), and it's set up again: the same
  // computer, at the same address, with keys of its own made anew.
  const kuvo = path.join(homes.macbook, ".kuvo-code");
  for (const file of ["box.pid", "link.pid"]) {
    let pid = 0;
    try { pid = Number(fs.readFileSync(path.join(kuvo, "run", file), "utf8")); } catch {}
    if (pid > 1 && alive(pid) && command(pid).includes(kuvo)) process.kill(pid);
  }
  fs.rmSync(path.join(kuvo, "run", "link.json"));
  const again = await ask("POST", `kuvo/computers/${done.computer.key}/setup`);
  const set = await over(again.body.try.id);
  assert.equal(set.state, "done", JSON.stringify(set.failure));
  const now = link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine");
  assert.deepEqual(now.map(each => [each.name, each.key === done.computer.key]), [["macbook", false]], "the one computer, by another key");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone", "MacBook screen"], "its app is still one of your devices");
  // Removing the computer leaves the app's grant until its own removal.
  const uninstall = await ask("POST", `kuvo/computers/${now[0].key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone", "MacBook screen"]);
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: appThere }] })).status, 200);
});

test("an add that fails, or is stopped, after Kuvo Code started on a computer that had nothing leaves nothing running there and no key behind", async () => {
  const kuvo = path.join(homes.mini, ".kuvo-code");
  const gone = why => {
    assert.deepEqual(pidsThere(kuvo).filter(alive), [], `${why}: nothing of it runs`);
    assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, `${why}: nothing starts it again`);
    assert.equal(fs.existsSync(path.join(homes.mini, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), false);
    assert.equal(fs.existsSync(path.join(kuvo, "box", "keys")), false, `${why}: the keys copied there are deleted`);
    assert.equal(fs.existsSync(path.join(kuvo, "run", "launch.env")), false);
    assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), [], `${why}: not one of your computers`);
  };
  // Its join fails (its link breaks as it's signed in): it had started, with this box's keys.
  fs.writeFileSync(path.join(fake, "fail"), 'kuvo-link.js" setup');
  let failed;
  try {
    failed = await added("mini");
  } finally {
    fs.rmSync(path.join(fake, "fail"));
  }
  assert.equal(failed.state, "failed");
  assert.deepEqual(steps(failed), ["checking:done", "files:done", "engine:done", "starting:done", "seeding:done", "joining:failed"]);
  assert.equal(failed.failure.message, "Kuvo Code on mini couldn’t join your computers. it broke Kuvo Code was stopped on mini again, and the keys and sign-ins copied there were deleted.");
  assert.equal(fs.existsSync(path.join(homes.mini, "engine-credentials.json")), true, "(it had been given its accounts)");
  gone("failed");

});

test("an add stopped while its accounts are being copied, on a computer that had nothing, is taken off there again", async () => {
  const kuvo = path.join(homes.nas, ".kuvo-code");
  fs.writeFileSync(path.join(fake, "hold"), 'kuvo-link.js" box seed');
  const seeding = await added("nas", found => found?.steps.find(step => step.id === "seeding")?.state === "now");
  assert.ok(pidsThere(kuvo).filter(alive).length > 0, "it runs there by now");
  assert.equal((await ask("DELETE", `kuvo/computers/add/${seeding.id}`)).status, 200);
  fs.rmSync(path.join(fake, "hold"));
  assert.equal(await over(seeding.id), null, "stopped, and gone from the list");
  assert.deepEqual(pidsThere(kuvo).filter(alive), [], "nothing of it runs");
  assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, "nothing starts it again");
  assert.equal(fs.existsSync(path.join(kuvo, "box", "keys")), false, "the keys copied there are deleted");
  assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), []);
});

test("a computer that had Kuvo Code before (uninstalled there, its sign-ins kept) is never seeded, and a failed add leaves what it keeps", async () => {
  // As `kuvo-link.js uninstall` leaves one: off, nothing to start it, its id, sign-ins and keys still there.
  const kuvo = path.join(homes.attic, ".kuvo-code");
  write(path.join(kuvo, "machine-id"), "0123456789abcdef0123456789abcdef", 0o600);
  write(path.join(kuvo, "run", "off"), "");
  write(path.join(kuvo, "box", "opencode.json"), JSON.stringify({ model: "its/own" }), 0o600);
  write(path.join(kuvo, "box", "keys", "KUVO_KEY_ITS_OWN"), "its-own-key", 0o600);
  write(path.join(kuvo, "agents", "codex", "auth.json"), "its-own-sign-in", 0o600);
  write(path.join(homes.attic, "engine-credentials.json"), JSON.stringify([{ id: "c", integrationID: "openai", label: "theirs", active: true, value: { type: "oauth", refresh: "its-own-refresh" } }]), 0o600);
  fs.writeFileSync(path.join(fake, "fail"), 'kuvo-link.js" setup');
  let failed;
  try {
    failed = await added("attic");
  } finally {
    fs.rmSync(path.join(fake, "fail"));
  }
  assert.equal(failed.state, "failed");
  assert.equal(failed.steps.find(step => step.id === "seeding").detail, "It keeps its own");
  assert.equal(failed.failure.message, "Kuvo Code on attic couldn’t join your computers. it broke", "nothing was deleted there, and it doesn't say so");
  // What it kept is as it was: none of it written over by this box's, none of it deleted.
  assert.deepEqual(fs.readdirSync(path.join(kuvo, "box", "keys")), ["KUVO_KEY_ITS_OWN"]);
  assert.equal(fs.readFileSync(path.join(kuvo, "box", "keys", "KUVO_KEY_ITS_OWN"), "utf8"), "its-own-key");
  assert.equal(fs.readFileSync(path.join(kuvo, "agents", "codex", "auth.json"), "utf8"), "its-own-sign-in");
  assert.equal(read(path.join(kuvo, "box", "opencode.json")).model, "its/own");
  assert.deepEqual(read(path.join(homes.attic, "engine-credentials.json")).map(each => [each.integrationID, each.label]), [["openai", "theirs"]]);
  assert.equal(fs.existsSync(path.join(kuvo, "run", "seeded.json")), false);
});

test("a phone's Remove, Uninstall and Forget (the old Mac app's `kuvo/machines/<id>` and `kuvo/hosts/<id>`) are this box's own", async () => {
  // (Its key trusted before, so nothing to ask.)
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  assert.deepEqual([added.body.machine.id, added.body.known], ["macbook", false]);
  const peer = () => link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  assert.ok(peer(), "signed in to your computers");
  const hosts = async () => (await ask("GET", "kuvo/hosts")).body.hosts.map(each => [each.machine, each.removed]);
  assert.deepEqual(await hosts(), [["macbook", false], ["studio", true]]);

  // Remove while Set Up Again is in flight. Its old join cannot restore this peer.
  fs.writeFileSync(path.join(fake, "hold"), 'bin/kuvo-start" start');
  let setup;
  try {
    setup = await ask("POST", `kuvo/computers/${peer().key}/setup`);
    await until(setup.body.try.id, found => found?.steps.find(step => step.id === "starting")?.state === "now");
    assert.equal((await ask("DELETE", "kuvo/machines/macbook")).status, 200);
  } finally {
    fs.rmSync(path.join(fake, "hold"));
  }
  const ended = await over(setup.body.try.id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /was removed from your computers/);
  assert.deepEqual(read(path.join(home, "machines.json")), []);
  assert.equal(peer(), undefined);
  assert.ok(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive).length > 0, "it runs on");
  assert.deepEqual(await hosts(), [["macbook", true], ["studio", true]]);
  assert.equal((await ask("DELETE", "kuvo/machines/macbook")).status, 404);

  // Added again, then uninstalled: waited for, so the answer says how it went.
  assert.equal((await ask("POST", "kuvo/machines", { destination: "macbook" })).status, 200);
  // (Never by a one-off command, which anything on the Mac could run.)
  const once = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/machines/macbook/uninstall", headers: {}, fromMac: true }, undefined)) once.push(piece);
  assert.equal(once.find(each => each.head).head.status, 409);
  assert.ok(peer(), "still one of your computers");
  fs.writeFileSync(path.join(fake, "away-macbook"), "");
  try {
    const failed = await ask("POST", "kuvo/machines/macbook/uninstall");
    assert.equal(failed.status, 502, JSON.stringify(failed.body));
    assert.ok(failed.body.message.length > 10);
    assert.equal(read(path.join(home, "machines.json")).length, 1, "still on the list, as it's still there");
  } finally {
    fs.rmSync(path.join(fake, "away-macbook"));
  }
  assert.deepEqual([(await ask("POST", "kuvo/machines/macbook/uninstall")).status, read(path.join(home, "machines.json"))], [200, []]);
  assert.deepEqual(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive), [], "nothing of it runs there");
  assert.equal(peer(), undefined);

  // Forgotten for your phones too, once. (The other stays kept, for the tests after.)
  assert.deepEqual([(await ask("DELETE", "kuvo/hosts/macbook")).status, (await ask("DELETE", "kuvo/hosts/macbook")).status], [200, 404]);
  assert.deepEqual(await hosts(), [["studio", true]]);
});

test("a member of a setup, and a request that can't stay, are told where to add from", async () => {
  // One request and gone (`kuvo-link.js box route`): nothing could stay to do it.
  const pieces = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/computers/add", headers: {}, fromMac: true, body: Buffer.from(JSON.stringify({ destination: "gpu" })) }, undefined)) pieces.push(piece);
  assert.equal(pieces.find(each => each.head).head.status, 409);
  // (Nor by the old route's name.)
  const old = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/machines", headers: {}, fromMac: true, body: Buffer.from(JSON.stringify({ destination: "gpu" })) }, undefined)) old.push(piece);
  assert.equal(old.find(each => each.head).head.status, 409);
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.destination === "gpu" && each.state === "working"), false);
  // The old app attached keeps its own list of machines: adding waits for it to be quit.
  link.macLink.appSaid = Date.now();
  try {
    const said = (await ask("GET", "kuvo/computers/add")).body;
    assert.equal(said.can, false);
    assert.match(said.why, /Quit the old Kuvo Code app on Studio first/);
    assert.equal((await ask("POST", "kuvo/computers/add", { destination: "gpu" })).status, 409);
  } finally {
    link.macLink.appSaid = undefined;
  }
});

test("a missing or unreadable list of machines isn't overwritten when this box still keeps machines' links (#753)", async () => {
  const list = path.join(home, "machines.json");
  const before = fs.readFileSync(list);
  assert.ok(Object.keys(read(path.join(home, "phone-link.json")).machines).length > 0);
  try {
    const front = link.macFront(handle, { wait: 10, look: 60_000 });
    for (const contents of [null, "{ invalid JSON"]) {
      if (contents === null) fs.rmSync(list);
      else fs.writeFileSync(list, contents);
      const said = (await ask("GET", "kuvo/computers/add")).body;
      assert.deepEqual([said.can, said.why], [false, "Studio’s list of machines can’t be read right now."]);
      const refused = await ask("POST", "kuvo/computers/add", { destination: "gpu" });
      assert.deepEqual([refused.status, refused.body.message], [503, "Studio’s list of machines can’t be read right now."]);
      assert.equal(fs.existsSync(list) ? fs.readFileSync(list, "utf8") : null, contents);
      const pieces = [];
      for await (const piece of front({ method: "GET", path: "kuvo/machines", headers: {} }, undefined)) pieces.push(piece);
      assert.equal(pieces.find(each => each.head).head.status, 503);
    }
  } finally {
    fs.writeFileSync(list, before);
  }
});
