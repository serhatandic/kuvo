// Bots have no keys of their own (#726), on this Mac's box, which keeps no keys itself: the ones an
// earlier Kuvo Code kept for bots are in the Mac's Keychain, which the box reaches through its app
// (the host interface; PROTOCOL.md, "Host interface"). Here a stand-in for the app keeps them in
// memory; the box is the real one. (A machine's files, and what a bot is told: bot-keys-gone.test.js.)
import "./test-safety.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { script, served, standInHost } from "./box-stand-ins.js";
import { removeAtEnd } from "./at-end.js";

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-keys-gone-host-")));
// (The app's folder, with its box in it, beside a home of the test's own.)
const support = path.join(root, "support"), home = path.join(support, "box"), bots = path.join(support, "bots");
const PASSWORD = "host-password-not-a-key";
const host = await standInHost(PASSWORD);
// The app's temporary folder, where the engine reads each key from a file the app writes.
const keyFiles = path.join(support, "app-temp", "integration-keys");
const env = {
  HOME: root, KUVO_BOX: "mac", KUVO_HOME: home, KUVO_BOTS: bots, KUVO_WAKES: path.join(support, "wakes"), KUVO_ENGINE_CONFIG: path.join(support, "opencode"),
  KUVO_HOST_URL: host.url, KUVO_HOST_PASSWORD: PASSWORD, KUVO_KEYS: keyFiles,
};
Object.assign(process.env, env);
const link = await import(script);
removeAtEnd(root);

const write = (file, text) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, text); };
const TODOIST = "KUVO_KEY_TODOIST_API_KEY";
const settings = path.join(bots, "bot_scout", "workspace", ".opencode", "opencode.json");
const notes = { type: "local", command: ["notes"] };
const botNames = () => [...host.keys.keys()].filter(name => name.includes("__")).sort();

/** This Mac as an earlier Kuvo Code left it: Scout's own Todoist key in the Keychain and its
 * folder's settings naming it, a key it kept for a machine's bot, a deleted bot's, and your own. */
function before() {
  fs.rmSync(bots, { recursive: true, force: true });
  write(path.join(bots, "bot_scout", "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_scout", name: "Scout", job: "", look: { color: "mint", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z", updated: "2026-10-01T09:00:00Z" }));
  write(path.join(bots, "bot_scout", "session"), "ses_scout");
  write(settings, JSON.stringify({ mcp: { servers: { todoist: { type: "local", command: ["kuvo-npx", "@doist/todoist-mcp@1.0.0"], environment: { API_KEY: `{file:${keyFiles}/${TODOIST}__BOT_SCOUT}` } }, notes } } }));
  host.keys.clear();
  host.keys.set(TODOIST, "tok_yours");
  host.keys.set("KUVO_KEY_SLACK_BOT_TOKEN", "xoxb_yours");
  host.keys.set(`${TODOIST}__BOT_SCOUT`, "tok_scout");
  host.keys.set("KUVO_KEY_GITHUB_TOKEN__BOT_HERALD0001", "ghp_a_machines_bot");
  host.keys.set(`${TODOIST}__BOT_GONE`, "tok_gone");
  host.asked.length = 0;
}

test("the keys this Mac's Keychain kept for bots go, every one, never read; your own stay, and the bot's folder stops naming its own", async () => {
  before();
  assert.equal(await link.dropBotsOwnKeys(), true);
  assert.deepEqual([...host.keys], [[TODOIST, "tok_yours"], ["KUVO_KEY_SLACK_BOT_TOKEN", "xoxb_yours"]]);
  assert.deepEqual(JSON.parse(fs.readFileSync(settings, "utf8")), { mcp: { servers: { notes } } });
  // Only their names were asked for, and only they were taken away: no key's value was read.
  assert.deepEqual(host.asked.map(each => each.call).sort().filter((call, i, all) => all.indexOf(call) === i), ["/keys/delete", "/keys/names"]);
  assert.deepEqual(host.asked.filter(each => each.call === "/keys/delete").map(each => each.name).sort(), ["KUVO_KEY_GITHUB_TOKEN__BOT_HERALD0001", `${TODOIST}__BOT_GONE`, `${TODOIST}__BOT_SCOUT`]);
  assert.equal(host.turnedAway, 0);
  assert.equal(fs.existsSync(path.join(home, "box", "bot-keys")), false);
  assert.equal(fs.existsSync(path.join(home, "box", "keys")), false, "no key in files of the box's own");
});

test("while its app isn't answering for the Keychain, the bot already uses your connection, and its keys go the next time they can be reached", async () => {
  before();
  host.down = true;
  try {
    assert.equal(await link.dropBotsOwnKeys(), false, "not done yet");
  } finally {
    host.down = false;
  }
  assert.deepEqual(JSON.parse(fs.readFileSync(settings, "utf8")), { mcp: { servers: { notes } } }, "its folder's settings first: none names a key that may go");
  assert.equal(botNames().length, 3, "still in the Keychain");
  assert.equal(await link.dropBotsOwnKeys(), true);
  assert.deepEqual(botNames(), []);
  assert.equal(host.keys.get(TODOIST), "tok_yours");
});

test("as this Mac's box starts: its bots' folders before it says where it is, their keys once its app answers; no key or password in what it prints", async () => {
  before();
  const gone = new Promise(resolve => { host.onDelete = () => { if (botNames().length === 0) resolve(); }; });
  // (Its password, which the box took out of this process's environment as it loaded here.)
  const box = await served({ ...process.env, ...env, KUVO_TEST_NO_PEERS: "1" });
  try {
    assert.deepEqual(JSON.parse(fs.readFileSync(settings, "utf8")), { mcp: { servers: { notes } } }, "before its engine starts");
    await gone;
    assert.deepEqual([...host.keys], [[TODOIST, "tok_yours"], ["KUVO_KEY_SLACK_BOT_TOKEN", "xoxb_yours"]]);
    assert.equal((await box.route("GET", "kuvo/bots/bot_scout/keys")).status, 404);
  } finally {
    host.onDelete = null;
    const output = await box.stop();
    assert.ok(!/tok_scout|tok_gone|ghp_a_machines_bot|tok_yours/.test(output), "no key's value in what it printed");
    assert.ok(!output.includes(PASSWORD), "nor its app's password");
  }
});
