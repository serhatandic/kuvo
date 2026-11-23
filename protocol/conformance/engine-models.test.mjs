// Read OpenCode model readiness through a real resident with a local engine fixture.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const reference = (providerID, id) => ({ providerID, id });
const session = (here, id, model) => ({ ...sessionOf({ id, directory: here.home }), ...(model && { model }) });

test("the engine's catalog distinguishes a working model, an account key and no default", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-models");
  const free = reference("opencode", "free");
  const needsKey = reference("openai", "example");
  here.engine.providers.push({ id: "opencode" });
  here.engine.models.push({ ...free, name: "Free" }, { ...needsKey, name: "Example" });
  here.engine.integrations.push({
    id: "openai", name: "OpenAI", connections: [],
    methods: [{ type: "oauth", id: "chatgpt-device" }, { type: "key", id: "api" }],
  });
  here.engine.sessions.push(session(here, "ses_free", free), session(here, "ses_key", needsKey), session(here, "ses_choose"));
  const record = await here.start().ready();
  const response = await request(record, "GET", "kuvo/sessions/models?sessions=ses_free,ses_key,ses_choose,ses_missing");
  assert.equal(response.status, 200, response.text);
  assert.deepEqual(response.json.sessions, {
    ses_free: { model: free, name: "Free", runs: true },
    ses_key: { model: needsKey, name: "Example", runs: false, missing: "key", account: { id: "openai", name: "OpenAI" },
      plan: { name: "ChatGPT", agent: "Codex", provider: "codex-cli" },
      refusal: "Example needs an OpenAI key: add one, or choose another model (with a ChatGPT plan, one under Codex).",
      action: { do: "addKey", label: "Add Key", help: "Add an OpenAI key (with a ChatGPT plan, choose a model under Codex instead)" } },
    ses_choose: { model: null, runs: false, missing: "model", choose: true, refusal: "Choose a model to send it.",
      action: { do: "choose", label: "Choose a Model", help: "Choose a model" } },
  });
});

test("a model removed from a catalog keeps the name the box last knew", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-name");
  const known = reference("local", "one");
  const other = reference("local", "two");
  here.engine.providers.push({ id: "local" });
  here.engine.models.push({ ...known, name: "Model One" }, { ...other, name: "Model Two" });
  here.engine.sessions.push(session(here, "ses_named", known));
  const first = here.start();
  const route = "kuvo/sessions/models?sessions=ses_named";
  const before = await request(await first.ready(), "GET", route);
  assert.deepEqual(before.json.sessions.ses_named, { model: known, name: "Model One", runs: true });
  await until(() => fs.existsSync(path.join(here.home, "box/model-names.json")), "the name to be retained");
  await first.stop();
  here.engine.models.shift();
  const after = await request(await here.start().ready(), "GET", route);
  assert.deepEqual(after.json.sessions.ses_named, {
    model: known, runs: false, missing: "gone", choose: true,
    refusal: "Model One is no longer available: choose another model.",
    action: { do: "choose", label: "Choose a Model", help: "Choose another model" },
  });
});
