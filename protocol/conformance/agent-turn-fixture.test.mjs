// A Rust-only stand-in for exercising the resident adapter contract.
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { existsSync, readdirSync } from "node:fs";
import { boxBinary, readJSON, request, until, write } from "./adapter.mjs";
import { withAgentFixture } from "./agent-fixture.mjs";

test("the resident writes and interrupts a turn through its registered stand-in adapter",
  { timeout: 30000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_fixture_turn";
    const route = `kuvo/agents/api/session/${id}`;
    const headers = { "x-kuvo-from": "app" };
    const folder = home => path.join(home, "agents", "sessions", id);
    return withAgentFixture(t, home => {
      write(path.join(folder(home), "session.json"), JSON.stringify({
        id, projectID: "fixture", agent: "build",
        location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
      }));
    }, async (listener, home) => {
      const call = (method, tail, body) => request(listener, method, route + tail, body, headers);
      const conversation = async () => (await call("GET", "/message?order=asc")).json.data;
      const initial = await call("POST", "/prompt", { id: "msg_fixture_first", text: "hello" });
      assert.equal(initial.status, 200, initial.text);
      const first = await until(async () => {
        const rows = await conversation();
        return rows.at(-1)?.type === "idle" && rows;
      }, "The stand-in did not finish its first turn.");
      assert.deepEqual(first.map(row => row.type), ["user", "assistant", "idle"]);
      assert.equal(first[1].content[0].text, "Fixture received hello");
      assert.equal(first[2].outcome, "succeeded");
      assert.equal(readJSON(path.join(folder(home), "agent.json")).threads["codex-cli"].state.nativeID, "fixture-native");
      await until(async () => {
        const answer = await call("POST", "/prompt", { id: "msg_fixture_next", text: "wait for interrupt" });
        assert.ok([200, 409].includes(answer.status), answer.text);
        return answer.status === 200;
      }, "The resident did not release the completed turn.");
      await until(async () => (await conversation()).some(row =>
        row.content?.[0]?.text === "Fixture received wait for interrupt"
      ), "The stand-in did not report the active turn.");
      const interrupted = await call("POST", "/interrupt");
      assert.equal(interrupted.status, 200, interrupted.text);
      assert.equal(interrupted.json.interrupted, true);
      const stopped = await until(async () => {
        const rows = await conversation();
        return rows.at(-1)?.outcome === "interrupted" && rows;
      }, "The stopped turn did not reach the saved conversation.");
      assert.deepEqual(stopped.map(row => row.type), ["user", "assistant", "idle", "user", "assistant", "idle"]);
    });
  });

test("a new Block keeps an outgoing approval from releasing a pending action",
  { timeout: 30000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_fixture_approval";
    const route = `kuvo/agents/api/session/${id}`;
    const headers = { "x-kuvo-from": "app" };
    return withAgentFixture(t, home => {
      write(path.join(home, "agents", "sessions", id, "session.json"), JSON.stringify({
        id, title: "Fixture approval", projectID: "fixture", agent: "build", location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
        permissions: [{ action: "edit", resource: "*", effect: "ask" }],
      }));
    }, async (listener, home) => {
      const call = (method, tail, body) => request(listener, method, route + tail, body, headers);
      const input = await call("POST", "/prompt", { id: "msg_fixture_permission", text: "wait for approval" });
      assert.equal(input.status, 200, input.text);
      const ask = await until(async () => {
        const answer = await call("GET", "/permission");
        assert.equal(answer.status, 200, answer.text);
        return answer.json.data[0];
      }, "The stand-in did not reach its permission ask.");
      assert.equal(ask.action, "edit");
      assert.deepEqual(ask.resources, ["fixture.txt"]);
      const active = await until(async () => {
        const answer = await request(listener, "GET", "kuvo/sessions/list", undefined, headers);
        const session = answer.json?.sessions?.[id];
        return session?.running && session;
      }, "The current agent turn did not reach the shared session list.");
      assert.equal(active.title, "Fixture approval");
      write(path.join(home, "box", "opencode.json"), JSON.stringify({
        policies: [{ action: "tool.use", resource: "edit:fixture.txt", effect: "deny" }],
      }));
      const reply = `/permission/${ask.id}/reply`;
      const denied = await call("POST", reply, { decision: "once" });
      assert.equal(denied.status, 403, denied.text);
      assert.equal((await call("GET", "/permission")).json.data[0].id, ask.id);
      const rejected = await call("POST", reply, { decision: "reject" });
      assert.equal(rejected.status, 200, rejected.text);
      const saved = await until(async () => {
        const answer = await call("GET", "/message?order=asc");
        const rows = answer.json.data;
        return rows.at(-1)?.type === "idle" && rows;
      }, "The turn did not settle after refusal.");
      assert.equal(saved[1].content[0].text, "Fixture received wait for approval; refused");
      assert.deepEqual((await call("GET", "/permission")).json.data, []);
      assert.equal((await call("POST", reply, { decision: "once" })).status, 404);
    });
  });

test("the shared send, queue, Stop and Resume routes work on a coding agent's conversation",
  { timeout: 45000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_fixture_queue";
    const headers = { "x-kuvo-from": "app" };
    return withAgentFixture(t, home => {
      write(path.join(home, "agents", "sessions", id, "session.json"), JSON.stringify({
        id, projectID: "fixture", agent: "build", location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
      }));
    }, async listener => {
      const call = (method, route, body) => request(listener, method, `kuvo/sessions/${id}${route}`, body, headers);
      const rows = async () => (await request(listener, "GET",
        `kuvo/agents/api/session/${id}/message?order=asc`, undefined, headers)).json.data;
      const sent = await call("POST", "/send", { id: "msg_queueFirst", words: "wait for interrupt" });
      assert.equal(sent.status, 200, sent.text);
      await until(async () => (await rows()).some(row =>
        row.content?.[0]?.text === "Fixture received wait for interrupt"), "The first turn didn't start.");
      const queued = await call("POST", "/send", { id: "msg_queueSecond", words: "after it", delivery: "queue" });
      assert.equal(queued.status, 200, queued.text);
      const line = await until(async () => {
        const answer = await call("GET", "/queue?items=1");
        assert.equal(answer.status, 200, answer.text);
        return answer.json.items?.length && answer.json;
      }, "The queued message didn't show in the queue.");
      assert.deepEqual(line.items.map(item => item.id), ["msg_queueSecond"]);
      const stopped = await call("POST", "/stop", {});
      assert.equal(stopped.status, 200, stopped.text);
      await until(async () => (await rows()).at(-1)?.outcome === "interrupted", "Stop didn't end the turn.");
      const held = await until(async () => {
        const answer = (await call("GET", "/queue?items=1")).json;
        return answer.paused && answer;
      }, "Stop didn't hold the queue.");
      assert.deepEqual(held.items.map(item => item.id), ["msg_queueSecond"]);
      assert.equal((await rows()).filter(row => row.type === "user").length, 1);
      const resumed = await call("POST", "/queue/resume", {});
      assert.equal(resumed.status, 200, resumed.text);
      const after = await until(async () => {
        const saved = await rows();
        return saved.at(-1)?.type === "idle" && saved.at(-1)?.outcome === "succeeded" && saved;
      }, "Resume didn't send the held message.");
      assert.deepEqual(after.filter(row => row.type === "user").map(row => row.id),
        ["msg_queueFirst", "msg_queueSecond"]);
      assert.equal(after.at(-2).content[0].text, "Fixture received after it");
      // (A read answers what the last one saw, and looks again.)
      const empty = await until(async () => {
        const answer = (await call("GET", "/queue?items=1")).json;
        return answer.items?.length === 0 && answer;
      }, "The sent message stayed in the queue.");
      assert.equal(empty.paused, false);
    });
  });

test("a resident replaced while its agent asks: the next shows the ask once, refuses stale answers, and sends what waited once",
  { timeout: 60000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_held_update";
    const route = `kuvo/agents/api/session/${id}`;
    const headers = { "x-kuvo-from": "app" };
    return withAgentFixture(t, home => {
      write(path.join(home, "agents", "sessions", id, "session.json"), JSON.stringify({
        id, projectID: "fixture", agent: "build", location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
        permissions: [{ action: "edit", resource: "*", effect: "ask" }],
      }));
    }, async (first, home, { restart }) => {
      let listener = first;
      const call = (method, tail, body) => request(listener, method, route + tail, body, headers);
      const rows = async () => (await call("GET", "/message?order=asc")).json.data;
      const said = all => all.filter(row => row.type === "assistant").map(row => row.content.map(part => part.text ?? "").join(""));
      assert.equal((await call("POST", "/prompt", { id: "msg_held_update_1", text: "edit" })).status, 200);
      const asked = await until(async () => (await call("GET", "/permission")).json.data[0],
        "The held stand-in did not ask.");
      assert.deepEqual(asked.resources, ["held.txt"]);
      // On a computer the tools relay listens from the start, and the next
      // resident listens where it was, for the agents it takes up.
      const relayRecord = path.join(home, "run", "agents", "relay.json");
      const relayPort = await until(() => existsSync(relayRecord) && readJSON(relayRecord).port,
        "The tools relay never listened.");
      const queued = await call("POST", "/prompt", { id: "msg_held_update_2", text: "next", delivery: "queue" });
      assert.equal(queued.status, 200, queued.text);
      // The update: its keeper switches releases, this resident makes way and
      // the next takes the turn up. (Its agent's lines come a second late: the
      // first read waits for the ask.)
      write(path.join(home, "run", "agent-fixture-slow-replay"), "");
      listener = await restart();
      await until(() => readJSON(relayRecord).port === relayPort, "The tools relay moved.");
      const shown = (await call("GET", "/permission")).json.data;
      assert.deepEqual(shown.map(ask => ask.id), [asked.id]);
      assert.equal((await call("POST", "/permission/per_00_00/reply", { decision: "once" })).status, 404);
      const answered = await call("POST", `/permission/${asked.id}/reply`, { decision: "once" });
      assert.equal(answered.status, 200, answered.text);
      const second = await until(async () => (await call("GET", "/permission")).json.data[0],
        "The message that waited did not start its turn after the one taken up.");
      assert.notEqual(second.id, asked.id);
      assert.equal((await call("POST", `/permission/${second.id}/reply`, { decision: "reject" })).status, 200);
      const done = await until(async () => {
        const all = await rows();
        return all.filter(row => row.type === "idle").length === 2 && all;
      }, "The two turns did not end.");
      assert.deepEqual(done.map(row => row.type), ["user", "assistant", "idle", "user", "assistant", "idle"]);
      assert.deepEqual(said(done), ["Asked to edit; approved", "Asked to next; refused"]);
      assert.deepEqual(done.filter(row => row.type === "idle").map(row => row.outcome), ["succeeded", "succeeded"]);
      assert.deepEqual(readJSON(path.join(home, "agents", "sessions", id, "agent.json")).inbox, []);
      assert.equal((await call("POST", `/permission/${asked.id}/reply`, { decision: "once" })).status, 404);
      // Both turns' held agents end with them: their holders leave nothing behind.
      const holders = path.join(home, "run", "agents", "hold");
      const left = () => existsSync(holders) ? readdirSync(holders).filter(name => name !== "lifecycle.log") : [];
      await until(() => left().length === 0, () => "Holders were left: " + left().join(", "), 30000);
    }, { kept: true });
  });

test("a Mac's held agent in the sandbox reaches Kuvo Code's tools through the box's forwarder, and still does after the resident is replaced",
  { timeout: 60000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_held_tools";
    const route = `kuvo/agents/api/session/${id}`;
    const headers = { "x-kuvo-from": "app" };
    // Kuvo Code's tools as the engine's settings name them: a program that
    // says whose call it answered.
    const tools = `require("readline").createInterface({ input: process.stdin }).on("line", line => {
      const m = JSON.parse(line);
      if (m.id === undefined) return;
      const said = m.method === "tools/call" ? { content: [{ type: "text", text: "for " + m.params._meta["ai.opencode/sessionID"] }] }
        : { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "tools", version: "1" } };
      process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: said }) + "\\n");
    });`;
    const settings = { mcp: { servers: { "kuvo-browser": { command: [process.execPath, "-e", tools] } } } };
    return withAgentFixture(t, home => {
      write(path.join(home, "box", "kind"), "mac\n");
      write(path.join(home, "agents", "sessions", id, "session.json"), JSON.stringify({
        id, projectID: "fixture", agent: "build", location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
        permissions: [{ action: "edit", resource: "*", effect: "ask" }],
      }));
    }, async (first, home, { restart }) => {
      let listener = first;
      const call = (method, tail, body) => request(listener, method, route + tail, body, headers);
      const said = async () => (await call("GET", "/message?order=asc")).json.data
        .filter(row => row.type === "assistant").map(row => row.content.map(part => part.text ?? "").join(""));
      assert.equal((await call("POST", "/prompt", { id: "msg_held_tools_1", text: "edit" })).status, 200);
      const asked = await until(async () => (await call("GET", "/permission")).json.data[0],
        "The held stand-in did not ask.");
      listener = await restart();
      const again = await until(async () => (await call("GET", "/permission")).json.data[0],
        "The ask did not come again.");
      assert.equal(again.id, asked.id);
      assert.equal((await call("POST", `/permission/${asked.id}/reply`, { decision: "once" })).status, 200);
      const done = await until(async () => {
        const all = (await call("GET", "/message?order=asc")).json.data;
        return all.some(row => row.type === "idle") && all;
      }, "The turn did not end.");
      assert.deepEqual(done.filter(row => row.type === "idle").map(row => row.outcome), ["succeeded"]);
      assert.deepEqual(await said(), [`Asked to edit; tools said for ${id}; approved; then for ${id}`]);
      const holders = path.join(home, "run", "agents", "hold");
      const left = () => existsSync(holders) ? readdirSync(holders).filter(name => name !== "lifecycle.log") : [];
      await until(() => left().length === 0, () => "Holders were left: " + left().join(", "), 30000);
    }, { env: { OPENCODE_CONFIG_CONTENT: JSON.stringify(settings) }, kept: true });
  });

test("a resident stopped for good stops the agents it holds: the next ends their turn as cut off",
  { timeout: 60000 }, t => {
    if (boxBinary().typescript) {
      t.skip("The stand-in implements the Rust adapter contract.");
      return;
    }
    const id = "ses_held_stopped";
    const route = `kuvo/agents/api/session/${id}`;
    const headers = { "x-kuvo-from": "app" };
    return withAgentFixture(t, home => {
      write(path.join(home, "agents", "sessions", id, "session.json"), JSON.stringify({
        id, projectID: "fixture", agent: "build", location: { directory: home },
        model: { providerID: "codex-cli", id: "stand-in" },
        time: { created: 1, updated: 1 },
        permissions: [{ action: "edit", resource: "*", effect: "ask" }],
      }));
    }, async (first, home, { restart }) => {
      let listener = first;
      const call = (method, tail, body) => request(listener, method, route + tail, body, headers);
      assert.equal((await call("POST", "/prompt", { id: "msg_held_stopped_1", text: "edit" })).status, 200);
      await until(async () => (await call("GET", "/permission")).json.data[0], "The held stand-in did not ask.");
      const holders = path.join(home, "run", "agents", "hold");
      const left = () => existsSync(holders) ? readdirSync(holders).filter(name => name !== "lifecycle.log") : [];
      assert.ok(left().length > 0);
      listener = await restart();
      await until(() => left().length === 0, () => "Holders were left: " + left().join(", "), 30000);
      const done = await until(async () => {
        const all = (await call("GET", "/message?order=asc")).json.data;
        return all.some(row => row.type === "idle") && all;
      }, "The cut-off turn was never closed.");
      assert.deepEqual(done.filter(row => row.type === "idle").map(row => row.outcome), ["interrupted"]);
      assert.deepEqual((await call("GET", "/permission")).json.data, []);
    });
  });
