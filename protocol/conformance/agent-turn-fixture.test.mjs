// A Rust-only stand-in for exercising the resident adapter contract.
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
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
