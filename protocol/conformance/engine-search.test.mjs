// Search real resident routes against a scratch engine with parent and child transcripts.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("search finds a subagent transcript under its conversation and reuses the indexed revision", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-search");
  const top = sessionOf({ id: "ses_search", title: "Main", directory: here.home });
  const child = sessionOf({ id: "ses_matchingChild", title: "Child", directory: here.home, parent: top.id });
  const hidden = { ...sessionOf({ id: "ses_hidden", title: "Internal", directory: here.home }),
    metadata: { kuvo: { engineFor: top.id } } };
  here.engine.sessions.push(top, child, hidden);
  here.engine.messages.set(child.id, [{ id: "msg_match", type: "user", text: "The **Résumé** was refreshed." }]);
  here.engine.messages.set(hidden.id, [{ id: "msg_hidden", type: "user", text: "Résumé in a hidden turn." }]);
  const record = await here.start().ready();
  const route = "kuvo/search?q=resume";
  const result = await request(record, "GET", route);
  assert.equal(result.status, 200, result.text);
  assert.deepEqual(result.json, { results: [
    { session: top.id, info: top, subagent: child, snippet: "The Résumé was refreshed." },
  ] });
  const reads = here.engine.asked.filter(call => call.path.endsWith("/message")).length;
  const repeated = await request(record, "GET", route);
  assert.deepEqual(repeated.json, result.json);
  assert.equal(here.engine.asked.filter(call => call.path.endsWith("/message")).length, reads);
  child.time.updated++;
  here.engine.messages.set(child.id, [{ id: "msg_replace", type: "user", text: "Another topic." }]);
  assert.deepEqual((await request(record, "GET", route)).json, { results: [] });
});

test("search prefers conversation words and keeps its cache when the engine cannot list sessions", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("search-history");
  const newest = sessionOf({ id: "ses_newest", directory: here.home, title: "Archived" });
  newest.time.updated = 9;
  newest.metadata = { "kuvo.archived": true };
  const older = sessionOf({ id: "ses_older", directory: here.home, title: "Open" });
  const detail = sessionOf({ id: "ses_reviewPart", directory: here.home, parent: newest.id });
  detail.metadata = { kuvo: { reviewPart: true } };
  const seat = sessionOf({ id: "ses_seat", directory: here.home });
  seat.metadata = { kuvo: { seat: "grp_test" } };
  const turn = sessionOf({ id: "ses_turn", directory: here.home });
  turn.metadata = { kuvo: { engineFor: newest.id } };
  const seatChild = sessionOf({ id: "ses_seatChild", parent: seat.id, directory: here.home });
  const turnChild = sessionOf({ id: "ses_turnChild", parent: turn.id, directory: here.home });
  const widget = sessionOf({ id: "ses_widget", directory: here.home });
  here.engine.sessions.push(newest, older, detail, seat, turn, seatChild, turnChild, widget);
  here.engine.messages.set(newest.id, [
    { type: "assistant", content: [
      { type: "tool", state: { input: { command: "find Quartz in the build log" } } },
      { type: "text", text: "Our Quartz result was accepted." },
    ] },
  ]);
  here.engine.messages.set(older.id, [
    { type: "user", text: "An earlier Quartz result." },
  ]);
  here.engine.messages.set(widget.id, [
    { type: "user", text: "Pick the left one.\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nChooseAmber\n</from-widget>" },
    { type: "user", text: "[Session started] The build began without you.", metadata: { kuvo: { note: { from: "Its sessions" } } } },
  ]);
  here.engine.messages.set(detail.id, [{ type: "user", text: "UnlistedReviewText" }]);
  for (const session of [seat, turn, seatChild, turnChild]) {
    here.engine.messages.set(session.id, [{ type: "user", text: "A HiddenQuartz result." }]);
  }
  const record = await here.start().ready();
  const search = (q, limit) => request(record, "GET", "kuvo/search?" + new URLSearchParams({ q, ...(limit ? { limit } : {}) }));
  const found = await search("quartz");
  assert.equal(found.status, 200, found.text);
  assert.deepEqual(found.json.results.map(row => [row.session, row.snippet]),
    [[newest.id, "Our Quartz result was accepted."], [older.id, "An earlier Quartz result."]]);
  assert.deepEqual((await search("UnlistedReviewText")).json, { results: [] });
  for (const hidden of ["HiddenQuartz", "ChooseAmber", "show_widget", "build began"]) {
    assert.deepEqual((await search(hidden)).json.results, [], hidden);
  }
  assert.deepEqual((await search("pick the left one")).json.results.map(row => row.session), [widget.id]);
  assert.deepEqual((await search("quartz", "1")).json.results.map(row => row.session), [newest.id]);
  assert.equal((await search("")).status, 400);
  here.engine.messages.set(older.id, [
    { type: "assistant", content: [
      { type: "tool", name: "read", state: { input: { filePath: path.join(here.home, "Sources", "relevant.swift") } } },
      { type: "text", text: "An earlier Quartz result." },
    ] },
  ]);
  older.time.updated++;
  for (const label of ["You", "Agent", "Asked", "Read"]) assert.deepEqual((await search(label)).json.results, [], label);
  const fileMatch = await search("relevant.swift");
  assert.deepEqual(fileMatch.json.results.map(row => row.session), [older.id]);
  const cached = path.join(here.home, "search", `engine.${newest.id}.json`);
  const original = fs.readFileSync(cached, "utf8");
  here.engine.sessionListUnavailable = true;
  assert.equal((await search("quartz")).status, 503);
  assert.equal(fs.readFileSync(cached, "utf8"), original);
  here.engine.sessionListUnavailable = false;
  here.engine.sessions.splice(here.engine.sessions.indexOf(newest), 1);
  assert.deepEqual((await search("quartz")).json.results.map(row => row.session), [older.id]);
  assert.equal(fs.existsSync(cached), false);
});

test("search reaches the oldest page and keeps conversation words ahead of tool history", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("search-paged");
  const id = "ses_page129";
  const sentence = "Step 4 of 5. Change src/timer.js so the countdown pauses when the window loses focus, then run the tests again and tell me what failed.";
  here.engine.paged = true;
  for (let at = 0; at < 130; at++) {
    here.engine.sessions.push(sessionOf({ id: "ses_page" + at, updated: 1000 - at, title: "Page " + at, directory: here.home }));
  }
  const tools = Array.from({ length: 50 }, (_, at) => ({
    type: "tool", state: { input: { command: `run ${at} ` + "x".repeat(10_000) } },
  }));
  here.engine.messages.set(id, [
    { type: "assistant", content: [{ type: "text", text: "Deploy to İstanbul, then 한국어 검색 and the café ☕" }] },
    ...Array.from({ length: 445 }, (_, at) => ({ type: "assistant", content: [{ type: "text", text: "Turn " + at }] })),
    { type: "assistant", content: tools },
    { type: "user", text: sentence },
  ]);
  const record = await here.start().ready();
  const search = q => request(record, "GET", "kuvo/search?" + new URLSearchParams({ q }));
  const found = await search("timer");
  assert.equal(found.status, 200, found.text);
  assert.equal(found.json.results[0].session, id);
  assert.equal(found.json.results[0].info.title, "Page 129");
  const snippet = found.json.results[0].snippet;
  assert.ok(snippet.startsWith("…") && snippet.endsWith("…"));
  assert.ok(snippet.includes("src/timer.js"));
  for (const word of snippet.replaceAll("…", "").split(" ")) assert.ok(sentence.split(" ").includes(word), word);
  for (const word of ["istanbul", "검색", "CAFE", "☕"]) {
    const result = await search(word);
    assert.deepEqual(result.json.results.map(row => row.session), [id], word);
  }
  const file = path.join(here.home, "search", `engine.${id}.json`);
  const retained = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.ok(retained.text.length <= 400_000);
  assert.ok(retained.text.startsWith(sentence));
  assert.equal(here.engine.asked.filter(call => call.path.endsWith(`/${id}/message`)).length, 3);
});

test("setup search merges two residents, keeps the latest duplicate, and reports an unavailable reader", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const first = await computer("Search Main");
  const second = await computer("Search Member");
  const marker = "SearchFixtureFossil";
  const names = ["ses_searchLocal", "ses_searchRemote", "ses_searchDuplicate"];
  const add = (here, id, updated, text, metadata) => {
    here.engine.sessions.push({ ...sessionOf({ id, directory: here.home, updated }), ...(metadata ? { metadata } : {}) });
    here.engine.messages.set(id, [{ id: "msg_searchExample", type: "user", text }]);
  };
  add(first, names[0], 7, `Here: ${marker}.`);
  add(first, names[2], 8, `Older: ${marker}.`);
  const large = "different computer ".repeat(4500);
  add(second, names[1], 6, `Remote: ${marker}.`, { kept: large });
  add(second, names[2], 10, `Newer: ${marker}.`);
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

  assert.deepEqual((await main("GET", route + "&limit=1")).json.results.map(row => row.session), [names[2]]);
  assert.deepEqual((await main("GET", `kuvo/search?q=${marker}`)).json.results.map(row => row.session), [names[2], names[0]]);
  assert.equal((await main("GET", "kuvo/setup/search?q=" + "x".repeat(513))).status, 400);
  add(first, names[1], 50, `Stale local copy: ${marker}.`);
  write(path.join(first.home, "run/moves/moved.json"), JSON.stringify([
    { session: names[1], key: secondKey, at: Date.now(), formatVersion: 0 },
  ]));
  const moved = await main("GET", route);
  assert.deepEqual(moved.json.results.map(row => [row.session, row.computer.id]), [
    [names[2], secondKey], [names[0], primaryKey], [names[1], secondKey],
  ]);
  second.engine.sessionListUnavailable = true;
  const partial = await main("GET", route);
  assert.deepEqual(partial.json.results.map(row => row.computer.id), [primaryKey, primaryKey]);
  assert.deepEqual(partial.json.unavailableComputers.map(computer => computer.id), [secondKey]);
  assert.deepEqual(partial.json.awayComputers, []);

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
