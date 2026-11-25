// Writes a Claude conversation for the differential runner into FOLDER: `seed/` (an offline home
// with one Claude Code session), `claude.json` (the session and what the stand-in plays) and
// `requests.json` (two turns, the transcript after each, and the session's background work).
// Pass IDENTITY_HOME to copy an offline setup identity (its `run/`) into the seed, so setup files
// compare unchanged.
//
//   node protocol/conformance/differential/claude-conversation.mjs FOLDER [IDENTITY_HOME]
//   KUVO_CONFORMANCE_AREA=agents-claude KUVO_DIFFERENTIAL_CLAUDE=FOLDER/claude.json \
//     KUVO_BOX_BIN_A=box/dist/kuvo-link.js KUVO_BOX_BIN_B=box/target/debug/kuvo-box \
//     node protocol/conformance/differential FOLDER/seed FOLDER/requests.json
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { result, streams, toolResult } from "../claude.mjs";

const [folder, identity] = process.argv.slice(2);
assert.ok(folder, "Usage: node claude-conversation.mjs FOLDER [IDENTITY_HOME]");
const seed = path.join(folder, "seed");
fs.mkdirSync(path.join(seed, "box"), { recursive: true, mode: 0o700 });
fs.writeFileSync(path.join(seed, "folder-version.json"), '{"version":23}\n');
fs.writeFileSync(path.join(seed, "box", "opencode.json"), "{}\n");
if (identity) fs.cpSync(path.join(identity, "run"), path.join(seed, "run"), { recursive: true });

const id = "ses_claude_diff_001";
const route = `kuvo/agents/api/session/${id}`;
const headers = { "x-kuvo-from": "app" };
const script = {
  "go one": [
    ...streams("$UUID", [{ type: "tool_use", id: "toolu_1", name: "Read", input: { file_path: "a.ts" } }], { input: 3000, read: 40000, write: 1000, output: 120 }),
    toolResult("toolu_1", "const a = 1"),
    ...streams("$UUID", [{ type: "text", text: "It sets a to 1." }], { input: 200, read: 44000, write: 300, output: 400 }),
    result(["$UUID"]),
  ],
  "and two": [
    ...streams("$UUID", [{ type: "text", text: "Still 1." }], { input: 100, read: 45000, write: 0, output: 50 }),
    result(["$UUID"]),
  ],
};
fs.writeFileSync(path.join(folder, "claude.json"), JSON.stringify({ sessions: { [id]: {} }, script }, null, 1));
fs.writeFileSync(path.join(folder, "requests.json"), JSON.stringify([
  { method: "POST", path: `${route}/prompt`, body: { id: "msg_claude_diff_001_a", text: "go one" }, headers },
  { method: "GET", path: `${route}/message?order=asc`, headers, idles: 1 },
  { method: "POST", path: `${route}/prompt`, body: { id: "msg_claude_diff_001_b", text: "and two" }, headers },
  { method: "GET", path: `${route}/message?order=asc`, headers, idles: 2 },
  { method: "GET", path: "kuvo/work/background", headers },
  { method: "GET", path: `kuvo/work/background/${id}`, headers },
], null, 1));
