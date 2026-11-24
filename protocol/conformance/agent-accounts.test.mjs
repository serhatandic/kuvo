// Account mutations through each real resident binary, with isolated homes and
// no vendor process. The Rust binary is built with agent-runtime-fixture.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import {
  boxBinary, isolatedEnv, launch, localRelay, privateHome, readJSON, request, scratch,
  startBox, typescriptRuntime, until, write,
} from "./adapter.mjs";

const accounts = "kuvo/agents/api/kuvo/accounts";
const headers = { "x-kuvo-from": "app" };

test("agent accounts open a legacy index and keep the original account when another is removed",
  { timeout: 60000 }, async t => {
    const binary = boxBinary();
    if (binary.typescript && process.platform === "win32") {
      t.skip("The reference bridge runs on Unix.");
      return;
    }
    if (!binary.typescript && process.env.KUVO_CONFORMANCE_AGENT_FIXTURE !== "1") {
      t.skip("Use a Rust binary built with agent-runtime-fixture and KUVO_CONFORMANCE_AGENT_FIXTURE=1.");
      return;
    }
    const root = scratch();
    const home = path.join(root, "box");
    const relay = await localRelay();
    let resident, bridge;
    try {
      privateHome(home);
      const env = isolatedEnv(root);
      if (process.platform !== "win32") env.PATH = "/usr/bin:/bin";
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      const index = path.join(home, "agents", "accounts.json");
      const earlier = path.join(home, "agents", "codex", "accounts", "saved");
      mkdirSync(earlier, { recursive: true });
      const original = { kind: "codex", id: "saved", label: "Earlier" };
      write(index, JSON.stringify([original]));
      write(path.join(earlier, "fixture"), "keep this");
      if (binary.typescript) {
        const bundle = fileURLToPath(new URL("../../box/dist/kuvo-agents.js", import.meta.url));
        const port = await freePort();
        bridge = launch(typescriptRuntime(), ["--no-env-file", bundle], {
          ...env, KUVO_HOME: home, KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PORT: String(port),
          KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PASSWORD: "fixture",
          KUVO_SANDBOX: "off", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
        }, home);
        await until(() => existsSync(path.join(home, "run", "bridge.json")),
          () => "The TypeScript bridge never published its fixture listener. " + bridge.log());
      } else {
        write(path.join(home, "run", "agent-fixture-program"), process.execPath);
      }

      resident = startBox({ binary, home, env, relay });
      const listener = await resident.ready();
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
    } finally {
      await resident?.stop();
      await bridge?.stop();
      await relay.close();
      rmSync(root, { recursive: true, force: true });
    }
  });

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
