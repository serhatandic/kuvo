// A coding-agent conversation as the app reads and changes it outside its turns: which have a run
// under way (`kuvo/agents/api/session/active`), one with its context and where it's summarized
// (`kuvo/agents/api/session/:id`), and its model (`POST …/:id/model`). Real: each box and its
// sessions. Stand-in: Codex.
import assert from "node:assert/strict";
import { test } from "node:test";
import { boxBinary, request, until } from "./adapter.mjs";
import { provider, scriptedCodex } from "./codex.mjs";

const headers = { "x-kuvo-from": "app" };
const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

async function withCodex(prepare, check) {
  const codex = await scriptedCodex();
  try {
    prepare(codex);
    await codex.start();
    const ask = async (method, route, body) => request(codex.listener, method, route, body, headers, 30000);
    await check(codex, ask);
  } catch (error) {
    throw new Error([error.message, codex.log()].join("\n"), { cause: error });
  } finally {
    await codex.close();
  }
}

test("which conversations have a run under way: one while its turn runs, not once it's over", { skip, timeout: 60000 }, () =>
  withCodex((codex) => {
    codex.script("keep working", [{ until: "turn-go" }, ["item/completed", { item: { type: "agentMessage", id: "a1", text: "Done." } }]]);
    codex.seed("ses_active");
    codex.seed("ses_resting");
  }, async (codex, ask) => {
    await codex.prompt("ses_active", "keep working");
    const busy = await until(async () => {
      const answer = await ask("GET", "kuvo/agents/api/session/active");
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data.ses_active && answer.json;
    }, "Its run was never said to be under way.");
    assert.deepEqual(busy, { data: { ses_active: { type: "busy" } } });
    const switching = await ask("GET", "kuvo/agents/api/session/active?release-switch=1");
    assert.deepEqual(switching.json, { data: { ses_active: { type: "busy" } }, engineTurns: [] });
    codex.go("turn-go");
    await codex.idles("ses_active");
    await until(async () => {
      const answer = await ask("GET", "kuvo/agents/api/session/active");
      return Object.keys(answer.json.data).length === 0;
    }, "Its run was still said to be under way.");
  }));

test("a conversation as clients read it: itself, where it's summarized, and as saved with context=0; one that isn't here is refused", { skip, timeout: 60000 }, () =>
  withCodex((codex) => codex.seed("ses_read"), async (codex, ask) => {
    const read = await ask("GET", "kuvo/agents/api/session/ses_read");
    assert.equal(read.status, 200, read.text);
    assert.equal(read.json.data.id, "ses_read");
    assert.deepEqual(read.json.data.model, { providerID: provider, id: "gpt-test" });
    assert.ok(read.json.data.summarize, JSON.stringify(read.json.data));
    const saved = await ask("GET", "kuvo/agents/api/session/ses_read?context=0");
    assert.equal(saved.status, 200, saved.text);
    assert.equal(saved.json.data.id, "ses_read");
    const missing = await ask("GET", "kuvo/agents/api/session/ses_nowhere");
    assert.equal(missing.status, 404, missing.text);
    assert.equal(typeof missing.json.message, "string");
  }));

test("a conversation's model: from its next turn, said once in its conversation, the same one changes nothing, and what isn't a model is refused", { skip, timeout: 60000 }, () =>
  withCodex((codex) => codex.seed("ses_model"), async (codex, ask) => {
    const route = "kuvo/agents/api/session/ses_model/model";
    const picked = { providerID: provider, id: "gpt-other", variant: "high" };
    const chosen = await ask("POST", route, { model: picked });
    assert.equal(chosen.status, 200, chosen.text);
    assert.deepEqual(chosen.json, {});
    const read = await ask("GET", "kuvo/agents/api/session/ses_model?context=0");
    assert.deepEqual(read.json.data.model, picked);
    assert.equal(read.json.data.metadata?.kuvoAgent, "codex");
    // The same again: nothing more.
    assert.equal((await ask("POST", route, { model: picked })).status, 200);
    const switched = (await codex.rows("ses_model")).filter((row) => row.type === "model-switched");
    assert.equal(switched.length, 1, JSON.stringify(switched));
    assert.deepEqual([switched[0].model, switched[0].previous], [picked, { providerID: provider, id: "gpt-test" }]);

    const notModel = await ask("POST", route, { model: { providerID: provider } });
    assert.deepEqual([notModel.status, notModel.json?.message], [400, "That isn't a model."]);
    const missing = await ask("POST", "kuvo/agents/api/session/ses_nowhere/model", { model: picked });
    assert.equal(missing.status, 404, missing.text);
  }));
