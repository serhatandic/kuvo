// Signed release and packaging checks. Fleet activation is exercised in fleet-switch.test.js.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const boxHome = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-release-home-"));
const home = path.join(boxHome, ".kuvo-code");
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);
const script = fileURLToPath(LINK_BUNDLE);
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-release-"));
removeAtEnd(boxHome);
removeAtEnd(scratch);
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const sha256 = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
}

test("versions order by number, a beta before its release", () => {
  assert.equal(link.newerVersion("0.10.0", "0.9.3"), true);
  assert.equal(link.newerVersion("0.9.3", "0.9.3"), false);
  assert.equal(link.newerVersion("0.8.0", "0.8.0-beta.2"), true);
  assert.equal(link.newerVersion("0.8.0-beta.3", "0.8.0-beta.2"), true);
  assert.equal(link.newerVersion("0.8.0-beta.1", "0.8.0"), false);
  assert.equal(link.newerVersion("0.8.0", null), true, "a box nothing marked takes any");
  assert.equal(link.newerVersion("not a version", "0.1.0"), false);
});

test("a release counts only signed with a pinned key, over what it says", () => {
  const bytes = Buffer.from(JSON.stringify({ kuvo: "box/1", version: "0.10.0", engine: "1.1.0",
    pack: { url: "https://example.invalid/box-0.10.0.tar.gz", sha256: "a".repeat(64) } }));
  const signed = JSON.stringify({ manifest: bytes.toString("base64"), key: pinned,
    signature: crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), bytes]), privateKey).toString("base64") });
  const manifest = link.verifyBoxes(signed, [pinned]);
  assert.equal(manifest.version, "0.10.0");
  assert.throws(() => link.verifyBoxes(signed, []), /isn't signed with a key/);
  assert.throws(() => link.verifyBoxes(signed, [crypto.generateKeyPairSync("ed25519").publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64")]), /isn't signed with a key/);
  const given = JSON.parse(signed);
  const changed = Buffer.from(given.manifest, "base64").toString("utf8").replace("0.10.0", "0.99.0");
  assert.throws(() => link.verifyBoxes(JSON.stringify({ ...given, manifest: Buffer.from(changed).toString("base64") }), [pinned]), /signature doesn't match/);
  // The same key's signature on the bytes alone (as for something else it signs) doesn't count.
  const bare = crypto.sign(null, Buffer.from(given.manifest, "base64"), privateKey).toString("base64");
  assert.throws(() => link.verifyBoxes(JSON.stringify({ ...given, signature: bare }), [pinned]), /signature doesn't match/);
});

test("stale anonymous and named Mac launcher marks do not block the computer’s own updater", () => {
  const own = path.join(os.homedir(), ".kuvo-code");
  assert.match(link.updatesBlocked({}, Date.now(), home), /own Kuvo Code folder/);
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  const now = Date.now();
  write(path.join(home, "run", "older-mac"), "");
  write(path.join(home, "run", "launchers.json"), JSON.stringify({ "an older Mac": now - 26 * 86_400_000, "mac-old": now }));
  assert.equal(link.updatesBlocked({}, now, own), null);
  assert.equal(link.updatesBlocked({ tolerantIDs: { "mac-old": false } }, now, own), null);
  fs.rmSync(path.join(home, "run", "older-mac"), { force: true });
  fs.rmSync(path.join(home, "run", "launchers.json"), { force: true });
});

test("a rollback that gave up with some of each version's files left says so, once, wherever it's looked at", () => {
  write(path.join(home, "run", "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.10.0", at: Date.now() } }));
  fs.mkdirSync(path.join(home, "updates", "rollback", "files"), { recursive: true });
  link.updateStatus();
  const state = read(path.join(home, "run", "update.json"));
  assert.equal(state.trial, undefined);
  assert.equal(state.last.result, "failed");
  assert.equal(state.last.message, "0.10.0 didn't run well here, and not all of it could go back to 0.9.0: Kuvo updates again later. Your conversations stay as they are.");
  fs.rmSync(path.join(home, "updates", "rollback"), { recursive: true });
});

const checkout = fileURLToPath(new URL("../..", import.meta.url));
// The browser as its build leaves it: a first line naming the rest.
const browserBody = "// kuvo-browser.js\n";
const builtBrowser = `// kuvo-browser ${sha256(browserBody).slice(0, 16)}\n${browserBody}`;
function sources() {
  const root = path.join(scratch, `sources-${crypto.randomUUID()}`);
  for (const file of ["box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  write(path.join(root, "box/src/keeper/kuvo-start"), fs.readFileSync(path.join(checkout, "box/src/keeper/kuvo-start")));
  write(path.join(root, "box/dist/kuvo-link.js"), fs.readFileSync(script));
  write(path.join(root, "box/dist/kuvo-agents.js"), "// kuvo-agents.js\n");
  write(path.join(root, "box/dist/kuvo-browser.js"), builtBrowser);
  const integrity = "sha512-" + crypto.createHash("sha512").update("fixture").digest("base64");
  const engine = { url: "https://example.invalid/engine-1.1.0.tgz", integrity };
  const builds = { x86_64: engine, aarch64: engine };
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.1.0", linux: builds, darwin: builds }));
  return root;
}
// Kuvo Helper as its build leaves it (scripts/build-helper.sh): on a Mac a program signed ad hoc,
// as packing checks the signature where codesign is.
function builtHelper() {
  const bundle = path.join(scratch, `helper-${crypto.randomUUID()}`, "Kuvo Helper.app");
  const program = path.join(bundle, "Contents", "MacOS", "Kuvo Helper");
  if (process.platform === "darwin") {
    write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Kuvo Helper</string><key>CFBundleIdentifier</key><string>run.kuvo.code.tests.box</string></dict></plist>\n`);
    fs.mkdirSync(path.dirname(program), { recursive: true });
    fs.copyFileSync("/usr/bin/true", program);
    fs.chmodSync(program, 0o755);
    const signed = spawnSync("codesign", ["--force", "--sign", "-", bundle], { encoding: "utf8" });
    assert.equal(signed.status, 0, signed.stderr);
  } else {
    write(path.join(bundle, "Contents", "Info.plist"), "<plist/>");
    write(program, "#!/bin/sh\nexit 0\n", 0o755);
  }
  return { bundle, program };
}
// The Bun the release's files are written with: the one named, the one on the PATH, or (run by
// Bun) this one.
const bun = process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun");
const boxRelease = (env, ...args) => spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), ...args], {
  env: { ...process.env, BUN: bun, NODE: process.execPath, ...env }, encoding: "utf8",
});
const signingKey = privateKey.export({ type: "pkcs8", format: "pem" });

test("scripts/box-release.sh packs the box's release from its sources with its helper, signed, and the box keeps it", async () => {
  write(path.join(home, "bin", "box-version"), "0.9.0");
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  const root = sources();
  const { bundle } = builtHelper();
  // As an app carries it (`Contents/Resources/box-release`), or any folder.
  const out = path.join(scratch, `box-release-${crypto.randomUUID()}`);
  const pack = (...args) => boxRelease({ KUVO_BOX_SIGNING_KEY: signingKey, KUVO_BOX_SPREADS: "1", BOX_SOURCES: root }, ...args);
  const wrongURL = pack("0.10.0", "https://example.invalid/pack.tgz", out, bundle);
  assert.equal(wrongURL.status, 2, "a box finds the pack by the name its URL ends in");
  assert.equal(boxRelease({ BOX_SOURCES: root }, "0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out, bundle).status, 2, "never unsigned");
  assert.equal(pack("ten", "https://example.invalid/box-ten.tar.gz", out, bundle).status, 1, "a version, or none");
  assert.equal(pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out, path.join(scratch, "no-such.app")).status, 2, "each bundle named is there");
  assert.deepEqual(fs.readdirSync(out), [], "nothing left there by those");
  const packed = pack("0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", out, bundle);
  assert.equal(packed.status, 0, packed.stderr);
  // (And the installer beside them, for an app that carries the release: install.test.js.)
  assert.deepEqual(fs.readdirSync(out).sort(), ["box-0.10.0.tar.gz", "boxes.json", "kuvo-install"]);
  assert.deepEqual(fs.readFileSync(path.join(out, "kuvo-install")), fs.readFileSync(path.join(checkout, "box/src/install/kuvo-install")));
  const manifest = link.verifyBoxes(fs.readFileSync(path.join(out, "boxes.json"), "utf8"), [pinned]);
  assert.equal(manifest.version, "0.10.0");
  assert.equal(manifest.engine, "1.1.0");
  assert.equal(manifest.pack.url, "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz");
  // What's packed: the helper under `bundles`, so a box from before bundles, which refuses a file
  // in `files` outside its own rule, still takes the release (leaving the helper out).
  const unpacked = path.join(scratch, `unpacked-${crypto.randomUUID()}`);
  fs.mkdirSync(unpacked);
  assert.equal(spawnSync("tar", ["xzf", path.join(out, "box-0.10.0.tar.gz"), "-C", unpacked]).status, 0);
  const packedBox = read(path.join(unpacked, "box.json"));
  const before = /^(bin\/[A-Za-z0-9._-]+|bin\/runtime\/[A-Za-z0-9._-]+|config\/opencode\/plugins\/[A-Za-z0-9._-]+)$/;
  assert.deepEqual(Object.keys(packedBox.files).filter(file => !before.test(file)), []);
  assert.deepEqual(Object.keys(packedBox.bundles["bin/Kuvo Helper.app"]).sort(), process.platform === "darwin" ? ["Contents/Info.plist", "Contents/MacOS/Kuvo Helper", "Contents/_CodeSignature/CodeResources"] : ["Contents/Info.plist", "Contents/MacOS/Kuvo Helper"]);

  assert.equal(link.receiveRelease(out, [pinned]).status, 202);
  assert.equal(link.boxVersion(), "0.9.0");
  assert.equal(sha256(fs.readFileSync(path.join(home, "updates/release/box-0.10.0.tar.gz"))), manifest.pack.sha256);

  // A bundle a box couldn't take (a link in it) isn't packed.
  fs.symlinkSync("/etc/hosts", path.join(bundle, "Contents", "hosts"));
  const linked = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.equal(linked.status, 1);
  assert.match(linked.stderr, /can't take .*hosts/);
  fs.rmSync(path.join(bundle, "Contents", "hosts"));

  // One whose signature doesn't hold once packed (here, changed after it was signed) isn't packed.
  if (process.platform === "darwin") {
    fs.appendFileSync(path.join(bundle, "Contents", "Info.plist"), "\n");
    const broken = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
    assert.equal(broken.status, 1);
    assert.match(broken.stderr, /Kuvo Helper\.app isn't signed as it was/);
  }
  assert.deepEqual(fs.readdirSync(path.join(scratch, "never")), [], "nothing left there");

  // The same files pack to the same bytes, wherever your temporary folder is: one named with a
  // last slash (a Mac's own is) stays as private as it was, and nothing above the pack is touched.
  // (The helper as it was signed again, where the line added above changed it.)
  if (process.platform === "darwin") fs.writeFileSync(path.join(bundle, "Contents", "Info.plist"), fs.readFileSync(path.join(bundle, "Contents", "Info.plist"), "utf8").replace(/\n$/, ""));
  const again = path.join(scratch, `again-${crypto.randomUUID()}`), temporary = path.join(scratch, `tmp-${crypto.randomUUID()}`);
  fs.mkdirSync(temporary, { mode: 0o700 });
  const repacked = boxRelease({ KUVO_BOX_SIGNING_KEY: signingKey, KUVO_BOX_SPREADS: "1", BOX_SOURCES: root, TMPDIR: `${temporary}/` }, "0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", again, bundle);
  assert.equal(repacked.status, 0, repacked.stderr);
  assert.equal((fs.statSync(temporary).mode & 0o777).toString(8), "700", "your temporary folder as it was");
  assert.deepEqual(fs.readdirSync(temporary), [], "and nothing left in it");
  assert.equal(sha256(fs.readFileSync(path.join(again, "box-0.10.0.tar.gz"))), sha256(fs.readFileSync(path.join(out, "box-0.10.0.tar.gz"))), "the same pack, byte for byte");

  // A bundle with a file whose name is too long to pack isn't signed short of it.
  const long = path.join(bundle, "Contents", "Resources", `${"n".repeat(120)}.txt`);
  write(long, "x\n");
  const short = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.notEqual(short.status, 0, "a pack missing a file box.json names is never made");
  fs.rmSync(path.dirname(long), { recursive: true });

  // A test's sources never make a pack for a place something is published at.
  for (const url of ["https://github.com/example/example/releases/download/v0.10.0/box-0.10.0.tar.gz", "https://github.com/example/example.invalid/box-0.10.0.tar.gz"]) {
    const published = pack("0.10.0", url, path.join(scratch, "never"), bundle);
    assert.equal(published.status, 2, url);
    assert.match(published.stderr, /BOX_SOURCES is for tests/);
  }
  // A browser that isn't what its first line says (changed by hand, cut short) isn't packed.
  fs.appendFileSync(path.join(root, "box/dist/kuvo-browser.js"), "// changed\n");
  const edited = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.equal(edited.status, 1);
  assert.match(edited.stderr, /kuvo-browser\.js isn't what its first line says/);
  write(path.join(root, "box/dist/kuvo-browser.js"), builtBrowser);
  assert.deepEqual(fs.readdirSync(path.join(scratch, "never")), [], "nothing left there");

  // A signing that fails leaves the release that was there whole: never a new pack beside the
  // old boxes.json, nor one with none.
  const kept = Object.fromEntries(fs.readdirSync(out).map(name => [name, sha256(fs.readFileSync(path.join(out, name)))]));
  const unsigned = boxRelease({ KUVO_BOX_SIGNING_KEY: "not a key", BOX_SOURCES: root }, "0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out);
  assert.notEqual(unsigned.status, 0);
  assert.deepEqual(Object.fromEntries(fs.readdirSync(out).map(name => [name, sha256(fs.readFileSync(path.join(out, name)))])), kept);
});

test("the release built from this checkout's own sources is whole: every file a box runs, as box.json names it, for the pinned engine", () => {
  const folder = path.join(scratch, `files-${crypto.randomUUID()}`);
  const written = boxRelease({}, "--files", "1.2.3-beta.4", folder);
  assert.equal(written.status, 0, written.stderr);
  const box = read(path.join(folder, "box.json"));
  const pin = read(path.join(checkout, "box/src/shared/engine-pin.json"));
  assert.deepEqual([box.version, box.engine, box.linux, box.darwin], ["1.2.3-beta.4", pin.version, pin.linux, pin.darwin]);
  // Named: each file there but box.json itself, by what it holds, and nothing a box wouldn't take.
  const there = fs.readdirSync(folder, { recursive: true, encoding: "utf8" }).filter(file => fs.statSync(path.join(folder, file)).isFile() && file !== "box.json").sort();
  assert.deepEqual(Object.keys(box.files), there);
  for (const file of there) assert.equal(box.files[file], sha256(fs.readFileSync(path.join(folder, file))), file);
  assert.deepEqual(there, [
    "bin/.kuvo-boot", "bin/bot-words.json", "bin/box-version", "bin/integrations-mac.json", "bin/integrations.json", "bin/kuvo-agents.js", "bin/kuvo-browser.js",
    "bin/kuvo-launch", "bin/kuvo-link.js", "bin/kuvo-npx", "bin/kuvo-setsid", "bin/kuvo-start", "bin/kuvo-wake", "bin/mcp-catalog.json", "bin/runtime/node",
    "config/opencode/plugins/kuvo-code.js",
  ]);
  const text = file => fs.readFileSync(path.join(folder, file), "utf8");
  assert.equal(text("bin/box-version"), "1.2.3-beta.4");
  // The keeper's files read, and run this engine: its version where each names it, never the mark it's written over.
  for (const file of ["bin/kuvo-start", "bin/kuvo-launch", "bin/kuvo-setsid", "bin/kuvo-wake", "bin/kuvo-npx", "bin/runtime/node"]) {
    assert.equal(spawnSync("sh", ["-n", path.join(folder, file)]).status, 0, `${file} reads`);
    assert.doesNotMatch(text(file), /@ENGINE@/, file);
  }
  assert.ok(text("bin/kuvo-start").includes(`\nV=${pin.version}\n`));
  assert.ok(text("bin/kuvo-launch").includes(`V=\${KUVO_VERSION:-${pin.version}}`));
  for (const file of ["bin/kuvo-npx", "bin/runtime/node"]) assert.ok(text(file).includes(`/.kuvo-code/engine/${pin.version}/opencode"`), file);
  // What says which keeper files a box has: their names and bytes, in the order a Mac wrote them.
  assert.equal(text("bin/.kuvo-boot"), sha256(["kuvo-launch", "kuvo-start", "kuvo-setsid"].map(name => `${name}\n${text(`bin/${name}`)}`).join("\n")));
  // The bridge, as built, then the line naming it.
  const bridge = fs.readFileSync(path.join(checkout, "box/dist/kuvo-agents.js"));
  assert.equal(text("bin/kuvo-agents.js"), `${bridge}\n// kuvo-agents ${sha256(bridge).slice(0, 16)}\n`);
  assert.ok(fs.readFileSync(path.join(folder, "bin/kuvo-link.js")).equals(fs.readFileSync(script)));
  // The catalogs: a machine's without what uses a Mac's own apps, a Mac's box's with it, each key
  // named where a box keeps it.
  const machines = read(path.join(folder, "bin/integrations.json")).integrations, macs = read(path.join(folder, "bin/integrations-mac.json")).integrations;
  const catalog = read(path.join(folder, "bin/mcp-catalog.json"));
  assert.deepEqual(macs.filter(each => each.macOnly).map(each => each.id), ["apple-calendar"]);
  assert.deepEqual(machines, macs.filter(each => !each.macOnly));
  assert.deepEqual(catalog.macOnly, ["apple-calendar", "chrome-devtools"]);
  const github = machines.find(each => each.id === "github");
  assert.deepEqual([github.variables, github.settings.headers], [{ TOKEN: "KUVO_KEY_GITHUB_TOKEN" }, { Authorization: "Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_GITHUB_TOKEN}" }]);
  for (const each of macs) assert.equal(each.settings === undefined, each.way === "browser", `${each.id}: a server unless it's used on its site`);
  assert.deepEqual(catalog.servers.filter(each => each.integration).map(each => each.id), ["linear", "notion", "atlassian"]);
  assert.equal(read(path.join(folder, "bin/bot-words.json")).kuvo, 1);
});
