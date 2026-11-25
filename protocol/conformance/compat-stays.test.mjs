// What a box keeps serving while a Mac from before 1.0 still connects ("stays" in
// protocol/compat-kinds.txt), through either box against the stand-in engine.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

// box/test/found.test.ts's scenarios at the route.
test("0.7's first launch finds an opencode's keys and sign-ins and brings a key into the engine", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("Studio", undefined, folder => ({ KUVO_DEMO_OPENCODE_AUTH: path.join(folder, "opencode-auth.json") }));
  write(here.env.KUVO_DEMO_OPENCODE_AUTH, JSON.stringify({
    openai: { type: "api", key: "sk-fixture", metadata: { org: "fixture" } },
    anthropic: { type: "oauth", refresh: "refresh-fixture", access: "a" },
    "https://example.com/": { type: "wellknown", key: "k", token: "t" },
    broken: "nothing",
  }));
  here.engine.integrations = [
    { id: "anthropic", name: "Anthropic", connections: [{ type: "credential", status: { status: "connected" } }] },
    { id: "openai", name: "OpenAI", connections: [{ type: "env", name: "OPENAI_API_KEY" }] },
    { id: "google", name: "Google", connections: [{ type: "credential", status: { status: "needs_auth" } }] },
  ];
  const ask = here.connect(await here.start().ready());
  assert.ok((await ask("GET", "kuvo/info")).json.features.includes("accounts.found"));
  const found = await ask("GET", "kuvo/accounts/found");
  assert.equal(found.status, 200, found.text);
  const { agents, opencode, shell, connected, unread = [] } = found.json;
  assert.deepEqual(opencode, [
    { id: "opencode:anthropic", integrationID: "anthropic", name: "Anthropic", kind: "signIn", connected: true },
    { id: "opencode:openai", integrationID: "openai", name: "OpenAI", kind: "key", connected: false },
  ]);
  assert.deepEqual(shell, [{ integrationID: "openai", name: "OpenAI", variable: "OPENAI_API_KEY" }]);
  assert.deepEqual(connected, ["anthropic"]);
  assert.equal(unread.includes("engine"), false);
  // The coding agents are read where they run; none read is said, never taken for none signed in.
  assert.ok(Array.isArray(agents) && (!unread.includes("agents") || agents.length === 0), JSON.stringify(found.json));
  for (const agent of agents) assert.deepEqual(Object.keys(agent).filter(key => key !== "label").sort(), ["id", "installed", "name", "signedIn"]);

  const use = id => ask("POST", "kuvo/accounts/found/use", id === undefined ? {} : { id });
  assert.equal((await use()).status, 400);
  assert.deepEqual([(await use("opencode:gone")).status, (await use("opencode:anthropic")).status], [404, 409]);
  assert.equal((await use("opencode:anthropic")).json.message, "This is a sign-in, not a key: sign in to it here instead.");
  const used = await use("opencode:openai");
  assert.equal(used.status, 200, used.text);
  assert.deepEqual(here.engine.credentials, [{ integrationID: "openai", label: "API key", value: { type: "key", key: "sk-fixture", metadata: { org: "fixture" } }, activate: true }]);
  // Brought over already (a second tap): once is enough.
  here.engine.integrations[1].connections.push({ type: "credential", status: { status: "connected" } });
  assert.equal((await use("opencode:openai")).status, 200);
  assert.equal(here.engine.credentials.length, 1);
  // No key leaves the box.
  assert.equal(JSON.stringify((await ask("GET", "kuvo/accounts/found")).json).includes("sk-fixture"), false);
  fs.rmSync(here.env.KUVO_DEMO_OPENCODE_AUTH);
  assert.deepEqual((await ask("GET", "kuvo/accounts/found")).json.opencode, []);
});
