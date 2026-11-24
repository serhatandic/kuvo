// Account mutations through each real resident binary, with isolated homes and
// no vendor process. The Rust binary is built with agent-runtime-fixture.
import assert from "node:assert/strict";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { readJSON, request, write } from "./adapter.mjs";
import { withAgentFixture } from "./agent-fixture.mjs";

const accounts = "kuvo/agents/api/kuvo/accounts";
const headers = { "x-kuvo-from": "app" };

test("agent accounts open a legacy index and keep the original account when another is removed",
  { timeout: 60000 }, t => {
    const original = { kind: "codex", id: "saved", label: "Earlier" };
    return withAgentFixture(t, home => {
      const index = path.join(home, "agents", "accounts.json");
      const earlier = path.join(home, "agents", "codex", "accounts", "saved");
      mkdirSync(earlier, { recursive: true });
      write(index, JSON.stringify([original]));
      write(path.join(earlier, "fixture"), "keep this");
    }, async (listener, home) => {
      const index = path.join(home, "agents", "accounts.json");
      const earlier = path.join(home, "agents", "codex", "accounts", "saved");
      const log = [];
      async function ask(method, route, body, normalize = value => value) {
        const answer = await request(listener, method, route, body, headers);
        log.push({ status: answer.status, value: normalize(answer.json) });
        return answer;
      }
      const create = await ask("POST", accounts, { provider: "codex-cli@saved", label: " Work " },
        value => ({ data: { providerID: value?.data?.providerID?.replace(/@[a-f\d]{8}$/, "@<new>") } }));
      assert.equal(create.status, 200, create.text);
      assert.match(create.json.data.providerID, /^codex-cli@[a-f\d]{8}$/);
      const id = create.json.data.providerID.split("@")[1];
      const folder = path.join(home, "agents", "codex", "accounts", id);
      assert.ok(existsSync(folder));
      assert.deepEqual(readJSON(index), [{ formatVersion: 1 }, original, { kind: "codex", id, label: "Work" }]);
      const rename = await ask("PATCH", `${accounts}/codex-cli%40saved`, { label: " New label " });
      assert.deepEqual([rename.status, rename.json], [200, {}]);
      const denied = await ask("DELETE", `${accounts}/codex-cli`);
      assert.equal(denied.status, 400);
      const remove = await ask("DELETE", `${accounts}/codex-cli%40${id}`);
      assert.deepEqual([remove.status, remove.json], [200, {}]);
      assert.equal(existsSync(folder), false);
      assert.ok(existsSync(path.join(earlier, "fixture")));
      const kept = readJSON(index);
      assert.deepEqual(kept, [{ formatVersion: 1 }, { ...original, label: "New label" }]);
      log.push({ index: kept, accountRemoved: true, originalFolderRetained: true });
      if (process.env.KUVO_CONFORMANCE_ACCOUNTS_TRANSCRIPT) {
        write(process.env.KUVO_CONFORMANCE_ACCOUNTS_TRANSCRIPT, JSON.stringify(log, null, 2) + "\n", { flag: "wx" });
      }
    });
  });
