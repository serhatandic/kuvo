// Both resident binaries read a private Claude account and speak to the same stand-in CLI.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  claudeEnvironment, headers, heldClaude, keptListener, provider, result, seedSession, startClaudeBridge,
  stopFixtureHolders, stopPids, streams, transcript, twoReleases,
} from "./claude.mjs";
import {
  boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, readJSON, request, scratch,
  startBox, until, write,
} from "./adapter.mjs";

const picker = "kuvo/models/offered?refresh=1";

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

// Claude Code's models as an account lists them: its default (Opus), Opus, Haiku, and Opus with a
// million tokens.
const listedModels = [
  { value: "default", resolvedModel: "claude-opus-5-5", displayName: "Default (recommended)", description: "Opus 5.5 · Most capable" },
  { value: "opus", resolvedModel: "claude-opus-5-5", displayName: "Opus 5.5", description: "Opus 5.5 · Most capable" },
  { value: "claude-haiku-4-5", displayName: "Haiku 4.5", description: "Haiku 4.5 · Fast" },
  { value: "opus[1m]", resolvedModel: "claude-opus-5-5[1m]", displayName: "Opus 5.5 (1M)", description: "Opus 5.5 with 1M context" },
];

/** How much each conversation's model holds, as its Claude Code is started with "never summarize"
 * (only a model of 900,000 tokens or more goes without), once Claude Code has listed its models. */
async function heldBy(listener, env, sessions) {
  await until(async () => {
    const answer = await request(listener, "GET", picker, undefined, headers, 45000);
    return answer.json?.models?.some(model => model.providerID === provider && model.id === "claude-haiku-4-5" && !model.provisional);
  }, "Claude Code never listed its models.", 45000);
  const heard = () => fs.readFileSync(path.join(env.HOME, "claude-heard.jsonl"), "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line));
  const held = {};
  for (const [session, model] of Object.entries(sessions)) {
    const route = `kuvo/agents/api/session/${session}`;
    const sent = await request(listener, "POST", route + "/prompt", { id: `msg_${session.slice(4)}`, text: `go ${session}` }, headers, 45000);
    assert.equal(sent.status, 200, sent.text);
    await until(async () => (await transcript(listener, route)).some(row => row.type === "idle"), `${model}'s turn never ended.`);
    const pid = heard().find(line => line.type === "user" && line.message?.content?.some(part => part.text?.startsWith(`go ${session}`)))?.pid;
    const launched = heard().find(line => line.argv && line.pid === pid);
    held[model] = launched.env.DISABLE_AUTO_COMPACT === "1" ? "never" : launched.env.CLAUDE_CODE_AUTO_COMPACT_WINDOW;
  }
  return held;
}

const windowSessions = { ses_window_opus: "opus", ses_window_haiku: "claude-haiku-4-5", ses_window_million: "opus[1m]" };
const seedWindows = (catalog) => (home, env) => {
  for (const [session, model] of Object.entries(windowSessions)) seedSession(home, session, { model });
  write(path.join(home, "box", "conversation-compaction.json"), JSON.stringify("never"));
  write(path.join(env.HOME, "claude-catalog.json"), JSON.stringify(catalog));
};

test("each model holds what Claude Code says it does on your plan, which its list doesn't say", { timeout: 90000 }, async t => {
  await withClaude(t, { turns: true }, async (listener, _home, env) => {
    assert.deepEqual(await heldBy(listener, env, windowSessions), { opus: "never", "claude-haiku-4-5": "433000", "opus[1m]": "never" });
  }, seedWindows({ models: listedModels, windows: { default: 1_000_000, "claude-haiku-4-5": 200_000, "opus[1m]": 1_000_000 } }));
});

test("what Claude Code said a model holds as a turn ended counts for the next conversation on it", { timeout: 90000 }, async t => {
  await withClaude(t, { turns: true }, async (listener, home, env) => {
    await until(async () => {
      const answer = await request(listener, "GET", picker, undefined, headers, 45000);
      return answer.json?.models?.some(model => model.providerID === provider && model.id === "claude-haiku-4-5" && !model.provisional);
    }, "Claude Code never listed its models.", 45000);
    // The first turn's end says how much Haiku holds; the second's says nothing of it.
    const reply = more => [
      ...streams("$UUID", [{ type: "text", text: "Hi." }], { input: 100, read: 9_000, write: 0, output: 5 }),
      result(["$UUID"], "success", more),
    ];
    write(path.join(env.HOME, "claude-script.json"), JSON.stringify({
      "go ses_window_haiku": reply({ modelUsage: { "claude-haiku-4-5": { contextWindow: 444_000 } } }),
      "go ses_window_haiku_two": reply(),
    }));
    const kept = async session => {
      const route = `kuvo/agents/api/session/${session}`;
      const sent = await request(listener, "POST", route + "/prompt", { id: `msg_${session.slice(4)}`, text: `go ${session}` }, headers, 45000);
      assert.equal(sent.status, 200, sent.text);
      await until(async () => (await transcript(listener, route)).some(row => row.type === "idle"), "The turn never ended.");
      return until(() => {
        const saved = readJSON(path.join(home, "agents/sessions", session, "session.json"));
        return saved.context?.used && saved.context;
      }, "The turn's context was never kept.");
    };
    assert.equal((await kept("ses_window_haiku")).window, 444_000);
    assert.equal((await kept("ses_window_haiku_two")).window, 444_000);
  }, (home, env) => {
    seedWindows({ models: listedModels })(home, env);
    seedSession(home, "ses_window_haiku_two", { model: "claude-haiku-4-5" });
  });
});

test("an older Claude Code that doesn't say how much each holds: as its list says", { timeout: 90000 }, async t => {
  await withClaude(t, { turns: true }, async (listener, _home, env) => {
    assert.deepEqual(await heldBy(listener, env, windowSessions), { opus: "433000", "claude-haiku-4-5": "433000", "opus[1m]": "never" });
  }, seedWindows({ models: listedModels }));
});

test("before Claude Code has listed its models, the usual ones still answer to its default", { timeout: 90000 }, async t => {
  await withClaude(t, {}, async (listener, _home, env) => {
    try {
      const offered = await until(async () => {
        const answer = await request(listener, "GET", "kuvo/models/offered", undefined, headers, 20000);
        const models = answer.json?.models?.filter(model => model.providerID === provider) ?? [];
        return models.length && models;
      }, "Claude Code's usual models were never offered.");
      assert.deepEqual(offered.filter(model => model.aliases?.includes("default")).map(model => model.id), ["opus"]);
      assert.ok(!offered.some(model => model.id === "default" || /default/i.test(model.name)), JSON.stringify(offered));
    } finally {
      write(path.join(env.HOME, "list-goes"), "");
    }
  }, (_home, env) => write(path.join(env.HOME, "claude-catalog.json"), JSON.stringify({ models: listedModels, until: "list-goes" })));
});

test("a session already on its default goes on: Claude Code's own default is what it runs", { timeout: 90000 }, async t => {
  const session = "ses_claude_default";
  const route = `kuvo/agents/api/session/${session}`;
  await withClaude(t, { turns: true }, async (listener, _home, env) => {
    const sent = await request(listener, "POST", route + "/prompt", { id: "msg_on_default", text: "go" }, headers, 45000);
    assert.equal(sent.status, 200, sent.text);
    const rows = await until(async () => {
      const rows = await transcript(listener, route);
      return rows.some(row => row.type === "idle") && rows;
    }, "The turn never ended.");
    assert.equal(rows.find(row => row.type === "idle").outcome, "succeeded");
    const heard = fs.readFileSync(path.join(env.HOME, "claude-heard.jsonl"), "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line));
    const pid = heard.find(line => line.type === "user")?.pid;
    // Never told which model: Claude Code runs its own default.
    assert.ok(!heard.some(line => line.pid === pid && (line.request?.subtype === "set_model" && line.request.model
      || line.argv?.some(arg => arg.startsWith("--model")))), JSON.stringify(heard.filter(line => line.pid === pid)));
    assert.equal(readJSON(path.join(_home, "agents/sessions", session, "session.json")).model.id, "default");
  }, home => seedSession(home, session, { model: "default" }));
});

/** Signs Claude Code in with a pasted code, as you would on a computer you reach remotely. */
async function signInClaude(listener) {
  const started = await request(listener, "POST", oauth, { methodID: "claudeai", remote: true }, headers, 20000);
  assert.equal(started.status, 200, started.text);
  const { attemptID } = started.json.data;
  const done = await request(listener, "POST", `${oauth}/${attemptID}/complete`, { code: "fixture-code" }, headers);
  assert.equal(done.status, 200, done.text);
  await until(async () => (await request(listener, "GET", `${oauth}/${attemptID}`, undefined, headers)).json?.data?.status === "complete",
    "The Claude sign-in did not finish.");
}

test("signed out, the Claude Code a conversation has open goes: its next message starts another", { timeout: 90000 }, async t => {
  const session = "ses_claude_signed_out";
  const route = `kuvo/agents/api/session/${session}`;
  await withClaude(t, { signedIn: false, turns: true }, async (listener, _home, env) => {
    const heard = () => fs.readFileSync(path.join(env.HOME, "claude-heard.jsonl"), "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line));
    const took = text => heard().find(line => line.type === "user" && line.message?.content?.some(part => part.text?.startsWith(text)))?.pid;
    const answered = async (id, text, turns) => {
      const sent = await request(listener, "POST", route + "/prompt", { id, text }, headers, 45000);
      assert.equal(sent.status, 200, sent.text);
      await until(async () => (await transcript(listener, route)).filter(row => row.type === "idle").length === turns,
        "The Claude fixture did not finish its turn.");
    };
    await signInClaude(listener);
    await answered("msg_signed_out_first", "first", 1);
    const out = await request(listener, "POST", "kuvo/agents/api/integration/claude-code/disconnect", {}, headers, 20000);
    assert.equal(out.status, 200, out.text);
    await signInClaude(listener);
    await answered("msg_signed_out_again", "again", 2);
    assert.ok(took("first") && took("again"));
    assert.notEqual(took("again"), took("first"));
    const rows = await transcript(listener, route);
    assert.equal(rows.filter(row => row.type === "idle").at(-1).outcome, "succeeded");
  }, home => seedSession(home, session));
});

test("a new Claude account starts a new probe, and an old probe can't change its models", { timeout: 90000 }, async t => {
  await withClaude(t, { signedIn: false }, async (listener, _home, env) => {
    const offered = async (query = "?refresh=1") => {
      const answer = await request(listener, "GET", `kuvo/models/offered${query}`, undefined, headers, 45000);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.models.filter(model => model.providerID === provider).map(model => model.id).sort();
    };
    const signIn = () => signInClaude(listener);
    const catalog = value => write(path.join(env.HOME, "claude-catalog.json"), JSON.stringify(value));
    const marked = name => fs.existsSync(path.join(env.HOME, name));
    const model = (value, displayName) => ({ value, resolvedModel: `claude-${value}`, displayName });
    try {
      await signIn();
      // The first account's models are asked for, and Claude Code takes its time to say.
      catalog({ models: [model("old-model", "Old model")], until: "old-goes" });
      const old = offered().catch(() => undefined);
      await until(() => marked("claude-catalog-waiting"), "The first account's probe never started.");
      // Meanwhile it signs out, and in as another account, with models of its own.
      const out = await request(listener, "POST", "kuvo/agents/api/integration/claude-code/disconnect", {}, headers, 20000);
      assert.equal(out.status, 200, out.text);
      catalog({ models: [model("new-model", "New model")] });
      await signIn();
      assert.deepEqual(await offered(), ["new-model"]);
      // The old probe answers at last, or was stopped: what it says isn't this account's.
      const probe = Number(fs.readFileSync(path.join(env.HOME, "claude-catalog-waiting"), "utf8"));
      const alive = () => { try { process.kill(probe, 0); return true; } catch { return false; } };
      write(path.join(env.HOME, "old-goes"), "");
      await until(() => marked("claude-catalog-answered") || !alive(), "The first account's probe neither answered nor stopped.");
      await old;
      await new Promise(resolve => setTimeout(resolve, 300));
      assert.deepEqual(await offered(""), ["new-model"]);
    } finally {
      write(path.join(env.HOME, "old-goes"), "");
    }
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

test("a saved Claude session receives a prompt through the resident", { timeout: 60000 }, async t => {
  const session = "ses_claude_fixture";
  const route = `kuvo/agents/api/session/${session}`;
  await withClaude(t, { turns: true }, async (listener, home) => {
    const started = await request(listener, "POST", route + "/prompt", {
      id: "msg_fixture_first", text: "hello",
    }, headers, 45000);
    assert.equal(started.status, 200, started.text);
    const rows = await until(async () => {
      const answer = await request(listener, "GET", route + "/message?order=asc", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      const rows = answer.json.data;
      return rows.at(-1)?.type === "idle" && rows;
    }, "The Claude fixture did not finish its turn.");
    assert.deepEqual(rows.map(row => row.type), ["user", "assistant", "idle"]);
    assert.equal(rows[1].content[0].text, "Claude fixture received hello");
    assert.equal(rows[2].outcome, "succeeded");
    const saved = readJSON(path.join(home, "agents/sessions", session, "agent.json"));
    assert.match((saved.threads?.[provider]?.state ?? saved).claudeSession, /^[a-f0-9-]{36}$/i);
  }, home => seedSession(home, session));
});

test("a saved Claude session in each agent's slot resumes its own conversation", { timeout: 60000 }, async t => {
  const session = "ses_claude_slots";
  const native = "6f1c2a4e-8b3d-4c5a-9e7f-0a1b2c3d4e5f";
  const route = `kuvo/agents/api/session/${session}`;
  const codex = { state: { codexThread: "thread-kept" } };
  await withClaude(t, { turns: true }, async (listener, home) => {
    const started = await request(listener, "POST", route + "/prompt", {
      id: "msg_fixture_resumed", text: "hello again",
    }, headers, 45000);
    assert.equal(started.status, 200, started.text);
    const rows = await until(async () => {
      const rows = await transcript(listener, route);
      return rows.at(-1)?.type === "idle" && rows;
    }, "The Claude fixture did not finish its turn.");
    assert.equal(rows.find(row => row.type === "assistant")?.content[0].text,
      `Claude fixture received hello again in ${native}`);
    const saved = readJSON(path.join(home, "agents/sessions", session, "agent.json"));
    assert.equal(saved.threads[provider].state.claudeSession, native);
    assert.equal(saved.threads[provider].state.claudeStarted, true);
    assert.deepEqual(saved.threads.codex, codex);
  }, home => {
    seedSession(home, session);
    write(path.join(home, "agents/sessions", session, "agent.json"), JSON.stringify({
      threads: { [provider]: { state: { claudeSession: native, claudeStarted: true } }, codex },
    }));
  });
});

test("the Rust keeper keeps an actual Claude adapter turn across a release switch",
  { timeout: 90000 }, async t => {
    const binary = boxBinary();
    if (binary.typescript) {
      t.skip("A Rust-to-Rust release switch uses the Rust holder and resident.");
      return;
    }
    const root = scratch();
    const home = path.join(root, "box");
    const relay = await localRelay();
    const session = "ses_claude_kept";
    const route = `kuvo/agents/api/session/${session}`;
    let keeper, kept;
    const residents = [];
    try {
      privateHome(home);
      const env = { ...claudeEnvironment(binary, home, isolatedEnv(root), { turns: true }), KUVO_SANDBOX: "off" };
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      seedSession(home, session);
      const { old, next, control } = twoReleases(binary, home, env, relay);
      try {
        await control(["select", old]);
        keeper = boxCommand(binary, home, env, relay, ["keep"]);
        const first = await keptListener(home, keeper);
        residents.push(first.pid);
        const started = await request(first, "POST", route + "/prompt", {
          id: "msg_fixture_kept", text: "wait for update",
        }, headers);
        assert.equal(started.status, 200, started.text);
        await until(async () => (await transcript(first, route)).some(row =>
          row.content?.[0]?.text === "Before the switch."
        ), "The fixture never began its turn.");
        const initial = await until(() => heldClaude(home, session), "Claude's holder was never recorded.");
        assert.equal(initial.meta.turns.main.turn, "msg_fixture_kept");
        await control(["select", next]);
        const second = await keptListener(home, keeper, first.pid);
        residents.push(second.pid);
        let last;
        const rows = await until(async () => {
          last = await transcript(second, route);
          return last.at(-1)?.outcome === "succeeded" && last;
        }, () => {
          const heard = path.join(env.HOME, "claude-heard.jsonl");
          return ["The replacement resident never finished the held turn.", JSON.stringify(last?.slice(-3)),
            fs.existsSync(heard) ? fs.readFileSync(heard, "utf8").slice(-3000) : "", keeper.log().slice(-6000)].join("\n");
        }, 30000);
        const after = heldClaude(home, session);
        assert.ok(after);
        assert.deepEqual([after.pid, after.child, after.generation], [initial.pid, initial.child, initial.generation + 1]);
        assert.deepEqual(rows.filter(row => row.type === "assistant").flatMap(row => row.content.map(part => part.text)),
          ["Before the switch.", "After the switch."]);
        kept = after.pid;
      } finally {
        await control(["off"]);
      }
    } finally {
      await keeper?.stop();
      await stopPids(residents);
      await stopFixtureHolders(home);
      await relay.close();
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    }
    assert.ok(kept);
  });

test("a box stopping for good stops the Claude Code it holds, at work or not", { timeout: 90000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript && process.platform === "win32") {
    t.skip("The reference bridge runs on Unix.");
    return;
  }
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  const session = "ses_claude_stopped";
  const route = `kuvo/agents/api/session/${session}`;
  let keeper, resident, bridge, held;
  const residents = [];
  const alive = pid => {
    try { process.kill(pid, 0); return true; }
    catch (error) { return error.code === "EPERM"; }
  };
  try {
    privateHome(home);
    const env = { ...claudeEnvironment(binary, home, isolatedEnv(root), { turns: true }), KUVO_SANDBOX: "off" };
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    seedSession(home, session);
    // The Rust box as a computer runs it, under its keeper (stopped for good: turned off); the
    // reference as its bridge stops on SIGTERM.
    const releases = binary.typescript ? undefined : twoReleases(binary, home, env, relay);
    let listener;
    if (releases) {
      await releases.control(["select", releases.old]);
      keeper = boxCommand(binary, home, env, relay, ["keep"]);
      listener = await keptListener(home, keeper);
      residents.push(listener.pid);
    } else {
      bridge = await startClaudeBridge(binary, home, env);
      resident = startBox({ binary, home, env, relay });
      listener = await resident.ready();
    }
    // A turn at work as the box stops: it says something, then waits (for a file that never comes).
    write(path.join(env.HOME, "claude-script.json"), JSON.stringify({
      [`go ${session}`]: [...streams("$UUID", [{ type: "text", text: "Working." }]), { until: "never" }],
    }));
    const started = await request(listener, "POST", route + "/prompt", { id: "msg_fixture_stopped", text: `go ${session}` }, headers);
    assert.equal(started.status, 200, started.text);
    await until(async () => (await transcript(listener, route)).some(row => row.content?.[0]?.text === "Working."),
      "The fixture never began its turn.");
    held = await until(() => heldClaude(home, session), "Claude's holder was never recorded.");
    assert.ok(alive(held.child));
    if (releases) await releases.control(["off"]);
    else {
      await resident.stop();
      await bridge.stop();
      resident = bridge = undefined;
    }
    await until(() => !alive(held.child) && !alive(held.pid) && !heldClaude(home, session),
      () => `Claude Code (${held.child}) or its holder (${held.pid}) went on after the box stopped.`, 25000);
    // Started again, the turn the stop cut off has ended.
    if (releases) {
      fs.rmSync(path.join(home, "run", "off"), { force: true });
      await keeper.stop();
      keeper = boxCommand(binary, home, env, relay, ["keep"]);
      listener = await keptListener(home, keeper, listener.pid);
      residents.push(listener.pid);
    } else {
      bridge = await startClaudeBridge(binary, home, env);
      resident = startBox({ binary, home, env, relay });
      listener = await resident.ready();
    }
    let rows;
    await until(async () => (rows = await transcript(listener, route)).at(-1)?.type === "idle",
      () => "The cut-off turn never ended: " + JSON.stringify(rows?.slice(-2)), 25000);
    assert.equal(rows.at(-1).outcome, "interrupted", JSON.stringify(rows.at(-1)));
  } finally {
    await resident?.stop();
    await bridge?.stop();
    await keeper?.stop();
    await stopPids(residents);
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
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
    const env = { ...claudeEnvironment(binary, home, isolatedEnv(root), fixture), KUVO_SANDBOX: "off" };
    prepare(home, env);
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
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
}
