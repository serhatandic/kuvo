// Bots on a coding agent's model (PROTOCOL.md, `kuvo/bots`): made with that agent, its conversation
// kept as a bot's (its rules, its notes), told what it is, and greeting you; changed, brought here,
// told again, woken and deleted through it; and Add Agent in the Marketplace makes one the same way.
// Claude Code and Codex, each with its stand-in.
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
  test(`a bot on ${name}'s model is made, changed, told again, woken and deleted with it; Add Agent makes one too`, { timeout: 120000, skip }, async () => {
    const harness = await scripted({});
    try {
      write(path.join(harness.home, "bin", "bot-words.json"), fs.readFileSync(words));
      write(path.join(harness.home, "bin", "kuvo-wake"), fs.readFileSync(new URL("../../box/src/keeper/kuvo-wake", import.meta.url)), { mode: 0o755 });
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

      // Changed: renamed and given a job, its conversation renamed and told so.
      const changed = await ask("PATCH", `kuvo/bots/${bot.id}`, { name: "Scout Two", jobLine: "Watch the build" });
      assert.equal(changed.status, 200, changed.text);
      assert.equal(changed.json.bot.name, "Scout Two");
      assert.equal(readJSON(path.join(harness.home, "agents", "sessions", session, "session.json")).title, "Scout Two");
      const told = () => readJSON(path.join(harness.home, "agents", "sessions", session, "agent.json")).kuvoInstructions["kuvo.bot"];
      assert.match(told(), /Scout Two/);
      assert.match(told(), /Watch the build/);
      // Its effort, one of its model's own, as its coding agent lists them.
      if (providerID === codex) {
        const effort = await ask("PATCH", `kuvo/bots/${bot.id}`, { effort: "high" });
        assert.equal(effort.status, 200, effort.text);
        assert.deepEqual(effort.json.bot.model, { providerID, id, variant: "high" });
        assert.equal((await ask("PATCH", `kuvo/bots/${bot.id}`, { effort: "max" })).status, 400, "not one of its efforts");
      }
      // Brought here (Move to Computer); and told again what it is as its words change (an update).
      const arrived = await ask("POST", `kuvo/bots/${bot.id}/arrived`, {});
      assert.equal(arrived.status, 200, arrived.text);
      const kit = JSON.parse(fs.readFileSync(words, "utf8"));
      kit.instructions = [...kit.instructions, "Words from an update."];
      write(path.join(harness.home, "bin", "bot-words.json"), JSON.stringify(kit));
      assert.equal((await ask("POST", "kuvo/bots/retell", {})).status, 200);
      await until(() => /Words from an update/.test(told()), "It wasn't told again what it is.", 15000, 100);

      // Its wake-ups reach its agent: one sent now (Wake Up Now), and one due, by the box's round.
      const agentHeard = () => JSON.stringify(providerID === codex ? harness.sent("turn/start") : harness.heard());
      const wakes = `kuvo/wakes/${encodeURIComponent(session)}`;
      const kept = await ask("POST", wakes, { due: Math.floor(Date.now() / 1000) + 3600, note: "Check the build" });
      assert.equal(kept.status, 200, kept.text);
      const wake = kept.json.wakes.find(each => each.note === "Check the build");
      const sent = await ask("POST", `${wakes}/now`, { wake: wake.id });
      assert.equal(sent.status, 200, sent.text);
      await until(() => agentHeard().includes("[Wake-up] Check the build"), "Wake Up Now never reached its agent.", 30000, 100);
      assert.deepEqual((await ask("GET", wakes)).json.wakes, [], "one that doesn't repeat goes once sent");
      write(path.join(folder, "wakes", `${Math.floor(Date.now() / 1000) + 2}-a1b2c3d4e5f60718`), "Look at the logs\n");
      await until(() => agentHeard().includes("[Wake-up] Look at the logs"), "Its due wake-up never reached its agent.", 45000, 250);
      await until(() => fs.readdirSync(path.join(folder, "wakes")).length === 0, "Its due wake-up stayed.", 15000, 100);

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

      // Deleted: its conversation goes with it.
      const deleted = await ask("DELETE", `kuvo/bots/${bot.id}`);
      assert.equal(deleted.status, 200, deleted.text);
      // A beta.1 Mac lets it go too, as it next connects.
      assert.equal(readJSON(path.join(harness.home, "bots", "deleted.json")).deleted[0], bot.id);
      await until(() => !fs.existsSync(path.join(harness.home, "agents", "sessions", session)), "Its conversation stayed.", 15000, 100);
      const left = fs.existsSync(path.join(harness.home, "run", "bots-left.json")) ? readJSON(path.join(harness.home, "run", "bots-left.json")).left : [];
      assert.notEqual(left.find(each => each.id === bot.id)?.conversation, true, "its conversation isn't left to go later");
    } catch (error) {
      throw new Error([error.message, harness.log()].join("\n"), { cause: error });
    } finally {
      await harness.close();
    }
  });
}
