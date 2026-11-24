// A computer's first box, installed from the release an app carries (PROTOCOL.md, "Installing this
// computer's box"; #713 piece 3): `kuvo-install`, as scripts/box-release.sh writes it beside
// `boxes.json` and the pack, run as the app runs it, in a home of the test's own. The release is
// packed here by the real script from this checkout's link as built; only its engine is a stand-in
// (it says its version, and runs the link's script as the engine's Bun does), its keeper's start
// script (it notes what it's asked, and starts nothing), and on a Mac Kuvo Helper (a program signed
// ad hoc) with stand-ins for `launchctl`, `crontab` and `open`: never this computer's launchd or
// crontab, nor anything fetched from the network (the engine's "download" is a file here).
// Run with `node --test box/bridge-tests/install.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { removeAtEnd } from "./at-end.js";

const mac = process.platform === "darwin";
const script = fileURLToPath(LINK_BUNDLE);
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const scratch = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-install-")));
removeAtEnd(scratch);

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const read = file => fs.readFileSync(file, "utf8");
const exists = file => fs.existsSync(file);
const modeOf = file => (fs.statSync(file).mode & 0o777).toString(8);

// The engine's binary: its version, and the link's script run as the engine's Bun runs it (here
// this Node). Each run of it is noted, to tell a download from one that's there already.
const ENGINE = "1.1.0", VERSION = "0.10.0";
const engineBinary = `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo ${ENGINE}; exit 0 ;;
*.js) exec '${process.execPath}' "$@" ;;
esac
exit 64
`;
const engineTar = path.join(scratch, `engine-${ENGINE}.tgz`);
write(path.join(scratch, "engine", "package", "bin", "opencode"), engineBinary, 0o755);
assert.equal(spawnSync("tar", ["czf", engineTar, "-C", path.join(scratch, "engine"), "package"], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
const engineBytes = fs.readFileSync(engineTar);

// The release's sources: this checkout's own, with the engine above pinned (fetched from a file)
// and a start script that only notes what it's asked.
const sources = path.join(scratch, "sources");
for (const file of ["box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
  write(path.join(sources, file), fs.readFileSync(path.join(checkout, file)));
}
write(path.join(sources, "box/dist/kuvo-link.js"), fs.readFileSync(script));
write(path.join(sources, "box/src/keeper/kuvo-start"), `#!/bin/sh\n# A stand-in for the keeper, for engine @ENGINE@: it notes what it's asked.\necho "$*" >>"$HOME/keeper-asked"\nif [ -f "$HOME/keeper-fails" ]; then mkdir -p "$HOME/.kuvo-code/box/keys" "$HOME/.kuvo-code/run"; echo '{"mac":"${"A".repeat(43)}","macs":["${"A".repeat(43)}"],"phones":[]}' >"$HOME/.kuvo-code/run/link.json"; echo "its engine didn't come up" >"$HOME/.kuvo-code/run/engine.log"; exit 3; fi\n`);
const build = { url: pathToFileURL(engineTar).href, integrity: "sha512-" + crypto.createHash("sha512").update(engineBytes).digest("base64") };
write(path.join(sources, "box/src/shared/engine-pin.json"), JSON.stringify({ version: ENGINE, linux: { x86_64: build, aarch64: build }, darwin: { x86_64: build, aarch64: build } }));

// Kuvo Helper as its build leaves it: on a Mac a program signed ad hoc (packing checks it).
const helper = path.join(scratch, "helper", "Kuvo Helper.app");
if (mac) {
  write(path.join(helper, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Kuvo Helper</string><key>CFBundleIdentifier</key><string>run.kuvo.code.tests.box</string></dict></plist>\n`);
  fs.mkdirSync(path.join(helper, "Contents", "MacOS"), { recursive: true });
  fs.copyFileSync("/usr/bin/true", path.join(helper, "Contents", "MacOS", "Kuvo Helper"));
  fs.chmodSync(path.join(helper, "Contents", "MacOS", "Kuvo Helper"), 0o755);
  const signed = spawnSync("codesign", ["--force", "--sign", "-", helper], { encoding: "utf8" });
  assert.equal(signed.status, 0, signed.stderr);
}

// Packed once, as a release is, into the folder an app carries.
const release = path.join(scratch, "box-release");
{
  const { privateKey } = crypto.generateKeyPairSync("ed25519");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), VERSION, `https://example.invalid/releases/download/v${VERSION}/box-${VERSION}.tar.gz`, release, ...(mac ? [helper] : [])], {
    env: { ...process.env, BUN: process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), NODE: process.execPath, BOX_SOURCES: sources, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
}

const APP_ID = "run.kuvo.code.trial713";
let count = 0;
/** A computer of the test's own: its home folder, the app's bundle, and stand-ins for what starts
 * a box from outside its folder, each noting what it's asked. */
function computer({ app = APP_ID } = {}) {
  const home = path.join(scratch, `home-${++count}`);
  const tools = path.join(home, "tools");
  write(path.join(tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${home}/launchctl-asked"\ncase "$1" in print) [ -f "${home}/loaded" ] ;; bootstrap) if [ -f "${home}/launchd-refuses" ]; then echo "Bootstrap failed: 5: Input/output error" >&2; exit 5; fi; touch "${home}/loaded" ;; bootout) rm -f "${home}/loaded" ;; print-disabled) exit 0 ;; esac\n`, 0o755);
  write(path.join(tools, "crontab"), `#!/bin/sh\nif [ "$1" = -l ]; then [ -f "${home}/crontab" ] && cat "${home}/crontab" || { echo "no crontab for you" >&2; exit 1; }; else cat >"${home}/crontab"; fi\n`, 0o755);
  write(path.join(tools, "open"), `#!/bin/sh\necho "$*" >>"${home}/open-asked"\n`, 0o755);
  // (Your Keychain, as `security` lists it: an earlier Kuvo Code's saved sign-ins, where the test
  // puts any; never this computer's Keychain.)
  write(path.join(tools, "security"), `#!/bin/sh\necho "$*" >>"${home}/security-asked"\n[ -f "${home}/saved-sign-ins" ] || exit 44\n`, 0o755);
  const bundle = path.join(home, "Applications", "Kuvo Code.app");
  write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${app}</string></dict></plist>\n`);
  return { home, tools, bundle, K: path.join(home, ".kuvo-code"), S: path.join(home, "Library", "Application Support", app) };
}
/** `kuvo-link.js rehome .kuvo-code <args>`, as the box runs it in its own home, on its own engine. */
function boxRehome(where, args) {
  const ran = spawnSync(path.join(where.K, "engine", ENGINE, "opencode"), [path.join(where.K, "bin", "kuvo-link.js"), "rehome", ".kuvo-code", ...args], {
    cwd: where.K, env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_HOME: where.K }, encoding: "utf8", timeout: 120_000,
  });
  return { status: ran.status, said: JSON.parse(ran.stdout.trim().split("\n").pop() || "{}"), stderr: ran.stderr };
}
/** `sh kuvo-install <verb>`, as the app runs it from the release it carries: its lines, the last
 * one its answer. A KUVO_ variable of the app's own never reaches the box's commands. */
function install(where, verb, { from = release, args = mac ? ["--app", where.bundle] : [], env = {} } = {}) {
  const ran = spawnSync("sh", [path.join(from, "kuvo-install"), verb, ...args], {
    env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_TEST_INSTALL_FILE_ENGINE: "1", KUVO_BOX: "mac", KUVO_HOME: "/nowhere", ...env },
    encoding: "utf8", timeout: 240_000,
  });
  const lines = ran.stdout.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
  return { status: ran.status, lines, steps: lines.filter(line => line.step).map(line => line.step), said: lines[lines.length - 1], stderr: ran.stderr };
}
const staging = where => path.join(where.K, "updates", "install");
const everything = folder => (exists(folder) ? fs.readdirSync(folder, { recursive: true, encoding: "utf8" }).sort() : []);

test("status on a new computer: the pack checked and unpacked, the release's engine fetched once and checked, and the verdict `fresh`", () => {
  const where = computer();
  const first = install(where, "status");
  assert.equal(first.status, 0, first.stderr + JSON.stringify(first.lines));
  assert.deepEqual(first.steps, ["checking", "engine"]);
  // (Each in words an app shows as they are.)
  assert.deepEqual(first.lines.filter(line => line.step).map(line => line.title), ["Getting ready", "Downloading the engine"]);
  assert.equal(first.said.verdict, "fresh");
  assert.equal(first.said.verb, "new");
  assert.equal(first.said.offer, undefined);
  assert.equal(first.said.status.state, null, "no move here");
  // What it staged, in the box's home alone: the release whole, and the engine by its version.
  assert.equal(read(path.join(staging(where), "release", "bin", "box-version")), VERSION);
  assert.equal(read(path.join(staging(where), "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(modeOf(where.K), "700");
  assert.deepEqual(fs.readdirSync(where.K), ["updates"], "no box made by asking");
  assert.deepEqual(fs.readdirSync(where.home).sort(), [".kuvo-code", "Applications", "tools"], "nothing else in the home folder");
  // Asked again (each time the app opens): nothing unpacked or fetched again.
  const again = install(where, "status");
  assert.equal(again.status, 0, again.stderr);
  assert.deepEqual(again.steps, []);
  assert.equal(again.said.verdict, "fresh");
});

test("failure bench: the installer offers reinstall only after the last setup wipe", () => {
  const where = computer();
  const setup = `set_${"A".repeat(22)}`;
  const operation = "b".repeat(32);
  const receipts = path.join(where.home, ".kuvo-code-removal-receipts");
  const marker = path.join(receipts, "last-setup-wipe");
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ setup: { id: setup, role: "primary" }, localWipe: { setup, operation } }), 0o600);
  write(path.join(where.K, "run", "off"), "");
  write(path.join(where.K, "data", "old-session"), "wipe this");
  const wiping = install(where, "status");
  assert.equal(wiping.status, 0, wiping.stderr);
  assert.equal(wiping.said.verdict, "waits");
  assert.equal(wiping.said.verb, null);
  assert.equal(wiping.said.offer, undefined);
  assert.equal(exists(marker), false, "the accepted wipe fences fresh install before its receipt");

  const { dev, ino } = fs.statSync(where.K, { bigint: true });
  write(marker, JSON.stringify({ setup, operation, source: `${dev}:${ino}`, phase: "erasing" }), 0o600);
  fs.chmodSync(receipts, 0o700);
  const erased = path.join(receipts, `erase-${operation}`);
  fs.renameSync(where.K, erased);
  fs.rmSync(path.join(erased, "run", "link.json"));
  const newFile = path.join(where.K, "desktop", "marker");
  write(newFile, "keep for the reinstall");
  const lock = `${where.K}-wipe.lock`;
  fs.mkdirSync(lock);
  write(path.join(lock, `owner-${process.pid}-${"a".repeat(16)}`), "");
  const busy = install(where, "status");
  assert.equal(busy.status, 0, busy.stderr);
  assert.equal(busy.said.verdict, "waits");
  assert.equal(busy.said.offer, undefined);
  assert.equal(exists(path.join(erased, "data", "old-session")), true);
  fs.rmSync(lock, { recursive: true });

  for (let attempt = 0; attempt < 2; attempt++) {
    const after = install(where, "status");
    assert.equal(after.status, 0, after.stderr);
    assert.equal(after.said.verdict, "fresh");
    assert.equal(after.said.offer, "reinstall-local");
    assert.equal(after.said.verb, null, "it waits for the user to choose reinstall");
    assert.equal(exists(path.join(where.K, "run", "link.json")), false);
    assert.equal(exists(erased), false);
    assert.equal(read(newFile), "keep for the reinstall");
  }
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.said));
  assert.equal(made.said.started, true);
  assert.equal(exists(marker), false);
  if (!mac) {
    const local = JSON.parse(read(path.join(where.K, "run", "link.json")));
    assert.notEqual(local.setup.id, setup);
    assert.equal(local.setup.role, "primary");
    assert.equal(local.setup.epoch, 1);
  }
});

test("failure bench: a stopped Mac's authorized last wipe finishes without its helper", async () => {
  const where = computer();
  const setup = `set_${"A".repeat(22)}`;
  const operation = "c".repeat(32);
  const key = crypto.randomBytes(32);
  const privateKey = crypto.createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b656e04220420", "hex"), key]), format: "der", type: "pkcs8" });
  const primary = { key: crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32).toString("base64url"), sign: "b".repeat(43), address: "c".repeat(43), name: "Mac", kind: "mac" };
  write(path.join(where.K, "box", "kind"), "mac\n");
  write(path.join(where.K, "folder-version.json"), '{"version":21}\n');
  const linkFile = path.join(where.K, "run", "link.json");
  write(linkFile, JSON.stringify({
    formatVersion: 1, phones: [], sharedSetup: setup,
    setup: { id: setup, epoch: 1, primary: primary.key, sign: primary.sign, role: "primary", checkpointed: true },
    shared: { membershipAndAccess: { value: { boxes: [primary] } } },
    sharedOutcomes: { [operation]: { status: "committed" } },
    localWipe: { setup, operation },
  }), 0o600);
  const oldData = path.join(where.K, "data", "old-session");
  write(oldData, "wipe this");
  const keeper = path.join(where.K, "bin", "kuvo-start");
  write(keeper, '#!/bin/sh\nK="$HOME/.kuvo-code"\nexit 1\n');
  const password = "fixture-host-password";
  const keys = new Set(["KUVO_KEY_DEMO_SELECTED"]);
  const helper = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const input = JSON.parse(Buffer.concat(chunks).toString() || "{}");
    const send = (value, status = 200) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
    if (request.url === "/hello") return send({ proof: crypto.createHmac("sha256", password).update(`kuvo-host-hello\0${input.nonce}\0${request.headers.host}`).digest("hex") });
    if (request.headers.authorization !== `Bearer ${password}`) return send({}, 401);
    if (request.url === "/link/identity") return send({ key: key.toString("base64url"), sign: "b".repeat(43), secret: "c".repeat(43) });
    if (request.url === "/keys/names") return send({ names: [...keys] });
    if (request.url === "/keys/delete") { keys.delete(input.name); return send({}); }
    return send({}, 404);
  });
  await new Promise(resolve => helper.listen(0, "127.0.0.1", resolve));
  try {
    const child = spawn(process.execPath, [script, "wipe-member"], { env: {
      PATH: process.env.PATH, HOME: where.home, KUVO_HOME: where.K, KUVO_TEST_UNINSTALL_TOOLS: where.tools,
      KUVO_TEST_RELAY: "ws://127.0.0.1:1", KUVO_HOST_URL: `http://127.0.0.1:${helper.address().port}`, KUVO_HOST_PASSWORD: password,
    }, stdio: "ignore" });
    assert.equal(await new Promise(resolve => child.once("exit", resolve)), 1);
    assert.ok(exists(oldData));
    assert.equal(keys.size, 0);
    const current = JSON.parse(read(linkFile));
    assert.deepEqual(current.localWipe, { setup, operation, prepared: true, hostKeysCleared: true });
    assert.equal(current.key, undefined);
  } finally {
    helper.closeAllConnections();
    await new Promise(resolve => helper.close(resolve));
  }
  fs.rmSync(keeper);
  const receipts = path.join(where.home, ".kuvo-code-removal-receipts");
  const marker = path.join(receipts, "last-setup-wipe");
  assert.equal(JSON.parse(read(marker)).phase, "stopping");
  const ended = install(where, "status");
  assert.equal(ended.status, 0, ended.stderr);
  assert.equal(ended.said.offer, "reinstall-local", JSON.stringify(ended.said));
  assert.equal(ended.said.verb, null);
  assert.equal(exists(oldData), false);
  assert.deepEqual(JSON.parse(read(marker)), { setup, operation, formatVersion: 1 });
});

test("the last app-owned wipe verifies preparation and leaves a completion receipt outside App Support data", () => {
  const where = computer();
  const box = path.join(where.S, "box");
  const setup = `set_${"A".repeat(22)}`, operation = "e".repeat(32);
  const privateKey = crypto.generateKeyPairSync("x25519").privateKey;
  const key = privateKey.export({ format: "der", type: "pkcs8" }).subarray(-32).toString("base64url");
  const primary = crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32).toString("base64url");
  const sign = "b".repeat(43);
  const writeWipe = extra => write(path.join(box, "run/link.json"), JSON.stringify({
    key, sign, phones: [],
    setup: { id: setup, epoch: 1, primary, sign, role: "primary", checkpointed: true },
    sharedOutcomes: { [operation]: { status: "committed" } },
    localWipe: { setup, operation, ...extra },
  }), 0o600);
  const receipt = path.join(path.dirname(where.S), `.${path.basename(where.S)}-removal-receipts`, "last-setup-wipe");
  for (const leaf of ["box/folder-version.json", "bots/bot_a/MEMORY.md", "wakes/ses_a/paused",
    "opencode/data/session", "Attachments/ses_a/attachment", "Chats/chat", "groups/grp_a/messages.json"]) {
    write(path.join(where.S, leaf), leaf.endsWith("folder-version.json") ? '{"version":21}' : "old data");
  }
  const outside = path.join(where.home, "unrelated", "project");
  write(outside, "keep");
  const env = { PATH: process.env.PATH, HOME: where.home, KUVO_HOME: box, KUVO_BOX: "mac", KUVO_TEST_UNINSTALL_TOOLS: where.tools,
    KUVO_ENGINE_CONFIG: path.join(where.S, "opencode/config"), KUVO_ENGINE_DATA: path.join(where.S, "opencode/data"),
    KUVO_BOTS: path.join(where.S, "bots"), KUVO_WAKES: path.join(where.S, "wakes"),
    KUVO_MAC_ATTACHMENTS: path.join(where.S, "Attachments"),
  };
  const wipe = (extra = {}) => spawnSync(process.execPath, [script, "wipe-member"], {
    encoding: "utf8", timeout: 45_000, env: { ...env, ...extra },
  });

  const siblingExport = path.join(where.S, "sibling-export");
  fs.mkdirSync(siblingExport);
  const separateExport = path.join(where.home, "separate-export");
  fs.mkdirSync(separateExport);
  writeWipe({});
  const attempt = spawnSync(process.env.KUVO_TEST_BUN || "bun", ["--config=/dev/null", "--no-env-file", "-e", `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { recordKey } from ${JSON.stringify(new URL("../src/link/ownership/store.ts", import.meta.url).href)};
    import { sourceGeneration } from ${JSON.stringify(new URL("../src/link/setup/removal-generation.ts", import.meta.url).href)};
    import { exportBeforeWipe, verifyExport } from ${JSON.stringify(new URL("../src/link/backup/engine.ts", import.meta.url).href)};
    const target = { destination: { kind: "disk", path: ${JSON.stringify(siblingExport)} }, recoveryKey: "A".repeat(43) };
    const point = { source: "b_" + "a".repeat(22), id: "2026-11-19T000000.000Z-wipe-0123456789abcdef", at: 0 };
    await assert.rejects(exportBeforeWipe(point, target, []), /separate from the Kuvo folder/);
    await assert.rejects(verifyExport(point, target), /separate from the Kuvo folder/);
    target.destination.path = ${JSON.stringify(separateExport)};
    await exportBeforeWipe(point, target, []);
    const generation = sourceGeneration([]);
    const folder = path.join(process.env.KUVO_HOME, "run", "wipe-preparation");
    const operation = ${JSON.stringify(operation)};
    const id = recordKey(${JSON.stringify(setup)}, ${JSON.stringify(primary)}, operation);
    fs.mkdirSync(folder, { recursive: true });
    fs.writeFileSync(path.join(folder, "source-" + id + ".json"), JSON.stringify({
      operation, generation, ready: true, exported: true, point, choice: { export: { ...target, projects: [] } },
    }));
    console.log(JSON.stringify({ generation, archive: path.join(target.destination.path, "kuvo-backups", point.source, point.id + ".kuvo") }));
  `], { encoding: "utf8", env: { ...process.env, ...env }, timeout: 15_000 });
  assert.equal(attempt.status, 0, attempt.stderr);
  const { generation, archive } = JSON.parse(attempt.stdout);
  const verifiedExport = fs.readFileSync(archive);
  assert.deepEqual(fs.readdirSync(siblingExport), []);
  writeWipe({ requiresPreparation: true });
  assert.equal(wipe().status, 1);
  assert.equal(exists(receipt), false);
  assert.equal(read(path.join(where.S, "Chats/chat")), "old data");
  writeWipe({});
  assert.equal(wipe({ KUVO_BOTS: path.dirname(outside) }).status, 1);
  assert.equal(exists(receipt), false);
  if (mac) assert.equal(install(where, "status").said.verdict, "waits");

  writeWipe({ requiresPreparation: true, generation });
  const keeper = path.join(box, "run", "keeper.json");
  write(keeper, "not readable as a keeper record", 0o600);
  assert.equal(wipe().status, 1);
  assert.equal(JSON.parse(read(receipt)).phase, "stopping");
  assert.equal(exists(where.S), true);
  fs.rmSync(keeper);
  if (mac) {
    const after = install(where, "status");
    assert.equal(after.said.offer, "reinstall-local", after.stderr);
    assert.deepEqual(after.said.wipeReceipt, { setup, operation });
    assert.deepEqual(read(path.join(where.home, "security-asked")).trim().split(String.fromCharCode(10)),
      ["key", "secret", "sign"].map(account => `delete-generic-password -s ${APP_ID}.phone-link -a ${account}`));
  } else {
    const erased = wipe();
    assert.equal(erased.status, 0, erased.stderr);
  }
  assert.equal(exists(where.S), false);
  assert.equal(read(outside), "keep");
  assert.deepEqual(fs.readFileSync(archive), verifiedExport);
  assert.deepEqual(JSON.parse(read(receipt)), { setup, operation, formatVersion: 1 });
});

test("two installers at once on a new computer each get their answer: neither takes the other's unpacking or download away", async () => {
  const where = computer();
  const run = () => new Promise(resolve => {
    const child = spawn("sh", [path.join(release, "kuvo-install"), "status", ...(mac ? ["--app", where.bundle] : [])], {
      env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_TEST_INSTALL_FILE_ENGINE: "1" }, stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    child.stdout.on("data", chunk => { out += chunk; });
    child.once("exit", status => resolve({ status, said: JSON.parse(out.trim().split("\n").pop() || "{}") }));
  });
  const all = await Promise.all([run(), run(), run(), run()]);
  for (const each of all) assert.deepEqual({ status: each.status, verdict: each.said.verdict }, { status: 0, verdict: "fresh" }, JSON.stringify(each.said));
  assert.deepEqual(fs.readdirSync(staging(where)).sort(), ["engine", "release"], "no scratch copy left");
  assert.equal(fs.readdirSync(path.join(staging(where), "release")).some(name => name.startsWith("release")), false, "nor one inside the other");
  assert.deepEqual(fs.readdirSync(path.join(staging(where), "engine")), [ENGINE]);
  assert.equal(read(path.join(staging(where), "release", "bin", "box-version")), VERSION);
});

test("an engine named as a file is only ever a test's: without a test's say-so the release is refused as damaged, and no engine is taken", () => {
  const where = computer();
  const refused = install(where, "status", { env: { KUVO_TEST_INSTALL_FILE_ENGINE: "" } });
  assert.equal(refused.status, 1);
  assert.deepEqual(refused.said, { result: "failed", message: "This copy of Kuvo is damaged. Download it again, then try again." });
  assert.deepEqual(everything(path.join(staging(where), "engine")), []);
});

test("a damaged copy is refused in words before anything of it runs: a pack that isn't what boxes.json says, an engine that isn't what the release says", () => {
  // The pack, changed after it was signed.
  const damaged = path.join(scratch, "damaged-release");
  fs.cpSync(release, damaged, { recursive: true });
  fs.appendFileSync(path.join(damaged, `box-${VERSION}.tar.gz`), "x");
  const one = computer();
  const refused = install(one, "status", { from: damaged });
  assert.equal(refused.status, 1);
  assert.deepEqual(refused.said, { result: "failed", message: "This copy of Kuvo is damaged. Download it again, then try again." });
  assert.deepEqual(everything(staging(one)), [], "nothing unpacked");

  // The engine, other bytes than the release pinned.
  const two = computer();
  fs.writeFileSync(engineTar, Buffer.concat([engineBytes, Buffer.from("x")]));
  try {
    const wrong = install(two, "status");
    assert.equal(wrong.status, 1);
    assert.deepEqual(wrong.said, { result: "failed", message: "The engine that was downloaded isn't what this copy of Kuvo's release says. Try again." });
    assert.deepEqual(everything(path.join(staging(two), "engine")), [], "no engine kept, nor what was fetched");
    // And one that can't be had (offline): said so, to try again.
    fs.rmSync(engineTar);
    const offline = install(two, "status");
    assert.equal(offline.status, 1);
    assert.match(offline.said.message, /^The engine couldn't be downloaded\. Check that this (Mac|computer) is online, then try again\.$/);
  } finally {
    fs.writeFileSync(engineTar, engineBytes);
  }
  // Tried again once it can be: it goes through.
  assert.equal(install(two, "status").said.verdict, "fresh");
});

test("beside a box that's here already, nothing is made, and the engine the installer fetched goes once that box has it itself", () => {
  const where = computer();
  assert.equal(install(where, "status").said.verdict, "fresh");
  // A machine's box set up here meanwhile, as another computer leaves it, with the same engine.
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(where.K, "engine", ENGINE, "opencode"), engineBinary, 0o755);
  const status = install(where, "status");
  assert.equal(status.status, 0, status.stderr);
  assert.equal(status.said.verdict, "machine");
  assert.equal(status.said.verb, null);
  assert.equal(exists(path.join(staging(where), "engine")), false, "one copy of the engine, the box's own");
  // Asked again, on the box's own engine: nothing fetched.
  const again = install(where, "status");
  assert.deepEqual(again.steps, []);
  assert.equal(again.said.verdict, "machine");
  assert.equal(exists(path.join(staging(where), "engine")), false);
  const refused = install(where, "new");
  assert.equal(refused.status, 1);
  assert.equal(refused.said.result, "refused");
  assert.equal(refused.said.message, "This computer is set up as one of your machines already: Kuvo uses what's here.");
  assert.equal(read(path.join(where.K, "bin", "kuvo-start")), "#!/bin/sh\n", "its files as they were");
});

test("an existing box updates through its fleet instead of the install command", () => {
  const where = computer();
  write(path.join(where.K, "bin", "box-version"), "0.9.0");
  const ran = install(where, "update");
  assert.equal(ran.status, 2);
  assert.equal(read(path.join(where.K, "bin", "box-version")), "0.9.0");
  assert.equal(exists(staging(where)), false);
});

test("a computer whose box has another engine than the release's is still told how it stands, offline: asking never waits on a download", () => {
  const where = computer();
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(where.K, "engine", "1.0.0", "opencode"), engineBinary, 0o755);
  fs.rmSync(engineTar);
  try {
    const status = install(where, "status");
    assert.equal(status.status, 0, status.stderr + JSON.stringify(status.lines));
    assert.deepEqual(status.steps, ["checking"], "nothing fetched");
    assert.equal(status.said.verdict, "machine");
    assert.equal(exists(path.join(staging(where), "engine")), false);
  } finally {
    fs.writeFileSync(engineTar, engineBytes);
  }
});

test("new on a Mac: this Mac's own box made in its home from the release and its engine, started by Kuvo Helper, the installer's staging gone", { skip: !mac && "a Mac's box" }, () => {
  const where = computer();
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  assert.equal(made.said.verdict, "ours");
  assert.equal(made.said.started, true);
  // Its files from the release, its helper whole, the release's engine, and its kind: every file of
  // the release, as its box.json says, the engine's plugin outside `bin` too (which gives the
  // commands agents run your own folders back).
  const box = JSON.parse(read(path.join(scratch, "unpacked-box.json")));
  for (const [name, sum] of Object.entries(box.files)) {
    assert.equal(crypto.createHash("sha256").update(fs.readFileSync(path.join(where.K, name))).digest("hex"), sum, name);
  }
  assert.equal(read(path.join(where.K, "bin", "box-version")), VERSION);
  assert.deepEqual(fs.readFileSync(path.join(where.K, "bin", "kuvo-link.js")), fs.readFileSync(script));
  assert.equal(spawnSync("codesign", ["--verify", "--strict", path.join(where.K, "bin", "Kuvo Helper.app")]).status, 0, "the helper, signed as it was");
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
  // Started at login and now: its launch agent names the helper in this home, under the app.
  const plist = read(path.join(where.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"));
  assert.ok(plist.includes(`<string>${path.join(where.K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper")}</string><string>keep</string><string>--home</string><string>${where.K}</string>`), plist);
  assert.ok(plist.includes(`<key>AssociatedBundleIdentifiers</key><array><string>${APP_ID}</string></array>`), plist);
  assert.match(read(path.join(where.home, "launchctl-asked")), /^bootstrap gui\/\d+ .*run\.kuvo\.code\.machine\.plist$/m);
  // Nothing of the installer's left, and the old app's folder never made.
  assert.equal(exists(staging(where)), false);
  assert.equal(exists(where.S), false);
  // From now on: ours, with nothing to run; asked to make it again, nothing.
  const status = install(where, "status");
  assert.equal(status.said.verdict, "ours");
  assert.equal(status.said.verb, null);
  const again = install(where, "new");
  assert.equal(again.status, 0, again.stderr);
  assert.equal(again.said.result, "nothing");
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
});

test("a Mac's install cut short is made again: what a making left, and a box made whose helper never got its launch agent", { skip: !mac && "a Mac's box" }, () => {
  // Cut short while its files went in: the mark of a making, half a `bin`, the scratch copies,
  // and an unpacking and a download that never finished.
  const cut = computer();
  write(path.join(cut.K, "updates", "rehome", "new"), "1\n");
  write(path.join(cut.K, "bin", "kuvo-link.js"), "// half of it");
  write(path.join(cut.K, "bin.new", "kuvo-start"), "half");
  write(path.join(staging(cut), "release.new", "bin", "kuvo-link.js"), "// half of it");
  write(path.join(staging(cut), "engine", ".fetch.1", "engine.tgz"), "half");
  const status = install(cut, "status");
  assert.equal(status.said.verdict, "fresh");
  const made = install(cut, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  // What the making cut short left is set aside, and the answer says where.
  assert.match(made.said.setAside, new RegExp(`^${cut.K}/trash/earlier-box-`));
  assert.equal(read(path.join(made.said.setAside, "bin", "kuvo-link.js")), "// half of it");
  assert.deepEqual(fs.readFileSync(path.join(cut.K, "bin", "kuvo-link.js")), fs.readFileSync(script), "the release's, whole");
  assert.equal(exists(path.join(cut.K, "bin.new")), false);
  assert.equal(read(path.join(cut.K, "box", "kind")), "mac\n");

  // Cut short between the box and its helper: made, with nothing to start it. Status says to
  // run `new`, which only puts the helper in place.
  const unstarted = computer();
  assert.equal(install(unstarted, "new").said.result, "done");
  fs.rmSync(path.join(unstarted.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"));
  fs.rmSync(path.join(unstarted.home, "loaded"));
  write(path.join(unstarted.K, "chats", "c1", "notes.md"), "kept");
  const asked = install(unstarted, "status");
  assert.equal(asked.said.verdict, "ours");
  assert.equal(asked.said.verb, "new");
  const finished = install(unstarted, "new");
  assert.equal(finished.status, 0, finished.stderr + JSON.stringify(finished.lines));
  assert.equal(finished.said.result, "nothing", "the box as it was");
  assert.equal(finished.said.started, true);
  assert.equal(exists(path.join(unstarted.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), true);
  assert.equal(read(path.join(unstarted.K, "chats", "c1", "notes.md")), "kept");
  assert.equal(install(unstarted, "status").said.verb, null);

  // Its launch agent written, but launchd wouldn't take it: said as failed, never as started, and
  // `new` is still what to run. Tried again once launchd takes it, it's started.
  const refused = computer();
  write(path.join(refused.home, "launchd-refuses"), "");
  const failed = install(refused, "new");
  assert.equal(failed.status, 1);
  assert.equal(failed.said.result, "failed");
  assert.equal(failed.said.started, false);
  assert.match(failed.said.message, /launchd didn't start it now: Bootstrap failed/);
  assert.equal(read(path.join(refused.K, "box", "kind")), "mac\n", "the box is made");
  assert.equal(exists(path.join(refused.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), true);
  assert.deepEqual({ verdict: install(refused, "status").said.verdict, verb: install(refused, "status").said.verb }, { verdict: "ours", verb: "new" });
  fs.rmSync(path.join(refused.home, "launchd-refuses"));
  const retried = install(refused, "new");
  assert.equal(retried.status, 0, retried.stderr + JSON.stringify(retried.lines));
  assert.deepEqual({ result: retried.said.result, started: retried.said.started }, { result: "nothing", started: true });
  assert.equal(exists(path.join(refused.home, "loaded")), true);
  assert.equal(install(refused, "status").said.verb, null);
});

test("a Mac that holds an earlier Kuvo Code's sessions gets no new box beside them: they move instead", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.S, "opencode", "data", "opencode", "opencode.db"), "sessions");
  const status = install(where, "status");
  assert.equal(status.said.verdict, "app-07");
  assert.equal(status.said.verb, "move");
  const refused = install(where, "new");
  assert.equal(refused.status, 1);
  assert.equal(refused.said.result, "refused");
  assert.equal(refused.said.message, "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.");
  assert.deepEqual(fs.readdirSync(where.K), ["updates"], "no box made");
  assert.equal(exists(path.join(where.home, "Library", "LaunchAgents")), false);
  // (Sessions only Kuvo's own engine can read: here the engine is a stand-in, which says so and changes nothing.)
  const move = install(where, "move");
  assert.equal(move.status, 1);
  assert.deepEqual({ result: move.said.result, verdict: move.said.verdict, message: move.said.message }, { result: "refused", verdict: "app-07", message: "Your sessions can only be moved by Kuvo's own engine." });
  assert.equal(read(path.join(where.S, "opencode", "data", "opencode", "opencode.db")), "sessions");
  assert.equal(exists(path.join(where.K, "box")), false, "no box made");
});

// `kuvo-keys`, as a stand-in the app carries: it notes what it's asked, and like the real one does
// nothing unless the move's lock names the very process that started it.
const keysStandIn = `#!/bin/sh
verb=$1; shift; H=""
while [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; *) shift ;; esac; done
[ "$(cat "$H/updates/rehome/lock/pid" 2>/dev/null)" = "$PPID" ] || { echo "not the move" >&2; exit 2; }
echo "$verb" >>"$HOME/keys-asked"
case "$verb" in copy) echo '{"copied":0}' ;; passwords) echo '{"v":1,"logins":[]}' ;; *) echo '{}' ;; esac
`;

test("install move progress stays in order when rehome is delayed between finishing and settling", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  // Hold the child after publishing each journal step until the installer's next poll has
  // finished reading it. At "done", the journal no longer provides progress and run.phase
  // still says "plan". Both the journal and phase are written by the real move.
  const schedule = path.join(where.home, "schedule.cjs");
  write(schedule, `
const fs = require("node:fs");
const path = require("node:path");
const journal = path.join(process.env.KUVO_HOME || "", "updates", "rehome", "journal.json");
const observed = step => path.join(process.env.HOME, "observed-" + step);
const steps = ["clear", "done"];
if (process.argv[2] === "install") {
  const read = fs.readFileSync;
  let previous;
  fs.readFileSync = function (file, ...args) {
    const text = read.call(this, file, ...args);
    if (file === journal) {
      const { step } = JSON.parse(text);
      if (steps.includes(step) && step === previous) fs.writeFileSync(observed(step), "");
      previous = step;
    }
    return text;
  };
}
if (process.argv[2] === "rehome" && process.argv.includes("--run")) {
  const rename = fs.renameSync;
  const nap = new Int32Array(new SharedArrayBuffer(4));
  fs.renameSync = function (from, to) {
    rename.call(this, from, to);
    if (to !== journal) return;
    const { step } = JSON.parse(fs.readFileSync(journal, "utf8"));
    if (!steps.includes(step)) return;
    const deadline = Date.now() + 10_000;
    while (!fs.existsSync(observed(step))) {
      if (Date.now() >= deadline) throw new Error("Installer did not poll " + step);
      Atomics.wait(nap, 0, 0, 10);
    }
  };
}
`);
  const moved = install(where, "move", { env: { NODE_OPTIONS: `--require "${schedule}"` } });
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.equal(moved.said.result, "done");
  assert.ok(exists(path.join(where.home, "observed-done")));
  assert.ok(moved.steps.includes("clear"), JSON.stringify(moved.lines));
  const indices = moved.lines.filter(line => line.index !== undefined).map(line => line.index);
  assert.deepEqual(indices, [...indices].sort((a, b) => a - b), JSON.stringify(moved.lines));
});

test("an earlier Kuvo Code's chats and settings move with `kuvo-install move`: each step said in the box's own words, its helper started, and the app's own folder as it was", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.bundle, "Contents", "MacOS", "kuvo-keys"), keysStandIn, 0o755);
  write(path.join(where.home, "Library", "Keychains", "login.keychain-db"), "");
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  write(path.join(where.S, "bots.json"), JSON.stringify([{ id: "bot_a1", name: "Scout", job: "Watch", instructions: "Watch.", look: {}, folder: path.join(where.S, "bots", "bot_a1"), sessionID: "ses_b", created: "2026-10-01T10:00:00Z" }]));
  write(path.join(where.S, "bots", "bot_a1", "MEMORY.md"), "what Scout remembers");
  write(path.join(where.S, "Browser", "history.json"), "[]");
  const before = everything(where.S).map(name => [name, fs.statSync(path.join(where.S, name)).isFile() ? read(path.join(where.S, name)) : ""]);
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb }, { verdict: "app-07", verb: "move" });

  const moved = install(where, "move");
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.deepEqual({ result: moved.said.result, verdict: moved.said.verdict, started: moved.said.started }, { result: "done", verdict: "ours", started: true });
  assert.equal(moved.said.message, undefined, "started: nothing more to say");
  assert.equal(/\bbox\b|Kuvo Core/i.test(JSON.stringify(moved.lines.map(line => [line.title, line.message, line.status?.dropped]))), false, "no word of the move's own workings");
  // Its steps, as the box says them: which, in words to show, where it is and of how many.
  const steps = moved.lines.filter(line => line.title);
  assert.deepEqual(steps[0], { step: "plan", title: "Checking this Mac", index: 1, of: 11 });
  assert.ok(steps.length >= 2 && steps.every(step => typeof step.title === "string" && step.of === 11 && step.index >= 1 && step.index <= 11), JSON.stringify(steps));
  assert.deepEqual(steps.map(step => step.index), [...steps.map(step => step.index)].sort((a, b) => a - b), "in order");
  // Moved: a Mac's own box, with the chat and the bot (described as the box reads one), its helper
  // loaded, this Mac's keys asked of `kuvo-keys` by the move itself.
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
  assert.equal(read(path.join(where.K, "chats", "c1", "notes.md")), "a chat's notes");
  assert.equal(JSON.parse(read(path.join(where.K, "bots", "bot_a1", "bot.json"))).name, "Scout");
  assert.equal(read(path.join(where.K, "bots", "bot_a1", "directory")), path.join(where.K, "bots", "bot_a1", "workspace"));
  assert.equal(read(path.join(where.K, "bin", "box-version")), VERSION);
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(exists(path.join(where.home, "loaded")), true);
  // Its saved sign-ins, never read by the move (macOS may ask about each): your Keychain lists
  // none, by what's listed alone, so none are owed.
  assert.deepEqual(read(path.join(where.home, "keys-asked")).trim().split("\n"), ["copy", "delete-originals"]);
  assert.deepEqual(read(path.join(where.home, "security-asked")).trim().split("\n"), [`find-internet-password -c KUVO ${path.join(where.home, "Library", "Keychains", "login.keychain-db")}`]);
  assert.deepEqual(moved.said.status.owed, []);
  assert.deepEqual(moved.said.status.dropped, ["Websites you were signed in to in Kuvo Code's browser need signing in again. Your history comes along."]);
  // The earlier Kuvo Code's folder: every file as it was. Nothing the installer staged is left.
  assert.deepEqual(everything(where.S).map(name => [name, fs.statSync(path.join(where.S, name)).isFile() ? read(path.join(where.S, name)) : ""]), before);
  assert.equal(exists(staging(where)), false);
  // Asked again as the app next opens: this Mac's own box, nothing to run, nothing moved twice.
  const after = install(where, "status");
  assert.deepEqual({ verdict: after.said.verdict, verb: after.said.verb }, { verdict: "ours", verb: null });
  const again = install(where, "move");
  assert.deepEqual({ status: again.status, result: again.said.result }, { status: 0, result: "nothing" });
  // Stopped until it next starts (Stop Kuvo on this Mac, #761): its helper went, its launch agent
  // still loaded. The app opening starts it again: `new` runs, which starts the helper.
  write(path.join(where.K, "run", "paused"), "");
  const stopped = install(where, "status");
  assert.deepEqual({ verdict: stopped.said.verdict, verb: stopped.said.verb }, { verdict: "ours", verb: "new" });
  fs.rmSync(path.join(where.home, "launchctl-asked"), { force: true });
  const started = install(where, "new");
  assert.deepEqual({ status: started.status, started: started.said.started }, { status: 0, started: true }, started.stderr);
  assert.match(read(path.join(where.home, "launchctl-asked")), /^kickstart gui\/\d+\/run\.kuvo\.code\.machine$/m);
});

test("a move launchd wouldn't start: opening Kuvo again starts it with `new`, never the move again; what it still owes the box settles as it runs", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.bundle, "Contents", "MacOS", "kuvo-keys"), keysStandIn, 0o755);
  write(path.join(where.home, "Library", "Keychains", "login.keychain-db"), "");
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  write(path.join(where.home, "launchd-refuses"), "");
  const moved = install(where, "move");
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.deepEqual({ result: moved.said.result, started: moved.said.started, owed: moved.said.status.owed }, { result: "done", started: false, owed: ["start", "keychain", "passwords"] });
  // (In words to act on: never the helper's own, nor what launchd answered.)
  assert.equal(moved.said.message, "Your sessions are moved, but Kuvo couldn't be started on this Mac. Try again.");
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb }, { verdict: "ours", verb: "new" });
  fs.rmSync(path.join(where.home, "launchd-refuses"));
  const started = install(where, "new");
  assert.equal(started.status, 0, started.stderr + JSON.stringify(started.lines));
  assert.deepEqual({ result: started.said.result, started: started.said.started, owed: started.said.status.owed }, { result: "nothing", started: true, owed: ["keychain", "passwords"] });
  assert.equal(install(where, "status").said.verb, null);
  // The box, as it runs: the Keychain's originals taken away, and with your Keychain found
  // unlocked, its saved sign-ins listed (none here).
  const settled = boxRehome(where, ["--settle"]);
  assert.equal(settled.status, 0, settled.stderr);
  assert.deepEqual({ result: settled.said.result, owed: settled.said.status.owed }, { result: "done", owed: [] });
  assert.deepEqual(read(path.join(where.home, "keys-asked")).trim().split("\n"), ["copy", "delete-originals"]);
});

test("a moved Mac whose saved sign-ins are still owed opens and updates without the move or the Keychain: they wait for you, and only you start them", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.bundle, "Contents", "MacOS", "kuvo-keys"), keysStandIn, 0o755);
  write(path.join(where.home, "Library", "Keychains", "login.keychain-db"), "");
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  // Your Keychain lists some.
  write(path.join(where.home, "saved-sign-ins"), "");
  const moved = install(where, "move");
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.deepEqual({ result: moved.said.result, started: moved.said.started, owed: moved.said.status.owed, signIns: moved.said.status.signIns }, { result: "done", started: true, owed: ["passwords"], signIns: null });
  assert.equal(moved.said.message, undefined, "nothing asked of them while it moves");
  assert.ok(moved.said.status.dropped.includes("Passwords the earlier Kuvo Code saved for websites stay in your Keychain for now: they aren't in Kuvo's vault yet."), JSON.stringify(moved.said.status.dropped));
  // The app opening again, after an update or not: nothing to run, and the move, asked all the
  // same (an older app would), does nothing. Neither asks kuvo-keys anything.
  for (let open = 0; open < 2; open++) {
    const status = install(where, "status");
    assert.deepEqual({ verdict: status.said.verdict, verb: status.said.verb }, { verdict: "ours", verb: null });
    const again = install(where, "move");
    assert.deepEqual({ status: again.status, result: again.said.result, owed: again.said.status.owed }, { status: 0, result: "nothing", owed: ["passwords"] });
  }
  // Nor what the box settles as it runs: they stay owed, unasked.
  assert.deepEqual(boxRehome(where, ["--settle"]).said.status.owed, ["passwords"]);
  assert.deepEqual(read(path.join(where.home, "keys-asked")).trim().split("\n"), ["copy", "delete-originals"]);
});

test("a Mac that's one of your machines too is told so in words by the installer, and nothing is moved or stopped", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  // A machine's box in its home, as another Mac set it up.
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  // (Nothing to run, and said in words: its earlier sessions stay where they are, unseen.)
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb, message: install(where, "status").said.message }, { verdict: "two", verb: null, message: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet." });
  const refused = install(where, "move");
  assert.equal(refused.status, 1);
  assert.deepEqual({ result: refused.said.result, verdict: refused.said.verdict, message: refused.said.message }, { result: "refused", verdict: "two", message: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet." });
  assert.equal(exists(path.join(where.K, "box", "kind")), false);
  assert.equal(exists(path.join(where.K, "chats")), false);
  assert.equal(exists(path.join(where.home, "launchctl-asked")), false);
});

test("on Linux there's nothing to move: said, and nothing made", { skip: mac && "Linux has no app's folder" }, () => {
  const where = computer();
  const refused = install(where, "move");
  assert.equal(refused.status, 1);
  assert.deepEqual({ result: refused.said.result, message: refused.said.message }, { result: "refused", message: "There's nothing to move on this computer: Kuvo is where it belongs already." });
  assert.equal(exists(path.join(where.K, "bin")), false);
});

test("a copy of the app never judges or installs for the real one, and is refused before anything is written: Kuvo Code's own ID goes with ~/.kuvo-code alone", { skip: !mac && "a Mac's app" }, () => {
  for (const verb of ["status", "new", "move"]) {
    const where = computer({ app: "run.kuvo.code" });
    const refused = install(where, verb);
    assert.equal(refused.status, 1);
    assert.deepEqual(refused.said, { result: "refused", message: "Kuvo itself sets up ~/.kuvo-code only." });
    assert.equal(exists(where.K), false, "nothing written, not even its folder");
    assert.deepEqual(fs.readdirSync(where.home).sort(), ["Applications", "tools"]);
  }
  // And with no app named, or one that isn't an app, a Mac's installer says what it needs.
  const unnamed = computer();
  for (const args of [[], ["--app", path.join(unnamed.home, "No Such.app")]]) {
    const asked = install(unnamed, "status", { args });
    assert.equal(asked.status, 1);
    assert.deepEqual(asked.said, { result: "refused", message: "Kuvo couldn't tell which app started it. Open Kuvo again." });
    assert.equal(exists(unnamed.K), false);
  }
});

test("the real home is told however it's written: on a Mac's disk a name in other letters' case is the same folder, there or not yet", { skip: !mac && "a Mac's disk" }, () => {
  // The installer's own rule, asked of folders of the test's own (never of the real home).
  const text = read(path.join(release, "kuvo-install"));
  const rule = text.slice(text.indexOf("same_place() {"), text.indexOf("\n}\n", text.indexOf("same_place() {")) + 3);
  const same = (a, b) => spawnSync("sh", ["-c", `${rule}\nsame_place "$1" "$2"`, "sh", a, b]).status === 0;
  const home = path.join(scratch, "case");
  fs.mkdirSync(path.join(home, ".kuvo-code"), { recursive: true });
  fs.mkdirSync(path.join(home, "other"));
  const insensitive = exists(path.join(home, ".KUVO-CODE"));
  assert.equal(same(path.join(home, ".kuvo-code"), path.join(home, ".kuvo-code")), true);
  assert.equal(same(path.join(home, ".Kuvo-Code"), path.join(home, ".kuvo-code")), insensitive, "the same folder, as the disk has it");
  assert.equal(same(path.join(home, "other"), path.join(home, ".kuvo-code")), false);
  // Neither there yet: by name, without regard to case.
  assert.equal(same(path.join(home, "none", ".Kuvo-Code"), path.join(home, "none", ".kuvo-code")), true);
  assert.equal(same(path.join(home, "none", ".kuvo-demo"), path.join(home, "none", ".kuvo-code")), false);
});

test("new on Linux: a machine's box in ~/.kuvo-code, its files and engine in place and started as a computer that's added is; one cut short is made again", { skip: mac && "a Linux computer's box" }, () => {
  const where = computer();
  assert.deepEqual(install(where, "status").said.verdict, "fresh");
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  assert.equal(made.said.verdict, "machine");
  assert.equal(made.said.started, true);
  // Every file of the release, as its box.json says, with who may read and run each.
  const box = JSON.parse(read(path.join(scratch, "unpacked-box.json")));
  for (const [name, sum] of Object.entries(box.files)) {
    assert.equal(crypto.createHash("sha256").update(fs.readFileSync(path.join(where.K, name))).digest("hex"), sum, name);
  }
  assert.equal(modeOf(path.join(where.K, "bin", "kuvo-link.js")), "600");
  assert.equal(modeOf(path.join(where.K, "bin", "kuvo-start")), "700");
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(exists(path.join(where.K, "bin", "Kuvo Helper.app")), false, "no helper on Linux");
  // Started as a machine is: how its engine starts noted, its id made, its line in the crontab
  // (the test's stand-in), and its own keeper asked to start.
  assert.match(read(path.join(where.K, "run", "launch.env")), /^PATH=/m);
  assert.match(read(path.join(where.K, "machine-id")), /^[0-9a-f]{32}$/);
  assert.match(read(path.join(where.home, "crontab")), /^@reboot sh "\$HOME\/\.kuvo-code\/bin\/kuvo-start" .*# kuvo-code: starts its engine again after a restart$/m);
  assert.equal(read(path.join(where.home, "keeper-asked")), "start\n");
  assert.equal(exists(staging(where)), false);
  // From now on a machine's box is here: nothing made over it.
  assert.equal(install(where, "status").said.verdict, "machine");
  const again = install(where, "new");
  assert.equal(again.status, 1);
  assert.equal(again.said.result, "refused");
  assert.equal(read(path.join(where.home, "keeper-asked")), "start\n", "nothing started twice");

  // Cut short after its files went in, before it started: fresh still, and made again whole.
  const cut = computer();
  install(cut, "status");
  write(path.join(staging(cut), "making"), "1\n");
  write(path.join(cut.K, "bin", "kuvo-start"), "half", 0o700);
  assert.equal(install(cut, "status").said.verdict, "fresh");
  const remade = install(cut, "new");
  assert.equal(remade.status, 0, remade.stderr + JSON.stringify(remade.lines));
  assert.equal(read(path.join(cut.home, "keeper-asked")), "start\n");
  assert.notEqual(read(path.join(cut.K, "bin", "kuvo-start")), "half");

  // A box the computer that set it up removed (turned off, its sessions left in its folder) is no
  // box: a new one is made, and what was there is set aside in its trash, nothing deleted. The
  // desktop app's own folder there stays where it is.
  const removed = computer();
  write(path.join(removed.K, "run", "off"), "");
  write(path.join(removed.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(removed.K, "chats", "c1", "notes.md"), "kept");
  write(path.join(removed.K, "desktop", "identity"), "the app's own");
  assert.equal(install(removed, "status").said.verdict, "fresh");
  const anew = install(removed, "new");
  assert.equal(anew.status, 0, anew.stderr + JSON.stringify(anew.lines));
  assert.equal(anew.said.verdict, "machine");
  const aside = fs.readdirSync(path.join(removed.K, "trash")).filter(name => name.startsWith("earlier-box-"));
  assert.equal(aside.length, 1);
  assert.equal(anew.said.setAside, path.join(removed.K, "trash", aside[0]), "the answer says where it went");
  assert.equal(made.said.setAside, undefined, "and says nothing where nothing was there");
  assert.equal(read(path.join(removed.K, "trash", aside[0], "chats", "c1", "notes.md")), "kept");
  assert.equal(exists(path.join(removed.K, "trash", aside[0], "run", "link.json")), true, "its old link with it, never the new box's");
  assert.equal(exists(path.join(removed.K, "run", "off")), false);
  assert.equal(exists(path.join(removed.K, "chats")), false);
  assert.equal(read(path.join(removed.K, "desktop", "identity")), "the app's own");
});

test("on Linux an install stays complete when retired staging cannot be deleted", { skip: mac && "a Linux computer's box" }, () => {
  const where = computer();
  const updates = path.join(where.K, "updates");
  const abandoned = path.join(staging(where), "abandoned");
  write(path.join(abandoned, "unfinished"), "discard this");
  fs.chmodSync(abandoned, 0o500);
  try {
    const made = install(where, "new");
    assert.equal(read(path.join(where.home, "keeper-asked")), "start\n");
    assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
    assert.deepEqual([made.said.result, made.said.verdict, made.said.started], ["done", "machine", true]);
    assert.equal(exists(staging(where)), false);
    const retired = fs.readdirSync(updates).find(folder => folder.startsWith("retired-install-"));
    assert.ok(retired, "the old staging must be retired even when deletion fails");
    assert.equal(read(path.join(updates, retired, "abandoned/unfinished")), "discard this");
    assert.equal(install(where, "status").said.verdict, "machine");
  } finally {
    for (const folder of fs.readdirSync(updates)) {
      const old = path.join(updates, folder, "abandoned");
      if (exists(old)) fs.chmodSync(old, 0o700);
    }
  }
});

test("on Linux nothing is made or set aside under something that still runs in the folder", { skip: mac && "a Linux computer's box" }, async () => {
  // A box turned off whose keeper still runs (it's ending): by its record and its command line.
  const where = computer();
  const keeper = spawn(process.execPath, ["-e", "process.stdin.resume()", "kuvo-start"], { stdio: ["pipe", "ignore", "ignore"] });
  try {
    write(path.join(where.K, "run", "off"), "");
    write(path.join(where.K, "run", "box.pid"), `${keeper.pid}\n`);
    write(path.join(where.K, "chats", "c1", "notes.md"), "kept");
    const refused = install(where, "new");
    assert.equal(refused.status, 1);
    assert.equal(refused.said.result, "refused");
    assert.equal(refused.said.message, "Something is running in this computer's ~/.kuvo-code: Kuvo is installed there once that's stopped.");
    assert.equal(read(path.join(where.K, "chats", "c1", "notes.md")), "kept");
    assert.equal(exists(path.join(where.K, "trash")), false);
    assert.equal(exists(path.join(where.K, "bin")), false);
  } finally {
    const gone = new Promise(resolve => keeper.once("exit", resolve));
    keeper.kill();
    await gone;
  }
  // Once it's stopped, it's made.
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(read(path.join(made.said.setAside, "chats", "c1", "notes.md")), "kept");
});

test("on Linux a start that fails after its keeper wrote keys and a link is still not a box: `new` is what to run, and it's made again whole", { skip: mac && "a Linux computer's box" }, () => {
  const where = computer();
  write(path.join(where.home, "keeper-fails"), "");
  const failed = install(where, "new");
  assert.equal(failed.status, 1);
  assert.equal(failed.said.result, "failed");
  assert.equal(failed.said.started, false);
  assert.match(failed.said.message, /couldn't be started: its engine didn't come up\. Try again\.$/);
  const failedLink = read(path.join(where.K, "run", "link.json"));
  assert.ok(failedLink, "what its start left");
  const status = install(where, "status");
  assert.deepEqual({ verdict: status.said.verdict, verb: status.said.verb }, { verdict: "fresh", verb: "new" });
  fs.rmSync(path.join(where.home, "keeper-fails"));
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.deepEqual({ result: made.said.result, verdict: made.said.verdict, started: made.said.started }, { result: "done", verdict: "machine", started: true });
  assert.equal(read(path.join(made.said.setAside, "run", "link.json")), failedLink, "the half-made one's link is archived verbatim");
  const freshLink = JSON.parse(read(path.join(where.K, "run", "link.json")));
  assert.equal(freshLink.setup.role, "primary");
  assert.ok(freshLink.setup.id);
  assert.equal(freshLink.mac, undefined, "the fresh setup does not inherit the earlier link");
  assert.equal(install(where, "status").said.verdict, "machine");
});

// (What the pack's box.json names, for the Linux test: read from the pack itself.)
{
  const unpacked = path.join(scratch, "unpacked");
  fs.mkdirSync(unpacked);
  assert.equal(spawnSync("tar", ["xzf", path.join(release, `box-${VERSION}.tar.gz`), "-C", unpacked]).status, 0);
  fs.copyFileSync(path.join(unpacked, "box.json"), path.join(scratch, "unpacked-box.json"));
}
