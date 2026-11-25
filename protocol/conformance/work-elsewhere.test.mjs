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

// Studio (the lead's computer, KUVO_LEAD_BIN) and Lab (the box under test) joined through the
// relay; `seed` gives each its engine's sessions and Lab's files before Lab starts.
async function joined(t, seed) {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio", lead);
  const lab = await computer("Lab");
  studio.engine.sessions.push(sessionOf({ id: "ses_lead", title: "Lead", directory: path.join(studio.home, "..", "work") }));
  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const joinedLab = await lab.join(invite.json.code);
  assert.equal(joinedLab.code, 0, joinedLab.log);
  const studioKey = setup.json.you.key;
  seed({ lab, studioKey });
  const labBox = lab.start();
  await labBox.ready();
  const tool = async (name, args = {}) => {
    const answer = await main("POST", "kuvo/agent-tools", { name, arguments: args, session: "ses_lead" });
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  return { studio, lab, labBox, main, tool, studioKey };
}

test("a lead on another computer lists, stops, summarizes and answers its sessions here, and hears their rows", { timeout: 120000, skip }, async t => {
  const { studio, lab, tool } = await joined(t, ({ lab, studioKey }) => {
    const work = { startedBy: "ses_lead", line: ["ses_lead"], wake: ["finished"], depth: 1 };
    lab.engine.sessions.push({ ...sessionOf({ id: "ses_build", title: "Build", directory: path.join(lab.home, "..", "app"), updated: 20 }), metadata: { kuvo: work } });
    lab.engine.sessions.push({ ...sessionOf({ id: "ses_docs", title: "Docs", directory: path.join(lab.home, "..", "app"), updated: 10 }), metadata: { kuvo: work } });
    lab.engine.sessions.push(sessionOf({ id: "ses_mine", title: "Mine", directory: path.join(lab.home, "..", "app") }));
    // Lab knows the lead lives on Studio, as a start it took from there would have noted.
    write(path.join(lab.home, "run", "work", "homes.json"), JSON.stringify({ ses_lead: { key: studioKey, at: Date.now() } }));
  });
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

test("a lead on another computer starts a session here as if it lived here, then lists, reads and stops it", { timeout: 120000, skip }, async t => {
  const { studio, lab, main, tool, studioKey } = await joined(t, ({ lab }) => {
    const api = path.join(lab.home, "..", "projects", "api");
    fs.mkdirSync(api, { recursive: true });
    write(path.join(lab.home, "box", "projects.json"), JSON.stringify({ projects: [{ directory: api, name: "api" }] }));
    lab.engine.models = [{ id: "m1", providerID: "p", name: "Model One", cost: [{ input: 1, output: 1 }] }];
    lab.engine.providers = [{ id: "p", name: "P" }];
    lab.engine.defaultModel = { providerID: "p", id: "m1" };
  });
  // The lead's computer knows Lab takes starts once Lab has said so.
  let last;
  await until(async () => {
    last = await tool("list_projects", { computer: "Lab" });
    return /^(\[On Lab\] )?Projects on Lab:\n- api: /.test(last.text);
  }, () => `Lab's projects never came: ${JSON.stringify(last)}`, 60000, 500);
  const said = await tool("start_session", { computer: "Lab", project: "api", title: "Sign-in", task: "Fix the sign-in." });
  assert.equal(said.isError, false, said.text);
  const made = lab.engine.sessions.find(each => each.title === "Sign-in");
  assert.ok(made, "made on Lab");
  assert.match(said.text, new RegExp(`Started "Sign-in" in api \\(address ${made.id}\\)`));
  assert.equal(made.location.directory, fs.realpathSync(path.join(lab.home, "..", "projects", "api")));
  assert.equal(made.metadata.kuvo.startedBy, "ses_lead");
  assert.equal(made.metadata.kuvo.lead.box, "Studio");
  assert.equal(made.metadata.kuvo.lead.key, studioKey);
  assert.match(made.metadata.kuvo.call, /^cal_/);
  const told = await until(() => lab.engine.asked.find(each => each.method === "POST" && each.path === `/api/session/${made.id}/prompt`), "Its task never reached it.", 20000, 100);
  assert.match(told.json.text, /^\[Message from [^\]]+ on Studio · ses_lead\]/);
  assert.match(told.json.text, /Fix the sign-in\.$/);
  // Its models, read from here.
  const models = await tool("list_models", { computer: "Lab" });
  assert.equal(models.isError, false, models.text);
  assert.match(models.text, /p\/m1 \(Model One\)/);
  // From here, the lead stops it as one of its own.
  assert.deepEqual(await until(async () => {
    const stopped = await tool("stop_session", { session: made.id });
    return !stopped.isError && stopped;
  }, "The lead never stopped it.", 30000, 500), { text: "Stopped \"Sign-in\".", isError: false });
  assert.ok(lab.engine.interrupted.some(each => each.session === made.id), "stopped on Lab");

  // A model it names that Lab has no account for: refused, saying so; nothing made.
  const before = lab.engine.sessions.length;
  const refused = await tool("start_session", { computer: "Lab", project: "api", title: "Elsewhere", task: "Try.", model: "nowhere/x" });
  assert.equal(refused.isError, true);
  assert.match(refused.text, /There's no account for nowhere on this machine\. Use list_models to see the models you can use\./);
  assert.equal(lab.engine.sessions.length, before);
  // One it names that runs there, with the effort it asks.
  lab.engine.models[0].variants = { high: {} };
  const named = await tool("start_session", { computer: "Lab", project: "api", title: "Named", task: "Go.", model: "p/m1" });
  assert.equal(named.isError, false, named.text);
  assert.match(named.text, /Started "Named" in api \(address ses_\w+\): Model One\./);
  const working = lab.engine.sessions.find(each => each.title === "Named");
  assert.deepEqual(working.model, { providerID: "p", id: "m1" });

  // The lead is a bot's conversation: its Stop All on Studio reaches Lab, where what's under it
  // that's at work stops.
  const bot = path.join(studio.home, "bots", "bot_lead1");
  write(path.join(bot, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_lead1", name: "Lead", job: "", look: { color: "mint", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z", updated: "2026-10-01T09:00:00Z" }));
  write(path.join(bot, "session"), "ses_lead");
  write(path.join(bot, "MEMORY.md"), "# Lead's memory\n");
  fs.mkdirSync(path.join(bot, "wakes"), { recursive: true });
  lab.engine.running.add(working.id);
  const stopAll = await main("POST", "kuvo/bots/bot_lead1/stop-all");
  assert.equal(stopAll.status, 200, stopAll.text);
  await until(() => lab.engine.interrupted.some(each => each.session === working.id), "Stop All never reached Lab.", 30000, 250);
});

test("a stop given while the computer its session is on is away is kept, done once it's back, and its lead hears how it went", { timeout: 300000, skip }, async t => {
  const { studio, lab, labBox, tool } = await joined(t, ({ lab, studioKey }) => {
    const work = { startedBy: "ses_lead", line: ["ses_lead"], depth: 1 };
    lab.engine.sessions.push({ ...sessionOf({ id: "ses_build", title: "Build", directory: path.join(lab.home, "..", "app") }), metadata: { kuvo: work } });
    write(path.join(lab.home, "run", "work", "homes.json"), JSON.stringify({ ses_lead: { key: studioKey, at: Date.now() } }));
  });
  // Once the lead's computer knows the session is on Lab.
  let last;
  await until(async () => {
    last = await tool("list_sessions");
    return /On Lab/.test(last.text);
  }, () => `The lead never listed its sessions on Lab: ${JSON.stringify(last)}`, 60000, 500);
  await labBox.stop();
  const kept = await until(async () => {
    last = await tool("stop_session", { session: "ses_build" });
    return /kept/.test(last.text) && last;
  }, () => `The stop was never kept: ${JSON.stringify(last)}`, 60000, 1000);
  assert.deepEqual(kept, { text: "Lab isn't answering (it may be asleep, off or offline). Your stop is kept and done when it's back: you'll hear how it went as a message.", isError: false });
  lab.engine.running.add("ses_build");
  await lab.start().ready();
  // (A kept call goes again once its computer is heard from: slower on a loaded computer.)
  await until(() => lab.engine.interrupted.some(each => each.session === "ses_build"), "The kept stop never reached Lab.", 120000, 250);
  const told = await until(() => studio.engine.asked.find(each => each.method === "POST" && each.path === "/api/session/ses_lead/prompt" && /Stopped "Build"\./.test(each.json?.text ?? "")),
    "The lead never heard how its stop went.", 60000, 250);
  assert.match(told.json.text, /Stopped "Build"\./);
});

test("a conversation writes to one on another computer and reads its conversation there", { timeout: 180000, skip }, async t => {
  const { lab, tool } = await joined(t, ({ lab }) => {
    lab.engine.sessions.push(sessionOf({ id: "ses_docs", title: "Docs", directory: path.join(lab.home, "..", "app"), updated: 5 }));
    lab.engine.messages.set("ses_docs", [
      { id: "msg_1", type: "user", text: "Write the release notes.", time: { created: 1 } },
      { id: "msg_2", type: "assistant", content: [{ type: "text", text: "The notes are in NOTES.md." }], time: { created: 2 } },
    ]);
  });
  // Once Lab has told the lead's computer who's there.
  let last;
  await until(async () => {
    last = await tool("list_agents");
    return /- Docs \(session, on Lab[^)]*\) address: ses_docs/.test(last.text);
  }, () => `Lab's agents never came: ${JSON.stringify(last)}`, 60000, 500);
  const sent = await tool("send_message", { to: "Docs", text: "Add the new flag." });
  assert.deepEqual(sent, { text: "On its way to Docs on Lab. Its answer, if it writes one, comes to you as a message.", isError: false });
  const told = await until(() => lab.engine.asked.find(each => each.method === "POST" && each.path === "/api/session/ses_docs/prompt"), "The message never reached Docs.", 60000, 250);
  assert.match(told.json.text, /^\[Message from Lead[^\]]* · ses_lead\] \(To answer, use send_message to ses_lead\.\)\n\nAdd the new flag\.$/);
  const read = await tool("read_conversation", { of: "Docs" });
  assert.equal(read.isError, false, read.text);
  assert.match(read.text, /^Docs's conversation \(session, on Lab[^)]*, address ses_docs\), oldest first\./);
  assert.match(read.text, /The notes are in NOTES\.md\./);
});
