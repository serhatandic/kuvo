// Bots' own keys, gone (PROTOCOL.md, "Bots have no keys of their own"): as a box starts, each bot's
// folder's settings lose the servers that name a key of its own, the keys go from where they were
// kept, and a bot is told while an integration it had one for isn't connected.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const files = new URL("../../box/test/fixtures/routes/box/", import.meta.url);
const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);

function bot(home, engine, id, name, settings) {
  const folder = path.join(home, "bots", id);
  const at = "2026-10-01T09:00:00Z";
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name, job: "", created: at, updated: at }));
  write(path.join(folder, "session"), `ses_${id}`);
  write(path.join(folder, "MEMORY.md"), `# ${name}'s memory\n`);
  write(path.join(folder, "workspace", ".opencode", "opencode.json"), settings);
  engine.sessions.push(sessionOf({ id: `ses_${id}`, title: name, directory: folder }));
  return folder;
}

test("a bot's old keys of its own go as the box starts, and it's told while that integration isn't connected; one whose settings can't be read keeps its own and is told so", { timeout: 90000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const { home, engine } = lab;
  for (const name of ["integrations.json", "mcp-catalog.json"]) write(path.join(home, "bin", name), fs.readFileSync(new URL(name, files)));
  write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
  // This computer's settings, with no GitHub connected.
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const scout = bot(home, engine, "bot_scout1", "Scout", `{
  // Written by an earlier box, with a comment, as the engine reads it.
  "mcp": { "servers": {
    "github": { "type": "remote", "headers": { "Authorization": "Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_GITHUB_TOKEN__BOT_SCOUT1}" } },
    "docs": { "type": "remote", "url": "https://docs.example" },
  } }
}`);
  const broken = bot(home, engine, "bot_broken1", "Broken", `{ "mcp": { "servers": { "linear": { "key": "{env:KUVO_KEY_LINEAR_KEY__BOT_BROKEN1}" } } /* never closed`);
  const keys = path.join(home, "box", "keys");
  write(path.join(keys, "KUVO_KEY_GITHUB_TOKEN__BOT_SCOUT1"), "scout's own");
  write(path.join(keys, "KUVO_KEY_LINEAR_KEY__BOT_BROKEN1"), "broken's own");
  write(path.join(keys, "KUVO_KEY_GITHUB_TOKEN"), "yours");
  write(path.join(home, "box", "bot-keys", "KUVO_KEY_GITHUB_TOKEN__BOT_GONE1"), "a deleted bot's");
  const ask = lab.connect(await lab.start().ready());
  const gone = (file) => !fs.existsSync(file);
  await until(() => gone(path.join(keys, "KUVO_KEY_GITHUB_TOKEN__BOT_SCOUT1")), "Scout's own key stayed.", 30000, 100);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(scout, "workspace", ".opencode", "opencode.json"), "utf8")),
    { mcp: { servers: { docs: { type: "remote", url: "https://docs.example" } } } });
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(scout, "keys-gone.json"), "utf8")), { integrations: ["github"] });
  assert.equal(fs.readFileSync(path.join(keys, "KUVO_KEY_GITHUB_TOKEN"), "utf8"), "yours", "your own key stays");
  assert.equal(fs.readFileSync(path.join(keys, "KUVO_KEY_LINEAR_KEY__BOT_BROKEN1"), "utf8"), "broken's own", "kept while its settings name it");
  assert.ok(gone(path.join(home, "box", "bot-keys")), "an earlier box's folder of bots' keys goes");
  // What each is told now.
  assert.equal((await ask("POST", "kuvo/bots/retell", {})).status, 200);
  const told = (id) => engine.asked.findLast(each => each.method === "PUT" && each.path.includes(`ses_${id}`) && each.path.endsWith("/kuvo.bot"))?.json?.value ?? "";
  await until(() => told("bot_scout1") && told("bot_broken1"), "Neither was told.", 30000, 100);
  assert.match(told("bot_scout1"), /You used to have a key of your own for GitHub\. .* isn't connected there/s);
  assert.match(told("bot_broken1"), /Your folder's settings file \(.*opencode\.json\) names a key of your own/);
  assert.doesNotMatch(told("bot_broken1"), /You used to have a key of your own/);
});

test("while an update is on trial nothing is taken away; an integration connected since is told no more", { timeout: 90000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const { home, engine } = lab;
  for (const name of ["integrations.json", "mcp-catalog.json"]) write(path.join(home, "bin", name), fs.readFileSync(new URL(name, files)));
  write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
  // GitHub connected here.
  write(path.join(home, "box", "opencode.json"), JSON.stringify({ mcp: { servers: { github: { type: "remote", url: "https://api.githubcopilot.com/mcp/" } } } }));
  const scout = bot(home, engine, "bot_scout1", "Scout", JSON.stringify({ mcp: { servers: { github: { key: "{env:KUVO_KEY_GITHUB_TOKEN__BOT_SCOUT1}" } } } }));
  write(path.join(scout, "keys-gone.json"), JSON.stringify({ integrations: ["github", "linear"] }));
  const key = path.join(home, "box", "keys", "KUVO_KEY_GITHUB_TOKEN__BOT_SCOUT1");
  write(key, "scout's own");
  write(path.join(home, "run", "update-trial"), "a release on trial");
  const ask = lab.connect(await lab.start().ready());
  assert.equal((await ask("POST", "kuvo/bots/retell", {})).status, 200);
  const told = () => engine.asked.findLast(each => each.method === "PUT" && each.path.includes("ses_bot_scout1") && each.path.endsWith("/kuvo.bot"))?.json?.value ?? "";
  await until(told, "It was never told.", 30000, 100);
  assert.ok(fs.existsSync(key), "kept while the update is on trial");
  assert.match(JSON.parse(fs.readFileSync(path.join(scout, "workspace", ".opencode", "opencode.json"), "utf8")).mcp.servers.github.key, /__BOT_SCOUT1/);
  // GitHub is connected: only Linear is still said, and the note keeps only it.
  assert.match(told(), /You used to have a key of your own for Linear\./);
  assert.doesNotMatch(told(), /GitHub\. /);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(scout, "keys-gone.json"), "utf8")), { integrations: ["linear"] });
});
