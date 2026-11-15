// Bots have no keys of their own (#726; PROTOCOL.md, "Bots have no keys of their own"): what an
// earlier box kept of them goes as this one starts, on a machine (its files; this Mac's Keychain:
// bot-keys-gone-host.test.js). A bot that had its own key goes on with the integration you
// connected on its computer; one whose integration you haven't connected there is told so, to say
// it plainly when its work needs it; and the routes that kept such keys are gone.
import "./test-safety.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { boxRoute, script, served } from "./box-stand-ins.js";
import { removeAtEnd } from "./at-end.js";

// The box at `~/.kuvo-code`, as on a machine.
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-keys-gone-")));
process.env.HOME = root;
const home = path.join(root, ".kuvo-code");
process.env.KUVO_HOME = home;
for (const name of ["KUVO_HOST_URL", "KUVO_HOST_PASSWORD", "KUVO_KEYS", "KUVO_BOX", "KUVO_BOTS", "KUVO_INTEGRATIONS", "KUVO_KEYS_ON_HOST"]) delete process.env[name];
const link = await import(script);
removeAtEnd(root);

const write = (file, text) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, text); };
const read = file => { try { return fs.readFileSync(file, "utf8"); } catch { return null; } };
const json = file => JSON.parse(fs.readFileSync(file, "utf8"));

const TODOIST = "KUVO_KEY_TODOIST_API_KEY", GITHUB = "KUVO_KEY_GITHUB_TOKEN";
const shared = path.join(home, "box", "keys"), botKeys = path.join(home, "box", "bot-keys");
const settingsFile = path.join(home, "box", "opencode.json");
const todoistYours = { type: "local", command: ["kuvo-npx", "@doist/todoist-mcp@1.0.0"], environment: { API_KEY: `{file:~/.kuvo-code/box/keys/${TODOIST}}` }, codemode: false };
const githubYours = { type: "remote", url: "https://api.githubcopilot.com/mcp/", headers: { Authorization: `Bearer {file:~/.kuvo-code/box/keys/${GITHUB}}` }, codemode: false };
const notes = { type: "local", command: ["notes"] };

// The catalog a Mac leaves on a machine, what a box tells its bots with, and a machine's kit.
write(path.join(home, "bin", "integrations.json"), JSON.stringify({ integrations: [
  { id: "todoist", title: "Todoist", summary: "Tasks", kind: "Code and Work", symbol: "checklist", site: "https://todoist.com", how: "Open source · your key", way: "package",
    keys: [{ name: "API_KEY", label: "API token", prompt: "From Todoist" }], variables: { API_KEY: TODOIST }, settings: todoistYours },
  { id: "github", title: "GitHub", summary: "Code", kind: "Code and Work", symbol: "chevron.left.forwardslash.chevron.right", site: "https://github.com", how: "Official · your token", way: "token",
    keys: [{ name: "TOKEN", label: "Token", prompt: "ghp_…" }], variables: { TOKEN: GITHUB }, settings: githubYours },
] }));
fs.copyFileSync(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/bot-words.json", import.meta.url), path.join(home, "bin", "bot-words.json"));
write(path.join(home, "bots", "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], startingModel: { id: "big-pickle", providerID: "opencode" }, projects: [], rules: [], timeZone: "Europe/Istanbul" }));

const folderOf = id => path.join(home, "bots", id);
const folderSettings = id => path.join(folderOf(id), "workspace", ".opencode", "opencode.json");
function bot(id, name) {
  write(path.join(folderOf(id), "bot.json"), JSON.stringify({ kuvo: 1, id, name, job: "", look: { color: "mint", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z", updated: "2026-10-01T09:00:00Z" }));
  write(path.join(folderOf(id), "session"), `ses_${id}`);
  fs.mkdirSync(path.join(folderOf(id), "workspace"), { recursive: true });
}
/** A server as an earlier box wrote it in a bot's folder's settings for its own key. */
const own = (server, variable, id) => JSON.parse(JSON.stringify(server).replaceAll(`box/keys/${variable}`, `box/bot-keys/${variable}__${id.toUpperCase()}`));

/** A machine as an earlier box left it: you connected Todoist (not GitHub); Scout has its own keys
 * for both, beside a server and instructions you wrote in its folder by hand; Atlas names a server
 * of its own by hand, with no key of the box's; and a deleted bot's key was still to go. */
function before() {
  for (const name of ["box", "bots/bot_scout", "bots/bot_atlas", "run"]) fs.rmSync(path.join(home, name), { recursive: true, force: true });
  write(settingsFile, JSON.stringify({ mcp: { servers: { todoist: todoistYours } } }));
  write(path.join(shared, TODOIST), "tok_yours");
  bot("bot_scout", "Scout");
  write(folderSettings("bot_scout"), JSON.stringify({ instructions: ["notes.md"], mcp: { servers: { todoist: own(todoistYours, TODOIST, "bot_scout"), github: own(githubYours, GITHUB, "bot_scout"), notes } } }));
  write(path.join(botKeys, `${TODOIST}__BOT_SCOUT`), "tok_scout");
  write(path.join(botKeys, `${GITHUB}__BOT_SCOUT`), "ghp_scout");
  bot("bot_atlas", "Atlas");
  write(folderSettings("bot_atlas"), JSON.stringify({ mcp: { servers: { todoist: { type: "remote", url: "https://example.com/mcp" } } } }));
  write(path.join(botKeys, `${TODOIST}__BOT_GONE`), "tok_gone");
  write(path.join(home, "run", "bot-keys-left.json"), JSON.stringify({ bots: ["bot_gone"] }));
}

/** An engine as a bot's run needs it: what each conversation is told, kept and given back. */
async function engineTelling(t) {
  const told = new Map();
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      const reply = value => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
      const entries = request.url.match(/^\/api\/experimental\/session\/([^/]+)\/instructions\/entries(\/kuvo\.bot)?$/);
      if (entries && request.method === "PUT") told.set(entries[1], JSON.parse(body).value);
      if (entries && request.method === "GET") return reply(told.has(entries[1]) ? [{ key: "kuvo.bot", value: told.get(entries[1]) }] : []);
      const one = request.url.match(/^\/api\/session\/([^/]+)$/);
      reply(one ? { data: { id: one[1], permissions: [] } } : { data: {} });
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  return { target: { port: server.address().port, password: "p" }, told };
}

test("a bot that had keys of its own: they're removed from where the machine kept them, and it goes on with the integration you connected", async () => {
  before();
  assert.equal(await link.dropBotsOwnKeys(), true);

  assert.equal(fs.existsSync(botKeys), false, "every bot's own key, a deleted bot's too, and their folder");
  assert.equal(fs.existsSync(path.join(home, "run", "bot-keys-left.json")), false);
  assert.equal(read(path.join(shared, TODOIST)), "tok_yours", "your own key stays");
  assert.deepEqual(json(settingsFile), { mcp: { servers: { todoist: todoistYours } } }, "and your connection as it was");
  // Its folder no longer names a server for either: its conversation gets the computer's own
  // Todoist, your connection. What you wrote there by hand stays.
  assert.deepEqual(json(folderSettings("bot_scout")), { instructions: ["notes.md"], mcp: { servers: { notes } } });
  assert.deepEqual(json(folderSettings("bot_atlas")), { mcp: { servers: { todoist: { type: "remote", url: "https://example.com/mcp" } } } }, "a server written by hand, naming no key of a bot's own, stays");
  for (const file of fs.readdirSync(home, { recursive: true })) {
    const text = fs.statSync(path.join(home, file)).isFile() ? read(path.join(home, file)) : "";
    assert.ok(!/tok_scout|ghp_scout|tok_gone/.test(text), `${file} keeps one of their keys`);
  }

  // Again: nothing left to do, and nothing written.
  const stamp = fs.statSync(folderSettings("bot_scout")).mtimeMs;
  assert.equal(await link.dropBotsOwnKeys(), true);
  assert.equal(fs.statSync(folderSettings("bot_scout")).mtimeMs, stamp);
});

test("a bot whose integration you haven't connected is told so, to say it plainly when its work needs it; connected, it's told no more", async t => {
  before();
  await link.dropBotsOwnKeys();
  const engine = await engineTelling(t);
  await link.freshenBot(engine.target, "ses_bot_scout");
  const told = engine.told.get("ses_bot_scout");
  assert.match(told, /^You are Scout/);
  assert.match(told, /You used to have a key of your own for GitHub\./);
  assert.match(told, /GitHub isn't connected there, so you have no tools for it now\./);
  assert.match(told, /say plainly that it isn't connected on this machine, and ask the user to connect it in Settings › Integrations\.$/);
  assert.doesNotMatch(told, /Todoist/, "nothing said of the one you connected: it just uses it");
  assert.deepEqual(json(path.join(folderOf("bot_scout"), "keys-gone.json")), { integrations: ["github"] }, "only the one still to connect is noted");
  // A bot that never had a key of its own is told nothing of this.
  await link.freshenBot(engine.target, "ses_bot_atlas");
  assert.doesNotMatch(engine.told.get("ses_bot_atlas"), /key of your own/);

  // You connect GitHub on this machine: at its next run it's told as any bot is, and the note goes.
  assert.equal((await link.answerIntegrations("POST", "kuvo/integrations/github", { keys: { TOKEN: "ghp_yours" } }, null, undefined, undefined, () => {})).status, 200);
  await link.freshenBot(engine.target, "ses_bot_scout");
  assert.doesNotMatch(engine.told.get("ses_bot_scout"), /key of your own|isn't connected/);
  assert.match(engine.told.get("ses_bot_scout"), /^You are Scout/);
  assert.equal(fs.existsSync(path.join(folderOf("bot_scout"), "keys-gone.json")), false);
});

test("while an update is on trial nothing is removed (a key can't come back with the version before): it goes once the update is confirmed", async () => {
  before();
  write(path.join(home, "run", "update-trial"), "{}");
  const was = read(folderSettings("bot_scout"));
  assert.equal(await link.dropBotsOwnKeys(), false);
  assert.equal(read(path.join(botKeys, `${TODOIST}__BOT_SCOUT`)), "tok_scout");
  assert.equal(read(folderSettings("bot_scout")), was, "its settings still name its keys, which are still there");
  fs.rmSync(path.join(home, "run", "update-trial"));
  assert.equal(await link.dropBotsOwnKeys(), true);
  assert.equal(fs.existsSync(botKeys), false);
  assert.deepEqual(json(folderSettings("bot_scout")).mcp.servers, { notes });
});

test("a bot's folder's settings that named only its own keys go with them, and ones that aren't plain JSON are left alone", async () => {
  before();
  write(folderSettings("bot_scout"), JSON.stringify({ mcp: { servers: { github: own(githubYours, GITHUB, "bot_scout") } } }));
  write(folderSettings("bot_atlas"), "// mine\n{}");
  assert.equal(await link.dropBotsOwnKeys(), true);
  assert.equal(fs.existsSync(path.dirname(folderSettings("bot_scout"))), false, "the file, and the folder made for it");
  assert.equal(read(folderSettings("bot_atlas")), "// mine\n{}");
});

test("as its box starts, with no client there, they go; and the routes that kept them are gone, for a phone and over SSH alike", async () => {
  before();
  const box = await served(process.env);
  try {
    // Done before it says where it is, so before its engine starts or a client asks anything.
    assert.equal(fs.existsSync(botKeys), false);
    assert.deepEqual(json(folderSettings("bot_scout")).mcp.servers, { notes });
    for (const [method, route, body] of [
      ["GET", "kuvo/bots/bot_scout/keys"], ["DELETE", "kuvo/bots/bot_scout/keys"], ["GET", "kuvo/bots/bot_scout/keys/todoist"],
      ["PUT", "kuvo/bots/bot_scout/keys/todoist", { keys: { API_KEY: "tok_again" } }], ["DELETE", "kuvo/bots/bot_scout/keys/todoist"],
      ["POST", "kuvo/bots/keys/refresh"], ["GET", "kuvo/box/kept-keys?bot=bot_scout"],
    ]) assert.equal((await box.route(method, route, body)).status, 404, `${method} ${route}`);
    assert.equal(fs.existsSync(botKeys), false, "nothing kept for it");
    // The bot itself is still there, and listed without a word of keys.
    const listed = (await box.route("GET", "kuvo/bots")).value.bots.find(each => each.id === "bot_scout");
    assert.equal(listed.name, "Scout");
    assert.ok(!listed.deletes.some(line => line.what === "keys"));
    assert.ok(!(await box.route("GET", "kuvo/info")).value.features.includes("bots.keys"));
  } finally {
    await box.stop();
  }
  assert.equal((await boxRoute({ method: "PUT", path: "kuvo/bots/bot_scout/keys/todoist", body: { keys: { API_KEY: "tok_again" } } }, process.env)).status, 404);
  assert.equal(fs.existsSync(botKeys), false);
});
