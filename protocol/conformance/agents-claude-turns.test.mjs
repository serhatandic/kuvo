// Claude Code's turns through each resident, on a scripted stand-in (claude.test.ts: "Claude Code's
// context and usage", "Claude Code's reply", "summarizing on its own, as the conversation fills up").
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, test } from "node:test";
import {
  askFirst, assertMatch, canUse, idles, result, scriptedClaude, steps, streams, texts, toolCalls,
  toolResult,
} from "./claude.mjs";
import { boxBinary, until } from "./adapter.mjs";

const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

/**
 * One resident for a describe's cases, with sessions seeded before it starts (the reference
 * bridge reads its sessions as it starts): plain ones, and Ask First ones.
 */
function resident(fixture = {}) {
  const pool = { plain: [], ask: [] };
  const box = {};
  before(async () => {
    if (skip) return;
    box.claude = await scriptedClaude(fixture);
    for (const [kind, permissions] of [["plain", undefined], ["ask", askFirst]]) {
      for (let index = 1; index <= 12; index++) {
        const id = `ses_claude_${kind}_${String(index).padStart(3, "0")}`;
        const folder = path.join(box.claude.home, "projects", id);
        fs.mkdirSync(folder, { recursive: true });
        box.claude.seed(id, { directory: folder, model: "default", permissions });
        pool[kind].push({ id, folder });
      }
    }
    await box.claude.start();
  });
  after(async () => box.claude?.close());
  box.peek = ({ ask = false } = {}) => pool[ask ? "ask" : "plain"][0];
  box.next = ({ ask = false } = {}) => {
    const session = pool[ask ? "ask" : "plain"].shift();
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
    const during = await box.claude.rows(id);
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
