// Settings › Agents end to end (PROTOCOL.md, "A box's settings", `agents`): a machine's box (the real
// kuvo-link.js, each call a process of its own, `box route`, as a client's over SSH is) lists the
// agents, makes one from New Agent's form, turns it off and on, resets a built-in one and deletes
// one, all in the settings file its engine reads; and, with the real engine running on that file,
// the engine has each change: the agent made with its instructions and rules, left out while it's
// off, a built-in one back as it's built in. The engine part is skipped unless
// OPENCODE_TEST_BINARY names the engine.
//
// Run with `node --test box/bridge-tests/settings-agents-live.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const binary = process.env.OPENCODE_TEST_BINARY;
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-settings-agents-")));
removeAtEnd(root);
const script = fileURLToPath(LINK_BUNDLE);
const home = path.join(root, "home"), kuvo = path.join(home, ".kuvo-code");
const settingsFile = path.join(kuvo, "box", "opencode.json");
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value));
};
const settings = () => JSON.parse(fs.readFileSync(settingsFile, "utf8"));

const env = { ...process.env, KUVO_HOME: kuvo, HOME: home, TMPDIR: path.join(root, "tmp") };
for (const name of Object.keys(env)) if (name.startsWith("KUVO_") && name !== "KUVO_HOME") delete env[name];
fs.mkdirSync(env.TMPDIR, { recursive: true });

/** A route of the box's, asked as a client over SSH asks it. */
function route(method, p, body) {
  const ran = spawnSync(process.execPath, [script, "box", "route"], { env, input: JSON.stringify({ method, path: p, ...(body ? { body } : {}) }), encoding: "utf8" });
  const said = JSON.parse(ran.stdout.trim().split("\n").pop());
  return { status: said.status, json: JSON.parse(said.body) };
}
const page = () => {
  const { status, json } = route("GET", "kuvo/settings/agents");
  assert.equal(status, 200, JSON.stringify(json));
  return json;
};
const rowOf = (shown, id) => shown.groups.flatMap(group => group.rows.flatMap(row => [row, ...(row.items ?? [])])).find(row => row.id === id);
const formOf = (row, action) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find(each => each.id === action)?.form;
const valuesOf = (row, action) => Object.fromEntries(formOf(row, action).fields.map(field => [field.id, field.value]));
const act = (row, action, values) => route("POST", `kuvo/settings/agents/${encodeURIComponent(row.id)}/${action}`, { was: formOf(row, action)?.revision ?? row.revision, ...(values ? { values } : {}) });

const waitFor = async (wanted, what) => {
  const end = Date.now() + 60_000;
  for (;;) {
    const found = await wanted().catch(() => null);
    if (found) return found;
    assert.ok(Date.now() < end, `Waited too long for ${what}`);
    await new Promise(resolve => setTimeout(resolve, 200));
  }
};

/** The real engine, reading the box's settings file as a machine's does (`OPENCODE_CONFIG`). */
async function startEngine() {
  for (const folder of ["work", "xdg/config", "xdg/data", "xdg/state", "xdg/cache"]) fs.mkdirSync(path.join(root, folder), { recursive: true });
  const work = path.join(root, "work");
  const child = spawn(binary, ["serve", "--stdio", "--port", "0"], {
    env: { PATH: process.env.PATH, HOME: home, TMPDIR: env.TMPDIR, OPENCODE_CONFIG: settingsFile, XDG_CONFIG_HOME: path.join(root, "xdg/config"), XDG_DATA_HOME: path.join(root, "xdg/data"), XDG_STATE_HOME: path.join(root, "xdg/state"), XDG_CACHE_HOME: path.join(root, "xdg/cache"), OPENCODE_PASSWORD: "pw" },
    cwd: work, stdio: ["pipe", "pipe", "pipe"],
  });
  atEnd(() => new Promise(resolve => { if (child.exitCode !== null) return resolve(); child.on("exit", resolve); child.stdin.end(); child.kill(); }));
  const port = await new Promise((resolve, reject) => {
    let said = "";
    const look = chunk => { said += chunk; const found = said.match(/127\.0\.0\.1:(\d+)/); if (found) resolve(Number(found[1])); };
    child.stdout.on("data", look);
    child.stderr.on("data", look);
    child.on("exit", code => reject(new Error(`The engine ended (${code}): ${said}`)));
  });
  const headers = { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "x-opencode-directory": encodeURIComponent(work) };
  const agents = async () => {
    const response = await fetch(`http://127.0.0.1:${port}/api/agent`, { headers });
    return response.ok ? (await response.json()).data : null;
  };
  await waitFor(agents, "the engine to start");
  // (The box reads its engine from its record, as a machine's keeper writes it.)
  write(path.join(kuvo, "run", "engine.json"), { port, password: "pw", version: "0.0.0" });
  return { agents, agent: async id => (await agents())?.find(each => each.id === id) };
}

test("a machine's Agents settings are listed, added, turned off and on, reset and deleted in the file its engine reads, and the real engine has each change", { timeout: 300_000 }, async () => {
  write(settingsFile, { agents: { plan: { system: "Plan in two steps." } } });
  const engine = binary ? await startEngine() : null;
  if (engine) assert.equal((await engine.agent("plan"))?.system, "Plan in two steps.");

  let shown = page();
  assert.deepEqual(rowOf(shown, "agents").items.map(each => each.title), ["Build", "Plan", "General", "Explore"]);
  if (!engine) assert.match(shown.notes[0].words, /isn’t answering/);

  // New Agent…, checked as it's typed, then made.
  const list = rowOf(shown, "agents");
  const opened = valuesOf(list, "add");
  const checked = route("POST", "kuvo/settings/agents/agents/add/check", { was: formOf(list, "add").revision, values: { ...opened, name: "Bug Hunter!" } });
  assert.equal(checked.status, 200);
  assert.deepEqual(checked.json.fields.find(each => each.id === "name"), { id: "name", words: "It'll be called Bug Hunter.", tone: "quiet" });
  const made = act(list, "add", { ...opened, name: "Bug Hunter", description: "Finds bugs.", system: "Look for bugs; change nothing.", "may-edit": "deny", allowed: "Run commands: npm test *" });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  assert.deepEqual(settings().agents["Bug Hunter"], {
    mode: "subagent", description: "Finds bugs.", system: "Look for bugs; change nothing.",
    permissions: [{ action: "edit", resource: "*", effect: "deny" }, { action: "shell", resource: "npm test *", effect: "allow" }],
  });
  if (engine) {
    const hunter = await waitFor(() => engine.agent("Bug Hunter"), "the engine to have the new agent");
    assert.equal(hunter.mode, "subagent");
    assert.equal(hunter.system, "Look for bugs; change nothing.");
    assert.ok(hunter.permissions.some(rule => rule.action === "shell" && rule.resource === "npm test *" && rule.effect === "allow"));
    assert.equal(hunter.permissions.filter(rule => rule.action === "edit" && rule.resource === "*").at(-1).effect, "deny");
  }

  // Turned off, it's left out; on again, it's back.
  shown = made.json;
  let changed = route("PATCH", "kuvo/settings/agents", { row: "agent-Bug Hunter", value: false, was: rowOf(shown, "agent-Bug Hunter").revision });
  assert.equal(changed.status, 200, JSON.stringify(changed.json));
  assert.equal(settings().agents["Bug Hunter"].disabled, true);
  if (engine) await waitFor(async () => (await engine.agents())?.every(each => each.id !== "Bug Hunter"), "the engine to leave it out");
  changed = route("PATCH", "kuvo/settings/agents", { row: "agent-Bug Hunter", value: true, was: rowOf(changed.json, "agent-Bug Hunter").revision });
  assert.equal(changed.status, 200);
  if (engine) await waitFor(() => engine.agent("Bug Hunter"), "the engine to have it again");

  // Plan reset to how it's built in: its own instructions are back.
  const reset = act(rowOf(changed.json, "agent-plan"), "reset");
  assert.equal(reset.status, 200, JSON.stringify(reset.json));
  assert.equal(settings().agents.plan, undefined);
  if (engine) await waitFor(async () => (await engine.agent("plan"))?.system !== "Plan in two steps.", "the engine to have Plan as it's built in");

  // A form read before the reset is stale, and nothing's written.
  const stale = act(rowOf(shown, "agent-plan"), "edit", valuesOf(rowOf(shown, "agent-plan"), "edit"));
  assert.equal(stale.status, 409);
  assert.equal(settings().agents.plan, undefined);

  // Deleted: out of the file and the engine.
  const deleted = act(rowOf(reset.json, "agent-Bug Hunter"), "delete");
  assert.equal(deleted.status, 200);
  assert.equal(settings().agents, undefined);
  if (engine) await waitFor(async () => (await engine.agents())?.every(each => each.id !== "Bug Hunter"), "the engine to drop it");
});
