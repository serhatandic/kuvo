// Exercise discovery through the real resident with recorded SSH probe lines.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, launch, scratch, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("the probe distinguishes an older installation from another setup on the same computer", async t => {
  const { computer } = await bench(t);
  const local = await computer("Studio");
  const ask = local.connect(await local.start().ready());
  const key = Buffer.alloc(32, 21).toString("base64url");
  const setup = "set_" + Buffer.alloc(16, 22).toString("base64url");
  const card = { formatVersion: 1, setup, role: "primary", self: { key, name: "Lab", kind: "machine" }, computers: 1, items: 0, macs: [] };
  const probe = [
    "box machine .kuvo-code", `link mac ${key}`, `link name ${key} "Old Mac"`,
    `link seen ${key} 1`, "box app Library/Application Support/run.kuvo.code/box",
    `card ${JSON.stringify(card)}`,
  ].join("\n");
  const judge = probe => ask("POST", "kuvo/setup/judge", { probe });
  const reply = await judge(probe);
  assert.equal(reply.status, 200, reply.text);
  assert.deepEqual(reply.json, {
    verdict: "two", joins: false, boxes: [
      { folder: ".kuvo-code", app: false, verdict: "older", key: null, name: "", kind: null, setup: null, role: null, primary: { key, name: "Old Mac", heard: 1 }, macs: [{ key, name: "Old Mac" }], computers: null, items: null },
      { folder: "Library/Application Support/run.kuvo.code/box", app: true, verdict: "main", key, name: "Lab", kind: "machine", setup, role: "primary", primary: null, macs: [], computers: 1, items: 0 },
    ],
  });

  const lone = `box machine .kuvo-code\ncard ${JSON.stringify(card)}`;
  assert.deepEqual((await judge(lone)).json, { verdict: "main", joins: true, boxes: [
    { ...reply.json.boxes[1], folder: ".kuvo-code", app: false },
  ] });
  const withApp = await judge(lone + "\napp Library/Application Support/run.kuvo.code");
  assert.equal(withApp.json.joins, false);
  const member = await judge(lone.replace('"primary"', '"member"'));
  assert.equal(member.json.verdict, "elsewhere");
  const current = (await ask("GET", "kuvo/setup")).json;
  const found = fields => `box machine .kuvo-code\ncard ${JSON.stringify({ ...card, ...fields })}`;
  const theirs = { key: Buffer.alloc(32, 23).toString("base64url"), name: "Other Mac", heard: 1 };
  const verdicts = [
    ["", "free", false],
    [found({ setup: current.setup, role: "member" }), "ours", false],
    [found({ self: { key: current.you.key, name: "Studio", kind: "mac" } }), "self", false],
    [found({ role: "member", primary: theirs, computers: 3, items: 12 }), "elsewhere", false],
    [found({ role: "primary", primary: theirs, computers: 3, items: 0 }), "main", false],
    [found({ role: "primary", primary: theirs, computers: undefined, items: "unknown" }), "main", false],
    [found({ app: true }), "main", false],
    [found({ setup: undefined, role: undefined, macs: [theirs] }), "elsewhere", false],
    [found({ setup: undefined, role: undefined, macs: [{ key: current.you.key, name: "Studio" }] }), "ours", false],
    [found({ setup: undefined, role: undefined }), "free", false],
    [found({ role: "member", primary: { key: current.you.key, name: "Studio", heard: 1 } }), "ours", false],
    [`box machine .kuvo-code\nlink mac ${current.you.key}`, "ours", false],
    ["box machine .kuvo-code\nlink phones some", "older", false],
    ["box machine .kuvo-code\nlink phones none", "free", false],
    ["box app Library/Application Support/run.kuvo.code/box\nlink phones none", "older", false],
  ];
  for (const [probe, verdict, joins] of verdicts) {
    const answer = await judge(probe);
    assert.equal(answer.status, 200, answer.text);
    assert.deepEqual([answer.json.verdict, answer.json.joins], [verdict, joins]);
  }
  assert.equal((await judge(undefined)).status, 400);

  const future = await judge(lone.replace('"formatVersion":1', '"formatVersion":2'));
  assert.equal(future.status, 409, future.text);
  assert.equal(future.json.code, "unsupportedFormatVersion");
  assert.match(future.json.message, /Setup discovery card format version 2/);
});

test("local discovery chooses the existing member and leaves the app home untouched", async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const app = path.join(root, "app");
  const machine = path.join(env.HOME, ".kuvo-code");
  const found = {
    formatVersion: 1, setup: "set_" + Buffer.alloc(16, 31).toString("base64url"), role: "member",
    self: { key: Buffer.alloc(32, 12).toString("base64url"), name: "Machine", kind: "machine" },
    primary: { key: Buffer.alloc(32, 13).toString("base64url"), name: "Main", heard: 1 },
    computers: 2, items: 1, macs: [],
  };
  write(path.join(machine, "run", "setup-card.json"), JSON.stringify(found));
  write(path.join(machine, "bin", "kuvo-start"), "");
  const discover = async () => {
    const child = boxCommand(boxBinary(), root, env, { url: "ws://127.0.0.1:1" },
      ["here", ".kuvo-code"], { KUVO_HOME: app }, "pipe");
    t.after(() => child.stop());
    let stdout = "";
    child.child.stdout.setEncoding("utf8").on("data", chunk => { stdout += chunk; });
    child.child.stdin.end();
    assert.equal((await child.exited).code, 0, child.log());
    return JSON.parse(stdout);
  };
  const attached = await discover();
  assert.deepEqual([attached.verdict, attached.use, attached.opens, attached.runs], ["elsewhere", "attach", "window", "stopped"]);
  assert.equal(attached.box.folder, ".kuvo-code");
  assert.equal(attached.box.primary.name, "Main");
  assert.equal(attached.box.primary.heard, 1);
  assert.equal(attached.own, null);
  assert.equal(fs.existsSync(app), false);

  write(path.join(app, "machines.json"), "[]");
  write(path.join(app, "bin", "integrations.json"), "{}");
  assert.deepEqual(await discover(), attached);
  const link = path.join(machine, "run", "link.json");
  const card = path.join(machine, "run", "setup-card.json");
  fs.unlinkSync(card);
  write(link, JSON.stringify({ mac: found.primary.key, macNames: { [found.primary.key]: "Main" } }));
  const legacy = await discover();
  assert.deepEqual([legacy.verdict, legacy.use, legacy.opens, legacy.runs, legacy.box.primary.name],
    ["older", "attach", "window", "stopped", "Main"]);
  fs.unlinkSync(link);
  write(card, JSON.stringify({ formatVersion: 1, self: found.self, macs: [] }));
  const lone = await discover();
  assert.deepEqual([lone.verdict, lone.use, lone.opens], ["free", "attach", "main"]);
  write(card, JSON.stringify(found));
  write(path.join(machine, "run", "off"), "");
  assert.deepEqual(await discover(), { verdict: "free", use: "own", opens: "main", runs: "gone", box: null, own: null });
  write(path.join(machine, "updates", "rehome", "journal.json"), '{"v":1,"step":"staging"}');
  assert.deepEqual(await discover(), { verdict: "moving", use: "wait", opens: "main", runs: null, box: null, own: null });
  assert.deepEqual(fs.readdirSync(app).sort(), ["bin", "machines.json"]);
});

test("local discovery keeps the app's own installation across older and current layouts", async t => {
  const { account, app, machine, read, command } = discovery(t);
  const markers = JSON.parse(fs.readFileSync(
    new URL("../../box/test/fixtures/routes/setup/own-box-ran.json", import.meta.url), "utf8")).files;
  for (const marker of markers) {
    const file = path.join(app, "run", marker);
    write(file, "{}");
    const answer = await read();
    assert.deepEqual([answer.verdict, answer.use, answer.own.folder], ["free", "own", "app"]);
    fs.unlinkSync(file);
  }
  const main = { key: Buffer.alloc(32, 30).toString("base64url"), name: "Studio" };
  const member = { key: Buffer.alloc(32, 31).toString("base64url"), name: "Lab" };
  const setup = "set_" + Buffer.alloc(16, 32).toString("base64url");
  const otherSetup = "set_" + Buffer.alloc(16, 33).toString("base64url");
  const own = path.join(app, "run", "setup-card.json");
  const card = path.join(machine, "run", "setup-card.json");
  const ownCard = { formatVersion: 1, self: { ...main, kind: "mac" }, setup, role: "primary",
    primary: main, computers: 2, items: 0, app: true };
  write(own, JSON.stringify(ownCard));
  write(path.join(machine, "bin", "kuvo-start"), "");
  const machineCard = { formatVersion: 1, self: { ...member, kind: "machine" }, setup,
    role: "member", primary: { ...main, heard: 1 }, computers: 2, items: 0 };
  write(card, JSON.stringify(machineCard));
  const shares = await read();
  assert.deepEqual([shares.verdict, shares.use, shares.opens, shares.box.role], ["ours", "own", "main", "member"]);
  write(card, JSON.stringify({ ...machineCard, setup: otherSetup, primary: member }));
  const separate = await read();
  assert.deepEqual([separate.verdict, separate.use, separate.box.primary.name], ["two", "own", "Lab"]);

  fs.unlinkSync(own);
  const engineData = path.join(account, "old-engine");
  write(path.join(engineData, "opencode.db"), "");
  fs.unlinkSync(card);
  write(path.join(machine, "run", "link.json"), JSON.stringify({ mac: member.key, macNames: { [member.key]: member.name } }));
  const older = await read({ KUVO_ENGINE_DATA: engineData });
  assert.deepEqual([older.verdict, older.use, older.opens, older.own.folder], ["two", "own", "main", "app"]);

  write(own, JSON.stringify({ ...ownCard, role: "member", primary: member }));
  fs.unlinkSync(path.join(machine, "bin", "kuvo-start"));
  const handedOver = await read();
  assert.deepEqual([handedOver.verdict, handedOver.use, handedOver.opens, handedOver.own.role, handedOver.box],
    ["free", "own", "window", "member", null]);
  for (const [root, extra] of [[[], {}], [[".kuvo-code"], { KUVO_HOME: "" }]]) {
    const refused = await command(root, extra);
    assert.equal(refused.code, 2, refused.log);
    assert.match(refused.log, /here needs KUVO_HOME/);
  }
  const future = JSON.stringify({ ...ownCard, formatVersion: 2 });
  write(own, future);
  const refused = await command([".kuvo-code"]);
  assert.equal(refused.code, 2, refused.log);
  assert.match(refused.log, /Setup discovery card format version 2/);
  assert.equal(fs.readFileSync(own, "utf8"), future);
});

test("local discovery recognizes a live keeper and ignores it when startup is removed", {
  skip: process.platform === "win32",
}, async t => {
  const { machine, env, read } = discovery(t);
  const card = { formatVersion: 1, setup: "set_" + Buffer.alloc(16, 23).toString("base64url"),
    role: "member", self: { key: Buffer.alloc(32, 24).toString("base64url"), name: "Lab", kind: "machine" },
    primary: { key: Buffer.alloc(32, 25).toString("base64url"), name: "Studio", heard: 1 } };
  write(path.join(machine, "run", "setup-card.json"), JSON.stringify(card));
  const start = path.join(machine, "bin", "kuvo-start");
  const ready = path.join(machine, "run", "fixture-keeper-ready");
  write(start, '#!/bin/sh\n: > "$1"\nread hold\n');
  const keeper = launch("/bin/sh", [start, ready], env, machine);
  t.after(() => keeper.stop());
  await until(() => fs.existsSync(ready), "The keeper fixture did not start.");
  write(path.join(machine, "run", "box.pid"), String(keeper.child.pid));
  const running = await read();
  assert.deepEqual([running.use, running.runs, running.box.primary.name], ["attach", "running", "Studio"]);
  fs.unlinkSync(start);
  assert.deepEqual(await read(), { verdict: "free", use: "own", opens: "main", runs: "gone", box: null, own: null });
  assert.equal(keeper.child.exitCode, null);
});

test("installer status distinguishes a fresh account, a machine, and an interrupted installation", async t => {
  const binary = boxBinary();
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const args = ["install", "--status", "--root", ".kuvo-code"];
  const mac = process.platform === "darwin";
  const legacy = path.join(env.HOME, "Library", "Application Support", "run.kuvo.code.trial713");
  if (mac) {
    const app = path.join(root, "Fixture.app");
    write(path.join(app, "Contents", "Info.plist"),
      '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.trial713</string></dict></plist>');
    args.push("--app", app);
  }
  const check = async (verdict, verb) => {
    const reply = boxCommand(binary, root, env, { url: "ws://127.0.0.1:1" }, args, { KUVO_HOME: home }, "pipe");
    let stdout = "";
    reply.child.stdout.setEncoding("utf8").on("data", part => { stdout += part; });
    reply.child.stdin.end();
    assert.equal((await reply.exited).code, 0, stdout + reply.log());
    const seen = JSON.parse(stdout);
    assert.deepEqual([seen.verdict, seen.verb], [verdict, verb]);
    return seen;
  };
  await check("fresh", "new");
  assert.equal(fs.existsSync(home), false);
  if (mac) {
    for (const file of ["engine/2.2.6/opencode", "desktop/window.json", "opencode/cache/models.json", ".DS_Store"]) {
      write(path.join(legacy, file), "");
    }
  }
  write(path.join(home, "updates", "install", "release", "box.json"), "{}");
  await check("fresh", "new");
  const launcher = process.platform === "win32" && !binary.typescript ? "kuvo-box.exe" : "kuvo-start";
  write(path.join(home, "bin", launcher), "");
  await check(mac ? "fresh" : "machine", mac ? "new" : null);
  const making = path.join(home, "updates", "install", "making");
  write(making, "1\n");
  await check("fresh", "new");
  const link = path.join(home, "run", "link.json");
  const saved = JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] });
  write(link, saved);
  write(path.join(home, "box", "keys", "fixture"), "test key");
  await check(mac ? "machine" : "fresh", mac ? null : "new");
  fs.unlinkSync(making);
  await check("machine", null);
  if (mac) {
    const notes = path.join(legacy, "Chats", "c1", "notes.md");
    write(notes, "keep the old work");
    const separate = await check("two", null);
    assert.equal(separate.message, "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.");
    assert.equal(fs.readFileSync(notes, "utf8"), "keep the old work");
    assert.equal(fs.readFileSync(link, "utf8"), saved);
    assert.equal(fs.existsSync(path.join(home, "box", "kind")), false);
    fs.rmSync(path.join(legacy, "Chats"), { recursive: true });
  }
  write(path.join(home, "run", "off"), "");
  await check("fresh", "new");
  assert.equal(fs.readFileSync(link, "utf8"), saved);
});

function discovery(t) {
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const account = env.HOME;
  const app = path.join(account, "app");
  const machine = path.join(account, ".kuvo-code");
  fs.mkdirSync(app);
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const command = async (args, extra = {}) => {
    const child = boxCommand(boxBinary(), app, env, { url: "ws://127.0.0.1:1" }, ["here", ...args], extra, "pipe");
    let stdout = "";
    child.child.stdout.setEncoding("utf8").on("data", chunk => { stdout += chunk; });
    child.child.stdin.end();
    return { ...await child.exited, stdout, log: child.log() };
  };
  const read = async extra => {
    const answer = await command([".kuvo-code"], extra);
    assert.equal(answer.code, 0, answer.log);
    return JSON.parse(answer.stdout);
  };
  return { account, app, machine, env, read, command };
}
