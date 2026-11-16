// A stand-in `codex app-server`, the coding agent the bridge runs over JSON-RPC on its pipes, for
// tests that hold a turn open: run as `<bun> codex-stand-in-source.js <folder>`.
// Its turns: each says in the log which bridge started it (its parent), and one whose words say
// "hold" waits until its `release-<last word>` file is there. Each ends with "Done: <its words>".
export const CODEX_STAND_IN = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const folder = process.argv[2];
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
let buffer = "", thread = 0, turns = 0;
async function turn(threadId, turnId, text) {
  fs.appendFileSync(path.join(folder, "turns.log"), JSON.stringify({ text, bridge: process.ppid }) + "\n");
  send({ method: "turn/started", params: { threadId, turn: { id: turnId } } });
  if (/hold/.test(text)) {
    const release = path.join(folder, "release-" + text.split(" ").pop());
    while (!fs.existsSync(release)) await new Promise(resolve => setTimeout(resolve, 20));
  }
  send({ method: "item/started", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "" } } });
  send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "Done: " + text } } });
  send({ method: "turn/completed", params: { threadId, turn: { id: turnId, status: "completed" } } });
}
process.stdin.on("end", () => process.exit(0));
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.method === "thread/start") send({ id: message.id, result: { thread: { id: "t" + ++thread } } });
    else if (message.method === "thread/resume") send({ id: message.id, result: { thread: { id: message.params.threadId } } });
    else if (message.method === "turn/start") {
      const turnId = "u" + ++turns;
      send({ id: message.id, result: { turn: { id: turnId, status: "inProgress" } } });
      void turn(message.params.threadId, turnId, message.params.input[0].text);
    } else if (message.method === "account/read") send({ id: message.id, result: { account: { type: "apiKey" } } });
    else if (message.method === "model/list") send({ id: message.id, result: { data: [{ id: "gpt-test", model: "gpt-test", displayName: "GPT Test", isDefault: true }] } });
    else if (message.id !== undefined) send({ id: message.id, result: {} });
  }
});
`;

