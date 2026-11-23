// Search real resident routes against a scratch engine with parent and child transcripts.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request } from "./adapter.mjs";
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
  here.engine.sessions.push(newest, older, detail);
  here.engine.messages.set(newest.id, [
    { type: "assistant", content: [
      { type: "tool", state: { input: { command: "find Quartz in the build log" } } },
      { type: "text", text: "Our Quartz result was accepted." },
    ] },
  ]);
  here.engine.messages.set(older.id, [
    { type: "user", text: "An earlier Quartz result." },
  ]);
  here.engine.messages.set(detail.id, [{ type: "user", text: "UnlistedReviewText" }]);
  const record = await here.start().ready();
  const search = (q, limit) => request(record, "GET", "kuvo/search?" + new URLSearchParams({ q, ...(limit ? { limit } : {}) }));
  const found = await search("quartz");
  assert.equal(found.status, 200, found.text);
  assert.deepEqual(found.json.results.map(row => [row.session, row.snippet]),
    [[newest.id, "Our Quartz result was accepted."], [older.id, "An earlier Quartz result."]]);
  assert.deepEqual((await search("UnlistedReviewText")).json, { results: [] });
  assert.deepEqual((await search("quartz", "1")).json.results.map(row => row.session), [newest.id]);
  assert.equal((await search("")).status, 400);
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
