// What the app shows of OpenCode's models (the picker, a conversation's readiness, Settings'
// keys and Refresh Models), on the pinned OpenCode with labelled stand-in providers. The reference
// reads them from its engine; the Rust box from the OpenCode agent, without starting OpenCode
// just to list them (its first start fills the list). These cases assert only what the app reads.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { STAND_IN, standInModel, standInSettings } from "./opencode-model.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const version = "2.2.6";
const headers = { "x-kuvo-from": "app" };
const binary = boxBinary();
const skip = (!pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode (used in place).")
  || (binary.typescript && process.platform === "win32" && "The reference runs on Unix.");

/** OpenCode's settings: the stand-in, a zero-priced provider, one named only by its id, and a signed-out default. */
function settingsFor(url) {
  const settings = standInSettings(url);
  const at = { baseURL: url, apiKey: "stand-in" };
  settings.provider.freebie = { npm: "@ai-sdk/openai-compatible", name: "Freebie", options: at,
    models: { zero: { name: "Zero", cost: { input: 0, output: 0 } } } };
  settings.provider.gateway = { npm: "@ai-sdk/openai-compatible", options: at,
    models: { local: { name: "Local" }, effort: { name: "Effort", variants: { low: {}, high: {} } } } };
  settings.model = "github-copilot/gpt-5.5";
  return settings;
}

describe("OpenCode's models", { skip }, () => {
  let root, home, work, other, model, relay, box, listener, worker;

  before(async () => {
    root = fs.realpathSync(scratch());
    home = path.join(root, "box");
    work = path.join(root, "work");
    other = path.join(root, "other");
    privateHome(home);
    for (const folder of [work, other]) fs.mkdirSync(folder, { recursive: true });
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    const program = path.join(home, "engine", version, process.platform === "win32" ? "opencode.exe" : "opencode");
    fs.mkdirSync(path.dirname(program), { recursive: true });
    if (process.platform === "win32") fs.copyFileSync(fs.realpathSync(pinned), program);
    else fs.symlinkSync(fs.realpathSync(pinned), program);
    model = await standInModel([]);
    write(path.join(home, "config", "opencode", "opencode.json"), JSON.stringify(settingsFor(model.url)));
    write(path.join(home, "box", "opencode.json"), "{}\n");
    // A project with a provider of its own.
    write(path.join(home, "box", "projects.json"), JSON.stringify({ projects: [{ directory: work, name: "Work" }] }));
    write(path.join(work, "opencode.json"), JSON.stringify({ provider: { workshop: {
      npm: "@ai-sdk/openai-compatible", name: "Workshop", options: { baseURL: model.url, apiKey: "stand-in" }, models: { bench: { name: "Bench" } },
    } } }));
    const env = { ...isolatedEnv(root), KUVO_SANDBOX: "off", OPENCODE_DISABLE_MODELS_FETCH: "1" };
    relay = await localRelay();
    if (binary.typescript) {
      const reserve = net.createServer();
      await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
      const port = reserve.address().port;
      await new Promise(resolve => reserve.close(resolve));
      const password = "scratch-engine";
      const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
      for (const folder of [...Object.values(dirs), path.join(home, "workspace")]) fs.mkdirSync(folder, { recursive: true });
      worker = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
        { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, path.join(home, "workspace"));
      await until(async () => (await request({ port, password }, "GET", "api/session?limit=1").catch(() => null))?.status === 200, "The reference's engine", 30000);
      write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version }));
    }
    box = startBox({ binary, home, env, relay });
    listener = await box.ready();
    // (Its first conversation is what fills OpenCode's list here.)
    const first = await call("POST", "kuvo/sessions", { folder: other, model: STAND_IN });
    assert.equal(first.status, 200, first.text);
    const id = first.json.info?.id ?? first.json.session;
    assert.equal((await call("POST", `kuvo/sessions/${id}/send`, { id: "msg_fillList", words: "Hello.", delivery: "steer" })).status, 200);
    await until(async () => (await offered()).models?.some(each => each.providerID === "freebie"), "OpenCode's list never filled.", 60000, 250);
  });

  after(async () => {
    await box?.stop();
    if (home) await stopFixtureHolders(home);
    await worker?.stop?.();
    await relay?.close();
    await model?.close();
    if (root) fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  });

  const call = (method, route, body, timeout = 30000) => request(listener, method, route, body, headers, timeout);
  const offered = async (query = "") => {
    const read = await call("GET", "kuvo/models/offered" + query);
    assert.equal(read.status, 200, read.text);
    return read.json;
  };
  /** A conversation on `on`, and what the composer is told about its model. */
  async function readiness(on) {
    const started = await call("POST", "kuvo/sessions", { folder: other, model: on });
    assert.equal(started.status, 200, started.text);
    const id = started.json.info?.id ?? started.json.session;
    const read = await until(async () => {
      const answer = await call("GET", `kuvo/sessions/models?sessions=${id}`);
      return answer.status === 200 && answer.json.sessions?.[id] && answer;
    }, () => `Its model's readiness never answered for ${JSON.stringify(on)}.`, 20000, 200);
    const { variant, ...model } = read.json.sessions[id].model ?? {};
    return { ...read.json.sessions[id], model };
  }

  test("the picker labels free models, names each provider, offers efforts and starts where asked", { timeout: 60000 }, async () => {
    const answer = await offered();
    const of = (provider, id) => answer.models.find(each => each.providerID === provider && each.id === id);
    assert.ok(answer.models.filter(each => each.providerID === "opencode").every(each => each.free === true), JSON.stringify(answer.models));
    assert.equal(of("freebie", "zero")?.free, undefined, "Zero pricing isn't free access.");
    assert.deepEqual(answer.providers.filter(each => ["opencode", "freebie", "gateway", "standin"].includes(each.id)), [
      { id: "opencode", name: "OpenCode Console" }, { id: "freebie", name: "Freebie" },
      { id: "gateway", name: "gateway" }, { id: "standin", name: "Stand-in (labelled test model)" },
    ]);
    const efforts = (await offered("?for=defaults")).models.find(each => each.providerID === "gateway" && each.id === "effort");
    assert.deepEqual(efforts?.efforts, ["low", "high"]);
    assert.deepEqual((await offered("?provider=gateway&model=effort&variant=high")).starting, { providerID: "gateway", id: "effort", variant: "high" });
  });

  test("the picker starts on a model that runs when OpenCode's default is signed out", { timeout: 60000 }, async () => {
    const { starting } = await offered();
    assert.ok(starting && starting.providerID !== "github-copilot", JSON.stringify(starting));
    assert.ok((await offered()).models.some(each => each.providerID === starting.providerID && each.id === starting.id));
  });

  test("a project's own models are offered in its pickers only", { timeout: 60000 }, async () => {
    const inProject = await offered("?folder=" + encodeURIComponent(work.replaceAll("\\", "/")));
    assert.ok(inProject.models.some(each => each.providerID === "workshop" && each.id === "bench"), JSON.stringify(inProject.models.map(each => each.providerID)));
    const elsewhere = await offered("?folder=" + encodeURIComponent(other.replaceAll("\\", "/")));
    assert.ok(!elsewhere.models.some(each => each.providerID === "workshop"));
  });

  test("a conversation's model says what it needs: a key, with a plan's hint, or a sign-in", { timeout: 90000 }, async () => {
    assert.deepEqual(await readiness({ providerID: "openai", id: "gpt-5.5" }), {
      model: { providerID: "openai", id: "gpt-5.5" }, runs: false, missing: "key", account: { id: "openai", name: "OpenAI" },
      plan: { name: "ChatGPT", agent: "Codex", provider: "codex-cli" },
      refusal: "gpt-5.5 needs an OpenAI key: add one, or choose another model (with a ChatGPT plan, one under Codex).",
      action: { do: "addKey", label: "Add Key", help: "Add an OpenAI key (with a ChatGPT plan, choose a model under Codex instead)" },
    });
    assert.deepEqual(await readiness({ providerID: "anthropic", id: "claude-opus-4-5" }), {
      model: { providerID: "anthropic", id: "claude-opus-4-5" }, runs: false, missing: "key", account: { id: "anthropic", name: "Anthropic" },
      refusal: "claude-opus-4-5 needs an Anthropic key: add one, or choose another model.",
      action: { do: "addKey", label: "Add Key", help: "Add an Anthropic key" },
    });
    assert.deepEqual(await readiness({ providerID: "github-copilot", id: "gpt-5.5" }), {
      model: { providerID: "github-copilot", id: "gpt-5.5" }, runs: false, missing: "signIn", account: { id: "github-copilot", name: "GitHub Copilot" },
      refusal: "gpt-5.5 needs a sign-in to GitHub Copilot: sign in, or choose another model.",
      action: { do: "signIn", label: "Sign In", help: "Sign in to GitHub Copilot" },
    });
    assert.deepEqual(await readiness({ providerID: "opencode", id: "claude-opus-4-5" }), {
      model: { providerID: "opencode", id: "claude-opus-4-5" }, runs: false, missing: "signIn", account: { id: "opencode", name: "OpenCode Console" },
      refusal: "claude-opus-4-5 needs a sign-in to OpenCode Console: sign in, or choose another model.",
      action: { do: "signIn", label: "Sign In", help: "Sign in to OpenCode Console" },
    });
  });

  test("a model a signed-in provider no longer has is marked gone, and one that runs runs", { timeout: 60000 }, async () => {
    assert.deepEqual(await readiness({ providerID: "gateway", id: "gone-now" }), {
      model: { providerID: "gateway", id: "gone-now" }, runs: false, missing: "gone", account: { id: "gateway", name: "gateway" },
      refusal: "gone-now is no longer among gateway’s models: choose another model.", choose: true,
      action: { do: "choose", label: "Choose a Model", help: "Choose another model" },
    });
    assert.deepEqual(await readiness({ providerID: "freebie", id: "zero" }), { model: { providerID: "freebie", id: "zero" }, name: "Zero", runs: true });
  });

  test("Refresh Models lists what it can refresh and says why it can't refresh the rest", { timeout: 60000 }, async () => {
    const refreshable = await call("GET", "kuvo/accounts/refreshable");
    assert.equal(refreshable.status, 200, refreshable.text);
    for (const id of ["github-copilot", "gitlab", "opencode"]) assert.ok(refreshable.json.providers.includes(id), refreshable.text);
    const byItself = await call("POST", "kuvo/accounts/anthropic/refresh");
    assert.equal(byItself.status, 409, byItself.text);
    assert.match(byItself.json.message, /by itself/);
    assert.equal((await call("GET", "kuvo/accounts/github-copilot/refresh")).status, 405);
    const refreshed = await call("POST", "kuvo/accounts/github-copilot/refresh");
    assert.equal(refreshed.status, 200, refreshed.text);
    assert.deepEqual(refreshed.json, { waiting: [] });
  });

  test("a key added in Settings shows its models without a restart", { timeout: 90000 }, async () => {
    assert.ok(!(await offered()).models.some(each => each.providerID === "anthropic"));
    const added = await call("POST", "kuvo/sign-in/action?integration=anthropic", { action: "addKey", key: "sk-ant-stand-in" });
    assert.ok(added.status < 300, added.text);
    await until(async () => (await offered()).models.some(each => each.providerID === "anthropic"),
      "The key's models never reached the picker.", 30000, 250);
    const ready = await readiness({ providerID: "anthropic", id: "claude-opus-4-5" });
    assert.equal(ready.runs, true, JSON.stringify(ready));
  });
});
