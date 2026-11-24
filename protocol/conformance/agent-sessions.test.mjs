import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { request, write } from "./adapter.mjs";
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
