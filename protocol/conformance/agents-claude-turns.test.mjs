// Claude Code's turns through each resident, on a scripted stand-in (claude.test.ts: "Claude Code's
// context and usage", "Claude Code's reply", "summarizing on its own, as the conversation fills up").
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, test } from "node:test";
import {
  answerSessionAsk, askFirst, assertCommandCard, assertMatch, canUse, sessionAsks, hook, idles, result, scriptedClaude, steps, streams, texts,
  toolCalls, toolResult,
} from "./claude.mjs";
import { boxBinary, until, write } from "./adapter.mjs";

const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

/**
 * One resident for a describe's cases, with sessions seeded before it starts (the reference
 * bridge reads its sessions as it starts): plain ones, and Ask First ones.
 */
function resident(fixture = {}) {
  const pool = { plain: [], ask: [], plan: [], slot: [] };
  const box = {};
  before(async () => {
    if (skip) return;
    box.claude = await scriptedClaude(fixture);
    for (const [kind, permissions, agent] of [["plain", undefined], ["ask", askFirst], ["plan", askFirst, "plan"]]) {
      for (let index = 1; index <= 12; index++) {
        const id = `ses_claude_${kind}_${String(index).padStart(3, "0")}`;
        const folder = path.join(box.claude.home, "projects", id);
        fs.mkdirSync(folder, { recursive: true });
        box.claude.seed(id, { directory: folder, model: "default", permissions, agent });
        pool[kind].push({ id, folder });
      }
    }
    // Ones Claude Code has a conversation for in its slot, but that haven't been handed to it.
    for (let index = 1; index <= 2; index++) {
      const id = `ses_claude_slot_${String(index).padStart(3, "0")}`;
      const folder = path.join(box.claude.home, "projects", id);
      fs.mkdirSync(folder, { recursive: true });
      box.claude.seed(id, { directory: folder, model: "default" });
      write(path.join(box.claude.home, "agents/sessions", id, "agent.json"), JSON.stringify({
        threads: { "claude-code": { state: { claudeSession: `6f1c2a4e-8b3d-4c5a-9e7f-0a1b2c3d4e5${index}`, claudeStarted: true } } },
      }));
      pool.slot.push({ id, folder });
    }
    await box.claude.start();
  });
  after(async () => box.claude?.close());
  const kind = ({ ask = false, plan = false, slot = false }) => slot ? "slot" : plan ? "plan" : ask ? "ask" : "plain";
  box.peek = (options = {}) => pool[kind(options)][0];
  box.next = (options = {}) => {
    const session = pool[kind(options)].shift();
    assert.ok(session, "Seed more sessions.");
    return session;
  };
  /** A session's first message, playing `steps`. */
  box.begin = async (steps, options, session = box.next(options)) => {
    const text = `go ${session.id}`;
    box.claude.script(text, steps);
    await box.claude.prompt(session.id, text);
    return session;
  };
  return box;
}

/** A user message the stand-in read, for a session's prompt that starts `text`. */
const heardUser = (claude, text) => claude.heard().filter(line => line.type === "user"
  && line.message.content.some(part => part.type === "text" && part.text.startsWith(text)));

describe("Claude Code's context and usage", { skip }, () => {
  const box = resident();

  test("how full its context is, as its last request was: what the context ring counts", async () => {
    const session = box.next();
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Read", input: { file_path: path.join(session.folder, "a.ts") } }], { input: 3_000, read: 40_000, write: 1_000, output: 120 }),
      toolResult("toolu_1", "const a = 1"),
      ...streams("$UUID", [{ type: "text", text: "It sets a to 1." }], { input: 200, read: 44_000, write: 300, output: 400 }),
      result(["$UUID"]),
    ], {}, session);
    const rows = await box.claude.idles(id);
    assert.deepEqual(steps(rows).at(-1).tokens, { input: 200, output: 400, reasoning: 0, cache: { read: 44_000, write: 300 } });
  });

  test("on a plan, a turn costs nothing here: the plan pays", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "text", text: "Hi." }]),
      result(["$UUID"], "success", { total_cost_usd: 0.12 }),
    ]);
    await box.claude.idles(id);
    assert.equal((await until(() => box.claude.saved(id).outcome && box.claude.saved(id), "Never saved.")).cost ?? 0, 0);
  });
});

describe("Claude Code's usage with an API key", { skip }, () => {
  const box = resident({ authMethod: "console_api_key" });

  test("a turn's usage: each request's tokens, and with an API key, what Claude Code says it cost since the turn before", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "ls" } }], { input: 3_000, read: 40_000, write: 1_000, output: 120 }),
      toolResult("toolu_1", "a.ts"),
      ...streams("$UUID", [{ type: "text", text: "One file." }], { input: 200, read: 44_000, write: 300, output: 400 }),
      result(["$UUID"], "success", { total_cost_usd: 0.12 }),
    ]);
    const rows = await box.claude.idles(id);
    // What the reply's footer counts.
    assert.equal(steps(rows).reduce((sum, step) => sum + (step.tokens?.output ?? 0), 0), 520);
    const saved = await until(() => box.claude.saved(id).outcome && box.claude.saved(id), "Never saved.");
    assert.deepEqual(saved.tokens, { input: 3_200, output: 520, reasoning: 0, cache: { read: 84_000, write: 1_300 } });
    assert.ok(Math.abs(saved.cost - 0.12) < 1e-9, String(saved.cost));
    box.claude.script("and now?", [
      ...streams("$UUID", [{ type: "text", text: "Still one." }], { input: 100, read: 45_000, write: 0, output: 50 }),
      result(["$UUID"], "success", { total_cost_usd: 0.2 }),
    ]);
    await box.claude.prompt(id, "and now?");
    await box.claude.idles(id, 2);
    const again = await until(() => box.claude.saved(id).cost > 0.15 && box.claude.saved(id), () => JSON.stringify(box.claude.saved(id)));
    assert.ok(Math.abs(again.cost - 0.2) < 1e-9, String(again.cost));
  });
});

describe("Claude Code's reply", { skip }, () => {
  const box = resident();

  test("in steps: its words, a command that failed, the edit it made with what changed, the command passing, then its answer", async () => {
    const session = box.next();
    const file = path.join(session.folder, "a.ts");
    fs.writeFileSync(file, "const a = 1\n");
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "text", text: "Running the tests." }, { type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "npm test" } }]),
      toolResult("toolu_1", "1 failed", { error: true }),
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_2", name: "Edit", input: { file_path: file, old_string: "const a = 1", new_string: "const a = 2" } }]),
      toolResult("toolu_2", "The file has been updated.", { structured: { filePath: file, structuredPatch: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-const a = 1", "+const a = 2"] }] } }),
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_3", name: "Bash", input: { command: "npm test" } }]),
      toolResult("toolu_3", "3 passed", { structured: { stdout: "3 passed", stderr: "", interrupted: false } }),
      ...streams("$UUID", [{ type: "text", text: "Fixed: a is 2 now." }]),
      result(["$UUID"]),
    ], {}, session);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "succeeded");
    const [one, two, three, four] = steps(rows);
    assertMatch(one.content, [
      { type: "text", text: "Running the tests." },
      { type: "tool", name: "shell", state: { status: "error", input: { command: "npm test" }, error: { message: "1 failed" } } },
    ]);
    assertMatch(two.content, [{ type: "tool", name: "edit", state: { status: "completed", input: { path: file }, metadata: { files: [{ file, additions: 1, deletions: 1 }] } } }]);
    assertMatch(three.content, [{ type: "tool", name: "shell", state: { status: "completed", content: [{ type: "text", text: "3 passed" }] } }]);
    assertMatch(four.content, [{ type: "text", text: "Fixed: a is 2 now." }]);
    assert.equal(steps(rows).length, 4);
  });

  test("Stop mid-turn: it ends stopped, the command still running shows stopped, what it said stays, and the same Claude Code takes your next message", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "text", text: "Building it." }, { type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "npm run build" } }]),
      { wait: "interrupt" },
      result(["$UUID"], "error_during_execution"),
    ]);
    await until(async () => toolCalls(await box.claude.rows(id))[0]?.state.status === "running", "The command never ran.");
    await box.claude.interrupt(id);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "interrupted");
    assert.equal(box.claude.heard().filter(line => line.type === "control_request" && line.request?.subtype === "interrupt").length, 1);
    assertMatch(toolCalls(rows)[0].state, { status: "error", error: { type: "aborted" } });
    assert.ok(texts(rows).includes("Building it."), JSON.stringify(rows));
    await box.claude.prompt(id, "try again");
    await box.claude.idles(id, 2);
    // The same Claude Code takes it. (The reference bridge, under its holder, starts it again here,
    // going on with the conversation; unheld, its own test has the same one.)
    if (!boxBinary().typescript) assert.equal(heardUser(box.claude, "try again")[0].pid, heardUser(box.claude, `go ${id}`)[0].pid);
  });

  test("Stop while it asks: the card goes, Claude Code is told no, and the turn ends stopped", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "rm -rf build" } }]),
      canUse("ask-stop", "Bash", { command: "rm -rf build" }, "toolu_1"),
      { wait: "interrupt" },
      result(["$UUID"], "error_during_execution"),
    ], { ask: true });
    await until(async () => (await box.claude.permissions(id)).length === 1, "The card never showed.");
    await box.claude.interrupt(id);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "interrupted");
    const answer = await until(() => box.claude.heard().find(line => line.type === "control_response" && line.response?.request_id === "ask-stop"), "Claude Code was never told.");
    assertMatch(answer.response.response, { behavior: "deny" });
    assert.deepEqual(await box.claude.permissions(id), []);
  });
});

describe("summarizing on its own, as the conversation fills up", { skip }, () => {
  const box = resident();
  // What Claude Code 2.1.319 sends as it summarizes by itself partway through a turn.
  const compacting = { type: "system", subtype: "status", status: "compacting", session_id: "$SESSION" };
  const over = (outcome, more = {}) => ({ type: "system", subtype: "status", status: null, ...(outcome ? { compact_result: outcome } : {}), ...more });
  const boundary = { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto", pre_tokens: 180000, post_tokens: 12000 } };
  const summaries = rows => rows.filter(row => row.type === "compaction");
  const notices = rows => rows.flatMap(row => row.type === "synthetic" ? [row.text] : []);
  const working = [
    ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "ls" } }]),
    toolResult("toolu_1", "a.ts"),
    compacting,
  ];
  /** A turn at work, then summarizing, then what `rest` says once the test says go. */
  async function summarizing(rest) {
    const session = box.next();
    await box.begin([...working, { until: `go-${session.id}` }, ...rest], {}, session);
    await until(async () => summaries(await box.claude.rows(session.id)).length === 1, "It never said it was summarizing.");
    return session.id;
  }

  test("while it runs the conversation says so, last, as it happens; it ends as one finished row, the turn going on after it", async () => {
    const id = await summarizing([
      compacting, over("success"), { type: "system", subtype: "init", session_id: "$SESSION" }, boundary,
      ...streams("$UUID", [{ type: "text", text: "Listed." }]), result(["$UUID"]),
    ]);
    const during = await box.claude.rows(id);
    assertMatch(summaries(during), [{ status: "running", reason: "auto" }]);
    assertMatch(during.at(-1), { type: "compaction", status: "running" });
    box.claude.go(`go-${id}`);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "succeeded");
    assertMatch(summaries(rows), [{ status: "completed", reason: "auto" }]);
    assert.equal(typeof summaries(rows)[0].time.completed, "number");
    assert.deepEqual(notices(rows), []);
    assert.deepEqual(rows.map(row => row.type), ["user", "assistant", "compaction", "assistant", "idle"]);
    assert.deepEqual(texts(rows), ["Listed."]);
  });

  test("one that fails says so on its row, once, and the turn goes on", async () => {
    const id = await summarizing([
      over("failed", { compact_error: "Conversation too long." }),
      { type: "system", subtype: "notification", key: "error-compacting-conversation", text: "Error compacting conversation", priority: "immediate" },
      ...streams("$UUID", [{ type: "text", text: "Carrying on." }]), result(["$UUID"]),
    ]);
    box.claude.go(`go-${id}`);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "succeeded");
    assertMatch(summaries(rows), [{ status: "failed", reason: "auto" }]);
    assert.deepEqual(notices(rows), ["Error compacting conversation"]);
  });

  test("one that ends with nothing summarized doesn't stay under way while the turn goes on", async () => {
    const session = box.next();
    const id = session.id;
    await box.begin([
      ...working, { until: `go-${id}` }, over(), ...streams("$UUID", [{ type: "text", text: "Carrying on." }]),
      { until: `end-${id}` }, result(["$UUID"]),
    ], {}, session);
    await until(async () => summaries(await box.claude.rows(id)).length === 1, "It never said it was summarizing.");
    box.claude.go(`go-${id}`);
    const during = await until(async () => {
      const rows = await box.claude.rows(id);
      return texts(rows).length === 1 && rows;
    }, "It never carried on.");
    assertMatch(summaries(during), [{ status: "failed" }]);
    box.claude.go(`end-${id}`);
    const rows = await box.claude.idles(id);
    assert.equal(summaries(rows).length, 1);
  });

  test("one it lets go of and begins again at once (or a permission mode set meanwhile) is still the one row", async () => {
    const id = await summarizing([
      over(), { type: "system", subtype: "status", status: null, permissionMode: "default" }, compacting, over("success"), boundary,
      ...streams("$UUID", [{ type: "text", text: "Listed." }]), result(["$UUID"]),
    ]);
    box.claude.go(`go-${id}`);
    const rows = await box.claude.idles(id);
    assertMatch(summaries(rows), [{ status: "completed", reason: "auto" }]);
  });

  test("Stop during it: the turn ends stopped and the row isn't left under way", async () => {
    const session = box.next();
    const { id } = await box.begin([...working, { wait: "interrupt" }, result(["$UUID"], "error_during_execution")], {}, session);
    await until(async () => summaries(await box.claude.rows(id)).length === 1, "It never said it was summarizing.");
    await box.claude.interrupt(id);
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "interrupted");
    assertMatch(summaries(rows), [{ status: "failed" }]);
  });

  test("Claude Code quitting during it: the row isn't left under way", async () => {
    const id = await summarizing([{ stderr: "Claude Code exited with code 1\n", exit: 1 }]);
    box.claude.go(`go-${id}`);
    const rows = await box.claude.idles(id);
    assertMatch(summaries(rows), [{ status: "failed" }]);
  });

  test("a message you send meanwhile goes to Claude Code, and the row stays the one until it's done", async () => {
    const session = box.next();
    const { id } = await box.begin([
      ...working, { wait: "user" }, { until: `go-${session.id}` }, over("success"), boundary, result(["$UUID", "$UUID2"]),
    ], {}, session);
    await until(async () => summaries(await box.claude.rows(id)).length === 1, "It never said it was summarizing.");
    await box.claude.prompt(id, "and the tests too");
    await until(() => heardUser(box.claude, "and the tests too").length === 1, "Claude Code never took it.");
    // (Its row is written as it's handed over: a moment after Claude Code has it.)
    const during = await until(async () => {
      const rows = await box.claude.rows(id);
      return rows.length >= 4 && rows;
    }, "Your message never showed.");
    assertMatch(summaries(during), [{ status: "running" }]);
    // After the row, which still says it's summarizing.
    assert.deepEqual(during.map(row => row.type), ["user", "assistant", "compaction", "user"]);
    box.claude.go(`go-${id}`);
    const rows = await box.claude.idles(id);
    assertMatch(summaries(rows), [{ status: "completed" }]);
  });

  test("told only once it's done (no word as it began): one finished row, not a note", async () => {
    const { id } = await box.begin([boundary, result(["$UUID"])]);
    const rows = await box.claude.idles(id);
    assertMatch(summaries(rows), [{ status: "completed", reason: "auto" }]);
    assert.deepEqual(notices(rows), []);
  });
});

describe("Claude Code's context window and its summaries", { skip }, () => {
  const box = resident();
  /** What the Claude Code that took the session's message `text` was started with. */
  const started = (text) => {
    const heard = box.claude.heard();
    const pid = heardUser(box.claude, text).at(-1)?.pid;
    return heard.find(line => line.argv && line.pid === pid);
  };
  const flag = (argv, name) => argv.find(arg => arg.startsWith(name + "="))?.slice(name.length + 1)
    ?? (argv.includes(name) ? argv[argv.indexOf(name) + 1] : undefined);
  const setting = value => fs.writeFileSync(path.join(box.claude.home, "box", "conversation-compaction.json"), JSON.stringify(value));

  test("Claude Code gets the chosen window and summarizes a long conversation at its native threshold", async () => {
    const session = box.next();
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "text", text: "Working." }], { input: 2, read: 399_500 - 502, write: 400, output: 100 }),
      ...streams("$UUID", [{ type: "text", text: "Working." }], { input: 2, read: 401_500 - 502, write: 400, output: 100 }),
      // The pinned CLI's native preflight rule for a 1M model, at the window it was given.
      { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto", pre_tokens: 401_500, post_tokens: 18_000 } },
      result(["$UUID"], "success", { modelUsage: { default: { contextWindow: 1_000_000 } } }),
    ], {}, session);
    const rows = await box.claude.idles(id);
    assert.equal(started(`go ${id}`).env.CLAUDE_CODE_AUTO_COMPACT_WINDOW, "433000");
    assertMatch(rows.filter(row => row.type === "compaction"), [{ reason: "auto", status: "completed" }]);
    assert.equal(heardUser(box.claude, `go ${id}`).length, 1);
  });

  test("changing the box's summary setting applies to the next message in the same Claude conversation", async () => {
    try {
      const { id } = await box.begin([result(["$UUID"])]);
      await box.claude.idles(id);
      const first = started(`go ${id}`);
      assert.equal(first.env.CLAUDE_CODE_AUTO_COMPACT_WINDOW, "433000");
      const native = flag(first.argv, "--session-id");
      assert.ok(native, JSON.stringify(first.argv));
      setting("600000");
      box.claude.script(`continue ${id}`, [result(["$UUID"], "success", { modelUsage: { default: { contextWindow: 1_000_000 } } })]);
      await box.claude.prompt(id, `continue ${id}`);
      await box.claude.idles(id, 2);
      const second = started(`continue ${id}`);
      assert.equal(flag(second.argv, "--resume"), native);
      assert.equal(second.env.CLAUDE_CODE_AUTO_COMPACT_WINDOW, "633000");
      setting("never");
      box.claude.script(`once more ${id}`, [result(["$UUID"])]);
      await box.claude.prompt(id, `once more ${id}`);
      await box.claude.idles(id, 3);
      const third = started(`once more ${id}`);
      assert.equal(flag(third.argv, "--resume"), native);
      assert.equal(third.env.DISABLE_AUTO_COMPACT, "1");
    } finally {
      setting("400000");
    }
  });

  test("Claude's session state uses the last request and includes its cached context", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "text", text: "Working." }], { input: 2, read: 238_000, write: 1100, output: 400 }),
      ...streams("$UUID", [{ type: "text", text: "Done." }], { input: 4, read: 240_000, write: 900, output: 300 }),
      result(["$UUID"], "success", { modelUsage: { default: { contextWindow: 1_000_000 } } }),
    ]);
    await box.claude.idles(id);
    const saved = await until(() => box.claude.saved(id).outcome && box.claude.saved(id), "Never saved.");
    assert.deepEqual(saved.context, { used: 241_204, window: 1_000_000 });
  });
});

describe("Claude Code, nothing dropped", { skip }, () => {
  const box = resident();
  async function turn(frames) {
    const { id } = await box.begin([...frames, result(["$UUID"])]);
    return box.claude.idles(id);
  }
  const notices = rows => rows.flatMap(row => row.type === "synthetic" ? [row.text] : []);

  test("a picture a tool brings back, and a server tool's results", async () => {
    const id = box.peek().id;
    const rows = await turn([
      { type: "assistant", message: { id: "a1", content: [{ type: "tool_use", id: "r1", name: "Read", input: { file_path: "/tmp/shot.png" } }] }, parent_tool_use_id: null, user_message_uuids: ["$UUID"] },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "r1", content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: "CCCC" } }] }] }, parent_tool_use_id: null },
      { type: "assistant", message: { id: "a2", content: [{ type: "server_tool_use", id: "s1", name: "web_search", input: { query: "kuvo" } }, { type: "web_search_tool_result", tool_use_id: "s1", content: [{ type: "web_search_result", title: "Kuvo", url: "https://kuvo.run" }] }] }, parent_tool_use_id: null },
    ]);
    const [read, search] = toolCalls(rows);
    // Kept as a file of its own beside the session's attachments.
    const kept = createHash("sha256").update(Buffer.from("CCCC", "base64")).digest("hex");
    assertMatch(read.state.content, [{ type: "text", text: "" }, { type: "file", mime: "image/png" }]);
    assert.ok(read.state.content[1].uri.endsWith(`/attachments/${id}/transcript-pictures/${kept}.png`), read.state.content[1].uri);
    assert.ok(fs.existsSync(fileURLToPath(read.state.content[1].uri)));
    assertMatch(search, { name: "websearch", state: { status: "completed", content: [{ type: "text", text: "Kuvo — https://kuvo.run" }] } });
  });

  test("what Claude Code says about itself: compacting, retrying, and a kind this bridge doesn't know", async () => {
    const rows = await turn([
      { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto" } },
      { type: "system", subtype: "api_retry", attempt: 2, max_retries: 10, error_status: 529, error: "overloaded" },
      { type: "system", subtype: "api_retry", attempt: 3, max_retries: 10, error_status: 529, error: "overloaded" },
      { type: "system", subtype: "brand_new_thing", content: "Something new to read." },
      { type: "system", subtype: "post_turn_summary", summary: "Already shown." },
    ]);
    assert.deepEqual(notices(rows), ["Claude's API is overloaded: trying again (2/10).", "Something new to read."]);
    assertMatch(rows.filter(row => row.type === "compaction"), [{ status: "completed", reason: "auto" }]);
  });

  test("several hooks failing the same way at the same moment show as one row, and the same failure again later as another", async () => {
    const failed = { type: "system", subtype: "hook_response", hook_name: "SessionStart:compact", hook_event: "SessionStart", outcome: "error", stderr: "Failed to run" };
    const rows = await turn([failed, failed, failed, failed, { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto" } }, failed]);
    const row = "Your SessionStart:compact hook failed: Failed to run.";
    assert.deepEqual(notices(rows), [row, row]);
    assert.deepEqual(rows.map(row => row.type), ["user", "synthetic", "compaction", "synthetic", "idle"]);
  });

  test("Claude Code's early warning of a limit says how much is used, and calls it close only near the end (#257)", async () => {
    const rows = await turn([
      { type: "rate_limit_event", rate_limit_info: { status: "allowed_warning", rateLimitType: "seven_day", utilization: 0.25, surpassedThreshold: 0.25 } },
      { type: "rate_limit_event", rate_limit_info: { status: "allowed_warning", rateLimitType: "five_hour", utilization: 0.93, surpassedThreshold: 0.9 } },
      { type: "rate_limit_event", rate_limit_info: { status: "allowed_warning", rateLimitType: "seven_day_opus", utilization: 0.4 } },
      { type: "rate_limit_event", rate_limit_info: { status: "allowed_warning", rateLimitType: "seven_day_opus", utilization: 0.6 } },
      { type: "rate_limit_event", rate_limit_info: { status: "allowed_warning", rateLimitType: "seven_day_opus", utilization: 0.92 } },
    ]);
    assert.deepEqual(notices(rows), [
      "You've used 25% of your weekly Claude limit.",
      "You're close to your 5-hour Claude limit: 93% used.",
      "You've used 40% of your weekly Opus limit.",
      "You're close to your weekly Opus limit: 92% used.",
    ]);
  });
});

describe("summarizing when you ask (/compact)", { skip }, () => {
  const box = resident();
  // What Claude Code 2.1.318 sends for it, in order.
  const compacts = (more = {}) => [
    { type: "system", subtype: "status", status: "compacting" },
    { type: "system", subtype: "hook_started", hook_name: "SessionStart:compact", hook_event: "SessionStart", hook_id: "h1" },
    { type: "system", subtype: "hook_response", hook_name: "SessionStart:compact", hook_event: "SessionStart", hook_id: "h1", outcome: "success", exit_code: 0 },
    { type: "system", subtype: "status", status: null, compact_result: "success", ...more },
    { type: "system", subtype: "init", session_id: "$SESSION" },
    { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "manual", pre_tokens: 32985, post_tokens: 10541 } },
    { type: "user", isSynthetic: true, message: { role: "user", content: "This session is being continued from a previous conversation." } },
    { type: "user", isReplay: true, message: { role: "user", content: "<local-command-stdout>Compacted </local-command-stdout>" } },
    { type: "command_lifecycle", command: "compact" },
    { type: "result", subtype: "success", is_error: false, result: "", num_turns: 0, session_id: "$SESSION" },
  ];
  const words = rows => rows.flatMap(row => row.type === "synthetic" ? [row.text] : []);
  const users = rows => rows.filter(row => row.type === "user").map(row => row.text.split("\n", 1)[0]);

  /** A session after a turn, and what its summary plays. */
  async function afterATurn(summary) {
    const { id } = await box.begin([...streams("$UUID", [{ type: "text", text: "HARBOUR" }]), result(["$UUID"])]);
    await box.claude.idles(id);
    box.claude.script("/compact", summary);
    return id;
  }
  const compact = async id => {
    const answer = await box.claude.compact(id);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.data;
  };

  test("it's typed as you would, and ends on its first result: the row shows it, the conversation stays as it was", async () => {
    const id = await afterATurn(compacts());
    const before = heardUser(box.claude, "/compact").length;
    assert.equal((await compact(id)).state, "compacting");
    const rows = await box.claude.idles(id, 2);
    assert.equal(heardUser(box.claude, "/compact").length, before + 1);
    const typed = heardUser(box.claude, "/compact").at(-1).message.content;
    assert.equal(typed.filter(part => part.type === "text").map(part => part.text).join(""), "/compact");
    assert.equal(idles(rows)[1].outcome, "succeeded");
    assertMatch(rows.find(row => row.type === "compaction"), { status: "completed", reason: "manual" });
    assert.deepEqual(words(rows), []);
    assert.deepEqual(texts(rows), ["HARBOUR"]);
    assert.deepEqual(users(rows), [`go ${id}`]);
  });

  test("one Claude Code hasn't started on is refused, saying why", async () => {
    const { id } = box.next();
    const answer = await box.claude.compact(id);
    assert.equal(answer.status, 400, answer.text);
    assert.match(answer.text, /There's nothing to summarize yet: Claude Code hasn't started on this conversation\./);
  });

  // The reference bridge counts a conversation as started once its agent has seen it (the
  // handover's `seen` cursor); the Rust resident, without that cursor yet, by the agent's saved
  // state. 4.6 brings the cursor and turns this into a passing case.
  test("one whose agent has a conversation saved but hasn't seen this one is refused, saying why", {
    todo: boxBinary().typescript ? false : "4.6: the handover's seen cursor",
  }, async () => {
    const { id } = box.next({ slot: true });
    const answer = await box.claude.compact(id);
    assert.equal(answer.status, 400, answer.text);
    assert.match(answer.text, /hasn't started on this conversation/);
  });

  test("one that fails shows why, though its result says it went through", async () => {
    const id = await afterATurn([
      { type: "system", subtype: "status", status: null, compact_result: "failed", compact_error: "Not enough messages to compact." },
      { type: "result", subtype: "success", is_error: false, result: "", num_turns: 0 },
    ]);
    await compact(id);
    const rows = await box.claude.idles(id, 2);
    assertMatch(rows.find(row => row.type === "compaction"), { status: "failed" });
    assert.equal(idles(rows)[1].error?.message, "Claude Code couldn't summarize the conversation: Not enough messages to compact.");
  });

  test("Stop during it: Claude Code is told to stop, and the row says it didn't", async () => {
    const id = await afterATurn([{ wait: "interrupt" }, result(["$UUID"], "error_during_execution")]);
    const before = heardUser(box.claude, "/compact").length;
    await compact(id);
    await until(() => heardUser(box.claude, "/compact").length === before + 1, "Claude Code was never asked.");
    await box.claude.interrupt(id);
    const rows = await box.claude.idles(id, 2);
    assert.equal(box.claude.heard().filter(line => line.type === "control_request" && line.request?.subtype === "interrupt").length, 1);
    assert.equal(idles(rows)[1].outcome, "interrupted");
    assertMatch(rows.find(row => row.type === "compaction"), { status: "failed" });
    assert.deepEqual(texts(rows), ["HARBOUR"]);
  });

  test("a hook that fails as it compacts still says so", async () => {
    const id = await afterATurn([
      { type: "system", subtype: "hook_response", hook_name: "SessionStart:compact", hook_event: "SessionStart", outcome: "error", stderr: "Failed to run" },
      ...compacts(),
    ]);
    await compact(id);
    const rows = await box.claude.idles(id, 2);
    assert.deepEqual(words(rows), ["Your SessionStart:compact hook failed: Failed to run."]);
  });

  test("shows as one row, not a second one for what Claude Code says as it summarizes", async () => {
    const id = await afterATurn(compacts());
    await compact(id);
    const rows = await box.claude.idles(id, 2);
    assertMatch(rows.filter(row => row.type === "compaction"), [{ status: "completed", reason: "manual" }]);
  });
});

describe("Claude Code's permissions", { skip }, () => {
  const box = resident();
  const decision = answer => answer.response?.hookSpecificOutput?.permissionDecision;
  const cards = async (id, count = 1) => until(async () => {
    const asked = await box.claude.permissions(id);
    return asked.length === count && asked;
  }, `There were never ${count} cards.`);
  const home = () => box.claude.env.HOME;

  test("Ask First: a command it would run is your card on its step; Allow Once lets it run", async () => {
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "npm test" } }]),
      hook("h1", "Bash", { command: "npm test" }, "toolu_1"), { reply: "h1" },
      canUse("c1", "Bash", { command: "npm test" }, "toolu_1", { title: "Claude wants to run npm test" }), { reply: "c1" },
      toolResult("toolu_1", "3 passed"),
      ...streams("$UUID", [{ type: "text", text: "They pass." }]),
      result(["$UUID"]),
    ], { ask: true });
    // Asked, even where Claude Code's own settings would let it run.
    assert.equal(decision(await box.claude.answered(id, "h1")), "ask");
    const [card] = await cards(id);
    assertMatch(card, { action: "shell", resources: ["npm test"], message: "Claude wants to run npm test", source: { type: "tool", id: "toolu_1" } });
    await box.claude.replyPermission(id, card.id, "once");
    assert.deepEqual((await box.claude.answered(id, "c1")).response, { behavior: "allow", updatedInput: { command: "npm test" }, toolUseID: "toolu_1" });
    const rows = await box.claude.idles(id);
    assertMatch(toolCalls(rows)[0], { name: "shell", state: { status: "completed", content: [{ type: "text", text: "3 passed" }] } });
  });

  test("the app's cards: its command and its question are the conversation's asks, and the answers given there reach it", async () => {
    const questions = [{ question: "Which size?", header: "Size", options: [{ label: "Small" }, { label: "Large" }], multiSelect: false }];
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_9", name: "Bash", input: { command: "make" } }]),
      hook("h9", "Bash", { command: "make" }, "toolu_9"), { reply: "h9" },
      canUse("c9", "Bash", { command: "make" }, "toolu_9"), { reply: "c9" },
      toolResult("toolu_9", "built"),
      canUse("q9", "AskUserQuestion", { questions }, "toolu_10"), { reply: "q9" },
      result(["$UUID"]),
    ], { ask: true });
    const card = await until(async () => (await sessionAsks(box.claude.listener, id)).find(ask => ask.kind === "permission"), "No card for the command.");
    assertCommandCard(card, "make");
    await answerSessionAsk(box.claude.listener, id, card.id, { answer: "once" });
    assertMatch((await box.claude.answered(id, "c9")).response, { behavior: "allow" });
    const question = await until(async () => (await sessionAsks(box.claude.listener, id)).find(ask => ask.kind === "question"), "No card for the question.");
    assertMatch(question.questions[0], { question: "Which size?", options: [{ value: "Small" }, { value: "Large" }] });
    await answerSessionAsk(box.claude.listener, id, question.id, { answers: { [question.questions[0].key]: "Large" } });
    assertMatch((await box.claude.answered(id, "q9")).response, { behavior: "allow", updatedInput: { questions, answers: { "Which size?": "Large" } } });
    await box.claude.idles(id);
    assert.deepEqual(await sessionAsks(box.claude.listener, id), []);
  });

  test("Allow Always: Claude Code keeps it for this session only, and the same command isn't asked about again; one more in the line is", async () => {
    const suggestions = [{ type: "addRules", rules: [{ toolName: "Bash", ruleContent: "npm test" }], behavior: "allow", destination: "localSettings" }];
    const { id } = await box.begin([
      canUse("c1", "Bash", { command: "npm test" }, "toolu_1", { permission_suggestions: suggestions }), { reply: "c1" },
      hook("h2", "Bash", { command: "npm test" }, "toolu_2"), { reply: "h2" },
      canUse("c2", "Bash", { command: "npm test" }, "toolu_2"), { reply: "c2" },
      hook("h3", "Bash", { command: "npm test && rm -rf build" }, "toolu_3"), { reply: "h3" },
      result(["$UUID"]),
    ], { ask: true });
    const [card] = await cards(id);
    await box.claude.replyPermission(id, card.id, "always");
    // Not written into the project's own Claude settings.
    assert.deepEqual((await box.claude.answered(id, "c1")).response.updatedPermissions, [{ ...suggestions[0], destination: "session" }]);
    assert.equal(decision(await box.claude.answered(id, "h2")), undefined);
    assertMatch((await box.claude.answered(id, "c2")).response, { behavior: "allow" });
    assert.equal(decision(await box.claude.answered(id, "h3")), "ask");
    await box.claude.idles(id);
    assert.deepEqual(await box.claude.permissions(id), []);
  });

  test("Allow Always on an edit lets every later edit in the session through, to other files and by other tools, without asking", async () => {
    const session = box.next({ ask: true });
    const [a, b] = [path.join(session.folder, "a.ts"), path.join(session.folder, "b.ts")];
    fs.writeFileSync(a, "const a = 1\n");
    fs.writeFileSync(b, "const b = 1\n");
    const second = { file_path: b, old_string: "const b = 1", new_string: "const b = 2" };
    const created = { file_path: path.join(session.folder, "c.ts"), content: "export {}\n" };
    const { id } = await box.begin([
      canUse("c1", "Edit", { file_path: a, old_string: "const a = 1", new_string: "const a = 2" }, "toolu_1"), { reply: "c1" },
      hook("h2", "Edit", second, "toolu_2"), { reply: "h2" },
      canUse("c2", "Edit", second, "toolu_2"), { reply: "c2" },
      hook("h3", "Write", created, "toolu_3"), { reply: "h3" },
      canUse("c3", "Write", created, "toolu_3"), { reply: "c3" },
      result(["$UUID"]),
    ], {}, session);
    const [card] = await cards(id);
    await box.claude.replyPermission(id, card.id, "always");
    assertMatch((await box.claude.answered(id, "c1")).response, { behavior: "allow" });
    assert.equal(decision(await box.claude.answered(id, "h2")), undefined);
    assertMatch((await box.claude.answered(id, "c2")).response, { behavior: "allow" });
    assert.equal(decision(await box.claude.answered(id, "h3")), undefined);
    assertMatch((await box.claude.answered(id, "c3")).response, { behavior: "allow" });
    await box.claude.idles(id);
    assert.deepEqual(await box.claude.permissions(id), []);
  });

  test("Deny: an edit's card shows what it would change; Claude Code is told you declined, and the turn goes on", async () => {
    const session = box.next({ ask: true });
    const file = path.join(session.folder, "a.ts");
    fs.writeFileSync(file, "const a = 1\n");
    const edit = { file_path: file, old_string: "const a = 1", new_string: "const a = 2" };
    const { id } = await box.begin([
      ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Edit", input: edit }]),
      canUse("c1", "Edit", edit, "toolu_1"), { reply: "c1" },
      toolResult("toolu_1", "The user declined this.", { error: true }),
      ...streams("$UUID", [{ type: "text", text: "Leaving it as it is." }]),
      result(["$UUID"]),
    ], {}, session);
    const [card] = await cards(id);
    assertMatch(card, { action: "edit", resources: [file], metadata: { files: [{ file, additions: 1, deletions: 1 }] } });
    await box.claude.replyPermission(id, card.id, "reject");
    assert.deepEqual((await box.claude.answered(id, "c1")).response, { behavior: "deny", message: "The user declined this.", toolUseID: "toolu_1" });
    const rows = await box.claude.idles(id);
    assert.equal(idles(rows)[0].outcome, "succeeded");
    assertMatch(toolCalls(rows)[0], { name: "edit", state: { status: "error" } });
    assert.ok(texts(rows).includes("Leaving it as it is."));
  });

  test("Approve All: what it asks about runs without a card, but reading a .env file still asks", async () => {
    const session = box.next();
    const edit = { file_path: path.join(session.folder, "a.ts"), old_string: "1", new_string: "2" };
    const secrets = path.join(session.folder, ".env");
    const { id } = await box.begin([
      hook("h1", "Edit", edit, "toolu_1"), { reply: "h1" },
      canUse("c1", "Edit", edit, "toolu_1"), { reply: "c1" },
      canUse("c2", "Bash", { command: "rm -rf build" }, "toolu_2"), { reply: "c2" },
      hook("h3", "Read", { file_path: secrets }, "toolu_3"), { reply: "h3" },
      canUse("c3", "Read", { file_path: secrets }, "toolu_3"), { reply: "c3" },
      result(["$UUID"]),
    ], {}, session);
    assert.equal(decision(await box.claude.answered(id, "h1")), undefined);
    assertMatch((await box.claude.answered(id, "c1")).response, { behavior: "allow", updatedInput: edit });
    assertMatch((await box.claude.answered(id, "c2")).response, { behavior: "allow" });
    assert.equal(decision(await box.claude.answered(id, "h3")), "ask");
    const [card] = await cards(id);
    assertMatch(card, { action: "read", resources: [secrets] });
    await box.claude.replyPermission(id, card.id, "once");
    assertMatch((await box.claude.answered(id, "c3")).response, { behavior: "allow" });
    await box.claude.idles(id);
  });

  test("reading a file outside the project asks first, as the engine's agents do, but not Claude Code's own: its memory, a long output it saved, its background commands' output", async () => {
    const keys = path.join(home(), ".ssh", "id_ed25519");
    const settings = path.join(home(), ".claude");
    const saved = path.join(settings, "projects", "-tmp-project", "0b5e1f1c-0000-4000-8000-000000000000", "tool-results", "toolu_0.txt");
    const memory = path.join(settings, "projects", "-tmp-project", "memory", "MEMORY.md");
    const unix = process.platform !== "win32";
    const uid = unix ? process.getuid() : 0;
    const own = [
      ["Read", { file_path: saved }],
      ["Read", { file_path: path.join(path.dirname(saved), "pages", "1.png") }],
      ["Read", { file_path: memory }],
      ["Write", { file_path: path.join(path.dirname(memory), "testing.md"), content: "Run the tests with bun test." }],
      ...(unix ? [["Read", { file_path: `/tmp/claude-${uid}/-tmp-project/0b5e1f1c-0000-4000-8000-000000000000/tasks/b1.output` }]] : []),
    ];
    const outside = [`${saved}/../../../../../.ssh/id_ed25519`, ...(unix ? [`/tmp/claude-${uid}/notes.txt`] : []), path.join(settings, "settings.json")];
    const { id } = await box.begin([
      hook("h1", "Read", { file_path: keys }, "toolu_1"), { reply: "h1" },
      canUse("c1", "Read", { file_path: keys }, "toolu_1"), { reply: "c1" },
      ...own.flatMap(([tool, input], index) => [
        hook(`own-h${index}`, tool, input, `toolu_own${index}`), { reply: `own-h${index}` },
        canUse(`own-c${index}`, tool, input, `toolu_own${index}`), { reply: `own-c${index}` },
      ]),
      ...outside.flatMap((file, index) => [hook(`out-h${index}`, "Read", { file_path: file }, `toolu_out${index}`), { reply: `out-h${index}` }]),
      result(["$UUID"]),
    ]);
    assert.equal(decision(await box.claude.answered(id, "h1")), "ask");
    const [card] = await cards(id);
    assertMatch(card, { action: "external_directory", resources: [keys] });
    await box.claude.replyPermission(id, card.id, "reject");
    assertMatch((await box.claude.answered(id, "c1")).response, { behavior: "deny" });
    for (const index of own.keys()) {
      assert.equal(decision(await box.claude.answered(id, `own-h${index}`)), undefined, JSON.stringify(own[index]));
      assertMatch((await box.claude.answered(id, `own-c${index}`)).response, { behavior: "allow" });
    }
    for (const [index, file] of outside.entries()) assert.equal(decision(await box.claude.answered(id, `out-h${index}`)), "ask", file);
    await box.claude.idles(id);
    assert.deepEqual(await box.claude.permissions(id), []);
  });

  test("its own files aren't outside the project, but your rules still hold for them: Ask First asks before it writes its memory", async () => {
    const memory = path.join(home(), ".claude", "projects", "-tmp-project", "memory", "MEMORY.md");
    const { id } = await box.begin([
      hook("h1", "Write", { file_path: memory, content: "-" }, "toolu_1"), { reply: "h1" },
      hook("h2", "Read", { file_path: memory }, "toolu_2"), { reply: "h2" },
      result(["$UUID"]),
    ], { ask: true });
    assert.equal(decision(await box.claude.answered(id, "h1")), "ask");
    assert.equal(decision(await box.claude.answered(id, "h2")), undefined);
    await box.claude.idles(id);
  });

  test("your Blocks refuse a tool before it runs, under Approve All too: any command in a line counts, and Claude Code is told why", async () => {
    const settings = path.join(box.claude.home, "box", "opencode.json");
    fs.writeFileSync(settings, JSON.stringify({ policies: [{ action: "tool.use", resource: "shell:rm *", effect: "deny" }] }));
    try {
      const { id } = await box.begin([
        hook("h1", "Bash", { command: "npm test && rm -rf build" }, "toolu_1"), { reply: "h1" },
        hook("h2", "Bash", { command: "npm test" }, "toolu_2"), { reply: "h2" },
        canUse("c3", "Bash", { command: "rm -rf build" }, "toolu_3"), { reply: "c3" },
        result(["$UUID"]),
      ]);
      const refused = (await box.claude.answered(id, "h1")).response.hookSpecificOutput;
      assert.equal(refused.permissionDecision, "deny");
      assert.match(refused.permissionDecisionReason, /Settings › Permissions/);
      assert.equal(decision(await box.claude.answered(id, "h2")), undefined);
      // Asked about by Claude Code all the same: refused, with no card.
      assertMatch((await box.claude.answered(id, "c3")).response, { behavior: "deny" });
      await box.claude.idles(id);
      assert.deepEqual(await box.claude.permissions(id), []);
    } finally {
      fs.writeFileSync(settings, "{}\n");
    }
  });

  test("plan mode is Claude Code's own: it writes its plan to its file even under Ask First, other edits are refused, and Build takes it out", async () => {
    const session = box.next({ plan: true });
    const plan = { file_path: path.join(home(), ".claude", "plans", "a-plan.md"), content: "# The plan" };
    const edit = { file_path: path.join(session.folder, "a.ts"), old_string: "1", new_string: "2" };
    const { id } = await box.begin([
      hook("h1", "Write", plan, "toolu_1"), { reply: "h1" },
      canUse("c2", "Edit", edit, "toolu_2"), { reply: "c2" },
      canUse("c3", "ExitPlanMode", { plan: "# The plan" }, "toolu_3"), { reply: "c3" },
      { type: "assistant", message: { id: "plan-said", role: "assistant", content: [{ type: "text", text: "Here's the plan." }] }, parent_tool_use_id: null },
      result(["$UUID"]),
    ], {}, session);
    const taken = await until(() => heardUser(box.claude, `go ${id}`)[0], () => JSON.stringify(box.claude.heard().filter(line => line.type === "user").map(line => [line.pid, line.message.content])));
    const started = box.claude.heard().find(line => line.argv && line.pid === taken.pid);
    assert.ok(started.argv.some((arg, index) => arg === "--permission-mode=plan" || (arg === "--permission-mode" && started.argv[index + 1] === "plan")), JSON.stringify(started.argv));
    assert.equal(decision(await box.claude.answered(id, "h1")), undefined);
    assertMatch((await box.claude.answered(id, "c2")).response, { behavior: "deny" });
    // Done planning: told to wait for you, who builds it when you're ready.
    const exit = (await box.claude.answered(id, "c3")).response;
    assert.equal(exit.behavior, "deny");
    assert.match(exit.message, /wait/);
    await box.claude.idles(id);
    assert.deepEqual(await box.claude.permissions(id), []);
    const modes = () => box.claude.heard().filter(line => line.request?.subtype === "set_permission_mode" && line.pid === started.pid).map(line => line.request.mode);
    const before = modes().length;
    const answer = await box.claude.selectAgent(id, "build");
    assert.equal(answer.status, 200, answer.text);
    await box.claude.prompt(id, `build it ${id}`);
    await box.claude.idles(id, 2);
    assert.deepEqual(modes().slice(before), ["default"]);
  });

  test("in Build, a plan mode it went into on its own it leaves when its plan is done, and carries on", async () => {
    const { id } = await box.begin([canUse("c1", "ExitPlanMode", { plan: "# The plan" }, "toolu_1"), { reply: "c1" }, result(["$UUID"])]);
    assertMatch((await box.claude.answered(id, "c1")).response, { behavior: "allow", updatedInput: { plan: "# The plan" } });
    await box.claude.idles(id);
    assert.deepEqual(await box.claude.permissions(id), []);
  });

  test("a question it asks is a form: your answers reach it, and dismissing one tells it so", async () => {
    const questions = [{ question: "Which database?", header: "Database", options: [{ label: "Postgres" }, { label: "SQLite", description: "One file" }], multiSelect: false }];
    const { id } = await box.begin([
      canUse("q1", "AskUserQuestion", { questions }, "toolu_1"), { reply: "q1" },
      canUse("q2", "AskUserQuestion", { questions }, "toolu_2"), { reply: "q2" },
      canUse("q3", "AskUserQuestion", { questions }, "toolu_3"), { reply: "q3" },
      result(["$UUID"]),
    ]);
    const form = async () => until(async () => (await box.claude.forms(id))[0], "No form.");
    const first = await form();
    assertMatch(first.fields[0], { title: "Database", description: "Which database?", options: [{ value: "Postgres" }, { value: "SQLite", description: "One file" }] });
    assertMatch(first.metadata, { kind: "question", tool: { id: "toolu_1" } });
    assert.equal((await box.claude.replyForm(id, first.id, { q0: "SQLite" })).status, 200);
    assertMatch((await box.claude.answered(id, "q1")).response, { behavior: "allow", updatedInput: { questions, answers: { "Which database?": "SQLite" } } });
    const second = await until(async () => (await box.claude.forms(id)).find(each => each.id !== first.id), "No second form.");
    assert.equal((await box.claude.dismissForm(id, second.id)).status, 200);
    assertMatch((await box.claude.answered(id, "q2")).response, { behavior: "deny" });
    // Turned down by its box, with words: those are what it hears.
    const third = await until(async () => (await box.claude.forms(id)).find(each => each.id !== first.id && each.id !== second.id), "No third form.");
    assert.equal((await box.claude.dismissForm(id, third.id, "This is on Autopilot: leave it.")).status, 200);
    assertMatch((await box.claude.answered(id, "q3")).response, { behavior: "deny", message: "This is on Autopilot: leave it." });
    await box.claude.idles(id);
  });
});

describe("Claude Code, instructions that changed as its conversation goes on", { skip }, () => {
  const box = resident();
  /** A message's words, without the block the reference's link adds for a phone. */
  const words = text => text.replace(/\n\n<from-phone>[\s\S]*<\/from-phone>$/, "");
  /** The session's last message as Claude Code was sent it. */
  const lastSent = id => {
    const pids = new Set(box.claude.heard().filter(line => line.type === "user" && words(line.message.content.findLast(part => part.type === "text")?.text ?? "").includes(id)).map(line => line.pid));
    const sent = box.claude.heard().filter(line => line.type === "user" && pids.has(line.pid) && !line.isSynthetic);
    return words(sent.at(-1).message.content.findLast(part => part.type === "text").text);
  };
  /** What started the Claude Code that took the session's last message. */
  const started = id => {
    const pid = box.claude.heard().filter(line => line.type === "user" && line.message.content.some(part => part.type === "text" && part.text.includes(id))).at(-1)?.pid;
    return box.claude.heard().filter(line => line.pid === pid);
  };
  /** Kuvo Code's instructions for the session, as the box has them from its next start. */
  async function instruct(id, entries) {
    await box.claude.stop();
    const file = path.join(box.claude.home, "agents/sessions", id, "agent.json");
    const agent = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : {};
    agent.kuvoInstructions = { ...agent.kuvoInstructions, ...entries };
    for (const [key, value] of Object.entries(entries)) if (value === undefined) delete agent.kuvoInstructions[key];
    write(file, JSON.stringify(agent));
    await box.claude.start();
  }
  /** A message (naming its session, so its Claude Code is found), answered. */
  async function answered(id, text, turns, steps) {
    box.claude.script(`${text} ${id}`, steps ?? [result(["$UUID"])]);
    await box.claude.prompt(id, `${text} ${id}`);
    return box.claude.idles(id, turns);
  }

  test("moved to another folder, the conversation it goes on with is told what changed, once; unchanged, nothing extra", async () => {
    const { id } = box.next();
    await instruct(id, { "kuvo.worktree": "You work in the worktree at /old/place/tree." });
    await answered(id, "first", 1);
    // A new conversation has them as its instructions: no note.
    assert.ok(JSON.stringify(started(id)).includes("/old/place/tree"));
    assert.equal(lastSent(id), `first ${id}`);
    // It moved (another computer, another folder), and the box started again there.
    await instruct(id, { "kuvo.worktree": "You work in the worktree at /new/place/tree.", "kuvo.moved": "You moved to this computer from the Old Mac." });
    await answered(id, "second", 2);
    // The same conversation, which keeps the instructions it started with: what changed goes with the message.
    assert.ok(started(id).find(line => line.argv)?.argv.some(arg => arg.startsWith("--resume")));
    const told = lastSent(id);
    assert.ok(told.includes("/new/place/tree"), told);
    assert.ok(told.includes("You moved to this computer from the Old Mac."), told);
    assert.match(told, /Replaced or taken out:\s+You work in the worktree at \/old\/place\/tree\./);
    assert.ok(told.endsWith(`\n\nsecond ${id}`), told);
    // Unchanged since: nothing more.
    await answered(id, "third", 3);
    assert.equal(lastSent(id), `third ${id}`);
  });

  test("a conversation from before Kuvo Code kept what it was told hears its instructions as they are now, once", async () => {
    const { id } = box.next();
    await instruct(id, { "kuvo.moved": "You moved to this computer from the Old Mac." });
    await answered(id, "first", 1);
    // As a conversation kept by an older bridge: it has no word of what it was told.
    await box.claude.stop();
    const file = path.join(box.claude.home, "agents/sessions", id, "agent.json");
    const agent = JSON.parse(fs.readFileSync(file, "utf8"));
    for (const slot of Object.values(agent.threads ?? {})) delete slot.state?.claudeTold;
    delete agent.claudeTold;
    write(file, JSON.stringify(agent));
    assert.ok(!fs.readFileSync(file, "utf8").includes("claudeTold"));
    await box.claude.start();
    await answered(id, "second", 2);
    assert.ok(lastSent(id).includes("You moved to this computer from the Old Mac."));
    assert.ok(lastSent(id).endsWith(`\n\nsecond ${id}`));
    await answered(id, "third", 3);
    assert.equal(lastSent(id), `third ${id}`);
  });

  test("changed while Claude Code runs: a command you type goes as typed, and the next message says what changed", async () => {
    const { id } = box.next();
    await answered(id, "first", 1);
    // Its worktree changed: it starts again, going on with the same conversation.
    await instruct(id, { "kuvo.worktree": "You work in the worktree at /a/tree." });
    box.claude.script("/review", [result(["$UUID"])]);
    await box.claude.prompt(id, "/review");
    await box.claude.idles(id, 2);
    const typed = box.claude.heard().filter(line => line.type === "user").at(-1);
    assert.equal(words(typed.message.content.findLast(part => part.type === "text").text), "/review");
    assert.ok(box.claude.heard().find(line => line.argv && line.pid === typed.pid).argv.some(arg => arg.startsWith("--resume")));
    await answered(id, "go on", 3);
    assert.ok(lastSent(id).includes("You work in the worktree at /a/tree."));
    assert.ok(lastSent(id).endsWith(`\n\ngo on ${id}`));
  });

  // (Its first half, a message Claude Code never took because it quit, depends on the line
  // handing that message over again after the exit: 4.1b's retry of missed messages.)
  test("a message Claude Code took and failed on: what changed isn't said again", async () => {
    const { id } = box.next();
    await answered(id, "first", 1);
    await instruct(id, { "kuvo.project-folders": "The project's other folder is /elsewhere." });
    // It takes it, and fails on it (Claude's API overloaded).
    const rows = await answered(id, "second", 2, [result(["$UUID"], "error_during_execution", { result: "Overloaded" })]);
    assert.ok(lastSent(id).includes("/elsewhere"));
    // The reference's SDK closes Claude Code on an error result, so there the turn ends as that
    // exit and the line hands the message over again (4.1b): the rest is the Rust resident's, whose
    // Claude Code goes on and fails the turn in Claude's words, as the reference adapter's test has it.
    if (boxBinary().typescript) return assertMatch(idles(rows)[1], { outcome: "interrupted", reason: "agent-exit" });
    assertMatch(idles(rows)[1], { outcome: "failed", error: { message: "Overloaded" } });
    await answered(id, "third", 3);
    assert.equal(lastSent(id), `third ${id}`);
  });

  test("once Claude Code summarized its conversation, what it has is what it was started with: nothing more to say", async () => {
    const { id } = box.next();
    await answered(id, "first", 1);
    await instruct(id, { "kuvo.worktree": "You work in the worktree at /b/tree." });
    // Your command, as typed, and Claude Code summarizes.
    box.claude.script("/compact", [
      { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "manual", pre_tokens: 100 }, parent_tool_use_id: null },
      result(["$UUID"]),
    ]);
    await box.claude.prompt(id, "/compact");
    await box.claude.idles(id, 2);
    await answered(id, "after", 3);
    assert.equal(lastSent(id), `after ${id}`);
  });
});

describe("what Claude Code says as it stops", { skip }, () => {
  const box = resident();
  const windows = process.platform === "win32";
  const plain = "Claude Code process exited with code 1";
  /** A message the next Claude Code stops before taking, having said `stderr`: how its turn ends. */
  async function stopsAsItStarts(id, stderr, more) {
    const turns = idles(await box.claude.rows(id)).length + 1;
    box.claude.stopsAsItStarts(stderr, more);
    const text = `go ${turns} ${id}`;
    box.claude.script(text, [result(["$UUID"])]);
    await box.claude.prompt(id, text);
    return idles(await box.claude.idles(id, turns))[turns - 1];
  }
  const said = async (id, stderr, more) => (await stopsAsItStarts(id, stderr, more)).error?.message;

  test("stopped before it took your message, the turn fails with the last line it said, without a secret or a stack", async () => {
    const { id } = box.next();
    for (const [line, told] of [
      ["cannot reach https://me:p@ss@proxy.test/v1", "cannot reach https://…@proxy.test/v1"],
      ["sent Bearer abc.def-ghi, Authorization: Basic Zm9v", "sent Bearer …, Authorization: …"],
      [`API_KEY="a b" TOKEN=k1 x-api-key: k2`, `API_KEY=… TOKEN=… x-api-key: …`],
      [`{"apiKey":"k3","password": "k4"}`, `{"apiKey":…,"password": …}`],
      ["password: k5 auth_token: k6", "password: … auth_token: …"],
      ["ran it with --token k7 --api-key=k8", "ran it with --token … --api-key=…"],
      ["Invalid token sk-ant-oat01-abcdefghijklmnop", "Invalid token sk-ant-…"],
    ]) {
      const ended = await stopsAsItStarts(id, `${line}\n`);
      try {
        assertMatch(ended, { outcome: "failed", error: { message: `${plain}: ${told}` } });
      } catch (error) {
        error.message += `\nFixture resident log:\n${box.claude.log()}`;
        throw error;
      }
    }
    // Its stack, and the runtime's banner, aren't why.
    assert.equal(await said(id, "Error: out of memory\n    at run (cli.js:1:1)\n\nBun v1.3.14 (macOS arm64)\n"), `${plain}: Error: out of memory`);
  });

  test("a plain sentence keeps its words", async () => {
    const { id } = box.next();
    for (const line of ["Bearer token expired", "Invalid API key: not found", "Invalid token: expired", "max_tokens=4096 is too many", "Authorization failed for that account"]) {
      assert.equal(await said(id, `${line}\n`), `${plain}: ${line}`);
    }
  });

  test("a line too long to keep whole is left out, so no piece of a token gets through", async () => {
    const { id } = box.next();
    assert.equal(await said(id, `sk-ant-oat01-${"a".repeat(5000)}\n`), plain);
  });

  test("what an earlier Claude Code said isn't why a later one stopped", async () => {
    const { id } = box.next();
    assert.equal(await said(id, "the first one's reason\n"), `${plain}: the first one's reason`);
    assert.equal(await said(id, ""), plain);
  });

  test("one being replaced, still writing as it goes, doesn't speak for the new one", async () => {
    const setting = value => fs.writeFileSync(path.join(box.claude.home, "box", "conversation-compaction.json"), JSON.stringify(value));
    try {
      const { id } = box.next();
      // It answers, leaving something running that holds its error output.
      box.claude.script(`first ${id}`, [{ linger: { stderr: "the old one's last words\n", until: "old-goes" } }, result(["$UUID"])]);
      await box.claude.prompt(id, `first ${id}`);
      await box.claude.idles(id, 1);
      // The summary setting changed: the next message starts another, which stops as it starts,
      // once what the old one left has spoken.
      const conversations = () => box.claude.heard().filter(line => line.argv?.some(arg => arg.startsWith("--session-id") || arg.startsWith("--resume"))).length;
      const before = conversations();
      setting("600000");
      const ended = stopsAsItStarts(id, "", { until: "new-goes" });
      await until(() => conversations() > before, "The next Claude Code never started.");
      box.claude.go("old-goes");
      // (On Windows what Claude Code left running ends with it: nothing of it is left to speak.)
      if (!windows) await until(() => box.claude.heard().some(line => line.lingered), "What the old one left never spoke.");
      box.claude.go("new-goes");
      assertMatch(await ended, { outcome: "failed", error: { message: plain } });
    } finally {
      fs.rmSync(path.join(box.claude.home, "box", "conversation-compaction.json"), { force: true });
    }
  });

  test("a warning from a turn that went through isn't why a later one stopped", async () => {
    const { id } = box.next();
    box.claude.script(`first ${id}`, [{ stderr: "warning: something old\n" }, result(["$UUID"])]);
    await box.claude.prompt(id, `first ${id}`);
    assertMatch(idles(await box.claude.idles(id, 1))[0], { outcome: "succeeded" });
    box.claude.script(`second ${id}`, [{ exit: 1 }]);
    await box.claude.prompt(id, `second ${id}`);
    const ended = idles(await box.claude.idles(id, 2))[1];
    assertMatch(ended, { outcome: "interrupted", reason: "agent-exit" });
    assert.equal(ended.error, undefined);
  });

  test("Claude Code quitting after it took your message stops the turn, with nothing failed to say", async () => {
    const { id } = box.next();
    box.claude.script(`go ${id}`, [{ stderr: "Error: out of memory\n    at run (cli.js:1:1)\n\nBun v1.3.14 (macOS arm64)\n", exit: 1 }]);
    await box.claude.prompt(id, `go ${id}`);
    const ended = idles(await box.claude.idles(id, 1))[0];
    assertMatch(ended, { outcome: "interrupted", reason: "agent-exit" });
    assert.equal(ended.error, undefined);
  });


  test("with its error output held open by what it left running: what comes after it stopped still counts", {
    skip: windows && "On Windows what Claude Code left running ends with it, so nothing speaks after it.",
  }, async () => {
    const { id } = box.next();
    assert.equal(await said(id, "", { linger: { stderr: "said as it went\n", after: 300 } }), `${plain}: said as it went`);
  });

  test("with its error output held open by what it left running: the failure doesn't wait for long, and leaves out a line still coming", async () => {
    const { id } = box.next();
    try {
      const started = Date.now();
      assert.equal(await said(id, "a whole line\nhttps://me:hunt", { linger: { stderr: "er2@proxy\n", until: "held-goes" } }), `${plain}: a whole line`);
      assert.ok(Date.now() - started < 8000);
    } finally {
      box.claude.go("held-goes");
    }
  });

  test("with its error output held open by what it left running: you can stop the turn while it waits", async () => {
    const { id } = box.next();
    try {
      box.claude.stopsAsItStarts("", { linger: { until: "waits-goes" } });
      box.claude.script(`go ${id}`, [result(["$UUID"])]);
      const before = box.claude.heard().filter(line => line.argv).length;
      await box.claude.prompt(id, `go ${id}`);
      await until(() => box.claude.heard().filter(line => line.argv).length > before, "Claude Code never started.");
      await box.claude.interrupt(id);
      assertMatch(idles(await box.claude.idles(id, 1))[0], { outcome: "interrupted" });
    } finally {
      box.claude.go("waits-goes");
    }
  });
});

describe("Claude Code's background commands", { skip }, () => {
  const box = resident();
  const bash = (call, command) => ({ type: "assistant", message: { id: `m_${call}`, role: "assistant", model: "opus", content: [{ type: "tool_use", id: call, name: "Bash", input: { command, run_in_background: true } }] }, parent_tool_use_id: null, session_id: "$SESSION" });
  const started = (task, call, more = {}) => ({ type: "system", subtype: "task_started", task_id: task, task_type: "local_bash", description: "In the background", tool_use_id: call, is_backgrounded: true, session_id: "$SESSION", ...more });
  const live = tasks => ({ type: "system", subtype: "background_tasks_changed", tasks: tasks.map(task => ({ task_id: task, task_type: "local_bash", description: "In the background" })), session_id: "$SESSION" });
  const ended = (task, call, more = {}) => ({ type: "system", subtype: "task_notification", task_id: task, status: "completed", tool_use_id: call, summary: "exit code 0", session_id: "$SESSION", ...more });
  const listed = id => until(async () => (await box.claude.work()).sessions[id], "The command it left running was never listed.");
  const gone = id => until(async () => !(await box.claude.work()).sessions[id], "The command it left running stayed listed.");
  const words = text => text.replace(/\n\n<from-phone>[\s\S]*<\/from-phone>$/, "");
  const took = (id, text) => box.claude.heard().find(line => line.type === "user" && words(line.message.content.findLast(part => part.type === "text")?.text ?? "") === `${text} ${id}`)?.pid;

  test("a command it leaves running shows in the bar with what it printed, until it ends", async () => {
    const { id } = box.next();
    box.claude.printed("t_server", "ready on :3000\n");
    box.claude.script(`go ${id}`, [bash("tool_server", "npm run dev"), started("t_server", "tool_server"), live(["t_server"]), result(["$UUID"]),
      { until: "server-ends" }, ended("t_server", "tool_server"), live([])]);
    await box.claude.prompt(id, `go ${id}`);
    await box.claude.idles(id, 1);
    assertMatch(await listed(id), { commands: 1, subagents: 0, tasks: 0 });
    const bar = await box.claude.work(`/${id}`);
    assert.equal(bar.commands.length, 1);
    assertMatch(bar.commands[0], { command: "npm run dev", agents: true });
    // What's at work under it besides itself, as the session lists say: nothing.
    assert.deepEqual(bar.activity, { working: 0, asking: 0, sessions: [] });
    const output = `/${id}/commands/${bar.commands[0].id}/output`;
    assertMatch(await box.claude.work(output), { output: "ready on :3000", cut: false, state: "running" });
    // It ends: the bar lets it go, and what it printed stands.
    box.claude.go("server-ends");
    await gone(id);
    assertMatch(await until(async () => {
      const read = await box.claude.work(output);
      return read.state === "ended" && read;
    }, "Its end never showed."), { command: { command: "npm run dev" }, state: "ended" });
  });

  test("Stop on what it left running ends it, through Claude Code", async () => {
    const { id } = box.next();
    box.claude.script(`go ${id}`, [bash("tool_watch", "npm run watch"), started("t_watch", "tool_watch"), live(["t_watch"]), result(["$UUID"])]);
    await box.claude.prompt(id, `go ${id}`);
    await box.claude.idles(id, 1);
    assertMatch(await listed(id), { commands: 1 });
    await box.claude.stopBackground(id);
    await gone(id);
    assert.ok(box.claude.heard().some((line) => line.type === "control_request" && line.request?.subtype === "stop_task" && line.request.task_id === "t_watch"),
      "Claude Code was never told to stop it.");
  });

  test("a finished background command shows the final output file after Claude Code forgets its output", async () => {
    const { id } = box.next();
    const file = path.join(box.claude.home, `build-${id}.output`);
    box.claude.printed("t_build", "Compiling\n");
    box.claude.script(`go ${id}`, [bash("tool_build", "cargo build"), started("t_build", "tool_build"), live(["t_build"]), result(["$UUID"]),
      { until: "build-ends" }, ended("t_build", "tool_build", { output_file: file }), live([])]);
    await box.claude.prompt(id, `go ${id}`);
    await box.claude.idles(id, 1);
    await listed(id);
    const [command] = (await box.claude.work(`/${id}`)).commands;
    const output = `/${id}/commands/${command.id}/output`;
    assertMatch(await box.claude.work(output), { command: { id: command.id, agents: true }, output: "Compiling", state: "running" });
    fs.writeFileSync(file, "Compiling\nFinished\n");
    box.claude.printed("t_build");
    box.claude.go("build-ends");
    await gone(id);
    assertMatch(await until(async () => {
      const read = await box.claude.work(output);
      return read.state === "ended" && read;
    }, "Its end never showed."), { command: { id: command.id, agents: true }, output: "Compiling\nFinished", state: "ended" });
  });

  test("a foreground command keeps its start time when moved to the background", async () => {
    const { id } = box.next();
    const before = Date.now();
    box.claude.script(`go ${id}`, [started("t_moved", "tool_moved", { is_backgrounded: false }), { until: "moves" },
      { type: "system", subtype: "task_updated", task_id: "t_moved", patch: { is_backgrounded: true }, session_id: "$SESSION" }, live(["t_moved"]),
      result(["$UUID"]), { until: "moved-ends" }, ended("t_moved", "tool_moved"), live([])]);
    await box.claude.prompt(id, `go ${id}`);
    await until(() => took(id, "go"), "Claude Code never took the message.");
    const startedBy = Date.now();
    await new Promise(resolve => setTimeout(resolve, 400));
    box.claude.go("moves");
    await listed(id);
    const [command] = (await box.claude.work(`/${id}`)).commands;
    assert.ok(command.started >= before && command.started < startedBy + 250, JSON.stringify({ before, startedBy, command }));
    box.claude.go("moved-ends");
    await gone(id);
    await box.claude.idles(id, 1);
  });

  test("changing the summary setting leaves Claude Code's background command running", async () => {
    const setting = path.join(box.claude.home, "box", "conversation-compaction.json");
    try {
      const { id } = box.next();
      box.claude.script(`go ${id}`, [bash("tool_tests", "scripts/test.sh"), started("t_tests", "tool_tests"), live(["t_tests"]), result(["$UUID"])]);
      await box.claude.prompt(id, `go ${id}`);
      await box.claude.idles(id, 1);
      await listed(id);
      fs.writeFileSync(setting, JSON.stringify("200000"));
      // Its next message goes to the same Claude Code: starting another would end the tests.
      box.claude.script(`still ${id}`, [result(["$UUID"]), { until: "tests-end" }, ended("t_tests", "tool_tests"), live([])]);
      await box.claude.prompt(id, `still ${id}`);
      await box.claude.idles(id, 2);
      assert.equal(took(id, "still"), took(id, "go"));
      assertMatch(await box.claude.work(), { sessions: { [id]: { commands: 1 } } });
      // Once they're done, the next starts one with the new setting.
      box.claude.go("tests-end");
      await gone(id);
      box.claude.script(`then ${id}`, [result(["$UUID"])]);
      await box.claude.prompt(id, `then ${id}`);
      await box.claude.idles(id, 3);
      assert.notEqual(took(id, "then"), took(id, "go"));
      const launched = box.claude.heard().find(line => line.argv && line.pid === took(id, "then"));
      assert.equal(launched.env.CLAUDE_CODE_AUTO_COMPACT_WINDOW, "233000");
    } finally {
      fs.rmSync(setting, { force: true });
    }
  });
});
