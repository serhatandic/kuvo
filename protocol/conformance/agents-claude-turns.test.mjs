// Claude Code's turns through each resident, on a scripted stand-in (claude.test.ts: "Claude Code's
// context and usage", "Claude Code's reply", "summarizing on its own, as the conversation fills up").
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
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
