// Search real resident routes against a scratch engine with parent and child transcripts.
import assert from "node:assert/strict";
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
