// The actual released beta.1 MachineBox SSH command, run against a Rust home: its tagged shell
// lines find the installed pinned engine runtime and run the delegate the Rust pack builder emits.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { relayLeaseStandIn } from "../../box/bridge-tests/relay-lease-stand-in.js";
import { boxBinary, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until } from "./adapter.mjs";
import { generateRelease } from "./compat/generate.mjs";
import { runSync, tags, withTag } from "./compat/tagged.mjs";

const runtime = process.env.KUVO_CONFORMANCE_ENGINE;
const repo = fileURLToPath(new URL("../../", import.meta.url));
const pin = readJSON(path.join(repo, "box/src/shared/engine-pin.json")).version;

// beta.1's `RemoteEngineInstaller.script` runtime choice and `MachineBox.lastLine` command, from
// its tagged source. (Its folders and `cd "$K/workspace"` are kept; its command environment
// script only restores the account's own variables for agents' commands.)
async function releasedCommand() {
  let lines;
  await withTag(tags[1], ({ tree }) => {
    const swift = file => fs.readFileSync(path.join(tree, file), "utf8");
    const engine = swift("Packages/OpenCodeKit/Sources/OpenCodeKit/Engine/RemoteEngine.swift");
    const version = swift("Packages/OpenCodeKit/Sources/OpenCodeKit/OpenCodeKit.swift").match(/static let version = "([^"]+)"/)[1];
    const newest = engine.match(/static let newestEngine = #"(.+)"#/)[1];
    const box = swift("KuvoCode/Stores/MachineBox.swift");
    const command = [...box.matchAll(/RemoteEngineInstaller\.script\("""\n([\s\S]+?)\n\s*"""\), input: file\)/g)]
      .map(match => match[1]).find(text => text.includes("box \\(arguments)"))
      .split("\n").map(line => line.trim()).join("\n").replaceAll("\\\\n", "\\n").replaceAll("\\(Bridge.fileName)", "kuvo-link.js");
    lines = [`K="$HOME/.kuvo-code"`, `E="$K/engine/${version}/opencode"`, `[ -x "$E" ] || E="${newest}"`,
      `mkdir -p "$K/bin" "$K/run" "$K/workspace" && chmod 700 "$K/run"`, `cd "$K/workspace" || exit 1`, command];
    assert.notEqual(version, pin, "the runtime is found as the newest installed engine, not beta.1's own pin");
  });
  return lines.join("\n");
}

test("the released beta.1 MachineBox command runs through the installed engine and Rust delegate", {
  timeout: 120000,
  skip: !runtime ? "Set KUVO_CONFORMANCE_ENGINE to the pinned engine executable." : boxBinary().typescript && "Only Rust packs include the delegate.",
}, async () => {
  const root = scratch(), leases = relayLeaseStandIn(), relay = await localRelay({ request: leases.request });
  let proc, server;
  try {
    const env = isolatedEnv(root), home = path.join(env.HOME, ".kuvo-code"), files = path.join(root, "files");
    const binary = boxBinary(), windows = process.platform === "win32", extension = windows ? ".exe" : "";
    privateHome(home);
    // Where the Rust box installs its pinned engine.
    const installedEngine = path.join(home, "engine", pin, "opencode" + extension);
    fs.mkdirSync(path.dirname(installedEngine), { recursive: true });
    fs.copyFileSync(runtime, installedEngine);
    if (!windows) fs.chmodSync(installedEngine, 0o700);
    assert.equal(runSync(installedEngine, ["--version"], env, home).trim().replace(/^opencode v/, ""), pin);
    // Exactly the pack builder used by scripts/box-release.sh --rust-pack.
    const bunEnv = { ...env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" };
    runSync(installedEngine, [path.join(repo, "box/src/release/files.ts"), repo, "0.8.0", files,
      windows ? "windows" : "unix", binary.file], bunEnv, home);
    const shim = path.join(files, "bin/kuvo-link.js");
    const manifest = readJSON(path.join(files, "box.json"));
    assert.equal(manifest.launch, "rust");
    assert.equal(manifest.engine, pin);
    assert.equal(manifest.files["bin/kuvo-link.js"], createHash("sha256").update(fs.readFileSync(shim)).digest("hex"));
    fs.cpSync(path.join(files, "bin"), path.join(home, "bin"), { recursive: true });
    const program = path.join(home, "bin", "kuvo-box" + extension);
    const tagged = path.join(root, "release");
    await generateRelease(tags[1], tagged, { app: false });
    const { ssh } = readJSON(path.join(tagged, "vectors.json"));
    assert.deepEqual([...new Set(ssh.map(item => item.args[1]))].sort(), ["adopt", "change", "key", "read", "route"]);
    // beta.1 reaches Linux machines only; elsewhere the same runtime runs the delegate directly.
    const script = windows ? null : await releasedCommand();
    const run = (args, input) => script
      ? runSync("/bin/sh", ["-c", script.replace("box \\(arguments)", "box " + args.slice(1).map(word => `'${word}'`).join(" ")), "sh"], env, home, input)
      : runSync(installedEngine, [path.join(home, "bin/kuvo-link.js"), ...args], bunEnv, home, input);
    for (const item of ssh) {
      const verb = item.args[1];
      if (verb === "route") {
        proc = startBox({ binary: boxBinary(program), home, env, relay });
        server = await proc.ready();
      }
      const output = run(item.args, item.input);
      const expected = JSON.parse(item.output), actual = JSON.parse(output.trim().split("\n").at(-1));
      assert.equal(actual.status, expected.status, verb);
      if (expected.settings) for (const key of Object.keys(expected.settings)) assert.deepEqual(actual.settings[key], expected.settings[key], verb);
      if ("own" in expected) assert.equal(actual.own, expected.own);
      if (expected.body) assert.deepEqual(JSON.parse(actual.body).bots, JSON.parse(expected.body).bots);
    }
    assert.equal(fs.readFileSync(path.join(home, "box/keys/KUVO_KEY_NOTION"), "utf8"), "fixture-value");
    // beta.1's Personalities.Library, met with the machine's own. The moved machine leads its
    // own setup, so the library is saved there once it holds the relay's lease.
    const { setup } = (await request(server, "GET", "kuvo/setup")).json ?? {};
    if (setup) await until(() => leases.leaseFor(setup), "The machine did not claim a relay lease.");
    const personality = { id: "custom.from-mac", name: "From the Mac", description: "A released Mac's.", updated: 5 };
    const met = JSON.parse(run(["box", "personalities"], JSON.stringify({ personalities: [personality], removed: [] })).trim().split("\n").at(-1));
    assert.equal(met.status, 200, JSON.stringify(met));
    assert.ok(met.personalities.some(each => each.id === personality.id), JSON.stringify(met));
    assert.ok(Array.isArray(met.removed));
    // Not served by design: beta.1's phone link setup, its machine removal's leave, and its bot
    // keys refresh (which the TypeScript box refused too) say to update the Mac.
    for (const args of [["setup"], ["leave", "a".repeat(43)], ["box", "bot-keys-refresh"], ["box", "key"]]) {
      const result = spawnSync(installedEngine, [path.join(home, "bin/kuvo-link.js"), ...args], { env: bunEnv, cwd: home, input: "", encoding: "utf8", timeout: 30000 });
      assert.equal(result.status, 2, args.join(" "));
      assert.equal(result.stderr.trim(), "Update Kuvo Code on this Mac to use this computer.");
    }
  } finally {
    await proc?.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true });
  }
});


// setup-handover-shared.test.js's SSH personalities scenario, through either box's command.
test("beta.1's SSH personalities command saves at the main and a member refuses it", { timeout: 90000 }, async t => {
  const { bench } = await import("./bench.mjs");
  const { computer, leases } = await bench(t);
  const main = await computer("Studio"), other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json;
  await until(() => leases.leaseFor(setup.setup), "The main computer did not claim a relay lease.");
  const personality = { id: "custom.personal-1", name: "Personality 1", description: "x".repeat(40), updated: 2 };
  const saved = await main.command(["box", "personalities"], JSON.stringify({ personalities: [personality], removed: [] }));
  assert.equal(saved.code, 0, saved.log);
  const answer = JSON.parse(saved.text.trim().split("\n").at(-1));
  assert.equal(answer.status, 200, saved.text);
  assert.deepEqual(answer.personalities.map(each => each.id), [personality.id]);
  assert.deepEqual(answer.removed, []);
  assert.equal(answer.save.status, "committed");
  assert.equal(answer.save.setup, setup.setup);
  assert.equal(answer.operation, answer.save.operation);
  assert.deepEqual((await primary("GET", "kuvo/personalities")).json.personalities.map(each => each.id), [personality.id]);
  const invite = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  assert.equal((await other.join(invite.json.code)).code, 0);
  const member = other.connect(await other.start().ready());
  await until(async () => (await member("GET", "kuvo/personalities")).json?.personalities?.length === 1, "The member did not take the main's library.");
  const refused = await other.command(["box", "personalities"], JSON.stringify({ personalities: [{ ...personality, updated: 3, name: "Other" }], removed: [] }));
  assert.equal(refused.code, 0, refused.log);
  const no = JSON.parse(refused.text.trim().split("\n").at(-1));
  assert.equal(no.status, 409);
  assert.equal(no.message, "Change your setup's personalities on its main computer.");
  assert.equal((await member("GET", "kuvo/personalities")).json.personalities[0].name, "Personality 1");
});
