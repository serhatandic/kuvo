// Codex through each resident, on a scripted `codex app-server` (codex.test.ts): its account,
// signing in, its models, and its turns.
import assert from "node:assert/strict";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import fs from "node:fs";
import {
  boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, request, scratch, until, write,
} from "./adapter.mjs";
import {
  assertMatch, askFirst, idles, keptListener, steps, stopFixtureHolders, stopPids, texts, toolCalls,
  transcript, twoReleases,
} from "./claude.mjs";
import { provider, scriptedCodex, seedCodexSession, standInCodex } from "./codex.mjs";

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
      for (let index = 1; index <= 16; index++) {
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
    assert.deepEqual((await box.codex.answeredFor("c1")).result, { decision: "accept" });
    const next = await until(async () => (await box.codex.permissions(id)).find((each) => each.resources?.[0] === "rm -rf build"), "No card for the second command.");
    await box.codex.replyPermission(id, next.id, "reject");
    assert.deepEqual((await box.codex.answeredFor("c2")).result, { decision: "decline" });
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

describe("Codex, more of what it does", { skip }, () => {
  const box = resident();
  const begun = (id) => until(() => box.codex.sent("turn/start").some((message) => message.params.input[0].text.includes(`go ${id}`)), "Codex never started.");

  test("summarizing when you ask: Codex is asked with its own request, no turn of words, and the row shows it with no reply", { timeout: 60000 }, async () => {
    const { id } = await box.begin([item("item/completed", { type: "agentMessage", id: "m1", text: "Hello." })]);
    await box.codex.idles(id);
    const answer = await box.codex.compact(id);
    assert.equal(answer.status, 200, answer.text);
    const rows = await box.codex.idles(id, 2);
    assert.equal(box.codex.sent("thread/compact/start").filter((message) => message.params.threadId === box.codex.agentState(id).codexThread).length, 1);
    const summaries = rows.filter((row) => row.type === "compaction");
    assertMatch(summaries, [{ reason: "manual", status: "completed" }]);
    assert.deepEqual(texts(rows), ["Hello."]);
  });

  test("its usage limit ends the turn as one, for the plan, with what Codex said", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      ["error", { error: { message: "You've hit your usage limit.", codexErrorInfo: "usageLimitExceeded" }, willRetry: false }],
      { end: { status: "failed", error: { message: "You've hit your usage limit.", codexErrorInfo: "usageLimitExceeded" } } },
    ]);
    const rows = await box.codex.idles(id);
    assertMatch(idles(rows)[0], { outcome: "failed" });
    const reason = idles(rows)[0].error ?? idles(rows)[0].reason;
    assertMatch(reason, { type: "limit", message: "You've hit your usage limit.", limit: { scope: "provider" } });
  });

  test("a file it cites as what it made shows as a file in the conversation, one it cites in its words as its path, and no directive shows raw", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      ["item/agentMessage/delta", { itemId: "m1", delta: "Made :codex-file-citation{path=\"/tmp/chart.png\" pur" }],
      ["item/agentMessage/delta", { itemId: "m1", delta: "pose=\"output\"} from :codex-file-citation{path=\"data.csv\"}." }],
      item("item/completed", { type: "agentMessage", id: "m1", text: "" }),
    ]);
    const rows = await box.codex.idles(id);
    assert.equal(texts(rows).join(""), "Made  from `data.csv`.");
    assertMatch(toolCalls(rows), [{ name: "kuvo-browser_show_file", state: { status: "completed", input: { path: "/tmp/chart.png" } } }]);
  });

  test("nothing dropped: an image it looked at, a function's output, and an item kind the box doesn't know", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      item("item/completed", { type: "imageView", id: "v1", path: "/tmp/shot.png" }),
      item("item/completed", { type: "functionCallOutput", id: "f1", namespace: "browser", name: "open", output: "Opened." }),
      item("item/started", { type: "futureThing", id: "x1", detail: "new" }),
      item("item/completed", { type: "futureThing", id: "x1", detail: "new", result: "It worked." }),
    ]);
    const rows = await box.codex.idles(id);
    const tools = toolCalls(rows);
    assert.deepEqual(tools.map((tool) => tool.name), ["read", "browser.open", "futureThing"]);
    assertMatch(tools[0].state, { status: "completed", input: { path: "/tmp/shot.png" } });
    assert.ok(tools[0].state.content.some((part) => part.type === "file" && part.mime === "image/png"), JSON.stringify(tools[0].state.content));
    assertMatch(tools[1].state, { status: "completed", content: [{ type: "text", text: "Opened." }] });
    assertMatch(tools[2].state, { status: "completed", input: { detail: "new" }, content: [{ type: "text", text: "It worked." }] });
  });

  test("what Codex says for you to read, and a hook of yours, as notes and a step", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      ["warning", { message: "Your config has an unknown key." }],
      ["model/rerouted", { fromModel: "gpt-test", toModel: "gpt-safe", reason: "highRiskCyberActivity" }],
      ["hook/started", { run: { id: "h1", eventName: "PreToolUse", sourcePath: "/hooks/check.sh" } }],
      ["hook/completed", { run: { id: "h1", eventName: "PreToolUse", status: "completed", entries: [{ kind: "context", text: "checked" }] } }],
      item("item/completed", { type: "agentMessage", id: "m1", text: "Done." }),
    ]);
    const rows = await box.codex.idles(id);
    const notes = rows.filter((row) => row.type === "synthetic").map((row) => row.text);
    assert.deepEqual(notes, ["Your config has an unknown key.", "gpt-safe answered instead of gpt-test (Codex moves security-sensitive work to it)."]);
    assertMatch(toolCalls(rows), [{ name: "hook", state: { status: "completed", input: { description: "PreToolUse hook" }, content: [{ type: "text", text: "checked" }] } }]);
  });

  test("Allow Always lets the same command through for the rest of the session without asking; another command still asks", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      { ask: ["item/commandExecution/requestApproval", { itemId: "c1", command: "npm test" }] },
      { ask: ["item/commandExecution/requestApproval", { itemId: "c2", command: "npm test" }] },
      { ask: ["item/commandExecution/requestApproval", { itemId: "c3", command: "rm -rf dist" }] },
    ], { ask: true });
    const card = await until(async () => (await box.codex.permissions(id))[0], "No card.");
    await box.codex.replyPermission(id, card.id, "always");
    assert.deepEqual((await box.codex.answeredFor("c1")).result, { decision: "accept" });
    // The same command again: answered for you, with no card.
    assert.deepEqual((await box.codex.answeredFor("c2")).result, { decision: "accept" });
    const other = await until(async () => (await box.codex.permissions(id)).find((each) => each.resources?.[0] === "rm -rf dist"), "The other command didn't ask.");
    await box.codex.replyPermission(id, other.id, "reject");
    await box.codex.idles(id);
  });

  test("a question it asks (request_user_input_async) is a card with its choices, and the choice goes into its turn", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      item("item/completed", { type: "agentMessage", id: "m1", text: "One thing first.", questions: [{ title: "Which database?", options: ["Postgres", "SQLite"] }] }),
      { until: "question-answered" },
      item("item/completed", { type: "agentMessage", id: "m2", text: "SQLite it is." }),
    ]);
    const form = await until(async () => (await box.codex.forms(id))[0], "No card for its question.");
    assertMatch(form.fields[0], { description: "Which database?", options: [{ value: "Postgres" }, { value: "SQLite" }] });
    assert.equal((await box.codex.replyForm(id, form.id, { q0: "SQLite" })).status, 200);
    const steered = await until(() => box.codex.sent("turn/steer").find((message) => message.params.input[0].text === "SQLite"), "Its answer never went into its turn.");
    assert.ok(steered.params.expectedTurnId);
    box.codex.go("question-answered");
    const rows = await box.codex.idles(id);
    assert.equal(idles(rows).length, 1);
    assert.ok(texts(rows).join("\n").includes("- Which database? (Postgres / SQLite)"), JSON.stringify(texts(rows)));
  });

  test("a question still open as its turn ends: the session waits, and your answer starts its next turn", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      item("item/completed", { type: "agentMessage", id: "m1", text: "Waiting on you.", questions: [{ title: "Go ahead?", options: ["Yes", "No"] }] }),
    ]);
    const form = await until(async () => (await box.codex.forms(id))[0], "No card for its question.");
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(idles(await box.codex.rows(id)).length, 0);
    box.codex.script("Yes", [item("item/completed", { type: "agentMessage", id: "m2", text: "Going ahead." })]);
    assert.equal((await box.codex.replyForm(id, form.id, { q0: "Yes" })).status, 200);
    const rows = await box.codex.idles(id);
    assert.equal(idles(rows).length, 1);
    assert.ok(texts(rows).includes("Going ahead."), JSON.stringify(texts(rows)));
    const thread = box.codex.agentState(id).codexThread;
    assert.deepEqual(box.codex.sent("turn/start").filter((message) => message.params.threadId === thread).map((message) => message.params.input[0].text.split("\n")[0]), [`go ${id}`, "Yes"]);
  });

  test("its form question (request_user_input) is a form whose answers reach it", { timeout: 60000 }, async () => {
    const { id } = await box.begin([
      { ask: ["item/tool/requestUserInput", { itemId: "q1", questions: [{ id: "db", header: "Database", question: "Which one?", options: [{ label: "Postgres", description: "A server" }, { label: "SQLite" }] }] }] },
    ]);
    const form = await until(async () => (await box.codex.forms(id))[0], "No form.");
    assertMatch(form, { title: "Database" });
    assertMatch(form.fields[0], { key: "db", title: "Database", description: "Which one?", options: [{ value: "Postgres", description: "A server" }, { value: "SQLite" }] });
    assert.equal((await box.codex.replyForm(id, form.id, { db: "SQLite" })).status, 200);
    assert.deepEqual((await box.codex.answeredFor("q1")).result, { answers: { db: { answers: ["SQLite"] } } });
    await box.codex.idles(id);
  });

  test("the speed picked for its account goes with each turn: Fast where its model runs at it", { timeout: 60000 }, async () => {
    write(path.join(box.codex.home, "box", "models.json"), JSON.stringify({ speeds: { [provider]: "priority" } }));
    try {
      const { id } = await box.begin([item("item/completed", { type: "agentMessage", id: "m1", text: "Fast." })]);
      await box.codex.idles(id);
      const turn = box.codex.sent("turn/start").find((message) => message.params.input[0].text.includes(`go ${id}`));
      assert.equal(turn.params.serviceTier, "priority");
    } finally {
      fs.rmSync(path.join(box.codex.home, "box", "models.json"), { force: true });
    }
  });
});

/** The holder of the account's Codex app-server. */
function heldCodex(home) {
  const folder = path.join(home, "run", "agents", "hold");
  if (!fs.existsSync(folder)) return undefined;
  return fs.readdirSync(folder).filter((name) => /^[a-f\d]{12}\.json$/.test(name)).flatMap((name) => {
    try { return [JSON.parse(fs.readFileSync(path.join(folder, name), "utf8"))]; } catch { return []; }
  }).find((record) => record.meta?.provider === provider && record.meta?.kind === "codex");
}

test("the Rust keeper keeps a Codex turn across a release switch: the same app-server finishes it, and what it asked is asked once more and answered once",
  { timeout: 90000 }, async (t) => {
    const binary = boxBinary();
    if (binary.typescript) {
      t.skip("A Rust-to-Rust release switch uses the Rust holder and resident.");
      return;
    }
    const root = scratch();
    const home = path.join(root, "box");
    const relay = await localRelay();
    const session = "ses_codex_kept";
    const route = `kuvo/agents/api/session/${session}`;
    let keeper;
    const residents = [];
    try {
      privateHome(home);
      const base = isolatedEnv(root);
      const bin = standInCodex(base.HOME);
      const system = process.platform === "win32" ? path.join(base.SystemRoot, "System32") : "/usr/bin:/bin";
      const env = { ...base, KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", KUVO_CODEX_ROOT: base.HOME, PATH: [bin, system].join(path.delimiter) };
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      seedCodexSession(home, session, { permissions: askFirst });
      write(path.join(base.HOME, "codex-script.json"), JSON.stringify({ "wait for update": [
        ["item/completed", { item: { type: "agentMessage", id: "m1", text: "Before the switch." } }],
        { ask: ["item/commandExecution/requestApproval", { itemId: "c1", command: "make" }] },
        ["item/completed", { item: { type: "agentMessage", id: "m2", text: "After the switch." } }],
      ] }));
      const { old, next, control } = twoReleases(binary, home, env, relay);
      try {
        await control(["select", old]);
        keeper = boxCommand(binary, home, env, relay, ["keep"]);
        const first = await keptListener(home, keeper);
        residents.push(first.pid);
        const started = await request(first, "POST", route + "/prompt", { id: "msg_codex_kept", text: "wait for update" }, headers);
        assert.equal(started.status, 200, started.text);
        const asked = await until(async () => {
          const answer = await request(first, "GET", route + "/permission", undefined, headers);
          return answer.json?.data?.[0];
        }, "Codex never asked.");
        const initial = heldCodex(home);
        assert.ok(initial, "Codex's holder was never recorded.");
        assert.equal(Object.values(initial.meta.turns)[0]?.turn, "msg_codex_kept");
        await control(["select", next]);
        const second = await keptListener(home, keeper, first.pid);
        residents.push(second.pid);
        const again = await until(async () => {
          const answer = await request(second, "GET", route + "/permission", undefined, headers);
          return answer.json?.data?.[0];
        }, () => "The ask didn't come again. " + keeper.log().slice(-4000));
        assert.equal(again.id, asked.id);
        const replied = await request(second, "POST", `${route}/permission/${again.id}/reply`, { decision: "once" }, headers);
        assert.ok(replied.status < 300, replied.text);
        let last;
        const rows = await until(async () => {
          last = await transcript(second, route);
          return last.at(-1)?.outcome === "succeeded" && last;
        }, () => ["The replacement resident never finished the held turn.", JSON.stringify(last?.slice(-3)), keeper.log().slice(-6000)].join("\n"), 30000);
        assert.deepEqual(texts(rows), ["Before the switch.", "After the switch."]);
        const heard = fs.readFileSync(path.join(base.HOME, "codex-heard.jsonl"), "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
        // One app-server all along, initialized once, and the ask answered once.
        assert.equal(heard.filter((line) => line.argv).length, 1);
        assert.equal(heard.filter((line) => line.message?.method === "initialize").length, 1);
        assert.equal(heard.filter((line) => line.message?.id === "ask-1" && line.message.method === undefined).length, 1);
        const after = heldCodex(home);
        assert.deepEqual([after.pid, after.child], [initial.pid, initial.child]);
        assert.ok(after.generation > initial.generation);
      } finally {
        await control(["off"]);
      }
    } finally {
      await keeper?.stop();
      await stopPids(residents);
      await stopFixtureHolders(home);
      await relay.close();
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    }
  });
