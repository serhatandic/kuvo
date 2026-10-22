// A machine's bots and wake-ups, answered by its bridge from their folders as the Mac answers for
// them (PROTOCOL.md, `kuvo/bots` and `kuvo/wakes`). Run with
// `node --test Packages/KuvoLink/Tests/Bridge/*.test.js`.
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { after, beforeEach, test } from "node:test";

// The bridge reads Kuvo Code's folder from KUVO_HOME: one of this test's own.
const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-bots-"));
process.env.KUVO_HOME = home;
after(() => fs.rmSync(home, { recursive: true, force: true }));
const link = await import("../../Sources/KuvoLink/Resources/kuvo-link.js");

const bots = path.join(home, "bots");
const write = (file, text) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
};

// A bot's folder as the app leaves it on a machine: its description, session and memory.
function bot(id, { name = "Scout", session = `ses_${id}`, created = "2026-10-01T09:00:00Z", kuvo = 1, described = true } = {}) {
  const folder = path.join(bots, id);
  fs.mkdirSync(path.join(folder, "wakes"), { recursive: true });
  write(path.join(folder, "MEMORY.md"), `# ${name}'s memory\n`);
  if (session) write(path.join(folder, "session"), session);
  if (described) {
    write(path.join(folder, "bot.json"), JSON.stringify({
      kuvo, id, name, job: "Watches the build", look: { color: "sky", shape: "round", face: "calm" },
      model: { id: "big-pickle", providerID: "opencode" }, created, updated: created,
    }));
  }
  return folder;
}

beforeEach(() => {
  fs.rmSync(bots, { recursive: true, force: true });
  fs.rmSync(path.join(home, "wakes"), { recursive: true, force: true });
  fs.rmSync(path.join(home, "run"), { recursive: true, force: true });
});

// What the handler answers a phone's request, whole.
async function ask(handle, method, p, body) {
  const parts = [];
  for await (const part of handle({ method, path: p, headers: {}, body: body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0) }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  let json;
  try { json = JSON.parse(text); } catch {}
  return { status: parts[0].head.status, json, text };
}

// An engine (or the agents' bridge) on a free port: its tools connected, prompts taken.
async function fakeEngine() {
  const asked = [];
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      asked.push({ method: request.method, url: request.url, auth: request.headers.authorization, directory: request.headers["x-opencode-directory"], body });
      response.writeHead(200, { "content-type": "application/json" });
      response.end(request.url === "/api/mcp" ? JSON.stringify({ browser: { status: { status: "connected" } } }) : "{}");
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { asked, port: server.address().port, close: () => { server.closeAllConnections(); server.close(); } };
}

test("wake-ups read from their names as the app reads them", () => {
  assert.deepEqual(link.wakeOf("1792000000-AbC123", "note"), { id: "1792000000-AbC123", due: 1792000000, note: "note", every: null, letters: "AbC123" });
  assert.equal(link.wakeOf("1792000000-AbC123-1d@0900").every, "1d");
  assert.equal(link.wakeOf("1792000000-AbC123-1w@Mon0900").every, "1w");
  assert.equal(link.wakeOf("1792000000-AbC123-weekdays@0900").every, "weekdays");
  assert.equal(link.wakeOf("1792000000-AbC123-30m").every, "30m");
  // Not a repeat kuvo-wake makes: no repeat at all, as the app reads it.
  for (const text of ["4m", "08h", "0d", "53w", "1y", "1234567m"]) assert.equal(link.repeatText(text), null, text);
  for (const name of [".firing", "notes.txt", "-AbC", ""]) assert.equal(link.wakeOf(name), null, name);
});

test("kuvo/bots lists the machine's described bots, each as its folder is now", async () => {
  const scout = bot("bot_scout00001", { created: "2026-10-02T09:00:00Z" });
  bot("bot_first00001", { name: "First", created: "2026-10-01T09:00:00Z" });
  bot("bot_noword0001", { described: false });
  bot("bot_newer00001", { kuvo: 2 });
  bot("bot_nosession1", { session: "" });
  write(path.join(bots, "notabot", "bot.json"), "{}");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "later");
  write(path.join(scout, "wakes", "1792000100-XyZ789-1d@0900"), "every morning");
  write(path.join(scout, "watches", "Qq1", "check"), "curl -s example.com");
  write(path.join(scout, "watches", "Qq2", "every"), "30m");
  write(path.join(scout, "paused"), "paused");
  write(path.join(scout, "via"), "bridge");

  const handle = link.engineHandler(() => { throw new Error("not needed"); }, { name: "my-server" });
  const answer = await ask(handle, "GET", "kuvo/bots");
  assert.equal(answer.status, 200);
  assert.deepEqual(answer.json.bots.map(each => each.name), ["First", "Scout"], "described ones, in the order they were made");
  assert.deepEqual(answer.json.bots[1], {
    id: "bot_scout00001", session: "ses_bot_scout00001", name: "Scout", look: { color: "sky", shape: "round", face: "calm" }, machine: "my-server",
    job: "Watches the build", paused: true, wakes: 1792000100, watching: 1, agents: true,
  });
  assert.equal("wakes" in answer.json.bots[0], false, "none waiting: left out, as the Mac leaves it out");
  // Made from the Mac, which keeps their list.
  for (const [method, route] of [["POST", "kuvo/bots"], ["GET", "kuvo/bots/templates"], ["POST", "kuvo/groups"]]) {
    const refused = await ask(handle, method, route);
    assert.equal(refused.status, 409, route);
    assert.equal(refused.json.message, "Make it from your Mac; it can live here.");
  }
  assert.equal((await ask(handle, "PUT", "kuvo/bots")).status, 405);
});

test("kuvo/wakes reads a bot's or a session's wake-ups, watches and pause", async () => {
  const scout = bot("bot_scout00001");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "check the\nbuild");
  write(path.join(scout, "wakes", "1792000100-XyZ789-1d@0900"), "every morning");
  write(path.join(scout, "watches", "Qq1", "check"), "curl -s example.com");
  write(path.join(scout, "watches", "Qq1", "every"), "30m\n");
  write(path.join(scout, "watches", "Qq1", "note"), "Tell me when it's up");
  const session = path.join(home, "wakes", "ses_plain");
  write(path.join(session, "session"), "ses_plain");
  write(path.join(session, "wakes", "1792000900-Plain1"), "a session's own");
  write(path.join(session, "paused"), "archived");

  const handle = link.engineHandler(() => ({}), { name: "my-server" });
  const theBot = await ask(handle, "GET", "kuvo/wakes/ses_bot_scout00001");
  assert.equal(theBot.status, 200);
  assert.deepEqual(theBot.json, {
    wakes: [
      { id: "1792000100-XyZ789-1d@0900", due: 1792000100, note: "every morning", every: "1d" },
      { id: "1792000500-AbC123", due: 1792000500, note: "check the build" },
    ],
    watches: [{ letters: "Qq1", every: "30m", check: "curl -s example.com", note: "Tell me when it's up" }],
    paused: false,
  });
  const own = await ask(handle, "GET", "kuvo/wakes/ses_plain");
  assert.deepEqual(own.json, { wakes: [{ id: "1792000900-Plain1", due: 1792000900, note: "a session's own" }], watches: [], paused: true });
  // One that keeps none, and one that can't name a folder: nothing waiting.
  assert.deepEqual((await ask(handle, "GET", "kuvo/wakes/ses_other")).json, { wakes: [], watches: [], paused: false });
  assert.deepEqual((await ask(handle, "GET", "kuvo/wakes/..%2F..%2Fetc")).json, { wakes: [], watches: [], paused: false });

  // All at once, for the marks beside them: each session's soonest and whether it watches; bots' come with kuvo/bots.
  const watching = path.join(home, "wakes", "ses_watch");
  write(path.join(watching, "session"), "ses_watch");
  write(path.join(watching, "watches", "Ww2", "check"), "test -f done");
  write(path.join(path.join(home, "wakes", "ses_none"), "session"), "ses_none");
  const all = await ask(handle, "GET", "kuvo/wakes");
  assert.equal(all.status, 200);
  assert.deepEqual(all.json.sessions.sort((a, b) => a.session.localeCompare(b.session)), [
    { session: "ses_plain", next: 1792000900, watching: false },
    { session: "ses_watch", watching: true },
  ]);
  assert.equal((await ask(handle, "POST", "kuvo/wakes")).status, 405);
});

test("a wake-up or watch is cancelled by its letters; nothing else goes", async () => {
  const scout = bot("bot_scout00001");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "once");
  write(path.join(scout, "wakes", "1792000100-XyZ789-1d@0900"), "every morning");
  write(path.join(scout, "watches", "Qq1", "check"), "true");
  const handle = link.engineHandler(() => ({}), {});

  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/cancel", { wake: "XyZ789" })).status, 200);
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), ["1792000500-AbC123"]);
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/cancel", { wake: "1792000500-AbC123" })).status, 200);
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), []);
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/cancel", { watch: "Qq1" })).status, 200);
  assert.equal(fs.existsSync(path.join(scout, "watches", "Qq1")), false);
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/cancel", { wake: "gone00" })).status, 404);
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/cancel", { watch: "../../.." })).status, 404);
  assert.equal((await ask(handle, "GET", "kuvo/wakes/ses_bot_scout00001/cancel")).status, 405);
  assert.ok(fs.existsSync(path.join(scout, "MEMORY.md")), "its folder stays");
});

test("Wake Up Now sends the whole note to its session, then a one-off is done and a repeating one stays", async t => {
  const engine = await fakeEngine();
  t.after(engine.close);
  const scout = bot("bot_scout00001");
  write(path.join(scout, "directory"), "/home/me/project one");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "check the\nbuild");
  write(path.join(scout, "wakes", "1792000100-XyZ789-1d@0900"), "every morning");
  const handle = link.engineHandler(() => ({ pid: process.pid, port: engine.port, password: "s3cret" }), {});

  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "AbC123" })).status, 200);
  const sent = engine.asked.find(each => each.url === "/api/session/ses_bot_scout00001/prompt");
  assert.ok(sent);
  assert.equal(sent.auth, `Basic ${Buffer.from("opencode:s3cret").toString("base64")}`);
  const body = JSON.parse(sent.body);
  assert.equal(body.text, "[Wake-up] check the\nbuild", "the whole note, not the line it's listed by");
  assert.equal(body.delivery, "queue");
  assert.match(body.id, /^msg_[0-9a-f]{12}[0-9A-Za-z]{14}$/);
  // Its folder's tools first.
  const tools = engine.asked.find(each => each.url === "/api/mcp");
  assert.equal(decodeURIComponent(tools.directory), "/home/me/project one");
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), ["1792000100-XyZ789-1d@0900"]);

  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "XyZ789" })).status, 200);
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), ["1792000100-XyZ789-1d@0900"], "a repeating one comes back at its time");
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "AbC123" })).status, 404, "a one-off is done");
});

test("Wake Up Now for one that runs with the coding agents goes to their bridge; with nothing running, it says so", async t => {
  const bridge = await fakeEngine();
  t.after(bridge.close);
  const scout = bot("bot_scout00001");
  write(path.join(scout, "via"), "bridge");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "once");
  const handle = link.engineHandler(() => { throw new Error("no engine"); }, {});

  const down = await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "AbC123" });
  assert.equal(down.status, 503);
  assert.match(down.json.message, /coding agents/);
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), ["1792000500-AbC123"], "still waiting");

  write(path.join(home, "run", "bridge.json"), JSON.stringify({ pid: process.pid, port: bridge.port, password: "agents" }));
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "AbC123" })).status, 200);
  assert.ok(bridge.asked.some(each => each.url === "/api/session/ses_bot_scout00001/prompt" && each.auth === `Basic ${Buffer.from("opencode:agents").toString("base64")}`));
  assert.equal(bridge.asked.some(each => each.url === "/api/mcp"), false, "their tools aren't the engine's to wait for");
});

test("a bot on its way to another computer isn't woken or changed from a phone until it's there", async t => {
  const engine = await fakeEngine();
  t.after(engine.close);
  const scout = bot("bot_scout00001");
  write(path.join(scout, "wakes", "1792000500-AbC123"), "check the build");
  // What a move writes while it holds the bot's wake-ups.
  write(path.join(scout, "paused"), "moving");
  const handle = link.engineHandler(() => ({ pid: process.pid, port: engine.port, password: "s3cret" }), {});

  for (const action of ["now", "cancel"]) {
    const answered = await ask(handle, "POST", `kuvo/wakes/ses_bot_scout00001/${action}`, { wake: "AbC123" });
    assert.equal(answered.status, 409, action);
    assert.match(answered.json.message, /moving/i, action);
  }
  assert.equal(engine.asked.some(each => each.url.endsWith("/prompt")), false, "nothing sent to a conversation on its way");
  assert.deepEqual(fs.readdirSync(path.join(scout, "wakes")), ["1792000500-AbC123"], "nothing taken away");

  // Paused by you instead: Wake Up Now still sends it, as on the Mac.
  write(path.join(scout, "paused"), "");
  assert.equal((await ask(handle, "POST", "kuvo/wakes/ses_bot_scout00001/now", { wake: "AbC123" })).status, 200);
});
