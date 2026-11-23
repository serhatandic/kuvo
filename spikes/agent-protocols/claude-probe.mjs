import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { EventEmitter, once } from "node:events";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fixture, permission, png } from "./fixture.mjs";

const scratch = process.env.SCRATCH;
assert(scratch && process.env.HOME === join(scratch, "home"));
const lane = process.argv[2];
assert(["sdk", "rust", "unsigned", "unsigned-sdk"].includes(lane));
const unsigned = lane.startsWith("unsigned");
const sdk = lane.endsWith("sdk")
  ? await import(join(scratch, "sdk/node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs"))
  : undefined;
const root = join(scratch, lane + "-" + randomUUID());
const work = join(root, "work");
const plugin = join(scratch, "plugin");
mkdirSync(join(plugin, ".claude-plugin"), { recursive: true });
mkdirSync(join(plugin, "skills/spike"), { recursive: true });
writeFileSync(join(plugin, ".claude-plugin/plugin.json"), JSON.stringify({ name: "spike-fixture" }));
writeFileSync(join(plugin, ".mcp.json"), JSON.stringify({
  mcpServers: { must_be_skipped: { command: "/usr/bin/false" } },
}));
writeFileSync(join(plugin, "skills/spike/SKILL.md"), "---\nname: spike\ndescription: A spike skill\n---\nLocal fixture.\n");
mkdirSync(work, { recursive: true });
mkdirSync(join(root, "memory"));
writeFileSync(join(work, "read.txt"), "LOCAL_READ_MARKER\n");
const settings = { autoMemoryDirectory: join(root, "memory"), skillOverrides: { "spike-fixture:spike": "off" }, sandbox: { enabled: false } };
const disableHooks = { disableAllHooks: true };
const model = await fixture(work);
const environment = {
  ...process.env, CLAUDE_CONFIG_DIR: join(root, "config"),
  // A synthetic credential for the fixture. The Seatbelt profile keeps requests on loopback.
  ...(!unsigned ? { ANTHROPIC_API_KEY: "sk-ant-loopback-fixture", ANTHROPIC_BASE_URL: model.url } : {}),
};
const cli = join(scratch, "bin/claude");
const report = { lane, runs: [], requests: model.seen };

function stream() {
  const backlog = [];
  let take;
  return {
    push(value) { if (take) { const read = take; take = undefined; read(value); } else backlog.push(value); },
    async *[Symbol.asyncIterator]() {
      while (true) {
        const value = backlog.length ? backlog.shift() : await new Promise(resolve => { take = resolve; });
        if (value === null) return;
        yield value;
      }
    },
  };
}

const isResult = frame => frame.type === "result";
const forMessage = message => frame => isResult(frame) && frame.user_message_uuids?.includes(message.uuid);
const hook = input => input.tool_name !== "Write" ? permission : {
  hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: "Fixture block" },
};
function decision(name, input) {
  if (name === "AskUserQuestion") {
    return { behavior: "allow", updatedInput: { ...input, answers: { "Fixture color?": "Blue" } } };
  }
  if (["Read", "Agent", "Task", "Bash"].includes(name)) return { behavior: "allow", updatedInput: input };
  return { behavior: "deny", message: "Fixture tool denied" };
}
const isInit = frame => frame.type === "system" && frame.subtype === "init";
function user(text, image = false) {
  return {
    type: "user", uuid: randomUUID(), parent_tool_use_id: null,
    message: { role: "user", content: [
      ...(image ? [{ type: "image", source: { type: "base64", media_type: "image/png", data: png } }] : []),
      { type: "text", text },
    ] },
  };
}

async function run(mode, session, scenario) {
  const trace = join(root, mode + "-input.jsonl");
  const events = new EventEmitter();
  const record = { mode, wireIn: [], wireOut: [], public: [], stderr: [], controls: {} };
  report.runs.push(record);
  const prompt = stream();
  let child, sdkQuery, next = 0, closed = false, permissionCalls = 0;
  let releasePermission;
  const pendingPermission = new Promise(resolve => { releasePermission = resolve; });
  const stderr = bytes => record.stderr.push(bytes.toString());
  const add = (list, frame) => { list.push(frame); events.emit("frame"); };
  const read = (input, list) => {
    createInterface({ input }).on("line", line => {
      try { add(list, JSON.parse(line)); }
      catch { record.stderr.push(line); }
    });
  };
  const raw = (start, predicate, list = record.wireOut) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => finish(new Error(`Timed out in ${mode} at wire offset ${start}`)), 15_000);
      function finish(error, frame) {
        clearTimeout(timer);
        events.off("frame", check);
        if (error) reject(error); else resolve(frame);
      }
      function check() {
        const frame = list.slice(start).find(predicate);
        if (frame) finish(undefined, frame);
        else if (closed) finish(new Error(`Child exited in ${mode}`));
      }
      events.on("frame", check);
      check();
    });

  function watch(process) {
    child = process;
    child.once("close", () => { closed = true; events.emit("frame"); });
    child.stderr?.on("data", stderr);
    read(child.stdout, record.wireOut);
    return child;
  }

  if (sdk) {
    sdkQuery = sdk.query({
      prompt: mode === "generate" ? "SPIKE_GENERATE" : prompt,
      options: {
        pathToClaudeCodeExecutable: cli, cwd: work, env: environment,
        ...(mode === "generate" ? {
          model: "haiku", maxTurns: 1, tools: [], allowedTools: [], mcpServers: {},
          strictMcpConfig: true, persistSession: false, settings: disableHooks,
        } : {
          model: "sonnet", effort: "low", permissionMode: "default",
          includePartialMessages: true, forwardSubagentText: true,
          systemPrompt: { type: "preset", preset: "claude_code", append: "SPIKE_APPENDED_INSTRUCTIONS" },
          agents: { "spike-child": { description: "A local fixture subagent", prompt: "SPIKE_CHILD_INSTRUCTIONS", model: "haiku" } },
          plugins: [{ type: "local", path: plugin, skipMcpDiscovery: true }],
          settings,
          ...(mode === "resume" ? { resume: session } : { sessionId: session }),
          hooks: { PreToolUse: [{ hooks: [async (input, _toolUseId, { signal }) => {
            if (input.tool_input?.file_path?.endsWith("/recover-hook.txt")) {
              await new Promise(resolve => signal.addEventListener("abort", resolve, { once: true }));
              return {};
            }
            return hook(input);
          }] }] },
          canUseTool: async (name, input, { signal }) => {
            permissionCalls++;
            if (mode === "recover") await Promise.race([pendingPermission, new Promise(resolve => signal.addEventListener("abort", resolve, { once: true }))]);
            return decision(name, input);
          },
        }),
        spawnClaudeCodeProcess: options => {
          record.argv = options.args;
          const process = watch(spawn(options.command, options.args, {
            cwd: options.cwd, env: options.env, stdio: ["pipe", "pipe", "pipe"],
          }));
          const write = process.stdin.write.bind(process.stdin);
          process.stdin.write = (data, ...args) => {
            for (const line of data.toString().trim().split("\n")) {
              try { record.wireIn.push(JSON.parse(line)); } catch { /* Plain output is recorded on stdout. */ }
            }
            return write(data, ...args);
          };
          return process;
        },
      },
    });
    (async () => {
      try {
        for await (const frame of sdkQuery) add(record.public, frame);
      } catch (error) {
        stderr(String(error));
      }
    })();
  } else {
    watch(spawn(join(scratch, "bin/claude-wire-spike"), [
      cli, session, mode, JSON.stringify(mode === "generate" ? disableHooks : settings),
    ], { cwd: work, env: { ...environment, TRACE_WIRE: trace }, stdio: ["pipe", "pipe", "pipe"] }));
  }

  function write(step) {
    child.stdin.write(JSON.stringify(step) + "\n");
  }

  if (!sdk) {
    function callbacks() {
      const frame = record.wireOut.at(-1);
      if (frame?.type !== "control_request") return;
      const request = frame.request;
      if (request.subtype === "hook_callback") {
        assert.equal(request.callback_id, "0");
        if (request.input.tool_input?.file_path?.endsWith("/recover-hook.txt")) return;
        write({ kind: "callback", id: frame.request_id, response: hook(request.input) });
      } else if (request.subtype === "can_use_tool" && mode !== "recover") {
        answerPermission(frame);
      }
    }
    events.on("frame", callbacks);
    child.once("close", () => events.off("frame", callbacks));
  }

  function answerPermission(frame) {
    const request = frame.request;
    write({
      kind: "callback", id: frame.request_id,
      response: { ...decision(request.tool_name, request.input), toolUseID: request.tool_use_id },
    });
  }

  async function control(name, request, sdkCall, kind = "control") {
    try {
      if (sdk) {
        let deadline;
        try {
          return record.controls[name] = await Promise.race([sdkCall(), new Promise((_, reject) => {
            deadline = setTimeout(() => reject(new Error(`Timed out waiting for SDK ${name}`)), 15_000);
          })]);
        } finally {
          clearTimeout(deadline);
        }
      }
      const id = String(++next);
      const start = record.wireOut.length;
      write({ kind, id, request });
      const { response } = await raw(start, frame => frame.type === "control_response" && frame.response.request_id === id);
      if (response.subtype !== "success") throw new Error(response.error);
      return record.controls[name] = {
        ...response.response,
        ...(response.pending_permission_requests?.length
          ? { pending_permission_requests: response.pending_permission_requests } : {}),
      };
    } catch (error) {
      record.controls[name] = { error: error.message };
      return record.controls[name];
    }
  }

  const api = {
    query: sdkQuery,
    init: () => control("initialize", undefined, () => sdkQuery.initializationResult(), "initialize"),
    control,
    message: frame => sdk ? prompt.push(frame) : write({ kind: "message", message: frame }),
    raw, record,
    approve: frame => sdk ? releasePermission() : answerPermission(frame),
  };
  try {
    const initial = await api.init();
    assert(initial.models?.length, JSON.stringify(initial));
    await scenario(api);
  } finally {
    record.permissionCalls = permissionCalls;
    releasePermission();
    if (!closed) {
      const done = once(child, "close");
      const timer = setTimeout(() => child.kill(), 4_000);
      if (sdkQuery) { prompt.push(null); sdkQuery.close(); }
      else child.stdin.end();
      await done;
      clearTimeout(timer);
    }
    if (!sdk && existsSync(trace)) {
      record.wireIn = readFileSync(trace, "utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
    }
  }
}

const session = randomUUID();
try {
  await run("new", session, async api => {
    if (unsigned) {
      api.message(user("SPIKE_UNSIGNED"));
      const result = await api.raw(0, isResult);
      assert.equal(result.is_error, true);
      return;
    }
    const servers = {
      local_http: { type: "http", url: model.url + "/mcp", headers: { "x-spike": "local-fixture" }, timeout: 5000 },
      local_stdio: { type: "stdio", command: process.execPath, args: [join(scratch, "stdio-mcp.mjs")], env: { SPIKE_MCP: "local" }, timeout: 5000 },
    };
    await api.control("mcp-set", { subtype: "mcp_set_servers", servers }, () => api.query.setMcpServers(servers));
    await api.control("settings", { subtype: "get_settings" }, () => api.query.getSettings());
    await api.control("mcp-status", { subtype: "mcp_status" }, () => api.query.mcpServerStatus());
    for (const [name, message] of [
      ["read", user("SPIKE_READ")], ["image", user("SPIKE_IMAGE", true)],
      ["denied", user("SPIKE_DENIED")], ["question", user("SPIKE_QUESTION")], ["subagent", user("SPIKE_SUBAGENT")],
      ["limit", user("SPIKE_LIMIT")],
    ]) {
      const since = api.record.wireOut.length;
      api.message(message);
      const result = await api.raw(since, forMessage(message));
      api.record[name] = result;
    }
    const normal = async (name, request, call) => api.control(name, request, () => call(api.query));
    await normal("set-model", { subtype: "set_model", model: "opus" }, q => q.setModel("opus"));
    await normal("set-effort", { subtype: "apply_flag_settings", settings: { effortLevel: "high" } }, q => q.applyFlagSettings({ effortLevel: "high" }));
    await normal("set-plan", { subtype: "set_permission_mode", mode: "plan" }, q => q.setPermissionMode("plan"));
    await normal("context", { subtype: "get_context_usage", detail: "summary" }, q => q.getContextUsage({ detail: "summary" }));
    let since = api.record.wireOut.length;
    const changed = user("SPIKE_CHANGED");
    api.message(changed);
    api.record.changed = await api.raw(since, forMessage(changed));
    since = api.record.wireOut.length;
    const exitPlan = user("SPIKE_EXIT_PLAN");
    api.message(exitPlan);
    api.record.plan = await api.raw(since, forMessage(exitPlan));
    await normal("non-default-model", { subtype: "set_model", model: "sonnet" }, q => q.setModel("sonnet"));
    await normal("clear-model", { subtype: "set_model" }, q => q.setModel(undefined));
    await normal("clear-effort", { subtype: "apply_flag_settings", settings: { effortLevel: null } }, q => q.applyFlagSettings({ effortLevel: null }));
    since = api.record.wireOut.length;
    const unset = user("SPIKE_MODEL_OMITTED");
    api.message(unset);
    await api.raw(since, forMessage(unset));
    await normal("set-default", { subtype: "set_permission_mode", mode: "default" }, q => q.setPermissionMode("default"));
    await normal("mcp-clear", { subtype: "mcp_set_servers", servers: {} }, q => q.setMcpServers({}));
    since = api.record.wireOut.length;
    const task = user("SPIKE_TASK");
    api.message(task);
    const started = await api.raw(since, frame => frame.subtype === "task_started" && frame.task_type === "local_bash");
    await normal("task-output", { subtype: "get_task_output", task_id: started.task_id }, q => q.getTaskOutput(started.task_id));
    await normal("stop-task", { subtype: "stop_task", task_id: started.task_id }, q => q.stopTask(started.task_id));
    api.record.stopped = await api.raw(since, frame => frame.subtype === "task_notification" && frame.task_id === started.task_id);
    await api.raw(since, forMessage(task));
    since = api.record.wireOut.length;
    api.message(user("/compact"));
    api.record.compact = await api.raw(since, isResult);
    since = api.record.wireOut.length;
    const interrupted = user("SPIKE_INTERRUPT");
    api.message(interrupted);
    await api.raw(since, frame => frame.type === "stream_event" && frame.event.type === "content_block_delta");
    await normal("interrupt", { subtype: "interrupt" }, q => q.interrupt());
    api.record.interrupted = await api.raw(since, forMessage(interrupted));
  });
  if (!unsigned) {
    await run("resume", session, async api => {
      api.message(user("SPIKE_RESUME"));
      const started = await api.raw(0, isInit);
      assert.equal(started.session_id, session);
      api.record.resumed = await api.raw(0, isResult);
    });
    await run("recover", randomUUID(), async api => {
      const reading = user("SPIKE_READ");
      api.message(reading);
      const asking = await api.raw(0, frame => frame.request?.subtype === "can_use_tool");
      const steering = { ...user("SPIKE_STEER"), priority: "next" };
      api.message(steering);
      const second = await api.control("reinitialize", undefined, () => api.query.reinitialize(), "initialize");
      assert(second.models?.length, JSON.stringify(second));
      const responses = api.record.wireOut.filter(frame => frame.type === "control_response");
      const [replayed] = responses.at(-1)?.response?.pending_permission_requests ?? [];
      assert.equal(replayed?.request_id, asking.request_id);
      api.approve(replayed);
      api.record.recovered = await api.raw(0, forMessage(reading));
      api.record.steered = await api.raw(0, forMessage(steering));
      let since = api.record.wireOut.length;
      api.message(user("/clear"));
      await api.raw(since, isResult);
      since = api.record.wireOut.length;
      const afterClear = user("SPIKE_AFTER_CLEAR");
      api.message(afterClear);
      api.record.cleared = await api.raw(since, forMessage(afterClear));
      since = api.record.wireOut.length;
      const hook = user("SPIKE_UNANSWERED_HOOK");
      api.message(hook);
      const abandoned = await api.raw(since, frame => frame.request?.subtype === "hook_callback");
      await api.control("reinitialize-hook", undefined, () => api.query.reinitialize(), "initialize");
      api.record.cancelledHook = await api.raw(since, frame => frame.type === "control_cancel_request" && frame.request_id === abandoned.request_id);
      await api.raw(since, forMessage(hook));
    });
    await run("generate", randomUUID(), async api => {
      if (!sdk) api.message(user("SPIKE_GENERATE"));
      api.record.generated = await api.raw(0, isResult);
    });
    verify();
  }
} finally {
  model.close();
  writeFileSync(join(scratch, lane + "-claude.json"), JSON.stringify(report, null, 2));
}


function verify() {
  const [main, resume, recover, generate] = report.runs;
  const fixtureRequests = model.seen.filter(request => request.model);
  const turns = ["read", "image", "denied", "question", "subagent", "limit", "changed", "compact"];
  for (const name of turns) assert.equal(main[name]?.is_error, false, name);
  for (const run of report.runs) {
    for (const [name, control] of Object.entries(run.controls)) assert(!control?.error, `${name}: ${control?.error}`);
  }
  assert(main.controls.initialize.agents.some(agent => agent.name === "spike-child"));
  assert(main.controls.initialize.hooks_applied);
  const started = main.wireOut.find(isInit);
  assert(started.plugins.some(entry => entry.name === "spike-fixture"));
  assert(!started.mcp_servers.some(entry => entry.name === "must_be_skipped"));
  assert.equal(main.controls.settings.effective.skillOverrides["spike-fixture:spike"], "off");
  assert.equal(main.controls.settings.effective.autoMemoryDirectory, settings.autoMemoryDirectory);
  assert.equal(main.controls.settings.effective.sandbox.enabled, false);
  assert(main.controls.context.maxTokens > 0);
  assert.deepEqual(main.controls["mcp-set"].added.sort(), ["local_http", "local_stdio"]);
  assert.deepEqual(main.controls["mcp-clear"].removed.sort(), ["local_http", "local_stdio"]);
  const mcpStatus = main.controls["mcp-status"].mcpServers ?? main.controls["mcp-status"];
  assert.deepEqual(mcpStatus.map(server => [server.name, server.status]).sort(), [
    ["local_http", "connected"], ["local_stdio", "connected"],
  ]);
  assert(mcpStatus.every(server => server.tools?.some(tool => tool.name === "echo")));
  assert(model.seen.some(entry => entry.mcp === "initialize" && entry.hasHeader));
  assert(fixtureRequests.some(request => request.append && request.image));
  assert(fixtureRequests.some(request => request.toolResult && request.prompt.includes("LOCAL_READ_MARKER")));
  assert(fixtureRequests.some(request => request.toolResult && request.prompt.includes("Your questions have been answered")));
  assert(!existsSync(join(work, "must-not-exist")));
  assert(fixtureRequests.some(request => request.toolResult && request.prompt.includes("Fixture block")));
  assert(fixtureRequests.some(request => request.child && request.model.includes("haiku")));
  assert(main.wireOut.some(frame => frame.parent_tool_use_id && frame.type === "assistant"
    && frame.message.content.some(block => block.text === "child-answer")));
  assert(main.wireOut.some(frame => frame.subtype === "api_retry" && frame.error === "rate_limit"));
  assert(fixtureRequests.some(request => request.prompt.includes("SPIKE_CHANGED")
    && request.model.includes("opus") && request.effort === "high"));
  const defaultModel = main.controls.initialize.models.find(model => model.value === "default")?.resolvedModel;
  assert(defaultModel);
  assert(fixtureRequests.some(request => request.prompt.includes("SPIKE_MODEL_OMITTED")
    && request.model === defaultModel && request.effort !== "high"));
  assert(main.wireOut.some(frame => isInit(frame) && frame.permissionMode === "plan"));
  assert(main.plan.permission_denials.some(entry => entry.tool_name === "ExitPlanMode"));
  assert(main.wireOut.some(frame => isInit(frame) && frame.permissionMode === "default"));
  assert(main.wireOut.some(frame => frame.subtype === "compact_boundary" && frame.compact_metadata.post_tokens > 0));
  assert.equal(main.stopped.status, "stopped");
  assert.equal(typeof main.controls["task-output"].total_bytes, "number");
  assert.equal(main.interrupted.terminal_reason, "aborted_streaming");
  assert(main.changed.usage.input_tokens > 0 && main.changed.total_cost_usd > 0);
  assert(Object.values(main.changed.modelUsage).some(usage => usage.contextWindow > 0));
  assert.equal(resume.resumed.is_error, false);
  assert.equal(recover.recovered.is_error, false);
  assert.equal(recover.steered.is_error, false);
  assert.notEqual(recover.cleared.session_id, recover.recovered.session_id);
  assert(recover.wireOut.some(frame => frame.type === "conversation_reset"));
  assert(recover.cancelledHook);
  assert.equal(generate.generated.result, "fixture-answer");
  const last = fixtureRequests.at(-1);
  assert(last.model.includes("haiku") && !last.append && last.tools.length === 0);
  if (sdk) {
    assert.equal(recover.permissionCalls, 1);
    const visible = new Set(main.public.map(frame => frame.type === "system" ? frame.subtype : frame.type));
    for (const type of ["assistant", "user", "stream_event", "result", "task_started", "task_notification", "compact_boundary", "api_retry"]) {
      assert(visible.has(type), type);
    }
    assert(!visible.has("control_request"));
  }
}
