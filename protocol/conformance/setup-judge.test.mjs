// Exercise discovery through the real resident with recorded SSH probe lines.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, scratch, write } from "./adapter.mjs";
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
  fs.mkdirSync(app);
  const machine = path.join(env.HOME, ".kuvo-code");
  const found = {
    formatVersion: 1, setup: "set_" + Buffer.alloc(16, 31).toString("base64url"), role: "member",
    self: { key: Buffer.alloc(32, 12).toString("base64url"), name: "Machine", kind: "machine" },
    primary: { key: Buffer.alloc(32, 13).toString("base64url"), name: "Main" },
    computers: 2, items: 1, macs: [],
  };
  write(path.join(machine, "run", "setup-card.json"), JSON.stringify(found));
  write(path.join(machine, "bin", "kuvo-start"), "");
  const discover = async () => {
    const child = boxCommand(boxBinary(), app, env, { url: "ws://127.0.0.1:1" }, ["here", ".kuvo-code"], {}, "pipe");
    t.after(() => child.stop());
    let stdout = "";
    child.child.stdout.setEncoding("utf8").on("data", chunk => { stdout += chunk; });
    child.child.stdin.end();
    assert.equal((await child.exited).code, 0, child.log());
    assert.deepEqual(fs.readdirSync(app), []);
    return JSON.parse(stdout);
  };
  const attached = await discover();
  assert.deepEqual([attached.verdict, attached.use, attached.opens, attached.runs], ["elsewhere", "attach", "window", "stopped"]);
  assert.equal(attached.box.folder, ".kuvo-code");
  assert.equal(attached.box.primary.name, "Main");
  assert.equal(attached.own, null);
  write(path.join(machine, "run", "off"), "");
  assert.deepEqual(await discover(), { verdict: "free", use: "own", opens: "main", runs: "gone", box: null, own: null });
  write(path.join(machine, "updates", "rehome", "journal.json"), '{"v":1,"step":"staging"}');
  assert.deepEqual(await discover(), { verdict: "moving", use: "wait", opens: "main", runs: null, box: null, own: null });
});
