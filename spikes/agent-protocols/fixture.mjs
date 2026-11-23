// Loopback-only Anthropic Messages and MCP fixtures. Model output is scripted.
import { createServer } from "node:http";

export const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a2xkAAAAASUVORK5CYII=";
export const permission = { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "ask" } };
const usage = { input_tokens: 30, output_tokens: 10, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 };

export function mcpResult(request) {
  if (request.method === "initialize") return {
    protocolVersion: request.params.protocolVersion, capabilities: { tools: {} },
    serverInfo: { name: "local-fixture", version: "1" },
  };
  if (request.method === "tools/list") return { tools: [
    { name: "echo", description: "Return a fixture marker", inputSchema: { type: "object" } },
  ] };
  if (request.method === "tools/call") return { content: [{ type: "text", text: "MCP_SPIKE_MARKER" }] };
  return {};
}

export async function fixture(work) {
  const seen = [];
  let serial = 0, limited = false;
  const server = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks)) : {};
    if (req.url === "/mcp") {
      seen.push({ mcp: body.method, hasHeader: req.headers["x-spike"] === "local-fixture" });
      if (body.id === undefined) return res.writeHead(202).end();
      return res.writeHead(200, { "Content-Type": "application/json" })
        .end(JSON.stringify({ jsonrpc: "2.0", id: body.id, result: mcpResult(body) }));
    }
    if (req.url?.includes("/count_tokens")) {
      return res.writeHead(200, { "Content-Type": "application/json" }).end('{"input_tokens":30}');
    }
    if (!req.url?.startsWith("/v1/messages")) {
      return res.writeHead(404, { "Content-Type": "application/json" }).end("{}");
    }
    const latest = body.messages.at(-1)?.content;
    const prompt = JSON.stringify(body.messages.slice(-2));
    const system = JSON.stringify(body.system ?? "");
    const child = system.includes("SPIKE_CHILD_INSTRUCTIONS") || latest?.some?.(block => block.text === "SPIKE_CHILD_TURN");
    const record = {
      prompt, model: body.model, effort: body.output_config?.effort,
      append: system.includes("SPIKE_APPENDED_INSTRUCTIONS"), child,
      toolResult: latest?.some?.(block => block.type === "tool_result"),
      tools: body.tools?.map(tool => tool.name) ?? [],
      image: latest?.some?.(block => block.type === "image" && block.source?.data === png),
    };
    seen.push(record);
    if (!limited && prompt.includes("SPIKE_LIMIT")) {
      limited = true;
      return res.writeHead(429, { "Content-Type": "application/json", "retry-after": "0" })
        .end(JSON.stringify({ type: "error", error: { type: "rate_limit_error", message: "fixture limit" } }));
    }
    let block = { type: "text", text: child ? "child-answer" : "fixture-answer" };
    if (!child && !record.toolResult && prompt.includes("SPIKE_READ")) {
      block = { type: "tool_use", id: "toolu_spike_read", name: "Read", input: { file_path: `${work}/read.txt` } };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_UNANSWERED_HOOK")) {
      block = { type: "tool_use", id: "toolu_spike_unanswered", name: "Read", input: { file_path: `${work}/recover-hook.txt` } };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_DENIED")) {
      block = { type: "tool_use", id: "toolu_spike_write", name: "Write", input: { file_path: `${work}/must-not-exist`, content: "denied" } };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_QUESTION")) {
      block = { type: "tool_use", id: "toolu_spike_question", name: "AskUserQuestion", input: {
        questions: [{ question: "Fixture color?", header: "Color", multiSelect: false,
          options: [{ label: "Blue", description: "First" }, { label: "Green", description: "Second" }] }],
      } };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_EXIT_PLAN")) {
      block = { type: "tool_use", id: "toolu_spike_plan", name: "ExitPlanMode", input: {} };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_TASK")) {
      block = { type: "tool_use", id: "toolu_spike_task", name: "Bash", input: {
        command: "printf SPIKE_TASK_OUTPUT; /bin/sleep 30", description: "Keep a fixture process alive", run_in_background: true,
      } };
    } else if (!child && !record.toolResult && prompt.includes("SPIKE_SUBAGENT")) {
      const name = record.tools.includes("Agent") ? "Agent" : "Task";
      block = { type: "tool_use", id: "toolu_spike_agent", name, input: {
        description: "Read a fixture", prompt: "SPIKE_CHILD_TURN", subagent_type: "spike-child",
      } };
    }
    const message = {
      id: `msg_spike_${++serial}`, type: "message", role: "assistant", model: body.model,
      stop_reason: block.type === "tool_use" ? "tool_use" : "end_turn", stop_sequence: null, usage,
    };
    const headers = {
      "anthropic-ratelimit-unified-status": "allowed_warning",
      "anthropic-ratelimit-unified-representative-claim": "five_hour",
      "anthropic-ratelimit-unified-reset": String(Math.floor(Date.now() / 1000) + 3600),
      "anthropic-ratelimit-unified-5h-utilization": "0.82",
      "anthropic-ratelimit-unified-5h-status": "allowed_warning",
    };
    if (!body.stream) {
      return res.writeHead(200, { ...headers, "Content-Type": "application/json" })
        .end(JSON.stringify({ ...message, content: [block] }));
    }
    res.writeHead(200, { ...headers, "Content-Type": "text/event-stream" });
    const emit = (type, fields = {}) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...fields })}\n\n`);
    emit("message_start", { message: { ...message, stop_reason: null, content: [] } });
    const tool = block.type === "tool_use";
    emit("content_block_start", { index: 0, content_block: tool ? { ...block, input: {} } : { type: "text", text: "" } });
    emit("content_block_delta", { index: 0, delta: tool
      ? { type: "input_json_delta", partial_json: JSON.stringify(block.input) }
      : { type: "text_delta", text: block.text } });
    // Wait for the peer's interrupt to cancel the request after a visible delta.
    if (!child && prompt.includes("SPIKE_INTERRUPT")) return;
    emit("content_block_stop", { index: 0 });
    emit("message_delta", { delta: { stop_reason: message.stop_reason, stop_sequence: null }, usage: { output_tokens: usage.output_tokens } });
    emit("message_stop");
    res.end();
  });
  await new Promise((resolve, reject) => server.listen(0, "127.0.0.1", resolve).on("error", reject));
  return {
    url: `http://127.0.0.1:${server.address().port}`, seen,
    close: () => { server.close(); server.closeAllConnections(); },
  };
}
