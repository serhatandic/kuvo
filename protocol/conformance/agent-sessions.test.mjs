import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { request, scratch, until, write } from "./adapter.mjs";
import { generateCurrent } from "./compat/generate.mjs";
import { withAgentFixture } from "./agent-fixture.mjs";

test("saved agent transcript pages honor order and cursor without reopening the agent",
  { timeout: 60000 }, t => {
    const id = "ses_golden_saved";
    const route = `kuvo/agents/api/session/${id}/message`;
    const trace = process.env.KUVO_CONFORMANCE_SESSION_TRANSCRIPT;
    const folderOf = home => path.join(home, "agents", "sessions", id);
    const messages = [
      { id: "msg_a", type: "user", text: "Start here", time: { created: 10 } },
      { id: "msg_b", type: "assistant", agent: "build", model: { providerID: "codex-cli", id: "stand-in" },
        content: [{ type: "text", text: "An answer" }], time: { created: 11, completed: 12 } },
      { id: "msg_c", type: "idle", outcome: "succeeded", time: { created: 13 } },
    ];
    let original;
    return withAgentFixture(t, home => {
      const folder = folderOf(home);
      mkdirSync(folder, { recursive: true });
      write(path.join(folder, "session.json"), JSON.stringify({
        id, projectID: "golden", location: { directory: home },
        time: { created: 10, updated: 13, activity: 13, idle: 13 }, outcome: "succeeded",
        title: "Golden conversation", cost: 0, tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
      }));
      write(path.join(folder, "messages.json"), "[]");
      const file = path.join(folder, "messages-v1.sqlite");
      const db = new DatabaseSync(file);
      try {
        db.exec(`CREATE TABLE message(position INTEGER PRIMARY KEY, id TEXT, body TEXT NOT NULL, insight TEXT, artifacts TEXT);
          CREATE INDEX message_id ON message(id);`);
        const add = db.prepare("INSERT INTO message(position, id, body) VALUES (?, ?, ?)");
        for (const [index, message] of messages.entries()) add.run(index, message.id, JSON.stringify(message));
      } finally {
        db.close();
      }
      original = readFileSync(file);
    }, async (listener, home) => {
      const log = [];
      for (const [query, data, next] of [
        ["limit=1", [messages[2]], "desc:msg_c"],
        ["cursor=desc%3Amsg_c&limit=2", [messages[1], messages[0]]],
        ["order=asc&limit=2", messages.slice(0, 2), "asc:msg_b"],
        ["cursor=asc%3Amsg_b&limit=2", [messages[2]]],
        ["cursor=desc%3Amsg_gone", []],
      ]) {
        const answer = await request(listener, "GET", `${route}?${query}`, undefined, { "x-kuvo-from": "app" });
        assert.equal(answer.status, 200, answer.text);
        assert.deepEqual(answer.json, { data, cursor: next ? { next } : {} });
        log.push({ status: answer.status, data: answer.json.data, cursor: answer.json.cursor });
      }
      const file = readFileSync(path.join(folderOf(home), "messages-v1.sqlite"));
      assert.deepEqual(file, original);
      assert.equal(readFileSync(path.join(folderOf(home), "messages.json"), "utf8"), "[]");
      if (trace) write(trace, JSON.stringify({ log, database: file.toString("base64") }) + "\n", { flag: "wx" });
    });
  });

test("a saved agent conversation's menu, rename, pin, archive and delete",
  { timeout: 60000 }, t => {
    const id = "ses_golden_actions";
    const folderOf = home => path.join(home, "agents", "sessions", id);
    const trace = process.env.KUVO_CONFORMANCE_SESSION_ACTIONS;
    return withAgentFixture(t, home => {
      const folder = folderOf(home);
      mkdirSync(folder, { recursive: true });
      write(path.join(folder, "session.json"), JSON.stringify({
        id, projectID: "golden", agent: "build", model: { providerID: "codex-cli", id: "stand-in" },
        location: { directory: home }, time: { created: 10, updated: 13, activity: 13, idle: 13 },
        outcome: "succeeded", title: "Golden conversation", cost: 0,
        tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
        metadata: { kuvoAgent: "codex", kuvo: { messages: "kept", approval: "askFirst" } },
      }));
      write(path.join(folder, "messages.json"), "[]");
    }, async (listener, home) => {
      const app = { "x-kuvo-from": "app" };
      const log = [];
      const call = async (method, route, body) => {
        let answer;
        for (let tries = 0; tries < 50; tries++) {
          answer = await request(listener, method, route, body, app);
          if (answer.status !== 503) break;
          await new Promise(resolve => setTimeout(resolve, 200));
        }
        log.push({ method, route, status: answer.status, json: answer.json ?? answer.text });
        return answer;
      };
      const saved = () => JSON.parse(readFileSync(path.join(folderOf(home), "session.json"), "utf8"));
      await call("GET", `kuvo/sessions/${id}/actions`);
      await call("POST", `kuvo/sessions/${id}/rename`, { title: "  Renamed  " });
      await call("POST", `kuvo/sessions/${id}/pin`, { pinned: true });
      await call("POST", `kuvo/sessions/${id}/archive`, { archived: true, was: { updated: 1, archived: false } });
      const before = saved();
      await call("POST", `kuvo/sessions/${id}/archive`, { archived: true, was: { updated: before.time.updated, archived: false } });
      await call("GET", `kuvo/sessions/${id}/actions`);
      await call("POST", `kuvo/sessions/${id}/archive`, { archived: false });
      const kept = saved();
      await call("DELETE", `kuvo/sessions/${id}`, {});
      log.push({ kept });
      assert.deepEqual(log.slice(0, -1).map(entry => entry.status), [200, 200, 200, 409, 200, 200, 200, 200]);
      assert.deepEqual(log[0].json.actions.map(action => action.id), ["pin", "rename", "archive", "delete"]);
      assert.deepEqual(log[0].json.actions[3].confirm,
        { title: "Delete “Golden conversation”?", message: "This can’t be undone.", label: "Delete" });
      assert.equal(log[1].json.info.title, "Renamed");
      assert.equal(log[1].json.info.summarize, undefined);
      assert.equal(log[2].json.info.summarize.value, "400000");
      assert.deepEqual(log[5].json.actions.map(action => action.id), ["unarchive", "delete"]);
      // The bridge's own notes survive every metadata replacement.
      assert.deepEqual(kept.metadata, { kuvoAgent: "codex", kuvo: { messages: "kept", approval: "askFirst" },
        "kuvo.pinned": true, "kuvo.archived": false });
      assert.equal(kept.title, "Renamed");
      assert.equal(existsSync(folderOf(home)), false);
      if (trace) write(trace, JSON.stringify(log) + "\n", { flag: "wx" });
    });
  });

test("starting a coding agent's conversation saves it with the agent's notes and its task",
  { timeout: 60000 }, t => {
    const trace = process.env.KUVO_CONFORMANCE_AGENT_START;
    return withAgentFixture(t, home => {
      mkdirSync(path.join(home, "work"), { recursive: true });
    }, async (listener, home) => {
      const app = { "x-kuvo-from": "app" };
      const folder = path.join(home, "work");
      const model = { providerID: "codex-cli", id: "stand-in" };
      const start = async body => {
        let answer;
        for (let tries = 0; tries < 50; tries++) {
          answer = await request(listener, "POST", "kuvo/sessions", body, app);
          if (answer.status !== 503) break;
          await new Promise(resolve => setTimeout(resolve, 200));
        }
        return answer;
      };
      const saved = id => JSON.parse(readFileSync(path.join(home, "agents", "sessions", id, "session.json"), "utf8"));
      const made = await start({ folder, model, title: "Agent start", approval: "askFirst" });
      assert.equal(made.status, 200, made.text);
      const { session, info } = made.json;
      assert.match(session, /^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$/);
      assert.deepEqual({ ...made.json, session: "<id>", info: undefined },
        { session: "<id>", title: "Agent start", folder, project: "work", info: undefined });
      const header = saved(session);
      assert.equal(header.agent, "build");
      assert.deepEqual(header.model, model);
      assert.equal(header.title, "Agent start");
      assert.equal(header.metadata.kuvoAgent, "codex");
      assert.deepEqual(header.metadata.kuvo, { approval: "askFirst", applied: "askFirst" });
      assert.equal(info.id, session);
      assert.equal(info.summarize.value, "400000");

      const tasked = await start({ folder, model, task: "Say hi", approval: "askFirst" });
      assert.equal(tasked.status, 200, tasked.text);
      const id = tasked.json.session;
      let user;
      await until(async () => {
        const page = await request(listener, "GET", `kuvo/agents/api/session/${id}/message?order=asc&limit=5`, undefined, app);
        user = page.json?.data?.find(message => message.type === "user");
        return Boolean(user);
      }, "the task never reached the conversation");
      assert.equal(user.text, "Say hi");
      assert.deepEqual(user.metadata, { kuvo: { parts: { words: "Say hi" } } });
      const log = {
        made: { ...made.json, session: "<id>", folder: "<folder>",
          info: { ...info, id: "<id>", time: "<time>", location: "<home>", projectID: "<project>" } },
        header: { ...header, id: "<id>", time: "<time>", location: "<home>", projectID: "<project>" },
        tasked: { ...tasked.json, session: "<id>", folder: "<folder>" },
        user: { ...user, id: "<id>", time: "<time>" },
      };
      if (trace) write(trace, JSON.stringify(log) + "\n", { flag: "wx" });
    });
  });

test("the current beta's coding-agent conversation, its waiting message and account open the same",
  { timeout: 90000 }, async t => {
    const golden = scratch();
    t.after(() => rmSync(golden, { recursive: true, force: true }));
    let source;
    try {
      source = generateCurrent(path.join(golden, "current")).home;
    } catch (error) {
      t.skip("The golden home is written by the TypeScript reference bundle with Bun: " + error.message);
      return;
    }
    const trace = process.env.KUVO_CONFORMANCE_AGENT_GOLDEN;
    const id = "ses_golden";
    const app = { "x-kuvo-from": "app" };
    return withAgentFixture(t, home => {
      for (const part of ["sessions/ses_golden", "accounts.json", "codex"]) {
        cpSync(path.join(source, "agents", part), path.join(home, "agents", part), { recursive: true });
      }
    }, async (listener, home) => {
      const get = async route => {
        const answer = await request(listener, "GET", route, undefined, app);
        assert.equal(answer.status, 200, route + ": " + answer.text);
        return answer.json;
      };
      // Its turn was left open: the resident closes it as one a restart cut short.
      const messages = await until(async () => {
        const rows = (await get(`kuvo/agents/api/session/${id}/message?order=asc`)).data;
        return rows.at(-1)?.type === "idle" && rows;
      }, "The golden conversation's open turn was not closed.");
      assert.deepEqual(messages.map(row => [row.type, row.id === "msg_golden" ? row.id : "new"]),
        [["user", "msg_golden"], ["idle", "new"]]);
      assert.equal(messages[0].text, "Keep this conversation.");
      assert.equal(messages[1].outcome, "interrupted");
      assert.equal(messages[1].reason, "restart");
      const queue = await get(`kuvo/sessions/${id}/queue`);
      assert.deepEqual(queue, { paused: false, held: [] });
      // The account index is kept as the TypeScript box wrote it.
      const accounts = readFileSync(path.join(home, "agents", "accounts.json"), "utf8");
      assert.equal(accounts, readFileSync(path.join(source, "agents", "accounts.json"), "utf8"));
      const folder = path.join(home, "agents", "sessions", id);
      const agent = JSON.parse(readFileSync(path.join(folder, "agent.json"), "utf8"));
      assert.equal(agent.threads["codex-cli"].state.codexThread, "thr_golden");
      assert.deepEqual(agent.inbox.map(input => input.id), ["msg_goldeninbox"]);
      const header = JSON.parse(readFileSync(path.join(folder, "session.json"), "utf8"));
      assert.equal(header.outcome, "interrupted");
      if (trace) {
        // (Generated per run: the account's ID and the scratch folder. The
        // bridge also adds an empty Undo record as it closes the turn; Undo is 4.6.)
        const { snapshots, ...kept } = agent;
        assert.deepEqual(snapshots ?? {}, {});
        const log = {
          messages: messages.map(row => row.type === "idle" ? { ...row, id: "<idle>" } : row),
          queue, agent: kept,
          accounts: JSON.parse(accounts).map(account => account.id ? { ...account, id: "<id>" } : account),
          header: { ...header, location: { directory: "<project>" } },
        };
        write(trace, JSON.stringify(log) + "\n", { flag: "wx" });
      }
    });
  });
