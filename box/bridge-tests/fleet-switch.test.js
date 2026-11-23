import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { execFile, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { removeAtEnd } from "./at-end.js";
import { bunRunsAs, NO_BUN, testBun } from "./bun-for-tests.js";

const bun = testBun();
const exec = (file, args, options) => new Promise((resolve, reject) => {
  execFile(file, args, options, (error, stdout, stderr) => error ? reject(error) : resolve({ stdout, stderr }));
});
const directory = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-fleet-")));
removeAtEnd(directory);
const bundle = fileURLToPath(LINK_BUNDLE);
const lib = await import(pathToFileURL(bundle));
const OLD = "0.8.0-beta.40", NEXT = "0.8.0-beta.41", CLOCK = Date.now(), LIMIT = 5 * 60_000;
const sha256 = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
const b64 = bytes => bytes.toString("base64url");
const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");

function write(file, data, mode = 0o600) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data, { mode });
}
function json(file, value) { write(file, JSON.stringify(value)); }
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const shell = word => "'" + word.replaceAll("'", "'\\''") + "'";

function release({ layout = 23, broken = false, version = NEXT, origin = "http://127.0.0.1:1", realLink = false, padding = 0 } = {}) {
  const folder = fs.mkdtempSync(path.join(directory, "release-"));
  const files = path.join(folder, "files");
  const keeper = `#!/bin/sh
# Kuvo release switch v1
V=1.0.0
[ "$1" = stop ] && [ "$2" = --switch ] || exit 1
exec ${shell(process.execPath)} -e 'const fs=require("node:fs"); fs.symlinkSync(process.argv[1],"bin.next"); fs.renameSync("bin.next","bin")' "$3"
`;
  const contents = {
    "bin/kuvo-start": keeper,
    "bin/kuvo-link.js": `// Kuvo folder layout ${layout} 0123456789ab
const mode = process.argv[2];
if (mode === "selftest") console.log("ok");
else if (${!broken}) console.log(JSON.stringify({status:200,internal:true,body:{}}));
else process.exitCode = 1;
`,
    "bin/kuvo-agents.js": 'console.log("ok");\n',
    "bin/box-version": version + "\n",
    "config/opencode/plugins/fixture.js": "export {};\n",
  };
  if (realLink) contents["bin/kuvo-link.js"] = fs.readFileSync(bundle);
  // (Bytes that don't compress: a pack of several parts.)
  if (padding) contents["bin/padding"] = crypto.randomBytes(padding);
  for (const [file, bytes] of Object.entries(contents)) write(path.join(files, file), bytes, file === "bin/kuvo-start" ? 0o700 : 0o600);
  json(path.join(files, "box.json"), { version, engine: "1.0.0", files: Object.fromEntries(Object.entries(contents).map(([name, bytes]) => [name, sha256(bytes)])) });
  const name = `box-${version}.tgz`;
  const made = spawnSync("tar", ["czf", path.join(folder, name), "-C", files, "."], { env: { ...process.env, COPYFILE_DISABLE: "1" } });
  assert.equal(made.status, 0, made.stderr.toString());
  const pack = fs.readFileSync(path.join(folder, name));
  const ref = { version, sha256: sha256(pack) };
  const manifest = Buffer.from(JSON.stringify({
    kuvo: "box/1", version, layout, engine: "1.0.0", spreads: true, released: new Date().toISOString(),
    pack: { url: `${origin}/${name}`, sha256: ref.sha256 },
  }));
  const boxes = JSON.stringify({
    manifest: manifest.toString("base64"), key: pinned,
    signature: crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), privateKey).toString("base64"),
  });
  write(path.join(folder, "boxes.json"), boxes);
  return { folder, name, boxes, pack, ref };
}

function fleet(versions = [OLD]) {
  const id = "set_" + b64(crypto.randomBytes(16));
  const nodes = versions.map((entry, index) => {
    const version = typeof entry === "string" ? entry : entry.version;
    const layout = typeof entry === "string" ? 23 : entry.layout;
    const home = fs.mkdtempSync(path.join(directory, "home-"));
    const root = path.join(home, ".kuvo-code");
    const key = lib.newKey();
    const peer = { key: b64(lib.publicOf(key)), kind: "machine", name: `Node ${index}`, id: `node-${index}`,
      address: b64(crypto.randomBytes(16)), sign: b64(crypto.randomBytes(32)) };
    const run = path.join(root, "run");
    fs.mkdirSync(run, { recursive: true });
    write(path.join(root, "box", "kind"), "machine\n");
    write(path.join(root, "bin", "box-version"), version + "\n");
    write(path.join(root, "bin", "kuvo-start"), "#!/bin/sh\nV=1.0.0\nexit 0\n", 0o700);
    // The predecessor layouts use the split launch contract.
    write(path.join(root, "bin", "kuvo-link.js"), fs.readFileSync(bundle, "utf8").replace("// Kuvo resident serve v1\n", ""));
    write(path.join(root, "engine", "1.0.0", "opencode"), `#!/bin/sh
if [ "$1" = --version ]; then echo 1.0.0; else exec ${shell(bunRunsAs(bun))} "$@"; fi
`, 0o700);
    if (layout) json(path.join(root, "folder-version.json"), { version: layout });
    if (entry.linked) {
      const selected = `versions/${version}-000000000001/bin`;
      const target = path.join(root, selected);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.renameSync(path.join(root, "bin"), target);
      fs.symlinkSync(selected, path.join(root, "bin"));
    }
    write(path.join(root, "data", "opencode", "opencode.db"), "conversations");
    return { home, root, key, peer, run, version };
  });
  const [main] = nodes;
  const choices = nodes.map(node => ({ ...node.peer }));
  for (const node of nodes) json(path.join(node.run, "link.json"), {
    formatVersion: 1, key: b64(node.key), sign: node.peer.sign, secret: b64(crypto.randomBytes(32)), relay: "ws://127.0.0.1:1", phones: [],
    setup: { id, epoch: 1, primary: main.peer.key, sign: main.peer.sign, role: node === main ? "primary" : "member", checkpointed: true, managed: true },
    sharedSetup: id,
    shared: { membershipAndAccess: { sequence: 1, value: { boxes: choices } } },
  });

  function tell(to, from, computer) {
    const file = path.join(to.run, "peer-agents.json");
    const kept = fs.existsSync(file) ? read(file) : {};
    kept[from.peer.key] = { heard: CLOCK, computer };
    json(file, kept);
  }
  for (const node of nodes) for (const other of nodes) if (node !== other) tell(node, other, { kuvo: other.version });
  return { nodes, tell };
}

async function call(node, source, args = {}) {
  const { stdout } = await exec(bun, ["--config=/dev/null", "--no-env-file", "-e", `
    const box = await import(${JSON.stringify(pathToFileURL(bundle).href)});
    const input = ${JSON.stringify(args)};
    const opts = { keys: [${JSON.stringify(pinned)}], blocked: null, clock: () => input.now ?? ${CLOCK} };
    box.fleetWith(opts);
    const run = async () => { ${source} };
    console.log(JSON.stringify(await run()));
  `], {
    cwd: node.root, timeout: 30_000,
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_"))),
      HOME: node.home, PATH: path.join(node.home, "tools") + ":" + process.env.PATH, BUN_BE_BUN: "1" },
  });
  return JSON.parse(stdout.trim().split("\n").at(-1));
}
const facts = node => call(node, "return box.fleetFacts()");
const share = (node, pack) => call(node, "return box.receiveRelease(input.folder, opts.keys)", { folder: pack.folder });
const prepare = (node, pack) => call(node, 'return await box.update({...opts, from: input.folder, phase: "prepare", expected: input.ref})', { folder: pack.folder, ref: pack.ref });
const activate = (node, pack, now) => call(node, 'return await box.update({...opts, phase: "activate", expected: input.ref})', { ref: pack.ref, now });
const selected = node => fs.readFileSync(path.join(node.root, "bin", "box-version"), "utf8").trim();
const commit = node => fs.unlinkSync(path.join(node.run, "previous"));

async function usesBox(node) {
  const statuses = await call(node, `
    const route = box.engineHandler(() => ({}), { name: "Fixture" }, () => null);
    const codes = [];
    for (const version of ["0.8.0-beta.35", "${NEXT}", "0.9.0"]) {
      for (const path of ["kuvo/info", "kuvo/bots"]) {
        const answer = route({ method: "GET", path, fromApp: true, served: true, headers: { "x-kuvo-client-version": version } });
        codes.push((await answer.next()).value.head.status);
        await answer.return();
      }
    }
    return codes;
  `);
  assert.deepEqual(statuses, Array(6).fill(200));
}

async function feed(t) {
  const hits = [];
  let pack, reachable = true;
  const server = http.createServer((request, response) => {
    hits.push(request.url);
    if (request.url === "/boxes.json") return response.end(pack.boxes);
    if (reachable && request.url === "/" + pack.name) return response.end(pack.pack);
    response.writeHead(404).end();
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => server.close());
  const origin = `http://127.0.0.1:${server.address().port}`;
  return { origin, hits, serve(release) { pack = release; }, unplug() { reachable = false; } };
}

async function getPack(node, fallback) {
  return call(node, `
    const { readFileSync } = await import("node:fs");
    let start;
    const prepared = new Promise(resolve => { start = resolve; });
    const transfers = [];
    const options = { ...opts, start(options) { start(options); return true; },
      send(peer, mail) {
        if (!input.fallback) throw new Error("Direct URL should avoid peer transfer");
        assert.equal(peer, input.fallback.peer);
        const pack = readFileSync(input.fallback.file);
        transfers.push(mail);
        const reply = mail.want ? { version: mail.want, boxes: input.fallback.boxes, size: pack.length }
          : { version: mail.version, i: mail.part, d: pack.subarray(mail.part * 30000, (mail.part + 1) * 30000).toString("base64") };
        queueMicrotask(() => box.takeReleaseMail({ key: peer }, reply, options));
        return true;
      } };
    const assert = (await import("node:assert/strict")).default;
    box.followPeers(options);
    const staging = await Promise.race([prepared, new Promise((_, reject) =>
      setTimeout(() => reject(new Error("No pack acquired")), 5000).unref())]);
    if (input.fallback) assert.ok(transfers.some(mail => mail.part === 0));
    return await box.update({ ...staging, ...opts });
  `, { fallback });
}

const requiresBun = { skip: !bun && NO_BUN };

test("T1: main prepares with two online members, commits first, and the off member catches up", requiresBun, async t => {
  const hosted = await feed(t);
  const pack = release({ layout: 24, origin: hosted.origin });
  hosted.serve(pack);
  const { nodes: [main, first, second, off], tell } = fleet([OLD, OLD, OLD, OLD]);
  // (Found away three minutes ago: past the two minutes a computer that just went quiet keeps
  // what it last told, as one restarting does.)
  json(path.join(main.run, "peer-reach.json"), { [off.peer.key]: CLOCK - 3 * 60_000 });
  assert.equal((await share(main, pack)).status, 202);
  for (const node of [main, first, second]) await usesBox(node);
  assert.equal((await prepare(main, pack)).result, "waiting");
  const announced = await facts(main);
  assert.deepEqual(announced.staged, pack.ref);
  assert.equal(announced.kuvo, OLD);
  assert.notEqual(announced.release, NEXT, "beta.35 only reads the committed release field");
  for (const member of [first, second]) tell(member, main, announced);
  assert.equal((await getPack(first)).result, "waiting");
  assert.equal((await getPack(second)).result, "waiting");
  assert.equal((await activate(first, pack)).result, "waiting");
  assert.equal(selected(first), OLD);
  tell(main, first, await facts(first));
  assert.equal((await activate(main, pack)).result, "waiting", "waits for the second online member");
  tell(main, second, await facts(second));
  const summary = await call(main, `
    const rows = await box.listComputers("Node", null);
    return box.fleetSummary(rows, box.fleetTarget());
  `);
  assert.equal(summary.shown, true);
  assert.equal(summary.phase, "ready");
  assert.equal(summary.title, "beta.41 is ready · 3 of 4");
  assert.equal(summary.detail, "They switch next.");
  const switched = await activate(main, pack);
  assert.equal(switched.result, "updated", switched.message);
  assert.equal((await facts(main)).kuvo, OLD, "start guard still owns the switch");
  const legacyRecord = `return box.readLink().shared.setupSecurityAndReleaseChoice?.value ?? null`;
  assert.equal(await call(main, `box.keepLegacyRelease(); ${legacyRecord}`), null);
  commit(main);
  const oldReader = await call(main, `
    box.keepLegacyRelease();
    const kept = box.readLink();
    return {
      choice: kept.shared.setupSecurityAndReleaseChoice?.value,
      peers: box.publishedShared(kept).length,
    };
  `);
  assert.equal(oldReader.choice.sha256, pack.ref.sha256);
  assert.equal(oldReader.peers, 1);
  for (const node of [first, second, off]) {
    tell(node, main, await facts(main));
    if (node === off) {
      assert.equal(selected(node), OLD);
      hosted.unplug();
      assert.equal((await getPack(node, { peer: main.peer.key, boxes: pack.boxes, file: path.join(pack.folder, pack.name) })).result, "waiting");
    }
    const switched = await activate(node, pack);
    assert.equal(switched.result, "updated", switched.message);
    commit(node);
    assert.equal(selected(node), NEXT);
    const ready = await facts(node);
    assert.equal(ready.cannot, undefined);
    tell(main, node, ready);
    await usesBox(node);
  }
  // Done but for the one the main still finds away: said so for ten minutes, as the main saw it
  // under way.
  const [settled, later] = await call(main, `
    const rows = await box.listComputers("Node", null);
    const target = box.fleetTarget();
    return [box.fleetSummary(rows, target), box.fleetSummary(rows, target, Date.now() + 10 * 60_000 + 1)];
  `);
  assert.deepEqual([settled.shown, settled.phase, settled.title, settled.detail], [true, "waiting", "beta.41 · 3 of 4 up to date", "Node 3 updates when it’s back."]);
  assert.equal(later.shown, false);
  assert.deepEqual(hosted.hits, Array(3).fill("/" + pack.name));
});

test("T3: a fixable prepare failure can retry and the five-minute deadline excludes a slow member", requiresBun, async () => {
  const pack = release();
  const bad = release({ broken: true, version: "0.8.0-beta.42" });
  const { nodes: [main, slow], tell } = fleet([OLD, OLD]);
  assert.equal((await share(main, pack)).status, 202);
  assert.equal((await prepare(main, pack)).result, "waiting");
  tell(slow, main, await facts(main));
  const disk = path.join(slow.home, "tools", "df");
  write(disk, "#!/bin/sh\necho 'disk 100000 100000 0 100% /'\n", 0o700);
  assert.equal((await share(slow, pack)).status, 202);
  const denied = await prepare(slow, pack);
  assert.equal(denied.result, "failed");
  const failed = await facts(slow);
  assert.match(failed.cannot.reason, /room/);
  assert.equal(failed.cannot.fixable, true);
  tell(main, slow, failed);
  assert.equal((await activate(main, pack, CLOCK)).result, "waiting");
  assert.equal((await activate(main, pack, CLOCK + LIMIT - 1)).result, "waiting");
  const switched = await activate(main, pack, CLOCK + LIMIT);
  assert.equal(switched.result, "updated", switched.message);
  commit(main);
  tell(slow, main, await facts(main));
  assert.equal(selected(slow), OLD);
  fs.unlinkSync(disk);
  // The main's list says what slow's row shows and that Try Again is offered there; Try Again,
  // asked of the main with slow's key, is passed on to slow, which retries.
  const asked = await call(main, `
    const row = (await box.listComputers("Node", null)).find(row => row.key === input.key);
    const sent = [];
    box.fleetWith({ ...opts, send(peer, mail) { sent.push({ peer, mail }); return true; } });
    const route = box.engineHandler(() => ({}), { name: "Fixture" }, () => null);
    const ask = async body => {
      const pieces = [];
      for await (const piece of route({ method: "POST", path: "kuvo/update/retry", fromApp: true, served: true, headers: {}, body: Buffer.from(JSON.stringify(body)) })) pieces.push(piece);
      return pieces.find(piece => piece.head).head.status;
    };
    const status = await ask({ target: input.ref.version, targetSha256: input.ref.sha256, key: input.key });
    const unknown = await ask({ target: input.ref.version, key: input.stranger });
    return { line: row.fleetUpdate.line, canRetry: row.fleetUpdate.canRetry, status, unknown, sent };
  `, { ref: pack.ref, key: slow.peer.key, stranger: b64(crypto.randomBytes(32)) });
  assert.deepEqual(asked.line, { words: `beta.40 · ${failed.cannot.reason}`, tone: "warning", short: "Can’t update" });
  assert.deepEqual([asked.canRetry, asked.status, asked.unknown], [true, 202, 404]);
  assert.deepEqual(asked.sent, [{ peer: slow.peer.key, mail: { retry: pack.ref } }]);
  const retry = await call(slow, `
    box.takeReleaseMail({ key: input.main }, input.mail);
    let staging;
    box.fleetWith({ ...opts, start(options) { staging = options; return true; } });
    box.consumeFleetRetry();
    return await box.update({ ...staging, ...opts });
  `, { main: main.peer.key, mail: asked.sent[0].mail });
  assert.equal(retry.result, "waiting", retry.message);
  assert.equal((await facts(slow)).cannot, undefined);
  assert.equal((await activate(slow, pack)).result, "updated");
  commit(slow);

  assert.equal((await share(main, bad)).status, 202);
  tell(slow, main, await facts(main));
  const rejected = await prepare(slow, bad);
  assert.match(rejected.message, /internal kuvo\/info/);
  assert.equal((await facts(slow)).cannot.fixable, false);
  assert.equal(selected(slow), NEXT);
});

test("a member ahead of the main shows as up to date and the main takes its signed version", requiresBun, async t => {
  const hosted = await feed(t);
  const pack = release({ origin: hosted.origin });
  hosted.serve(pack);
  const { nodes: [main, ahead], tell } = fleet([OLD, NEXT]);
  const row = await call(ahead, "return (await box.listComputers('Node', null)).find(row => row.here).fleetUpdate");
  assert.equal(row.state, "up-to-date");
  tell(main, ahead, { kuvo: NEXT, installedSha256: pack.ref.sha256, runningManifest: pack.boxes });
  assert.equal((await getPack(main)).result, "waiting");
  assert.equal((await activate(main, pack, CLOCK)).result, "updated");
});

test("Update Now on the main takes the signed release immediately despite an existing choice", requiresBun, async t => {
  const hosted = await feed(t);
  const pack = release({ origin: hosted.origin });
  hosted.serve(pack);
  const { nodes: [main] } = fleet();
  const linkFile = path.join(main.run, "link.json");
  const link = read(linkFile);
  link.shared.setupSecurityAndReleaseChoice = { sequence: 1, value: { version: OLD, sha256: "a".repeat(64), signed: "old" } };
  json(linkFile, link);
  const kept = path.join(main.root, "updates", "release", "boxes.json");
  fs.mkdirSync(path.dirname(path.dirname(kept)), { recursive: true });
  const accepted = await call(main, "return box.requestUpdateNow({ keys: opts.keys, url: input.url })", { url: hosted.origin + "/boxes.json" });
  assert.equal(accepted.status, 202, accepted.message);
  const waiting = await call(main, `
    const { watch } = await import("node:fs/promises");
    const { existsSync } = await import("node:fs");
    if (existsSync(input.kept)) return true;
    for await (const _ of watch("updates", { signal: AbortSignal.timeout(5000) })) {
      if (existsSync(input.kept)) return true;
    }
    return false;
  `, { kept });
  assert.equal(waiting, true);
  assert.equal(fs.readFileSync(kept, "utf8"), pack.boxes);
  assert.deepEqual(hosted.hits, ["/boxes.json", "/" + pack.name]);
  assert.equal(selected(main), OLD, "the feed check only keeps a candidate");
  assert.equal((await getPack(main)).result, "waiting");
  assert.equal((await activate(main, pack)).result, "updated");
  commit(main);
  assert.equal(selected(main), NEXT);
});

for (const old of [{ version: "0.8.0-beta.35", layout: 18 }, { version: OLD, layout: 23 }, { version: NEXT, layout: 23, linked: true }, { version: "0.7.2", layout: 0 }]) {
  test(`a ${old.version} style home prepares and switches with its backup destination unplugged`, requiresBun, async () => {
    const pack = release({ realLink: true, version: "0.8.0-beta.42" });
    const { nodes: [main] } = fleet([old]);
    const backup = path.join(main.root, "backup", "configuration.json");
    json(backup, { version: 1, enabled: true, projects: [], key: pinned, source: "fixture",
      dest: { kind: "disk", path: path.join(main.home, "unplugged") }, disk: -1 });
    assert.equal((await share(main, pack)).status, 202);
    const prepared = await prepare(main, pack);
    assert.equal(prepared.result, "waiting", prepared.message);
    assert.equal(selected(main), old.version);
    const switched = await activate(main, pack);
    assert.equal(switched.result, "updated", switched.message);
    const upgraded = await call(main, `
      const { spawnSync } = await import("node:child_process");
      const ran = spawnSync(process.execPath, ["bin/kuvo-link.js", "folder-upgrade"], { encoding: "utf8" });
      return { status: ran.status, stderr: ran.stderr };
    `);
    assert.equal(upgraded.status, 0, upgraded.stderr);
    assert.equal(read(path.join(main.root, "folder-version.json")).version, 23);
    commit(main);
    assert.equal(selected(main), pack.ref.version);
    assert.equal(fs.readFileSync(path.join(main.root, "data", "opencode", "opencode.db"), "utf8"), "conversations");
    assert.equal(read(backup).error, undefined);
  });
}

test("the setup's update is said in the box's words: shown while computers get ready or one can be fixed, and for a while once it's done", requiresBun, async () => {
  const { nodes: [main] } = fleet([OLD]);
  const row = (name, state, current, more = {}) => ({ name, fleetUpdate: { state, current, target: NEXT, canRetry: false, ...more } });
  const say = rows => call(main, "return box.fleetSummary(input.rows, { version: input.target })", { rows, target: NEXT });
  const room = "There isn’t room on it to keep its data while it updates.";

  // Nothing you can do, on a box that never saw this update under way: a computer that's off, or one
  // that can't take this release, hides it.
  const quietRows = [row("spare-linux", "off", OLD), row("spare-linux", "cannot", OLD, { reason: "beta.41 doesn’t run here.", fixable: false })];
  for (const rest of quietRows) {
    const quiet = await say([row("main-linux", "up-to-date", NEXT), row("macbook", "up-to-date", "0.8.0-beta.42"), rest]);
    assert.equal(quiet.shown, false);
    assert.equal(quiet.warning, false);
    assert.equal(quiet.detail, undefined);
  }

  const preparing = await say([row("main-linux", "ready", OLD), row("mac-mini", "getting-ready", OLD), row("macbook", "ready", OLD), row("spare-linux", "off", "0.8.0-beta.38")]);
  assert.equal(preparing.shown, true);
  assert.equal(preparing.warning, false);
  assert.equal(preparing.phase, "getting-ready");
  assert.equal(preparing.title, "Getting beta.41 ready · 2 of 4", "counts ready, of all of them");
  assert.equal(preparing.detail, "Your computers stay on beta.40 meanwhile.");

  const mixed = await say([row("main-linux", "getting-ready", OLD), row("mac-mini", "getting-ready", "0.8.0-beta.39")]);
  assert.equal(mixed.title, "Getting beta.41 ready");
  assert.equal(mixed.detail, "Your computers keep what they run meanwhile.");

  const fixable = await say([row("main-linux", "up-to-date", NEXT), row("spare-linux", "cannot", OLD, { reason: room, fixable: true, canRetry: true })]);
  assert.equal(fixable.shown, true);
  assert.equal(fixable.warning, true);
  assert.equal(fixable.title, "spare-linux can’t update to beta.41");
  assert.equal(fixable.detail, `spare-linux: ${room}`);

  const two = await say([row("a", "cannot", OLD, { reason: room, fixable: true }), row("b", "cannot", OLD, { reason: room, fixable: true })]);
  assert.equal(two.title, "2 computers can’t update to beta.41");

  // Seen under way here: done but for the one that's off, it says so.
  const waiting = await say([row("main-linux", "up-to-date", NEXT), row("macbook", "up-to-date", "0.8.0-beta.42"), quietRows[0]]);
  assert.deepEqual([waiting.shown, waiting.phase, waiting.title, waiting.detail], [true, "waiting", "beta.41 · 2 of 3 up to date", "spare-linux updates when it’s back."]);
});

test("each computer's row says in the box's words how its update stands, and whether Try Again is offered", requiresBun, async () => {
  const { nodes: [main] } = fleet([OLD]);
  const sha = "a".repeat(64);
  const room = "There isn’t room on it to keep its data while it updates.";
  const cases = [
    [{ kuvo: NEXT }, false, { words: "beta.41 · Up to date", tone: "done", phase: "done" }, false],
    [{ kuvo: "0.8.0-beta.42" }, false, { words: "beta.42 · Up to date", tone: "done", phase: "done" }, false],
    [{ kuvo: OLD, staged: { version: NEXT, sha256: sha } }, false, { words: "Ready for beta.41", tone: "done", phase: "ready" }, false],
    [{ kuvo: OLD }, false, { words: "Getting ready…", tone: "working", phase: "getting-ready" }, false],
    [{ kuvo: OLD }, true, { words: "beta.40 · Can’t be reached: updates when it’s back", tone: "off", phase: "off" }, false],
    [{ kuvo: OLD, cannot: { version: NEXT, sha256: sha, reason: room, fixable: true } }, false, { words: `beta.40 · ${room}`, tone: "warning", short: "Can’t update" }, true],
    [{ kuvo: OLD, cannot: { version: NEXT, sha256: sha, reason: "beta.41 doesn’t run here.", fixable: false } }, false, { words: "beta.40 · beta.41 doesn’t run here. · Updates with the next release", tone: "blocked" }, false],
  ];
  const rows = await call(main, "return input.cases.map(([facts, away]) => box.peerFleetUpdate({ version: input.target, sha256: input.sha }, facts, away))", { cases, target: NEXT, sha });
  rows.forEach((row, ix) => assert.deepEqual([row.line, row.canRetry], [cases[ix][2], cases[ix][3]], JSON.stringify(cases[ix][0])));
});

test("a computer getting a release from another of yours says how far it got, and never steps back", requiresBun, async () => {
  // (Its pack's address can't be reached: it comes part by part from the computer that holds it.)
  const pack = release({ padding: 100_000 });
  const { nodes: [main, holder], tell } = fleet([OLD, OLD]);
  tell(main, holder, { kuvo: OLD, candidate: NEXT });
  const seen = await call(main, `
    const { readFileSync } = await import("node:fs");
    const pack = readFileSync(input.file);
    const queue = [];
    const options = { ...opts, start() { return true; }, send(peer, mail) {
      queue.push(mail.want ? { version: mail.want, boxes: input.boxes, size: pack.length }
        : { version: mail.version, i: mail.part, d: pack.subarray(mail.part * 30000, (mail.part + 1) * 30000).toString("base64") });
      return true;
    } };
    box.fleetWith(options);
    const deliver = count => { for (const mail of queue.splice(0, count)) box.takeReleaseMail({ key: input.holder }, mail, options); };
    const partsAsked = async () => {
      for (let tries = 0; tries < 500 && !queue.length; tries++) await new Promise(done => setTimeout(done, 10));
      if (!queue.length) throw new Error("No parts were asked for.");
    };
    const row = () => { const update = box.localFleetUpdate(box.fleetTarget()); return [update.state, update.fill ?? null, update.line.words, update.line.phase]; };
    const seen = [];
    box.followPeers(options);
    seen.push(row());
    deliver(1);
    await partsAsked();
    const parts = box.pullUnderWay().parts;
    seen.push(row());
    deliver(2);
    seen.push(row());
    // Its pull starts again (Try Again, another computer): the row keeps how far it got.
    box.forgetFleet();
    queue.length = 0;
    box.followPeers(options);
    deliver(1);
    await partsAsked();
    deliver(1);
    seen.push(row());
    deliver(2);
    seen.push(row());
    return { parts, seen };
  `, { file: path.join(pack.folder, pack.name), boxes: pack.boxes, holder: holder.peer.key });
  assert.equal(seen.parts, 4);
  assert.deepEqual(seen.seen, [
    ["getting-ready", null, "Getting ready…", "getting-ready"],
    ["getting-ready", 0, "Getting ready · 0%", "getting-ready"],
    ["getting-ready", 0.5, "Getting ready · 50%", "getting-ready"],
    ["getting-ready", 0.5, "Getting ready · 50%", "getting-ready"],
    ["getting-ready", 0.75, "Getting ready · 75%", "getting-ready"],
  ]);
});

test("Check for Updates on a member asks its main to look at the feed, at once or once it's back", requiresBun, async () => {
  const pack = release();
  const { nodes: [main, member] } = fleet([OLD, OLD]);
  const check = () => call(member, "return box.requestUpdateNow({ keys: opts.keys })");
  const consume = () => call(member, `
    const sent = [];
    box.consumeFleetCheck({ ...opts, send(peer, mail) { sent.push([peer, mail]); return true; } });
    return sent;
  `);

  assert.deepEqual(await check(), { status: 202, message: "Node 0 is checking for updates." });
  assert.deepEqual(await consume(), [[main.peer.key, { check: true }]]);
  assert.deepEqual(await consume(), [], "told once");

  const takes = sender => call(main, `
    let started = null;
    box.takeReleaseMail({ key: input.sender }, { check: true }, { ...opts, start(options) { started = options; return true; } });
    return started;
  `, { sender });
  assert.deepEqual(await takes(member.peer.key), { now: true });
  assert.equal(await takes("not-a-peer"), null, "only your own computers ask");

  // The main off: it's kept until the main is back, and said so.
  json(path.join(member.run, "peer-reach.json"), { [main.peer.key]: CLOCK });
  assert.deepEqual(await check(), { status: 202, message: "Kuvo checks for updates when Node 0 (your main computer) is back." });
  assert.equal((await share(member, pack)).status, 202);
  assert.deepEqual(await check(), { status: 202, message: "beta.41 is ready. It installs on all your computers when Node 0 (your main computer) is back." });
  assert.deepEqual(await consume(), []);
  fs.unlinkSync(path.join(member.run, "peer-reach.json"));
  assert.deepEqual(await consume(), [[main.peer.key, { check: true }]], "sent once the main is back");
});
