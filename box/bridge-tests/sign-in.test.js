// Signing in to an account (PROTOCOL.md, "Signing in"), as the box answers every client: the ways
// to sign in, in its order and words (`kuvo/sign-in/methods`), and how a started sign-in is going,
// with what to say once it didn't work (`kuvo/sign-in/status`), from the engine or, for a coding
// agent's account, the agents' bridge. The real box code; the engine and the bridge are stand-ins.
//
// Run with `node --test box/bridge-tests/sign-in.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { runningAs } from "./running-as.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-sign-in-"));
process.env.KUVO_HOME = home;
removeAtEnd(home);
const link = await import(LINK_BUNDLE.href);

// A stand-in on a free port: `routes(url)` gives its `data` as their APIs do, `{status, message}`
// for a refusal, or nothing (404). Every path asked, kept.
const asked = [];
async function standIn(routes) {
  const server = http.createServer((request, response) => {
    asked.push(request.url);
    const value = routes(request.url);
    const refusal = value?.refused;
    response.writeHead(value === undefined ? 404 : refusal ? refusal.status : 200, { "content-type": "application/json" });
    response.end(JSON.stringify(value === undefined ? { message: "Not found." } : refusal ? { message: refusal.message } : { location: null, data: value }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  atEnd(() => { server.closeAllConnections(); server.close(); });
  return { port: server.address().port, password: "p" };
}

// The engine: OpenAI (its plan goes through Codex, and it keeps two legacy ways), Bedrock (AWS
// credentials already on the computer, or a key), and sign-ins that ended each way.
const attempts = {
  att_pending: { status: "pending", message: "Downloading… 40%", url: "https://example.com/later" },
  att_done: { status: "complete" },
  att_denied: { status: "failed", message: "access_denied" },
  att_said: { status: "failed", message: "The page said no." },
  att_late: { status: "expired" },
};
const engine = await standIn(url => {
  if (url === "/api/integration") return [
    { id: "openai", name: "OpenAI", methods: [{ type: "key" }, { type: "env", names: ["OPENAI_API_KEY"] }, { type: "oauth", id: "chatgpt-browser", label: "Codex browser (legacy)" }, { type: "oauth", id: "chatgpt-token-sharing", label: "Sign in with ChatGPT" }], connections: [] },
    { id: "amazon-bedrock", name: "Amazon Bedrock", methods: [{ type: "key", label: "Bedrock API key" }, { type: "external", id: "aws", label: "AWS credentials on this computer", form: [{ key: "region", type: "text", label: "Region" }] }, { type: "env", names: ["AWS_PROFILE"] }], connections: [] },
    { id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth", id: "device", label: "Sign in with GitHub" }, { type: "command", id: "gh", label: "Use gh" }], connections: [] },
  ];
  const attempt = url.match(/^\/api\/integration\/github-copilot\/connect\/oauth\/([^/?]+)$/)?.[1];
  // Signed in on its page between the first ask and the second.
  if (attempt === "att_soon") return asked.filter(each => each === url).length >= 2 ? { status: "complete" } : { status: "pending" };
  if (attempt === "att_broken") return { refused: { status: 500, message: "The engine fell over." } };
  if (attempt) return attempts[decodeURIComponent(attempt)];
  return undefined;
});
// The coding agents' bridge: Claude Code, signed in with its own command or a key.
const bridge = await standIn(url => {
  if (url === "/api/integration") return [{ id: "claude-code", name: "Claude Code", methods: [{ type: "key", label: "Anthropic API key" }, { type: "oauth", id: "subscription", label: "Claude Pro or Max" }], connections: [] }];
  if (url === "/api/integration/claude-code/connect/oauth/att_agent_pending") return { status: "pending", userCode: "AB12-CD34" };
  if (url === "/api/integration/claude-code/connect/oauth/att_agent") return { status: "failed", message: "server_error", userCode: "AB12-CD34" };
  return undefined;
});
const agents = await runningAs("kuvo-agents", { after: atEnd });
const handlerOn = (bridgeRecord) => link.engineHandler(() => ({ pid: process.pid, ...engine, version: "2.0.32" }), { name: "Studio" }, bridgeRecord);
const handle = handlerOn(() => ({ pid: agents, ...bridge }));

async function answer(method, route, on = handle) {
  const parts = [];
  for await (const part of on({ method, path: route, headers: {}, body: Buffer.from("") }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  return { status: parts[0].head.status, json: text ? JSON.parse(text) : undefined };
}

test("an engine provider's ways in: an account's sign-in first, then a key, legacy ways last and marked; a plan another agent signs in to left out; its environment variables", async () => {
  // OpenAI's ChatGPT plan goes through Codex: only the key, and its variable.
  const openai = await answer("GET", "kuvo/sign-in/methods?integration=openai");
  assert.equal(openai.status, 200);
  assert.deepEqual(openai.json, { methods: [{ type: "key", label: "Use an API key" }], environment: ["OPENAI_API_KEY"], hint: "API key" });

  const bedrock = await answer("GET", "kuvo/sign-in/methods?integration=amazon-bedrock");
  assert.deepEqual(bedrock.json, {
    methods: [
      { type: "external", id: "aws", label: "AWS credentials on this computer", form: [{ key: "region", type: "text", label: "Region" }] },
      { type: "key", label: "Bedrock API key" },
    ],
    environment: ["AWS_PROFILE"],
    hint: "Connect",
  });

  // A way no client can start (a command) isn't offered.
  const copilot = await answer("GET", "kuvo/sign-in/methods?integration=github-copilot");
  assert.deepEqual(copilot.json.methods, [{ type: "oauth", id: "device", label: "Sign in with GitHub" }]);
});

test("a coding agent's account: its ways from the agents' bridge", async () => {
  const claude = await answer("GET", "kuvo/sign-in/methods?integration=claude-code&agents=1");
  assert.equal(claude.status, 200);
  assert.deepEqual(claude.json, { methods: [{ type: "oauth", id: "subscription", label: "Claude Pro or Max" }, { type: "key", label: "Anthropic API key" }], environment: [], hint: "Claude Pro or Max" });
  // The engine doesn't have it.
  assert.equal((await answer("GET", "kuvo/sign-in/methods?integration=claude-code")).status, 404);
});

test("every account's ways in, for Accounts' Sign In buttons and the way in each card names", async () => {
  const engine = await answer("GET", "kuvo/sign-in/accounts");
  assert.equal(engine.status, 200);
  assert.deepEqual(engine.json.accounts.map(({ id, hint, methods }) => ({ id, hint, ways: methods.length })), [
    // OpenAI offers only its key here: its plan goes through Codex.
    { id: "openai", hint: "API key", ways: 1 },
    { id: "amazon-bedrock", hint: "Connect", ways: 2 },
    { id: "github-copilot", hint: "Sign in with GitHub", ways: 1 },
  ]);
  // Amazon Bedrock is connected with keys or the computer's credentials, never signed in to: its
  // button says Connect, and its sheet is titled by the box with its own name (#734). A key-only
  // account says Add Key, never Sign In.
  const actions = Object.fromEntries(engine.json.accounts.map(account => [account.id, account.actions]));
  assert.deepEqual(actions["amazon-bedrock"], [{ id: "signIn", label: "Connect", sheetTitle: "Connect · Amazon Bedrock" }]);
  assert.deepEqual(actions["openai"], [{ id: "signIn", label: "Add Key", sheetTitle: "Add Key · OpenAI" }]);
  const agents = await answer("GET", "kuvo/sign-in/accounts?agents=1");
  assert.deepEqual(agents.json.accounts.map(({ id, hint }) => ({ id, hint })), [{ id: "claude-code", hint: "Claude Pro or Max" }]);
  assert.equal((await answer("GET", "kuvo/sign-in/accounts?agents=1", handlerOn(() => null))).status, 503);
});

test("how each account stands and what can be done with it, in the box's words: a sign-in the box saw end says so, with Sign In Again, until it's signed in again", async () => {
  // Not signed in: its line names its way in, and Sign In. A coding agent's, with its sign-in's
  // page coming back to its computer: Sign In only there.
  const before = await answer("GET", "kuvo/sign-in/accounts");
  assert.deepEqual(before.json.accounts.map(({ id, state, line, actions, connections }) => ({ id, state, line, actions, connections })).at(-1), {
    id: "github-copilot", state: "signedOut", line: "Sign in with GitHub", actions: [{ id: "signIn", label: "Sign In", sheetTitle: "Sign In to GitHub Copilot" }], connections: [],
  });
  const agents = await answer("GET", "kuvo/sign-in/accounts?agents=1");
  assert.deepEqual(agents.json.accounts.map(({ state, line, actions }) => ({ state, line, actions })), [{ state: "signedOut", line: "Not signed in", actions: [{ id: "signIn", label: "Sign In", sheetTitle: "Sign In to Claude Code", here: true }] }]);

  // Signed in with two accounts, the first in use.
  const signedIn = await standIn(url => {
    if (url === "/api/integration") return [{
      id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth", id: "device", label: "Sign in with GitHub" }],
      connections: [{ type: "credential", id: "cred_1", label: "you@example.com", method: "oauth" }, { type: "credential", id: "cred_2", label: "work@example.com", method: "oauth" }],
    }];
    if (url === "/api/provider") return [];
    if (url === "/api/session/ses_1") return { id: "ses_1", model: { providerID: "github-copilot", id: "gpt-5" } };
    return undefined;
  });
  const on = link.engineHandler(() => ({ pid: process.pid, ...signedIn, version: "2.0.32" }), { name: "Studio" }, () => null);
  const copilot = async () => (await answer("GET", "kuvo/sign-in/accounts", on)).json.accounts[0];
  const displayName = (await answer("GET", "kuvo/computers", on)).json.computers.find(computer => computer.here).name;
  const disconnect = { id: "disconnect", label: "Disconnect", confirm: { title: "Disconnect GitHub Copilot?", message: `Its sign-ins and keys go from ${displayName}, and its models leave the model picker there. Sessions on its models stop until you sign in again.`, label: "Disconnect" } };
  const another = { id: "addAnother", label: "Add Another Account", sheetTitle: "Sign In to GitHub Copilot" };
  const again = { id: "signInAgain", label: "Sign In Again", sheetTitle: "Sign In to GitHub Copilot" };
  let said = await copilot();
  assert.deepEqual({ state: said.state, line: said.line, actions: said.actions, connections: said.connections }, {
    state: "signedIn", line: "Signed in as you@example.com · 2 accounts", actions: [again, another, disconnect],
    connections: [{ id: "cred_1", state: "signedIn", line: "you@example.com" }, { id: "cred_2", state: "signedIn", line: "work@example.com" }],
  });

  // A run on it is refused, and it couldn't be renewed: its sign-in ended, as the box keeps it.
  await link.noteSignIn({ type: "session.execution.failed", data: { sessionID: "ses_1", error: { type: "provider.auth", message: "Request failed" } } }, signedIn);
  said = await copilot();
  assert.deepEqual({ state: said.state, line: said.line, actions: said.actions, connections: said.connections.map(each => each.state) }, {
    state: "ended", line: "Sign-in ended", actions: [again, another, disconnect], connections: ["ended", "signedIn"],
  });
  // The same as `kuvo/accounts/ended` says.
  assert.deepEqual((await answer("GET", "kuvo/accounts/ended", on)).json, { providers: ["github-copilot"] });

  // A step on its model ends well: it works after all.
  await link.noteSignIn({ type: "session.step.ended", data: { sessionID: "ses_1" } }, signedIn);
  assert.equal((await copilot()).state, "signedIn");
});

test("how a sign-in is going, as the engine says, with what to say once it ended without signing in", async () => {
  const status = async attempt => (await answer("GET", `kuvo/sign-in/status?integration=github-copilot&attempt=${attempt}`)).json;
  // Still going: as the engine says it, the page that came later too.
  assert.deepEqual(await status("att_pending"), { status: "pending", message: "Downloading… 40%", url: "https://example.com/later" });
  assert.deepEqual(await status("att_done"), { status: "complete" });
  // Both message fields carry the box's display-safe sign-in words.
  assert.deepEqual(await status("att_denied"), { status: "failed", message: "The sign-in was turned down on its page. Try again to sign in.", failure: "The sign-in was turned down on its page. Try again to sign in." });
  assert.deepEqual(await status("att_said"), { status: "failed", message: "Signing in didn’t work.", failure: "Signing in didn’t work." });
  assert.deepEqual(await status("att_late"), { status: "expired", message: "The sign-in timed out. Try again.", failure: "The sign-in timed out. Try again." });
  // A device code is shown for a pending attempt. Once it ends, the safe failure is all that's shown.
  const pendingAgent = await answer("GET", "kuvo/sign-in/status?integration=claude-code&attempt=att_agent_pending&agents=1");
  assert.deepEqual(pendingAgent.json, { status: "pending", userCode: "AB12-CD34" });
  const agent = await answer("GET", "kuvo/sign-in/status?integration=claude-code&attempt=att_agent&agents=1");
  assert.deepEqual(agent.json, { status: "failed", message: "The service couldn’t sign you in just now. Try again in a moment.", failure: "The service couldn’t sign you in just now. Try again in a moment." });
});

test("asked to wait for a change (a Mac asking a machine's box over SSH): answered once the sign-in ends, not before", async () => {
  const before = asked.length;
  const status = await answer("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_soon&wait=20");
  assert.deepEqual(status, { status: 200, json: { status: "complete" } });
  assert.equal(asked.slice(before).filter(each => each.endsWith("/att_soon")).length, 2);
  // One that ended already is answered at once, however long it may wait.
  assert.deepEqual((await answer("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_denied&wait=20")).json.failure, "The sign-in was turned down on its page. Try again to sign in.");
});

test("a sign-in the engine doesn't know (it started over), or can't say: its status and words, for the client to count", async () => {
  const unknown = await answer("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_gone");
  assert.equal(unknown.status, 404);
  const broken = await answer("GET", "kuvo/sign-in/status?integration=github-copilot&attempt=att_broken");
  assert.deepEqual(broken, { status: 500, json: { message: "The sign-in status couldn’t be read. Try again." } });
});

test("asked of the coding agents while their bridge isn't running: why not", async () => {
  const down = handlerOn(() => null);
  const methods = await answer("GET", "kuvo/sign-in/methods?integration=claude-code&agents=1", down);
  assert.equal(methods.status, 503);
  // (A box with no bridge's program: said so, in the box's one sentence for it.)
  assert.equal(methods.json.message, "The coding agents aren't installed here yet. Kuvo's next update brings them.");
  assert.equal(methods.json.reason, "agentsDown");
  assert.equal((await answer("GET", "kuvo/sign-in/status?integration=claude-code&attempt=att_agent&agents=1", down)).status, 503);
});

test("asked without saying which, or to change something: refused", async () => {
  assert.equal((await answer("GET", "kuvo/sign-in/methods")).status, 400);
  assert.equal((await answer("GET", "kuvo/sign-in/status?integration=github-copilot")).status, 400);
  assert.equal((await answer("POST", "kuvo/sign-in/methods?integration=openai")).status, 405);
});

test("kuvo/info says the box answers them", async () => {
  const info = await answer("GET", "kuvo/info");
  assert.ok(info.json.features.includes("sign-in.methods"));
  assert.ok(info.json.features.includes("sign-in.accounts.state"));
});

test("adding an agent account forwards provider and returns the separate provider ID", async () => {
  let given;
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    given = { method: request.method, path: request.url, body: JSON.parse(Buffer.concat(chunks).toString()) };
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ data: { providerID: "codex-cli@1234abcd" } }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const on = handlerOn(() => ({ pid: agents, port: server.address().port, password: "p" }));
    const pieces = [];
    for await (const piece of on({ method: "POST", path: "kuvo/agents/api/kuvo/accounts", headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ provider: "codex-cli" })) }, {}, undefined)) pieces.push(piece);
    assert.equal(pieces[0].head.status, 200);
    assert.deepEqual(JSON.parse(Buffer.concat(pieces.filter(piece => piece.body).map(piece => piece.body)).toString()), { data: { providerID: "codex-cli@1234abcd" } });
    assert.deepEqual(given, { method: "POST", path: "/api/kuvo/accounts", body: { provider: "codex-cli" } });
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});
