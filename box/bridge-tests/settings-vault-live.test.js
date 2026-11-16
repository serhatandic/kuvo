// Settings › Vault end to end (PROTOCOL.md, "A box's settings", Vault): a machine's box (the real
// kuvo-link.js `serve`, which holds its vault, in a home of the test's own) describes its vault as a
// settings page to the app beside it (`x-kuvo-from: app`), adds an item from Add Item's form, shows a
// value only through the vault's own values route, saves an edit, lets a bot use it, answers what an
// agent waits for, pauses, deletes, and follows the page as it changes; after a restart the vault is
// locked and the page says so, until a client that kept its key unlocks it. A request from anything
// else on the machine (`box route` without a client's proof) reads nothing of it. No value is ever
// in a page, a page followed, a check's answer or the vault's activity.
//
// Run with `node --test box/bridge-tests/settings-vault-live.test.js`.
import { testsFolder } from "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { script } from "./box-stand-ins.js";
import { atEnd, removeAtEnd } from "./at-end.js";

// (A short folder: the vault's socket lives in it.)
const root = fs.realpathSync(testsFolder("ksv"));
removeAtEnd(root);
const home = path.join(root, ".kuvo-code");
const env = { ...process.env, HOME: root, KUVO_HOME: home };
for (const name of Object.keys(env)) if (name.startsWith("KUVO_") && name !== "KUVO_HOME") delete env[name];

const SECRET = "hunter2-correct-horse";
const NEW_SECRET = "battery-staple-9";

fs.mkdirSync(path.join(home, "run"), { recursive: true, mode: 0o700 });
fs.mkdirSync(path.join(home, "bots", "bot_scout", "workspace"), { recursive: true });
fs.writeFileSync(path.join(home, "bots", "bot_scout", "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_scout", name: "Scout" }));
fs.writeFileSync(path.join(home, "bots", "bot_scout", "session"), "ses_scout");

/** The box's long-running process, which holds the vault, and the app beside it asking it. */
async function start() {
  const run = path.join(home, "run");
  fs.rmSync(path.join(run, "vault.sock"), { force: true });
  const child = spawn(process.execPath, [script, "serve"], { env: { ...env, OPENCODE_PASSWORD: "box-pw" }, stdio: ["pipe", "pipe", "pipe"] });
  let out = "", err = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", chunk => { err += chunk; });
  const exited = new Promise(resolve => child.once("exit", resolve));
  const url = await new Promise((resolve, reject) => {
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve(JSON.parse(out.split("\n")[0]).url);
    });
    exited.then(code => reject(new Error(`serve exited ${code}: ${err}`)));
  });
  const headers = { authorization: `Basic ${Buffer.from("opencode:box-pw").toString("base64")}`, "content-type": "application/json", "x-kuvo-from": "app" };
  const app = async (method, route, body) => {
    const response = await fetch(`${url}/${route}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    const text = await response.text();
    return { status: response.status, json: text ? JSON.parse(text) : {}, text };
  };
  /** The page followed: its lines as they come. */
  const follow = () => {
    const stop = new AbortController();
    const lines = (async function* () {
      const response = await fetch(`${url}/kuvo/settings/vault/events`, { headers, signal: stop.signal });
      let buffer = "";
      for await (const chunk of response.body) {
        buffer += Buffer.from(chunk).toString();
        for (let end = buffer.indexOf("\n"); end >= 0; end = buffer.indexOf("\n")) {
          yield buffer.slice(0, end);
          buffer = buffer.slice(end + 1);
        }
      }
    })();
    return { next: async () => (await lines.next()).value, stop: () => stop.abort() };
  };
  let stopping;
  const stop = () => stopping ??= (async () => { child.stdin.end(); child.kill("SIGTERM"); await exited; })();
  atEnd(stop);
  return { app, follow, stop };
}

/** `box route`: anything on the machine, without a client's proof. */
function routed(method, route, body) {
  const ran = spawnSync(process.execPath, [script, "box", "route"], { env, input: JSON.stringify({ method, path: route, ...(body ? { body } : {}) }), encoding: "utf8" });
  const said = JSON.parse(ran.stdout.trim().split("\n").pop());
  return { status: said.status, json: JSON.parse(said.body || "{}") };
}

const rowOf = (shown, id) => shown.groups.flatMap(group => group.rows.flatMap(row => [row, ...(row.items ?? [])])).find(row => row.id === id);
const actionOf = (row, id) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find(each => each.id === id);
const opening = (row, id) => Object.fromEntries(actionOf(row, id).form.fields.filter(field => "value" in field).map(field => [field.id, field.value]));
const onDisk = () => fs.readFileSync(path.join(home, "box", "vault", "activity.jsonl"), "utf8");

test("a machine's vault is described, changed and followed as a settings page, and no value is ever on it", async () => {
  let box = await start();
  const page = async () => {
    const { status, json, text } = await box.app("GET", "kuvo/settings/vault");
    assert.equal(status, 200, text);
    for (const secret of [SECRET, NEW_SECRET]) assert.ok(!text.includes(secret), "a value is on the page");
    return json;
  };
  const act = (row, action, values) => box.app("POST", `kuvo/settings/vault/${encodeURIComponent(row.id)}/${action}`, { was: actionOf(row, action)?.form?.revision ?? row.revision, ...(values ? { values } : {}) });

  // Its features say it describes its vault.
  const info = await box.app("GET", "kuvo/info");
  assert.ok(info.json.features.includes("settings.vault"));
  // No vault yet: the page says where its first item is added (its key kept there).
  let items = rowOf(await page(), "items");
  assert.match(items.add.can.why, /Add its first item in Kuvo on your iPhone/);
  // A client makes it, and keeps its key.
  const { json: { key } } = await box.app("POST", "kuvo/vault/key", {});
  assert.ok(key);

  // Add Item: checked with the value masked, then added with it.
  items = rowOf(await page(), "items");
  const opened = opening(items, "add");
  const checked = await box.app("POST", "kuvo/settings/vault/items/add/check", { was: actionOf(items, "add").form.revision, values: { ...opened, name: "GitHub", site: "github.com", password: "•".repeat(SECRET.length) } });
  assert.deepEqual(checked.json, { fields: [], ready: true });
  const added = await act(items, "add", { ...opened, name: "GitHub", site: "github.com", username: "octo", password: SECRET });
  assert.equal(added.status, 200, added.text);
  assert.ok(!added.text.includes(SECRET));
  const listed = rowOf(added.json, "items").items;
  assert.deepEqual(listed.map(each => [each.title, each.line]), [["GitHub", "Sign-in · octo · github.com"]]);
  let row = listed[0];

  // Show: only through the vault's values route, which notes it.
  const password = actionOf(row, "edit").form.fields.find(field => field.id === "password");
  assert.equal(password.kind, "secret");
  const shown = await box.app("GET", password.shows.route);
  assert.equal(shown.json.values[password.shows.field], SECRET);
  assert.match(onDisk(), /"what":"shown"/);

  // The page followed is sent again as it changes, without a value.
  const followed = box.follow();
  try {
    assert.ok(rowOf(JSON.parse(await followed.next()), row.id));
    const saved = await act(row, "edit", { ...opening(row, "edit"), name: "GitHub Work", password: NEW_SECRET });
    assert.equal(saved.status, 200, saved.text);
    let line;
    do line = await followed.next(); while (!line.includes("GitHub Work"));
    assert.ok(!line.includes(NEW_SECRET));
  } finally {
    followed.stop();
  }
  assert.equal((await box.app("GET", password.shows.route)).json.values.password, NEW_SECRET);

  // Who Can Use It: Scout may use it, then on its own.
  row = rowOf(await page(), row.id);
  assert.equal((await act(row, "who", { ...opening(row, "who"), "add-bot": "bot_scout" })).status, 200);
  row = rowOf(await page(), row.id);
  assert.deepEqual(row.badge, { words: "Asks", tone: "quiet" });
  assert.equal((await act(row, "who", { ...opening(row, "who"), "bot-bot_scout": "own" })).status, 200);
  const grants = (await box.app("GET", "kuvo/vault/grants")).json.grants;
  assert.deepEqual(grants.map(grant => [grant.who.bot, grant.how]), [["bot_scout", "own"]]);

  // Pause, from its switch.
  const paused = await box.app("PATCH", "kuvo/settings/vault", { row: "pause", value: true, was: rowOf(await page(), "pause").revision });
  assert.equal(paused.status, 200, paused.text);
  assert.equal((await box.app("GET", "kuvo/vault")).json.state.paused, true);

  // Anything else on the machine reads nothing of it, and changes nothing.
  const outside = routed("GET", "kuvo/settings/vault");
  assert.equal(outside.status, 200);
  assert.deepEqual(outside.json.groups, []);
  assert.equal(outside.json.notes[0].words, "Only Kuvo Code on your Mac or iPhone can do this.");
  // (Its page lists nothing to it, so there's nothing there to delete.)
  assert.equal(routed("POST", `kuvo/settings/vault/${row.id}/delete`, { was: row.revision }).status, 404);
  assert.equal((await box.app("GET", "kuvo/vault")).json.items.length, 1);

  // Restarted, it's locked: the page says so and lists nothing, until a client's key unlocks it.
  await box.stop();
  box = await start();
  const locked = await page();
  assert.deepEqual(locked.groups, []);
  assert.match(locked.notes[0].words, /vault is locked since it started again/);
  assert.equal((await box.app("POST", "kuvo/vault/unlock", { key })).status, 200);
  row = rowOf(await page(), row.id);
  assert.equal(row.title, "GitHub Work");

  // Deleted, with its values and grants.
  const deleted = await act(row, "delete");
  assert.equal(deleted.status, 200, deleted.text);
  assert.deepEqual(rowOf(deleted.json, "items").items, []);
  assert.deepEqual((await box.app("GET", "kuvo/vault/grants")).json.grants, []);

  // The activity said what was done, never a value.
  const titles = rowOf(await page(), "activity").items.map(each => each.title);
  assert.deepEqual(titles.slice(0, 2), ["Removed “GitHub Work”", "The vault was paused"]);
  for (const secret of [SECRET, NEW_SECRET]) assert.ok(!onDisk().includes(secret));
  await box.stop();
});
