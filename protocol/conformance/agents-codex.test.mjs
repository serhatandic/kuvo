// Codex through each resident, on a scripted `codex app-server` (codex.test.ts): its account,
// signing in, its models, and its turns.
import assert from "node:assert/strict";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { boxBinary, request, until, write } from "./adapter.mjs";
import { assertMatch, askFirst, idles, steps, texts, toolCalls } from "./claude.mjs";
import { provider, scriptedCodex } from "./codex.mjs";

const headers = { "x-kuvo-from": "app" };
const picker = "kuvo/models/offered?refresh=1";
const oauth = `kuvo/agents/api/integration/${provider}/connect/oauth`;
const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

const offered = async (listener) => {
  const answer = await request(listener, "GET", picker, undefined, headers, 45000);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.models.filter((model) => model.providerID === provider);
};

/** A resident of its own for one case. */
async function withCodex(fixture, check) {
  const codex = await scriptedCodex(fixture);
  try {
    await codex.start();
    await check(codex);
  } catch (error) {
    throw new Error([error.message, codex.log()].join("\n"), { cause: error });
  } finally {
    await codex.close();
  }
}

describe("Codex's account and models", { skip }, () => {
  test("its models as it lists them: its default first, with its speeds, and one it hides from its picker", { timeout: 60000 }, async () => {
    await withCodex({}, async (codex) => {
      const models = await offered(codex.listener);
      // (One it hides from its picker is listed too, as Codex still runs it.)
      assert.deepEqual(models.map((model) => model.id), ["gpt-test", "gpt-mini"]);
      assertMatch(models[0], { name: "GPT Test" });
      assert.deepEqual(models[0].speeds?.map((speed) => speed.id), ["priority"]);
      // Asked with the ones it hides, which it still runs.
      assert.ok(codex.sent("model/list").every((message) => message.params.includeHidden === true));
      const [started] = codex.heard().filter((line) => line.argv);
      assert.deepEqual(started.argv.slice(0, 1), ["app-server"]);
      assert.ok(started.argv.includes("plugins.computer-use@openai-bundled.enabled=false"), JSON.stringify(started.argv));
      assert.equal(started.env.originator, "kuvo-code");
    });
  });

  test("a model list in pages is read whole, in Codex's order with its default first", { timeout: 60000 }, async () => {
    await withCodex({}, async (codex) => {
      write(path.join(codex.root, "codex-models.json"), JSON.stringify([
        [{ id: "gpt-a", model: "gpt-a", displayName: "GPT A" }],
        [{ id: "gpt-b", model: "gpt-b", displayName: "GPT B", isDefault: true }, { id: "gpt-c", model: "gpt-c", displayName: "GPT C" }],
      ]));
      const models = await offered(codex.listener);
      assert.deepEqual(models.map((model) => model.id).sort(), ["gpt-a", "gpt-b", "gpt-c"]);
      assert.equal(codex.sent("model/list").filter((message) => message.params.cursor === "1").length >= 1, true);
    });
  });

  test("a signed-out Codex offers no models", { timeout: 60000 }, async () => {
    await withCodex({ signedIn: false }, async (codex) => {
      assert.deepEqual(await offered(codex.listener), []);
    });
  });

  test("ChatGPT signs it in on its page, and its models are offered then", { timeout: 60000 }, async () => {
    await withCodex({ signedIn: false }, async (codex) => {
      const started = await request(codex.listener, "POST", oauth, { methodID: "chatgpt" }, headers, 20000);
      assert.equal(started.status, 200, started.text);
      const { attemptID, url, mode, instructions } = started.json.data;
      assert.equal(url, "https://auth.example.test/codex");
      assert.equal(mode, "auto");
      assert.equal(instructions, "Finish signing in with ChatGPT in your browser.");
      assert.deepEqual(codex.sent("account/login/start").map((message) => message.params.type), ["chatgpt"]);
      codex.go("codex-login-complete");
      await until(async () => {
        const checked = await request(codex.listener, "GET", `${oauth}/${attemptID}`, undefined, headers);
        return checked.json?.data?.status === "complete";
      }, "The Codex sign-in never finished.");
      assert.ok((await offered(codex.listener)).some((model) => model.id === "gpt-test"));
    });
  });

  test("an OpenAI API key signs it in", { timeout: 60000 }, async () => {
    await withCodex({ signedIn: false }, async (codex) => {
      const answer = await request(codex.listener, "POST", `kuvo/agents/api/integration/${provider}/connect/key`, { key: "sk-fixture" }, headers, 20000);
      assert.ok(answer.status < 300, answer.text);
      assertMatch(codex.sent("account/login/start")[0].params, { type: "apiKey", apiKey: "sk-fixture" });
      assert.ok((await offered(codex.listener)).some((model) => model.id === "gpt-test"));
    });
  });
});

/** One resident for a describe's cases, with sessions seeded before it starts. */
function resident(fixture = {}) {
  const pool = { plain: [], ask: [], kept: [] };
  const box = {};
  before(async () => {
    if (skip) return;
    box.codex = await scriptedCodex(fixture);
    for (const [kind, permissions] of [["plain"], ["ask", askFirst]]) {
      for (let index = 1; index <= 8; index++) {
        const id = `ses_codex_${kind}_${String(index).padStart(3, "0")}`;
        const folder = path.join(box.codex.home, "projects", id);
        write(path.join(folder, ".keep"), "");
        box.codex.seed(id, { directory: folder, permissions });
        pool[kind].push({ id, folder });
      }
    }
    // One whose Codex thread is kept in its slot, beside another agent's.
    const id = "ses_codex_kept_001";
    const folder = path.join(box.codex.home, "projects", id);
    write(path.join(folder, ".keep"), "");
    box.codex.seed(id, { directory: folder });
    write(path.join(box.codex.home, "agents/sessions", id, "agent.json"), JSON.stringify({
      threads: {
        [provider]: { state: { codexThread: "0199a000-0000-7000-8000-000000000042" } },
        "claude-code": { state: { claudeSession: "6f1c2a4e-8b3d-4c5a-9e7f-0a1b2c3d4e5f" } },
      },
    }));
    pool.kept.push({ id, folder });
    await box.codex.start();
  });
  after(async () => box.codex?.close());
  box.next = ({ ask = false, kept = false } = {}) => {
    const session = pool[kept ? "kept" : ask ? "ask" : "plain"].shift();
    assert.ok(session, "Seed more sessions.");
    return session;
  };
  box.begin = async (steps, options, session = box.next(options)) => {
    const text = `go ${session.id}`;
    box.codex.script(text, steps);
    await box.codex.prompt(session.id, text);
    return session;
  };
  return box;
}

const item = (method, value) => [method, { item: value }];

describe("Codex's turns", { skip }, () => {
  const box = resident();

  test("its thinking and words show once as they stream, a command with what it printed and how it ended, a change with each file's lines, and what it says after them as a step of its own", { timeout: 60000 }, async () => {
    const { id, folder } = await box.begin([
      item("item/started", { type: "reasoning", id: "r1" }),
      ["item/reasoning/summaryTextDelta", { itemId: "r1", delta: "Checking the tests." }],
      item("item/completed", { type: "reasoning", id: "r1", summary: ["Checking the tests."] }),
      item("item/started", { type: "commandExecution", id: "c1", command: "/bin/zsh -lc \"npm test\"", cwd: "/w" }),
      ["item/commandExecution/outputDelta", { itemId: "c1", delta: "3 passed\n" }],
      item("item/completed", { type: "commandExecution", id: "c1", command: "/bin/zsh -lc \"npm test\"", status: "completed", exitCode: 0, aggregatedOutput: "3 passed\n" }),
      item("item/started", { type: "fileChange", id: "f1", changes: [{ path: "a.ts", kind: { type: "update" }, diff: "@@ -1,1 +1,1 @@\n-const a = 1\n+const a = 2\n" }] }),
      item("item/completed", { type: "fileChange", id: "f1", status: "completed", changes: [{ path: "a.ts", kind: { type: "update" }, diff: "@@ -1,1 +1,1 @@\n-const a = 1\n+const a = 2\n" }] }),
      ["thread/tokenUsage/updated", { tokenUsage: { last: { totalTokens: 5000, inputTokens: 4000, cachedInputTokens: 1000, outputTokens: 1000, reasoningOutputTokens: 200 }, modelContextWindow: 272000 } }],
      item("item/started", { type: "agentMessage", id: "m1", text: "" }),
      ["item/agentMessage/delta", { itemId: "m1", delta: "They pass, " }],
      ["item/agentMessage/delta", { itemId: "m1", delta: "and a is 2." }],
      item("item/completed", { type: "agentMessage", id: "m1", text: "They pass, and a is 2." }),
    ]);
    const rows = await box.codex.idles(id);
    assert.equal(idles(rows)[0].outcome, "succeeded");
    assert.deepEqual(texts(rows), ["They pass, and a is 2."]);
    const reasoning = steps(rows).flatMap((row) => row.content.filter((part) => part.type === "reasoning").map((part) => part.text));
    assert.deepEqual(reasoning, ["Checking the tests."]);
    const [command, change] = toolCalls(rows);
    assertMatch(command, { name: "shell", state: { status: "completed", input: { command: "npm test" }, content: [{ type: "text", text: "3 passed\n" }], metadata: { exit: 0 } } });
    assertMatch(change, { name: "patch", state: { status: "completed", input: { files: ["a.ts"] }, metadata: { files: [{ file: "a.ts", additions: 1, deletions: 1, status: "modified" }] } } });
    // The words after the tools are a step of their own.
    assert.ok(steps(rows).at(-1).content.some((part) => part.type === "text"));
    const [started] = box.codex.sent("thread/start");
    assertMatch(started.params, { cwd: folder, model: "gpt-test" });
    assert.match(started.params.developerInstructions, /request_user_input_async/);
    assert.equal(started.params.config["shell_environment_policy.set.OPENCODE_SESSION_ID"], id);
    const turn = box.codex.sent("turn/start").find((message) => message.params.input[0].text.includes(`go ${id}`));
    assertMatch(turn.params, { model: "gpt-test", summary: "detailed", approvalsReviewer: "user", approvalPolicy: "on-request", sandboxPolicy: { type: "dangerFullAccess" } });
    assert.match(box.codex.agentState(id).codexThread, /^0199a000-/);
  });

  test("its next message goes on in the same thread, resumed", { timeout: 60000 }, async () => {
    const { id } = await box.begin([item("item/completed", { type: "agentMessage", id: "m1", text: "First." })]);
    await box.codex.idles(id);
    const thread = box.codex.agentState(id).codexThread;
    box.codex.script(`again ${id}`, [item("item/completed", { type: "agentMessage", id: "m2", text: "Second." })]);
    await box.codex.prompt(id, `again ${id}`);
    const rows = await box.codex.idles(id, 2);
    assert.deepEqual(texts(rows), ["First.", "Second."]);
    const turns = box.codex.sent("turn/start").filter((message) => message.params.threadId === thread);
    assert.equal(turns.length, 2);
    assert.equal(box.codex.agentState(id).codexThread, thread);
  });

  test("a thread kept in its slot is resumed, and the other agent's slot stays", { timeout: 60000 }, async () => {
    const session = box.next({ kept: true });
    await box.begin([item("item/completed", { type: "agentMessage", id: "m1", text: "Back." })], undefined, session);
    await box.codex.idles(session.id);
    const resumed = box.codex.sent("thread/resume").find((message) => message.params.threadId === "0199a000-0000-7000-8000-000000000042");
    assertMatch(resumed?.params, { excludeTurns: true, cwd: session.folder });
    const saved = await import("node:fs").then((fs) => JSON.parse(fs.readFileSync(path.join(box.codex.home, "agents/sessions", session.id, "agent.json"), "utf8")));
    assert.equal(saved.threads["claude-code"].state.claudeSession, "6f1c2a4e-8b3d-4c5a-9e7f-0a1b2c3d4e5f");
    assert.equal(saved.threads[provider].state.codexThread, "0199a000-0000-7000-8000-000000000042");
  });

  test("a command it asks to run waits for you on its step: Allow Once lets it through; No declines the next", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      item("item/started", { type: "commandExecution", id: "c1", command: "npm test" }),
      { ask: ["item/commandExecution/requestApproval", { itemId: "c1", command: "npm test", reason: "It runs the tests." }] },
      item("item/completed", { type: "commandExecution", id: "c1", command: "npm test", status: "completed", exitCode: 0, aggregatedOutput: "ok" }),
      { ask: ["item/commandExecution/requestApproval", { itemId: "c2", command: "rm -rf build" }] },
    ], { ask: true });
    const card = await until(async () => (await box.codex.permissions(id))[0], "No card for the command.");
    assertMatch(card, { action: "shell", resources: ["npm test"], message: "It runs the tests." });
    await box.codex.replyPermission(id, card.id, "once");
    assert.deepEqual((await box.codex.answered("ask-1")).result, { decision: "accept" });
    const next = await until(async () => (await box.codex.permissions(id)).find((each) => each.resources?.[0] === "rm -rf build"), "No card for the second command.");
    await box.codex.replyPermission(id, next.id, "reject");
    assert.deepEqual((await box.codex.answered("ask-2")).result, { decision: "decline" });
    await box.codex.idles(id);
  });

  test("Stop: Codex is told to stop the turn it's on, and the session ends stopped", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      item("item/started", { type: "commandExecution", id: "c1", command: "sleep 100" }),
      "stop",
    ]);
    await until(() => box.codex.sent("turn/start").some((message) => message.params.input[0].text.includes(`go ${id}`)) && box.codex.heard().some((line) => line.message === undefined), "Codex never started.");
    await until(async () => toolCalls(await box.codex.rows(id)).length === 1, "The command never showed.");
    await box.codex.interrupt(id);
    const rows = await box.codex.idles(id);
    assert.equal(idles(rows)[0].outcome, "interrupted");
    assert.equal(box.codex.sent("turn/interrupt").length >= 1, true);
    assertMatch(toolCalls(rows)[0], { name: "shell", state: { status: "error" } });
  });

  test("a message sent while it works goes into its turn, with its own ID", { timeout: 60000 }, async () => {
    const { id } = await box.begin([{ until: "steered-in" }, item("item/completed", { type: "agentMessage", id: "m1", text: "Heard you." })]);
    await until(async () => box.codex.sent("turn/start").some((message) => message.params.input[0].text.includes(`go ${id}`)), "Codex never started.");
    await new Promise((resolve) => setTimeout(resolve, 300));
    const message = `msg_${id.slice(4)}_steered`;
    const answer = await request(box.codex.listener, "POST", box.codex.route(id) + "/prompt", { id: message, text: "also check b", delivery: "steer" }, headers, 20000);
    assert.equal(answer.status, 200, answer.text);
    const steered = await until(() => box.codex.sent("turn/steer").find((each) => each.params.clientUserMessageId === message), "The message never went into its turn.");
    // (The reference adds its phone note to a message not from the desktop link: 7.3's.)
    assert.ok(steered.params.input[0].text.startsWith("also check b"), steered.params.input[0].text);
    box.codex.go("steered-in");
    const rows = await box.codex.idles(id);
    assert.equal(idles(rows).length, 1);
  });
});
