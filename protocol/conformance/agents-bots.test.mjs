// Bots on a coding agent's model (PROTOCOL.md, `kuvo/bots`): made with that agent, its conversation
// kept as a bot's (its rules, its notes), told what it is, and greeting you; and Add Agent in the
// Marketplace makes one the same way. Claude Code and Codex, each with its stand-in.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { boxBinary, readJSON, request, until, write } from "./adapter.mjs";
import { provider as claude, scriptedClaude } from "./claude.mjs";
import { provider as codex, scriptedCodex } from "./codex.mjs";

const headers = { "x-kuvo-from": "app" };
const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);
const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

const agents = [
  ["Claude Code", claude, "opus", scriptedClaude],
  ["Codex", codex, "gpt-test", scriptedCodex],
];

for (const [name, providerID, id, scripted] of agents) {
  test(`a bot on ${name}'s model is made with it, kept as a bot's, told what it is, and greets you; Add Agent makes one too`, { timeout: 120000, skip }, async () => {
    const harness = await scripted({});
    try {
      write(path.join(harness.home, "bin", "bot-words.json"), fs.readFileSync(words));
      const listener = await harness.start();
      const ask = (method, route, body) => request(listener, method, route, body, headers, 45000);
      const model = { providerID, id };
      await until(async () => (await ask("GET", "kuvo/models/offered?refresh=1")).json?.models?.some(each => each.providerID === providerID && each.id === id),
        `${name}'s model was never offered.`, 45000, 250);

      const made = await ask("POST", "kuvo/bots", { name: "Scout", model, request: `agents-bot-${providerID}-request` });
      assert.equal(made.status, 201, made.text);
      const { bot } = made.json;
      assert.equal(bot.name, "Scout");
      assert.equal(bot.agents, true, "it runs with the coding agents");
      assert.equal(bot.model.providerID, providerID);
      const folder = path.join(harness.home, "bots", bot.id);
      assert.equal(fs.readFileSync(path.join(folder, "via"), "utf8"), "bridge");
      const session = fs.readFileSync(path.join(folder, "session"), "utf8").trim();
      assert.equal(session, bot.session);
      // Its conversation: a bot's, with its rules for its folder.
      const saved = readJSON(path.join(harness.home, "agents", "sessions", session, "session.json"));
      assert.equal(saved.metadata.kuvo.bot, bot.id);
      assert.equal(typeof saved.metadata.kuvo.approval, "string");
      assert.ok(saved.permissions.some(rule => rule.action === "external_directory" && rule.resource === path.join(folder, "*") && rule.effect === "allow"), JSON.stringify(saved.permissions));
      // Told what it is, as its agent is told Kuvo Code's instructions.
      const agent = readJSON(path.join(harness.home, "agents", "sessions", session, "agent.json"));
      assert.match(agent.kuvoInstructions["kuvo.bot"], /Scout/);
      // It writes first: its greeting starts its turn.
      await until(async () => {
        const rows = (await ask("GET", `kuvo/agents/api/session/${session}/message`)).json?.data ?? [];
        return rows.some(row => row.type === "synthetic") && rows.some(row => row.type === "idle");
      }, "It never greeted you.", 45000, 250);
      // (Its agent was told what it is as its turn started.)
      const heard = JSON.stringify(providerID === codex ? harness.sent("thread/start") : harness.heard());
      assert.match(heard, /Scout/, "its agent wasn't told what it is");

      // Add Agent in the Marketplace, on the same model.
      const details = await ask("GET", "kuvo/marketplace/agents/coding-reviewer");
      assert.equal(details.status, 200, details.text);
      const here = details.json.computers[0].key;
      const options = await ask("GET", `kuvo/marketplace/agents/coding-reviewer/options?computer=${encodeURIComponent(here)}`);
      assert.ok(options.json.models.some(each => each.providerID === providerID && each.id === id), JSON.stringify(options.json.models));
      const added = await ask("POST", "kuvo/marketplace/agents/coding-reviewer/add", { computer: here, version: "1.0.0", request: `agents-add-${providerID}-request`, model });
      assert.equal(added.status, 201, added.text);
      assert.equal(added.json.bot.agents, true);
      assert.equal(added.json.bot.job, "Review changes for regressions and summarize findings for the author.");
      const reviewer = readJSON(path.join(harness.home, "agents", "sessions", added.json.bot.session, "agent.json"));
      assert.match(reviewer.kuvoInstructions["kuvo.bot"], /Review a selected diff/);
    } catch (error) {
      throw new Error([error.message, harness.log()].join("\n"), { cause: error });
    } finally {
      await harness.close();
    }
  });
}
