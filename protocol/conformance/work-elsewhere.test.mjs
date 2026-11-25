// A lead on one computer runs its sessions on another (PROTOCOL.md, "Peers", `call`): it lists them,
// stops one and has one summarized, and the computer they're on tells it their rows. The box under
// test (KUVO_BOX_BIN) holds the sessions; the lead's computer runs KUVO_LEAD_BIN, the TypeScript
// box by default, whose agents' tools make the calls.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { boxBinary, readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const leadFile = process.env.KUVO_LEAD_BIN || new URL("../../box/dist/kuvo-link.js", import.meta.url).pathname;
const lead = fs.existsSync(leadFile) ? boxBinary(leadFile) : null;
// (The TypeScript box doesn't run on native Windows: a lead there is the Rust box's, once it has
// its tools.)
const skip = !lead ? "no lead box (KUVO_LEAD_BIN)" : lead.typescript && process.platform === "win32" ? "the TypeScript box doesn't run on native Windows" : false;

test("a lead on another computer lists, stops, summarizes and answers its sessions here, and hears their rows", { timeout: 120000, skip }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio", lead);
  const lab = await computer("Lab");
  studio.engine.sessions.push(sessionOf({ id: "ses_lead", title: "Lead", directory: path.join(studio.home, "..", "work") }));
  const work = { startedBy: "ses_lead", line: ["ses_lead"], wake: ["finished"], depth: 1 };
  lab.engine.sessions.push({ ...sessionOf({ id: "ses_build", title: "Build", directory: path.join(lab.home, "..", "app"), updated: 20 }), metadata: { kuvo: work } });
  lab.engine.sessions.push({ ...sessionOf({ id: "ses_docs", title: "Docs", directory: path.join(lab.home, "..", "app"), updated: 10 }), metadata: { kuvo: work } });
  lab.engine.sessions.push(sessionOf({ id: "ses_mine", title: "Mine", directory: path.join(lab.home, "..", "app") }));

  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  // Lab knows the lead lives on Studio, as a start it took from there would have noted.
  const studioKey = setup.json.you.key;
  write(path.join(lab.home, "run", "work", "homes.json"), JSON.stringify({ ses_lead: { key: studioKey, at: Date.now() } }));
  await lab.start().ready();
  const tool = async (name, args = {}) => {
    const answer = await main("POST", "kuvo/agent-tools", { name, arguments: args, session: "ses_lead" });
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  // The lead's computer knows Lab takes work calls once Lab has said so.
  let last;
  const listed = await until(async () => {
    last = await tool("list_sessions");
    return /On Lab/.test(last.text) && last;
  }, () => `The lead never listed its sessions on Lab: ${JSON.stringify(last)}`, 60000, 500);
  assert.equal(listed.isError, false, listed.text);
  assert.match(listed.text, /On Lab, the latest first:\n- Build \(address ses_build\), in app: (idle|finished)[^\n]*\n- Docs \(address ses_docs\)/);
  assert.doesNotMatch(listed.text, /ses_mine/, "only the lead's own sessions");

  // Their rows reach the lead's computer, which then knows where each is.
  const rowsAt = () => {
    const folder = path.join(studio.home, "run", "work", "elsewhere");
    for (const name of fs.existsSync(folder) ? fs.readdirSync(folder) : []) {
      const kept = readJSON(path.join(folder, name));
      if (kept?.leads?.ses_lead?.some(row => row.session === "ses_build")) return kept.leads.ses_lead;
    }
    return null;
  };
  const rows = await until(rowsAt, "Lab never told the lead's computer its rows.", 60000, 250);
  assert.deepEqual(rows.map(row => [row.session, row.title, row.startedBy]), [["ses_build", "Build", "ses_lead"], ["ses_docs", "Docs", "ses_lead"]]);

  assert.deepEqual(await tool("stop_session", { session: "ses_build" }), { text: "Stopped \"Build\".", isError: false });
  assert.ok(lab.engine.interrupted.some(each => each.session === "ses_build"), "stopped on Lab");
  const compacted = await tool("compact_session", { session: "ses_docs" });
  assert.equal(compacted.isError, false, compacted.text);
  assert.match(compacted.text, /Summarizing "Docs" now\./);
  assert.ok(lab.engine.asked.some(each => each.method === "POST" && each.path === "/api/session/ses_docs/compact"), "summarized on Lab");

  // What its sessions there ask, answered from here: within what the lead could do itself.
  lab.engine.permissions.set("ses_docs", [
    { id: "per_push", sessionID: "ses_docs", action: "shell", resources: ["git push"] },
    { id: "per_status", sessionID: "ses_docs", action: "shell", resources: ["git status"] },
  ]);
  lab.engine.forms.set("ses_docs", [{ id: "frm_color", sessionID: "ses_docs", title: "Colors", fields: [{ key: "color", type: "string", title: "Which color?" }] }]);
  lab.engine.permissions.set("ses_mine", [{ id: "per_mine", sessionID: "ses_mine", action: "shell", resources: ["ls"] }]);
  const replied = path => lab.engine.asked.findLast(each => each.method === "POST" && each.path === path)?.json;
  assert.deepEqual(await tool("answer_ask", { session: "ses_docs", ask: "per_push", decision: "deny", reason: "Not now." }),
    { text: "Denied: git push. \"Docs\" is told why.", isError: false });
  assert.equal(replied("/api/session/ses_docs/permission/per_push/reply")?.decision, "reject");
  assert.match(replied("/api/session/ses_docs/permission/per_push/reply")?.message ?? "", /: Not now\.$/);
  // (On Auto with no sandbox running there, it asks first for now: the user's to allow, not the lead's.)
  const allowed = await tool("answer_ask", { session: "ses_docs", ask: "per_status", decision: "allow", reason: "Just looks." });
  assert.equal(allowed.isError, true);
  // (Why Auto can't be had there is that computer's: no sandbox running, or not yet on Windows.)
  assert.match(allowed.text, /^You can't allow that: "Docs" asks first for now \([^)]+\), so the user answers every ask it makes, whatever mode you're in\. The user sees the ask on "Docs" and answers it there\. Tell them in a line why it's needed if it matters, and carry on with the rest\. You can still deny it\.$/);
  assert.equal(replied("/api/session/ses_docs/permission/per_status/reply"), undefined, "nothing answered");
  assert.deepEqual(await tool("answer_ask", { session: "ses_docs", ask: "frm_color", answer: "Blue", reason: "The brand's." }),
    { text: "Answered \"Docs\".", isError: false });
  assert.deepEqual(replied("/api/session/ses_docs/form/frm_color/reply"), { answer: { color: "Blue" } });
  const kept = lab.engine.sessions.find(each => each.id === "ses_docs").metadata.kuvo.answers;
  assert.deepEqual(kept.map(each => [each.ask, each.decision]), [["per_push", "deny"], ["frm_color", "answer"]]);
  const notMine = await tool("answer_ask", { session: "ses_mine", ask: "per_mine", decision: "allow", reason: "Fine." });
  assert.equal(notMine.isError, true);
  assert.match(notMine.text, /"Mine" isn't one of your sessions: only the user answers what it asks\./);
});
