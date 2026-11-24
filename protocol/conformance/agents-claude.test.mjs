// Both resident binaries read a private Claude account and speak to the same stand-in CLI.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { claudeEnvironment, startClaudeBridge } from "./claude.mjs";
import {
  boxBinary, isolatedEnv, localRelay, privateHome, readJSON, request, scratch,
  startBox, until, write,
} from "./adapter.mjs";

const picker = "kuvo/models/offered?refresh=1";
const headers = { "x-kuvo-from": "app" };
const provider = "claude-code";

const opus = { value: "opus", resolvedModel: "claude-opus-5-5", displayName: "Opus 5.5" };
const haiku = { value: "haiku", resolvedModel: "claude-haiku-5-5", displayName: "Haiku 5.5" };
const defaultFor = model => ({ ...model, value: "default", displayName: "Default (recommended)" });
const scenarios = [
  ["account default is an alias of the model Claude names", { stands: "sonnet" }, [
    ["opus", "Opus 5.5", []], ["sonnet", "Sonnet 5.5", ["default"]],
  ]],
  ["an otherwise unlisted default has its concrete name and ID", {
    catalog: [defaultFor(haiku), opus],
  }, [
    ["claude-haiku-5-5", "Haiku 5.5", ["default"]], ["opus", "Opus 5.5", []],
  ]],
  ["a signed-out Claude Code does not offer models", { signedIn: false }, []],
];

for (const [name, fixture, expected] of scenarios) {
  test(name, { timeout: 60000 }, async t => {
    await withClaude(t, fixture, async listener => {
      const answer = await request(listener, "GET", picker, undefined, headers, 45000);
      assert.equal(answer.status, 200, answer.text);
      assert.equal(answer.json.known.agents, true, answer.text);
      const models = answer.json.models.filter(model => model.providerID === provider)
        .sort((a, b) => a.id.localeCompare(b.id));
      assert.deepEqual(models.map(model => [model.id, model.name, model.aliases ?? []]), expected);
      assert.ok(models.every(model => !model.provisional));
    });
  });
}

const oauth = "kuvo/agents/api/integration/claude-code/connect/oauth";

test("Claude accepts a pasted sign-in code, then offers that account's models", { timeout: 60000 }, async t => {
  await withClaude(t, { signedIn: false }, async (listener, _home, env) => {
    const started = await request(listener, "POST", oauth, { methodID: "console", remote: true }, headers, 20000);
    assert.equal(started.status, 200, started.text);
    const { attemptID, url, mode, acceptsCode } = started.json.data;
    assert.ok(attemptID);
    assert.equal(url, "https://claude.example.test/sign-in");
    assert.equal(mode, "code");
    assert.equal(acceptsCode, true);
    const bad = await request(listener, "POST", `${oauth}/${attemptID}/complete`, { code: "wrong" }, headers);
    assert.ok(bad.status >= 400, bad.text);
    const done = await request(listener, "POST", `${oauth}/${attemptID}/complete`, { code: "fixture-code" }, headers);
    assert.equal(done.status, 200, done.text);
    await until(async () => {
      const checked = await request(listener, "GET", `${oauth}/${attemptID}`, undefined, headers);
      return checked.json?.data?.status === "complete";
    }, "The Claude sign-in did not finish.");
    assert.equal(fs.readFileSync(path.join(env.HOME, "fixture-signed-in"), "utf8"), "--console");
    const offered = await request(listener, "GET", picker, undefined, headers, 45000);
    assert.equal(offered.status, 200, offered.text);
    assert.ok(offered.json.models.some(model => model.providerID === provider && model.id === "opus"));
  });
});

test("the saved Claude Console account keeps its own directory and model access", { timeout: 60000 }, async t => {
  await withClaude(t, { signedIn: false }, async (listener, home) => {
    const offered = await request(listener, "GET", picker, undefined, headers, 45000);
    assert.equal(offered.status, 200, offered.text);
    const rows = readJSON(path.join(home, "agents", "accounts.json")).filter(row => row.kind === "claude");
    assert.equal(rows.length, 1);
    const [row] = rows;
    assert.equal(row.folder, "claude/console");
    assert.equal(row.label, "Anthropic Console");
    const claude = offered.json.models.filter(model => model.providerID.startsWith(provider));
    assert.ok(claude.length > 0);
    assert.ok(claude.every(model => model.providerID === `${provider}@${row.id}`));
    assert.deepEqual(offered.json.providers.find(entry => entry.id === `${provider}@${row.id}`), {
      id: `${provider}@${row.id}`, label: "Anthropic Console", name: "Claude Code · Anthropic Console",
      account: { agent: "claude", id: row.id, agentName: "Claude Code", name: "Anthropic Console" },
    });
    assert.equal(fs.readFileSync(path.join(home, "agents/claude/console/projects/kept"), "utf8"), "fixture transcript");
  }, home => {
    write(path.join(home, "agents/claude/console/projects/kept"), "fixture transcript");
    write(path.join(home, "agents/claude/console/fixture-signed-in"), "--console");
    // macOS adopts an unindexed folder left by the old Console sign-in.
    if (process.platform !== "darwin") {
      write(path.join(home, "agents/accounts.json"), JSON.stringify([
        { kind: "claude", id: "console", folder: "claude/console", label: "Anthropic Console" },
      ]));
    }
  });
});

test("a saved setup token is replaced only when Claude reports its own sign-in", { timeout: 60000 }, async t => {
  await withClaude(t, { signedIn: false }, async (listener, home, env, restart) => {
    const token = path.join(home, "agents/claude/tokens/default.json");
    const before = await request(listener, "GET", picker, undefined, headers, 45000);
    assert.equal(before.status, 200, before.text);
    assert.ok(before.json.models.some(model => model.providerID === provider));
    assert.ok(fs.existsSync(token));
    write(path.join(env.HOME, "fixture-signed-in"), "--claudeai");
    const after = await request(await restart(), "GET", picker, undefined, headers, 45000);
    assert.equal(after.status, 200, after.text);
    assert.ok(after.json.models.some(model => model.providerID === provider));
    assert.equal(fs.existsSync(token), false);
  }, home => {
    write(path.join(home, "agents/claude/tokens/default.json"), '{"token":"sk-ant-oat01-fixture","made":1}');
  });
});

async function withClaude(t, fixture, check, prepare = () => {}) {
  const binary = boxBinary();
  if (binary.typescript && process.platform === "win32") {
    t.skip("The reference bridge runs on Unix.");
    return;
  }
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  let resident, bridge;
  try {
    privateHome(home);
    const env = claudeEnvironment(binary, home, isolatedEnv(root), fixture);
    prepare(home);
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    const start = async () => {
      bridge = await startClaudeBridge(binary, home, env);
      resident = startBox({ binary, home, env, relay });
      return resident.ready();
    };
    const restart = async () => {
      await resident?.stop();
      await bridge?.stop();
      return start();
    };
    await check(await start(), home, env, restart);
  } catch (error) {
    throw new Error([error.message, resident?.log(), bridge?.log()].filter(Boolean).join("\n"), { cause: error });
  } finally {
    await resident?.stop();
    await bridge?.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
