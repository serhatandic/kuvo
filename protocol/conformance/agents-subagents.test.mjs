// A native spawned agent is a conversation under the one that started it.
// Real: the resident, bridge, session store and message routes. ACP stand-in.
import assert from "node:assert/strict";
import { test } from "node:test";
import { boxBinary, request, until } from "./adapter.mjs";
import { headers, toolCalls, texts } from "./claude.mjs";
import { providers, scriptedAcp } from "./acp.mjs";

const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;
const words = (text, session) => ({
  ...(session ? { session } : {}),
  update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } },
});

async function withChild(check, stopped = false) {
  const acp = await scriptedAcp({ names: ["grok"], agy: false });
  const parent = "ses_parent";
  const native = "native-child";
  try {
    acp.seed(parent, providers.grok);
    acp.script("Count the files", [
      { session: native, update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Count the files" } } },
      words("There are three.", native),
      { session: native, update: { sessionUpdate: "tool_call", toolCallId: "child-read", kind: "read", title: "Read the list", status: "in_progress", rawInput: { path: "files.txt" } } },
      { session: native, update: { sessionUpdate: "tool_call_update", toolCallId: "child-read", status: "completed", rawOutput: "three files" } },
      { until: "parent-finish" },
      ...(stopped ? [words("Words after Stop.", native)] : []),
      words("Done."),
    ]);
    await acp.start();
    await acp.prompt(parent, "Count the files");
    const child = await until(async () => {
      const rows = await acp.rows(parent);
      const step = toolCalls(rows).find((tool) => tool.name === "subagent");
      return step?.state?.metadata?.sessionID;
    }, () => "The parent did not link the spawned agent: " + acp.log());
    await check(acp, parent, child);
  } finally {
    await acp.close();
  }
}

test("a spawned agent has its own task, tool steps and answer, linked from the parent", { skip, timeout: 60000 }, () =>
  withChild(async (acp, parent, child) => {
    const ask = (method, route, body) => request(acp.listener, method, route, body, headers);
    const head = await ask("GET", acp.route(child));
    assert.equal(head.status, 200, head.text);
    assert.equal(head.json.data.parentID, parent);
    assert.equal(head.json.data.title, "Count the files");
    await until(async () => {
      const live = await ask("GET", "kuvo/agents/api/session/active");
      return live.json.data?.[child];
    }, "The child was not shown as working.");
    const rows = await until(async () => {
      const rows = await acp.rows(child);
      return toolCalls(rows).some((tool) => tool.state?.status === "completed") && rows;
    }, "The child's tool didn't appear.");
    assert.equal(rows[0].text, "Count the files");
    assert.ok(texts(rows).join("").includes("There are three."));
    assert.match(JSON.stringify(toolCalls(rows)), /three files/);
    const writing = await ask("POST", acp.route(child) + "/prompt", { id: "msg_for_child", text: "More work" });
    assert.equal(writing.status, 400, writing.text);
    assert.match(writing.text, /can't take messages for its subagents/);
    acp.go("parent-finish");
    const final = await acp.idles(parent);
    const childEnd = await acp.idles(child);
    assert.equal(childEnd.at(-1).outcome, "succeeded");
    const link = toolCalls(final).find((tool) => tool.name === "subagent");
    assert.equal(link.state.metadata.sessionID, child);
    assert.equal(link.state.status, "completed");
    assert.equal(link.state.input.description, "Count the files");
    const removed = await ask("DELETE", `kuvo/sessions/${parent}`, {});
    assert.equal(removed.status, 200, removed.text);
    assert.equal((await ask("GET", acp.route(child))).status, 404);
  }));

test("stopping a spawned agent closes its transcript without stopping its parent", { skip, timeout: 60000 }, () =>
  withChild(async (acp, parent, child) => {
    await acp.interrupt(child);
    const ended = await acp.idles(child);
    assert.equal(ended.at(-1).outcome, "interrupted");
    acp.go("parent-finish");
    const final = await acp.idles(parent);
    assert.equal(final.at(-1).outcome, "succeeded");
    assert.doesNotMatch(texts(await acp.rows(child)).join(""), /Words after Stop/);
  }, true));

test("Claude Code gives a foreground Task its own transcript and keeps the parent link", { skip, timeout: 60000 }, async () => {
  const { scriptedClaude, streams, toolResult, result } = await import("./claude.mjs");
  const claude = await scriptedClaude();
  const parent = "ses_claude_parent";
  const call = "toolu_child";
  try {
    claude.seed(parent, { model: "default" });
    claude.script("Delegate", [
      ...streams("$UUID", [{ type: "tool_use", id: call, name: "Task", input: { description: "Count files", prompt: "Count them", subagent_type: "Explore", run_in_background: false } }]),
      ...streams("$UUID", [{ type: "text", text: "Three files." }]).map(frame => ({ ...frame, parent_tool_use_id: call })),
      toolResult(call, "Three files."),
      ...streams("$UUID", [{ type: "text", text: "Done." }]),
      result(["$UUID"]),
    ]);
    await claude.start();
    await claude.prompt(parent, "Delegate");
    const rows = await claude.idles(parent);
    const link = toolCalls(rows).find(tool => tool.name === "subagent");
    const child = link?.state?.metadata?.sessionID;
    assert.ok(child, JSON.stringify(link));
    const childRows = await claude.idles(child);
    assert.equal(childRows[0].text, "Count them");
    assert.ok(texts(childRows).join("").includes("Three files."));
    assert.equal(childRows.at(-1).outcome, "succeeded");
    assert.doesNotMatch(texts(rows).join(""), /Three files/);
  } finally {
    await claude.close();
  }
});
