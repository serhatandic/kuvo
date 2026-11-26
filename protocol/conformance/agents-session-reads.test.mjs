// A coding-agent conversation as the app reads and changes it outside its turns: which have a run
// under way (`kuvo/agents/api/session/active`), one with its context and where it's summarized
// (`kuvo/agents/api/session/:id`), and its model (`POST …/:id/model`). Real: each box and its
// sessions. Stand-in: Codex.
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, openLine, request, until, write } from "./adapter.mjs";
import { askFirst, scriptedClaude } from "./claude.mjs";
import { providers, scriptedAcp } from "./acp.mjs";
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

test("a conversation's changes reach a client following the agents' feed on the line: its model, its mode, and an approval asked and answered", { skip, timeout: 60000 }, () =>
  withCodex((codex) => {
    codex.seed("ses_told", { permissions: askFirst });
    codex.script("ask first", [{ ask: ["item/commandExecution/requestApproval", { itemId: "c1", command: "make" }] }]);
  }, async (codex, ask) => {
    const watch = Buffer.from(JSON.stringify({ agents: "kuvo/agents/api/event" })).toString("base64url");
    const line = await openLine(codex.listener, "?watch=" + watch);
    try {
      const frames = line.frames();
      const seen = [];
      const next = async (wanted, what) => {
        for (;;) {
          const { value: frame, done } = await frames.next().catch((error) => {
            throw new Error("Waiting for " + what + ": " + JSON.stringify(seen), { cause: error });
          });
          assert.equal(done, false, "The line ended waiting for " + what);
          if (frame?.t !== "agents" || !frame.d?.type) continue;
          seen.push(frame.d.type);
          if (wanted(frame.d)) return frame.d;
        }
      };
      await next((event) => event.type === "server.connected", "the feed");
      const picked = { providerID: provider, id: "gpt-other" };
      assert.equal((await ask("POST", "kuvo/agents/api/session/ses_told/model", { model: picked })).status, 200);
      const model = await next((event) => event.type === "session.model.selected", "the model");
      assert.deepEqual(model.data, { sessionID: "ses_told", model: picked, previous: { providerID: provider, id: "gpt-test" } });
      assert.equal(typeof model.location?.directory, "string");
      assert.equal((await ask("POST", "kuvo/agents/api/session/ses_told/agent", { agent: "plan" })).status, 200);
      const mode = await next((event) => event.type === "session.agent.selected", "the mode");
      assert.deepEqual([mode.data.sessionID, mode.data.agent], ["ses_told", "plan"]);

      await codex.prompt("ses_told", "ask first");
      const asked = await next((event) => event.type === "permission.asked", "the approval");
      assert.equal(asked.data.sessionID, "ses_told");
      const replied = await ask("POST", `kuvo/agents/api/session/ses_told/permission/${encodeURIComponent(asked.data.id)}/reply`, { decision: "once" });
      assert.ok(replied.status < 300, replied.text);
      const answered = await next((event) => event.type === "permission.replied", "its answer");
      assert.deepEqual(answered.data, { sessionID: "ses_told", requestID: asked.data.id });
    } finally {
      line.close();
    }
  }));

test("a new agent is told what came before, but its next message is just the user's words", { skip, timeout: 60000 }, () =>
  withCodex((codex) => {
    codex.seed("ses_carried");
    write(path.join(codex.home, "agents/sessions/ses_carried/messages.json"), JSON.stringify([
      { type: "user", id: "msg_old", time: { created: 1 }, text: "Please explain the parser." },
      { type: "assistant", id: "msg_reply", time: { created: 2, completed: 3 }, agent: "build", model: { providerID: "claude-code", id: "haiku" }, content: [
        { type: "text", id: "part_old", text: "The parser reads a token." },
        { type: "tool", id: "call_old", name: "edit", state: { status: "completed", input: { path: "src/parser.ts" }, content: [] } },
      ] },
      { type: "idle", id: "msg_idle", outcome: "succeeded", time: { created: 4 } },
    ]));
  }, async (codex) => {
    await codex.prompt("ses_carried", "What about the lexer?");
    await codex.idles("ses_carried", 2);
    const words = (sent) => sent.params.input[0].text.split("\n\n<from-phone>")[0];
    const first = words(codex.sent("turn/start")[0]);
    assert.match(first, /This conversation began with another agent/);
    assert.match(first, /Please explain the parser\./);
    assert.match(first, /The parser reads a token\./);
    assert.match(first, /\[File change\]\nsrc\/parser\.ts/);
    assert.ok(first.indexOf("The parser reads a token.") < first.indexOf("src/parser.ts"));
    assert.ok(first.endsWith("User message:\nWhat about the lexer?"), first);
    const rows = await codex.rows("ses_carried");
    assert.equal(rows.filter((row) => row.type === "user").at(-1).text.split("\n\n<from-phone>")[0], "What about the lexer?");
    await codex.prompt("ses_carried", "And its output?");
    await codex.idles("ses_carried", 3);
    const next = words(codex.sent("turn/start")[1]);
    assert.equal(next, "And its output?");
  }));

test("a missing native conversation starts over with its transcript, not just the next question", { skip, timeout: 60000 }, () =>
  withCodex((codex) => {
    const native = "0199a000-0000-7000-8000-000000000001";
    codex.seed("ses_missing_native", { state: { codexThread: native } });
    const folder = path.join(codex.home, "agents/sessions/ses_missing_native");
    write(path.join(folder, "agent.json"), JSON.stringify({
      threads: { [provider]: { state: { codexThread: native }, seen: "msg_idle" } },
    }));
    write(path.join(folder, "messages.json"), JSON.stringify([
      { type: "user", id: "msg_old", time: { created: 1 }, text: "Remember the first question." },
      { type: "assistant", id: "msg_answer", time: { created: 2, completed: 3 }, agent: "build", model: { providerID: provider, id: "gpt-test" }, content: [
        { type: "text", id: "part_answer", text: "Here is the first answer." },
      ] },
      { type: "idle", id: "msg_idle", outcome: "succeeded", time: { created: 4 } },
    ]));
    write(path.join(codex.root, "codex-gone.json"), JSON.stringify([native]));
  }, async (codex) => {
    await codex.prompt("ses_missing_native", "Continue from there.");
    await codex.idles("ses_missing_native", 2);
    const prompt = codex.sent("turn/start")[0].params.input[0].text;
    assert.match(prompt, /Remember the first question\./);
    assert.match(prompt, /Here is the first answer\./);
    assert.match(prompt, /User message:\nContinue from there\./);
    assert.equal(codex.sent("thread/start").length, 1);
  }));

test("returning to an earlier agent tells it only what happened since its last turn", { skip, timeout: 60000 }, () =>
  withCodex((codex) => {
    const native = "0199a000-0000-7000-8000-000000000003";
    codex.seed("ses_back");
    const folder = path.join(codex.home, "agents/sessions/ses_back");
    write(path.join(folder, "agent.json"), JSON.stringify({
      threads: { [provider]: { state: { codexThread: native }, seen: "msg_seen" } },
    }));
    const reply = (id, maker, model, text, created) => ({
      type: "assistant", id, time: { created, completed: created }, agent: "build",
      model: { providerID: maker, id: model }, content: [{ type: "text", id: id + "_part", text }],
    });
    write(path.join(folder, "messages.json"), JSON.stringify([
      { type: "user", id: "msg_first", time: { created: 1 }, text: "First question, already seen." },
      reply("msg_first_answer", provider, "gpt-test", "First answer, already seen.", 2),
      { type: "idle", id: "msg_seen", outcome: "succeeded", time: { created: 3 } },
      { type: "user", id: "msg_second", time: { created: 4 }, text: "New work for the other agent." },
      reply("msg_second_answer", "claude-code", "haiku", "Unseen answer.", 5),
      { type: "idle", id: "msg_second_done", outcome: "succeeded", time: { created: 6 } },
    ]));
  }, async (codex) => {
    await codex.prompt("ses_back", "Check the new work.");
    await codex.idles("ses_back", 3);
    const prompt = codex.sent("turn/start")[0].params.input[0].text;
    assert.doesNotMatch(prompt, /already seen/);
    assert.match(prompt, /New work for the other agent\./);
    assert.match(prompt, /Unseen answer\./);
    assert.match(prompt, /User message:\nCheck the new work\./);
    assert.equal(codex.sent("thread/start").length, 0);
    assert.equal(codex.sent("thread/resume").length, 1);
  }));

// The shared history cursor is used by the other resident adapters too. The stand-ins
// answer with no tools; we check the words actually received, not their canned replies.
for (const [name, make, seed, prompts] of [
  ["Claude Code", scriptedClaude,
    (agent, id) => agent.seed(id),
    (agent) => agent.heard().filter((line) => line.type === "user").map((line) =>
      line.message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n"))],
  ...[["Grok", "grok", providers.grok], ["Antigravity", "agy", providers.agy]].map(([name, cli, provider]) => [
    name, () => scriptedAcp({ names: [cli], grok: cli === "grok", agy: cli === "agy" }),
    (agent, id) => agent.seed(id, provider),
    (agent) => agent.sent("session/prompt").map((line) =>
      line.params.prompt.filter((part) => part.type === "text").map((part) => part.text).join("\n")),
  ]),
]) {
  test(`${name} receives the previous agent's history once`, { skip, timeout: 60000 }, async () => {
    const agent = await make();
    const id = "ses_carried";
    try {
      seed(agent, id);
      write(path.join(agent.home, "agents/sessions", id, "messages.json"), JSON.stringify([
        { type: "user", id: "msg_old", time: { created: 1 }, text: "Earlier project question." },
        { type: "assistant", id: "msg_reply", time: { created: 2, completed: 3 }, agent: "build", model: { providerID: provider, id: "gpt-test" }, content: [
          { type: "text", id: "part_old", text: "Earlier project answer." },
        ] },
        { type: "idle", id: "msg_idle", outcome: "succeeded", time: { created: 4 } },
      ]));
      await agent.start();
      await agent.prompt(id, "What is next?");
      await agent.idles(id, 2);
      const first = prompts(agent).find((text) => text.includes("What is next?"));
      assert.match(first, /Earlier project question\./);
      assert.match(first, /Earlier project answer\./);
      assert.match(first, /User message:\nWhat is next\?/);
      await agent.stop();
      await agent.start();
      await agent.prompt(id, "And then?");
      await agent.idles(id, 3);
      const next = prompts(agent).find((text) => text.includes("And then?"));
      assert.ok(next, JSON.stringify(prompts(agent)));
      assert.doesNotMatch(next, /Earlier project question|User message:/);
    } catch (error) {
      throw new Error([error.message, agent.log()].join("\n"), { cause: error });
    } finally {
      await agent.close();
    }
  });
}
