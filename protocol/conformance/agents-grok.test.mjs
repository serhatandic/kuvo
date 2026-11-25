// Grok through each resident, on a scripted `grok agent stdio` (acp.test.ts): its account and
// models, its turns, what it asks, Stop, its own turns and what it keeps going.
import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import path from "node:path";
import { boxBinary, request, until, write } from "./adapter.mjs";
import { askFirst, assertMatch, headers, idles, texts, toolCalls } from "./claude.mjs";
import { providers, scriptedAcp } from "./acp.mjs";

const provider = providers.grok;
const typescript = boxBinary().typescript;
const skip = typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;
const words = (text) => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });

describe("Grok", { skip }, () => {
  let acp;
  const pool = { plain: [], ask: [] };
  before(async () => {
    if (skip) return;
    acp = await scriptedAcp({ names: ["grok"], agy: false });
    // Conversations on Grok: Approve All (plain), or asking first.
    for (const [kind, permissions] of [["plain"], ["ask", askFirst]]) {
      for (let index = 1; index <= 8; index++) {
        const id = `ses_grok_${kind}_${index}`;
        const directory = path.join(acp.home, "projects", id);
        write(path.join(directory, ".keep"), "");
        acp.seed(id, provider, { directory, permissions });
        pool[kind].push(id);
      }
    }
    await acp.start();
  });
  after(async () => acp?.close());

  /** A conversation on Grok whose first message plays `steps`. */
  async function begin(steps, { ask = false } = {}) {
    const id = pool[ask ? "ask" : "plain"].shift();
    assert.ok(id, "Seed more sessions.");
    if (steps) acp.script(`go ${id}`, steps);
    await acp.prompt(id, `go ${id}`);
    return id;
  }

  test("opening the app reads Grok's account and models without starting Grok", { timeout: 60000, skip: typescript && "The reference asks `grok models`." }, async () => {
    const accounts = await request(acp.listener, "GET", "kuvo/agents/api/integration", undefined, headers);
    assert.equal(accounts.status, 200, accounts.text);
    const row = accounts.json.data.find((each) => each.id === provider);
    assert.equal(row?.connected ?? row?.metadata?.signedIn ?? true, true, JSON.stringify(row));
    const offered = await request(acp.listener, "GET", "kuvo/models/offered?refresh=1", undefined, headers, 45000);
    assert.equal(offered.status, 200, offered.text);
    assert.ok(offered.json.models.some((model) => model.providerID === provider), JSON.stringify(offered.json.models.map((model) => model.providerID)));
    assert.equal(acp.heard().filter((line) => line.argv).length, 0, "Listing started Grok");
  });

  test("a turn: its thinking, its words, a command with its output, its to-do list and how full its context is", { timeout: 60000 }, async () => {
    const id = await begin([
      { update: { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "Thinking it over." } } },
      words("Running the tests."),
      { update: { sessionUpdate: "tool_call", toolCallId: "t1", kind: "execute", title: "Run `npm test`", status: "in_progress", rawInput: { command: "npm test" } } },
      { update: { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed", rawOutput: { stdout: "all passed", exit_code: 0 } } },
      { update: { sessionUpdate: "plan", entries: [{ content: "Test", status: "completed" }, { content: "Ship", status: "in_progress" }] } },
      { update: { sessionUpdate: "usage_update", size: 200000, used: 1200 } },
      words("All good."),
    ]);
    const rows = await acp.idles(id);
    assert.ok(texts(rows).join("").includes("Running the tests."), JSON.stringify(texts(rows)));
    assert.ok(texts(rows).join("").includes("All good."));
    const tools = toolCalls(rows);
    assertMatch(tools.find((tool) => tool.tool === "shell" || tool.name === "shell"), { state: { status: "completed", input: { command: "npm test" } } });
    assert.ok(tools.some((tool) => (tool.tool ?? tool.name) === "todowrite"), JSON.stringify(tools.map((tool) => tool.tool ?? tool.name)));
    assert.equal(idles(rows).at(-1).outcome ?? "succeeded", "succeeded");
  });

  test("two conversations on the account share one Grok", { timeout: 60000, skip: typescript && "The reference starts one Grok for each conversation." }, async () => {
    const before = acp.started("grok").length;
    const first = await begin();
    await acp.idles(first);
    const second = await begin();
    await acp.idles(second);
    const started = acp.started("grok").length - before;
    assert.ok(started <= 1, `Grok was started ${started} times for two conversations.`);
    const opened = acp.sent("session/new").map((message) => message.params.cwd);
    assert.ok(opened.length >= 2);
  });

  test("a command it asks to run waits for you: Allow Once reaches it as the allow-once it offered", { timeout: 60000 }, async () => {
    const id = await begin([
      { update: { sessionUpdate: "tool_call", toolCallId: "t9", kind: "execute", title: "Run `rm -rf build`", status: "pending", rawInput: { command: "rm -rf build" } } },
      { ask: ["session/request_permission", { toolCall: { toolCallId: "t9" }, options: [{ optionId: "yes", kind: "allow_once", name: "Allow" }, { optionId: "always", kind: "allow_always", name: "Always" }, { optionId: "no", kind: "reject_once", name: "No" }] }] },
      words("Removed."),
    ], { ask: true });
    const card = await until(async () => (await acp.permissions(id))[0], "No card for the command.");
    assertMatch(card, { action: "shell", resources: ["rm -rf build"] });
    await acp.replyPermission(id, card.id, "once");
    const answer = await acp.answered("session/request_permission");
    assert.deepEqual(answer.result, { outcome: { outcome: "selected", optionId: "yes" } });
    await acp.idles(id);
  });

  test("its question is a card; your answer is what it hears", { timeout: 60000 }, async () => {
    const id = await begin([
      { ask: ["_x.ai/ask_user_question", { toolCallId: "q1", questions: [{ id: "color", question: "Which color?", options: [{ label: "Red" }, { label: "Blue" }] }] }] },
      words("Blue it is."),
    ]);
    const form = await until(async () => (await acp.forms(id))[0], "No card for the question.");
    const answered = await acp.replyForm(id, form.id, { color: "Blue" });
    assert.ok(answered.status < 300, answered.text);
    const answer = await acp.answered("_x.ai/ask_user_question");
    assert.deepEqual(answer.result, { outcome: "accepted", answers: { "Which color?": ["Blue"] } });
    await acp.idles(id);
  });

  test("Stop: Grok is told to cancel, the turn ends stopped, and the next message starts Grok again", { timeout: 60000 }, async () => {
    const id = await begin([words("Working."), "hang"]);
    await until(async () => texts(await acp.rows(id)).join("").includes("Working."), "Grok never started its turn.");
    const before = acp.started("grok").length;
    await acp.interrupt(id);
    const rows = await acp.idles(id);
    assert.equal(idles(rows).at(-1).outcome, "interrupted");
    assert.ok(acp.sent("session/cancel").some((message) => message.params.sessionId), "Grok wasn't told to cancel.");
    await acp.prompt(id, "carry on");
    await acp.idles(id, 2);
    assert.ok(acp.started("grok").length > before, "The next message went to the Grok that was stopped.");
  });

  test("a turn it begins on its own after its answer (its task ended) is taken as a turn here", { timeout: 60000 }, async () => {
    const go = `own-${pool.plain[0]}`;
    const id = await begin([
      words("Started a task."),
      { own: { until: go, steps: [words("The task finished: all done."), { ext: ["_x.ai/session/prompt_complete", { promptId: "task-completed-1", stopReason: "end_turn" }] }] } },
    ]);
    await acp.idles(id);
    acp.go(go);
    const rows = await acp.idles(id, 2, "Grok's own turn was never taken.");
    assert.ok(texts(rows).join("").includes("The task finished: all done."), JSON.stringify(texts(rows)));
  });

  test("what it keeps going in the background shows as at work", { timeout: 60000 }, async () => {
    const id = await begin([
      words("Started the server."),
      { ext: ["_x.ai/session/update", { update: { sessionUpdate: "background_tasks", tasks: [{ task_id: "bg1", status: "running", command: "npm run dev", started_at: new Date().toISOString() }] } }] },
    ]);
    await acp.idles(id);
    const work = await until(async () => (await acp.work()).sessions?.[id], "Its background task never showed.");
    assertMatch(work, { tasks: 1 });
  });
});
