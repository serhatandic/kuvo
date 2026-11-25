// Add Agent in the Marketplace (`kuvo/marketplace/agents/<id>/options` and `…/add`): what adding
// one offers here (the models that run here, its routines, each off), and the bot it makes, once
// for a request: its name and job from the catalog, its model, its scope and its routines.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { request, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);

test("Add Agent offers the models that run here and makes its bot once, with its job, scope and a paused routine", { timeout: 90000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const { home, engine } = lab;
  write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const model = { providerID: "test-account", id: "test-model" };
  engine.providers.push({ id: model.providerID });
  engine.models.push({ ...model, name: "Test Model" });
  engine.defaultModel = model;
  const ready = await lab.start().ready();
  const ask = (method, route, body) => request(ready, method, route, body, { "x-kuvo-from": "app" });
  const agent = "kuvo/marketplace/agents/coding-reviewer";

  const details = await ask("GET", agent);
  assert.equal(details.status, 200, details.text);
  const here = details.json.computers[0].key;
  const options = await ask("GET", `${agent}/options?computer=${encodeURIComponent(here)}`);
  assert.equal(options.status, 200, options.text);
  assert.ok(options.json.models.some(each => each.providerID === model.providerID && each.id === model.id), JSON.stringify(options.json.models));
  assert.ok(options.json.models.every(each => each.providerID !== "auto" && !each.retiring && !each.hidden));
  assert.deepEqual(options.json.missing, []);
  assert.deepEqual(options.json.routines.map(each => [each.id, each.enabled]), [["weekly", false]]);
  assert.equal(options.json.blocked, undefined);

  const body = { computer: here, version: "1.0.0", request: "stable-marketplace-request", model };
  const refusals = [
    [{ ...body, request: "another-marketplace-request", routines: ["nightly"] }, 400],
    [{ ...body, request: "another-marketplace-request", version: "0.9.0" }, 409],
    [{ ...body, request: "another-marketplace-request", model: { providerID: "test-account", id: "not-here" } }, 422],
  ];
  for (const [given, status] of refusals) {
    const refused = await ask("POST", `${agent}/add`, given);
    assert.equal(refused.status, status, `${JSON.stringify(given)}: ${refused.text}`);
  }
  assert.ok(!fs.existsSync(path.join(home, "bots")) || fs.readdirSync(path.join(home, "bots")).every(name => !name.startsWith("bot_")), "nothing was made for those");

  const made = await ask("POST", `${agent}/add`, body);
  assert.equal(made.status, 201, made.text);
  const { bot } = made.json;
  assert.equal(made.json.firstMessage, null);
  assert.equal(bot.name, "Coding Reviewer");
  assert.deepEqual(bot.model, model);
  assert.equal(bot.job, "Review changes for regressions and summarize findings for the author.");
  assert.equal(bot.paused, true, "its routine waits until you turn it on");
  const folder = path.join(home, "bots", bot.id);
  const wakes = fs.readdirSync(path.join(folder, "wakes"));
  assert.equal(wakes.length, 1);
  assert.match(wakes[0], /^\d+-[0-9a-f]{16}-1w$/);
  assert.equal(fs.readFileSync(path.join(folder, "summaries", wakes[0].split("-")[1]), "utf8"), "Check for changes awaiting review");
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(folder, "workspace", ".kuvo-marketplace.json"), "utf8")),
    { agent: "coding-reviewer", version: "1.0.0", request: body.request, apps: [], skills: ["review-changes"] });
  const told = engine.asked.findLast(each => each.method === "PUT" && each.path.endsWith("/kuvo.bot"))?.json?.value ?? "";
  assert.match(told, /Review a selected diff/);

  // The same request again: the same bot, made once.
  const again = await ask("POST", `${agent}/add`, body);
  assert.equal(again.status, 201, again.text);
  assert.equal(again.json.bot.id, bot.id);
  assert.equal(fs.readdirSync(path.join(home, "bots")).filter(name => name.startsWith("bot_")).length, 1);
});
