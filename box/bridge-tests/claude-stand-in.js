// Claude Code, as far as the agents bridge asks it in these tests: signed in (`claude auth status`),
// the models it has, and optional scripted turns. Its
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
export function standInClaude({ root, agents, stands = "opus", signedIn = true, catalog, turns = false }) {
  const bin = path.join(root, ".local", "bin");
  fs.mkdirSync(bin, { recursive: true });
  const named = { opus: "Opus 5.5", sonnet: "Sonnet 5.5" };
  const models = catalog ?? [
    { value: "default", resolvedModel: `claude-${stands}-5-5`, displayName: "Default (recommended)", description: `${named[stands]} · Recommended` },
    { value: "opus", resolvedModel: "claude-opus-5-5", displayName: "Opus 5.5", description: "Opus 5.5 · Most capable" },
    { value: "sonnet", resolvedModel: "claude-sonnet-5-5", displayName: "Sonnet 5.5", description: "Sonnet 5.5 · Everyday" },
  ];
  const program = `const args = process.argv.slice(2);
const fs = require("node:fs");
const signIn = require("node:path").join(process.env.CLAUDE_CONFIG_DIR || process.env.HOME, "fixture-signed-in");
if (args[0] === "auth") {
  if (args[1] === "login") {
    console.log("Open https://claude.example.test/sign-in");
    process.stdout.write("Paste code here if prompted > ");
    require("node:readline").createInterface({ input: process.stdin }).on("line", code => {
      if (code === "fixture-code") {
        fs.writeFileSync(signIn, args[2]);
        process.exit(0);
      }
      process.stdout.write("Invalid code\\nPaste code here if prompted > ");
    });
  } else if (args[1] === "logout") {
    fs.rmSync(signIn, { force: true });
  } else {
    const loggedIn = ${signedIn} || fs.existsSync(signIn);
    console.log(JSON.stringify({ loggedIn, authMethod: loggedIn ? "claude.ai" : "none" }));
    process.exit(loggedIn ? 0 : 1);
  }
  return;
}
if (!args.includes("stream-json")) process.exit(1);
const models = ${JSON.stringify(models)};
const send = message => process.stdout.write(JSON.stringify(message) + "\\n");
const answer = (request_id, said) => send({ type: "control_response", response: { request_id, ...said } });
const native = args.find(arg => arg.startsWith("--session-id="))?.split("=")[1]
  ?? args[args.indexOf("--session-id") + 1];
const scriptedTurns = ${turns};
let held, step = 0;
const textOf = value => value.message.content.find(block => block.type === "text")?.text;
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
  if (message.type === "user" && scriptedTurns) {
    send({ type: "system", subtype: "init", session_id: native });
    const text = textOf(message);
    if (text === "wait for update") {
      held = message.uuid;
      say("Before the switch.");
    } else {
      say("Claude fixture received " + text.split("\\n", 1)[0]);
      finish(message.uuid);
    }
    return;
  }
  if (message.type !== "control_request") return;
  const subtype = message.request?.subtype;
  if (subtype === "initialize") {
    answer(message.request_id, { subtype: "success", pending_permission_requests: [], response: { models, commands: [], agents: [], output_style: "default", available_output_styles: ["default"], account: {} } });
    if (scriptedTurns && held) {
      say("After the switch.");
      finish(held);
      held = undefined;
    }
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
