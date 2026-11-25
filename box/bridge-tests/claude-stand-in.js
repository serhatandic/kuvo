// Claude Code, as far as the agents bridge asks it in these tests: signed in (`claude auth status`),
// the models it has, and optional scripted turns. With `turns`, a message whose first line names a
// script in `<root>/claude-script.json` plays that script (see `play` below); every line the stand-in
// reads, and the arguments it started with, go to `<root>/claude-heard.jsonl`. Its
// `default` is the model it recommends on that account (`stands`: Opus on one plan, Sonnet on
// another). The bridge is given the SDK as the box's own tests have it (`bun install` in
// box, before these tests, as CI does), so nothing is downloaded, and the stand-in comes
// first on its PATH, so your own Claude Code is never run.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const boxModules = fileURLToPath(new URL("../node_modules", import.meta.url));

/** The stand-in in `root`'s `.local/bin`, and the SDK in the bridge's `agents` folder: the folder to
 * put first on the bridge's PATH. */
export function standInClaude({ root, agents, stands = "opus", signedIn = true, catalog, turns = false, authMethod = "claude.ai" }) {
  const bin = path.join(root, ".local", "bin");
  fs.mkdirSync(bin, { recursive: true });
  const named = { opus: "Opus 5.5", sonnet: "Sonnet 5.5" };
  const models = catalog ?? [
    { value: "default", resolvedModel: `claude-${stands}-5-5`, displayName: "Default (recommended)", description: `${named[stands]} · Recommended` },
    { value: "opus", resolvedModel: "claude-opus-5-5", displayName: "Opus 5.5", description: "Opus 5.5 · Most capable" },
    { value: "sonnet", resolvedModel: "claude-sonnet-5-5", displayName: "Sonnet 5.5", description: "Sonnet 5.5 · Everyday" },
  ];
  if (signedIn) seedClaudeAuth(root, undefined, authMethod);
  const program = `const args = process.argv.slice(2);
const fs = require("node:fs");
const paths = require("node:path");
const recordHome = process.env.CLAUDE_CONFIG_DIR || process.env.HOME;
const credentialHome = process.env.CLAUDE_CONFIG_DIR || paths.join(process.env.HOME, ".claude");
const signIn = paths.join(recordHome, "fixture-signed-in");
const credential = paths.join(credentialHome, ".credentials.json");
const record = paths.join(recordHome, ".claude.json");
fs.appendFileSync(paths.join(${JSON.stringify(root)}, "claude-processes.jsonl"), JSON.stringify({pid:process.pid,args}) + String.fromCharCode(10));
if (args[0] === "auth") {
  if (args[1] === "login") {
    console.log("Open https://claude.example.test/sign-in");
    process.stdout.write("Paste code here if prompted > ");
    require("node:readline").createInterface({ input: process.stdin }).on("line", code => {
      if (code === "fixture-code") {
        fs.writeFileSync(signIn, args[2]);
        fs.mkdirSync(credentialHome,{recursive:true});
        fs.writeFileSync(credential, JSON.stringify({claudeAiOauth:{accessToken:"fixture",refreshToken:"fixture"}}));
        fs.writeFileSync(record, JSON.stringify({oauthAccount:{accountUuid:"fixture-"+Date.now(),organizationType:args[2]==="--console"?"api":"claude_max"}}));
        process.exit(0);
      }
      process.stdout.write("Invalid code\\nPaste code here if prompted > ");
    });
  } else if (args[1] === "logout") {
    fs.rmSync(signIn, { force: true });
    fs.rmSync(credential, { force: true });
    fs.rmSync(record, { force: true });
  } else {
    const loggedIn = ${signedIn} || fs.existsSync(signIn);
    console.log(JSON.stringify({ loggedIn, authMethod: loggedIn ? ${JSON.stringify(authMethod)} : "none" }));
    process.exit(loggedIn ? 0 : 1);
  }
  return;
}
if (!args.includes("stream-json")) process.exit(1);
const models = ${JSON.stringify(models)};
const send = message => process.stdout.write(JSON.stringify(message) + "\\n");
const answer = (request_id, said) => send({ type: "control_response", response: { request_id, ...said } });
const flag = name => args.find(arg => arg.startsWith(name + "="))?.slice(name.length + 1)
  ?? (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const resumed = flag("--resume");
const native = resumed ?? flag("--session-id");
const scriptedTurns = ${turns};
const scriptFile = ${JSON.stringify(path.join(root, "claude-script.json"))};
const heard = ${JSON.stringify(path.join(root, "claude-heard.jsonl"))};
const hear = value => { if (scriptedTurns) fs.appendFileSync(heard, JSON.stringify({ pid: process.pid, ...value }) + "\\n"); };
// Its arguments, and what the box set for it (a secret's only that it's set).
hear({ argv: args, cwd: process.cwd(), env: Object.fromEntries(Object.entries(process.env)
  .filter(([name]) => /^(CLAUDE_|DISABLE_|ANTHROPIC_|KUVO_)/.test(name))
  .map(([name, value]) => [name, /TOKEN|KEY|SECRET|PASSWORD/.test(name) ? "<set>" : value])) });
// Something it leaves running that holds its error output open: it says \`stderr\` once
// \`until\` (a file beside the script) is there, or \`after\` ms, then says it's done in the heard file.
function linger(spec) {
  const pathOf = require("node:path");
  const go = spec.until ? pathOf.join(${JSON.stringify(root)}, spec.until) : null;
  const script = pathOf.join(${JSON.stringify(root)}, "claude-linger.cjs");
  fs.writeFileSync(script, "const fs = require('node:fs'); const spec = JSON.parse(process.argv[2]);" +
    "const say = () => fs.write(2, spec.stderr || '', () => { fs.appendFileSync(spec.heard, JSON.stringify({ lingered: spec.stderr || '' }) + String.fromCharCode(10)); process.exit(0); });" +
    // (A case that ends before it's told to go leaves it no reason to stay: it goes within a minute.)
    "const begun = Date.now();" +
    "const wait = () => (!spec.go || fs.existsSync(spec.go)) ? setTimeout(say, spec.after || 0) : Date.now() - begun > 60000 ? process.exit(0) : setTimeout(wait, 20);" +
    "wait();");
  // (A Windows process outlives the one that started it without being detached, which would cut it
  // off from the error output it's to hold.)
  require("node:child_process").spawn(process.execPath, [script, JSON.stringify({ ...spec, go, heard })],
    { stdio: ["ignore", "ignore", "inherit"], detached: process.platform !== "win32", windowsHide: true }).unref();
}
// How the next one for a conversation starts, when the test says (\`<root>/claude-start.json\`, taken once):
// once \`until\` is there, leaving \`linger\` running, saying \`stderr\` and stopping with \`exit\`, before
// it reads anything.
const startFile = ${JSON.stringify(path.join(root, "claude-start.json"))};
if (scriptedTurns && native && fs.existsSync(startFile)) {
  const start = JSON.parse(fs.readFileSync(startFile, "utf8"));
  fs.rmSync(startFile);
  const go = start.until && require("node:path").join(${JSON.stringify(root)}, start.until);
  while (go && !fs.existsSync(go)) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20);
  if (start.linger) linger(start.linger);
  if (start.stderr !== undefined) fs.writeSync(2, start.stderr);
  process.exit(start.exit ?? 1);
}
let held, step = 0, playing = false, hookID, chosen;
const waiting = [], queued = [];
const waitFor = test => new Promise(resolve => waiting.push({ test, resolve }));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
// A script is a list of steps: a frame to send (with $UUID, $UUID2… for the messages taken and
// $SESSION for the conversation, $HOOK for the box's PreToolUse hook), or {wait: "<control subtype>" | "user"}, {reply: "<request id>",
// allow: [...], deny: [...]} (waits for the box's answer to a request the script sent, then plays the
// branch for it), {until: "<file>"} (until the test makes that file beside the script), {sleep: ms},
// {stderr: "..."}, {exit: code} and {linger: {stderr, until, after}} (see linger).
async function play(steps, vars) {
  for (const step of steps) {
    if (step.wait === "user") {
      const message = queued.shift() ?? await waitFor(message => message.type === "user");
      vars["UUID" + (Object.keys(vars).filter(name => name.startsWith("UUID")).length + 1)] = message.uuid;
    } else if (step.wait) {
      await waitFor(message => message.type === "control_request" && message.request?.subtype === step.wait);
    } else if (step.reply) {
      const answer = await waitFor(message => message.type === "control_response" && message.response?.request_id === step.reply);
      const behavior = answer.response?.response?.behavior;
      await play((behavior === "allow" ? step.allow : step.deny) ?? [], vars);
    } else if (step.sleep) {
      await delay(step.sleep);
    } else if (step.until) {
      // Until the test says go: a file of that name beside the script.
      while (!fs.existsSync(require("node:path").join(require("node:path").dirname(scriptFile), step.until))) await delay(20);
    } else if (step.linger) {
      linger(step.linger);
    } else if (step.type === undefined && (step.stderr !== undefined || step.exit !== undefined)) {
      if (step.stderr !== undefined) await new Promise(done => process.stderr.write(step.stderr, done));
      if (step.exit !== undefined) process.exit(step.exit);
    } else {
      send(JSON.parse(JSON.stringify(step).replace(/\\$(UUID\\d*|SESSION|HOOK)/g, (_, name) => name === "SESSION" ? native : name === "HOOK" ? hookID : vars[name])));
    }
  }
}
function script(text) {
  if (!fs.existsSync(scriptFile)) return;
  // By its first line, or another (a note the box put ahead of your words).
  const scripts = JSON.parse(fs.readFileSync(scriptFile, "utf8"));
  const line = text.split("\\n").find(each => Object.hasOwn(scripts, each));
  return line === undefined ? undefined : scripts[line];
}
async function begin(message) {
  playing = true;
  send({ type: "system", subtype: "init", session_id: native });
  const text = textOf(message) ?? "";
  const echo = [
    { type: "assistant", parent_tool_use_id: null, session_id: native, message: { id: "echo-" + ++step, role: "assistant", content: [{ type: "text", text: "Claude fixture received " + text }] } },
    { type: "result", subtype: "success", is_error: false, result: "", session_id: native, user_message_uuids: ["$UUID"] },
  ];
  try { await play(script(text) ?? echo, { UUID: message.uuid }); }
  finally { playing = false; }
  if (queued.length) void begin(queued.shift());
}
const textOf = value => typeof value.message?.content === "string" ? value.message.content : value.message?.content?.find?.(block => block.type === "text")?.text;
const say = text => send({
  type: "assistant", parent_tool_use_id: null, session_id: native,
  message: { id: "step-" + ++step, role: "assistant", model: "opus", content: [{ type: "text", text }] },
});
const finish = (uuid, interrupted = false) => send({
  type: "result", subtype: interrupted ? "error_during_execution" : "success",
  is_error: interrupted, session_id: native, user_message_uuids: [uuid],
});
require("node:readline").createInterface({ input: process.stdin }).on("line", line => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  hear(message);
  const waiter = waiting.findIndex(each => each.test(message));
  if (waiter >= 0 && message.type !== "control_request") return waiting.splice(waiter, 1)[0].resolve(message);
  if (waiter >= 0) waiting.splice(waiter, 1)[0].resolve(message);
  if (message.type === "user" && scriptedTurns && (playing || (textOf(message) !== undefined && script(textOf(message))))) {
    if (playing) queued.push(message);
    else void begin(message);
    return;
  }
  if (message.type === "user" && scriptedTurns) {
    send({ type: "system", subtype: "init", session_id: native });
    const text = textOf(message);
    if (text === "wait for update") {
      held = message.uuid;
      say("Before the switch.");
    } else {
      say("Claude fixture received " + text.split("\\n", 1)[0] + (resumed ? " in " + native : ""));
      finish(message.uuid);
    }
    return;
  }
  if (message.type !== "control_request") return;
  const subtype = message.request?.subtype;
  if (subtype === "initialize") {
    // Its PreToolUse hook, as the box registered it ($HOOK in a script).
    hookID = message.request?.hooks?.PreToolUse?.[0]?.hookCallbackIds?.[0] ?? hookID;
    // A probe for its models (no conversation), as the test says (\`<root>/claude-catalog.json\`):
    // those \`models\`, once \`until\` is there (it says it's waiting, then that it answered).
    const catalogFile = ${JSON.stringify(path.join(root, "claude-catalog.json"))};
    const catalog = !native && fs.existsSync(catalogFile) ? JSON.parse(fs.readFileSync(catalogFile, "utf8")) : undefined;
    if (catalog) {
      const mark = name => fs.writeFileSync(${JSON.stringify(root)} + require("node:path").sep + name, String(process.pid));
      const reply = () => {
        answer(message.request_id, { subtype: "success", pending_permission_requests: [], response: { models: catalog.models, commands: [], agents: [], output_style: "default", available_output_styles: ["default"], account: {} } });
        if (catalog.until) mark("claude-catalog-answered");
      };
      if (!catalog.until) return reply();
      mark("claude-catalog-waiting");
      const go = ${JSON.stringify(root)} + require("node:path").sep + catalog.until;
      const wait = () => fs.existsSync(go) ? reply() : setTimeout(wait, 20);
      return wait();
    }
    answer(message.request_id, { subtype: "success", pending_permission_requests: [], response: { models, commands: [], agents: [], output_style: "default", available_output_styles: ["default"], account: {} } });
    if (scriptedTurns && held) {
      say("After the switch.");
      finish(held);
      held = undefined;
    }
  } else if (subtype === "get_usage") {
    const usage = ${JSON.stringify(path.join(root, "claude-usage.json"))};
    if (fs.existsSync(usage)) answer(message.request_id, { subtype: "success", response: JSON.parse(fs.readFileSync(usage, "utf8")) });
    else answer(message.request_id, { subtype: "error", error: "No fixture usage." });
  } else if (!native && ["set_model", "get_context_usage"].includes(subtype)) {
    // A probe asking how much each model holds: as the test says (\`windows\` in
    // \`<root>/claude-catalog.json\`, by model), or, as an older Claude Code, it doesn't say.
    const catalogFile = ${JSON.stringify(path.join(root, "claude-catalog.json"))};
    const windows = (fs.existsSync(catalogFile) ? JSON.parse(fs.readFileSync(catalogFile, "utf8")) : {}).windows;
    if (subtype === "set_model") chosen = message.request.model;
    if (!windows) answer(message.request_id, { subtype: "error", error: "Not in this stand-in." });
    else if (subtype === "set_model") answer(message.request_id, { subtype: "success", response: {} });
    else answer(message.request_id, { subtype: "success", response: { maxTokens: windows[chosen] } });
  } else if (scriptedTurns && subtype === "get_task_output") {
    // What a command it left running printed so far, as the test says (\`<root>/claude-task-output.json\`).
    const outputs = ${JSON.stringify(path.join(root, "claude-task-output.json"))};
    const output = (fs.existsSync(outputs) ? JSON.parse(fs.readFileSync(outputs, "utf8")) : {})[message.request.task_id] ?? "";
    answer(message.request_id, { subtype: "success", response: { output, total_bytes: Buffer.byteLength(output) } });
  } else if (scriptedTurns && subtype === "stop_task") {
    // Stopped: it says so, and that nothing's left going.
    answer(message.request_id, { subtype: "success", response: {} });
    send({ type: "system", subtype: "task_notification", task_id: message.request.task_id, status: "stopped", summary: "stopped", session_id: native });
    send({ type: "system", subtype: "background_tasks_changed", tasks: [], session_id: native });
  } else if (scriptedTurns && ["set_model", "set_permission_mode", "apply_flag_settings", "interrupt"].includes(subtype)) {
    answer(message.request_id, { subtype: "success", response: {} });
    if (held && subtype === "interrupt") { finish(held, true); held = undefined; }
  } else {
    answer(message.request_id, { subtype: "error", error: "Not in this stand-in." });
  }
}).on("close", () => process.exit(0));
`;
  if (process.platform === "win32") {
    fs.writeFileSync(path.join(bin, "claude.cjs"), program);
    fs.writeFileSync(path.join(bin, "claude.cmd"), `@"${process.execPath}" "%~dp0claude.cjs" %*\r\n`);
  } else {
    fs.writeFileSync(path.join(bin, "claude"), `#!${process.execPath}\n${program}`, { mode: 0o755 });
  }
  if (!agents) return bin;
  const runtime = path.join(agents, "runtime", "claude");
  fs.mkdirSync(runtime, { recursive: true });
  if (!fs.existsSync(path.join(runtime, "node_modules"))) fs.symlinkSync(boxModules, path.join(runtime, "node_modules"));
  return bin;
}

/** Native sign-in metadata for the stand-in only; never use real credentials. */
export function seedClaudeAuth(root, config, method = "claude.ai") {
  const credential = config ?? path.join(root, ".claude");
  const record = config ?? root;
  fs.mkdirSync(credential, { recursive: true });
  fs.writeFileSync(path.join(credential,".credentials.json"), JSON.stringify({claudeAiOauth:{accessToken:"fixture",refreshToken:"fixture"}}));
  const content=JSON.stringify({oauthAccount:{accountUuid:"fixture",organizationType:method==="claude.ai"?"claude_max":"api"}});
  fs.writeFileSync(path.join(record,".claude.json"), content);
  if (!config) fs.writeFileSync(path.join(credential,".claude.json"), content);
}
