// Search real resident routes over coding agents' conversations (the scripted Grok's, `acp.mjs`),
// with parent and subagent transcripts.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const headers = { "x-kuvo-from": "app" };
const ids = rows => rows.map(row => row.session);
/** A finished step, as an agent's reply keeps it. */
const tool = (id, name, input) => ({ type: "tool", id, name, state: { status: "completed", input, content: [] } });

test("search finds a subagent transcript under its conversation, reuses what it kept and reads a new turn", { timeout: 45000 }, async t => {
  const { computer } = await bench(t, { agents: true });
  const here = await computer("engine-search");
  const top = "ses_search", child = "ses_matchingChild", hidden = "ses_hidden";
  here.agents.seed(top, { title: "Main", directory: here.home });
  here.agents.seed(child, { title: "Child", directory: here.home, parent: top, messages: [{ type: "user", text: "The **Résumé** was refreshed." }] });
  here.agents.seed(hidden, { title: "Internal", directory: here.home, metadata: { kuvo: { engineFor: top } }, messages: [{ type: "user", text: "Résumé in a hidden turn." }] });
  const record = await here.start().ready();
  const route = "kuvo/search?q=resume";
  const result = await request(record, "GET", route, undefined, headers);
  assert.equal(result.status, 200, result.text);
  assert.deepEqual(result.json.results.map(row => [row.session, row.info.id, row.info.title, row.subagent?.id, row.snippet]), [
    [top, top, "Main", child, "The Résumé was refreshed."],
  ]);
  const cached = path.join(here.home, "search", `agents.${child}.json`);
  const kept = fs.readFileSync(cached, "utf8");
  const repeated = await request(record, "GET", route, undefined, headers);
  assert.deepEqual(repeated.json.results.map(row => [row.session, row.subagent?.id, row.snippet]), [[top, child, "The Résumé was refreshed."]]);
  assert.equal(fs.readFileSync(cached, "utf8"), kept);
  // A turn on the conversation since: what it said is read, and found.
  here.agents.script("Look again", [{ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Found the Zircon notes." } } }]);
  const sent = await request(record, "POST", `kuvo/sessions/${top}/send`, { id: "msg_lookAgain", words: "Look again", delivery: "steer" }, headers);
  assert.equal(sent.status, 200, sent.text);
  const zircon = await until(async () => {
    const found = await request(record, "GET", "kuvo/search?q=zircon", undefined, headers);
    return found.json?.results?.length && found.json.results;
  }, "The conversation's new turn was never found.", 20000, 100);
  assert.deepEqual(zircon.map(row => [row.session, row.subagent?.id, row.snippet]), [[top, undefined, "Found the Zircon notes."]]);
  assert.equal(fs.readFileSync(cached, "utf8"), kept);
});

test("search prefers conversation words and forgets a deleted conversation's kept text", { timeout: 45000 }, async t => {
  const { computer } = await bench(t, { agents: true });
  const here = await computer("search-history");
  const at = { directory: here.home };
  here.agents.seed("ses_newest", { ...at, title: "Archived", updated: 9, metadata: { "kuvo.archived": true }, messages: [
    { type: "assistant", content: [
      tool("call_find", "bash", { command: "find Quartz in the build log" }),
      { type: "text", text: "Our Quartz result was accepted." },
    ] },
  ] });
  here.agents.seed("ses_older", { ...at, title: "Open", updated: 2, messages: [
    { type: "assistant", content: [
      tool("call_read", "read", { filePath: path.join(here.home, "Sources", "relevant.swift") }),
      { type: "text", text: "An earlier Quartz result." },
    ] },
  ] });
  here.agents.seed("ses_reviewPart", { ...at, parent: "ses_newest", metadata: { kuvo: { reviewPart: true } }, messages: [{ type: "user", text: "UnlistedReviewText" }] });
  here.agents.seed("ses_seat", { ...at, metadata: { kuvo: { seat: "grp_test" } }, messages: [{ type: "user", text: "A HiddenQuartz result." }] });
  here.agents.seed("ses_turn", { ...at, metadata: { kuvo: { engineFor: "ses_newest" } }, messages: [{ type: "user", text: "A HiddenQuartz result." }] });
  here.agents.seed("ses_seatChild", { ...at, parent: "ses_seat", messages: [{ type: "user", text: "A HiddenQuartz result." }] });
  here.agents.seed("ses_turnChild", { ...at, parent: "ses_turn", messages: [{ type: "user", text: "A HiddenQuartz result." }] });
  here.agents.seed("ses_widget", { ...at, messages: [
    { type: "user", text: "Pick the left one.\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nChooseAmber\n</from-widget>" },
    { type: "user", text: "[Session started] The build began without you.", metadata: { kuvo: { note: { from: "Its sessions" } } } },
  ] });
  const record = await here.start().ready();
  const search = (q, limit) => request(record, "GET", "kuvo/search?" + new URLSearchParams({ q, ...(limit ? { limit } : {}) }), undefined, headers);
  const found = await search("quartz");
  assert.equal(found.status, 200, found.text);
  assert.deepEqual(found.json.results.map(row => [row.session, row.snippet]),
    [["ses_newest", "Our Quartz result was accepted."], ["ses_older", "An earlier Quartz result."]]);
  assert.deepEqual((await search("UnlistedReviewText")).json.results, []);
  for (const hidden of ["HiddenQuartz", "ChooseAmber", "show_widget", "build began"]) {
    assert.deepEqual((await search(hidden)).json.results, [], hidden);
  }
  assert.deepEqual(ids((await search("pick the left one")).json.results), ["ses_widget"]);
  assert.deepEqual(ids((await search("quartz", "1")).json.results), ["ses_newest"]);
  assert.equal((await search("")).status, 400);
  for (const label of ["You", "Agent", "Asked", "Read"]) assert.deepEqual((await search(label)).json.results, [], label);
  assert.deepEqual(ids((await search("relevant.swift")).json.results), ["ses_older"]);
  const cached = path.join(here.home, "search", "agents.ses_newest.json");
  assert.ok(fs.existsSync(cached));
  const deleted = await request(record, "DELETE", "kuvo/agents/api/session/ses_newest", undefined, headers);
  assert.ok(deleted.status < 300, deleted.text);
  assert.deepEqual(ids((await search("quartz")).json.results), ["ses_older"]);
  assert.equal(fs.existsSync(cached), false);
});

test("search reaches the oldest conversation and keeps conversation words ahead of tool history", { timeout: 60000 }, async t => {
  const { computer } = await bench(t, { agents: true });
  const here = await computer("search-paged");
  const id = "ses_page129";
  const sentence = "Step 4 of 5. Change src/timer.js so the countdown pauses when the window loses focus, then run the tests again and tell me what failed.";
  for (let at = 0; at < 129; at++) here.agents.seed("ses_page" + at, { updated: 1000 - at, title: "Page " + at, directory: here.home });
  const tools = Array.from({ length: 50 }, (_, at) => tool(`call_${at}`, "bash", { command: `run ${at} ` + "x".repeat(10_000) }));
  here.agents.seed(id, { updated: 1000 - 129, title: "Page 129", directory: here.home, messages: [
    { type: "assistant", content: [{ type: "text", text: "Deploy to İstanbul, then 한국어 검색 and the café ☕" }] },
    ...Array.from({ length: 445 }, (_, at) => ({ type: "assistant", content: [{ type: "text", text: "Turn " + at }] })),
    { type: "assistant", content: tools },
    { type: "user", text: sentence },
  ] });
  const record = await here.start().ready();
  const search = q => request(record, "GET", "kuvo/search?" + new URLSearchParams({ q }), undefined, headers);
  const found = await search("timer");
  assert.equal(found.status, 200, found.text);
  assert.equal(found.json.results[0].session, id);
  assert.equal(found.json.results[0].info.title, "Page 129");
  const snippet = found.json.results[0].snippet;
  assert.ok(snippet.startsWith("…") && snippet.endsWith("…"), snippet);
  assert.ok(snippet.includes("src/timer.js"));
  for (const word of snippet.replaceAll("…", "").split(" ")) assert.ok(sentence.split(" ").includes(word), word);
  for (const word of ["istanbul", "검색", "CAFE", "☕"]) {
    const result = await search(word);
    assert.deepEqual(ids(result.json.results), [id], word);
  }
  const retained = JSON.parse(fs.readFileSync(path.join(here.home, "search", `agents.${id}.json`), "utf8"));
  assert.ok(retained.text.length <= 400_000);
  // (What was said, oldest first and the latest last, then the steps, in the room it leaves.)
  const said = retained.text.indexOf(sentence);
  assert.ok(said > 0 && retained.text.startsWith("Deploy to İstanbul"), retained.text.slice(0, 300));
  assert.ok(retained.text.indexOf("\n\nrun ") > said, "The tool history came before what was said.");
});

test("setup search merges two residents, keeps the latest duplicate, and reports an unavailable reader", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t, { agents: true });
  const first = await computer("Search Main");
  const second = await computer("Search Member");
  const marker = "SearchFixtureFossil";
  const names = ["ses_searchLocal", "ses_searchRemote", "ses_searchDuplicate"];
  const add = (here, id, updated, text, metadata) =>
    here.agents.seed(id, { directory: here.home, updated, metadata, messages: [{ id: "msg_searchExample", type: "user", text }] });
  add(first, names[0], 7, `Here: ${marker}.`);
  add(first, names[2], 8, `Older: ${marker}.`);
  const main = first.connect(await first.start().ready());
  assert.ok((await main("GET", "kuvo/info")).json.features.includes("search.setup"));
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  const primaryKey = setup.json.you.key;
  const invitation = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invitation.status, 201, invitation.text);
  await until(() => leases.leaseFor(setup.json.setup), "The searching computer did not claim its setup.");
  const joined = await second.join(invitation.json.code);
  assert.equal(joined.code, 0, joined.log);
  // (Its conversations once it joined: a computer with conversations of its own can't join.)
  const large = "different computer ".repeat(4500);
  add(second, names[1], 6, `Remote: ${marker}.`, { kept: large });
  add(second, names[2], 10, `Newer: ${marker}.`);
  const member = second.connect(await second.start().ready());
  const memberSetup = await member("GET", "kuvo/setup");
  assert.equal(memberSetup.status, 200, memberSetup.text);
  const secondKey = memberSetup.json.you.key;
  const route = `kuvo/setup/search?q=${marker}`;
  const firstRead = relay.delivered.length;
  const merged = await until(async () => {
    const response = await main("GET", route);
    return response.json?.results?.length === 3 && response;
  }, "The local and multipart remote search answers did not merge.", 40000, 100);
  assert.deepEqual(merged.json.results.map(row => [row.session, row.computer.id]), [
    [names[2], secondKey], [names[0], primaryKey], [names[1], secondKey],
  ]);
  assert.equal(merged.json.results[0].snippet, `Newer: ${marker}.`);
  assert.equal(merged.json.results[2].info.metadata.kept, large);
  assert.deepEqual(merged.json.awayComputers, []);
  assert.deepEqual(merged.json.unavailableComputers, []);

  assert.deepEqual(ids((await main("GET", route + "&limit=1")).json.results), [names[2]]);
  assert.deepEqual(ids((await main("GET", `kuvo/search?q=${marker}`)).json.results), [names[2], names[0]]);
  assert.equal((await main("GET", "kuvo/setup/search?q=" + "x".repeat(513))).status, 400);
  add(first, names[1], 50, `Stale local copy: ${marker}.`);
  fs.mkdirSync(path.join(first.home, "run/moves"), { recursive: true });
  fs.writeFileSync(path.join(first.home, "run/moves/moved.json"), JSON.stringify([
    { session: names[1], key: secondKey, at: Date.now(), formatVersion: 0 },
  ]));
  const moved = await main("GET", route);
  assert.deepEqual(moved.json.results.map(row => [row.session, row.computer.id]), [
    [names[2], secondKey], [names[0], primaryKey], [names[1], secondKey],
  ]);

  // Only the read's relay frames are failed. An ordinary peer announcement
  // could legitimately change reach and would make this assertion ambiguous.
  const fence = Buffer.from("kuvo-fence:");
  const isFence = bytes => bytes.subarray(0, fence.length).equals(fence);
  const echoes = relay.delivered.slice(firstRead).filter(row => row.from.equals(row.to) && isFence(row.payload));
  const source = echoes[0].from;
  const destination = echoes.find(row => !row.from.equals(source)).from;
  let inRead = false;
  const watchRead = relay.intercept(source, source, ({ payload }) => {
    if (isFence(payload)) inRead = !inRead;
    return false;
  });
  const quietOnly = relay.intercept(source, destination, () => !inRead);
  const reconnect = relay.disconnect(destination);
  const reach = path.join(first.home, "run/peer-reach.json");
  const kept = () => fs.existsSync(reach) ? fs.readFileSync(reach, "utf8") : null;
  const beforeReach = kept();
  try {
    const lost = await main("GET", route);
    assert.deepEqual(lost.json.unavailableComputers.map(computer => computer.id), [secondKey]);
    assert.deepEqual(lost.json.awayComputers, []);
    assert.deepEqual(lost.json.results.map(row => row.computer.id), [primaryKey, primaryKey]);
    assert.equal(kept(), beforeReach);
  } finally {
    watchRead();
    quietOnly();
    reconnect();
  }
  for (const here of [first, second]) {
    const spool = path.join(here.home, "run/work-queue/forward");
    assert.deepEqual(fs.existsSync(spool) ? fs.readdirSync(spool) : [], []);
  }
});
