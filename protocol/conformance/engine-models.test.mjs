// Read OpenCode model readiness through a real resident with a local engine fixture.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("a model starred on a member is signed by the main and appears in both pickers", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Stars Main");
  const other = await computer("Stars Member");
  const kept = { providerID: "opencode", id: "already-starred" };
  const newStar = { providerID: "opencode-go", id: "deepseek-v4.1-flash" };
  const route = "kuvo/models/starred";
  const desktop = { "x-kuvo-from": "app" };
  write(path.join(main.home, "box/models.json"), JSON.stringify({ starred: [kept] }));
  const holder = main.connect(await main.start().ready());
  const setup = await holder("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main computer did not take the relay lease.");
  const code = await holder("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(code.status, 201, code.text);
  const joined = await other.join(code.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = other.connect(await other.start().ready());
  const view = await member("GET", "kuvo/setup");
  assert.equal(view.json.you.role, "member", view.text);
  const mainFeatures = (await holder("GET", "kuvo/info")).json.features;
  if (mainFeatures.includes("data.ownership")) {
    const peerBook = path.join(other.home, "run", "peer-agents.json");
    await until(() => {
      try {
        return JSON.parse(fs.readFileSync(peerBook, "utf8"))[setup.json.you.key]?.features?.includes("data.ownership");
      } catch { return false; }
    }, "The member did not receive the main computer's forwarding capability.");
  }
  await until(async () => (await member("GET", route)).json?.starred?.some(star => star.id === kept.id),
    "The member did not receive the shared stars before changing them.");
  const starred = await member("POST", route, { ...newStar, starred: true }, desktop);
  assert.equal(starred.status, 200, starred.text);
  assert.deepEqual(starred.json.starred, [kept, newStar]);
  assert.deepEqual((await holder("GET", route)).json.starred, [kept, newStar]);
  await until(async () => {
    const result = await member("GET", "kuvo/models/offered");
    return result.json?.starred?.some(star => star.id === newStar.id);
  }, "The member's picker did not receive the accepted stars.", 20000, 100);
  const removed = await holder("POST", route, { ...kept, starred: false }, desktop);
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json.starred, [newStar]);
  const followed = await until(async () => {
    const result = await member("GET", route);
    return result.json?.starred?.length === 1 && result.json.starred;
  }, "The member kept a removed star.", 20000, 100);
  assert.deepEqual(followed, [newStar]);

  const copied = { providerID: "opencode-go", id: "historical-copy" };
  const later = { providerID: "opencode", id: "new-local-copy" };
  write(path.join(other.home, "box/models.json"), JSON.stringify({ starred: [copied] }));
  write(path.join(main.home, "box/models.json"), JSON.stringify({ starred: [kept, later] }));
  const received = await until(async () => {
    const result = await member("GET", route);
    return result.json?.starred?.length >= 3 && result.json.starred;
  }, "The main did not merge the computers' own stars.", 40000, 150);
  assert.deepEqual(new Set(received.map(star => star.id)), new Set([newStar.id, copied.id, later.id]));
  assert.deepEqual((await holder("GET", route)).json.starred, received);
});

test("model speeds and standalone stars share one file and survive a restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-preferences");
  const stars = "kuvo/models/starred";
  const speed = "kuvo/models/speed";
  const one = { providerID: "anthropic", id: "one" };
  const two = { providerID: "codex-cli@work", id: "two" };
  // Keep the machine standalone while the SSH enrollment fixture holds the account reservation.
  fs.mkdirSync(path.join(path.dirname(here.home), ".kuvo-code-adding"));
  const file = path.join(here.home, "box", "models.json");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify({ starred: [one, one], speeds: { "codex-cli": "priority" }, other: 42 }));
  const first = here.start();
  const record = await first.ready();
  assert.deepEqual((await request(record, "GET", stars)).json, { starred: [one] });
  assert.deepEqual((await request(record, "POST", stars, { ...two, starred: true })).json, { starred: [one, two] });
  assert.deepEqual((await request(record, "POST", stars, { ...one, starred: true })).json, { starred: [one, two] });
  assert.equal((await request(record, "POST", speed, { providerID: two.providerID, speed: "fast mode" })).status, 400);
  assert.deepEqual((await request(record, "POST", speed, { providerID: two.providerID, speed: "priority" })).json,
    { providerID: two.providerID, speed: "priority" });
  await first.stop();
  const restarted = await here.start().ready();
  assert.deepEqual((await request(restarted, "GET", stars)).json, { starred: [one, two] });
  assert.deepEqual((await request(restarted, "POST", stars, { ...one, starred: false })).json, { starred: [two] });
  assert.deepEqual((await request(restarted, "POST", speed, { providerID: two.providerID, speed: null })).json,
    { providerID: two.providerID, speed: null });
  assert.deepEqual(JSON.parse(fs.readFileSync(file, "utf8")), {
    starred: [two], speeds: { "codex-cli": "priority", [two.providerID]: null }, other: 42,
  });
});

const reference = (providerID, id) => ({ providerID, id });
const session = (here, id, model) => ({ ...sessionOf({ id, directory: here.home }), ...(model && { model }) });

test("the model picker offers models that run and chooses a supported starting model", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-picker");
  const ready = reference("opencode", "one");
  const retired = reference("opencode", "old");
  const signedOut = reference("openai", "signed-out");
  here.engine.providers.push({ id: "opencode" });
  here.engine.models.push(
    { ...ready, name: "One Free", aliases: ["latest"], variants: [{ id: "low" }, { id: "high" }] },
    { ...retired, name: "Old", status: "deprecated", time: { released: 2 } },
    { ...signedOut, name: "Sign in" },
  );
  here.engine.integrations.push(
    { id: "opencode", name: "OpenCode Zen", methods: [{ type: "key" }], connections: [] },
    { id: "openai", name: "OpenAI", methods: [{ type: "key" }], connections: [] },
  );
  here.engine.defaultModel = retired;
  here.engine.sessions.push(session(here, "ses_picker", ready));
  const record = await here.start().ready();
  const route = "kuvo/models/offered";
  const answer = await request(record, "GET", route + "?provider=opencode&model=latest&variant=high");
  assert.equal(answer.status, 200, answer.text);
  const offered = [
    { ...retired, name: "Old", side: "engine", aliases: [], efforts: [], retiring: true },
    { ...ready, name: "One Free", side: "engine", aliases: ["latest"], efforts: ["low", "high"], free: true },
  ];
  assert.deepEqual(answer.json, {
    models: offered, providers: [{ id: "opencode", name: "OpenCode Zen" }], starred: [],
    known: { engine: true, agents: false }, starting: { ...ready, variant: "high" },
  });
  const defaults = await request(record, "GET", route + "?for=defaults");
  assert.equal(defaults.status, 200, defaults.text);
  assert.deepEqual(defaults.json.models, [offered[1]]);
  assert.deepEqual(defaults.json.starting, ready);
  assert.equal(defaults.json.scoped, true);
  const scoped = await request(record, "GET", route + "?session=ses_picker");
  assert.deepEqual(scoped.json.models, offered);
  assert.equal(scoped.json.scoped, true);
  assert.equal(scoped.json.note,
    "This conversation uses provider models. Start a new conversation to use a coding agent’s models.");
});

test("the picker distinguishes free access from zero pricing and supplies an unnamed provider’s id", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-free");
  const stock = [
    { providerID: "github-copilot", id: "mini", name: "Mini", cost: { input: 0, output: 0 } },
    { providerID: "anthropic", id: "opus", name: "Opus", cost: { input: 5, output: 25 } },
    { providerID: "opencode", id: "zen", name: "Zen", cost: { input: 0, output: 0 } },
    { providerID: "private-gateway", id: "local", name: "Local" },
  ];
  here.engine.models.push(...stock);
  here.engine.providers.push(...stock.filter(model => model.providerID !== "opencode").map(model => ({ id: model.providerID })));
  here.engine.integrations.push(
    { id: "anthropic", name: "Anthropic", methods: [{ type: "key" }], connections: [{}] },
    { id: "github-copilot", name: "GitHub Copilot", methods: [{ type: "oauth" }], connections: [{}] },
    { id: "opencode", name: "OpenCode Zen", methods: [{ type: "key" }], connections: [] },
  );
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow model updates");
  const offered = () => request(record, "GET", "kuvo/models/offered");
  const answer = await offered();
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json.models.map(model => [model.providerID, model.id, model.free ?? false]),
    stock.map(model => [model.providerID, model.id, model.providerID === "opencode"]));
  assert.deepEqual(answer.json.providers, [
    { id: "github-copilot", name: "GitHub Copilot" }, { id: "anthropic", name: "Anthropic" },
    { id: "opencode", name: "OpenCode Zen" }, { id: "private-gateway", name: "private-gateway" },
  ]);
  here.engine.models.length = 0;
  here.engine.models.push({ providerID: "unavailable", id: "only", name: "Unavailable" });
  here.engine.integrations.push({ id: "unavailable", methods: [{ type: "key" }], connections: [] });
  here.engine.emit({ type: "model.updated", data: {} });
  let empty;
  await until(async () => {
    empty = await offered();
    return empty.json?.models?.length === 0;
  }, "the catalog without a runnable model to replace the saved models");
  assert.deepEqual(empty.json, { known: { engine: true, agents: false }, models: [],
    providers: [], starred: [], starting: null,
    startChoice: { label: "Choose a Model", refusal: "Choose a model to send it." } });
});

test("catalog reads are reused until a credential change publishes the new picker", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-cache");
  const first = { ...reference("anthropic", "first"), name: "First" };
  const go = { ...reference("opencode-go", "go"), name: "Go" };
  here.engine.providers.push({ id: "anthropic" });
  here.engine.models.push(first, go);
  here.engine.integrations.push(
    { id: "anthropic", name: "Anthropic", methods: [{ type: "key" }], connections: [{}] },
    { id: "opencode-go", name: "OpenCode Go", methods: [{ type: "key" }], connections: [] },
  );
  here.engine.sessions.push({
    ...session(here, "ses_providerBot", first),
    metadata: { kuvo: { bot: "bot_provider" } },
  });
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The box did not subscribe to catalog changes.");
  const read = async (refresh = false) => {
    const answer = await request(record, "GET", "kuvo/models/offered" + (refresh ? "?refresh=1" : ""));
    assert.equal(answer.status, 200, answer.text);
    return answer.json.models.map(model => model.id);
  };
  const reads = () => here.engine.asked.filter(call => call.path === "/api/model").length;
  assert.deepEqual(await read(), ["first"]);
  const firstReads = reads();
  assert.deepEqual(await read(), ["first"]);
  assert.equal(reads(), firstReads);
  assert.deepEqual(await read(true), ["first"]);
  const reopenReads = reads();
  assert.equal(reopenReads, firstReads + 1);
  assert.deepEqual(await read(true), ["first"]);
  assert.equal(reads(), reopenReads);

  here.engine.providers.push({ id: "opencode-go" });
  here.engine.integrations[1].connections.push({});
  here.engine.emit({ type: "credential.updated", data: {} });
  await until(async () => (await read(true)).join() === "first,go", "The refreshed credentials did not reach the picker.");
  const bot = await request(record, "GET", "kuvo/models/offered?session=ses_providerBot");
  assert.equal(bot.status, 200, bot.text);
  assert.equal(bot.json.scoped, true);
  assert.deepEqual(bot.json.models.map(model => model.id), ["first", "go"]);
  assert.equal(bot.json.note, "To use a coding agent in this bot, change its model in Edit Bot.");
  here.engine.providers.pop();
  here.engine.integrations[1].connections.length = 0;
  here.engine.emit({ type: "credential.switched", data: {} });
  await until(async () => (await read()).join() === "first", "The picker kept the signed-out credentials.");
});

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

test("only known folders use a project’s models, and session readiness uses its own folder", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-folders");
  const project = path.join(here.home, "project");
  const worktree = path.join(here.home, "tree");
  const chat = path.join(here.home, "chat");
  const stranger = path.join(here.home, "stranger");
  for (const folder of [project, worktree, chat, stranger]) fs.mkdirSync(folder, { recursive: true });
  const plain = reference("anthropic", "base");
  here.engine.providers.push({ id: plain.providerID });
  here.engine.models.push({ ...plain, name: "Base" });
  here.engine.defaultModel = plain;
  const names = [[project, "project"], [worktree, "worktree"], [chat, "chat"], [stranger, "stranger"]];
  here.engine.catalogs = new Map(names.map(([folder, name]) => {
    const model = reference("own-" + name, "local");
    return [folder, { models: [{ ...model, name }], providers: [{ id: model.providerID }], defaultModel: model }];
  }));
  const projects = (directory, removed = false) => write(path.join(here.home, "box", "projects.json"),
    JSON.stringify({ projects: [{ directory, folders: [], removed }] }));
  projects(project + "/");
  write(path.join(here.home, "box", "worktrees.json"),
    JSON.stringify({ worktrees: [{ project, root: worktree, folder: worktree, branch: "kuvo/fixture" }] }));
  here.engine.sessions.push(sessionOf({ id: "ses_folder", directory: chat }));
  const record = await here.start().ready();
  const picker = folder => request(record, "GET", "kuvo/models/offered" +
    (folder ? "?folder=" + encodeURIComponent(folder.replaceAll("\\", "/")) : ""));
  const assertStarting = async (folder, model) => {
    const reply = await picker(folder);
    assert.equal(reply.status, 200, reply.text);
    assert.deepEqual(reply.json.starting, model, folder);
    assert.equal(reply.json.models.length, 1);
    assert.equal(reply.json.models[0].providerID, model.providerID, folder);
  };
  await assertStarting(project, reference("own-project", "local"));
  await assertStarting(worktree, reference("own-worktree", "local"));
  const ready = await request(record, "GET", "kuvo/sessions/models?sessions=ses_folder");
  assert.equal(ready.status, 200, ready.text);
  assert.deepEqual(ready.json.sessions, { ses_folder: { model: reference("own-chat", "local"), name: "chat", runs: true } });
  await until(async () => (await picker(chat)).json?.starting?.providerID === "own-chat", "the session folder to join the picker");
  for (const folder of [null, stranger, "relative/folder", path.join(here.home, "missing")]) await assertStarting(folder, plain);
  projects(project, true);
  await assertStarting(project, plain);
});

test("session readiness retries a default the engine could not answer", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-default-temporary");
  const model = reference("anthropic", "base");
  here.engine.providers.push({ id: model.providerID });
  here.engine.models.push({ ...model, name: "Base" });
  here.engine.defaultUnavailable = true;
  here.engine.sessions.push(session(here, "ses_default"), session(here, "ses_explicit", model));
  const record = await here.start().ready();
  const picker = await request(record, "GET", "kuvo/models/offered");
  assert.equal(picker.status, 200, picker.text);
  assert.equal(Object.hasOwn(picker.json, "starting"), false);
  assert.equal((await request(record, "GET", "kuvo/sessions/models")).status, 400);
  assert.equal((await request(record, "POST", "kuvo/sessions/models?sessions=ses_explicit", {})).status, 405);
  const read = async () => {
    const reply = await request(record, "GET", "kuvo/sessions/models?sessions=ses_default,ses_explicit");
    assert.equal(reply.status, 200, reply.text);
    return reply.json.sessions;
  };
  const ready = { model, name: "Base", runs: true };
  assert.deepEqual(await read(), { ses_explicit: ready });
  here.engine.defaultUnavailable = false;
  here.engine.defaultModel = model;
  assert.deepEqual(await read(), { ses_default: ready, ses_explicit: ready });
});

test("session readiness distinguishes sign-in, project credentials, and a model turned off", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-credentials");
  const openai = reference("openai", "main");
  const copilot = reference("github-copilot", "base");
  const free = reference("opencode", "free");
  const zenPaid = reference("opencode", "paid");
  const local = reference("ollama", "default");
  const disabled = reference("ollama", "previous");
  here.engine.providers.push({ id: "opencode" });
  here.engine.models.push(
    { ...openai, name: "Main" }, { ...copilot, name: "Base" }, { ...free, name: "Free" },
    { ...local, name: "Local" }, { ...disabled, name: "Previous", enabled: false },
  );
  here.engine.integrations.push(
    { id: "openai", name: "OpenAI", connections: [], methods: [{ type: "key" }] },
    { id: "github-copilot", name: "GitHub Copilot", connections: [], methods: [{ type: "oauth", id: "device" }] },
    { id: "opencode", name: "OpenCode Zen", connections: [], methods: [{ type: "key" }, { type: "oauth", id: "device" }] },
    { id: "ollama", name: "Ollama", methods: [] },
  );
  const entries = [[openai, "key"], [copilot, "signIn"], [free, "free"], [zenPaid, "paid"],
    [local, "local"], [disabled, "disabled"]];
  here.engine.sessions.push(...entries.map(([model, id]) => session(here, "ses_" + id, model)));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow accounts");
  const read = async () => {
    const reply = await request(record, "GET", "kuvo/sessions/models?sessions=" + entries.map(([, id]) => "ses_" + id).join(","));
    assert.equal(reply.status, 200, reply.text);
    return reply.json.sessions;
  };
  const start = await read();
  for (const state of Object.values(start)) {
    assert.doesNotMatch(JSON.stringify({ refusal: state.refusal, action: state.action }), /Settings|›| on your | on Studio/);
  }
  assert.equal(start.ses_key.missing, "key");
  assert.equal(start.ses_key.plan.agent, "Codex");
  assert.deepEqual(start.ses_signIn, {
    model: copilot, name: "Base", runs: false, missing: "signIn",
    account: { id: "github-copilot", name: "GitHub Copilot" },
    refusal: "Base needs a sign-in to GitHub Copilot: sign in, or choose another model.",
    action: { do: "signIn", label: "Sign In", help: "Sign in to GitHub Copilot" },
  });
  assert.equal(start.ses_paid.missing, "signIn");
  assert.deepEqual(start.ses_free, { model: free, name: "Free", runs: true });
  assert.deepEqual(start.ses_local, { model: local, name: "Local", runs: true });
  assert.equal(start.ses_disabled.missing, "gone");
  assert.equal(start.ses_disabled.refusal, "Previous is no longer among Ollama’s models: choose another model.");

  here.engine.providers.push({ id: "openai" });
  here.engine.models.push({ ...zenPaid, name: "Zen Paid" });
  here.engine.emit({ type: "provider.updated", data: {} });
  await until(async () => (await read()).ses_key?.runs, "the engine’s own credentials to reach the picker");
  const next = await read();
  assert.deepEqual(next.ses_key, { model: openai, name: "Main", runs: true });
  assert.deepEqual(next.ses_paid, { model: zenPaid, name: "Zen Paid", runs: true });
  const picker = await request(record, "GET", "kuvo/models/offered?provider=openai&model=main");
  assert.ok(picker.json.models.some(model => model.providerID === "openai" && model.id === "main"));
  assert.deepEqual(picker.json.starting, openai);
});

test("a model removed from a catalog keeps the name the box last knew", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-name");
  const known = reference("local", "one");
  const other = reference("local", "two");
  here.engine.providers.push({ id: "local" });
  here.engine.models.push({ ...known, name: "Model One" }, { ...other, name: "Model Two" });
  here.engine.sessions.push(session(here, "ses_named", known), session(here, "ses_newName", other));
  const first = here.start();
  const route = "kuvo/sessions/models?sessions=ses_named";
  const before = await request(await first.ready(), "GET", route);
  assert.deepEqual(before.json.sessions.ses_named, { model: known, name: "Model One", runs: true });
  const file = path.join(here.home, "box/model-names.json");
  await until(() => fs.existsSync(file), "the name to be retained");
  const kept = [
    ...Array.from({ length: 1999 }, (_, at) => ["other-provider", `id-${at}`, `Older name ${at}`]),
    [known.providerID, known.id, "Model One"],
  ];
  fs.writeFileSync(file, JSON.stringify({ names: kept }));
  const added = await request(await first.ready(), "GET", "kuvo/sessions/models?sessions=ses_newName");
  assert.deepEqual(added.json.sessions.ses_newName, { model: other, name: "Model Two", runs: true });
  const retained = await until(() => {
    const { names } = JSON.parse(fs.readFileSync(file));
    return names.at(-1)?.[2] === "Model Two" && names;
  }, "the name index did not retain the second name", 15000, 50);
  assert.equal(retained.length, 2000);
  assert.deepEqual(retained.slice(0, 2), kept.slice(1, 3));
  await first.stop();
  here.engine.models.shift();
  const after = await request(await here.start().ready(), "GET", route);
  assert.deepEqual(after.json.sessions.ses_named, {
    model: known, runs: false, missing: "gone", choose: true,
    refusal: "Model One is no longer available: choose another model.",
    action: { do: "choose", label: "Choose a Model", help: "Choose another model" },
  });
});

test("provider refresh holds folders with active work and settles them after the work ends", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-refresh");
  const quiet = path.join(here.home, "quiet");
  const terminal = path.join(here.home, "terminal");
  here.engine.open.add(quiet).add(terminal);
  here.engine.terminals.add(terminal);
  for (const id of ["gitlab", "github-copilot"]) {
    here.engine.providers.push({ id });
    here.engine.integrations.push({ id, methods: [], connections: [] });
  }
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the box to follow engine work");
  const refreshable = await request(record, "GET", "kuvo/accounts/refreshable");
  assert.deepEqual(refreshable.json, { providers: ["digitalocean", "github-copilot", "gitlab", "modal", "opencode"] });
  const shared = await request(record, "POST", "kuvo/accounts/github-copilot/refresh");
  assert.equal(shared.status, 409, shared.text);
  assert.match(shared.json.message, /within twenty minutes/);
  assert.deepEqual(here.engine.closed, []);
  const local = await request(record, "POST", "kuvo/accounts/gitlab/refresh");
  assert.equal(local.status, 200, local.text);
  assert.deepEqual(local.json, { waiting: [terminal] });
  assert.deepEqual(here.engine.closed, [quiet]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(here.home, "run/models-due.json"), "utf8")).folders, [terminal]);

  here.engine.terminals.delete(terminal);
  here.engine.emit({ type: "pty.exited", data: {} });
  await until(() => !fs.existsSync(path.join(here.home, "run/models-due.json")), "the deferred refresh to settle");
  assert.deepEqual(here.engine.closed, [quiet, terminal]);
  here.engine.open.add(quiet).add(terminal);
  const refreshed = await request(record, "POST", "kuvo/accounts/github-copilot/refresh");
  assert.equal(refreshed.status, 200, refreshed.text);
  assert.deepEqual(refreshed.json, { waiting: [] });
  assert.deepEqual(here.engine.closed, [quiet, terminal, quiet, terminal]);
});

test("a replacement engine clears an older pending provider refresh without closing a folder", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-refresh-restart");
  const folder = path.join(here.home, "project");
  here.engine.open.add(folder);
  const due = path.join(here.home, "run/models-due.json");
  write(due, JSON.stringify({ engine: `0:${here.engine.record.port}`, folders: [folder] }));
  await here.start().ready();
  await until(() => !fs.existsSync(due), "The replacement engine kept the old refresh.");
  assert.deepEqual([...here.engine.open], [folder]);
  assert.deepEqual(here.engine.closed, []);
});

test("the picker settles a folder that finishes before its deferred GitLab refresh is saved", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-picker-refresh-race");
  const folder = path.join(here.home, "project");
  const id = "ses_finishing";
  here.engine.sessions.push(sessionOf({ id, directory: folder }));
  here.engine.open.add(folder);
  here.engine.providers.push({ id: "gitlab" });
  here.engine.models.push({ providerID: "gitlab", id: "duo", name: "Duo" });
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow engine work");
  here.engine.locationsRead = () => {
    here.engine.locationsRead = null;
    here.engine.running.add(id);
    here.engine.activeRead = () => {
      here.engine.activeRead = null;
      here.engine.running.delete(id);
    };
  };
  const refreshed = await request(record, "GET", "kuvo/models/offered?refresh=1");
  assert.equal(refreshed.status, 200, refreshed.text);
  await until(() => here.engine.closed.includes(folder), "the refresh to close the folder after the last step finished");
  await until(() => !fs.existsSync(path.join(here.home, "run/models-due.json")), "the deferred refresh to clear");
  assert.deepEqual(here.engine.closed, [folder]);
});

test("overlapping picker refreshes share the release of a quiet folder and keep the busy one", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-overlapping-picker");
  const quiet = path.join(here.home, "quiet");
  const busy = path.join(here.home, "busy");
  const id = "ses_busyPicker";
  here.engine.sessions.push(sessionOf({ id, directory: busy }));
  here.engine.running.add(id);
  here.engine.open.add(quiet).add(busy);
  here.engine.providers.push({ id: "gitlab" });
  here.engine.integrations.push({ id: "gitlab", connections: [{}] });
  here.engine.models.push({ providerID: "gitlab", id: "stale", name: "Stale" });
  here.engine.locationClosed = folder => {
    if (folder === quiet) here.engine.models.splice(0, 1, { providerID: "gitlab", id: "updated", name: "Updated" });
  };
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow engine work");
  const open = async () => {
    const answer = await request(record, "GET", "kuvo/models/offered?refresh=1");
    assert.equal(answer.status, 200, answer.text);
    assert.deepEqual(answer.json.models.map(model => model.id), ["updated"]);
  };
  await Promise.all([open(), open()]);
  await open();
  assert.deepEqual(here.engine.closed, [quiet]);
  here.engine.running.delete(id);
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: id } });
  await until(() => here.engine.closed.length > 1, "the deferred folder to close after the session becomes idle");
  assert.deepEqual(here.engine.closed, [quiet, busy]);
});

test("an engine-wide refresh checks new work and locations reopened while it closes them", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-refresh-sweep");
  const one = path.join(here.home, "one");
  const two = path.join(here.home, "two");
  const reopened = path.join(here.home, "reopened");
  here.engine.open.add(one).add(two);
  const session = "ses_refreshStarted";
  here.engine.sessions.push(sessionOf({ id: session, directory: two }));
  here.engine.providers.push({ id: "github-copilot" });
  here.engine.integrations.push({ id: "github-copilot", methods: [], connections: [] });
  const record = await here.start().ready();
  const refresh = () => request(record, "POST", "kuvo/accounts/github-copilot/refresh");
  here.engine.locationClosed = folder => {
    if (folder === one) here.engine.running.add(session);
  };
  const busy = await refresh();
  assert.equal(busy.status, 409, busy.text);
  assert.deepEqual(here.engine.closed, [one]);
  assert.deepEqual([...here.engine.open], [two]);

  here.engine.running.clear();
  here.engine.locationClosed = folder => {
    if (folder === two) here.engine.open.add(reopened);
  };
  const quiet = await refresh();
  assert.equal(quiet.status, 200, quiet.text);
  assert.deepEqual(quiet.json, { waiting: [] });
  assert.deepEqual(here.engine.closed, [one, two, reopened]);
  assert.equal(fs.existsSync(path.join(here.home, "run/models-due.json")), false);
});

test("provider refresh refuses unavailable catalogs and engines without closing a folder", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-model-refresh-errors");
  here.engine.open.add(path.join(here.home, "project"));
  here.engine.providers.push({ id: "github-copilot" });
  here.engine.integrations.push({ id: "github-copilot", methods: [], connections: [] });
  const record = await here.start().ready();
  const refresh = (provider, method = "POST") => request(record, method, `kuvo/accounts/${provider}/refresh`);
  const catalog = await refresh("anthropic");
  assert.equal(catalog.status, 409, catalog.text);
  assert.match(catalog.json.message, /by itself/);
  const missing = await refresh("modal");
  assert.equal(missing.status, 404, missing.text);
  assert.match(missing.json.message, /isn't on this computer's engine/);
  assert.equal((await refresh("github-copilot", "GET")).status, 405);
  here.engine.cantClose = true;
  const cant = await refresh("github-copilot");
  assert.equal(cant.status, 503, cant.text);
  assert.match(cant.json.message, /restart/i);
  assert.deepEqual(here.engine.closed, []);
  assert.equal(fs.existsSync(path.join(here.home, "run/models-due.json")), false);

  const down = await computer("engine-model-refresh-down");
  fs.rmSync(path.join(down.home, "run/engine.json"));
  const unavailable = await request(await down.start().ready(), "POST", "kuvo/accounts/github-copilot/refresh");
  assert.equal(unavailable.status, 503, unavailable.text);
  assert.match(unavailable.json.message, /engine isn't running/i);
});
